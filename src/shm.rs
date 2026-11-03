//! Shared memory between the resident process (tray, sampler, log) and the
//! window process. The resident writes the whole Sample history once per
//! second under a seqlock; the window reads a consistent copy.
//!
//! Splitting the window into its own process keeps the resident footprint at
//! a few MB: the GPU context the window needs costs 50 MB or more, and winit
//! cannot create a second event loop in one process once the first is gone.

use crate::metrics::{PageFile, ProcessCommit, SystemMemory};
use crate::sample::Sample;
use anyhow::{Context, Result, bail};
use chrono::{Local, TimeZone};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering, fence};
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS,
    MapViewOfFile, OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows::core::w;

const NAME: windows::core::PCWSTR = w!("Local\\CommitWatch.state");
const MAGIC: u32 = 0x4357_5348; // "CWSH"
const VERSION: u32 = 1;
pub const CAPACITY: usize = 600;
pub const MAX_LOGICAL: usize = 64;
pub const MAX_PROCS: usize = 32;
const NAME_CHARS: usize = 64;

#[repr(C)]
struct Header {
    magic: u32,
    version: u32,
    seq: AtomicU32,
    len: u32,
    procs_len: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SampleWire {
    at_ms: i64,
    commit_charge: u64,
    commit_limit: u64,
    physical_total: u64,
    physical_available: u64,
    free: u64,
    standby: u64,
    modified: u64,
    page_file_in_use: u64,
    page_file_total: u64,
    kernel_commit: u64,
    vm_memory: u64,
    cpu_total: f32,
    thermal_zone_c: f32, // NaN = none
    package_temp_c: f32, // NaN = none
    temp_margin_c: f32,  // NaN = none
    page_file_automatic: u8,
    logical_count: u8,
    clock_count: u8,
    _pad: u8,
    cpu_per_logical: [f32; MAX_LOGICAL],
    clock_mhz: [f32; MAX_LOGICAL],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ProcWire {
    pid: u32,
    name_len: u32,
    commit: u64,
    working_set: u64,
    name: [u16; NAME_CHARS],
}

const SIZE: usize =
    size_of::<Header>() + CAPACITY * size_of::<SampleWire>() + MAX_PROCS * size_of::<ProcWire>();

struct Mapping {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl Mapping {
    fn header(&self) -> &Header {
        unsafe { &*(self.view.Value as *const Header) }
    }
    fn samples_ptr(&self) -> *mut SampleWire {
        unsafe { (self.view.Value as *mut u8).add(size_of::<Header>()) as *mut SampleWire }
    }
    fn procs_ptr(&self) -> *mut ProcWire {
        unsafe {
            (self.view.Value as *mut u8)
                .add(size_of::<Header>() + CAPACITY * size_of::<SampleWire>())
                as *mut ProcWire
        }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

unsafe impl Send for Mapping {}

pub struct Writer {
    map: Mapping,
}

impl Writer {
    pub fn create() -> Result<Self> {
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                SIZE as u32,
                NAME,
            )
        }
        .context("CreateFileMappingW")?;
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, SIZE) };
        if view.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            bail!("MapViewOfFile failed");
        }
        let map = Mapping { handle, view };
        unsafe {
            let h = map.view.Value as *mut Header;
            (*h).magic = MAGIC;
            (*h).version = VERSION;
            (*h).seq = AtomicU32::new(0);
            (*h).len = 0;
            (*h).procs_len = 0;
        }
        Ok(Self { map })
    }

    /// Publishes the history (oldest first) and the latest Top Processes.
    pub fn publish<'a>(
        &mut self,
        history: impl Iterator<Item = &'a Sample>,
        newest: Option<&Sample>,
    ) {
        let h = self.map.header();
        h.seq.fetch_add(1, Ordering::AcqRel); // odd: writing
        fence(Ordering::SeqCst);
        let mut n = 0usize;
        let samples = self.map.samples_ptr();
        for s in history.take(CAPACITY) {
            unsafe { samples.add(n).write(to_wire(s)) };
            n += 1;
        }
        let procs = self.map.procs_ptr();
        let mut pn = 0usize;
        if let Some(s) = newest {
            for p in s.top_processes.iter().take(MAX_PROCS) {
                unsafe { procs.add(pn).write(proc_to_wire(p)) };
                pn += 1;
            }
        }
        unsafe {
            let hm = self.map.view.Value as *mut Header;
            (*hm).len = n as u32;
            (*hm).procs_len = pn as u32;
        }
        fence(Ordering::SeqCst);
        h.seq.fetch_add(1, Ordering::AcqRel); // even: stable
    }
}

pub struct Reader {
    map: Mapping,
}

impl Reader {
    /// `Ok(None)` when no resident process has created the mapping.
    pub fn open() -> Result<Option<Self>> {
        let handle = match unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, NAME) } {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, SIZE) };
        if view.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            bail!("MapViewOfFile failed");
        }
        let map = Mapping { handle, view };
        let h = map.header();
        if h.magic != MAGIC || h.version != VERSION {
            bail!("shared memory version mismatch");
        }
        Ok(Some(Self { map }))
    }

    /// A consistent copy of the history (oldest first). Top Processes are
    /// attached to the newest Sample.
    pub fn read(&self) -> Vec<Sample> {
        for _ in 0..16 {
            let h = self.map.header();
            let s1 = h.seq.load(Ordering::Acquire);
            if s1 % 2 == 1 {
                std::thread::yield_now();
                continue;
            }
            fence(Ordering::SeqCst);
            let len = (h.len as usize).min(CAPACITY);
            let pn = (h.procs_len as usize).min(MAX_PROCS);
            let mut wires = Vec::with_capacity(len);
            for i in 0..len {
                wires.push(unsafe { self.map.samples_ptr().add(i).read() });
            }
            let mut procs = Vec::with_capacity(pn);
            for i in 0..pn {
                procs.push(unsafe { self.map.procs_ptr().add(i).read() });
            }
            fence(Ordering::SeqCst);
            if h.seq.load(Ordering::Acquire) != s1 {
                continue;
            }
            let mut out: Vec<Sample> = wires.iter().map(from_wire).collect();
            if let Some(last) = out.last_mut() {
                last.top_processes = procs.iter().map(proc_from_wire).collect();
            }
            return out;
        }
        Vec::new()
    }
}

fn to_wire(s: &Sample) -> SampleWire {
    let mut w = SampleWire {
        at_ms: s.at.timestamp_millis(),
        commit_charge: s.memory.commit_charge,
        commit_limit: s.memory.commit_limit,
        physical_total: s.memory.physical_total,
        physical_available: s.memory.physical_available,
        free: s.memory.free,
        standby: s.memory.standby,
        modified: s.memory.modified,
        page_file_in_use: s.memory.page_file_in_use(),
        page_file_total: s.memory.page_file_total(),
        kernel_commit: s.kernel_commit,
        vm_memory: s.vm_memory,
        cpu_total: s.cpu_total as f32,
        thermal_zone_c: s.thermal_zone_c.map_or(f32::NAN, |v| v as f32),
        package_temp_c: s.package_temp_c.map_or(f32::NAN, |v| v as f32),
        temp_margin_c: s.temp_margin_c.map_or(f32::NAN, |v| v as f32),
        page_file_automatic: s.memory.page_file_automatic as u8,
        logical_count: s.cpu_per_logical.len().min(MAX_LOGICAL) as u8,
        clock_count: s.clock_mhz.len().min(MAX_LOGICAL) as u8,
        _pad: 0,
        cpu_per_logical: [0.0; MAX_LOGICAL],
        clock_mhz: [0.0; MAX_LOGICAL],
    };
    for (i, v) in s.cpu_per_logical.iter().take(MAX_LOGICAL).enumerate() {
        w.cpu_per_logical[i] = *v as f32;
    }
    for (i, v) in s.clock_mhz.iter().take(MAX_LOGICAL).enumerate() {
        w.clock_mhz[i] = *v as f32;
    }
    w
}

fn from_wire(w: &SampleWire) -> Sample {
    let n = (w.logical_count as usize).min(MAX_LOGICAL);
    let nc = (w.clock_count as usize).min(MAX_LOGICAL);
    let opt = |v: f32| if v.is_nan() { None } else { Some(v as f64) };
    Sample {
        at: Local
            .timestamp_millis_opt(w.at_ms)
            .single()
            .unwrap_or_else(Local::now),
        memory: SystemMemory {
            commit_charge: w.commit_charge,
            commit_limit: w.commit_limit,
            physical_total: w.physical_total,
            physical_available: w.physical_available,
            free: w.free,
            standby: w.standby,
            modified: w.modified,
            page_files: vec![PageFile {
                path: String::new(),
                total: w.page_file_total,
                in_use: w.page_file_in_use,
                peak: 0,
            }],
            page_file_automatic: w.page_file_automatic != 0,
        },
        kernel_commit: w.kernel_commit,
        vm_memory: w.vm_memory,
        top_processes: Vec::new(),
        cpu_total: w.cpu_total as f64,
        cpu_per_logical: w.cpu_per_logical[..n].iter().map(|v| *v as f64).collect(),
        clock_mhz: w.clock_mhz[..nc].iter().map(|v| *v as f64).collect(),
        thermal_zone_c: opt(w.thermal_zone_c),
        package_temp_c: opt(w.package_temp_c),
        temp_margin_c: opt(w.temp_margin_c),
    }
}

fn proc_to_wire(p: &ProcessCommit) -> ProcWire {
    let mut name = [0u16; NAME_CHARS];
    let mut n = 0;
    for u in p.name.encode_utf16().take(NAME_CHARS) {
        name[n] = u;
        n += 1;
    }
    ProcWire {
        pid: p.pid,
        name_len: n as u32,
        commit: p.commit,
        working_set: p.working_set,
        name,
    }
}

fn proc_from_wire(w: &ProcWire) -> ProcessCommit {
    let n = (w.name_len as usize).min(NAME_CHARS);
    ProcessCommit {
        pid: w.pid,
        name: String::from_utf16_lossy(&w.name[..n]),
        commit: w.commit,
        working_set: w.working_set,
    }
}

#[allow(dead_code)]
fn _assert_void(_: *mut c_void) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minute::tests::sample_at;

    #[test]
    fn round_trip_through_mapping() {
        let mut w = Writer::create().unwrap();
        let r = Reader::open().unwrap().expect("mapping exists");
        let t0 = Local::now();
        let samples: Vec<Sample> = (0..3)
            .map(|i| {
                sample_at(
                    t0 + chrono::Duration::seconds(i),
                    60 + i as u64,
                    0.1 * i as f64,
                )
            })
            .collect();
        w.publish(samples.iter(), samples.last());
        let got = r.read();
        assert_eq!(got.len(), 3);
        assert_eq!(got[2].memory.commit_charge, 62);
        assert_eq!(
            got[2].at.timestamp_millis(),
            samples[2].at.timestamp_millis()
        );
        assert_eq!(got[2].top_processes[0].name, "a.exe");
        assert_eq!(got[2].top_processes[0].commit, 31);
        assert!(got[1].top_processes.is_empty());
        assert_eq!(got[0].clock_mhz, vec![4000.0, 3000.0]);
        assert_eq!(got[0].thermal_zone_c, Some(30.0));
        assert_eq!(got[0].package_temp_c, None);
    }

    #[test]
    fn open_without_writer_is_none() {
        // Another test may hold a Writer; only assert the call does not error.
        assert!(Reader::open().is_ok());
    }
}
