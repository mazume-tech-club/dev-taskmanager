//! Log: Minute Records as daily CSV files, UTF-8 with BOM so Excel opens
//! them directly. `logs\YYYY-MM-DD.csv`; files older than the retention are
//! deleted when a new day starts.

use crate::minute::MinuteRecord;
use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const BOM: &[u8] = b"\xEF\xBB\xBF";
const MIB: f64 = 1024.0 * 1024.0;

pub struct CsvLog {
    dir: PathBuf,
    retention_days: u32,
    top_n: usize,
    last_day: Option<NaiveDate>,
}

impl CsvLog {
    pub fn new(dir: PathBuf, retention_days: u32, top_n: usize) -> Self {
        Self {
            dir,
            retention_days,
            top_n,
            last_day: None,
        }
    }

    pub fn header(top_n: usize) -> String {
        let mut cols = vec![
            "time",
            "samples",
            "commit_charge_avg_mb",
            "commit_charge_max_mb",
            "commit_limit_mb",
            "commit_usage_avg_pct",
            "commit_usage_max_pct",
            "available_avg_mb",
            "available_min_mb",
            "free_avg_mb",
            "standby_avg_mb",
            "pagefile_used_avg_mb",
            "pagefile_total_mb",
            "kernel_commit_avg_mb",
            "vm_memory_avg_mb",
            "vm_memory_max_mb",
            "cpu_load_avg_pct",
            "cpu_load_max_pct",
            "clock_avg_mhz",
            "thermal_zone_avg_c",
            "thermal_zone_max_c",
            "package_temp_avg_c",
            "package_temp_max_c",
            "core_temp_max_c",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        for i in 1..=top_n {
            cols.push(format!("top{i}_name"));
            cols.push(format!("top{i}_commit_mb"));
        }
        cols.join(",")
    }

    pub fn row(rec: &MinuteRecord, top_n: usize) -> String {
        let mb = |b: f64| format!("{:.1}", b / MIB);
        let pct = |r: f64| format!("{:.1}", r * 100.0);
        let opt = |v: Option<f64>| v.map(|t| format!("{t:.1}")).unwrap_or_default();
        let mut cols = vec![
            rec.minute.format("%Y-%m-%d %H:%M").to_string(),
            rec.samples.to_string(),
            mb(rec.commit_charge_avg),
            mb(rec.commit_charge_max as f64),
            mb(rec.commit_limit as f64),
            pct(rec.commit_usage_avg),
            pct(rec.commit_usage_max),
            mb(rec.available_avg),
            mb(rec.available_min as f64),
            mb(rec.free_avg),
            mb(rec.standby_avg),
            mb(rec.page_file_used_avg),
            mb(rec.page_file_total as f64),
            mb(rec.kernel_commit_avg),
            mb(rec.vm_memory_avg),
            mb(rec.vm_memory_max as f64),
            pct(rec.cpu_avg),
            pct(rec.cpu_max),
            format!("{:.0}", rec.clock_avg_mhz),
            opt(rec.thermal_zone_avg_c),
            opt(rec.thermal_zone_max_c),
            opt(rec.package_temp_avg_c),
            opt(rec.package_temp_max_c),
            opt(rec.core_temp_max_c),
        ];
        for i in 0..top_n {
            match rec.top_processes.get(i) {
                Some(p) => {
                    cols.push(csv_quote(&p.name));
                    cols.push(mb(p.commit as f64));
                }
                None => {
                    cols.push(String::new());
                    cols.push(String::new());
                }
            }
        }
        cols.join(",")
    }

    pub fn path_for(&self, day: NaiveDate) -> PathBuf {
        self.dir.join(format!("{}.csv", day.format("%Y-%m-%d")))
    }

    /// Appends one Minute Record, creating the day's file (with BOM and
    /// header) when needed, and pruning old files on the first write of a day.
    pub fn append(&mut self, rec: &MinuteRecord) -> Result<()> {
        let day = rec.minute.date_naive();
        if self.last_day != Some(day) {
            self.prune(day);
            self.last_day = Some(day);
        }
        let path = self.path_for(day);
        self.rotate_if_header_changed(&path);
        let is_new = !path.exists()
            || std::fs::metadata(&path)
                .map(|m| m.len() == 0)
                .unwrap_or(true);
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        if is_new {
            f.write_all(BOM)?;
            f.write_all(Self::header(self.top_n).as_bytes())?;
            f.write_all(b"\r\n")?;
        }
        f.write_all(Self::row(rec, self.top_n).as_bytes())?;
        f.write_all(b"\r\n")?;
        Ok(())
    }

    /// When the column layout changed since the file was started (an app
    /// update mid-day, or a different Top N), the old file is kept as
    /// `YYYY-MM-DD.1.csv` and a fresh one begins, so rows never misalign
    /// with their header.
    fn rotate_if_header_changed(&self, path: &Path) {
        let Ok(mut f) = std::fs::File::open(path) else {
            return;
        };
        let mut first = String::new();
        {
            use std::io::BufRead;
            let mut r = std::io::BufReader::new(&mut f);
            if r.read_line(&mut first).is_err() {
                return;
            }
        }
        drop(f);
        let first = first.trim_start_matches('\u{feff}').trim_end();
        if first.is_empty() || first == Self::header(self.top_n) {
            return;
        }
        for n in 1..100 {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("log");
            let backup = path.with_file_name(format!("{stem}.{n}.csv"));
            if !backup.exists() {
                let _ = std::fs::rename(path, backup);
                return;
            }
        }
    }

    /// Deletes `YYYY-MM-DD.csv` files older than the retention window.
    pub fn prune(&self, today: NaiveDate) {
        let cutoff = today - Duration::days(i64::from(self.retention_days));
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            if let Some(day) = day_of(&entry.path())
                && day < cutoff
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

fn day_of(path: &Path) -> Option<NaiveDate> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".csv")?;
    NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()
}

fn csv_quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minute::MinuteAccumulator;
    use crate::minute::tests::sample_at;
    use chrono::{Local, TimeZone};

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("commitwatch-log-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn record(at: chrono::DateTime<Local>) -> MinuteRecord {
        let mut acc = MinuteAccumulator::default();
        acc.push(&sample_at(at, 60 * 1024 * 1024, 0.25));
        acc.flush().unwrap()
    }

    #[test]
    fn header_and_row_have_same_column_count() {
        let rec = record(Local.with_ymd_and_hms(2026, 11, 3, 9, 5, 0).unwrap());
        let h = CsvLog::header(5).split(',').count();
        let r = CsvLog::row(&rec, 5).split(',').count();
        assert_eq!(h, r);
        assert_eq!(h, 24 + 10);
        assert!(CsvLog::row(&rec, 5).starts_with("2026-11-03 09:05,1,60.0,60.0,"));
    }

    #[test]
    fn writes_bom_header_and_appends() {
        let dir = temp_dir("write");
        let mut log = CsvLog::new(dir.clone(), 30, 2);
        let t = Local.with_ymd_and_hms(2026, 11, 3, 9, 5, 0).unwrap();
        log.append(&record(t)).unwrap();
        log.append(&record(t + Duration::minutes(1))).unwrap();
        let bytes = std::fs::read(dir.join("2026-11-03.csv")).unwrap();
        assert!(bytes.starts_with(BOM));
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], CsvLog::header(2));
        assert!(lines[2].starts_with("2026-11-03 09:06,"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotates_when_header_changes() {
        let dir = temp_dir("rotate");
        let path = dir.join("2026-11-03.csv");
        std::fs::write(
            &path,
            "\u{feff}time,samples,old_column\r\n2026-11-03 09:00,1,2\r\n",
        )
        .unwrap();
        let mut log = CsvLog::new(dir.clone(), 30, 2);
        log.append(&record(
            Local.with_ymd_and_hms(2026, 11, 3, 9, 5, 0).unwrap(),
        ))
        .unwrap();
        assert!(dir.join("2026-11-03.1.csv").exists());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.trim_start_matches('\u{feff}')
                .starts_with(&CsvLog::header(2))
        );
        // a second append with the same layout must not rotate again
        log.append(&record(
            Local.with_ymd_and_hms(2026, 11, 3, 9, 6, 0).unwrap(),
        ))
        .unwrap();
        assert!(!dir.join("2026-11-03.2.csv").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prunes_old_files_only() {
        let dir = temp_dir("prune");
        for name in [
            "2026-09-01.csv",
            "2026-10-20.csv",
            "notes.txt",
            "bad-name.csv",
        ] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        let log = CsvLog::new(dir.clone(), 30, 5);
        log.prune(NaiveDate::from_ymd_opt(2026, 11, 3).unwrap());
        assert!(!dir.join("2026-09-01.csv").exists());
        assert!(dir.join("2026-10-20.csv").exists());
        assert!(dir.join("notes.txt").exists());
        assert!(dir.join("bad-name.csv").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quotes_names_with_commas() {
        assert_eq!(csv_quote("plain.exe"), "plain.exe");
        assert_eq!(csv_quote("a,b"), "\"a,b\"");
        assert_eq!(csv_quote("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
