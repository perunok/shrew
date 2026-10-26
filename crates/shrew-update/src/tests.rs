use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

use crate::{checksum::encode_hex, feed::parse_feed, *};

const LINUX: &str = "x86_64-unknown-linux-gnu";

/// A unique scratch dir under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "shrew-update-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Hands out its bytes slowly: a chunk every few milliseconds, so a download can be caught in the middle.
struct Slow(Vec<u8>, usize);

impl Read for Slow {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_millis(3));
        let n = buf.len().min(8 << 10).min(self.0.len() - self.1);
        buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
        self.1 += n;
        Ok(n)
    }
}

/// Serves fixed bodies over plain http on a random local port; unknown paths are 404. Paths starting with `/slow/`
/// trickle.
fn serve(routes: HashMap<String, Vec<u8>>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            match routes.get(&url) {
                Some(body) if url.starts_with("/slow/") => {
                    let len = body.len();
                    let r = tiny_http::Response::new(200.into(), vec![], Slow(body.clone(), 0), Some(len), None);
                    std::thread::spawn(move || request.respond(r));
                }
                Some(body) => {
                    let _ = request.respond(tiny_http::Response::from_data(body.clone()));
                }
                None => {
                    let _ =
                        request.respond(tiny_http::Response::from_data(b"not found".to_vec()).with_status_code(404));
                }
            }
        }
    });
    base
}

fn release_json(tag: &str, base: &str, target: &str, size: usize) -> String {
    release_json_at(tag, base, "dl", target, size)
}

fn release_json_at(tag: &str, base: &str, dir: &str, target: &str, size: usize) -> String {
    format!(
        r#"{{"tag_name":"{tag}","name":"shrew {tag}","body":"notes for {tag}","draft":false,"prerelease":false,
        "html_url":"https://github.com/perunok/shrew/releases/tag/{tag}",
        "published_at":"2026-10-14T12:00:00Z","assets":[
          {{"name":"{bin}","browser_download_url":"{base}/{dir}/{bin}","size":{size}}},
          {{"name":"{sum}","browser_download_url":"{base}/dl/{sum}","size":90}},
          {{"name":"shrew-other-triple","browser_download_url":"{base}/dl/other","size":1}}]}}"#,
        bin = asset_name(target),
        sum = checksum_name(target),
    )
}

/// A feed with release `tag` whose binary is `binary` and whose checksum file says `sha_of`'s sha256. `slow`: the
/// binary trickles.
fn fake_release_with(tag: &str, binary: &[u8], sha_of: &[u8], slow: bool) -> String {
    let bin = asset_name(LINUX);
    let sum = format!("{}  {bin}\n", encode_hex(&Sha256::digest(sha_of)));
    let dir = if slow { "slow" } else { "dl" };
    let mut routes = HashMap::new();
    routes.insert(format!("/{dir}/{bin}"), binary.to_vec());
    routes.insert(format!("/dl/{}", checksum_name(LINUX)), sum.into_bytes());
    let base = serve(routes.clone());
    // The feed must know the server's address, so serve it from a second server.
    let mut feed = HashMap::new();
    let path = "/repos/perunok/shrew/releases/latest";
    feed.insert(path.to_string(), release_json_at(tag, &base, dir, LINUX, binary.len()).into_bytes());
    format!("{}{path}", serve(feed))
}

fn fake_release(tag: &str, binary: &[u8], sha_of: &[u8]) -> String {
    fake_release_with(tag, binary, sha_of, false)
}

fn feed(url: String) -> Feed {
    Feed { url, target: LINUX.into() }
}

fn wait_for(updater: &Updater, done: impl Fn(&Phase) -> bool) -> Phase {
    let start = Instant::now();
    loop {
        let phase = updater.phase();
        if done(&phase) {
            return phase;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "stuck at {phase:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn wait_checked(updater: &Updater) -> Result<Checked, String> {
    let start = Instant::now();
    loop {
        if let Some(r) = updater.take_checked() {
            assert!(!updater.checking());
            return r;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "the check never answered");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn version_comparison() {
    assert!(is_newer("v0.2.0", "0.1.0"));
    assert!(is_newer("0.1.1", "v0.1.0"));
    assert!(is_newer("v1.0.0", "0.99.99"));
    assert!(is_newer("v0.10.0", "0.9.0"), "numeric, not lexical");
    assert!(!is_newer("v0.1.0", "0.1.0"));
    assert!(!is_newer("v0.0.9", "0.1.0"));
    assert!(!is_newer("v0.2.0-beta.1", "0.1.0"), "prereleases are never offered");
    assert!(is_newer("v0.2.0", "0.2.0-beta.1"), "a prerelease build updates to the final release");
    assert!(!is_newer("v0.1.0+build5", "0.1.0"), "build metadata is not a new version");
    assert!(!is_newer("nightly", "0.1.0"));
    assert!(!is_newer("v0.2.0", "garbage"));
}

#[test]
fn asset_names() {
    assert_eq!(asset_name(LINUX), "shrew-x86_64-unknown-linux-gnu");
    assert_eq!(asset_name("x86_64-pc-windows-msvc"), "shrew-x86_64-pc-windows-msvc.exe");
    assert_eq!(checksum_name("x86_64-pc-windows-msvc"), "shrew-x86_64-pc-windows-msvc.sha256");
    assert_eq!(asset_name("aarch64-apple-darwin"), "shrew-aarch64-apple-darwin");
    assert!(!TARGET.is_empty() && TARGET.contains('-'), "build.rs bakes the triple in: {TARGET}");
}

#[test]
fn feed_selection() {
    let base = "https://example.com";
    let json = release_json("v0.2.0", base, LINUX, 1234);
    let release = parse_feed(&json, "0.1.0", LINUX, true).unwrap().unwrap();
    assert_eq!(release.version, "0.2.0");
    assert_eq!(release.tag, "v0.2.0");
    assert_eq!(release.notes, "notes for v0.2.0");
    assert_eq!(release.published, "2026-10-14T12:00:00Z");
    assert_eq!(release.asset_url, format!("{base}/dl/shrew-{LINUX}"));
    assert_eq!(release.checksum_url, format!("{base}/dl/shrew-{LINUX}.sha256"));
    assert_eq!(release.size, 1234);
    assert_eq!(release.page, "https://github.com/perunok/shrew/releases/tag/v0.2.0");

    // Same or older version, other target, drafts and prereleases: nothing to offer.
    assert_eq!(parse_feed(&json, "0.2.0", LINUX, true).unwrap(), None);
    assert_eq!(parse_feed(&json, "0.3.0", LINUX, true).unwrap(), None);
    assert_eq!(parse_feed(&json, "0.1.0", "riscv64gc-unknown-linux-gnu", true).unwrap(), None);
    let draft = json.replace(r#""draft":false"#, r#""draft":true"#);
    assert_eq!(parse_feed(&draft, "0.1.0", LINUX, true).unwrap(), None);
    let pre = json.replace(r#""prerelease":false"#, r#""prerelease":true"#);
    assert_eq!(parse_feed(&pre, "0.1.0", LINUX, true).unwrap(), None);

    // No checksum file, no update: never install unverified bytes.
    let no_sum = json.replace(&format!(r#""name":"shrew-{LINUX}.sha256""#), r#""name":"x""#);
    assert_eq!(parse_feed(&no_sum, "0.1.0", LINUX, true).unwrap(), None);

    // An https feed never hands out plain http assets.
    let http = release_json("v0.2.0", "http://example.com", LINUX, 1);
    assert_eq!(parse_feed(&http, "0.1.0", LINUX, true).unwrap(), None);
    assert!(parse_feed(&http, "0.1.0", LINUX, false).unwrap().is_some());

    // A page link that is not https is replaced by the canonical one.
    let odd = json.replace("https://github.com/perunok/shrew/releases/tag/v0.2.0", "javascript:alert(1)");
    let r = parse_feed(&odd, "0.1.0", LINUX, true).unwrap().unwrap();
    assert_eq!(r.page, release_page(REPO, "v0.2.0"));

    // A list feed picks the newest stable release.
    let list = format!(
        "[{},{},{}]",
        release_json("v0.3.0", base, LINUX, 1),
        release_json("v0.4.0", base, LINUX, 1).replace(r#""prerelease":false"#, r#""prerelease":true"#),
        release_json("v0.2.0", base, LINUX, 1),
    );
    assert_eq!(parse_feed(&list, "0.1.0", LINUX, true).unwrap().unwrap().tag, "v0.3.0");

    assert!(matches!(parse_feed("<html>rate limited</html>", "0.1.0", LINUX, true), Err(Error::Feed(_))));
}

#[test]
fn checked_names_the_latest_release_even_when_it_is_not_newer() {
    let json = release_json("v0.2.0", "https://example.com", LINUX, 1);
    let c = feed::parse_checked(&json, "0.2.0", LINUX, true).unwrap();
    assert_eq!(c, Checked { latest: Some("0.2.0".into()), update: None });
    // the latest release has no binary for this target: still the latest, but nothing to install
    let c = feed::parse_checked(&json, "0.1.0", "riscv64gc-unknown-linux-gnu", true).unwrap();
    assert_eq!((c.latest.as_deref(), c.update), (Some("0.2.0"), None));
    let c = feed::parse_checked(&json, "0.1.0", LINUX, true).unwrap();
    assert_eq!(c.update.unwrap().version, "0.2.0");
}

#[test]
fn checksum_parsing() {
    let hex = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    let want = Sha256::digest(b"test");
    assert_eq!(parse_checksum(&format!("{hex}  shrew-x\n"), "shrew-x").unwrap(), want[..]);
    assert_eq!(parse_checksum(&format!("{hex} *shrew-x"), "shrew-x").unwrap(), want[..]);
    assert_eq!(parse_checksum(&format!("{}  dist/shrew-x", hex.to_uppercase()), "shrew-x").unwrap(), want[..]);
    assert_eq!(parse_checksum(&format!("{hex}\n"), "shrew-x").unwrap(), want[..], "bare hash");
    let multi = format!("{}  shrew-y\n{hex}  shrew-x\n", "0".repeat(64));
    assert_eq!(parse_checksum(&multi, "shrew-x").unwrap(), want[..]);

    assert!(parse_checksum(&format!("{hex}  shrew-y"), "shrew-x").is_err(), "wrong file");
    assert!(parse_checksum(&format!("{}  shrew-x", &hex[..60]), "shrew-x").is_err(), "short");
    assert!(parse_checksum(&format!("{}zz  shrew-x", &hex[..62]), "shrew-x").is_err(), "not hex");
    assert!(parse_checksum("", "shrew-x").is_err());
}

#[test]
fn check_against_local_feed() {
    let url = fake_release("v0.2.0", b"new", b"new");
    let release = feed(url.clone()).check("0.1.0").unwrap().unwrap();
    assert_eq!(release.tag, "v0.2.0");
    assert_eq!(feed(url).check("0.2.0").unwrap(), None);

    // A repo without releases (GitHub answers 404) is "nothing yet", not an error.
    let base = serve(HashMap::new());
    let none = feed(format!("{base}/repos/perunok/shrew/releases/latest"));
    assert_eq!(none.fetch("0.1.0").unwrap(), Checked::default());

    // Unreachable host: a plain error the forced check can show.
    assert!(matches!(feed("http://127.0.0.1:1/latest".into()).check("0.1.0"), Err(Error::Http(_))));
}

#[test]
fn check_against_file_feed() {
    let dir = TempDir::new();
    let path = dir.0.join("latest.json");
    fs::write(&path, release_json("v9.0.0", "file:///nowhere", LINUX, 3)).unwrap();
    let release = feed(format!("file://{}", path.display())).check("0.1.0").unwrap().unwrap();
    assert_eq!(release.version, "9.0.0");
    assert_eq!(feed(format!("file://{}", dir.0.join("missing.json").display())).check("0.1.0").unwrap(), None);
}

#[test]
fn install_replaces_target() {
    let new_binary: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let release = feed(fake_release("v0.2.0", &new_binary, &new_binary)).check("0.1.0").unwrap().unwrap();

    let dir = TempDir::new();
    let exe = dir.0.join("shrew");
    fs::write(&exe, b"old binary").unwrap();
    let mut calls = Vec::new();
    let installed = install_to(&release, &exe, |done, total| calls.push((done, total))).unwrap();

    assert_eq!(installed, Installed { path: exe.clone(), version: "0.2.0".into() });
    assert_eq!(fs::read(&exe).unwrap(), new_binary);
    assert_eq!(calls.first(), Some(&(0, new_binary.len() as u64)));
    assert_eq!(calls.last(), Some(&(new_binary.len() as u64, new_binary.len() as u64)));
    assert!(calls.windows(2).all(|w| w[0].0 <= w[1].0), "progress is monotonic");
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1, "no temp file left behind");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&exe).unwrap().permissions().mode() & 0o111, 0o111, "executable");
    }
}

#[test]
fn install_rejects_bad_checksum() {
    let release = feed(fake_release("v0.2.0", b"tampered", b"genuine")).check("0.1.0").unwrap().unwrap();
    let dir = TempDir::new();
    let exe = dir.0.join("shrew");
    fs::write(&exe, b"old binary").unwrap();
    let err = install_to(&release, &exe, |_, _| {}).unwrap_err();
    assert!(matches!(err, Error::Checksum { .. }), "{err}");
    assert_eq!(fs::read(&exe).unwrap(), b"old binary", "target untouched");
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1, "temp file removed");
}

#[test]
fn download_stages_then_apply_installs() {
    let binary: Vec<u8> = (0..200_000u32).map(|i| (i % 241) as u8).collect();
    let release = feed(fake_release("v0.3.0", &binary, &binary)).check("0.1.0").unwrap().unwrap();
    let cache = TempDir::new();
    let bin = TempDir::new();
    let exe = bin.0.join("srw");
    fs::write(&exe, b"old").unwrap();

    let mut steps = Vec::new();
    let staged = download(&release, &cache.0, &AtomicBool::new(false), |s| steps.push(s)).unwrap();
    assert_eq!(staged.path, cache.0.join(format!("shrew-0.3.0-{LINUX}")));
    assert_eq!(fs::read(&staged.path).unwrap(), binary);
    assert_eq!(staged.sha256_hex(), encode_hex(&Sha256::digest(&binary)));
    assert_eq!(steps.last(), Some(&Step::Verifying), "verifying comes last");
    assert!(matches!(steps[0], Step::Downloading { done: 0, .. }));
    assert_eq!(fs::read(&exe).unwrap(), b"old", "downloading replaces nothing");
    assert_eq!(fs::read_dir(&cache.0).unwrap().count(), 1, "only the verified file");

    // a staged file changed after it was verified is refused, and dropped
    let mut bad = staged.clone();
    bad.sha256[0] ^= 1;
    assert!(matches!(apply(&bad, &exe), Err(Error::Checksum { .. })));
    assert_eq!(fs::read(&exe).unwrap(), b"old");
    assert_eq!(fs::read_dir(&bin.0).unwrap().count(), 1, "no temp file next to the target");
    let staged = download(&release, &cache.0, &AtomicBool::new(false), |_| {}).unwrap();

    let installed = apply(&staged, &exe).unwrap();
    assert_eq!(installed, Installed { path: exe.clone(), version: "0.3.0".into() });
    assert_eq!(fs::read(&exe).unwrap(), binary);
    assert!(!staged.path.exists(), "the staged file goes once installed");
    assert_eq!(fs::read_dir(&bin.0).unwrap().count(), 1);
    assert!(matches!(apply(&staged, &exe), Err(Error::Feed(m)) if m.contains("gone")));
}

#[test]
fn cancelling_a_download_deletes_the_partial_file() {
    let binary = vec![7u8; 400_000];
    let release = feed(fake_release_with("v0.3.0", &binary, &binary, true)).check("0.1.0").unwrap().unwrap();
    let cache = TempDir::new();
    let cancel = AtomicBool::new(false);
    let err = download(&release, &cache.0, &cancel, |s| {
        if matches!(s, Step::Downloading { done, .. } if done > 0) {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err}");
    assert_eq!(fs::read_dir(&cache.0).unwrap().count(), 0, "nothing left behind");
}

#[test]
fn cleanup_drops_installed_and_stale_downloads() {
    let dir = TempDir::new();
    let keep = dir.0.join(format!("shrew-0.9.0-{LINUX}"));
    let old = dir.0.join(format!("shrew-0.2.0-{LINUX}"));
    let other = dir.0.join("notes.txt");
    let fresh_part = dir.0.join(format!("shrew-0.9.0-{LINUX}.part-1"));
    for f in [&keep, &old, &other, &fresh_part] {
        fs::write(f, b"x").unwrap();
    }
    cleanup("0.2.0", Some(&dir.0));
    assert!(keep.exists(), "newer than this build: still to be installed");
    assert!(!old.exists(), "installed already");
    assert!(other.exists(), "not ours");
    assert!(fresh_part.exists(), "maybe another shrew is downloading it right now");
}

#[cfg(unix)]
#[test]
fn install_into_read_only_dir_says_use_package_manager() {
    use std::os::unix::fs::PermissionsExt;
    let release = feed(fake_release("v0.2.0", b"new", b"new")).check("0.1.0").unwrap().unwrap();
    let dir = TempDir::new();
    let exe = dir.0.join("shrew");
    fs::write(&exe, b"old").unwrap();
    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o555)).unwrap();
    let writable = fs::write(dir.0.join("probe"), b"").is_ok(); // root ignores permissions
    let result = install_to(&release, &exe, |_, _| {});
    let support = support_for(&exe);
    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
    if !writable {
        let err = result.unwrap_err();
        assert!(matches!(err, Error::NotWritable(_)), "{err}");
        assert!(err.to_string().contains("package manager"));
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        assert_eq!(support, Err(Unsupported::NotWritable(dir.0.clone())));
        assert!(support.unwrap_err().to_string().contains("is not writable"));
    }
}

#[test]
fn support_spots_development_builds_and_packages() {
    let dir = TempDir::new();
    let target = dir.0.join("target");
    fs::create_dir_all(target.join("debug")).unwrap();
    fs::write(target.join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55").unwrap();
    let exe = target.join("debug/srw");
    fs::write(&exe, b"x").unwrap();
    let err = support_for(&exe).unwrap_err();
    assert_eq!(err, Unsupported::DevBuild(target.clone()));
    assert!(err.to_string().contains("development build"), "{err}");
    // a cross build: target/<triple>/release
    fs::create_dir_all(target.join(LINUX).join("release")).unwrap();
    assert!(matches!(support_for(&target.join(LINUX).join("release/srw")), Err(Unsupported::DevBuild(_))));
    // a copy elsewhere updates itself
    let copy = dir.0.join("bin/srw");
    fs::create_dir_all(copy.parent().unwrap()).unwrap();
    fs::write(&copy, b"x").unwrap();
    assert_eq!(support_for(&copy), Ok(()));
    #[cfg(unix)]
    assert!(matches!(support_for(Path::new("/usr/bin/srw")), Err(Unsupported::PackageManaged(_))));
}

#[test]
fn updater_checks_downloads_and_installs() {
    let binary = b"#!/bin/sh\necho new\n".to_vec();
    let dir = TempDir::new();
    let exe = dir.0.join("shrew");
    fs::write(&exe, b"old").unwrap();
    let downloads = dir.0.join("cache");
    let wakes = Arc::new(AtomicUsize::new(0));
    let opts = Options {
        current: "0.1.0".into(),
        feed: feed(fake_release("v0.2.0", &binary, &binary)),
        target: Some(exe.clone()),
        downloads: Some(downloads.clone()),
    };
    let w = wakes.clone();
    let updater = Updater::with_options(opts, move || {
        w.fetch_add(1, Ordering::Relaxed);
    });
    assert_eq!(updater.phase(), Phase::Idle);
    assert_eq!(updater.take_checked(), None);
    assert_eq!(updater.support(), Ok(exe.clone()));

    assert!(updater.check());
    let checked = wait_checked(&updater).unwrap();
    assert_eq!(checked.latest.as_deref(), Some("0.2.0"));
    let release = checked.update.unwrap();
    assert_eq!(updater.take_checked(), None, "answered once");
    assert!(wakes.load(Ordering::Relaxed) >= 1);

    assert!(updater.download(&release));
    let Phase::Ready(staged) = wait_for(&updater, |p| matches!(p, Phase::Ready(_) | Phase::Failed(_))) else {
        panic!("{:?}", updater.phase())
    };
    assert_eq!(staged.release, release);
    assert_eq!(fs::read(&exe).unwrap(), b"old", "Ready installs nothing by itself");
    assert!(!updater.download(&release), "already downloaded");

    let installed = updater.install().unwrap();
    assert_eq!(installed, Installed { path: exe.clone(), version: "0.2.0".into() });
    assert_eq!(fs::read(&exe).unwrap(), binary);

    // Up to date: the answer says which version is the latest.
    let quiet = Updater::with_options(
        Options {
            current: "0.2.0".into(),
            feed: feed(fake_release("v0.2.0", b"x", b"x")),
            target: Some(exe.clone()),
            downloads: None,
        },
        || {},
    );
    assert!(quiet.check());
    assert_eq!(wait_checked(&quiet), Ok(Checked { latest: Some("0.2.0".into()), update: None }));

    // A dead feed: the message names the host.
    let failing = Updater::with_options(
        Options {
            current: "0.1.0".into(),
            feed: feed("http://127.0.0.1:1/latest".into()),
            target: Some(exe.clone()),
            downloads: None,
        },
        || {},
    );
    assert!(failing.check());
    let msg = wait_checked(&failing).unwrap_err();
    assert!(msg.contains("127.0.0.1"), "{msg}");
}

#[test]
fn updater_cancel_and_failure() {
    let binary = vec![3u8; 600_000];
    let dir = TempDir::new();
    let exe = dir.0.join("shrew");
    fs::write(&exe, b"old").unwrap();
    let downloads = dir.0.join("cache");
    let url = fake_release_with("v0.2.0", &binary, &binary, true);
    let release = feed(url.clone()).check("0.1.0").unwrap().unwrap();
    let opts = Options {
        current: "0.1.0".into(),
        feed: feed(url),
        target: Some(exe.clone()),
        downloads: Some(downloads.clone()),
    };
    let updater = Updater::with_options(opts, || {});
    assert!(updater.download(&release));
    assert!(!updater.download(&release), "one download at a time");
    let Phase::Downloading { version, total, .. } =
        wait_for(&updater, |p| matches!(p, Phase::Downloading { done, .. } if *done > 0))
    else {
        unreachable!()
    };
    assert_eq!((version.as_str(), total), ("0.2.0", binary.len() as u64));
    updater.cancel();
    assert_eq!(wait_for(&updater, |p| *p == Phase::Idle), Phase::Idle);
    // the thread is done once the slot is free again: then nothing may be left
    let start = Instant::now();
    while fs::read_dir(&downloads).map(|d| d.count()).unwrap_or(0) > 0 {
        assert!(start.elapsed() < Duration::from_secs(5), "partial file left behind");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fs::read(&exe).unwrap(), b"old");

    // a corrupt download fails with the checksum message; reset clears it
    let bad = feed(fake_release("v0.2.0", b"evil", b"good")).check("0.1.0").unwrap().unwrap();
    let start = Instant::now();
    while !updater.download(&bad) {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    let Phase::Failed(msg) = wait_for(&updater, |p| matches!(p, Phase::Failed(_))) else { unreachable!() };
    assert!(msg.contains("sha256"), "{msg}");
    assert!(updater.install().is_err());
    updater.reset();
    assert_eq!(updater.phase(), Phase::Idle);
}

#[test]
fn speed_meter() {
    let t0 = Instant::now();
    let mut m = updater::Meter::new(t0);
    assert_eq!(m.add(t0 + Duration::from_millis(100), 10_000), 0, "no full window yet");
    let s = m.add(t0 + Duration::from_secs(1), 1_000_000);
    assert_eq!(s, 1_000_000);
    let s = m.add(t0 + Duration::from_secs(2), 1_500_000);
    assert!(s > 500_000 && s < 1_000_000, "smoothed: {s}");
}

/// Live: the real GitHub API through the same pure-Rust TLS path. `cargo test -p shrew-update -- --ignored`.
#[test]
#[ignore = "network"]
fn live_github() {
    // A repo with releases: parses, but helix doesn't ship shrew-<triple> assets, so nothing to offer.
    let helix = Feed::github("helix-editor/helix");
    assert_eq!(helix.check("0.0.1").unwrap(), None);
    let json = crate::net::get_text(&helix.url, "application/vnd.github+json", 8 << 20).unwrap().unwrap();
    let latest: serde_json::Value = serde_json::from_str(&json).unwrap();
    let tag = latest["tag_name"].as_str().unwrap();
    eprintln!("helix latest: {tag}");

    // Release assets redirect to another host (release-assets.githubusercontent.com): stream the first 64 KiB.
    let asset = latest["assets"][0]["browser_download_url"].as_str().unwrap();
    let body = crate::net::get(asset, "application/octet-stream", Some(Duration::from_secs(60))).unwrap().unwrap();
    let mut head = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(body.reader, 64 << 10), &mut head).unwrap();
    assert_eq!(head.len(), 64 << 10);
    eprintln!("downloaded 64 KiB of {asset} (content-length {:?})", body.len);

    // Our repo and a repo that surely doesn't exist: 404 is "no releases", not an error.
    let ours = Feed::github(REPO).fetch("0.0.1").unwrap();
    eprintln!("perunok/shrew: latest {:?}", ours.latest);
    assert!(ours.update.is_some(), "perunok/shrew publishes {}", asset_name(TARGET));
    assert_eq!(Feed::github("perunok/this-repo-does-not-exist-shrew").check("0.0.1").unwrap(), None);
}

#[cfg(unix)]
#[test]
fn package_managed_paths() {
    use crate::install::package_managed;
    let managed = ["/usr/bin/shrew", "/bin/shrew", "/nix/store/abc-shrew/bin/shrew", "/opt/homebrew/Cellar/s/bin/x"];
    for path in managed {
        assert!(package_managed(Path::new(path)), "{path}");
    }
    for path in ["/usr/local/bin/shrew", "/home/me/.local/bin/shrew", "/home/me/.cargo/bin/shrew", "/opt/shrew/shrew"] {
        assert!(!package_managed(Path::new(path)), "{path}");
    }
}
