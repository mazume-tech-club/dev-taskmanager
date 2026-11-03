//! The resident process: tray icon, balloon notification, Alert Level
//! monitor, log writer, and launcher for the window process.
//!
//! Runs a hidden Win32 window with a 1 s timer on the calling thread. The
//! `tray-icon` crate was not used because it cannot show balloon
//! notifications. The window is a separate process (see ADR 0003) so that
//! the resident footprint stays at a few MB.

use crate::alert::{AlertLevel, Notifier};
use crate::autostart;
use crate::config::{self, Config};
use crate::icon;
use crate::logger::CsvLog;
use crate::metrics::fmt_gib;
use crate::sample::Sampler;
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW, HICON,
    IMAGE_FLAGS, KillTimer, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SW_RESTORE, SetForegroundWindow,
    SetTimer, ShowWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK,
    WM_NULL, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{HSTRING, PCWSTR, w};

const WM_TRAY: u32 = WM_APP + 1;
/// Not exported by the windows crate: NIN_SELECT + 1.
const NIN_KEYSELECT: u32 = NIN_SELECT + 1;
const TIMER_ID: usize = 1;
const ICON_SIZE: usize = 32;
const CONFIG_POLL: Duration = Duration::from_secs(5);
/// Title of the egui window, used to bring an existing one to the front.
pub const WINDOW_TITLE: &str = "CommitWatch";

const ID_SHOW: usize = 1;
const ID_OPEN_LOGS: usize = 2;
const ID_AUTOSTART: usize = 3;
const ID_CHECK_UPDATE: usize = 4;
const ID_EXIT: usize = 9;

struct State {
    hwnd: HWND,
    nid: NOTIFYICONDATAW,
    icon: HICON,
    sampler: Sampler,
    log: CsvLog,
    cfg: Config,
    notifier: Notifier,
    last_key: Option<(AlertLevel, u8)>,
    taskbar_created: u32,
    exe: PathBuf,
    window: Option<Child>,
    config_checked: Instant,
    config_mtime: Option<SystemTime>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Blocks on the message loop until "終了" is chosen.
pub fn run(sampler: Sampler, log: CsvLog, cfg: Config, exe: PathBuf, open_window: bool) {
    let hwnd = match create_window() {
        Ok(h) => h,
        Err(e) => {
            log::error!("tray window: {e}");
            return;
        }
    };
    let icon = make_icon(AlertLevel::Normal, 0.0);
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    copy_wide(&mut nid.szTip, "CommitWatch");
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    let config_mtime = config_mtime();

    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            hwnd,
            nid,
            icon,
            sampler,
            log,
            cfg,
            notifier: Notifier::default(),
            last_key: None,
            taskbar_created,
            exe,
            window: None,
            config_checked: Instant::now(),
            config_mtime,
        })
    });
    if open_window {
        with_state(|s| s.show_window());
    }
    unsafe { SetTimer(Some(hwnd), TIMER_ID, 1000, None) };

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn create_window() -> windows::core::Result<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let class_name = w!("CommitWatchTrayWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("CommitWatch Tray"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            with_state(|s| s.tick());
            LRESULT(0)
        }
        WM_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            match event {
                NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONDBLCLK => with_state(|s| s.show_window()),
                WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            let taskbar_created = STATE.with(|s| s.borrow().as_ref().map(|st| st.taskbar_created));
            if Some(msg) == taskbar_created {
                with_state(|s| s.re_add());
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|s| {
        if let Ok(mut guard) = s.try_borrow_mut()
            && let Some(state) = guard.as_mut()
        {
            f(state);
        }
    });
}

/// Builds and shows the context menu. The state borrow is released while the
/// menu's modal loop runs, because that loop re-enters `wndproc`.
fn show_menu(hwnd: HWND) {
    let autostart_on = autostart::is_enabled();
    let choice = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let item = |id: usize, text: &str, flags| {
            let _ = AppendMenuW(menu, flags, id, &HSTRING::from(text));
        };
        item(ID_SHOW, "表示(&S)", MF_STRING);
        item(ID_OPEN_LOGS, "ログフォルダを開く(&L)", MF_STRING);
        item(
            ID_AUTOSTART,
            "Windows 起動時に開始(&A)",
            if autostart_on {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            },
        );
        item(
            ID_CHECK_UPDATE,
            "更新を確認(&U)（未実装）",
            MF_STRING | MF_GRAYED,
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        item(ID_EXIT, "終了(&X)", MF_STRING);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let r = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
            pt.x,
            pt.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        r.0 as usize
    };
    match choice {
        ID_SHOW => with_state(|s| s.show_window()),
        ID_OPEN_LOGS => open_logs(),
        ID_AUTOSTART => {
            if let Err(e) = autostart::set(!autostart_on) {
                log::error!("autostart: {e}");
            }
        }
        ID_EXIT => with_state(|s| s.exit()),
        _ => {}
    }
}

fn open_logs() {
    if let Ok(dir) = config::log_dir() {
        let _ = Command::new("explorer.exe").arg(dir).spawn();
    }
}

fn config_mtime() -> Option<SystemTime> {
    config::config_path()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

/// Restores and focuses an existing window process's window. False when
/// there is none.
pub fn focus_existing_window() -> bool {
    let hwnd = unsafe { FindWindowW(None, &HSTRING::from(WINDOW_TITLE)) };
    match hwnd {
        Ok(hwnd) if !hwnd.is_invalid() => unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
            true
        },
        _ => false,
    }
}

impl State {
    /// Brings the existing window to the front, or starts the window process.
    fn show_window(&mut self) {
        let alive = self
            .window
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        if alive && focus_existing_window() {
            return;
        }
        match Command::new(&self.exe).arg("--window").spawn() {
            Ok(child) => self.window = Some(child),
            Err(e) => log::error!("start window: {e}"),
        }
    }

    fn tick(&mut self) {
        if self.config_checked.elapsed() >= CONFIG_POLL {
            self.config_checked = Instant::now();
            let mtime = config_mtime();
            if mtime != self.config_mtime {
                self.config_mtime = mtime;
                self.reload_config();
            }
        }
        if let Some(c) = self.window.as_mut()
            && !matches!(c.try_wait(), Ok(None))
        {
            self.window = None;
        }

        let latest = self.sampler.shared.lock().unwrap().latest().map(|s| {
            (
                s.commit_usage(),
                s.memory.commit_charge,
                s.memory.commit_limit,
                s.temp_margin_c,
            )
        });
        if let Some((usage, charge, limit, margin)) = latest {
            let level = AlertLevel::from_commit_usage(usage, &self.cfg.alert)
                .max(AlertLevel::from_temp_margin(margin, &self.cfg.alert));
            let key = (level, (usage * 20.0) as u8);
            if self.last_key != Some(key) {
                self.last_key = Some(key);
                self.update_icon(level, usage, charge, limit);
            }
            if self.notifier.observe(level) {
                self.balloon(
                    "CommitWatch: コミット不足",
                    &format!(
                        "Commit Usage {:.0}%（{} / {}）。RAM が空いていてもメモリ確保が失敗する可能性があります。",
                        usage * 100.0,
                        fmt_gib(charge),
                        fmt_gib(limit)
                    ),
                );
            }
        }

        self.drain_minutes();
    }

    fn reload_config(&mut self) {
        match Config::load() {
            Ok(cfg) => {
                if let Ok(dir) = config::log_dir() {
                    self.log = CsvLog::new(dir, cfg.log.retention_days, cfg.general.top_processes);
                }
                self.cfg = cfg;
                self.last_key = None; // re-evaluate the icon with new thresholds
            }
            Err(e) => log::error!("reload config: {e}"),
        }
    }

    fn drain_minutes(&mut self) {
        while let Ok(rec) = self.sampler.minutes.try_recv() {
            if self.cfg.log.enabled
                && let Err(e) = self.log.append(&rec)
            {
                log::error!("log: {e}");
            }
        }
    }

    fn update_icon(&mut self, level: AlertLevel, usage: f64, charge: u64, limit: u64) {
        let new_icon = make_icon(level, usage);
        if new_icon.is_invalid() {
            return;
        }
        let old = std::mem::replace(&mut self.icon, new_icon);
        self.nid.hIcon = self.icon;
        self.nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        copy_wide(
            &mut self.nid.szTip,
            &format!(
                "CommitWatch  {}  Commit {:.0}% ({} / {})",
                level.label_ja(),
                usage * 100.0,
                fmt_gib(charge),
                fmt_gib(limit)
            ),
        );
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &self.nid);
            let _ = DestroyIcon(old);
        }
    }

    fn balloon(&mut self, title: &str, text: &str) {
        let mut nid = self.nid;
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        copy_wide(&mut nid.szInfoTitle, title);
        copy_wide(&mut nid.szInfo, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    fn re_add(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &self.nid);
            self.nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &self.nid);
        }
    }

    fn exit(&mut self) {
        if let Some(mut c) = self.window.take() {
            let _ = c.kill();
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.nid);
        }
        self.sampler.stop();
        self.drain_minutes();
        unsafe {
            let _ = DestroyIcon(self.icon);
            PostQuitMessage(0);
        }
    }
}

fn make_icon(level: AlertLevel, usage: f64) -> HICON {
    let image = icon::ico_image(ICON_SIZE, level, usage);
    unsafe {
        CreateIconFromResourceEx(
            &image,
            true,
            0x0003_0000,
            ICON_SIZE as i32,
            ICON_SIZE as i32,
            IMAGE_FLAGS(0),
        )
        .unwrap_or_default()
    }
}

fn copy_wide(dst: &mut [u16], text: &str) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let n = wide.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&wide[..n]);
    dst[n..].fill(0);
}
