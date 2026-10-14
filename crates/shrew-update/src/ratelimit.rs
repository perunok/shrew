//! At most one automatic check per [`CHECK_INTERVAL`]: a timestamp file in the user's cache dir.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether an automatic check is due (no successful check in the last [`CHECK_INTERVAL`]).
/// With no cache dir it always is; forced checks (the user asked) skip this entirely.
pub fn should_check() -> bool {
    stamp_path().is_none_or(|path| due(&path, SystemTime::now()))
}

/// Records a successful check now. Best effort: a read-only cache only means checking more often.
pub fn mark_checked() {
    if let Some(path) = stamp_path() {
        mark(&path, SystemTime::now());
    }
}

/// `<cache dir>/shrew/update-check`.
pub(crate) fn stamp_path() -> Option<PathBuf> {
    Some(cache_dir()?.join("shrew").join("update-check"))
}

pub(crate) fn due(path: &Path, now: SystemTime) -> bool {
    let Some(last) = fs::read_to_string(path).ok().and_then(|s| s.trim().parse::<u64>().ok()) else { return true };
    let now = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    // A stamp from the future (clock moved back) would block checks for its whole lead: treat it as stale.
    last > now || now - last >= CHECK_INTERVAL.as_secs()
}

pub(crate) fn mark(path: &Path, now: SystemTime) {
    let secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Err(err) = fs::write(path, secs.to_string()) {
        log::debug!("update: cannot write {}: {err}", path.display());
    }
}

fn cache_dir() -> Option<PathBuf> {
    let env = |key| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        Some(env("HOME")?.join("Library/Caches"))
    } else {
        env("XDG_CACHE_HOME").filter(|p| p.is_absolute()).or_else(|| Some(env("HOME")?.join(".cache")))
    }
}
