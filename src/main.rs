mod alert;
mod config;
mod cpu;
mod logger;
mod metrics;
mod minute;
mod pdh;
mod sample;
mod thermal;

use alert::{AlertLevel, Notifier};
use anyhow::Result;
use config::Config;
use metrics::fmt_gib;

fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--probe") => probe(),
        Some("--headless") => {
            let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            headless(secs)
        }
        _ => {
            println!(
                "CommitWatch {} (GUI not implemented yet; try --probe or --headless [seconds])",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
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
