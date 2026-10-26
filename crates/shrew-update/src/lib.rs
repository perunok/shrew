//! Self-update from GitHub Releases: check for a newer tag, download the binary for this build's target triple,
//! verify its sha256 and atomically swap it in for the running executable.
//!
//! Nothing here touches the UI and nothing decides *when*: [`Updater`] runs checks and downloads on short-lived
//! background threads, calls a wake callback when something changed, and exposes a small [`Phase`] and the last
//! check's [`Checked`] answer. The app owns the schedule, the notifications and the restart.
//!
//! Release contract (matched by `.github/workflows/release.yml`): tags `vX.Y.Z`; per target a raw binary
//! `shrew-<triple>` (`shrew-<triple>.exe` on Windows) and `shrew-<triple>.sha256` in `sha256sum` format.
//!
//! TLS is pure Rust: rustls with the RustCrypto provider and the Mozilla root store compiled in (no ring/aws-lc).

mod checksum;
mod feed;
mod install;
mod net;
mod updater;
mod version;

use std::{fmt, io, path::PathBuf};

pub use checksum::parse_checksum;
pub use feed::{Checked, Feed, Release, check, release_page};
pub use install::{
    Installed, Staged, Step, Unsupported, apply, cleanup, download, install, install_target, install_to,
    published_sha256, restart, support, support_for,
};
pub use updater::{Options, Phase, Updater};
pub use version::is_newer;

/// GitHub `owner/name` releases are fetched from; `SHREW_UPDATE_REPO` overrides it.
pub const REPO: &str = "perunok/shrew";

/// Target triple this binary was built for (from cargo's `TARGET`), e.g. `x86_64-unknown-linux-gnu`.
pub const TARGET: &str = env!("SHREW_UPDATE_TARGET");

const USER_AGENT: &str = concat!("shrew/", env!("CARGO_PKG_VERSION"), " (+https://github.com/perunok/shrew)");

/// Release asset name of the binary for `target`: `shrew-<triple>`, plus `.exe` on Windows.
pub fn asset_name(target: &str) -> String {
    if target.contains("windows") { format!("shrew-{target}.exe") } else { format!("shrew-{target}") }
}

/// Release asset name of the checksum file for `target`: `shrew-<triple>.sha256`.
pub fn checksum_name(target: &str) -> String {
    format!("shrew-{target}.sha256")
}

/// shrew's folder in the user's cache dir: `~/.cache/shrew` (`$XDG_CACHE_HOME`), `~/Library/Caches/shrew`,
/// `%LOCALAPPDATA%\shrew`.
pub fn cache_dir() -> Option<PathBuf> {
    let env = |key| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        Some(env("HOME")?.join("Library/Caches"))
    } else {
        env("XDG_CACHE_HOME").filter(|p| p.is_absolute()).or_else(|| Some(env("HOME")?.join(".cache")))
    };
    Some(base?.join("shrew"))
}

/// Where downloads wait until they are installed: `<cache dir>/updates`.
pub fn downloads_dir() -> Option<PathBuf> {
    Some(cache_dir()?.join("updates"))
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    /// Network, TLS or HTTP status failure, already phrased for the user.
    Http(String),
    /// The feed or a checksum file could not be understood.
    Feed(String),
    /// The download does not match the published sha256; nothing was replaced.
    Checksum {
        expected: String,
        actual: String,
    },
    /// The executable lives somewhere shrew must not (or cannot) write: a distro package, a read-only dir.
    NotWritable(PathBuf),
    /// The download was cancelled; its partial file is gone.
    Cancelled,
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Http(msg) | Error::Feed(msg) => f.write_str(msg),
            Error::Checksum { expected, actual } => write!(
                f,
                "the download does not match the published sha256 (got {actual}, expected {expected}); it was \
                 discarded and nothing was changed"
            ),
            Error::NotWritable(path) => write!(
                f,
                "{} is not writable by shrew (installed by a package manager?); update shrew with your package manager",
                path.display()
            ),
            Error::Cancelled => f.write_str("download cancelled"),
            Error::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

#[cfg(test)]
mod tests;
