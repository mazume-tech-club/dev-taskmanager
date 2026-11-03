//! Intel sensors through the Sensor Provider: Core Temperature per physical
//! core, Package Temperature, TjMax, VID and package power (RAPL).
//!
//! MSR layouts are from the Intel SDM vol. 4:
//! - IA32_THERM_STATUS (0x19C) / IA32_PACKAGE_THERM_STATUS (0x1B1):
//!   bits 22:16 = degrees below TjMax, bit 31 = reading valid.
//! - MSR_TEMPERATURE_TARGET (0x1A2): bits 23:16 = TjMax.
//! - IA32_PERF_STATUS (0x198): bits 47:32 = core voltage in 1/8192 V.
//! - MSR_RAPL_POWER_UNIT (0x606): bits 3:0 power unit, bits 12:8 energy unit.
//! - MSR_PKG_ENERGY_STATUS (0x611): bits 31:0 energy counter.
//! - MSR_PKG_POWER_INFO (0x614): bits 14:0 TDP in power units.

use crate::pawnio::{INTEL_MSR_MODULE, PawnIo, Unavailable};
use anyhow::{Context, Result};
use std::time::Instant;
use windows::Win32::System::SystemInformation::{
    GROUP_AFFINITY, GetLogicalProcessorInformationEx, RelationProcessorCore,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadGroupAffinity};

const IA32_THERM_STATUS: u32 = 0x19C;
const IA32_PACKAGE_THERM_STATUS: u32 = 0x1B1;
const MSR_TEMPERATURE_TARGET: u32 = 0x1A2;
const IA32_PERF_STATUS: u32 = 0x198;
const MSR_RAPL_POWER_UNIT: u32 = 0x606;
const MSR_PKG_ENERGY_STATUS: u32 = 0x611;
const MSR_PKG_POWER_INFO: u32 = 0x614;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SensorReading {
    /// Core Temperature in Celsius, one per physical core.
    pub core_temps_c: Vec<f64>,
    pub package_temp_c: Option<f64>,
    pub tj_max_c: f64,
    pub vid_v: Option<f64>,
    pub package_power_w: Option<f64>,
    pub tdp_w: Option<f64>,
}

impl SensorReading {
    /// Hottest of package and cores, the value the Alert Level uses.
    pub fn hottest_c(&self) -> Option<f64> {
        self.core_temps_c
            .iter()
            .copied()
            .chain(self.package_temp_c)
            .fold(None, |m, t| Some(m.map_or(t, |m: f64| m.max(t))))
    }

    pub fn margin_to_tj_max_c(&self) -> Option<f64> {
        self.hottest_c().map(|t| self.tj_max_c - t)
    }
}

pub struct IntelSensors {
    io: PawnIo,
    /// One affinity per physical core (its first logical processor).
    cores: Vec<GROUP_AFFINITY>,
    tj_max_c: f64,
    energy_unit_j: Option<f64>,
    power_unit_w: Option<f64>,
    last_energy: Option<(Instant, u64)>,
}

impl IntelSensors {
    /// Opens PawnIO and loads the Intel MSR module. `Err(Unavailable)` says
    /// why it cannot be used (not installed, not elevated, not Intel...).
    pub fn new() -> std::result::Result<Self, Unavailable> {
        let io = PawnIo::open()?;
        io.load_module(INTEL_MSR_MODULE).map_err(|e| {
            Unavailable::Other(format!(
                "IntelMSR モジュールを読み込めません（Intel CPU 以外?）: {e}"
            ))
        })?;
        let cores = physical_cores().map_err(|e| Unavailable::Other(e.to_string()))?;
        let mut s = Self {
            io,
            cores,
            tj_max_c: 100.0,
            energy_unit_j: None,
            power_unit_w: None,
            last_energy: None,
        };
        s.init_units()
            .map_err(|e| Unavailable::Other(format!("MSR 読み取りに失敗: {e}")))?;
        Ok(s)
    }

    pub fn core_count(&self) -> usize {
        self.cores.len()
    }

    fn init_units(&mut self) -> Result<()> {
        let first = *self.cores.first().context("no cores")?;
        let target = self.read_on(first, MSR_TEMPERATURE_TARGET)?;
        let tj = (target >> 16) & 0xFF;
        if tj > 0 {
            self.tj_max_c = tj as f64;
        }
        if let Ok(units) = self.read_on(first, MSR_RAPL_POWER_UNIT) {
            let power_shift = units & 0xF;
            let energy_shift = (units >> 8) & 0x1F;
            self.power_unit_w = Some(1.0 / f64::from(1u32 << power_shift));
            self.energy_unit_j = Some(1.0 / f64::from(1u32 << energy_shift));
        }
        Ok(())
    }

    pub fn read(&mut self) -> Result<SensorReading> {
        let mut core_temps_c = Vec::with_capacity(self.cores.len());
        for &core in &self.cores {
            let status = self.read_on(core, IA32_THERM_STATUS)?;
            if status & (1 << 31) != 0 {
                let delta = (status >> 16) & 0x7F;
                core_temps_c.push(self.tj_max_c - delta as f64);
            }
        }
        let first = *self.cores.first().context("no cores")?;
        let package_temp_c = self
            .read_on(first, IA32_PACKAGE_THERM_STATUS)
            .ok()
            .and_then(|s| {
                (s & (1 << 31) != 0).then_some(self.tj_max_c - ((s >> 16) & 0x7F) as f64)
            });
        let vid_v = self
            .read_on(first, IA32_PERF_STATUS)
            .ok()
            .map(|v| ((v >> 32) & 0xFFFF) as f64 / 8192.0)
            .filter(|v| *v > 0.0);

        let mut package_power_w = None;
        if let Some(unit) = self.energy_unit_j
            && let Ok(e) = self.read_on(first, MSR_PKG_ENERGY_STATUS)
        {
            let now = Instant::now();
            let energy = e & 0xFFFF_FFFF;
            if let Some((t0, e0)) = self.last_energy {
                let dt = now.duration_since(t0).as_secs_f64();
                if dt > 0.05 {
                    let delta = energy.wrapping_sub(e0) & 0xFFFF_FFFF;
                    package_power_w = Some(delta as f64 * unit / dt);
                }
            }
            self.last_energy = Some((now, energy));
        }
        let tdp_w = self
            .power_unit_w
            .and_then(|unit| {
                self.read_on(first, MSR_PKG_POWER_INFO)
                    .ok()
                    .map(|v| (v & 0x7FFF) as f64 * unit)
            })
            .filter(|w| *w > 0.0);

        Ok(SensorReading {
            core_temps_c,
            package_temp_c,
            tj_max_c: self.tj_max_c,
            vid_v,
            package_power_w,
            tdp_w,
        })
    }

    /// Reads an MSR on a specific core by pinning this thread there for the
    /// duration of the call.
    fn read_on(&self, core: GROUP_AFFINITY, msr: u32) -> Result<u64> {
        let mut previous = GROUP_AFFINITY::default();
        let thread = unsafe { GetCurrentThread() };
        let pinned =
            unsafe { SetThreadGroupAffinity(thread, &core, Some(&mut previous)) }.as_bool();
        let result = self.io.read_msr(msr);
        if pinned {
            unsafe {
                let _ = SetThreadGroupAffinity(thread, &previous, None);
            }
        }
        result
    }
}

/// First logical processor of every physical core.
pub fn physical_cores() -> Result<Vec<GROUP_AFFINITY>> {
    let mut len = 0u32;
    let _ = unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut len) };
    if len == 0 {
        anyhow::bail!("GetLogicalProcessorInformationEx size query failed");
    }
    let mut buf = vec![0u8; len as usize];
    unsafe {
        GetLogicalProcessorInformationEx(
            RelationProcessorCore,
            Some(buf.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX),
            &mut len,
        )
    }
    .context("GetLogicalProcessorInformationEx")?;
    let mut cores = Vec::new();
    let mut offset = 0usize;
    while offset + size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() <= buf.len() {
        let entry = unsafe {
            &*(buf.as_ptr().add(offset) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX)
        };
        if entry.Size == 0 {
            break;
        }
        if entry.Relationship == RelationProcessorCore {
            let proc_ = unsafe { entry.Anonymous.Processor };
            let mask = proc_.GroupMask[0];
            if mask.Mask != 0 {
                let lowest = mask.Mask & mask.Mask.wrapping_neg();
                cores.push(GROUP_AFFINITY {
                    Mask: lowest,
                    Group: mask.Group,
                    Reserved: [0; 3],
                });
            }
        }
        offset += entry.Size as usize;
    }
    if cores.is_empty() {
        anyhow::bail!("no processor cores reported");
    }
    Ok(cores)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_physical_cores() {
        let cores = physical_cores().unwrap();
        assert!(!cores.is_empty());
        assert!(cores.len() <= crate::cpu::logical_processor_count());
        for c in &cores {
            assert_eq!(c.Mask.count_ones(), 1);
        }
    }

    #[test]
    fn hottest_and_margin() {
        let r = SensorReading {
            core_temps_c: vec![40.0, 55.0],
            package_temp_c: Some(50.0),
            tj_max_c: 100.0,
            ..Default::default()
        };
        assert_eq!(r.hottest_c(), Some(55.0));
        assert_eq!(r.margin_to_tj_max_c(), Some(45.0));
        assert_eq!(SensorReading::default().hottest_c(), None);
    }
}
