//! Commits, branches, tags, stashes, conflicts and the network against a local bare remote.

mod common;
use common::{git_in, repo_or_skip};
use shrew_git::*;

#[test]
fn commit_amend_signoff_empty() {
    let t = repo_or_skip!();
    let r = t.repo();
    t.write("a.txt", "a\n");
    r.stage(&["a.txt".into()]).unwrap();
    let id = r.commit(&CommitOptions { message: "first\n\nbody line".into(), ..Default::default() }).unwrap();
    assert_eq!(id.to_string(), t.git(&["rev-parse", "HEAD"]).trim());
    assert_eq!(t.git(&["log", "-1", "--format=%B"]).trim(), "first\n\nbody line");

    // nothing staged: refused, like git
    let err = r.commit(&CommitOptions { message: "empty".into(), ..Default::default() }).unwrap_err();
    assert!(matches!(err.kind, ErrorKind::Git(_)), "{err:?}");
    assert!(err.message.contains("nothing"), "{}", err.message);
    r.commit(&CommitOptions { message: "empty".into(), allow_empty: true, ..Default::default() }).unwrap();

    t.write("a.txt", "changed\n");
    r.stage(&["a.txt".into()]).unwrap();
    let amended = r
        .commit(&CommitOptions { message: "reworded".into(), amend: true, signoff: true, ..Default::default() })
        .unwrap();
    let msg = t.git(&["log", "-1", "--format=%B"]);
    assert!(msg.starts_with("reworded") && msg.contains("Signed-off-by: Test <test@example.com>"), "{msg}");
    assert_eq!(t.git(&["rev-list", "--count", "HEAD"]).trim(), "2");
    assert_eq!(r.head().unwrap().oid, Some(amended));
}

#[cfg(unix)]
#[test]
fn failing_hook_output_is_the_error() {
    use std::os::unix::fs::PermissionsExt;
    let t = repo_or_skip!();
    t.write("a.txt", "a\n");
    t.git(&["add", "a.txt"]);
    let hook = t.abs(".git/hooks/pre-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\necho 'lint: trailing whitespace in a.txt'\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let r = t.repo();
    let err = r.commit(&CommitOptions { message: "x".into(), ..Default::default() }).unwrap_err();
    assert!(err.message.contains("lint: trailing whitespace"), "{}", err.message);
    // --no-verify skips it
    r.commit(&CommitOptions { message: "x".into(), no_verify: true, ..Default::default() }).unwrap();
}

#[test]
fn branches_and_tags() {
    let t = repo_or_skip!();
    t.write("a.txt", "a\n");
    let c1 = t.commit_all("one");
    let r = t.repo();
    r.create_branch("feature", None, true).unwrap();
    assert_eq!(r.head().unwrap().branch.as_deref(), Some("feature"));
    t.write("a.txt", "b\n");
    t.commit_all("two");
    r.switch("main").unwrap();
    assert_eq!(t.read("a.txt"), "a\n");
    r.create_branch("other", Some(&c1), false).unwrap();
    r.rename_branch("other", "renamed").unwrap();
    // unmerged branch: plain delete refused, forced delete works
    assert!(r.delete_branch("feature", false).is_err());
    r.delete_branch("feature", true).unwrap();
    r.create_tag("v1", None, None).unwrap();
    r.create_tag("v2", Some(&c1), Some("release two")).unwrap();
    let refs = r.refs().unwrap();
    let names: Vec<_> = refs.branches.iter().map(|b| (b.name.as_str(), b.is_head)).collect();
    assert_eq!(names, [("main", true), ("renamed", false)]);
    assert_eq!(refs.branches[0].summary, "one");
    let tags: Vec<_> = refs.tags.iter().map(|t| (t.name.as_str(), t.annotated, t.message.as_deref())).collect();
    assert_eq!(tags, [("v1", false, None), ("v2", true, Some("release two"))]);
    assert_eq!(refs.tags[1].target.to_string(), c1);
    r.delete_tag("v1").unwrap();
    r.switch_detach(&c1).unwrap();
    assert!(r.head().unwrap().detached);
    // names that look like options are refused before git sees them
    assert!(r.switch("--orphan").is_err());
}

#[test]
fn stashes() {
    let t = repo_or_skip!();
    t.write("a.txt", "a\n");
    t.commit_all("base");
    let r = t.repo();
    t.write("a.txt", "first\n");
    r.stash_push(&StashOptions { message: Some("one".into()), ..Default::default() }).unwrap();
    t.write("a.txt", "second\n");
    t.write("u.txt", "untracked\n");
    r.stash_push(&StashOptions { message: Some("two".into()), include_untracked: true, ..Default::default() }).unwrap();
    assert_eq!(t.read("a.txt"), "a\n");
    assert!(!t.abs("u.txt").exists());
    let list = r.stashes().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].index, 0);
    assert!(list[0].message.ends_with("two"), "{}", list[0].message);
    assert!(list[1].message.ends_with("one"));
    assert_eq!(list[0].id.to_string(), t.git(&["rev-parse", "stash@{0}"]).trim());
    r.stash_apply(1).unwrap();
    assert_eq!(t.read("a.txt"), "first\n");
    r.discard(&["a.txt".into()]).unwrap();
    r.stash_pop(0).unwrap();
    assert_eq!(t.read("a.txt"), "second\n");
    assert!(t.abs("u.txt").exists());
    r.stash_drop(0).unwrap();
    assert!(r.stashes().unwrap().is_empty());
    assert!(r.refs().unwrap().stashes.is_empty());
}

#[test]
fn conflicts_take_sides() {
    let t = repo_or_skip!();
    t.write("c.txt", "base\n");
    t.write("d.txt", "base\n");
    t.commit_all("base");
    t.git(&["checkout", "-q", "-b", "other"]);
    t.write("c.txt", "theirs\n");
    t.write("d.txt", "theirs\n");
    t.commit_all("theirs");
    t.git(&["checkout", "-q", "main"]);
    t.write("c.txt", "ours\n");
    t.write("d.txt", "ours\n");
    t.commit_all("ours");
    assert!(!common::try_git_in(t.path(), &["merge", "other"]).0);
    let r = t.repo();
    r.take_side("c.txt", ConflictSide::Theirs, true).unwrap();
    assert_eq!(t.read("c.txt"), "theirs\n");
    r.take_side("d.txt", ConflictSide::Ours, false).unwrap();
    assert_eq!(t.read("d.txt"), "ours\n");
    let s = r.status(&StatusOptions::default(), &Cancel::new()).unwrap();
    assert_eq!(s.conflicts().count(), 1);
    r.mark_resolved(&["d.txt".into()]).unwrap();
    let s = r.status(&StatusOptions::default(), &Cancel::new()).unwrap();
    assert_eq!(s.conflicts().count(), 0);
    r.continue_operation().unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(t.git(&["log", "-1", "--format=%p"]).split_whitespace().count(), 2);
}

#[test]
fn abort_merge() {
    let t = repo_or_skip!();
    t.write("c.txt", "base\n");
    t.commit_all("base");
    t.git(&["checkout", "-q", "-b", "other"]);
    t.write("c.txt", "theirs\n");
    t.commit_all("theirs");
    t.git(&["checkout", "-q", "main"]);
    t.write("c.txt", "ours\n");
    t.commit_all("ours");
    assert!(!common::try_git_in(t.path(), &["merge", "other"]).0);
    let r = t.repo();
    assert_eq!(r.state(), RepoState::Merging);
    r.abort_operation().unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(t.read("c.txt"), "ours\n");
}

/// A repository with a bare `origin` and a second clone to make remote commits from.
fn with_remote() -> Option<(common::TestRepo, tempfile::TempDir, std::path::PathBuf)> {
    let t = common::TestRepo::new()?;
    t.write("a.txt", "a\n");
    t.commit_all("base");
    let remote = tempfile::tempdir().unwrap();
    git_in(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
    t.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    let other =
        remote.path().join("..").join(format!("{}-clone", remote.path().file_name().unwrap().to_str().unwrap()));
    Some((t, remote, other))
}

#[test]
fn push_sets_upstream_then_fetch_and_pull() {
    let Some((t, remote, other)) = with_remote() else { return };
    let r = t.repo();
    assert!(r.head().unwrap().upstream.is_none());
    let mut lines = Vec::new();
    let out = r.push(&PushOptions::default(), &Cancel::new(), &mut |l| lines.push(l.to_string())).unwrap();
    assert!(out.contains("refs/heads/main"), "{out}");
    // a Repo snapshots the config when opened: open it again to see the new upstream
    let r = t.repo();
    let up = r.head().unwrap().upstream.expect("upstream set by push");
    assert_eq!(up.name, "origin/main");

    // a commit from elsewhere
    git_in(remote.path().parent().unwrap(), &["clone", "-q", remote.path().to_str().unwrap(), other.to_str().unwrap()]);
    std::fs::write(other.join("b.txt"), "b\n").unwrap();
    git_in(&other, &["add", "b.txt"]);
    git_in(&other, &["-c", "user.name=O", "-c", "user.email=o@x", "commit", "-q", "-m", "remote change"]);
    git_in(&other, &["push", "-q"]);

    let mut progress = Vec::new();
    r.fetch(&FetchOptions { prune: true, ..Default::default() }, &Cancel::new(), &mut |l| progress.push(l.to_string()))
        .unwrap();
    assert_eq!(r.head().unwrap().upstream.unwrap().behind, 1);
    r.pull(PullMode::FastForwardOnly, &Cancel::new(), &mut |_| {}).unwrap();
    assert_eq!(t.read("b.txt"), "b\n");
    assert_eq!(r.head().unwrap().upstream.unwrap().behind, 0);

    // diverged: ff-only refuses, rebase works
    std::fs::write(other.join("b.txt"), "b2\n").unwrap();
    git_in(&other, &["-c", "user.name=O", "-c", "user.email=o@x", "commit", "-q", "-am", "remote 2"]);
    git_in(&other, &["push", "-q"]);
    t.write("a.txt", "local\n");
    t.commit_all("local");
    let err = r.pull(PullMode::FastForwardOnly, &Cancel::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(err.kind, ErrorKind::Git(_)));
    r.pull(PullMode::Rebase, &Cancel::new(), &mut |_| {}).unwrap();
    let up = r.head().unwrap().upstream.unwrap();
    assert_eq!((up.ahead, up.behind), (1, 0));
    r.push(&PushOptions::default(), &Cancel::new(), &mut |_| {}).unwrap();
    assert_eq!(r.head().unwrap().upstream.unwrap().ahead, 0);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn cancelled_before_start() {
    let Some((t, _remote, _)) = with_remote() else { return };
    let c = Cancel::new();
    c.cancel();
    let err = t.repo().fetch(&FetchOptions::default(), &c, &mut |_| {}).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled);
}

#[cfg(unix)]
#[test]
fn cancel_kills_a_running_git() {
    // a remote whose upload-pack hangs (run through the shell by the local transport); cancel must kill it
    let Some((t, remote, _)) = with_remote() else { return };
    t.git(&["remote", "add", "slow", remote.path().to_str().unwrap()]);
    t.git(&["config", "remote.slow.uploadpack", "sleep 30; git-upload-pack"]);
    let r = t.repo();
    let c = Cancel::new();
    let c2 = c.clone();
    let start = std::time::Instant::now();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        c2.cancel();
    });
    let err =
        r.fetch(&FetchOptions { remote: Some("slow".into()), ..Default::default() }, &c, &mut |_| {}).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled);
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn remotes_listed() {
    let Some((t, remote, _)) = with_remote() else { return };
    let refs = t.repo().refs().unwrap();
    assert_eq!(refs.remotes.len(), 1);
    assert_eq!(refs.remotes[0].name, "origin");
    assert!(refs.remotes[0].url.as_deref().unwrap().contains(remote.path().file_name().unwrap().to_str().unwrap()));
    t.git(&["push", "-q", "-u", "origin", "main"]);
    let refs = t.repo().refs().unwrap();
    assert_eq!(refs.remote_branches.len(), 1);
    assert_eq!(refs.remote_branches[0].name, "origin/main");
    assert_eq!(refs.branches[0].upstream.as_deref(), Some("origin/main"));
}
