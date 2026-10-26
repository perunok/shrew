//! End to end against the real GitHub release (network, so ignored by default):
//!
//! ```sh
//! cargo test -p shrew --test update_e2e -- --ignored --nocapture
//! ```
//!
//! A copy of the `srw` this test was built with goes into a temp folder and runs `--self-update` pretending to be
//! shrew 0.1.0 (`SHREW_PRETEND_VERSION`, honoured by debug builds only). It must find the latest release, download
//! the binary for this platform, verify it and install it over the temp copy, which must then be byte for byte the
//! published asset (its sha256 from the release's `.sha256` file). Nothing outside the temp folder is touched: the
//! cache goes to the temp folder too, and the test checks that `~/.local/bin/srw` did not change.
#![cfg(feature = "update")]

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

fn sha256(path: &Path) -> [u8; 32] {
    Sha256::digest(std::fs::read(path).unwrap()).into()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
#[ignore = "network: downloads the real release from GitHub"]
fn self_update_against_the_real_release() {
    if cfg!(not(debug_assertions)) {
        panic!("SHREW_PRETEND_VERSION works in debug builds only: run without --release");
    }
    let tmp = std::env::temp_dir().join(format!("shrew-update-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let bin = tmp.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("srw");
    std::fs::copy(env!("CARGO_BIN_EXE_srw"), &exe).unwrap();
    let before = sha256(&exe);

    // the user's own install must not move
    let home_srw = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/srw"));
    let home_before = home_srw.as_deref().filter(|p| p.exists()).map(sha256);

    let latest = shrew_update::Feed::github(shrew_update::REPO).fetch("0.1.0").expect("GitHub answers");
    let release = latest.update.expect("a release with a binary for this platform");
    let published = shrew_update::published_sha256(&release).expect("the release's .sha256");
    eprintln!("latest release {} ({} bytes), published sha256 {}", release.tag, release.size, hex(&published));

    let out = Command::new(&exe)
        .arg("--self-update")
        .env("SHREW_PRETEND_VERSION", "0.1.0")
        .env("XDG_CACHE_HOME", tmp.join("cache"))
        .env_remove("SHREW_UPDATE_FEED_URL")
        .env_remove("SHREW_UPDATE_REPO")
        .env_remove("APPIMAGE")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("--- srw --self-update\n{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "srw --self-update failed");
    assert!(stdout.contains(&format!("shrew {} is available (you have 0.1.0)", release.version)), "{stdout}");
    assert!(stdout.contains(&format!("installed shrew {}", release.version)), "{stdout}");

    let after = sha256(&exe);
    assert_ne!(after, before, "the temp copy was replaced");
    assert_eq!(hex(&after), hex(&published), "the installed binary is the published asset");
    assert_eq!(std::fs::metadata(&exe).unwrap().len(), release.size);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o111, 0o111, "still executable");
    }
    // the new binary runs and says which version it is
    let v = Command::new(&exe).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&v.stdout).trim(), format!("shrew {}", release.version));
    // the download was moved into place: nothing left in the cache, nothing next to the binary
    let left: Vec<_> =
        std::fs::read_dir(tmp.join("cache/shrew/updates")).map(|d| d.flatten().collect()).unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 1);
    // and nothing outside the temp folder changed
    assert_eq!(home_srw.as_deref().filter(|p| p.exists()).map(sha256), home_before, "~/.local/bin/srw untouched");
    let _ = std::fs::remove_dir_all(&tmp);
}
