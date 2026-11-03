//! Sensor Provider: Core Temp (alcpu.com) shared memory.
//!
//! Core Temp publishes `core_temp_shared_data_ex` in a named file mapping
//! "CoreTempMappingObjectEx" while it runs. Reading it is one map + one copy,
//! so it costs nothing measurable. When Core Temp is not running the mapping
//! does not exist and [`read`] returns `Ok(None)`.
//!
//! Layout source: https://alcpu.com/CoreTemp/developers.html

use anyhow::{Context, Result};
use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND};
use windows::Win32::System::Memory::{
    FILE_MAP_READ, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile,
};
use windows::core::w;

const MAPPING_NAME: windows::core::PCWSTR = w!("CoreTempMappingObjectEx");

#[repr(C)]
#[derive(Clone, Copy)]
struct SharedDataEx {
    ui_load: [u32; 256],
    ui_tj_max: [u32; 128],
    ui_core_cnt: u32,
    ui_cpu_cnt: u32,
    f_temp: [f32; 256],
    f_vid: f32,
    f_cpu_speed: f32,
    f_fsb_speed: f32,
    f_multiplier: f32,
    s_cpu_name: [u8; 100],
    uc_fahrenheit: u8,
    uc_delta_to_tj_max: u8,
    uc_tdp_supported: u8,
    uc_power_supported: u8,
    ui_struct_version: u32,
    ui_tdp: [u32; 128],
    f_power: [f32; 128],
    f_multipliers: [f32; 256],
}

/// One Core Temp reading, normalised to Celsius and absolute temperatures.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoreTempReading {
    pub cpu_name: String,
    /// Physical cores per CPU package.
    pub cores_per_cpu: usize,
    /// CPU packages (sockets).
    pub cpu_count: usize,
    /// Core Temperature in Celsius, index = cpu * cores_per_cpu + core.
    pub core_temps_c: Vec<f32>,
    /// Core Temp's own per-core load in percent, same indexing.
    pub core_loads_pct: Vec<u32>,
    /// TjMax in Celsius per CPU package.
    pub tj_max_c: Vec<u32>,
    pub cpu_speed_mhz: f32,
    pub fsb_speed_mhz: f32,
    pub multiplier: f32,
    pub vid: f32,
    pub struct_version: u32,
    /// Package power in watts per CPU, when supported.
    pub power_w: Vec<f32>,
    pub tdp_w: Vec<u32>,
}

impl CoreTempReading {
    /// Package Temperature approximation: the hottest core of each package.
    pub fn package_temp_c(&self, cpu: usize) -> Option<f32> {
        let start = cpu * self.cores_per_cpu;
        let end = (start + self.cores_per_cpu).min(self.core_temps_c.len());
        self.core_temps_c
            .get(start..end)?
            .iter()
            .copied()
            .fold(None, |m, t| Some(m.map_or(t, |m: f32| m.max(t))))
    }

    /// Margin to TjMax for the hottest core of package 0, in Celsius.
    pub fn margin_to_tj_max_c(&self) -> Option<f32> {
        let tj = *self.tj_max_c.first()? as f32;
        Some(tj - self.package_temp_c(0)?)
    }
}

/// `Ok(None)` when Core Temp is not running.
pub fn read() -> Result<Option<CoreTempReading>> {
    let handle = match unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, MAPPING_NAME) } {
        Ok(h) => h,
        Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => return Ok(None),
        Err(e) => return Err(e).context("OpenFileMappingW(CoreTempMappingObjectEx)"),
    };
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, size_of::<SharedDataEx>()) };
    let result = if view.Value.is_null() {
        Err(windows::core::Error::from_thread()).context("MapViewOfFile")
    } else {
        let raw: SharedDataEx =
            unsafe { std::ptr::read_unaligned(view.Value as *const SharedDataEx) };
        unsafe { UnmapViewOfFile(view) }
            .ok()
            .context("UnmapViewOfFile")?;
        Ok(Some(normalise(&raw)))
    };
    unsafe { CloseHandle(handle) }.ok().context("CloseHandle")?;
    result
}

fn normalise(raw: &SharedDataEx) -> CoreTempReading {
    let cores = (raw.ui_core_cnt as usize).min(256);
    let cpus = (raw.ui_cpu_cnt as usize)
        .min(128)
        .max(if cores > 0 { 1 } else { 0 });
    let total = (cores * cpus).min(256);

    let to_c = |v: f32| {
        if raw.uc_fahrenheit != 0 {
            (v - 32.0) * 5.0 / 9.0
        } else {
            v
        }
    };
    let tj_max_c: Vec<u32> = raw.ui_tj_max[..cpus]
        .iter()
        .map(|&t| {
            if raw.uc_fahrenheit != 0 {
                to_c(t as f32).round() as u32
            } else {
                t
            }
        })
        .collect();

    let core_temps_c: Vec<f32> = (0..total)
        .map(|i| {
            let t = to_c(raw.f_temp[i]);
            if raw.uc_delta_to_tj_max != 0 {
                let tj = tj_max_c.get(i / cores.max(1)).copied().unwrap_or(100) as f32;
                tj - t
            } else {
                t
            }
        })
        .collect();

    let name_len = raw.s_cpu_name.iter().position(|&b| b == 0).unwrap_or(100);
    let v2 = raw.ui_struct_version >= 2;

    CoreTempReading {
        cpu_name: String::from_utf8_lossy(&raw.s_cpu_name[..name_len])
            .trim()
            .to_string(),
        cores_per_cpu: cores,
        cpu_count: cpus,
        core_temps_c,
        core_loads_pct: raw.ui_load[..total].to_vec(),
        tj_max_c,
        cpu_speed_mhz: raw.f_cpu_speed,
        fsb_speed_mhz: raw.f_fsb_speed,
        multiplier: raw.f_multiplier,
        vid: raw.f_vid,
        struct_version: raw.ui_struct_version,
        power_w: if v2 && raw.uc_power_supported != 0 {
            raw.f_power[..cpus].to_vec()
        } else {
            Vec::new()
        },
        tdp_w: if v2 && raw.uc_tdp_supported != 0 {
            raw.ui_tdp[..cpus].to_vec()
        } else {
            Vec::new()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_layout_matches_sdk() {
        // Offsets computed by hand from the SDK declaration (all 4-byte aligned).
        assert_eq!(std::mem::offset_of!(SharedDataEx, ui_core_cnt), 1536);
        assert_eq!(std::mem::offset_of!(SharedDataEx, f_temp), 1544);
        assert_eq!(std::mem::offset_of!(SharedDataEx, s_cpu_name), 2584);
        assert_eq!(std::mem::offset_of!(SharedDataEx, ui_struct_version), 2688);
        assert_eq!(std::mem::offset_of!(SharedDataEx, f_multipliers), 3716);
        assert_eq!(size_of::<SharedDataEx>(), 4740);
    }

    fn sample_raw() -> SharedDataEx {
        let mut raw: SharedDataEx = unsafe { std::mem::zeroed() };
        raw.ui_core_cnt = 2;
        raw.ui_cpu_cnt = 1;
        raw.ui_tj_max[0] = 100;
        raw.f_temp[0] = 40.0;
        raw.f_temp[1] = 55.0;
        raw.ui_load[1] = 77;
        raw.s_cpu_name[..3].copy_from_slice(b"Xeo");
        raw.ui_struct_version = 2;
        raw
    }

    #[test]
    fn normalise_absolute_celsius() {
        let r = normalise(&sample_raw());
        assert_eq!(r.cpu_name, "Xeo");
        assert_eq!(r.core_temps_c, vec![40.0, 55.0]);
        assert_eq!(r.package_temp_c(0), Some(55.0));
        assert_eq!(r.margin_to_tj_max_c(), Some(45.0));
        assert_eq!(r.core_loads_pct[1], 77);
    }

    #[test]
    fn normalise_delta_mode() {
        let mut raw = sample_raw();
        raw.uc_delta_to_tj_max = 1;
        let r = normalise(&raw);
        assert_eq!(r.core_temps_c, vec![60.0, 45.0]);
    }

    #[test]
    fn normalise_fahrenheit() {
        let mut raw = sample_raw();
        raw.uc_fahrenheit = 1;
        raw.f_temp[0] = 212.0;
        raw.ui_tj_max[0] = 212;
        let r = normalise(&raw);
        assert!((r.core_temps_c[0] - 100.0).abs() < 1e-3);
        assert_eq!(r.tj_max_c[0], 100);
    }

    #[test]
    fn read_does_not_fail_when_absent() {
        // Either Core Temp is running (Some) or not (None); never an error.
        assert!(read().is_ok());
    }
}
