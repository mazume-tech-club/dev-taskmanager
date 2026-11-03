//! Minute Record: one minute of Samples folded into averages and maxima, cut
//! at wall-clock minute boundaries so rows line up with other logs.

use crate::metrics::ProcessCommit;
use crate::sample::Sample;
use chrono::{DateTime, Local, Timelike};

#[derive(Debug, Clone, PartialEq)]
pub struct MinuteRecord {
    /// Start of the minute, local time.
    pub minute: DateTime<Local>,
    pub samples: u32,
    pub commit_charge_avg: f64,
    pub commit_charge_max: u64,
    pub commit_limit: u64,
    pub commit_usage_avg: f64,
    pub commit_usage_max: f64,
    pub available_avg: f64,
    pub available_min: u64,
    pub free_avg: f64,
    pub standby_avg: f64,
    pub page_file_used_avg: f64,
    pub page_file_total: u64,
    pub kernel_commit_avg: f64,
    pub vm_memory_avg: f64,
    pub vm_memory_max: u64,
    pub cpu_avg: f64,
    pub cpu_max: f64,
    pub clock_avg_mhz: f64,
    pub thermal_zone_avg_c: Option<f64>,
    pub thermal_zone_max_c: Option<f64>,
    pub package_temp_avg_c: Option<f64>,
    pub package_temp_max_c: Option<f64>,
    /// Top Processes at the end of the minute.
    pub top_processes: Vec<ProcessCommit>,
}

#[derive(Debug, Default)]
struct Stat {
    sum: f64,
    max: f64,
    min: f64,
    n: u32,
}

impl Stat {
    fn push(&mut self, v: f64) {
        if self.n == 0 {
            self.min = v;
            self.max = v;
        } else {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.sum += v;
        self.n += 1;
    }
    fn avg(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.sum / self.n as f64
        }
    }
    fn avg_opt(&self) -> Option<f64> {
        (self.n > 0).then(|| self.avg())
    }
    fn max_opt(&self) -> Option<f64> {
        (self.n > 0).then_some(self.max)
    }
}

#[derive(Debug, Default)]
pub struct MinuteAccumulator {
    minute: Option<DateTime<Local>>,
    samples: u32,
    commit_charge: Stat,
    commit_limit: u64,
    commit_usage: Stat,
    available: Stat,
    free: Stat,
    standby: Stat,
    page_file_used: Stat,
    page_file_total: u64,
    kernel_commit: Stat,
    vm_memory: Stat,
    cpu: Stat,
    clock: Stat,
    thermal_zone: Stat,
    package_temp: Stat,
    top_processes: Vec<ProcessCommit>,
}

pub fn minute_of(at: DateTime<Local>) -> DateTime<Local> {
    at.with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(at)
}

impl MinuteAccumulator {
    /// Adds a Sample. When the Sample belongs to a later minute than the one
    /// being accumulated, the finished minute is returned first.
    pub fn push(&mut self, s: &Sample) -> Option<MinuteRecord> {
        let m = minute_of(s.at);
        let finished = match self.minute {
            Some(cur) if cur != m => self.flush(),
            _ => None,
        };
        if self.minute.is_none() {
            self.minute = Some(m);
        }
        self.samples += 1;
        self.commit_charge.push(s.memory.commit_charge as f64);
        self.commit_limit = s.memory.commit_limit;
        self.commit_usage.push(s.commit_usage());
        self.available.push(s.memory.physical_available as f64);
        self.free.push(s.memory.free as f64);
        self.standby.push(s.memory.standby as f64);
        self.page_file_used.push(s.memory.page_file_in_use() as f64);
        self.page_file_total = s.memory.page_file_total();
        self.kernel_commit.push(s.kernel_commit as f64);
        self.vm_memory.push(s.vm_memory as f64);
        self.cpu.push(s.cpu_total);
        if !s.clock_mhz.is_empty() {
            let avg = s.clock_mhz.iter().sum::<f64>() / s.clock_mhz.len() as f64;
            self.clock.push(avg);
        }
        if let Some(t) = s.thermal_zone_c {
            self.thermal_zone.push(t);
        }
        if let Some(t) = s.package_temp_c {
            self.package_temp.push(t);
        }
        self.top_processes = s.top_processes.clone();
        finished
    }

    /// Emits the current (possibly partial) minute and resets.
    pub fn flush(&mut self) -> Option<MinuteRecord> {
        let minute = self.minute?;
        let rec = MinuteRecord {
            minute,
            samples: self.samples,
            commit_charge_avg: self.commit_charge.avg(),
            commit_charge_max: self.commit_charge.max as u64,
            commit_limit: self.commit_limit,
            commit_usage_avg: self.commit_usage.avg(),
            commit_usage_max: self.commit_usage.max,
            available_avg: self.available.avg(),
            available_min: self.available.min as u64,
            free_avg: self.free.avg(),
            standby_avg: self.standby.avg(),
            page_file_used_avg: self.page_file_used.avg(),
            page_file_total: self.page_file_total,
            kernel_commit_avg: self.kernel_commit.avg(),
            vm_memory_avg: self.vm_memory.avg(),
            vm_memory_max: self.vm_memory.max as u64,
            cpu_avg: self.cpu.avg(),
            cpu_max: self.cpu.max,
            clock_avg_mhz: self.clock.avg(),
            thermal_zone_avg_c: self.thermal_zone.avg_opt(),
            thermal_zone_max_c: self.thermal_zone.max_opt(),
            package_temp_avg_c: self.package_temp.avg_opt(),
            package_temp_max_c: self.package_temp.max_opt(),
            top_processes: std::mem::take(&mut self.top_processes),
        };
        *self = Self::default();
        Some(rec)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::metrics::SystemMemory;
    use chrono::TimeZone;

    pub(crate) fn sample_at(at: DateTime<Local>, commit: u64, cpu: f64) -> Sample {
        Sample {
            at,
            memory: SystemMemory {
                commit_charge: commit,
                commit_limit: 100,
                physical_total: 100,
                physical_available: 50,
                free: 10,
                standby: 40,
                modified: 0,
                page_files: Vec::new(),
                page_file_automatic: true,
            },
            kernel_commit: 5,
            vm_memory: 7,
            top_processes: vec![ProcessCommit {
                pid: 1,
                name: "a.exe".into(),
                commit: commit / 2,
                working_set: 0,
            }],
            cpu_total: cpu,
            cpu_per_logical: vec![cpu],
            clock_mhz: vec![4000.0, 3000.0],
            thermal_zone_c: Some(30.0),
            package_temp_c: None,
            temp_margin_c: None,
        }
    }

    #[test]
    fn minute_boundary_emits_previous_minute() {
        let t0 = Local.with_ymd_and_hms(2026, 11, 3, 12, 0, 58).unwrap();
        let mut acc = MinuteAccumulator::default();
        assert!(acc.push(&sample_at(t0, 60, 0.2)).is_none());
        assert!(
            acc.push(&sample_at(t0 + chrono::Duration::seconds(1), 80, 0.4))
                .is_none()
        );
        let rec = acc
            .push(&sample_at(t0 + chrono::Duration::seconds(2), 10, 0.1))
            .unwrap();
        assert_eq!(
            rec.minute,
            Local.with_ymd_and_hms(2026, 11, 3, 12, 0, 0).unwrap()
        );
        assert_eq!(rec.samples, 2);
        assert_eq!(rec.commit_charge_avg, 70.0);
        assert_eq!(rec.commit_charge_max, 80);
        assert_eq!(rec.commit_usage_max, 0.8);
        assert_eq!(rec.cpu_max, 0.4);
        assert_eq!(rec.clock_avg_mhz, 3500.0);
        assert_eq!(rec.thermal_zone_max_c, Some(30.0));
        assert_eq!(rec.package_temp_avg_c, None);
        assert_eq!(rec.top_processes[0].commit, 40);
        // the new minute continues with the third sample
        let partial = acc.flush().unwrap();
        assert_eq!(partial.samples, 1);
        assert_eq!(partial.commit_charge_max, 10);
        assert!(acc.flush().is_none());
    }
}
