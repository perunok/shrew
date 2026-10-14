//! Downloading, verifying and swapping in a new binary, and re-executing it.
//!
//! The new binary is streamed into a temp file next to the target (same filesystem, so the final rename is atomic),
//! hashed on the fly and only renamed over the target once its sha256 matches. On Unix the running process keeps its
//! old inode; on Windows the running exe is renamed to `<exe>.old` first (Windows allows renaming, not deleting, a
//! running image) and [`cleanup`] deletes it on the next start.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use sha2::{Digest, Sha256};

use crate::{
    Error, Release, Result,
    checksum::{encode_hex, parse_checksum},
    net,
};

/// Downloads larger than this are refused whatever the feed claims (shrew is a few MB).
const MAX_DOWNLOAD: u64 = 512 << 20;

/// A verified binary in place of the old one; [`restart`] runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The file that was replaced: the executable, or the AppImage it runs from.
    pub path: PathBuf,
    pub version: String,
}

/// The file an update replaces: `$APPIMAGE` when running from an AppImage, else the running executable with
/// symlinks resolved. Errors with [`Error::NotWritable`] for package-managed locations (`/usr/bin`, Nix, Homebrew
/// Cellar, ...) where only the package manager may update shrew.
pub fn install_target() -> Result<PathBuf> {
    let path = match std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()) {
        Some(appimage) => PathBuf::from(appimage),
        None => current_exe()?,
    };
    let path = fs::canonicalize(&path).unwrap_or(path);
    if package_managed(&path) {
        return Err(Error::NotWritable(path));
    }
    Ok(path)
}

/// Updates the running shrew to `release` (see [`install_to`]); `progress(downloaded, total)` is called as bytes
/// arrive (`total` is 0 when unknown). Blocking: run it on a background thread.
pub fn install(release: &Release, progress: impl FnMut(u64, u64)) -> Result<Installed> {
    install_to(release, &install_target()?, progress)
}

/// Downloads `release`, verifies it against its checksum file and atomically replaces `target` with it.
/// On any error `target` is left untouched and the temp file is removed.
pub fn install_to(release: &Release, target: &Path, mut progress: impl FnMut(u64, u64)) -> Result<Installed> {
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().ok_or_else(|| Error::Feed(format!("bad install path {}", target.display())))?;

    // Checksum first: tiny, and a release without one fails before downloading megabytes.
    let asset = release.asset_url.rsplit('/').next().unwrap_or_default();
    let sums = net::get_text(&release.checksum_url, "application/octet-stream", 64 << 10)?
        .ok_or_else(|| Error::Http(format!("release {} has no checksum file", release.tag)))?;
    let expected = parse_checksum(&sums, asset)?;

    let tmp = TempFile(dir.join(format!(".{}.update-{}", name.to_string_lossy(), std::process::id())));
    let mut file =
        OpenOptions::new().write(true).create(true).truncate(true).open(&tmp.0).map_err(|err| match err.kind() {
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => Error::NotWritable(dir.into()),
            _ => err.into(),
        })?;

    let body = net::get(&release.asset_url, "application/octet-stream", Some(Duration::from_secs(30 * 60)))?
        .ok_or_else(|| Error::Http(format!("release {} binary is gone (404)", release.tag)))?;
    let total = body.len.filter(|&n| n > 0).unwrap_or(release.size);
    let limit = if total > 0 { total.min(MAX_DOWNLOAD) } else { MAX_DOWNLOAD };
    let mut reader = body.reader;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 << 10];
    let mut done = 0u64;
    progress(0, total);
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(Error::Http(format!("download interrupted: {err}"))),
        };
        done += n as u64;
        if done > limit {
            return Err(Error::Http(format!("download is larger than the expected {limit} bytes")));
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
        progress(done, total.max(done));
    }
    let actual: [u8; 32] = hasher.finalize().into();
    if actual != expected {
        return Err(Error::Checksum { expected: encode_hex(&expected), actual: encode_hex(&actual) });
    }
    file.sync_all()?;
    set_executable(&file, target)?;
    drop(file);

    replace(&tmp.0, target)?;
    std::mem::forget(tmp);
    // Make the rename itself durable; best effort, not every platform can fsync a directory.
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    log::info!("update: installed {} to {}", release.tag, target.display());
    Ok(Installed { path: target.into(), version: release.version.clone() })
}

/// Removes what a previous update left behind (the Windows `<exe>.old`). Cheap: one unlink, call it at startup.
pub fn cleanup() {
    if let Ok(target) = install_target() {
        let _ = fs::remove_file(old_path(&target));
    }
}

/// Replaces the current process with the installed binary, same arguments. Save state first: on Unix this `exec`s
/// (no destructors run), on Windows it spawns the new process and exits. Only returns on failure.
pub fn restart(installed: &Installed) -> Error {
    let mut cmd = std::process::Command::new(&installed.path);
    cmd.args(std::env::args_os().skip(1));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Error::Io(cmd.exec())
    }
    #[cfg(not(unix))]
    {
        match cmd.spawn() {
            Ok(_) => std::process::exit(0),
            Err(err) => Error::Io(err),
        }
    }
}

/// Deleted on drop unless forgotten: a failed or abandoned download never leaves a stray file behind.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    // Linux reports an already replaced binary as "<path> (deleted)".
    if !exe.exists()
        && let Some(path) = exe.to_str().and_then(|s| s.strip_suffix(" (deleted)"))
    {
        return Ok(PathBuf::from(path));
    }
    Ok(exe)
}

/// Locations owned by a package manager: replacing a file there would desync it from the package database.
pub(crate) fn package_managed(path: &Path) -> bool {
    if !cfg!(unix) {
        return false;
    }
    let path = path.to_string_lossy();
    const SYSTEM: [&str; 8] = ["/usr/", "/bin/", "/sbin/", "/nix/", "/gnu/", "/snap/", "/app/", "/var/lib/flatpak/"];
    (SYSTEM.iter().any(|p| path.starts_with(p)) && !path.starts_with("/usr/local/"))
        || path.contains("/Cellar/")
        || path.contains("/.nix-profile/")
}

#[cfg(unix)]
fn set_executable(file: &File, target: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // Keep the old file's mode, executable wherever it is readable; never carry setuid/setgid over.
    let mode = fs::metadata(target).map_or(0o755, |m| m.permissions().mode() & 0o777);
    file.set_permissions(fs::Permissions::from_mode(mode | 0o100 | ((mode & 0o444) >> 2)))
}

#[cfg(not(unix))]
fn set_executable(_: &File, _: &Path) -> io::Result<()> {
    Ok(())
}

fn old_path(target: &Path) -> PathBuf {
    let mut old = target.as_os_str().to_owned();
    old.push(".old");
    old.into()
}

#[cfg(not(windows))]
fn replace(new: &Path, target: &Path) -> io::Result<()> {
    fs::rename(new, target)
}

#[cfg(windows)]
fn replace(new: &Path, target: &Path) -> io::Result<()> {
    let old = old_path(target);
    let _ = fs::remove_file(&old);
    let moved = match fs::rename(target, &old) {
        Ok(()) => true,
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(err) => return Err(err),
    };
    fs::rename(new, target).inspect_err(|_| {
        if moved {
            let _ = fs::rename(&old, target);
        }
    })?;
    // succeeds when the old binary is not running (e.g. updating another install); otherwise cleanup() removes it
    // at the next start
    let _ = fs::remove_file(&old);
    Ok(())
}
