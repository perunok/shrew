//! The git parts end to end on real repositories (made with the `git` program; skipped without it): markers
//! follow edits, hunks stage, unstage and revert, the status reaches the status bar and the sidebar.

use crate::state::State;
use shrew_core::KeyPress;
use shrew_core::features::Feature;
use shrew_core::git_hunks::HunkKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static N: AtomicUsize = AtomicUsize::new(0);

/// A fresh repository with `files` committed.
fn repo(files: &[(&str, &str)]) -> Option<PathBuf> {
    if !shrew_git::git_available() {
        eprintln!("git not found: skipped");
        return None;
    }
    let dir =
        std::env::temp_dir().join(format!("shrew-git-ui-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = shrew_git::canonicalize(&dir).unwrap();
    run(&dir, &["init", "-q", "-b", "main"]);
    run(&dir, &["config", "user.name", "Ann"]);
    run(&dir, &["config", "user.email", "ann@example.com"]);
    run(&dir, &["config", "commit.gpgsign", "false"]);
    run(&dir, &["config", "core.autocrlf", "false"]);
    for (p, text) in files {
        let f = dir.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    }
    run(&dir, &["add", "-A"]);
    run(&dir, &["commit", "-q", "-m", "first"]);
    Some(dir)
}

fn run(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args).env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn state() -> State {
    State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true)
}

fn keys(st: &mut State, s: &str) {
    for k in KeyPress::parse_seq(s) {
        st.key(k);
    }
}

/// Polls until `f` holds and nothing git is in flight (or fails after a while).
fn wait(st: &mut State, what: &str, f: impl Fn(&State) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        st.poll();
        if f(st) && !crate::git::busy(st) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}; message {:?}", st.ed.message);
}

fn kinds(st: &State) -> Vec<(HunkKind, u32, u32)> {
    st.ed.hunks().iter().map(|h| (h.kind(), h.new.start, h.new.end)).collect()
}

#[test]
fn markers_follow_edits_and_hunks_stage_unstage_and_revert() {
    let Some(dir) = repo(&[("a.txt", "one\ntwo\nthree\nfour\n")]) else { return };
    let file = dir.join("a.txt");
    let mut st = state();
    st.open_at(&file, None, None);
    st.after_input();
    wait(&mut st, "the first markers", |_| true);
    assert!(kinds(&st).is_empty(), "a clean file has no markers");

    // an edit, then a new line: changed and added markers
    keys(&mut st, "ggA!<Esc>Gkonew<Esc>");
    wait(&mut st, "markers for the edits", |st| kinds(st).len() == 2);
    assert_eq!(kinds(&st), vec![(HunkKind::Modified, 0, 1), (HunkKind::Added, 4, 5)]);

    // stage the first hunk: it leaves the markers (they compare with the index) and is in the index
    keys(&mut st, "gg");
    keys(&mut st, "ghs");
    wait(&mut st, "the staged hunk to drop out", |st| kinds(st).len() == 1);
    assert_eq!(kinds(&st), vec![(HunkKind::Added, 4, 5)]);
    let staged = run(&dir, &["diff", "--cached"]);
    assert!(staged.contains("+one!") && !staged.contains("new"), "{staged}");
    assert!(st.ed.message.as_ref().is_some_and(|m| m.0.contains("staged")), "{:?}", st.ed.message);

    // unstage it again
    keys(&mut st, "ghu");
    wait(&mut st, "the unstaged hunk to come back", |st| kinds(st).len() == 2);
    assert_eq!(run(&dir, &["diff", "--cached"]), "");

    // revert the added line: an edit, undone by u
    keys(&mut st, "G[c");
    assert_eq!(st.ed.buf().cursor_line(), 4);
    keys(&mut st, "ghr");
    assert_eq!(st.ed.buf().rope().to_string(), "one!\ntwo\nthree\nfour\n");
    keys(&mut st, "u");
    assert_eq!(st.ed.buf().rope().to_string(), "one!\ntwo\nthree\nfour\nnew\n");

    // the peek opens on the hunk at the cursor, follows ]c, and Esc closes it
    keys(&mut st, "gg");
    keys(&mut st, "ghp");
    assert_eq!(st.git.gutter.peek.map(|p| p.line), Some(0));
    keys(&mut st, "]c");
    wait(&mut st, "the peek to follow", |st| st.git.gutter.peek.is_some());
    assert_eq!(st.git.gutter.peek.map(|p| p.line), Some(4));
    keys(&mut st, "<Esc>");
    assert!(st.git.gutter.peek.is_none());

    // visual mode stages only the selected lines
    keys(&mut st, "ggVghs");
    wait(&mut st, "the selected line staged", |st| kinds(st).len() == 1);
    assert_eq!(st.ed.mode, shrew_core::Mode::Normal, "visual mode ends");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn markers_can_compare_with_head_and_turn_off() {
    let Some(dir) = repo(&[("a.txt", "one\ntwo\n")]) else { return };
    std::fs::write(dir.join("a.txt"), "one\nTWO\n").unwrap();
    run(&dir, &["add", "a.txt"]);
    let mut st = state();
    st.open_at(&dir.join("a.txt"), None, None);
    st.after_input();
    wait(&mut st, "the index markers", |_| true);
    assert!(kinds(&st).is_empty(), "staged: nothing against the index");
    keys(&mut st, ":git_gutter_base<CR>");
    wait(&mut st, "the HEAD markers", |st| !kinds(st).is_empty());
    assert_eq!(kinds(&st), vec![(HunkKind::Modified, 1, 2)]);
    keys(&mut st, ":set nogit_gutter<CR>");
    assert!(st.ed.hunks().is_empty());
    assert!(st.ed.git_hunks.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn branch_and_file_states_reach_the_chrome() {
    let Some(dir) = repo(&[("src/a.rs", "fn a() {}\n"), ("b.txt", "b\n")]) else { return };
    std::fs::write(dir.join("src/a.rs"), "fn a() { 1 }\n").unwrap();
    std::fs::write(dir.join("new.txt"), "n\n").unwrap();
    let mut st = state();
    st.open_at(&dir.join("b.txt"), None, None);
    st.after_input();
    wait(&mut st, "the status", |st| crate::git::status_ui::branch(st).is_some());
    let b = crate::git::status_ui::branch(&st).unwrap();
    assert_eq!((b.label.as_str(), b.dirty), ("main", true));
    assert_eq!(b.name(), "main*");
    // the sidebar's project gets the same repository's states
    st.ed.project = Some(dir.clone());
    keys(&mut st, ":set sidebar<CR>");
    wait(&mut st, "the project's repository", |st| st.git.known_dir_repo(&dir).is_some());
    let c = st.colors;
    let deco = crate::git::status_ui::tree_deco(&st.git, &st.ed.features, &dir).expect("decorations");
    assert_eq!(deco.row(&dir.join("src/a.rs"), false, false, &c).unwrap().letter, "M");
    assert_eq!(deco.row(&dir.join("src"), true, false, &c).unwrap().letter, "•");
    assert_eq!(deco.row(&dir.join("new.txt"), false, false, &c).unwrap().letter, "?");
    assert_eq!(deco.row(&dir.join("b.txt"), false, false, &c), None);
    // off: nothing shows
    keys(&mut st, ":set nogit_status<CR>");
    assert!(crate::git::status_ui::branch(&st).is_none());
    assert!(!st.ed.features.on(Feature::GitStatus));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn nothing_starts_outside_a_repository() {
    let dir = std::env::temp_dir().join(format!("shrew-no-git-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("x.txt");
    std::fs::write(&file, "x\n").unwrap();
    let mut st = state();
    // a scratch buffer asks nothing at all
    st.after_input();
    assert!(st.git.link.service.is_none(), "no file, no thread");
    st.open_at(&file, None, None);
    st.after_input();
    wait(&mut st, "discovery", |st| !st.git.dirs.is_empty());
    keys(&mut st, "ihello<Esc>");
    st.poll();
    assert!(st.ed.git_hunks.is_empty());
    assert!(st.git.statuses.is_empty() && st.git.watching.is_empty(), "nothing watched");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn nothing_git_starts_before_the_first_frame() {
    let Some(dir) = repo(&[("a.txt", "one\n")]) else { return };
    let mut st = state();
    // the real app starts inactive and wakes git after its first frame (`State::start_deferred`)
    st.git.active = false;
    st.open_at(&dir.join("a.txt"), None, None);
    st.after_input();
    st.poll();
    assert!(st.git.link.service.is_none(), "a lookup started before the first frame");
    assert!(st.ed.hunks().is_empty());
    // the first frame is up: the lookup starts, and the markers follow
    st.git.active = true;
    st.after_input();
    keys(&mut st, "Ox<Esc>");
    wait(&mut st, "markers", |st| !st.ed.hunks().is_empty());
}
