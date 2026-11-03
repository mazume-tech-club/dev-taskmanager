//! User settings. The TOML file is the source of truth; the settings tab in
//! the window edits the same file.
//!
//! Location: `%LOCALAPPDATA%\CommitWatch\config.toml`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "CommitWatch";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub alert: AlertConfig,
    pub log: LogConfig,
    pub update: UpdateConfig,
    pub general: GeneralConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertConfig {
    /// Commit Usage (0.0..=1.0) at which Alert Level becomes Warning.
    pub warning_commit_usage: f64,
    /// Commit Usage at which Alert Level becomes Critical.
    pub critical_commit_usage: f64,
    /// Margin to TjMax in Celsius at which temperature becomes Warning.
    pub warning_temp_margin_c: f64,
    /// Margin to TjMax in Celsius at which temperature becomes Critical.
    pub critical_temp_margin_c: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    pub enabled: bool,
    /// Daily files older than this are deleted.
    pub retention_days: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    pub enabled: bool,
    pub check_interval_hours: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub autostart: bool,
    /// Number of Top Processes shown and logged.
    pub top_processes: usize,
    /// Seconds between Top Processes refreshes (totals refresh every second).
    pub process_interval_secs: u64,
    /// Seconds of history kept for the window's graphs.
    pub history_secs: usize,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            warning_commit_usage: 0.85,
            critical_commit_usage: 0.95,
            warning_temp_margin_c: 15.0,
            critical_temp_margin_c: 5.0,
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 30,
        }
    }
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_hours: 24,
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            autostart: false,
            top_processes: 5,
            process_interval_secs: 5,
            history_secs: 600,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            alert: AlertConfig::default(),
            log: LogConfig::default(),
            update: UpdateConfig::default(),
            general: GeneralConfig::default(),
        }
    }
}

/// `%LOCALAPPDATA%\CommitWatch`, created on demand.
pub fn data_dir() -> Result<PathBuf> {
    let base = dirs::data_local_dir().context("no local data directory")?;
    let dir = base.join(APP_NAME);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("config.toml"))
}

pub fn log_dir() -> Result<PathBuf> {
    let dir = data_dir()?.join("logs");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

impl Config {
    /// Loads the file, or returns defaults when it does not exist yet. A file
    /// that fails to parse is reported, not silently replaced.
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parse {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&config_path()?)
    }

    #[allow(dead_code)] // used by the settings tab
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).context("serialize config")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
    }

    #[allow(dead_code)] // used by the settings tab
    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_design() {
        let c = Config::default();
        assert_eq!(c.alert.warning_commit_usage, 0.85);
        assert_eq!(c.alert.critical_commit_usage, 0.95);
        assert_eq!(c.log.retention_days, 30);
        assert_eq!(c.update.check_interval_hours, 24);
        assert_eq!(c.general.top_processes, 5);
        assert_eq!(c.general.history_secs, 600);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = std::env::temp_dir().join(format!("commitwatch-cfg-{}", std::process::id()));
        let path = dir.join("nope").join("config.toml");
        assert_eq!(Config::load_from(&path).unwrap(), Config::default());
    }

    #[test]
    fn round_trip_and_partial_file() {
        let dir = std::env::temp_dir().join(format!("commitwatch-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut c = Config::default();
        c.alert.warning_commit_usage = 0.8;
        c.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), c);

        std::fs::write(&path, "[log]\nretention_days = 7\n").unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.log.retention_days, 7);
        assert_eq!(loaded.alert, AlertConfig::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
