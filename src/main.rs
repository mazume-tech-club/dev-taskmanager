mod cpu;
mod metrics;
mod pdh;
mod thermal;

use anyhow::Result;
use metrics::fmt_gib;

fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--probe") {
        return probe();
    }
    println!(
        "CommitWatch {} (GUI not implemented yet; try --probe)",
        env!("CARGO_PKG_VERSION")
    );
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
