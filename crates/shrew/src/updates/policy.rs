//! When to ask GitHub and when to tell the user, like Slinger:
//!
//! - **Startup**: every start, a few seconds after the first frame, while automatic checks are on (`update_check`,
//!   `[update] check`). Silent unless a newer version exists; then it is announced at every start until installed.
//! - **Daily**: while shrew runs, once a day after the last successful check (an event-loop deadline, no polling).
//!   Announces a version at most once per day.
//! - **Manual** (Help > Check for Updates…, Ctrl+Shift+U, `:checkupdate`): always answers: a newer version, up to
//!   date, or why it could not check. Runs whatever the setting says.
//! - **Settings** (Check now): answers inline in Settings > Updates, no notification.
//!
//! What was seen is kept in `<cache dir>/shrew/update-state.json` (when it last checked, the latest version, which
//! version it announced and when).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Seconds in a day: the daily check's period, and how long one announcement of a version lasts.
pub const DAY: u64 = 24 * 60 * 60;
/// The startup check waits this long after the first frame (startup work goes first).
pub const STARTUP_DELAY: Duration = Duration::from_secs(3);
/// After an automatic check failed (offline...), the next try.
pub const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

/// Who asked for a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Startup,
    Daily,
    Manual,
    Settings,
}

impl Source {
    /// Errors and "up to date" are only told to someone who asked.
    pub fn asked(self) -> bool {
        matches!(self, Source::Manual | Source::Settings)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Unix seconds of the last successful check.
    pub last_checked: Option<u64>,
    /// The latest release that check saw.
    pub latest: Option<String>,
    /// The version last announced, and when.
    pub notified: Option<String>,
    pub notified_at: Option<u64>,
}

impl Prefs {
    pub fn path() -> Option<PathBuf> {
        shrew_update::cache_dir().map(|d| d.join("update-state.json"))
    }

    /// Reads the file; missing or unreadable means "never checked".
    pub fn load(path: &Path) -> Prefs {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Writes it (a temp file first). Best effort: a read-only cache only means checking more often.
    pub fn save(&self, path: &Path) {
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
            std::fs::rename(&tmp, path)
        };
        if let Err(e) = write() {
            log::debug!("update: cannot write {}: {e}", path.display());
        }
    }

    /// Seconds until the daily check is due (0: due now). A last check in the future (the clock went back) counts
    /// as long ago.
    pub fn daily_in(&self, now: u64) -> u64 {
        match self.last_checked {
            Some(t) if t <= now => DAY.saturating_sub(now - t),
            _ => 0,
        }
    }

    /// Whether a check from `source` that found `update` (a version newer than this build, if any) announces it.
    pub fn should_notify(&self, source: Source, update: Option<&str>, now: u64) -> bool {
        let Some(v) = update else { return false };
        match source {
            Source::Startup | Source::Manual => true,
            Source::Settings => false,
            Source::Daily => {
                let told_today =
                    self.notified.as_deref() == Some(v) && self.notified_at.is_some_and(|t| t <= now && now - t < DAY);
                !told_today
            }
        }
    }

    /// Records a successful check at `now`, and the announcement if there was one.
    pub fn checked(&mut self, now: u64, latest: Option<String>, notified: Option<&str>) {
        self.last_checked = Some(now);
        self.latest = latest;
        if let Some(v) = notified {
            self.notified = Some(v.to_string());
            self.notified_at = Some(now);
        }
    }
}

/// Unix seconds now.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// "just now", "3 minutes ago", "yesterday", "5 days ago".
pub fn ago(then: u64, now: u64) -> String {
    let s = now.saturating_sub(then);
    let n = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match s {
        0..60 => "just now".into(),
        60..3600 => n(s / 60, "minute"),
        3600..86400 => n(s / 3600, "hour"),
        86400..172800 => "yesterday".into(),
        _ => n(s / 86400, "day"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1_800_000_000;

    #[test]
    fn startup_and_manual_always_announce_a_newer_version() {
        let mut p = Prefs::default();
        p.checked(T, Some("0.3.0".into()), Some("0.3.0"));
        for s in [Source::Startup, Source::Manual] {
            assert!(p.should_notify(s, Some("0.3.0"), T + 10), "{s:?}: every start until installed");
            assert!(!p.should_notify(s, None, T + 10), "{s:?}: nothing newer, nothing to announce");
        }
        assert!(!p.should_notify(Source::Settings, Some("0.3.0"), T), "Settings answers inline");
    }

    #[test]
    fn the_daily_check_announces_a_version_once_a_day() {
        let mut p = Prefs::default();
        assert!(p.should_notify(Source::Daily, Some("0.3.0"), T), "never told");
        p.checked(T, Some("0.3.0".into()), Some("0.3.0"));
        assert!(!p.should_notify(Source::Daily, Some("0.3.0"), T + 60));
        assert!(!p.should_notify(Source::Daily, Some("0.3.0"), T + DAY - 1));
        assert!(p.should_notify(Source::Daily, Some("0.3.0"), T + DAY), "a day later, again");
        assert!(p.should_notify(Source::Daily, Some("0.3.1"), T + 60), "a newer version at once");
        assert!(p.should_notify(Source::Daily, Some("0.3.0"), T - 10), "clock went back: tell again");
        // a check that found nothing new does not reset the announcement
        p.checked(T + 120, Some("0.3.0".into()), None);
        assert_eq!((p.notified.as_deref(), p.notified_at), (Some("0.3.0"), Some(T)));
    }

    #[test]
    fn the_daily_check_is_due_a_day_after_the_last_one() {
        let mut p = Prefs::default();
        assert_eq!(p.daily_in(T), 0, "never checked");
        p.checked(T, None, None);
        assert_eq!(p.daily_in(T), DAY);
        assert_eq!(p.daily_in(T + 3600), DAY - 3600);
        assert_eq!(p.daily_in(T + DAY), 0);
        assert_eq!(p.daily_in(T + 3 * DAY), 0);
        assert_eq!(p.daily_in(T - 50), 0, "a last check in the future is stale");
    }

    #[test]
    fn prefs_survive_a_round_trip_and_garbage() {
        let dir = std::env::temp_dir().join(format!("shrew-update-prefs-{}", std::process::id()));
        let path = dir.join("nested/update-state.json");
        assert_eq!(Prefs::load(&path), Prefs::default());
        let mut p = Prefs::default();
        p.checked(T, Some("0.2.0".into()), Some("0.2.0"));
        p.save(&path);
        assert_eq!(Prefs::load(&path), p);
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Prefs::load(&path), Prefs::default());
        std::fs::write(&path, r#"{"latest":"0.9.0","future_key":1}"#).unwrap();
        assert_eq!(Prefs::load(&path).latest.as_deref(), Some("0.9.0"), "unknown keys are fine");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ago_reads_well() {
        assert_eq!(ago(T, T + 5), "just now");
        assert_eq!(ago(T, T + 60), "1 minute ago");
        assert_eq!(ago(T, T + 180), "3 minutes ago");
        assert_eq!(ago(T, T + 7200), "2 hours ago");
        assert_eq!(ago(T, T + DAY + 5), "yesterday");
        assert_eq!(ago(T, T + 5 * DAY), "5 days ago");
        assert_eq!(ago(T + 10, T), "just now", "future: no negative ages");
    }
}
