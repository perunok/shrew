//! The git_* tools against real repositories built with the `git` program (every test passes quietly without it).

use super::*;
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Ann Author")
        .env("GIT_AUTHOR_EMAIL", "ann@example.com")
        .env("GIT_COMMITTER_NAME", "Ann Author")
        .env("GIT_COMMITTER_EMAIL", "ann@example.com")
        .env("GIT_AUTHOR_DATE", "2024-03-05T10:20:30+0200")
        .env("GIT_COMMITTER_DATE", "2024-03-05T10:20:30+0200")
        .env_remove("GIT_DIR")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn have_git() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// A repository with two commits, then an unstaged edit of a.txt, a staged edit of b.txt and an untracked file;
/// the editor has a.txt open.
fn repo() -> Option<(tempfile::TempDir, PathBuf, FakeHost)> {
    if !have_git() {
        return None;
    }
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(root.join("b.txt"), "b1\nb2\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "First commit\n\nWith a body line."]);
    std::fs::write(root.join("a.txt"), "one\n2\nthree\nfour\n").unwrap();
    git(&root, &["commit", "-q", "-am", "Change a"]);
    git(&root, &["tag", "v1"]);
    std::fs::write(root.join("a.txt"), "one\n2\nthree\nfour\nfive\n").unwrap();
    std::fs::write(root.join("b.txt"), "b1\nB2\n").unwrap();
    git(&root, &["add", "b.txt"]);
    std::fs::write(root.join("new.txt"), "fresh\n").unwrap();
    let r = root.clone();
    let host = FakeHost::new(move |ed| {
        ed.cwd = r.clone();
        ed.open(&r.join("a.txt")).unwrap();
    });
    Some((d, root, host))
}

#[test]
fn status_lists_branch_and_changes() {
    let Some((_d, _root, host)) = repo() else { return };
    let (text, data) = ok(&host, "git_status", json!({}));
    assert!(text.starts_with("## main\n"), "{text}");
    assert!(text.contains(" M a.txt\n") && text.contains("M  b.txt\n") && text.contains("?? new.txt\n"), "{text}");
    assert_eq!(data["branch"], "main");
    assert_eq!(data["clean"], false);
    let b = data["entries"].as_array().unwrap().iter().find(|e| e["path"] == "b.txt").unwrap();
    assert_eq!((b["staged"].as_str(), b["unstaged"].as_str()), (Some("M"), None));
    // limited to a folder, and untracked files can be left out
    let (text, _) = ok(&host, "git_status", json!({ "path": "src" }));
    assert!(text.contains("Nothing to commit"), "{text}");
    let (text, _) = ok(&host, "git_status", json!({ "untracked": false }));
    assert!(!text.contains("new.txt"), "{text}");
    let (text, data) = ok(&host, "git_status", json!({ "limit": 1 }));
    assert!(text.contains("more entries not shown"), "{text}");
    assert_eq!(data["truncated"], true);
}

#[test]
fn diff_modes() {
    let Some((_d, _root, host)) = repo() else { return };
    let (text, data) = ok(&host, "git_diff", json!({}));
    assert!(text.contains("diff --git a/a.txt b/a.txt") && text.contains("+five"), "{text}");
    assert!(!text.contains("b.txt") || text.contains("new.txt"), "unstaged shows no staged-only file: {text}");
    assert!(text.contains("+++ b/new.txt") && text.contains("--- /dev/null"), "untracked shows as added: {text}");
    assert_eq!(data["files"].as_array().unwrap().len(), 2);

    let (text, _) = ok(&host, "git_diff", json!({ "mode": "staged" }));
    assert!(text.contains("-b2\n+B2\n"), "{text}");
    assert!(!text.contains("a.txt"), "{text}");

    let (text, data) = ok(&host, "git_diff", json!({ "mode": "head", "path": "a.txt", "context": 0 }));
    assert!(text.contains("@@ -2,0 +3,0") || text.contains("+five"), "{text}");
    assert_eq!(data["files"].as_array().unwrap().len(), 1);

    // the second commit against its parent, and against an explicit base
    let (text, _) = ok(&host, "git_diff", json!({ "mode": "commit", "rev": "v1" }));
    assert!(text.contains("-two\n+2\n") && text.contains("+four"), "{text}");
    let (text, _) =
        ok(&host, "git_diff", json!({ "mode": "commit", "rev": "HEAD", "base": "HEAD~1", "path": "a.txt" }));
    assert!(text.contains("-two\n+2\n"), "{text}");

    let (text, data) = ok(&host, "git_diff", json!({ "mode": "staged", "path": "src" }));
    assert!(text.starts_with("No differences"), "{text}");
    assert_eq!(data["files"], json!([]));
}

#[test]
fn diff_reports_unsaved_edits_it_cannot_see() {
    let Some((_d, _root, host)) = repo() else { return };
    ui(&host, |ed| {
        let end = ed.buf().doc.rope.len_chars();
        ed.buf_mut().apply(
            shrew_core::ChangeSet::new(vec![shrew_core::Change { from: end, to: end, text: "six\n".into() }]),
            None,
        );
    });
    let (text, _) = ok(&host, "git_diff", json!({ "path": "a.txt" }));
    assert!(text.contains("unsaved edits"), "{text}");
    assert!(!text.contains("six"), "{text}");
}

#[test]
fn log_pages_and_follows_a_file() {
    let Some((_d, _root, host)) = repo() else { return };
    let (text, data) = ok(&host, "git_log", json!({}));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].contains("2024-03-05 Ann Author: Change a (HEAD -> main, tag: v1)"), "{text}");
    assert_eq!(data["more"], false);

    let (text, data) = ok(&host, "git_log", json!({ "limit": 1 }));
    assert!(text.contains("call again with skip=1"), "{text}");
    assert_eq!(data["next_skip"], 1);
    let (text, _) = ok(&host, "git_log", json!({ "limit": 1, "skip": 1 }));
    assert!(text.contains("First commit") && !text.contains("skip="), "{text}");

    let (text, data) = ok(&host, "git_log", json!({ "path": "b.txt" }));
    assert_eq!(data["commits"].as_array().unwrap().len(), 1, "{text}");
    assert!(text.contains("First commit"), "{text}");
    let msg = fails(&host, "git_log", json!({ "path": "b.txt", "rev": "v1" }));
    assert!(msg.contains("either"), "{msg}");
    let msg = fails(&host, "git_log", json!({ "rev": "nonesuch" }));
    assert!(msg.to_lowercase().contains("nonesuch") || msg.contains("unknown"), "{msg}");
}

#[test]
fn blame_a_range_and_unsaved_lines() {
    let Some((_d, _root, host)) = repo() else { return };
    let (text, data) = ok(&host, "git_blame", json!({ "path": "a.txt", "start_line": 1, "end_line": 4 }));
    let ranges = data["ranges"].as_array().unwrap();
    assert!(ranges.iter().any(|r| r["summary"] == "First commit" && r["start_line"] == 1), "{text}");
    assert!(ranges.iter().any(|r| r["summary"] == "Change a" && r["start_line"] == 2), "{text}");
    assert!(text.contains("Ann Author") && text.contains("2024-03-05"), "{text}");
    // the saved edit (line 5) is not committed yet
    let (_, data) = ok(&host, "git_blame", json!({ "path": "a.txt", "start_line": 5 }));
    assert_eq!(data["ranges"][0]["uncommitted"], true);
    // no path: the buffer on screen
    let (_, data) = ok(&host, "git_blame", json!({ "start_line": 1, "end_line": 1 }));
    assert_eq!(data["path"], "a.txt");
    // before the file's history (rev), the second line was still "two"
    let (_, data) = ok(&host, "git_blame", json!({ "path": "a.txt", "rev": "HEAD~1" }));
    assert_eq!(data["lines"], 3);
    // an unsaved edit in the buffer shows as not committed
    ui(&host, |ed| {
        ed.buf_mut().apply(
            shrew_core::ChangeSet::new(vec![shrew_core::Change { from: 0, to: 0, text: "zero\n".into() }]),
            None,
        );
    });
    let (_, data) = ok(&host, "git_blame", json!({ "path": "a.txt", "start_line": 1, "end_line": 1 }));
    assert_eq!(data["ranges"][0]["uncommitted"], true);
    assert!(fails(&host, "git_blame", json!({ "path": "a.txt", "start_line": 5, "end_line": 2 })).contains("after"));
    assert!(!fails(&host, "git_blame", json!({ "path": "new.txt" })).is_empty());
}

#[test]
fn show_commit_with_and_without_the_diff() {
    let Some((_d, _root, host)) = repo() else { return };
    let (text, data) = ok(&host, "git_show_commit", json!({ "rev": "HEAD~1" }));
    assert!(text.contains("Author: Ann Author <ann@example.com>"), "{text}");
    assert!(
        text.contains("    First commit\n    \n    With a body line.") || text.contains("With a body line."),
        "{text}"
    );
    assert!(text.contains("A  a.txt  +3 -0"), "{text}");
    assert!(!text.contains("@@"), "{text}");
    assert_eq!(data["files"].as_array().unwrap().len(), 3);

    let (text, _) = ok(&host, "git_show_commit", json!({ "rev": "HEAD~1", "diff": true, "path": "src" }));
    assert!(text.contains("--- /dev/null\n+++ b/src/lib.rs\n@@ -0,0 +1 @@\n+pub fn a() {}"), "{text}");
    assert!(!text.contains("b.txt"), "{text}");

    let (text, _) = ok(&host, "git_show_commit", json!({ "diff": true }));
    assert!(
        text.contains("commit ") && text.contains("(HEAD -> main, tag: v1)") && text.contains("-two\n+2"),
        "{text}"
    );
}

#[test]
fn paths_stay_in_the_repository_and_a_non_repo_is_reported() {
    let Some((d, root, host)) = repo() else { return };
    let outside = d.path().join("elsewhere.txt");
    std::fs::write(&outside, "x\n").unwrap();
    let msg = fails(&host, "git_diff", json!({ "path": outside.to_string_lossy() }));
    assert!(msg.contains("outside"), "{msg}");
    let msg = fails(&host, "git_blame", json!({ "path": "../elsewhere.txt" }));
    assert!(msg.contains("outside"), "{msg}");
    assert!(root.join("a.txt").exists());

    // a working directory that is no repository
    let plain = tempfile::tempdir().unwrap();
    let p = std::fs::canonicalize(plain.path()).unwrap();
    let host = FakeHost::new(move |ed| ed.cwd = p.clone());
    assert!(fails(&host, "git_status", json!({})).contains("not inside a git repository"));
    assert!(fails(&host, "git_log", json!({})).contains("not inside a git repository"));
}

#[test]
fn a_fresh_repository_without_commits_works() {
    if !have_git() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("f.txt"), "x\n").unwrap();
    let r = root.clone();
    let host = FakeHost::new(move |ed| ed.cwd = r.clone());
    let (text, _) = ok(&host, "git_status", json!({}));
    assert!(text.contains("(no commits yet)") && text.contains("?? f.txt"), "{text}");
    let (text, _) = ok(&host, "git_log", json!({}));
    assert_eq!(text, "No commits.");
    let (text, _) = ok(&host, "git_diff", json!({}));
    assert!(text.contains("+++ b/f.txt"), "{text}");
}

#[test]
fn dates_are_formatted_in_the_authors_zone() {
    assert_eq!(crate::tools::git_iso_for_tests(0, 0), "1970-01-01T00:00:00+00:00");
    assert_eq!(crate::tools::git_iso_for_tests(1_709_626_830, 7200), "2024-03-05T10:20:30+02:00");
    assert_eq!(crate::tools::git_iso_for_tests(951_782_400, -19_800), "2000-02-28T18:30:00-05:30");
}
