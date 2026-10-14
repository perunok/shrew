//! Files shared between the app (which runs the endpoint) and the stdio bridge (which assistants launch), in
//! `~/.config/shrew/mcp/` (see [`mcp_dir`]):
//!
//! - `settings.json` (0600): `{port, token}`. Whether MCP is on lives in shrew's config (`[features] mcp`).
//! - `endpoint.json` (0600): `{url, token}`; present only while an endpoint runs, so the bridge can find it.
//! - `launch.json`: `{enabled}`; no secrets. Lets the bridge start shrew when it is not running, and refuse when MCP
//!   access is turned off.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 7356;
/// Tokens look like `shrew_` + 64 hex characters.
pub const TOKEN_PREFIX: &str = "shrew_";

/// `~/.config/shrew/mcp` (or the platform's config dir).
pub fn mcp_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("shrew").join("mcp"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Preferred port (the next few are tried when it is taken); 0 = any free port.
    pub port: u16,
    pub token: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { port: DEFAULT_PORT, token: new_token() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub url: String,
    pub token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub enabled: bool,
}

/// `shrew_` + 64 hex characters (32 random bytes from the OS).
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    let mut s = String::with_capacity(TOKEN_PREFIX.len() + 64);
    s.push_str(TOKEN_PREFIX);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Writes through a temp file created owner-only (0600 on Unix) and renamed over the target, so a token is never
/// readable by other users, not even briefly.
pub fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("no parent folder"))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).expect("plain structs serialise")
}

/// The saved settings, if present and valid.
pub fn read_settings(dir: &Path) -> Option<Settings> {
    read_json::<Settings>(&dir.join("settings.json"))
        .filter(|s| s.token.starts_with(TOKEN_PREFIX) && s.token.len() == TOKEN_PREFIX.len() + 64)
}

/// The saved settings, or fresh ones (new token) when missing or invalid. Does not write anything.
pub fn load_settings(dir: &Path) -> Settings {
    read_settings(dir).unwrap_or_default()
}

pub fn save_settings(dir: &Path, s: &Settings) -> io::Result<()> {
    write_private(&dir.join("settings.json"), &to_json(s))
}

pub fn read_endpoint(dir: &Path) -> Option<Endpoint> {
    read_json(&dir.join("endpoint.json"))
}

pub fn write_endpoint(dir: &Path, e: &Endpoint) -> io::Result<()> {
    write_private(&dir.join("endpoint.json"), &to_json(e))
}

/// Removes `endpoint.json` if it still names `ours` (another shrew may have taken over since).
pub fn remove_endpoint(dir: &Path, ours: &Endpoint) {
    if read_endpoint(dir).is_some_and(|e| e == *ours) {
        let _ = fs::remove_file(dir.join("endpoint.json"));
    }
}

/// Missing or unreadable means off: the bridge never starts shrew unless the user turned MCP on.
pub fn read_launch(dir: &Path) -> Launch {
    read_json(&dir.join("launch.json")).unwrap_or(Launch { enabled: false })
}

pub fn write_launch(dir: &Path, l: Launch) -> io::Result<()> {
    write_private(&dir.join("launch.json"), &to_json(&l))
}
