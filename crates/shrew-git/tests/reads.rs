//! Discovery, HEAD, blobs and status, checked against the git program's own answers.

mod common;
use common::{git_in, repo_or_skip};
use shrew_git::*;

fn ours(status: &Status) -> Vec<String> {
    let mut v: Vec<String> = status
        .entries
        .iter()
        .map(|e| {
            // git quotes nothing here because the test paths avoid special characters, except where noted
            format!("{} {}", e.short_code(), e.path)
        })
        .collect();
    v.sort();
    v
}

fn status(r: &Repo) -> Status {
    r.status(&StatusOptions::default(), &Cancel::new()).unwrap()
}

#[test]
fn discovery() {
    let t = repo_or_skip!();
    t.write("src/deep/a.rs", "fn main() {}\n");
    let r = Repo::discover(&t.abs("src/deep/a.rs")).unwrap().unwrap();
    assert_eq!(r.workdir(), std::fs::canonicalize(t.path()).unwrap());
    assert_eq!(r.info().kind, RepoKind::Main);
    assert_eq!(r.rel_path(&t.abs("src/deep/a.rs")).as_deref(), Some("src/deep/a.rs"));
    assert_eq!(r.abs_path("src/deep/a.rs"), r.workdir().join("src").join("deep").join("a.rs"));
    // a file that does not exist yet still maps
    assert_eq!(r.rel_path(&t.abs("src/new.rs")).as_deref(), Some("src/new.rs"));
    // inside .git is not part of the worktree
    assert!(Repo::discover(&t.abs(".git/HEAD")).unwrap().is_none());
    assert!(r.rel_path(&t.abs(".git/config")).is_none());
    // outside any repository
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("x.txt"), "x").unwrap();
    assert!(Repo::discover(&outside.path().join("x.txt")).unwrap().is_none());
}

#[test]
fn head_states() {
    let t = repo_or_skip!();
    let r = t.repo();
    let h = r.head().unwrap();
    assert!(h.is_unborn());
    assert_eq!(h.branch.as_deref(), Some("main"));
    assert!(!h.detached);

    t.write("a.txt", "one\n");
    let c1 = t.commit_all("first commit\n\nbody");
    let h = r.head().unwrap();
    assert_eq!(h.oid.unwrap().to_string(), c1);
    assert_eq!(h.summary.as_deref(), Some("first commit"));
    assert_eq!(h.label(), "main");
    assert_eq!(r.state(), RepoState::Clean);

    t.write("a.txt", "two\n");
    t.commit_all("second");
    t.git(&["checkout", "-q", "--detach", &c1]);
    let h = r.head().unwrap();
    assert!(h.detached);
    assert_eq!(h.branch, None);
    assert_eq!(h.label(), c1[..7]);
}

#[test]
fn upstream_ahead_behind() {
    let t = repo_or_skip!();
    t.write("a.txt", "1\n");
    t.commit_all("base");
    let remote = tempfile::tempdir().unwrap();
    git_in(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
    t.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    t.git(&["push", "-q", "-u", "origin", "main"]);
    let r = t.repo();
    let up = r.head().unwrap().upstream.unwrap();
    assert_eq!((up.name.as_str(), up.ahead, up.behind, up.gone), ("origin/main", 0, 0, false));

    // two local commits, one remote commit made from a second clone
    t.write("a.txt", "2\n");
    t.commit_all("local 1");
    t.write("a.txt", "3\n");
    t.commit_all("local 2");
    let other = tempfile::tempdir().unwrap();
    git_in(other.path(), &["clone", "-q", remote.path().to_str().unwrap(), "c"]);
    let c = other.path().join("c");
    std::fs::write(c.join("b.txt"), "b\n").unwrap();
    git_in(&c, &["add", "b.txt"]);
    git_in(&c, &["-c", "user.name=O", "-c", "user.email=o@x", "commit", "-q", "-m", "remote"]);
    git_in(&c, &["push", "-q"]);
    t.git(&["fetch", "-q"]);
    let up = r.head().unwrap().upstream.unwrap();
    assert_eq!((up.ahead, up.behind), (2, 1));

    t.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    assert!(r.head().unwrap().upstream.unwrap().gone);
}

#[test]
fn blobs_and_entries() {
    let t = repo_or_skip!();
    t.write("a.txt", "committed\n");
    t.commit_all("c");
    t.write("a.txt", "staged\n");
    t.git(&["add", "a.txt"]);
    t.write("a.txt", "worktree\n");
    let r = t.repo();
    assert_eq!(r.head_blob("a.txt").unwrap().unwrap().text.bytes(), b"committed\n");
    assert_eq!(r.index_blob("a.txt").unwrap().unwrap().text.bytes(), b"staged\n");
    assert_eq!(r.worktree_file("a.txt").unwrap().unwrap().0.bytes(), b"worktree\n");
    assert_eq!(r.index_blob("a.txt").unwrap().unwrap().mode, MODE_FILE);
    assert!(r.index_blob("missing").unwrap().is_none());
    assert!(r.head_blob("missing").unwrap().is_none());
    let (id, _) = r.index_entry("a.txt").unwrap().unwrap();
    assert_eq!(id.to_string(), t.git(&["rev-parse", ":a.txt"]).trim());
    assert_eq!(r.resolve("HEAD").unwrap().to_string(), t.git(&["rev-parse", "HEAD"]).trim());
}

#[test]
fn status_matches_git() {
    let t = repo_or_skip!();
    t.write("keep.txt", "k\n");
    t.write("mod.txt", "m\n");
    t.write("del.txt", "d\n");
    t.write("staged_del.txt", "s\n");
    t.write("both.txt", "1\n");
    t.write(".gitignore", "*.log\nbuild/\n");
    t.commit_all("base");
    t.write("mod.txt", "m2\n");
    std::fs::remove_file(t.abs("del.txt")).unwrap();
    t.git(&["rm", "-q", "staged_del.txt"]);
    t.write("both.txt", "2\n");
    t.git(&["add", "both.txt"]);
    t.write("both.txt", "3\n");
    t.write("new_staged.txt", "n\n");
    t.git(&["add", "new_staged.txt"]);
    t.write("untracked.txt", "u\n");
    t.write("newdir/a.txt", "a\n");
    t.write("newdir/sub/b.txt", "b\n");
    t.write("x.log", "ignored\n");
    t.write("build/out.o", "ignored\n");
    let r = t.repo();
    let s = status(&r);
    let mut git: Vec<String> = t.short_status(&[]);
    git.sort();
    assert_eq!(ours(&s), git);
    assert_eq!(s.get("newdir/sub/b.txt").unwrap().path, "newdir/");
    assert_eq!(s.get("mod.txt").unwrap().mark(), Mark::Modified);
    assert!(s.get("keep.txt").is_none());
    assert!(!s.is_clean());

    let all = r.status(&StatusOptions { untracked: Untracked::All, ignored: false }, &Cancel::new()).unwrap();
    assert_eq!(ours(&all), t.short_status(&["--untracked-files=all"]));
    let ignored = r.status(&StatusOptions { untracked: Untracked::Normal, ignored: true }, &Cancel::new()).unwrap();
    assert_eq!(ours(&ignored), t.short_status(&["--ignored"]));
    assert!(ignored.get("build/out.o").unwrap().ignored);
}

#[test]
fn status_staged_rename() {
    let t = repo_or_skip!();
    t.write("old name.txt", "same content for a rename\nline 2\nline 3\n");
    t.commit_all("base");
    t.git(&["mv", "old name.txt", "new name.txt"]);
    let s = status(&t.repo());
    assert_eq!(s.entries.len(), 1);
    let e = &s.entries[0];
    assert_eq!(e.path, "new name.txt");
    assert_eq!(e.orig_path.as_deref(), Some("old name.txt"));
    assert_eq!(e.staged, Some(FileState::Renamed));
}

#[test]
fn status_conflicts() {
    let t = repo_or_skip!();
    t.write("c.txt", "base\n");
    t.write("gone.txt", "base\n");
    t.commit_all("base");
    t.git(&["checkout", "-q", "-b", "other"]);
    t.write("c.txt", "theirs\n");
    t.write("gone.txt", "changed by them\n");
    t.commit_all("theirs");
    t.git(&["checkout", "-q", "main"]);
    t.write("c.txt", "ours\n");
    t.git(&["rm", "-q", "gone.txt"]);
    t.commit_all("ours");
    let (ok, _) = common::try_git_in(t.path(), &["merge", "other"]);
    assert!(!ok);
    let r = t.repo();
    assert_eq!(r.state(), RepoState::Merging);
    let s = status(&r);
    assert_eq!(ours(&s), t.short_status(&[]));
    let c = s.get("c.txt").unwrap();
    assert_eq!(c.conflict, Some(ConflictKind::BothModified));
    assert_eq!(c.mark(), Mark::Conflict);
    assert_eq!(s.get("gone.txt").unwrap().conflict, Some(ConflictKind::DeletedByUs));
    let v = r.conflict_versions("c.txt").unwrap();
    assert_eq!(v[0].as_ref().unwrap().text.bytes(), b"base\n");
    assert_eq!(v[1].as_ref().unwrap().text.bytes(), b"ours\n");
    assert_eq!(v[2].as_ref().unwrap().text.bytes(), b"theirs\n");
    assert!(r.conflict_versions("gone.txt").unwrap()[1].is_none());
    assert_eq!(r.head().unwrap().state, RepoState::Merging);
}

#[test]
fn status_unicode_paths() {
    let t = repo_or_skip!();
    t.write("café/naïve file.txt", "x\n");
    t.commit_all("base");
    t.write("café/naïve file.txt", "y\n");
    t.write("日本.txt", "z\n");
    let s = status(&t.repo());
    let paths: Vec<_> = s.entries.iter().map(|e| (e.path.as_str(), e.short_code())).collect();
    assert_eq!(paths, [("café/naïve file.txt", " M".to_string()), ("日本.txt", "??".to_string())]);
}

#[test]
fn status_unborn_and_clean() {
    let t = repo_or_skip!();
    let r = t.repo();
    assert!(status(&r).is_clean());
    t.write("a.txt", "a\n");
    t.git(&["add", "a.txt"]);
    let s = status(&r);
    assert_eq!(ours(&s), ["A  a.txt"]);
    t.commit_all("c");
    assert!(status(&r).is_clean());
}

#[cfg(unix)]
#[test]
fn status_typechange_and_exec_bit() {
    use std::os::unix::fs::PermissionsExt;
    let t = repo_or_skip!();
    t.write("link", "target\n");
    t.write("run.sh", "echo\n");
    t.commit_all("base");
    std::fs::remove_file(t.abs("link")).unwrap();
    std::os::unix::fs::symlink("run.sh", t.abs("link")).unwrap();
    std::fs::set_permissions(t.abs("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let s = status(&t.repo());
    assert_eq!(ours(&s), t.short_status(&[]));
    assert_eq!(t.repo().worktree_file("link").unwrap().unwrap().1, MODE_LINK);
    assert_eq!(t.repo().worktree_file("run.sh").unwrap().unwrap().1, MODE_EXEC);
}

#[test]
fn linked_worktree() {
    let t = repo_or_skip!();
    t.write("a.txt", "a\n");
    t.commit_all("base");
    let wt = tempfile::tempdir().unwrap();
    let wt_path = wt.path().join("feature");
    t.git(&["worktree", "add", "-q", "-b", "feature", wt_path.to_str().unwrap()]);
    std::fs::write(wt_path.join("a.txt"), "changed\n").unwrap();
    let r = Repo::discover(&wt_path.join("a.txt")).unwrap().unwrap();
    assert_eq!(r.info().kind, RepoKind::Worktree);
    assert_eq!(r.head().unwrap().branch.as_deref(), Some("feature"));
    assert_ne!(r.info().git_dir, r.info().common_dir);
    assert_eq!(ours(&status(&r)), [" M a.txt"]);
    // the main checkout is untouched
    assert!(status(&t.repo()).is_clean());
    // writes go to the right worktree
    r.stage(&["a.txt".into()]).unwrap();
    assert_eq!(ours(&status(&r)), ["M  a.txt"]);
    assert!(status(&t.repo()).is_clean());
}

#[test]
fn submodules() {
    let t = repo_or_skip!();
    let sub = repo_or_skip!();
    sub.write("lib.txt", "lib\n");
    sub.commit_all("lib");
    t.write("a.txt", "a\n");
    t.commit_all("base");
    t.git(&["-c", "protocol.file.allow=always", "submodule", "add", "-q", sub.path().to_str().unwrap(), "vendor/lib"]);
    t.commit_all("add submodule");
    // a change inside the submodule shows as the submodule entry in the superproject
    std::fs::write(t.abs("vendor/lib/lib.txt"), "changed\n").unwrap();
    let s = status(&t.repo());
    let e = s.get("vendor/lib").expect("submodule entry");
    assert!(e.submodule);
    assert_eq!(e.unstaged, Some(FileState::Modified));
    // and a file inside it belongs to the submodule's own repository
    let r = Repo::discover(&t.abs("vendor/lib/lib.txt")).unwrap().unwrap();
    assert_eq!(r.info().kind, RepoKind::Submodule);
    assert_eq!(r.rel_path(&t.abs("vendor/lib/lib.txt")).as_deref(), Some("lib.txt"));
    assert_eq!(ours(&status(&r)), [" M lib.txt"]);
}

#[test]
fn stamps_see_changes() {
    let t = repo_or_skip!();
    t.write("a.txt", "a\n");
    t.commit_all("base");
    let r = t.repo();
    let s0 = r.stamp();
    assert!(!r.stamp().changes(&s0).any());
    std::thread::sleep(std::time::Duration::from_millis(20));
    t.write("a.txt", "b\n");
    t.git(&["add", "a.txt"]);
    let s1 = r.stamp();
    let c = s1.changes(&s0);
    assert!(c.index && !c.head && !c.refs);
    std::thread::sleep(std::time::Duration::from_millis(20));
    t.git(&["commit", "-q", "-m", "x"]);
    let c = r.stamp().changes(&s1);
    assert!(c.head);
    let s2 = r.stamp();
    std::thread::sleep(std::time::Duration::from_millis(20));
    t.git(&["branch", "other"]);
    let c = r.stamp().changes(&s2);
    assert!(c.refs && !c.head);
}
