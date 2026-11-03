//! CPU Load and clock per logical processor, read without any external tool.
//!
//! Load is derived from the difference of two readings of per-processor idle /
//! kernel / user times, so callers keep a [`CpuSampler`] and call
//! [`CpuSampler::sample`] once per interval. One kernel call per reading.

use crate::pdh::CounterQuery;
use anyhow::{Result, bail};
use std::ffi::c_void;
use windows::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessorPerformanceInformation,
};
use windows::Win32::Foundation::STATUS_SUCCESS;
use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows::Win32::System::WindowsProgramming::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION;

/// CPU Load for one interval, 0.0..=1.0 per logical processor plus the total.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CpuLoad {
    pub total: f64,
    pub per_logical: Vec<f64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Times {
    idle: i64,
    busy: i64,
}

pub struct CpuSampler {
    last: Vec<Times>,
}

impl CpuSampler {
    /// Takes the first reading so that the next `sample` has a baseline.
    pub fn new() -> Result<Self> {
        Ok(Self {
            last: read_times()?,
        })
    }

    /// CPU Load since the previous call. The first call after `new` measures
    /// from construction.
    pub fn sample(&mut self) -> Result<CpuLoad> {
        let now = read_times()?;
        let n = now.len().min(self.last.len());
        let mut per_logical = Vec::with_capacity(n);
        let mut sum_busy = 0i64;
        let mut sum_total = 0i64;
        for i in 0..n {
            let d_idle = now[i].idle - self.last[i].idle;
            let d_busy = now[i].busy - self.last[i].busy;
            let total = d_idle + d_busy;
            per_logical.push(if total > 0 {
                d_busy as f64 / total as f64
            } else {
                0.0
            });
            sum_busy += d_busy;
            sum_total += total;
        }
        self.last = now;
        Ok(CpuLoad {
            total: if sum_total > 0 {
                sum_busy as f64 / sum_total as f64
            } else {
                0.0
            },
            per_logical,
        })
    }
}

pub fn logical_processor_count() -> usize {
    let mut info = SYSTEM_INFO::default();
    unsafe { GetSystemInfo(&mut info) };
    (info.dwNumberOfProcessors as usize).max(1)
}

fn read_times() -> Result<Vec<Times>> {
    let n = logical_processor_count();
    let mut buf = vec![SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION::default(); n];
    let len = (n * size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>()) as u32;
    let mut ret = 0u32;
    let status = unsafe {
        NtQuerySystemInformation(
            SystemProcessorPerformanceInformation,
            buf.as_mut_ptr() as *mut c_void,
            len,
            &mut ret,
        )
    };
    if status != STATUS_SUCCESS {
        bail!(
            "NtQuerySystemInformation(SystemProcessorPerformanceInformation) = {:#x}",
            status.0
        );
    }
    let got = (ret as usize / size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>()).min(n);
    Ok(buf[..got]
        .iter()
        .map(|t| Times {
            idle: t.IdleTime,
            // KernelTime includes IdleTime on Windows.
            busy: t.KernelTime - t.IdleTime + t.UserTime,
        })
        .collect())
}

/// Effective Clock per logical processor, from the performance counters
/// `% Processor Performance` (ratio to nominal, may exceed 100% under turbo)
/// and `Processor Frequency` (nominal MHz). Readable without administrator
/// rights. The power manager's `CurrentMhz` only reports the nominal clock.
pub struct ClockSampler {
    query: CounterQuery,
}

impl ClockSampler {
    pub fn new() -> Result<Self> {
        let mut query = CounterQuery::new(&[
            r"\Processor Information(*)\% Processor Performance",
            r"\Processor Information(*)\Processor Frequency",
        ])?;
        query.collect()?; // baseline for the rate counter
        Ok(Self { query })
    }

    /// Effective Clock in MHz, indexed by logical processor number. Entries
    /// without data yet are 0.
    pub fn sample(&mut self) -> Result<Vec<f64>> {
        self.query.collect()?;
        let perf = self.query.values(0)?;
        let freq = self.query.values(1)?;
        let n = logical_processor_count();
        let mut out = vec![0.0; n];
        for (name, pct) in perf {
            let Some(idx) = logical_index(&name) else {
                continue;
            };
            let nominal = freq
                .iter()
                .find(|(f, _)| *f == name)
                .map(|(_, v)| *v)
                .unwrap_or(0.0);
            if idx < n {
                out[idx] = nominal * pct / 100.0;
            }
        }
        Ok(out)
    }
}

/// Instance "g,i" is logical processor i of processor group g (64 per group).
/// "_Total" and anything else yields None.
fn logical_index(instance: &str) -> Option<usize> {
    let (group, idx) = instance.split_once(',')?;
    Some(group.parse::<usize>().ok()? * 64 + idx.parse::<usize>().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_load_is_in_range() {
        let mut s = CpuSampler::new().expect("sampler");
        std::thread::sleep(std::time::Duration::from_millis(200));
        let load = s.sample().expect("sample");
        assert_eq!(load.per_logical.len(), logical_processor_count());
        assert!((0.0..=1.0).contains(&load.total));
        assert!(load.per_logical.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn live_effective_clock_present() {
        let mut c = ClockSampler::new().expect("clock sampler");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mhz = c.sample().expect("sample");
        assert_eq!(mhz.len(), logical_processor_count());
        assert!(mhz.iter().any(|&m| m > 0.0), "{mhz:?}");
    }

    #[test]
    fn logical_index_parses_group_and_index() {
        assert_eq!(logical_index("0,3"), Some(3));
        assert_eq!(logical_index("1,2"), Some(66));
        assert_eq!(logical_index("_Total"), None);
    }
}
