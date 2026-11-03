//! Alert Level: Normal / Warning / Critical, derived from Commit Usage and
//! (when a Sensor Provider exists) the margin to TjMax. The tray icon colour
//! and the one-shot notification follow the worse of the two.

use crate::config::AlertConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertLevel {
    Normal,
    Warning,
    Critical,
}

impl AlertLevel {
    pub fn from_commit_usage(usage: f64, cfg: &AlertConfig) -> Self {
        if usage >= cfg.critical_commit_usage {
            Self::Critical
        } else if usage >= cfg.warning_commit_usage {
            Self::Warning
        } else {
            Self::Normal
        }
    }

    /// `margin_c` is TjMax minus Package Temperature. `None` when unknown.
    pub fn from_temp_margin(margin_c: Option<f64>, cfg: &AlertConfig) -> Self {
        match margin_c {
            Some(m) if m <= cfg.critical_temp_margin_c => Self::Critical,
            Some(m) if m <= cfg.warning_temp_margin_c => Self::Warning,
            _ => Self::Normal,
        }
    }

    pub fn label_ja(self) -> &'static str {
        match self {
            Self::Normal => "正常",
            Self::Warning => "警告",
            Self::Critical => "危険",
        }
    }
}

/// Decides when the one-shot notification fires: once on entering Critical,
/// and again only after the level has dropped below Critical in between.
#[derive(Debug, Default)]
pub struct Notifier {
    notified: bool,
}

impl Notifier {
    /// Returns true when a notification should be shown for this level.
    pub fn observe(&mut self, level: AlertLevel) -> bool {
        match level {
            AlertLevel::Critical if !self.notified => {
                self.notified = true;
                true
            }
            AlertLevel::Critical => false,
            _ => {
                self.notified = false;
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_thresholds() {
        let cfg = AlertConfig::default();
        assert_eq!(AlertLevel::from_commit_usage(0.5, &cfg), AlertLevel::Normal);
        assert_eq!(
            AlertLevel::from_commit_usage(0.85, &cfg),
            AlertLevel::Warning
        );
        assert_eq!(
            AlertLevel::from_commit_usage(0.949, &cfg),
            AlertLevel::Warning
        );
        assert_eq!(
            AlertLevel::from_commit_usage(0.95, &cfg),
            AlertLevel::Critical
        );
    }

    #[test]
    fn temp_margin_thresholds() {
        let cfg = AlertConfig::default();
        assert_eq!(AlertLevel::from_temp_margin(None, &cfg), AlertLevel::Normal);
        assert_eq!(
            AlertLevel::from_temp_margin(Some(40.0), &cfg),
            AlertLevel::Normal
        );
        assert_eq!(
            AlertLevel::from_temp_margin(Some(15.0), &cfg),
            AlertLevel::Warning
        );
        assert_eq!(
            AlertLevel::from_temp_margin(Some(5.0), &cfg),
            AlertLevel::Critical
        );
    }

    #[test]
    fn worse_level_orders() {
        assert!(AlertLevel::Critical > AlertLevel::Warning);
        assert!(AlertLevel::Warning > AlertLevel::Normal);
        assert_eq!(
            AlertLevel::Warning.max(AlertLevel::Critical),
            AlertLevel::Critical
        );
    }

    #[test]
    fn notifier_fires_once_per_critical_episode() {
        let mut n = Notifier::default();
        assert!(!n.observe(AlertLevel::Normal));
        assert!(n.observe(AlertLevel::Critical));
        assert!(!n.observe(AlertLevel::Critical));
        assert!(!n.observe(AlertLevel::Warning));
        assert!(n.observe(AlertLevel::Critical));
    }
}
