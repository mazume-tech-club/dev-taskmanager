//! Client for the PawnIO kernel driver (https://pawnio.eu, GPL-2.0),
//! the Sensor Provider (see GLOSSARY.md and ADR 0002). PawnIO loads small
//! sandboxed modules; we use the official `IntelMSR` module (LGPL-2.1) to
//! read model-specific registers. Opening the device needs administrator
//! rights.
//!
//! Protocol (from LibreHardwareMonitor's PawnIo.cs and the module source):
//! - `IOCTL_PIO_LOAD_BINARY`: input = module bytes.
//! - `IOCTL_PIO_EXECUTE_FN`: input = 32-byte ASCII function name (NUL
//!   padded) followed by i64 arguments; output = i64 results.

use anyhow::{Context, Result, anyhow, bail};
use std::ffi::c_void;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::w;

const DEVICE_PATH: windows::core::PCWSTR = w!(r"\\?\GLOBALROOT\Device\PawnIO");
const DEVICE_TYPE: u32 = 41394 << 16;
const IOCTL_PIO_LOAD_BINARY: u32 = DEVICE_TYPE | (0x821 << 2);
const IOCTL_PIO_EXECUTE_FN: u32 = DEVICE_TYPE | (0x841 << 2);
const FN_NAME_LEN: usize = 32;

/// Official Intel MSR module, from LibreHardwareMonitor's bundled copy of
/// PawnIO.Modules 0.2.11 (see assets/pawnio/COPYING).
pub const INTEL_MSR_MODULE: &[u8] = include_bytes!("../assets/pawnio/IntelMSR.bin");

pub const INSTALLER_URL: &str =
    "https://github.com/namazso/PawnIO.Setup/releases/latest/download/PawnIO_setup.exe";

/// Why the Sensor Provider is not usable right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    NotInstalled,
    NotElevated,
    Other(String),
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => write!(f, "PawnIO がインストールされていません"),
            Self::NotElevated => write!(f, "管理者権限がありません"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

/// Installed version from the uninstall registry key, if present.
pub fn installed_version() -> Option<String> {
    let key = w!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO");
    let value = w!("DisplayVersion");
    let mut len = 0u32;
    let st = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut len),
        )
    };
    if st.is_err() || len == 0 {
        return None;
    }
    let mut buf = vec![0u16; (len as usize).div_ceil(2)];
    let st = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut len),
        )
    };
    if st.is_err() {
        return None;
    }
    let s = String::from_utf16_lossy(&buf);
    Some(s.trim_end_matches('\0').to_string())
}

pub struct PawnIo {
    handle: HANDLE,
}

unsafe impl Send for PawnIo {}

impl PawnIo {
    /// Opens the device. Distinguishes "driver not installed" from
    /// "not running as administrator".
    pub fn open() -> std::result::Result<Self, Unavailable> {
        let handle = unsafe {
            CreateFileW(
                DEVICE_PATH,
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        };
        match handle {
            Ok(h) => Ok(Self { handle: h }),
            Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => {
                if installed_version().is_some() {
                    Err(Unavailable::Other(
                        "PawnIO デバイスが見つかりません（再起動が必要かもしれません）".into(),
                    ))
                } else {
                    Err(Unavailable::NotInstalled)
                }
            }
            Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => Err(Unavailable::NotElevated),
            Err(e) => Err(Unavailable::Other(format!("PawnIO を開けません: {e}"))),
        }
    }

    pub fn load_module(&self, module: &[u8]) -> Result<()> {
        let mut returned = 0u32;
        unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_PIO_LOAD_BINARY,
                Some(module.as_ptr() as *const c_void),
                module.len() as u32,
                None,
                0,
                Some(&mut returned),
                None,
            )
        }
        .context("PawnIO load module")
    }

    /// Calls a module function with i64 arguments, expecting `out_len` i64
    /// results.
    pub fn execute(&self, name: &str, input: &[i64], out_len: usize) -> Result<Vec<i64>> {
        if name.len() >= FN_NAME_LEN || !name.is_ascii() {
            bail!("bad function name {name:?}");
        }
        let mut inbuf = vec![0u8; FN_NAME_LEN + input.len() * 8];
        inbuf[..name.len()].copy_from_slice(name.as_bytes());
        for (i, v) in input.iter().enumerate() {
            let off = FN_NAME_LEN + i * 8;
            inbuf[off..off + 8].copy_from_slice(&v.to_le_bytes());
        }
        let mut outbuf = vec![0u8; out_len * 8];
        let mut returned = 0u32;
        unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_PIO_EXECUTE_FN,
                Some(inbuf.as_ptr() as *const c_void),
                inbuf.len() as u32,
                if out_len == 0 {
                    None
                } else {
                    Some(outbuf.as_mut_ptr() as *mut c_void)
                },
                outbuf.len() as u32,
                Some(&mut returned),
                None,
            )
        }
        .map_err(|e| anyhow!("PawnIO {name}: {e}"))?;
        let n = (returned as usize / 8).min(out_len);
        let (chunks, _) = outbuf[..n * 8].as_chunks::<8>();
        Ok(chunks.iter().map(|c| i64::from_le_bytes(*c)).collect())
    }

    /// `ioctl_read_msr` from the IntelMSR module, on the current CPU.
    pub fn read_msr(&self, msr: u32) -> Result<u64> {
        let out = self.execute("ioctl_read_msr", &[i64::from(msr)], 1)?;
        out.first()
            .map(|v| *v as u64)
            .ok_or_else(|| anyhow!("ioctl_read_msr(0x{msr:x}) returned nothing"))
    }
}

impl Drop for PawnIo {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_is_embedded() {
        assert!(INTEL_MSR_MODULE.len() > 1000);
    }

    #[test]
    fn ioctl_codes_match_reference() {
        // Values from LibreHardwareMonitor PawnIo.cs: (41394 << 16) | (0x821 << 2) etc.
        assert_eq!(IOCTL_PIO_LOAD_BINARY, 0xA1B2_2084);
        assert_eq!(IOCTL_PIO_EXECUTE_FN, 0xA1B2_2104);
    }

    #[test]
    fn open_reports_a_reason_when_unusable() {
        // Without admin rights or the driver this must be a typed error, never a panic.
        match PawnIo::open() {
            Ok(_) => {}
            Err(Unavailable::NotInstalled | Unavailable::NotElevated | Unavailable::Other(_)) => {}
        }
    }
}
