//! The diff view against real repositories (built with the `git` program; skipped when it is missing).

use super::*;
use shrew_core::KeyPress;
use shrew_core::diffview::RowKind;
use std::process::Command as Proc;
use std::time::{Duration, Instant};

fn state() -> State {
    State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true)
}

/// A fresh repository in the temp folder, with an identity of its own.
struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Option<Repo> {
        if !shrew_git::git_available() {
            eprintln!("git not found: skipping");
            return None;
        }
        let dir = std::env::temp_dir().join(format!("shrew-diffview-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = Repo(shrew_git::canonicalize(&dir).unwrap());
        r.git(&["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Test"),
            ("user.email", "t@example.com"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            r.git(&["config", k, v]);
        }
        r.git(&["config", "core.autocrlf", "false"]);
        Some(r)
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Proc::new("git")
            .current_dir(&self.0)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn write(&self, path: &str, text: &str) -> PathBuf {
        let p = self.0.join(path);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&p, text).unwrap();
        p
    }

    fn commit(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Lets the service and the re-diff threads finish, as the event loop would.
fn settle(st: &mut State) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        crate::git::poll(st);
        st.after_input();
        if !crate::git::busy(st) {
            crate::git::poll(st);
            break;
        }
        assert!(Instant::now() < deadline, "the diff view never settled");
        std::thread::sleep(Duration::from_millis(2));
    }
    let buf = st.ed.buf().id;
    if let Some(v) = st.git.diff.views.get_mut(&buf) {
        v.hits.page = 30;
        if v.stale {
            v.rebuild();
        }
    }
}

fn keys(st: &mut State, s: &str) {
    for k in KeyPress::parse_seq(s) {
        st.key(k);
    }
}

fn view(st: &State) -> &DiffView {
    st.git.diff.active(&st.ed).expect("a diff tab is active")
}

fn lines(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn white_space_does_not_count_when_ignored() {
    let old = Text::new("fn a() {\n    x();\n}\nkeep\n");
    let new = Text::new("fn a() {\n        x();\n}\nchanged\n");
    let (hunks, _) = ignore_whitespace(&old, &new);
    assert_eq!(hunks, vec![Hunk { old: 3..4, new: 3..4 }], "the re-indented line is not a change");
    let real = shrew_git::diff::diff(&old, &new, EolMode::Exact);
    assert_eq!(real.len(), 2);
}

#[test]
fn titles_name_both_sides() {
    assert_eq!(DiffSpec::file("/r/src/a.rs", Rev::Index, Rev::Worktree).title(), "diff: a.rs (index ↔ working tree)");
    assert_eq!(DiffSpec::staged().title(), "diff: staged (HEAD ↔ index)");
    assert_eq!(DiffSpec::commit(Rev::Named("v1".into())).title(), "commit v1");
    assert_eq!(DiffSpec::tree(Rev::Named("main".into()), Rev::Head).title(), "diff: main ↔ HEAD");
}

#[test]
fn file_diff_folds_navigates_and_switches_layout() {
    let Some(r) = Repo::new("file") else { return };
    let path = r.write("a.txt", &lines(100));
    r.commit("one");
    let mut text = lines(100).replace("line 10\n", "line ten\n").replace("line 80\n", "");
    text.push_str("tail\n");
    std::fs::write(&path, &text).unwrap();
    let mut st = state();
    // from a closed file: the diff reads the working tree from disk
    st.open_diff(DiffSpec::file(&path, Rev::Index, Rev::Worktree));
    settle(&mut st);
    let v = view(&st);
    assert_eq!(st.ed.buf().doc.display_name(), "diff: a.txt (index ↔ working tree)");
    assert_eq!(v.rows.starts.len(), 3, "{:?}", v.message);
    assert_eq!(v.total_stats(), (2, 2));
    assert!(v.rows.rows.iter().any(|r| r.kind == RowKind::Fold), "unchanged lines fold");
    // ]c walks the changes, [c comes back
    keys(&mut st, "gg]c");
    let at = view(&st).cursor;
    assert_eq!(view(&st).rows.rows[at].old, 10);
    keys(&mut st, "]c]c");
    assert_eq!(view(&st).rows.rows[view(&st).cursor].new, 99, "the added tail");
    keys(&mut st, "[c");
    assert_eq!(view(&st).rows.rows[view(&st).cursor].old, 80);
    // Enter on a fold opens it
    keys(&mut st, "gg");
    assert_eq!(view(&st).rows.rows[0].kind, RowKind::Fold);
    keys(&mut st, "<CR>");
    settle(&mut st);
    assert_eq!(view(&st).rows.rows[0].kind, RowKind::Same);
    // t: unified, one row per removed and added line
    let split = view(&st).rows.len();
    keys(&mut st, "t");
    settle(&mut st);
    assert_eq!(view(&st).opts.layout, Layout::Unified);
    assert_eq!(view(&st).rows.len(), split + 1, "the modified line takes two rows");
    // q closes the tab
    keys(&mut st, "q");
    st.after_input();
    assert!(st.git.diff.views.is_empty());
}

#[test]
fn an_open_buffer_is_diffed_live() {
    let Some(r) = Repo::new("live") else { return };
    let path = r.write("b.rs", &lines(20));
    r.commit("one");
    let mut st = state();
    st.open_at(&path, Some(5), None);
    keys(&mut st, ":Gdiff<CR>");
    settle(&mut st);
    assert_eq!(view(&st).rows.starts.len(), 0, "nothing changed yet");
    assert!(view(&st).sections[0].live.is_some());
    // an edit in the buffer shows up without saving
    let file = st.ed.buffers.iter().position(|b| b.doc.path.as_deref() == Some(path.as_path())).unwrap();
    let diff = st.ed.active;
    st.ed.switch_to(file);
    keys(&mut st, "ggOnew first line<Esc>");
    st.ed.switch_to(diff);
    st.after_input();
    settle(&mut st);
    let v = view(&st);
    assert_eq!(v.rows.starts.len(), 1);
    assert_eq!(v.total_stats(), (1, 0));
    // X reverts it in the buffer (undoable), and the diff follows
    keys(&mut st, "gg]cX");
    settle(&mut st);
    assert_eq!(st.ed.buffers[file].doc.rope.to_string(), lines(20));
    assert_eq!(view(&st).rows.starts.len(), 0);
}

#[test]
fn staging_a_change_from_the_diff() {
    let Some(r) = Repo::new("stage") else { return };
    let path = r.write("c.txt", &lines(30));
    r.commit("one");
    std::fs::write(&path, lines(30).replace("line 3\n", "three\n").replace("line 25\n", "twenty-five\n")).unwrap();
    let mut st = state();
    st.open_diff(DiffSpec::file(&path, Rev::Index, Rev::Worktree));
    settle(&mut st);
    assert_eq!(view(&st).rows.starts.len(), 2);
    // s on the first change stages just that one
    keys(&mut st, "gg]cs");
    settle(&mut st);
    let staged = r.git(&["diff", "--cached"]);
    assert!(staged.contains("+three") && !staged.contains("twenty-five"), "{staged}");
    assert_eq!(view(&st).rows.starts.len(), 1, "the view reloads");
    // the staged diff shows it, and u takes it back
    keys(&mut st, ":git_diff_staged<CR>");
    settle(&mut st);
    let v = view(&st);
    assert!(v.multi);
    assert_eq!(v.sections.len(), 1, "{:?} {:?} {:?}", v.message, v.left, v.repo);
    assert_eq!(v.rows.rows[0].kind, RowKind::File);
    keys(&mut st, "gg]cu");
    settle(&mut st);
    assert_eq!(r.git(&["diff", "--cached"]), "");
}

#[test]
fn commit_diff_lists_its_files() {
    let Some(r) = Repo::new("commit") else { return };
    r.write("x.txt", "a\nb\n");
    r.write("dir/y.txt", "y\n");
    r.commit("first");
    r.write("x.txt", "a\nB\n");
    r.write("dir/z.txt", "z\n");
    std::fs::remove_file(r.0.join("dir/y.txt")).unwrap();
    r.commit("second");
    let mut st = state();
    st.open_diff(DiffSpec::commit(Rev::Head).in_repo(&r.0));
    settle(&mut st);
    let v = view(&st);
    assert_eq!(v.summary.as_deref(), Some("second"));
    let files: Vec<&str> = v.sections.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(files, ["dir/y.txt", "dir/z.txt", "x.txt"]);
    assert_eq!(v.total_stats(), (2, 2));
    // ]f jumps between file headers; Enter on one collapses it
    keys(&mut st, "gg]f");
    assert_eq!(view(&st).rows.rows[view(&st).cursor].file, 1);
    keys(&mut st, "<CR>");
    settle(&mut st);
    assert!(view(&st).sections[1].collapsed);
    // the root commit: everything added
    st.open_diff(DiffSpec::commit(Rev::Named("HEAD~1".into())).in_repo(&r.0));
    settle(&mut st);
    let v = view(&st);
    assert_eq!(v.left, Some(Rev::Empty));
    assert_eq!(v.total_stats(), (3, 0));
}

#[test]
fn outside_a_repository_it_says_so() {
    let dir = std::env::temp_dir().join(format!("shrew-diffview-norepo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.txt");
    std::fs::write(&path, "x\n").unwrap();
    let mut st = state();
    st.open_diff(DiffSpec::file(&path, Rev::Index, Rev::Worktree));
    settle(&mut st);
    // a temp folder may sit inside some repository; when not, the view says why it is empty
    let v = view(&st);
    if v.repo.is_none() {
        assert_eq!(v.message.as_deref(), Some("not in a git repository"));
    }
    std::fs::remove_dir_all(&dir).ok();
}
