//! Memory metrics collection for Windows.
//!
//! Terminology follows GLOSSARY.md: Commit Charge, Commit Limit, Available /
//! Free / Standby Memory, Page File, Process Commit, VM Memory.

use anyhow::{Context, Result, anyhow, bail};
use std::ffi::c_void;
use windows::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS, SystemProcessInformation,
};
use windows::Win32::Foundation::{STATUS_INFO_LENGTH_MISMATCH, STATUS_SUCCESS, UNICODE_STRING};
use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_MULTI_SZ, RegGetValueW};
use windows::Win32::System::WindowsProgramming::SYSTEM_PROCESS_INFORMATION;
use windows::core::w;

/// Undocumented but stable since Windows 2000. Used by Process Explorer and RAMMap.
const SYSTEM_MEMORY_LIST_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(80);
const SYSTEM_PAGEFILE_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(18);

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct SystemMemoryListInformation {
    zero_page_count: usize,
    free_page_count: usize,
    modified_page_count: usize,
    modified_no_write_page_count: usize,
    bad_page_count: usize,
    page_count_by_priority: [usize; 8],
    repurposed_pages_by_priority: [usize; 8],
    modified_page_count_page_file: usize,
}

#[repr(C)]
struct SystemPagefileInformation {
    next_entry_offset: u32,
    total_size: u32,
    total_in_use: u32,
    peak_usage: u32,
    page_file_name: UNICODE_STRING,
}

/// All values in bytes, taken at one instant. See GLOSSARY.md "Sample".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemMemory {
    pub commit_charge: u64,
    pub commit_limit: u64,
    pub physical_total: u64,
    pub physical_available: u64,
    pub free: u64,
    pub standby: u64,
    pub modified: u64,
    pub page_files: Vec<PageFile>,
    pub page_file_automatic: bool,
}

impl SystemMemory {
    /// Commit Usage: Commit Charge / Commit Limit, in 0.0..=1.0.
    pub fn commit_usage(&self) -> f64 {
        if self.commit_limit == 0 {
            0.0
        } else {
            self.commit_charge as f64 / self.commit_limit as f64
        }
    }

    pub fn page_file_total(&self) -> u64 {
        self.page_files.iter().map(|p| p.total).sum()
    }

    pub fn page_file_in_use(&self) -> u64 {
        self.page_files.iter().map(|p| p.in_use).sum()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageFile {
    pub path: String,
    pub total: u64,
    pub in_use: u64,
    pub peak: u64,
}

/// One process's share of Commit Charge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessCommit {
    pub pid: u32,
    pub name: String,
    /// Process Commit in bytes (private committed pages).
    pub commit: u64,
    pub working_set: u64,
}

impl ProcessCommit {
    /// VM Memory: the Hyper-V worker process that backs WSL2 / Docker Desktop.
    pub fn is_vm(&self) -> bool {
        self.name.to_ascii_lowercase().starts_with("vmmem")
    }
}

pub fn system_memory() -> Result<SystemMemory> {
    let perf = performance_info()?;
    let page = perf.PageSize as u64;
    let lists = memory_lists()?;
    let page_files = page_files(page).unwrap_or_default();

    Ok(SystemMemory {
        commit_charge: perf.CommitTotal as u64 * page,
        commit_limit: perf.CommitLimit as u64 * page,
        physical_total: perf.PhysicalTotal as u64 * page,
        physical_available: perf.PhysicalAvailable as u64 * page,
        free: (lists.zero_page_count + lists.free_page_count) as u64 * page,
        standby: lists.page_count_by_priority.iter().sum::<usize>() as u64 * page,
        modified: lists.modified_page_count as u64 * page,
        page_files,
        page_file_automatic: page_file_is_automatic(),
    })
}

fn performance_info() -> Result<PERFORMANCE_INFORMATION> {
    let mut perf = PERFORMANCE_INFORMATION {
        cb: size_of::<PERFORMANCE_INFORMATION>() as u32,
        ..Default::default()
    };
    unsafe { GetPerformanceInfo(&mut perf, perf.cb) }
        .ok()
        .context("GetPerformanceInfo")?;
    Ok(perf)
}

fn memory_lists() -> Result<SystemMemoryListInformation> {
    let mut info = SystemMemoryListInformation::default();
    let mut ret = 0u32;
    let status = unsafe {
        NtQuerySystemInformation(
            SYSTEM_MEMORY_LIST_INFORMATION,
            &mut info as *mut _ as *mut c_void,
            size_of::<SystemMemoryListInformation>() as u32,
            &mut ret,
        )
    };
    if status != STATUS_SUCCESS {
        bail!(
            "NtQuerySystemInformation(SystemMemoryListInformation) = {:#x}",
            status.0
        );
    }
    Ok(info)
}

fn page_files(page: u64) -> Result<Vec<PageFile>> {
    let buf = query_growing(SYSTEM_PAGEFILE_INFORMATION)?;
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + size_of::<SystemPagefileInformation>() <= buf.len() {
        let entry = unsafe { &*(buf.as_ptr().add(offset) as *const SystemPagefileInformation) };
        out.push(PageFile {
            path: unicode_to_string(&entry.page_file_name),
            total: entry.total_size as u64 * page,
            in_use: entry.total_in_use as u64 * page,
            peak: entry.peak_usage as u64 * page,
        });
        if entry.next_entry_offset == 0 {
            break;
        }
        offset += entry.next_entry_offset as usize;
    }
    Ok(out)
}

/// `PagingFiles` holds e.g. `?:\pagefile.sys` (system managed, any drive),
/// `C:\pagefile.sys 0 0` (system managed on C:) or `C:\pagefile.sys 4096 8192` (custom).
fn page_file_is_automatic() -> bool {
    let key = w!(r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management");
    let value = w!("PagingFiles");
    let mut len = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            value,
            RRF_RT_REG_MULTI_SZ,
            None,
            None,
            Some(&mut len),
        )
    };
    if status.is_err() || len == 0 {
        return true;
    }
    let mut buf = vec![0u16; (len as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            value,
            RRF_RT_REG_MULTI_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut len),
        )
    };
    if status.is_err() {
        return true;
    }
    let text = String::from_utf16_lossy(&buf);
    let entries: Vec<&str> = text.split('\0').filter(|s| !s.trim().is_empty()).collect();
    if entries.is_empty() {
        return true;
    }
    entries.iter().all(|e| {
        let parts: Vec<&str> = e.split_whitespace().collect();
        parts.first().is_some_and(|p| p.starts_with("?:"))
            || parts.len() == 1
            || (parts.len() >= 3 && parts[1] == "0" && parts[2] == "0")
    })
}

/// Every process with its Process Commit, from one kernel call. Works without
/// administrator rights and includes protected processes.
pub fn processes() -> Result<Vec<ProcessCommit>> {
    let buf = query_growing(SystemProcessInformation)?;
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + size_of::<SYSTEM_PROCESS_INFORMATION>() <= buf.len() {
        let entry = unsafe { &*(buf.as_ptr().add(offset) as *const SYSTEM_PROCESS_INFORMATION) };
        let pid = entry.UniqueProcessId.0 as usize as u32;
        if pid != 0 {
            let name = unicode_to_string(&entry.ImageName);
            out.push(ProcessCommit {
                pid,
                name: if name.is_empty() {
                    "System".to_string()
                } else {
                    name
                },
                commit: entry.PrivatePageCount as u64,
                working_set: entry.WorkingSetSize as u64,
            });
        }
        if entry.NextEntryOffset == 0 {
            break;
        }
        offset += entry.NextEntryOffset as usize;
    }
    Ok(out)
}

fn query_growing(class: SYSTEM_INFORMATION_CLASS) -> Result<Vec<u8>> {
    let mut buf: Vec<u8> = vec![0; 256 * 1024];
    for _ in 0..8 {
        let mut ret = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                class,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut ret,
            )
        };
        if status == STATUS_SUCCESS {
            buf.truncate(ret as usize);
            return Ok(buf);
        }
        if status == STATUS_INFO_LENGTH_MISMATCH {
            let want = (ret as usize).max(buf.len() * 2);
            buf.resize(want, 0);
            continue;
        }
        bail!("NtQuerySystemInformation({}) = {:#x}", class.0, status.0);
    }
    Err(anyhow!(
        "NtQuerySystemInformation({}) buffer never large enough",
        class.0
    ))
}

fn unicode_to_string(s: &UNICODE_STRING) -> String {
    if s.Buffer.is_null() || s.Length == 0 {
        return String::new();
    }
    let slice = unsafe { std::slice::from_raw_parts(s.Buffer.0, (s.Length / 2) as usize) };
    String::from_utf16_lossy(slice)
}

/// Top N processes by Process Commit, largest first.
pub fn top_by_commit(mut procs: Vec<ProcessCommit>, n: usize) -> Vec<ProcessCommit> {
    procs.sort_by(|a, b| b.commit.cmp(&a.commit));
    procs.truncate(n);
    procs
}

pub fn fmt_gib(bytes: u64) -> String {
    format!("{:.2} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_usage_is_ratio() {
        let m = SystemMemory {
            commit_charge: 30,
            commit_limit: 40,
            ..Default::default()
        };
        assert!((m.commit_usage() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn commit_usage_zero_limit_is_zero() {
        assert_eq!(SystemMemory::default().commit_usage(), 0.0);
    }

    #[test]
    fn vm_detection() {
        let p = |name: &str| ProcessCommit {
            pid: 1,
            name: name.into(),
            commit: 0,
            working_set: 0,
        };
        assert!(p("vmmem").is_vm());
        assert!(p("vmmemWSL").is_vm());
        assert!(!p("vmware-vmx").is_vm());
    }

    #[test]
    fn top_by_commit_sorts_desc_and_truncates() {
        let p = |pid: u32, commit: u64| ProcessCommit {
            pid,
            name: String::new(),
            commit,
            working_set: 0,
        };
        let top = top_by_commit(vec![p(1, 5), p(2, 50), p(3, 20)], 2);
        assert_eq!(top.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![2, 3]);
    }

    #[test]
    fn live_system_memory_is_consistent() {
        let m = system_memory().expect("system_memory");
        assert!(m.commit_limit >= m.commit_charge);
        assert!(m.physical_total >= m.physical_available);
        // Available and the list counts come from two separate kernel calls, so
        // they are not from the same instant. Allow a small drift.
        let lists = m.free + m.standby;
        let tolerance = m.physical_total / 50; // 2%
        assert!(
            m.physical_available.abs_diff(lists) <= tolerance,
            "available={} free+standby={}",
            m.physical_available,
            lists
        );
    }

    #[test]
    fn live_processes_include_self() {
        let me = std::process::id();
        let procs = processes().expect("processes");
        assert!(procs.iter().any(|p| p.pid == me && p.commit > 0));
    }
}
