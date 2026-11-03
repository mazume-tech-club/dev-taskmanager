#![windows_subsystem = "windows"]

mod alert;
mod app;
mod autostart;
mod config;
mod cpu;
mod icon;
mod logger;
mod metrics;
mod minute;
mod pdh;
mod sample;
mod shm;
mod thermal;
mod tray;
mod update;

use alert::{AlertLevel, Notifier};
use anyhow::{Context, Result};
use config::Config;
use metrics::fmt_gib;
use windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

/// Process roles:
/// - no argument / `--hidden`: the resident process (tray, sampler, log);
///   `--hidden` does not open the window (used by autostart).
/// - `--window`: the window process, started by the resident.
/// - `--probe`, `--headless [secs]`, `--check-update`, `--apply-update`: console diagnostics.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--probe") => {
            attach_console();
            probe()
        }
        Some("--headless") => {
            attach_console();
            let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            headless(secs)
        }
        Some("--check-update") => {
            attach_console();
            check_update()
        }
        Some("--apply-update") => {
            attach_console();
            update::detach_inherited_stdio();
            let staged = update::staged().ok_or_else(|| anyhow::anyhow!("no staged update"))?;
            update::apply(&staged)?;
            println!("applied {}; restarting", staged.version);
            Ok(())
        }
        Some("--after-update") => {
            let pid: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            update::finish_after_update(pid);
            resident(false)
        }
        Some("--window") => window(),
        Some("--hidden") => resident(true),
        _ => resident(false),
    }
}

/// With the windows subsystem there is no console; borrow the parent's so
/// `--probe` and `--headless` can print when run from a terminal.
fn attach_console() {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    env_logger::init();
}

/// A tray app has no console, so panics go to `%LOCALAPPDATA%\CommitWatch\crash.log`.
fn install_panic_log() {
    let Ok(dir) = config::data_dir() else { return };
    let path = dir.join("crash.log");
    std::panic::set_hook(Box::new(move |info| {
        let line = format!(
            "{} {}
",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            info
        );
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
        }
    }));
}

/// True when this is the first resident process. The mutex lives as long as
/// the process.
fn single_instance() -> bool {
    let handle = unsafe { CreateMutexW(None, false, w!("Local\\CommitWatch.single-instance")) };
    match handle {
        Ok(_) => windows::core::Error::from_thread().code() != ERROR_ALREADY_EXISTS.to_hresult(),
        Err(_) => true,
    }
}

fn resident(start_hidden: bool) -> Result<()> {
    install_panic_log();
    update::detach_inherited_stdio();
    update::cleanup_old_exe();
    let exe = std::env::current_exe().context("current_exe")?;
    if !single_instance() {
        // Already running: a second launch just shows the window.
        if !start_hidden && !tray::focus_existing_window() {
            let _ = std::process::Command::new(&exe).arg("--window").spawn();
        }
        return Ok(());
    }
    let cfg = Config::load()?;
    let log = logger::CsvLog::new(
        config::log_dir()?,
        cfg.log.retention_days,
        cfg.general.top_processes,
    );
    let sampler = sample::Sampler::start(cfg.clone());
    tray::run(sampler, log, cfg, exe, !start_hidden);
    Ok(())
}

fn window() -> Result<()> {
    install_panic_log();
    update::detach_inherited_stdio();
    let cfg = Config::load()?;
    if shm::Reader::open()?.is_none() {
        // No resident process: start one hidden; the window will attach.
        let exe = std::env::current_exe().context("current_exe")?;
        let _ = std::process::Command::new(exe).arg("--hidden").spawn();
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(tray::WINDOW_TITLE)
            .with_inner_size(app::NORMAL_SIZE)
            .with_min_inner_size([320.0, 140.0])
            .with_icon(app::window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        tray::WINDOW_TITLE,
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, cfg)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}

/// Console diagnostic: check, download and verify, but do not apply.
fn check_update() -> Result<()> {
    let cfg = Config::load()?;
    let endpoint = update::endpoint(&cfg.update);
    println!("current version: {}", update::CURRENT_VERSION);
    println!("endpoint:        {endpoint}");
    match update::check_and_stage(&endpoint)? {
        update::Outcome::UpToDate => println!("up to date"),
        update::Outcome::Staged(s) => {
            println!("staged {} at {}", s.version, s.path.display());
            if !s.notes.is_empty() {
                println!("notes: {}", s.notes);
            }
        }
    }
    Ok(())
}

/// Runs the sampler and the log without a window. `secs == 0` runs until
/// Ctrl+C. Each second prints one line; each finished minute is logged.
fn headless(secs: u64) -> Result<()> {
    let cfg = Config::load()?;
    let log_dir = config::log_dir()?;
    println!("config: {}", config::config_path()?.display());
    println!("logs:   {}", log_dir.display());
    let mut log = logger::CsvLog::new(log_dir, cfg.log.retention_days, cfg.general.top_processes);
    let mut notifier = Notifier::default();
    let mut sampler = sample::Sampler::start(cfg.clone());
    let started = std::time::Instant::now();
    let mut printed = 0usize;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        {
            let shared = sampler.shared.lock().unwrap();
            if let Some(e) = &shared.last_error {
                eprintln!("sampler error: {e}");
            }
            if shared.history.len() > printed {
                let s = shared.latest().unwrap();
                let level = AlertLevel::from_commit_usage(s.commit_usage(), &cfg.alert)
                    .max(AlertLevel::from_temp_margin(s.temp_margin_c, &cfg.alert));
                if notifier.observe(level) {
                    println!(
                        "*** NOTIFY: Commit Usage {:.1}% ***",
                        s.commit_usage() * 100.0
                    );
                }
                println!(
                    "{} [{}] commit {} / {} ({:.1}%)  avail {}  vm {}  kernel {}  cpu {:>5.1}%  clock {:>4.0} MHz  tz {}",
                    s.at.format("%H:%M:%S"),
                    level.label_ja(),
                    fmt_gib(s.memory.commit_charge),
                    fmt_gib(s.memory.commit_limit),
                    s.commit_usage() * 100.0,
                    fmt_gib(s.memory.physical_available),
                    fmt_gib(s.vm_memory),
                    fmt_gib(s.kernel_commit),
                    s.cpu_total * 100.0,
                    if s.clock_mhz.is_empty() {
                        0.0
                    } else {
                        s.clock_mhz.iter().sum::<f64>() / s.clock_mhz.len() as f64
                    },
                    s.thermal_zone_c
                        .map(|t| format!("{t:.1}C"))
                        .unwrap_or_else(|| "-".into()),
                );
                printed = shared.history.len();
            }
        }
        while let Ok(rec) = sampler.minutes.try_recv() {
            if cfg.log.enabled {
                log.append(&rec)?;
            }
            println!(
                "--- minute {} logged ({} samples)",
                rec.minute.format("%H:%M"),
                rec.samples
            );
        }
        if secs > 0 && started.elapsed().as_secs() >= secs {
            break;
        }
    }
    sampler.stop();
    while let Ok(rec) = sampler.minutes.try_recv() {
        if cfg.log.enabled {
            log.append(&rec)?;
        }
        println!(
            "--- partial minute {} logged ({} samples)",
            rec.minute.format("%H:%M"),
            rec.samples
        );
    }
    Ok(())
}

fn probe() -> Result<()> {
    let m = metrics::system_memory()?;
    println!("Commit Charge      {:>12}", fmt_gib(m.commit_charge));
    println!("Commit Limit       {:>12}", fmt_gib(m.commit_limit));
    println!("Commit Usage       {:>11.1}%", m.commit_usage() * 100.0);
    println!("Physical Total     {:>12}", fmt_gib(m.physical_total));
    println!("Available          {:>12}", fmt_gib(m.physical_available));
    println!("  Free             {:>12}", fmt_gib(m.free));
    println!("  Standby          {:>12}", fmt_gib(m.standby));
    println!("Modified           {:>12}", fmt_gib(m.modified));
    println!(
        "Page File          {:>12} total, {} in use, automatic={}",
        fmt_gib(m.page_file_total()),
        fmt_gib(m.page_file_in_use()),
        m.page_file_automatic
    );
    for p in &m.page_files {
        println!(
            "  {}  {} / {} (peak {})",
            p.path,
            fmt_gib(p.in_use),
            fmt_gib(p.total),
            fmt_gib(p.peak)
        );
    }

    let procs = metrics::processes()?;
    let sum: u64 = procs.iter().map(|p| p.commit).sum();
    let vm: u64 = procs.iter().filter(|p| p.is_vm()).map(|p| p.commit).sum();
    println!(
        "\nProcesses: {}  (sum of Process Commit {}, VM Memory {})",
        procs.len(),
        fmt_gib(sum),
        fmt_gib(vm)
    );
    println!(
        "{:<6} {:<32} {:>12} {:>12}",
        "PID", "Name", "Commit", "WorkingSet"
    );
    for p in metrics::top_by_commit(procs, 10) {
        println!(
            "{:<6} {:<32} {:>12} {:>12}",
            p.pid,
            p.name,
            fmt_gib(p.commit),
            fmt_gib(p.working_set)
        );
    }

    let mut sampler = cpu::CpuSampler::new()?;
    let mut clocks = cpu::ClockSampler::new().ok();
    let mut thermal = thermal::ThermalSampler::new()?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    let load = sampler.sample()?;
    println!(
        "
CPU Load (1s): total {:.1}%  per logical: {}",
        load.total * 100.0,
        load.per_logical
            .iter()
            .map(|v| format!("{:.0}", v * 100.0))
            .collect::<Vec<_>>()
            .join(" ")
    );
    match clocks.as_mut().map(|c| c.sample()) {
        Some(Ok(mhz)) => println!(
            "Effective Clock MHz: {}",
            mhz.iter()
                .map(|m| format!("{m:.0}"))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Some(Err(e)) => println!("Effective Clock: unavailable ({e})"),
        None => println!("Effective Clock: counters not available"),
    }
    match thermal.as_mut() {
        None => println!("Thermal Zone: none on this machine"),
        Some(t) => {
            for z in t.sample()? {
                println!("Thermal Zone {}: {:.1} C", z.name, z.celsius);
            }
        }
    }
    println!("Core Temperature: unavailable (Sensor Provider is a later phase)");
    Ok(())
}
