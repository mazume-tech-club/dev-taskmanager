//! Sample: every metric read at one instant. The sampler thread produces one
//! per second, keeps the recent history for the graphs, and hands finished
//! minutes to the log.

use crate::config::Config;
use crate::cpu::{ClockSampler, CpuSampler};
use crate::metrics::{self, ProcessCommit, SystemMemory};
use crate::minute::{MinuteAccumulator, MinuteRecord};
use crate::pawnio::Unavailable;
use crate::sensors::IntelSensors;
use crate::shm;
use crate::thermal::ThermalSampler;
use anyhow::Result;
use chrono::{DateTime, Local};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub at: DateTime<Local>,
    pub memory: SystemMemory,
    /// Commit Charge minus the sum of all Process Commit. See GLOSSARY.
    pub kernel_commit: u64,
    pub vm_memory: u64,
    pub top_processes: Vec<ProcessCommit>,
    pub cpu_total: f64,
    pub cpu_per_logical: Vec<f64>,
    pub clock_mhz: Vec<f64>,
    /// Thermal Zone Temperature in Celsius, when the machine has one.
    pub thermal_zone_c: Option<f64>,
    /// Package Temperature in Celsius. `None` until a Sensor Provider exists.
    pub package_temp_c: Option<f64>,
    /// TjMax minus the hottest temperature. `None` without a Sensor Provider.
    pub temp_margin_c: Option<f64>,
    /// Core Temperature per physical core, from the Sensor Provider.
    pub core_temps_c: Vec<f64>,
    pub tj_max_c: Option<f64>,
    pub vid_v: Option<f64>,
    pub package_power_w: Option<f64>,
    pub tdp_w: Option<f64>,
    pub sensor_state: SensorState,
}

/// Whether the Sensor Provider is delivering values, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SensorState {
    #[default]
    Disabled,
    Available,
    NotInstalled,
    NotElevated,
    Failed,
}

impl SensorState {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Disabled => 0,
            Self::Available => 1,
            Self::NotInstalled => 2,
            Self::NotElevated => 3,
            Self::Failed => 4,
        }
    }
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Available,
            2 => Self::NotInstalled,
            3 => Self::NotElevated,
            4 => Self::Failed,
            _ => Self::Disabled,
        }
    }
    pub fn label_ja(self) -> &'static str {
        match self {
            Self::Disabled => "設定で無効",
            Self::Available => "取得中",
            Self::NotInstalled => "PawnIO が未インストール",
            Self::NotElevated => "管理者権限が必要",
            Self::Failed => "読み取りに失敗",
        }
    }
}

impl Sample {
    pub fn commit_usage(&self) -> f64 {
        self.memory.commit_usage()
    }
}

/// What the sampler shares with the UI.
#[derive(Debug, Default)]
pub struct Shared {
    pub history: VecDeque<Sample>,
    pub last_error: Option<String>,
}

impl Shared {
    pub fn latest(&self) -> Option<&Sample> {
        self.history.back()
    }
}

pub struct Sampler {
    pub shared: Arc<Mutex<Shared>>,
    pub minutes: Receiver<MinuteRecord>,
    stop: Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl Sampler {
    /// Starts the sampler thread. Finished Minute Records arrive on `minutes`.
    pub fn start(cfg: Config) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (minute_tx, minute_rx) = channel();
        let (stop_tx, stop_rx) = channel::<()>();
        let thread_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("sampler".into())
            .spawn(move || run(cfg, thread_shared, minute_tx, stop_rx))
            .expect("spawn sampler thread");
        Self {
            shared,
            minutes: minute_rx,
            stop: stop_tx,
            handle: Some(handle),
        }
    }

    /// Stops the thread. The partial current minute is flushed to `minutes`
    /// before the thread exits so the log has no gap at shutdown.
    pub fn stop(&mut self) {
        let _ = self.stop.send(());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Sources {
    cpu: CpuSampler,
    clock: Option<ClockSampler>,
    thermal: Option<ThermalSampler>,
    sensors: Option<IntelSensors>,
    sensor_state: SensorState,
}

fn open_sensors(enabled: bool) -> (Option<IntelSensors>, SensorState) {
    if !enabled {
        return (None, SensorState::Disabled);
    }
    match IntelSensors::new() {
        Ok(s) => (Some(s), SensorState::Available),
        Err(Unavailable::NotInstalled) => (None, SensorState::NotInstalled),
        Err(Unavailable::NotElevated) => (None, SensorState::NotElevated),
        Err(Unavailable::Other(e)) => {
            log::warn!("sensor provider: {e}");
            (None, SensorState::Failed)
        }
    }
}

fn run(cfg: Config, shared: Arc<Mutex<Shared>>, minutes: Sender<MinuteRecord>, stop: Receiver<()>) {
    let mut sources = match CpuSampler::new() {
        Ok(cpu) => {
            let (sensors, sensor_state) = open_sensors(cfg.sensors.enabled);
            Sources {
                cpu,
                clock: ClockSampler::new().ok(),
                thermal: ThermalSampler::new().ok().flatten(),
                sensors,
                sensor_state,
            }
        }
        Err(e) => {
            shared.lock().unwrap().last_error = Some(format!("CPU sampler: {e}"));
            return;
        }
    };
    let process_every = Duration::from_secs(cfg.general.process_interval_secs.max(1));
    let mut last_processes: Vec<ProcessCommit> = Vec::new();
    let mut last_process_read: Option<Instant> = None;
    let mut shm = match shm::Writer::create() {
        Ok(w) => Some(w),
        Err(e) => {
            log::error!("shared memory: {e}");
            None
        }
    };
    let mut acc = MinuteAccumulator::default();
    let interval = Duration::from_secs(1);
    let mut next = Instant::now() + interval;

    loop {
        match stop.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        next += interval;
        if next < Instant::now() {
            next = Instant::now() + interval; // we fell behind (sleep/hibernate)
        }

        let refresh = last_process_read.is_none_or(|t| t.elapsed() >= process_every);
        if refresh {
            match metrics::processes() {
                Ok(p) => {
                    last_processes = p;
                    last_process_read = Some(Instant::now());
                }
                Err(e) => shared.lock().unwrap().last_error = Some(format!("processes: {e}")),
            }
        }

        match take_sample(&mut sources, &last_processes, cfg.general.top_processes) {
            Ok(sample) => {
                if let Some(record) = acc.push(&sample) {
                    let _ = minutes.send(record);
                }
                let mut s = shared.lock().unwrap();
                s.history.push_back(sample);
                while s.history.len() > cfg.general.history_secs.max(1) {
                    s.history.pop_front();
                }
                s.last_error = None;
                if let Some(w) = shm.as_mut() {
                    w.publish(s.history.iter(), s.history.back());
                }
            }
            Err(e) => shared.lock().unwrap().last_error = Some(e.to_string()),
        }
    }

    if let Some(record) = acc.flush() {
        let _ = minutes.send(record);
    }
}

fn take_sample(sources: &mut Sources, processes: &[ProcessCommit], top_n: usize) -> Result<Sample> {
    let memory = metrics::system_memory()?;
    let load = sources.cpu.sample()?;
    let clock_mhz = sources
        .clock
        .as_mut()
        .and_then(|c| c.sample().ok())
        .unwrap_or_default();
    let thermal_zone_c = sources
        .thermal
        .as_mut()
        .and_then(|t| t.sample().ok())
        .and_then(|zones| zones.first().map(|z| z.celsius));

    let reading = match sources.sensors.as_mut() {
        Some(sensors) => match sensors.read() {
            Ok(r) => Some(r),
            Err(e) => {
                log::warn!("sensor read failed, disabling: {e}");
                sources.sensors = None;
                sources.sensor_state = SensorState::Failed;
                None
            }
        },
        None => None,
    };

    let process_sum: u64 = processes.iter().map(|p| p.commit).sum();
    let vm_memory = processes
        .iter()
        .filter(|p| p.is_vm())
        .map(|p| p.commit)
        .sum();
    // Keep more than the logged count so the window can show a longer list.
    let top_processes = metrics::top_by_commit(processes.to_vec(), top_n.max(20));

    Ok(Sample {
        at: Local::now(),
        kernel_commit: memory.commit_charge.saturating_sub(process_sum),
        vm_memory,
        top_processes,
        cpu_total: load.total,
        cpu_per_logical: load.per_logical,
        clock_mhz,
        thermal_zone_c,
        package_temp_c: reading.as_ref().and_then(|r| r.package_temp_c),
        temp_margin_c: reading.as_ref().and_then(|r| r.margin_to_tj_max_c()),
        core_temps_c: reading
            .as_ref()
            .map(|r| r.core_temps_c.clone())
            .unwrap_or_default(),
        tj_max_c: reading.as_ref().map(|r| r.tj_max_c),
        vid_v: reading.as_ref().and_then(|r| r.vid_v),
        package_power_w: reading.as_ref().and_then(|r| r.package_power_w),
        tdp_w: reading.as_ref().and_then(|r| r.tdp_w),
        sensor_state: sources.sensor_state,
        memory,
    })
}
