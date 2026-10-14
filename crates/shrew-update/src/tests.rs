use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use sha2::{Digest, Sha256};

use crate::{checksum::encode_hex, feed::parse_feed, ratelimit, *};

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

/// Serves fixed bodies over plain http on a random local port; unknown paths are 404.
fn serve(routes: HashMap<String, Vec<u8>>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let response = match routes.get(request.url()) {
                Some(body) => tiny_http::Response::from_data(body.clone()),
                None => tiny_http::Response::from_data(b"not found".to_vec()).with_status_code(404),
            };
            let _ = request.respond(response);
        }
    });
    base
}

fn release_json(tag: &str, base: &str, target: &str, size: usize) -> String {
    format!(
        r#"{{"tag_name":"{tag}","name":"shrew {tag}","body":"notes for {tag}","draft":false,"prerelease":false,
        "published_at":"2026-10-14T12:00:00Z","assets":[
          {{"name":"{bin}","browser_download_url":"{base}/dl/{bin}","size":{size}}},
          {{"name":"{sum}","browser_download_url":"{base}/dl/{sum}","size":90}},
          {{"name":"shrew-other-triple","browser_download_url":"{base}/dl/other","size":1}}]}}"#,
        bin = asset_name(target),
        sum = checksum_name(target),
    )
}

/// A feed with release `tag` whose binary is `binary` and whose checksum file says `sha_of`'s sha256.
fn fake_release(tag: &str, binary: &[u8], sha_of: &[u8]) -> String {
    let bin = asset_name(LINUX);
    let sum = format!("{}  {bin}\n", encode_hex(&Sha256::digest(sha_of)));
    let mut routes = HashMap::new();
    routes.insert(format!("/dl/{bin}"), binary.to_vec());
    routes.insert(format!("/dl/{}", checksum_name(LINUX)), sum.into_bytes());
    let base = serve(routes.clone());
    // The feed must know the server's address, so serve it from a second server.
    let mut feed = HashMap::new();
    let path = "/repos/perunok/shrew/releases/latest";
    feed.insert(path.to_string(), release_json(tag, &base, LINUX, binary.len()).into_bytes());
    format!("{}{path}", serve(feed))
}

fn feed(url: String) -> Feed {
    Feed { url, target: LINUX.into() }
}

fn wait_for(updater: &Updater, done: impl Fn(&Status) -> bool) -> Status {
    let start = Instant::now();
    loop {
        let status = updater.status();
        if done(&status) {
            return status;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "stuck at {status:?}");
        std::thread::sleep(Duration::from_millis(5));
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
fn rate_limiting() {
    let dir = TempDir::new();
    let stamp = dir.0.join("nested/update-check");
    let now = SystemTime::now();
    assert!(ratelimit::due(&stamp, now), "never checked");
    ratelimit::mark(&stamp, now);
    assert!(!ratelimit::due(&stamp, now));
    assert!(!ratelimit::due(&stamp, now + CHECK_INTERVAL - Duration::from_secs(60)));
    assert!(ratelimit::due(&stamp, now + CHECK_INTERVAL));
    ratelimit::mark(&stamp, now + 10 * CHECK_INTERVAL);
    assert!(ratelimit::due(&stamp, now), "a stamp from the future does not block checks");
    fs::write(&stamp, "garbage").unwrap();
    assert!(ratelimit::due(&stamp, now));
}

#[test]
fn check_against_local_feed() {
    let url = fake_release("v0.2.0", b"new", b"new");
    let release = feed(url.clone()).check("0.1.0").unwrap().unwrap();
    assert_eq!(release.tag, "v0.2.0");
    assert_eq!(feed(url).check("0.2.0").unwrap(), None);

    // A repo without releases (GitHub answers 404) is "nothing yet", not an error.
    let base = serve(HashMap::new());
    assert_eq!(feed(format!("{base}/repos/perunok/shrew/releases/latest")).check("0.1.0").unwrap(), None);

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
    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
    if !writable {
        let err = result.unwrap_err();
        assert!(matches!(err, Error::NotWritable(_)), "{err}");
        assert!(err.to_string().contains("package manager"));
        assert_eq!(fs::read(&exe).unwrap(), b"old");
    }
}

#[test]
fn updater_state_machine() {
    let binary = b"#!/bin/sh\necho new\n".to_vec();
    let dir = TempDir::new();
    let exe = dir.0.join("shrew");
    fs::write(&exe, b"old").unwrap();
    let stamp = dir.0.join("stamp");
    let wakes = Arc::new(AtomicUsize::new(0));
    let opts = Options {
        current: "0.1.0".into(),
        feed: feed(fake_release("v0.2.0", &binary, &binary)),
        target: Some(exe.clone()),
        stamp: Some(stamp.clone()),
    };
    let w = wakes.clone();
    let updater = Updater::with_options(opts, move || {
        w.fetch_add(1, Ordering::Relaxed);
    });
    assert_eq!(updater.status(), Status::Idle);
    assert_eq!(updater.status().label(), None);

    assert!(updater.check(false));
    let Status::Available(release) = wait_for(&updater, |s| matches!(s, Status::Available(_))) else { unreachable!() };
    assert_eq!(updater.status().label().unwrap(), "Update to v0.2.0");
    assert!(!stamp.exists(), "a pending update is offered again on the next start");
    assert!(wakes.load(Ordering::Relaxed) >= 1);

    updater.activate(); // click: install
    let status = wait_for(&updater, |s| matches!(s, Status::Ready(_) | Status::Failed(_)));
    assert_eq!(status, Status::Ready(Installed { path: exe.clone(), version: release.version }));
    assert_eq!(fs::read(&exe).unwrap(), binary);
    assert!(!updater.check(true), "nothing to check once an update is installed");

    // Up to date: a forced check reports it and stamps the rate limit; automatic ones are then skipped.
    let quiet = Updater::with_options(
        Options {
            current: "0.2.0".into(),
            feed: feed(fake_release("v0.2.0", b"x", b"x")),
            target: Some(exe.clone()),
            stamp: Some(stamp.clone()),
        },
        || {},
    );
    assert!(quiet.check(true));
    wait_for(&quiet, |s| *s == Status::UpToDate);
    assert!(!ratelimit::due(&stamp, SystemTime::now()));
    quiet.activate();
    assert_eq!(quiet.status(), Status::Idle);

    // Forced check against a dead feed: Failed with a message; automatic: stays Idle.
    let broken = Options {
        current: "0.1.0".into(),
        feed: feed("http://127.0.0.1:1/latest".into()),
        target: Some(exe.clone()),
        stamp: None,
    };
    let failing = Updater::with_options(broken.clone(), || {});
    failing.check(false);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(failing.status(), Status::Idle);
    wait_for(&failing, |_| failing.check(true));
    let Status::Failed(msg) = wait_for(&failing, |s| matches!(s, Status::Failed(_))) else { unreachable!() };
    assert!(msg.contains("127.0.0.1"), "{msg}");
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

    // Our repo (may not exist yet) and a repo that surely doesn't: 404 is "no releases", not an error.
    eprintln!("perunok/shrew: {:?}", Feed::github(REPO).check("0.0.1").map(|r| r.map(|r| r.tag)));
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
