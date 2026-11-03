//! Start-at-login via `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//! Per user, no administrator rights.

use crate::config::APP_NAME;
use anyhow::{Context, Result, bail};
use std::ffi::c_void;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteValueW,
    RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::{HSTRING, w};

const RUN_KEY: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");

fn value_name() -> HSTRING {
    HSTRING::from(APP_NAME)
}

/// The command registered for autostart: this executable, started hidden.
pub fn command() -> Result<String> {
    let exe = std::env::current_exe().context("current_exe")?;
    Ok(format!("\"{}\" --hidden", exe.display()))
}

pub fn is_enabled() -> bool {
    let mut len = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            &value_name(),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut len),
        )
    };
    status.is_ok() && len > 0
}

pub fn set(enabled: bool) -> Result<()> {
    let mut key = HKEY::default();
    let status =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key) };
    if status.is_err() {
        bail!("open Run key: {:?}", status);
    }
    let result = if enabled {
        let cmd = HSTRING::from(command()?);
        let bytes =
            unsafe { std::slice::from_raw_parts(cmd.as_ptr() as *const u8, (cmd.len() + 1) * 2) };
        let status = unsafe { RegSetValueExW(key, &value_name(), None, REG_SZ, Some(bytes)) };
        if status.is_err() {
            Err(anyhow::anyhow!("set value: {:?}", status))
        } else {
            Ok(())
        }
    } else {
        let status = unsafe { RegDeleteValueW(key, &value_name()) };
        // Deleting a value that does not exist is fine.
        if status.is_err() && status != windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
            Err(anyhow::anyhow!("delete value: {:?}", status))
        } else {
            Ok(())
        }
    };
    unsafe { RegCloseKey(key) }.ok().context("close key")?;
    result
}

#[allow(dead_code)]
fn _assert_void(_: *mut c_void) {}
