mod coretemp;
mod cpu;
mod metrics;

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
    std::thread::sleep(std::time::Duration::from_secs(1));
    let load = sampler.sample()?;
    let clocks = cpu::clocks_mhz().unwrap_or_default();
    println!(
        "\nCPU Load (1s): total {:.1}%  per logical: {}",
        load.total * 100.0,
        load.per_logical
            .iter()
            .map(|v| format!("{:.0}", v * 100.0))
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!(
        "Clock MHz (power manager): {}",
        clocks
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    );

    match coretemp::read()? {
        None => println!("\nCore Temp: not running (no shared memory)"),
        Some(ct) => {
            println!(
                "\nCore Temp v{}: {} ({} CPU x {} cores) {:.0} MHz = {:.0} x {:.1}, VID {:.3} V",
                ct.struct_version,
                ct.cpu_name,
                ct.cpu_count,
                ct.cores_per_cpu,
                ct.cpu_speed_mhz,
                ct.fsb_speed_mhz,
                ct.multiplier,
                ct.vid
            );
            println!(
                "  TjMax {:?} C, package {:?} C, margin {:?} C, power {:?} W, TDP {:?} W",
                ct.tj_max_c,
                ct.package_temp_c(0),
                ct.margin_to_tj_max_c(),
                ct.power_w,
                ct.tdp_w
            );
            for (i, t) in ct.core_temps_c.iter().enumerate() {
                println!(
                    "  core {:<2} {:>5.1} C  load {:>3}%",
                    i,
                    t,
                    ct.core_loads_pct.get(i).copied().unwrap_or(0)
                );
            }
        }
    }
    Ok(())
}
