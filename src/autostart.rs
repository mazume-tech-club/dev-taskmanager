//! Start-at-login. Without administrator rights this is the per-user
//! `HKCU\...\Run` key. When the process is elevated (Sensor Provider
//! mode) it is a scheduled task with highest privileges instead, because a
//! Run-key entry cannot start elevated without a UAC prompt at logon.

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

fn run_key_is_enabled() -> bool {
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

fn run_key_set(enabled: bool) -> Result<()> {
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

const TASK_NAME: &str = "CommitWatch";

fn is_elevated() -> bool {
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}

fn schtasks(args: &[&str]) -> Result<bool> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("run schtasks")?;
    Ok(out.status.success())
}

fn task_is_enabled() -> bool {
    schtasks(&["/Query", "/TN", TASK_NAME]).unwrap_or(false)
}

fn task_set(enabled: bool) -> Result<()> {
    if enabled {
        let cmd = command()?;
        let ok = schtasks(&[
            "/Create", "/F", "/TN", TASK_NAME, "/TR", &cmd, "/SC", "ONLOGON", "/RL", "HIGHEST",
            "/IT",
        ])?;
        if !ok {
            bail!("schtasks /Create failed");
        }
        Ok(())
    } else {
        if task_is_enabled() && !schtasks(&["/Delete", "/F", "/TN", TASK_NAME])? {
            bail!("schtasks /Delete failed");
        }
        Ok(())
    }
}

/// True when either mechanism is registered.
pub fn is_enabled() -> bool {
    run_key_is_enabled() || task_is_enabled()
}

/// Registers (or removes) autostart using the mechanism that fits the
/// current privilege level, and clears the other one.
pub fn set(enabled: bool) -> Result<()> {
    if is_elevated() {
        run_key_set(false)?;
        task_set(enabled)
    } else {
        if task_is_enabled() {
            // Can only be removed elevated; leave it and report.
            if !enabled {
                bail!(
                    "管理者権限のタスクが登録されています。管理者権限で起動してから無効にしてください"
                );
            }
            return Ok(());
        }
        run_key_set(enabled)
    }
}
