//! CPU Load and clock per logical processor, read without any external tool.
//!
//! Load is derived from the difference of two readings of per-processor idle /
//! kernel / user times, so callers keep a [`CpuSampler`] and call
//! [`CpuSampler::sample`] once per interval. One kernel call per reading.

use anyhow::{Result, bail};
use std::ffi::c_void;
use windows::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessorPerformanceInformation,
};
use windows::Win32::Foundation::STATUS_SUCCESS;
use windows::Win32::System::Power::{
    CallNtPowerInformation, PROCESSOR_POWER_INFORMATION, ProcessorInformation,
};
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

/// Current clock in MHz per logical processor, as reported by the power
/// manager. On some systems this is the nominal clock rather than the live
/// one; a Sensor Provider's value is preferred when available.
pub fn clocks_mhz() -> Result<Vec<u32>> {
    let n = logical_processor_count();
    let mut buf = vec![PROCESSOR_POWER_INFORMATION::default(); n];
    let len = (n * size_of::<PROCESSOR_POWER_INFORMATION>()) as u32;
    let status = unsafe {
        CallNtPowerInformation(
            ProcessorInformation,
            None,
            0,
            Some(buf.as_mut_ptr() as *mut c_void),
            len,
        )
    };
    if status != STATUS_SUCCESS {
        bail!(
            "CallNtPowerInformation(ProcessorInformation) = {:#x}",
            status.0
        );
    }
    Ok(buf.iter().map(|p| p.CurrentMhz).collect())
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
    fn live_clocks_present() {
        let c = clocks_mhz().expect("clocks");
        assert_eq!(c.len(), logical_processor_count());
        assert!(c.iter().all(|&m| m > 0));
    }
}
