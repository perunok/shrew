//! The update flow end to end inside the app, offline: a feed and its assets as `file://` URLs in a temp dir, an
//! install target and download folder there too (never the test binary, never the user's cache).

use super::*;
use sha2::{Digest, Sha256};
use shrew_core::Mode;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("shrew-updates-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn state() -> State {
    State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true)
}

fn keys(st: &mut State, k: &str) {
    for k in KeyPress::parse_seq(k) {
        st.key(k);
    }
}

fn release(version: &str) -> Release {
    Release {
        version: version.into(),
        tag: format!("v{version}"),
        notes: "## New\n- a thing".into(),
        published: "2026-10-26T12:00:00Z".into(),
        asset_url: String::new(),
        checksum_url: String::new(),
        size: 1000,
        page: format!("https://github.com/perunok/shrew/releases/tag/v{version}"),
    }
}

/// A feed offering `version` with `binary`, as files under `dir`; returns its URL.
fn file_feed(dir: &Path, version: &str, binary: &[u8]) -> String {
    let t = shrew_update::TARGET;
    let bin = dir.join(shrew_update::asset_name(t));
    std::fs::write(&bin, binary).unwrap();
    let hex: String = Sha256::digest(binary).iter().map(|b| format!("{b:02x}")).collect();
    let sum = dir.join(shrew_update::checksum_name(t));
    std::fs::write(&sum, format!("{hex}  {}\n", shrew_update::asset_name(t))).unwrap();
    // serde_json escapes Windows paths' backslashes, which a format!-built string would not
    let json = serde_json::json!({
        "tag_name": format!("v{version}"),
        "body": format!("Notes for {version}"),
        "draft": false,
        "prerelease": false,
        "html_url": format!("https://github.com/perunok/shrew/releases/tag/v{version}"),
        "published_at": "2026-10-26T12:00:00Z",
        "assets": [
            {"name": shrew_update::asset_name(t), "browser_download_url": format!("file://{}", bin.display()), "size": binary.len()},
            {"name": shrew_update::checksum_name(t), "browser_download_url": format!("file://{}", sum.display()), "size": 90},
        ],
    });
    let feed = dir.join("latest.json");
    std::fs::write(&feed, json.to_string()).unwrap();
    format!("file://{}", feed.display())
}

/// A state whose updater runs as `current` against a local feed, installing to `<tmp>/bin/srw`.
fn with_feed(tmp: &Tmp, current: &str, offer: &str, binary: &[u8]) -> (State, PathBuf) {
    let feed_dir = tmp.0.join("feed");
    std::fs::create_dir_all(&feed_dir).unwrap();
    let url = file_feed(&feed_dir, offer, binary);
    let exe = tmp.0.join("bin/srw");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"old binary").unwrap();
    let opts = shrew_update::Options {
        current: current.into(),
        feed: shrew_update::Feed { url, target: shrew_update::TARGET.into() },
        target: Some(exe.clone()),
        downloads: Some(tmp.0.join("cache/updates")),
    };
    let mut st = state();
    st.updates.set_updater(Updater::with_options(opts, || {}), tmp.0.join("cache/update-state.json"));
    (st, exe)
}

/// Polls like the event loop does after a wake, until `done`.
fn until(st: &mut State, what: &str, done: impl Fn(&State) -> bool) {
    let start = Instant::now();
    while !done(st) {
        st.poll();
        assert!(start.elapsed() < Duration::from_secs(20), "waiting for {what}: {:?}", st.updates.phase());
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn toast_title(st: &State) -> Option<String> {
    match &st.updates.toast {
        Some(Toast::Available(r)) => Some(format!("shrew {} is available", r.version)),
        Some(Toast::Note { title, .. }) => Some(title.clone()),
        None => None,
    }
}

fn checked(update: Option<Release>, latest: &str) -> Result<shrew_update::Checked, String> {
    Ok(shrew_update::Checked { latest: Some(latest.into()), update })
}

#[test]
fn who_hears_about_a_check() {
    let tmp = Tmp::new("policy");
    let mut st = state();
    st.updates.prefs_path = Some(tmp.0.join("update-state.json"));
    st.updates.current = "0.2.0".into();

    // the startup check announces a newer version
    st.updates.source = Some(Source::Startup);
    answer(&mut st, checked(Some(release("0.3.0")), "0.3.0"));
    assert_eq!(toast_title(&st).as_deref(), Some("shrew 0.3.0 is available"));
    let saved = Prefs::load(&tmp.0.join("update-state.json"));
    assert_eq!((saved.latest.as_deref(), saved.notified.as_deref()), (Some("0.3.0"), Some("0.3.0")));
    assert!(saved.last_checked.is_some());

    // the daily check the same day stays quiet; the next start announces it again
    st.updates.toast = None;
    st.updates.source = Some(Source::Daily);
    answer(&mut st, checked(Some(release("0.3.0")), "0.3.0"));
    assert_eq!(st.updates.toast, None);
    assert_eq!(status_label(&st), Some(("⬆ shrew 0.3.0".into(), Tone::Accent)), "the status bar still offers it");
    st.updates.source = Some(Source::Startup);
    answer(&mut st, checked(Some(release("0.3.0")), "0.3.0"));
    assert!(st.updates.toast.is_some());

    // Settings answers inline
    st.updates.toast = None;
    st.updates.source = Some(Source::Settings);
    answer(&mut st, checked(Some(release("0.3.0")), "0.3.0"));
    assert_eq!(st.updates.toast, None);
    assert_eq!(status_text(&st), "shrew 0.3.0 is available (you have 0.2.0)");

    // a manual check always answers: up to date...
    st.updates.source = Some(Source::Manual);
    answer(&mut st, checked(None, "0.2.0"));
    assert_eq!(toast_title(&st).as_deref(), Some("shrew is up to date"));
    let Some(Toast::Note { body, .. }) = &st.updates.toast else { unreachable!() };
    assert_eq!(body, "0.2.0 is the latest release.");
    assert_eq!(status_text(&st), "shrew is up to date (0.2.0 is the latest release)");
    assert_eq!(status_label(&st), None);
    // ...or why it could not check, with Retry
    st.updates.source = Some(Source::Manual);
    answer(&mut st, Err("cannot reach api.github.com: timed out".into()));
    assert!(matches!(&st.updates.toast, Some(Toast::Note { error: true, retry: true, .. })));
    assert_eq!(status_text(&st), "Could not check: cannot reach api.github.com: timed out");
    assert!(st.updates.retry_at.is_none(), "asked for: no automatic retry");

    // an automatic check that fails is silent and tries again later
    st.updates.toast = None;
    st.updates.source = Some(Source::Daily);
    answer(&mut st, Err("offline".into()));
    assert_eq!(st.updates.toast, None);
    assert!(st.updates.retry_at.is_some());
}

#[test]
fn the_schedule_runs_on_deadlines() {
    let tmp = Tmp::new("schedule");
    let (mut st, _) = with_feed(&tmp, "0.2.0", "0.2.0", b"same");
    st.updates.headless = false;
    assert_eq!(deadline(&st), None, "nothing before the first frame");
    start(&mut st);
    let first = deadline(&st).expect("the startup check");
    assert!(first > Instant::now() + Duration::from_secs(2), "a few seconds after the first frame");
    assert!(!tick(&mut st, Instant::now()), "not yet");
    assert!(tick(&mut st, first));
    until(&mut st, "the startup check", |st| !st.updates.checking() && st.updates.prefs.last_checked.is_some());
    assert_eq!(st.updates.toast, None, "up to date at startup: silent");
    // next: the daily check, a day after the last one
    let daily = deadline(&st).expect("the daily check");
    assert!(daily > Instant::now() + Duration::from_secs(23 * 3600), "{:?}", daily - Instant::now());
    // the setting off: no deadline at all (idle CPU stays 0)
    st.ed.features.set(Feature::UpdateCheck, false);
    assert_eq!(deadline(&st), None);
    st.ed.features.set(Feature::UpdateCheck, true);
    // a day passes (the clock says so): due now
    st.updates.prefs.last_checked = Some(policy::now() - policy::DAY - 5);
    let due = deadline(&st).unwrap();
    assert!(due <= Instant::now() + Duration::from_secs(1));
    tick(&mut st, Instant::now());
    assert!(st.updates.checking() || st.updates.source.is_some());
    until(&mut st, "the daily check", |st| !st.updates.checking() && st.updates.source.is_none());
    assert!(deadline(&st).unwrap() > Instant::now() + Duration::from_secs(23 * 3600));
}

#[test]
fn update_now_downloads_verifies_and_waits_for_restart() {
    let tmp = Tmp::new("flow");
    let binary: Vec<u8> = (0..150_000u32).map(|i| (i % 253) as u8).collect();
    let (mut st, exe) = with_feed(&tmp, "0.2.0", "0.3.0", &binary);

    // Ctrl+Shift+U: a manual check, which always answers
    keys(&mut st, "<C-S-u>");
    until(&mut st, "the check", |st| st.updates.toast.is_some());
    assert_eq!(toast_title(&st).as_deref(), Some("shrew 0.3.0 is available"));
    assert_eq!(st.updates.support(), Ok(()));
    // with the notification showing, Ctrl+Shift+U opens the dialog
    keys(&mut st, "<C-S-u>");
    assert!(st.updates.dialog.is_some());
    assert_eq!(st.updates.toast, None, "the dialog takes over from the notification");
    assert_eq!(st.updates.stage(), Stage::Offer);
    // the dialog is modal: keys go nowhere else
    let text = st.ed.buf().doc.rope.to_string();
    keys(&mut st, "ix");
    assert_eq!(st.ed.buf().doc.rope.to_string(), text);
    // Enter: Download
    keys(&mut st, "<CR>");
    until(&mut st, "the download", |st| matches!(st.updates.phase(), Phase::Ready(_)));
    assert_eq!(std::fs::read(&exe).unwrap(), b"old binary", "nothing is installed before the restart");
    assert_eq!(st.updates.stage(), Stage::Ready);
    assert_eq!(status_label(&st), Some(("⟳ Restart to update".into(), Tone::Success)));
    // Esc: Later keeps the download; Settings and the status bar offer the restart
    keys(&mut st, "<Esc>");
    assert!(st.updates.dialog.is_none());
    assert!(matches!(st.updates.phase(), Phase::Ready(_)));
    let (label, _, button) = settings_action(&st).unwrap();
    assert_eq!((label.as_str(), button), ("shrew 0.3.0", "Restart to update"));
    assert_eq!(status_text(&st), "shrew 0.3.0 is downloaded and verified");
    st.activate_update();
    assert!(st.updates.dialog.is_some(), "the status-bar button reopens the dialog");
    // what Restart to update puts in place (the exec itself is not for a test)
    let u = st.updates.updater.clone().unwrap();
    u.install().unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), binary);
}

#[test]
fn hide_keeps_downloading_and_cancel_stops() {
    let mut st = state();
    st.updates.current = "0.2.0".into();
    st.updates.available = Some(release("0.3.0"));
    st.updates.demo =
        Some((Phase::Downloading { version: "0.3.0".into(), done: 370, total: 1000, speed: 100 }, Ok(())));
    open_dialog(&mut st);
    assert_eq!(st.updates.stage(), Stage::Downloading { done: 370, total: 1000, speed: 100 });
    keys(&mut st, "<Esc>");
    assert!(st.updates.dialog.is_none(), "Esc hides it");
    assert!(matches!(st.updates.phase(), Phase::Downloading { .. }), "the download goes on");
    assert_eq!(status_label(&st), Some(("⬇ 37%".into(), Tone::Muted)));
    // Ctrl+Shift+U (or the status bar) brings it back; Cancel is left of Hide
    keys(&mut st, "<C-S-u>");
    assert!(st.updates.dialog.is_some());
    keys(&mut st, "<Left><CR>");
    assert_eq!(st.updates.phase(), Phase::Idle, "cancelled");
    assert_eq!(st.updates.stage(), Stage::Offer, "and offered again");
}

#[test]
fn an_installation_that_cannot_update_itself_offers_the_release_page() {
    let mut st = state();
    st.updates.current = "0.2.0".into();
    st.updates.available = Some(release("0.3.0"));
    let why = "/usr/bin/srw belongs to a package manager; update shrew with it".to_string();
    st.updates.demo = Some((Phase::Idle, Err(why.clone())));
    assert_eq!(st.updates.stage(), Stage::Unsupported(why.clone()));
    assert_eq!(st.updates.stage().buttons(), [Button::Close, Button::ViewRelease]);
    let (_, description, button) = settings_action(&st).unwrap();
    assert_eq!(button, "View release");
    assert_eq!(
        description,
        format!("This installation cannot update itself: {why}. Download the new version from the release page.")
    );
    // :update opens the dialog but downloads nothing
    st.ed.run_ex("update");
    st.process_events();
    assert!(st.updates.dialog.is_some());
    assert_eq!(st.updates.phase(), Phase::Idle);
    assert_eq!(st.updates.page(), "https://github.com/perunok/shrew/releases/tag/v0.3.0");
}

#[test]
fn esc_dismisses_the_notification_and_still_reaches_the_editor() {
    let mut st = state();
    keys(&mut st, "i");
    assert_eq!(st.ed.mode, Mode::Insert);
    st.updates.show(Toast::Available(release("0.3.0")));
    keys(&mut st, "<Esc>");
    assert_eq!(st.updates.toast, None);
    assert_eq!(st.ed.mode, Mode::Normal, "Esc still left insert mode");
    // notes go away by themselves
    st.updates.note("shrew is up to date", "0.2.0 is the latest release.", false);
    let until = st.updates.toast_until.expect("a note hides itself");
    assert_eq!(deadline(&st), Some(until));
    tick(&mut st, until);
    assert_eq!(st.updates.toast, None);
}

#[test]
fn the_restarted_process_reopens_the_session_and_says_so() {
    // what restart() execs parses as a foreground start that restores the session
    let session_file = Path::new("/tmp/shrew-session-restart.json");
    let args: Vec<String> = restart_args(session_file).iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let cli = crate::cli::Cli::parse(args).unwrap();
    assert_eq!(cli.action, crate::cli::Action::Edit);
    assert!(cli.foreground && cli.new_window && cli.files.is_empty());
    assert_eq!(cli.restore.as_deref(), Some(session_file));

    let tmp = Tmp::new("restore");
    let file = tmp.0.join("notes.md");
    std::fs::write(&file, "saved\n").unwrap();
    let s = session::Session {
        from: "0.1.0".into(),
        page: Some("https://github.com/perunok/shrew/releases/tag/v0.2.0".into()),
        buffers: vec![session::Buf {
            path: Some(file.clone()),
            text: Some("saved\nand not saved\n".into()),
            dirty: true,
            sel: vec![(8, 8)],
            ..Default::default()
        }],
        ..Default::default()
    };
    let path = tmp.0.join("session-restart.json");
    session::save(&path, &s).unwrap();
    let mut st = state();
    st.updates.current = "0.2.0".into();
    restore_session(&mut st, Some(&path));
    assert!(!path.exists(), "used once");
    assert_eq!(st.ed.buf().doc.rope.to_string(), "saved\nand not saved\n");
    assert!(st.ed.buf().doc.dirty, "still unsaved");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "saved\n");
    let Some(Toast::Note { title, link, .. }) = &st.updates.toast else { panic!("{:?}", st.updates.toast) };
    assert_eq!(title, "Updated to shrew 0.2.0");
    assert_eq!(link.as_ref().map(|l| l.0), Some("What's new"));
}

#[test]
fn settings_rows_follow_the_state() {
    let mut st = state();
    st.updates.current = "0.2.0".into();
    assert_eq!(settings_action(&st), None, "nothing to update to: no row");
    assert_eq!(last_checked_text(&st), "never");
    st.updates.prefs.last_checked = Some(policy::now() - 180);
    assert_eq!(last_checked_text(&st), "3 minutes ago");
    st.updates.available = Some(release("0.3.0"));
    st.updates.demo = Some((Phase::Idle, Ok(())));
    assert_eq!(settings_action(&st).unwrap().2, "Update now");
    st.updates.demo = Some((Phase::Failed("disk full".into()), Ok(())));
    let (_, d, b) = settings_action(&st).unwrap();
    assert_eq!((d.as_str(), b), ("The download failed: disk full", "Retry"));
    // the panel shows them
    st.open_settings("updates");
    let p = st.settings.panel.as_ref().unwrap();
    let labels: Vec<&str> = p.shown().iter().map(|&i| p.rows[i].label.as_str()).collect();
    assert_eq!(
        labels,
        ["Check for new releases automatically", "Version", "Status", "Last checked", "Check now", "shrew 0.3.0"]
    );
}
