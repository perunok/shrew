//! The git module against real repositories (temporary ones, made with the git program): the panel's keys stage,
//! discard and commit, the lists switch branches, the log loads, blame comes, conflicts resolve, and nothing
//! starts outside a repository.

use crate::git::{ago_at, ago_short};
use crate::state::State;
use shrew_core::KeyPress;
use shrew_core::features::Feature;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Repo {
    dir: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Proc::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_AUTHOR_NAME", "Tess")
        .env("GIT_AUTHOR_EMAIL", "tess@example.org")
        .env("GIT_COMMITTER_NAME", "Tess")
        .env("GIT_COMMITTER_EMAIL", "tess@example.org")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repository with `a.txt` and `b.txt` committed; `None` (the test is skipped) without a git program.
fn repo(name: &str) -> Option<Repo> {
    if !shrew_git::git_available() {
        eprintln!("no git program: skipped");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("shrew-git-ui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "Tess"), ("user.email", "tess@example.org"), ("commit.gpgsign", "false")] {
        git(&dir, &["config", k, v]);
    }
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(dir.join("b.txt"), "bee\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "first"]);
    Some(Repo { dir })
}

fn state(dir: &Path) -> State {
    let mut st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
    st.ed.cwd = dir.to_path_buf();
    st
}

/// Waits for every git answer the state is waiting for.
fn settle(st: &mut State) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        st.poll();
        if !crate::git::busy(st) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "git did not answer: {:?} {:?} {}",
            st.git.scm.tickets,
            st.git.asking,
            st.git.scm.status_pending
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    st.poll();
}

/// Types keys as a person would: each once git has answered the one before.
fn keys(st: &mut State, s: &str) {
    for k in KeyPress::parse_seq(s) {
        settle(st);
        st.key(k);
    }
    settle(st);
}

fn porcelain(dir: &Path) -> String {
    git(dir, &["status", "--porcelain"])
}

fn select(st: &mut State, path: &str, section: crate::git::panel::Section) {
    let p = &mut st.git.panel;
    p.selected = p
        .rows
        .iter()
        .position(|r| matches!(r, crate::git::panel::Row::File(f) if f.path == path && f.section == section))
        .unwrap_or_else(|| panic!("{path} in {section:?}: {:?}", p.rows));
}

#[test]
fn the_panel_stages_unstages_and_commits() {
    let Some(r) = repo("commit") else { return };
    std::fs::write(r.dir.join("a.txt"), "one\n2\nthree\n").unwrap();
    let mut st = state(&r.dir);
    st.open_at(&r.dir.join("a.txt"), None, None);
    keys(&mut st, "<C-S-g>");
    assert!(st.git.panel.focused && crate::git::panel::draw::shown(&st));
    assert_eq!(st.git.panel.total(), 1);
    select(&mut st, "a.txt", crate::git::panel::Section::Changes);
    keys(&mut st, "s");
    assert_eq!(porcelain(&r.dir), "M  a.txt\n");
    assert_eq!(st.git.panel.files[1].len(), 1, "the panel shows it staged");
    keys(&mut st, "u");
    assert_eq!(porcelain(&r.dir), " M a.txt\n");
    keys(&mut st, "S");
    // a commit without a message is refused and the message box gets the keys
    keys(&mut st, "<C-CR>");
    assert_eq!(st.git.panel.focus, crate::git::panel::Focus::Message);
    keys(&mut st, "Change two<CR><CR>Details.<C-CR>");
    assert_eq!(git(&r.dir, &["log", "-1", "--format=%B"]).trim(), "Change two\n\nDetails.");
    assert_eq!(porcelain(&r.dir), "");
    assert!(st.git.panel.message.text.is_empty(), "the box is cleared after a commit");
    assert_eq!(st.git.panel.total(), 0);
}

#[test]
fn discarding_asks_first() {
    let Some(r) = repo("discard") else { return };
    std::fs::write(r.dir.join("b.txt"), "changed\n").unwrap();
    std::fs::write(r.dir.join("new.txt"), "new\n").unwrap();
    let mut st = state(&r.dir);
    st.open_at(&r.dir.join("a.txt"), None, None);
    keys(&mut st, "<C-S-g>");
    select(&mut st, "b.txt", crate::git::panel::Section::Changes);
    keys(&mut st, "x");
    assert!(st.git.ask.is_some(), "a question first");
    keys(&mut st, "n");
    assert_eq!(std::fs::read_to_string(r.dir.join("b.txt")).unwrap(), "changed\n");
    keys(&mut st, "xy");
    assert_eq!(std::fs::read_to_string(r.dir.join("b.txt")).unwrap(), "bee\n");
    select(&mut st, "new.txt", crate::git::panel::Section::Untracked);
    keys(&mut st, "xy");
    assert!(!r.dir.join("new.txt").exists(), "an untracked file is deleted");
}

#[test]
fn the_branch_list_creates_switches_and_deletes() {
    let Some(r) = repo("branches") else { return };
    let mut st = state(&r.dir);
    st.open_at(&r.dir.join("a.txt"), None, None);
    keys(&mut st, ":git_branches<CR>");
    assert!(st.ed.picker.is_some());
    keys(&mut st, "topic<C-CR>");
    assert_eq!(git(&r.dir, &["branch", "--show-current"]).trim(), "topic");
    keys(&mut st, ":git_branches<CR>");
    let p = st.ed.picker.as_ref().unwrap();
    assert_eq!(p.current().map(|i| i.label.as_str()), Some("main"), "the other branch comes first");
    keys(&mut st, "<CR>");
    assert_eq!(git(&r.dir, &["branch", "--show-current"]).trim(), "main");
    keys(&mut st, ":git_branches<CR>topic<Del>y");
    assert!(!git(&r.dir, &["branch"]).contains("topic"), "deleted after asking");
}

#[test]
fn stashes_push_and_pop() {
    let Some(r) = repo("stash") else { return };
    std::fs::write(r.dir.join("a.txt"), "stashed\n").unwrap();
    let mut st = state(&r.dir);
    st.open_at(&r.dir.join("b.txt"), None, None);
    keys(&mut st, ":git_stash<CR>wip<CR>");
    assert_eq!(porcelain(&r.dir), "");
    assert!(git(&r.dir, &["stash", "list"]).contains("wip"));
    keys(&mut st, ":git_stashes<CR><CR>");
    assert_eq!(porcelain(&r.dir), " M a.txt\n");
    assert_eq!(git(&r.dir, &["stash", "list"]), "");
}

#[test]
fn the_log_tab_pages_and_shows_a_commit() {
    let Some(r) = repo("log") else { return };
    for i in 0..5 {
        std::fs::write(r.dir.join("a.txt"), format!("{i}\n")).unwrap();
        git(&r.dir, &["commit", "-q", "-am", &format!("change {i}")]);
    }
    let mut st = state(&r.dir);
    st.open_at(&r.dir.join("a.txt"), None, None);
    keys(&mut st, ":git_log<CR>");
    let v = crate::git::log::active(&st).expect("the log tab is active");
    assert_eq!(v.rows.len(), 6);
    assert!(v.rows.iter().all(|r| r.graph.is_some()));
    assert_eq!(v.rows[0].commit.summary, "change 4");
    keys(&mut st, "j<CR>");
    let v = crate::git::log::active(&st).unwrap();
    let d = v.details.as_ref().and_then(|d| d.data.as_ref()).expect("details came");
    assert_eq!(d.message.trim(), "change 3");
    assert_eq!(d.files.len(), 1);
    keys(&mut st, "<Esc>/change 1<CR>");
    assert_eq!(crate::git::log::active(&st).unwrap().current().unwrap().commit.summary, "change 1");
    // the tab is a buffer: closing it forgets the view
    keys(&mut st, "q");
    assert!(crate::git::log::active(&st).is_none());
    assert!(st.git.views.ids().is_empty());
}

#[test]
fn blame_comes_for_the_active_file() {
    let Some(r) = repo("blame") else { return };
    let mut st = state(&r.dir);
    st.open_at(&r.dir.join("a.txt"), None, None);
    keys(&mut st, ":git_blame<CR>");
    let b = st.git.blame_view.get(st.ed.buf().id).expect("blame is on");
    assert!(b.done && b.error.is_none(), "{:?}", b.error);
    assert_eq!(b.at(2).map(|h| h.commit.summary.as_str()), Some("first"));
    keys(&mut st, ":git_blame<CR>");
    assert!(st.git.blame_view.get(st.ed.buf().id).is_none(), "again turns it off");
}

#[test]
fn conflicts_resolve_and_are_marked_resolved() {
    let Some(r) = repo("conflict") else { return };
    git(&r.dir, &["switch", "-q", "-c", "other"]);
    std::fs::write(r.dir.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    git(&r.dir, &["commit", "-q", "-am", "theirs"]);
    git(&r.dir, &["switch", "-q", "main"]);
    std::fs::write(r.dir.join("a.txt"), "one\nzwei\nthree\n").unwrap();
    git(&r.dir, &["commit", "-q", "-am", "ours"]);
    let merge = Proc::new("git").args(["merge", "-q", "other"]).current_dir(&r.dir).output().unwrap();
    assert!(!merge.status.success(), "a conflict");
    let mut st = state(&r.dir);
    st.open_at(&r.dir.join("a.txt"), None, None);
    keys(&mut st, "<C-S-g><Esc>");
    assert_eq!(st.git.panel.files[0].len(), 1, "Merge Changes holds it");
    keys(&mut st, "]x");
    assert_eq!(shrew_core::text::line_of(st.ed.buf().rope(), st.ed.buf().sel.primary().head), 1);
    keys(&mut st, ":git_conflict_both<CR>");
    assert_eq!(st.ed.buf().rope().to_string(), "one\nzwei\nTWO\nthree\n");
    keys(&mut st, ":git_mark_resolved<CR>");
    assert_eq!(porcelain(&r.dir), "M  a.txt\n", "saved and staged");
    assert_eq!(std::fs::read_to_string(r.dir.join("a.txt")).unwrap(), "one\nzwei\nTWO\nthree\n");
}

#[test]
fn nothing_starts_outside_a_repository_or_with_the_feature_off() {
    let dir = std::env::temp_dir().join(format!("shrew-git-ui-{}-norepo", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("x.txt"), "x\n").unwrap();
    let mut st = state(&dir);
    // the change markers, inline blame and status look each file up (see the markers' tests); without them the
    // panel alone starts nothing until it is used
    for f in [Feature::GitGutter, Feature::GitBlameInline, Feature::GitStatus] {
        st.ed.features.set(f, false);
    }
    st.open_at(&dir.join("x.txt"), None, None);
    keys(&mut st, "ihello<Esc>:w<CR>");
    assert!(st.git.link.service.is_none(), "typing and saving start nothing");
    keys(&mut st, ":set nogit_panel<CR>:git_log<CR>");
    assert!(st.git.link.service.is_none());
    assert!(st.ed.message.as_ref().is_some_and(|m| m.0.contains("git_panel")));
    keys(&mut st, ":set git_panel<CR><C-S-g>");
    // the panel looked (one discovery) and found nothing to watch
    assert!(st.git.scm.repo.is_none());
    assert!(st.git.scm.status.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ages_read_naturally() {
    let now = 1_000_000_000;
    assert_eq!(ago_at(now - 5, now), "just now");
    assert_eq!(ago_at(now - 120, now), "2 minutes ago");
    assert_eq!(ago_at(now - 3600, now), "1 hour ago");
    assert_eq!(ago_at(now - 3 * 86_400, now), "3 days ago");
    assert_eq!(ago_at(now - 15 * 86_400, now), "2 weeks ago");
    assert_eq!(ago_at(now - 400 * 86_400, now), "1 year ago");
    assert_eq!(ago_short(now - 90 * 86_400, now), "3mo");
    assert_eq!(ago_at(now + 50, now), "just now", "a clock ahead of ours");
}

/// The whole flow across the parts: an edit shows in the gutter, `ghs` stages it, the panel lists it as staged,
/// Enter opens it in the diff view, the panel commits it, the log shows the commit and `d` opens its diff.
#[test]
fn edit_stage_review_commit_and_log() {
    let Some(r) = repo("flow") else { return };
    let mut st = state(&r.dir);
    st.open_at(&r.dir.join("a.txt"), None, None);
    keys(&mut st, "jciwTWO<Esc>:w<CR>");
    assert_eq!(st.ed.hunks().len(), 1, "the gutter marks the changed line");

    keys(&mut st, "ghs");
    assert_eq!(porcelain(&r.dir), "M  a.txt\n");
    assert!(st.ed.hunks().is_empty(), "staged: nothing left against the index");

    keys(&mut st, "<C-S-g>");
    assert_eq!(st.git.panel.files[1].len(), 1, "Staged Changes holds it");
    select(&mut st, "a.txt", crate::git::panel::Section::Staged);
    keys(&mut st, "<CR>");
    assert!(crate::git::diffview::is_active(&st), "Enter opened the diff view");
    assert_eq!(st.ed.buf().doc.display_name(), "diff: a.txt (HEAD ↔ index)");
    assert_eq!(diff_changes(&mut st), 1);

    keys(&mut st, "<C-S-g>c");
    assert_eq!(st.git.panel.focus, crate::git::panel::Focus::Message);
    keys(&mut st, "Shout two<C-CR>");
    assert_eq!(git(&r.dir, &["log", "-1", "--format=%s"]).trim(), "Shout two");
    assert_eq!(porcelain(&r.dir), "");

    keys(&mut st, "<Esc>:git_log<CR>");
    let v = crate::git::log::active(&st).expect("the log tab");
    assert_eq!(v.rows.len(), 2);
    assert_eq!(v.rows[0].commit.summary, "Shout two");
    assert!(v.rows.iter().all(|r| r.graph.is_some()), "every row has its graph");
    keys(&mut st, "d");
    assert!(crate::git::diffview::is_active(&st), "d opened the commit's diff");
    assert!(st.ed.buf().doc.display_name().starts_with("commit "), "{}", st.ed.buf().doc.display_name());
}

/// A click on the blame column (here: Enter on a blamed line) opens what that commit did to the file.
#[test]
fn blame_opens_the_commit_diff() {
    let Some(r) = repo("blamediff") else { return };
    std::fs::write(r.dir.join("a.txt"), "one\n2\nthree\n").unwrap();
    git(&r.dir, &["commit", "-q", "-am", "second"]);
    let mut st = state(&r.dir);
    st.open_at(&r.dir.join("a.txt"), None, None);
    keys(&mut st, ":git_blame<CR>j<CR>");
    assert!(crate::git::diffview::is_active(&st));
    let name = st.ed.buf().doc.display_name();
    assert!(name.starts_with("diff: a.txt"), "{name}");
    assert_eq!(diff_changes(&mut st), 1, "the commit changed one line of it");
}

/// The changes the active diff tab shows (its rows are laid out as it would be drawn).
fn diff_changes(st: &mut State) -> usize {
    let buf = st.ed.buf().id;
    let v = st.git.diff.views.get_mut(&buf).expect("a diff tab");
    if v.stale {
        v.rebuild();
    }
    v.rows.starts.len()
}
