//! Downloading and verifying a new binary, swapping it in for the running one, and re-executing it.
//!
//! Two steps, so the app can ask before replacing anything:
//!
//! 1. [`download`] streams the release binary into `<dir>/shrew-<version>-<triple>.part-<pid>` (the cache dir by
//!    default), then reads it back from disk and compares its sha256 with the release's checksum file before
//!    renaming it to `shrew-<version>-<triple>`: a [`Staged`] update. Cancelling or any error deletes the partial file.
//! 2. [`apply`] copies the staged file into a temp file next to the target (same filesystem, so the final rename is
//!    atomic), hashing it again on the way (the cache may have changed since), and only renames it over the target
//!    when the digest still matches. On Unix the running process keeps its old inode; on Windows the running exe is
//!    renamed to `<exe>.old` first (Windows allows renaming, not deleting, a running image) and [`cleanup`] deletes it
//!    on the next start.
//!
//! [`support`] says up front whether this installation can update itself at all.

use std::{
    ffi::OsStr,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};

use sha2::{Digest, Sha256};

use crate::{
    Error, Release, Result,
    checksum::{encode_hex, parse_checksum},
    net, version,
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

/// A downloaded and verified release binary waiting to be installed ([`apply`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    pub path: PathBuf,
    pub release: Release,
    /// The published sha256 it matched.
    pub sha256: [u8; 32],
}

impl Staged {
    /// The digest as lower-case hex.
    pub fn sha256_hex(&self) -> String {
        encode_hex(&self.sha256)
    }
}

/// Where [`download`] is: bytes so far of `total` (0 when unknown), then the check of what landed on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Downloading { done: u64, total: u64 },
    Verifying,
}

/// Why this installation cannot replace its own binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// Running from cargo's target folder.
    DevBuild(PathBuf),
    /// A package manager owns the file (`/usr/bin`, Nix, Homebrew's Cellar, Flatpak, Snap...).
    PackageManaged(PathBuf),
    /// The folder of the binary cannot be written to.
    NotWritable(PathBuf),
    /// The running executable could not be found.
    NoExecutable(String),
}

impl fmt::Display for Unsupported {
    /// A clause to follow "This installation cannot update itself: ".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unsupported::DevBuild(dir) => write!(
                f,
                "this is a development build (it runs from cargo's target folder {}); rebuild it, or install a release",
                dir.display()
            ),
            Unsupported::PackageManaged(path) => {
                write!(f, "{} belongs to a package manager; update shrew with it", path.display())
            }
            Unsupported::NotWritable(dir) => write!(f, "its folder {} is not writable", dir.display()),
            Unsupported::NoExecutable(err) => write!(f, "shrew cannot find its own executable ({err})"),
        }
    }
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

/// The file this installation would replace, or why it cannot update itself (a development build, a package
/// manager's file, a folder it may not write to). Cheap: a few `stat`s and one probe file.
pub fn support() -> std::result::Result<PathBuf, Unsupported> {
    let path = match install_target() {
        Ok(path) => path,
        Err(Error::NotWritable(path)) => return Err(Unsupported::PackageManaged(path)),
        Err(err) => return Err(Unsupported::NoExecutable(err.to_string())),
    };
    support_for(&path).map(|()| path)
}

/// [`support`] for a given install target.
pub fn support_for(target: &Path) -> std::result::Result<(), Unsupported> {
    if package_managed(target) {
        return Err(Unsupported::PackageManaged(target.into()));
    }
    if let Some(dir) = cargo_target_dir(target) {
        return Err(Unsupported::DevBuild(dir));
    }
    let dir = parent(target);
    if !writable(dir) {
        return Err(Unsupported::NotWritable(dir.into()));
    }
    Ok(())
}

/// Updates the running shrew to `release` (see [`install_to`]); `progress(downloaded, total)` is called as bytes
/// arrive (`total` is 0 when unknown). Blocking: run it on a background thread.
pub fn install(release: &Release, progress: impl FnMut(u64, u64)) -> Result<Installed> {
    install_to(release, &install_target()?, progress)
}

/// Downloads `release` next to `target`, verifies it against its checksum file and atomically replaces `target`
/// with it ([`download`] then [`apply`]). On any error `target` is left untouched and no file is left behind.
pub fn install_to(release: &Release, target: &Path, mut progress: impl FnMut(u64, u64)) -> Result<Installed> {
    let staged = download(release, parent(target), &AtomicBool::new(false), |step| {
        if let Step::Downloading { done, total } = step {
            progress(done, total);
        }
    })?;
    apply(&staged, target).inspect_err(|_| {
        let _ = fs::remove_file(&staged.path);
    })
}

/// Downloads `release` into `dir` and verifies it (see the module docs). `cancel` is polled between chunks: set it
/// and this returns [`Error::Cancelled`] with the partial file deleted. Blocking: run it on a background thread.
pub fn download(release: &Release, dir: &Path, cancel: &AtomicBool, mut progress: impl FnMut(Step)) -> Result<Staged> {
    // Checksum first: tiny, and a release without one fails before downloading megabytes.
    let asset = release.asset_url.rsplit('/').next().unwrap_or_default();
    let expected = published_sha256(release)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }

    fs::create_dir_all(dir).map_err(|err| not_writable(err, dir))?;
    let staged = dir.join(staged_name(&release.version, asset));
    let part = TempFile(part_path(&staged));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&part.0)
        .map_err(|err| not_writable(err, dir))?;

    let body = net::get(&release.asset_url, "application/octet-stream", Some(Duration::from_secs(30 * 60)))?
        .ok_or_else(|| Error::Http(format!("release {} binary is gone (404)", release.tag)))?;
    let total = body.len.filter(|&n| n > 0).unwrap_or(release.size);
    let limit = if total > 0 { total.min(MAX_DOWNLOAD) } else { MAX_DOWNLOAD };
    let mut reader = body.reader;
    let mut buf = vec![0; 64 << 10];
    let mut done = 0u64;
    progress(Step::Downloading { done: 0, total });
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
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
        file.write_all(&buf[..n])?;
        progress(Step::Downloading { done, total: total.max(done) });
    }
    file.sync_all()?;
    drop(file);

    // Verify what is on disk, not what went past: that is what gets installed.
    progress(Step::Verifying);
    let actual = hash_file(&part.0, cancel)?;
    if actual != expected {
        return Err(Error::Checksum { expected: encode_hex(&expected), actual: encode_hex(&actual) });
    }
    fs::rename(&part.0, &staged)?;
    std::mem::forget(part);
    log::info!("update: {} downloaded and verified: {}", release.tag, staged.display());
    Ok(Staged { path: staged, release: release.clone(), sha256: expected })
}

/// The sha256 the release publishes for its binary (its `.sha256` file).
pub fn published_sha256(release: &Release) -> Result<[u8; 32]> {
    let asset = release.asset_url.rsplit('/').next().unwrap_or_default();
    let sums = net::get_text(&release.checksum_url, "application/octet-stream", 64 << 10)?
        .ok_or_else(|| Error::Http(format!("release {} has no checksum file", release.tag)))?;
    parse_checksum(&sums, asset)
}

/// Installs a [`Staged`] update over `target`: copies it next to the target while hashing it again, and renames the
/// copy over the target only when it still matches. Removes the staged file once installed.
pub fn apply(staged: &Staged, target: &Path) -> Result<Installed> {
    let dir = parent(target);
    let name = target.file_name().ok_or_else(|| Error::Feed(format!("bad install path {}", target.display())))?;
    let mut src = File::open(&staged.path).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => {
            Error::Feed(format!("the downloaded update {} is gone: download it again", staged.path.display()))
        }
        _ => err.into(),
    })?;
    let tmp = TempFile(dir.join(format!(".{}.update-{}", name.to_string_lossy(), std::process::id())));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp.0)
        .map_err(|err| not_writable(err, dir))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 << 10];
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        };
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
    }
    let actual: [u8; 32] = hasher.finalize().into();
    if actual != staged.sha256 {
        drop(src);
        let _ = fs::remove_file(&staged.path);
        return Err(Error::Checksum { expected: encode_hex(&staged.sha256), actual: encode_hex(&actual) });
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
    drop(src);
    if staged.path != target {
        let _ = fs::remove_file(&staged.path);
    }
    log::info!("update: installed {} to {}", staged.release.tag, target.display());
    Ok(Installed { path: target.into(), version: staged.release.version.clone() })
}

/// Removes what earlier updates left behind: the Windows `<exe>.old`, downloads in `downloads` that are not newer
/// than `current` (installed already), partial downloads older than an hour (a shrew that was killed meanwhile) and
/// the rate-limit stamp of shrew 0.2. Cheap: call it at startup.
pub fn cleanup(current: &str, downloads: Option<&Path>) {
    if let Ok(target) = install_target() {
        let _ = fs::remove_file(old_path(&target));
    }
    if let Some(dir) = crate::cache_dir() {
        let _ = fs::remove_file(dir.join("update-check"));
    }
    let Some(entries) = downloads.and_then(|d| fs::read_dir(d).ok()) else { return };
    let hour_ago = SystemTime::now() - Duration::from_secs(60 * 60);
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("shrew-") else { continue };
        let stale = if name.contains(".part-") {
            e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < hour_ago)
        } else {
            let v = rest.split_once('-').map_or(rest, |(v, _)| v);
            !version::is_newer(v, current)
        };
        if stale {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Replaces the current process with the installed binary, run with `args`. Save state first: on Unix this `exec`s
/// (no destructors run), on Windows it spawns the new process and exits. Only returns on failure.
pub fn restart<I, S>(installed: &Installed, args: I) -> Error
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = std::process::Command::new(&installed.path);
    cmd.args(args);
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

/// `shrew-<version>-<triple>[.exe]`, from the asset name `shrew-<triple>[.exe]`.
fn staged_name(version: &str, asset: &str) -> String {
    let rest = asset.strip_prefix("shrew-").filter(|r| !r.is_empty()).unwrap_or(asset);
    format!("shrew-{version}-{rest}")
}

fn part_path(staged: &Path) -> PathBuf {
    let mut p = staged.as_os_str().to_owned();
    p.push(format!(".part-{}", std::process::id()));
    p.into()
}

fn hash_file(path: &Path, cancel: &AtomicBool) -> Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 << 10];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(hasher.finalize().into())
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

fn not_writable(err: io::Error, dir: &Path) -> Error {
    match err.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => Error::NotWritable(dir.into()),
        _ => err.into(),
    }
}

/// Whether a file can be created in `dir` (a probe file, removed at once: permission bits alone do not tell about
/// read-only mounts or ACLs).
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".shrew-write-test-{}", std::process::id()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(err) => err.kind() == io::ErrorKind::AlreadyExists,
    }
}

/// The cargo target folder `exe` was built into, if it runs from one (`target/debug/srw`,
/// `target/x86_64-unknown-linux-gnu/release/srw`): cargo marks its target folders with `CACHEDIR.TAG`.
pub(crate) fn cargo_target_dir(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .skip(1)
        .take(4)
        .find(|d| d.join("CACHEDIR.TAG").is_file() && (d.join(".rustc_info.json").is_file() || d.ends_with("target")))
        .map(Path::to_path_buf)
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
