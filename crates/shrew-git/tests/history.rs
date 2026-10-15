//! Log with graph rows, commit details, file history across renames, blame, and diffs between versions.

mod common;
use common::repo_or_skip;
use shrew_git::graph::EdgeKind;
use shrew_git::*;

fn ids(page: &LogPage) -> Vec<String> {
    page.rows.iter().map(|r| r.commit.summary.clone()).collect()
}

#[test]
fn log_matches_git_topo_order_and_pages() {
    let t = repo_or_skip!();
    t.write("a.txt", "0\n");
    t.commit_all("c0");
    t.git(&["checkout", "-q", "-b", "side"]);
    for i in 1..=3 {
        t.write("s.txt", format!("{i}\n"));
        t.commit_all(&format!("side {i}"));
    }
    t.git(&["checkout", "-q", "main"]);
    for i in 1..=3 {
        t.write("a.txt", format!("{i}\n"));
        t.commit_all(&format!("main {i}"));
    }
    t.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    t.write("a.txt", "after\n");
    t.commit_all("after merge");
    t.git(&["tag", "v1"]);

    // without a commit-graph the walk is by date: every commit once, children before parents here
    let r = t.repo();
    let by_date = r.log(&LogQuery { limit: 100, ..Default::default() }, &Cancel::new()).unwrap();
    assert_eq!(by_date.rows.len(), 9);
    assert_eq!(by_date.rows.last().unwrap().commit.summary, "c0");
    assert_eq!(by_date.rows.last().unwrap().graph.as_ref().unwrap().width, 1);

    // with one (git writes it on gc), topological like `git log --graph`
    t.git(&["commit-graph", "write", "--reachable"]);
    let r = t.repo();
    let all = r.log(&LogQuery { limit: 100, ..Default::default() }, &Cancel::new()).unwrap();
    let git: Vec<String> = t.git(&["log", "--topo-order", "--format=%s"]).lines().map(str::to_string).collect();
    assert_eq!(ids(&all), git);
    assert!(!all.more);

    // the merge row opens a second lane, the side commits run in it, and the fork closes it
    let merge = &all.rows[1];
    assert!(merge.graph.as_ref().unwrap().merge);
    assert_eq!(merge.graph.as_ref().unwrap().width, 2);
    let last = all.rows.last().unwrap().graph.as_ref().unwrap();
    assert_eq!(last.width, 1);
    for row in &all.rows {
        let g = row.graph.as_ref().unwrap();
        assert!(g.node < g.width);
        assert!(g.edges.iter().all(|e| e.from < g.width && e.to < g.width));
        if row.commit.parents.len() > 1 {
            assert_eq!(g.edges.iter().filter(|e| e.kind == EdgeKind::Out).count(), 2);
        }
    }

    // decorations
    let head_row = &all.rows[0];
    let labels: Vec<_> = head_row.refs.iter().map(|l| (l.name.as_str(), l.kind, l.is_head)).collect();
    assert_eq!(labels, [("main", RefKind::Branch, true), ("v1", RefKind::Tag, false)]);

    // pages line up with the full walk, graph included
    let p1 = r.log(&LogQuery { limit: 3, ..Default::default() }, &Cancel::new()).unwrap();
    let p2 = r.log(&LogQuery { skip: 3, limit: 3, ..Default::default() }, &Cancel::new()).unwrap();
    assert!(p1.more && p2.more);
    assert_eq!(p1.rows, all.rows[..3]);
    assert_eq!(p2.rows, all.rows[3..6]);

    let fp = r.log(&LogQuery { first_parent: true, limit: 100, ..Default::default() }, &Cancel::new()).unwrap();
    let git: Vec<String> = t.git(&["log", "--first-parent", "--format=%s"]).lines().map(str::to_string).collect();
    assert_eq!(ids(&fp), git);
    assert!(fp.rows.iter().all(|r| r.graph.as_ref().unwrap().width == 1));

    // from another rev, and every ref
    let side =
        r.log(&LogQuery { revs: vec!["side".into()], limit: 100, ..Default::default() }, &Cancel::new()).unwrap();
    assert_eq!(side.rows.len(), 4);
    t.git(&["branch", "orphan-ish", "HEAD~2"]);
    let all_refs = r.log(&LogQuery { all: true, limit: 100, ..Default::default() }, &Cancel::new()).unwrap();
    assert_eq!(all_refs.rows.len(), all.rows.len());
}

#[test]
fn log_of_unborn_repo_is_empty() {
    let t = repo_or_skip!();
    let page = t.repo().log(&LogQuery::default(), &Cancel::new()).unwrap();
    assert!(page.rows.is_empty() && !page.more);
}

#[test]
fn commit_details_and_tree_diffs() {
    let t = repo_or_skip!();
    t.write("keep.txt", "1\n2\n3\n");
    t.write("old.txt", "rename me\nwith enough lines\nto be similar\n");
    t.write("gone.txt", "bye\n");
    let c0 = t.commit_all("base");
    t.write("keep.txt", "1\nTWO\n3\n4\n");
    t.git(&["mv", "old.txt", "new.txt"]);
    t.git(&["rm", "-q", "gone.txt"]);
    t.write("bin.dat", b"\x00\x01binary");
    let c1 = t.commit_all("change things\n\nlonger message");
    let r = t.repo();
    let d = r.commit_details(Oid::from_hex(&c1).unwrap()).unwrap();
    assert_eq!(d.info.summary, "change things");
    assert_eq!(d.message.trim(), "change things\n\nlonger message");
    assert_eq!(d.info.parents, vec![Oid::from_hex(&c0).unwrap()]);
    let files: Vec<_> = d.files.iter().map(|f| (f.path.as_str(), f.state, f.added, f.removed, f.binary)).collect();
    assert_eq!(
        files,
        [
            ("bin.dat", FileState::Added, 0, 0, true),
            ("gone.txt", FileState::Deleted, 0, 1, false),
            ("keep.txt", FileState::Modified, 2, 1, false),
            ("new.txt", FileState::Renamed, 0, 0, false),
        ]
    );
    assert_eq!(d.files[3].old_path.as_deref(), Some("old.txt"));
    // the root commit diffs against nothing
    let root = r.commit_details(Oid::from_hex(&c0).unwrap()).unwrap();
    assert_eq!(root.files.len(), 3);
    assert!(root.files.iter().all(|f| f.state == FileState::Added));

    // versions: staged, unstaged (with an untracked file), commit vs worktree, and reversed
    t.write("keep.txt", "staged\n");
    t.git(&["add", "keep.txt"]);
    t.write("keep.txt", "worktree\n");
    t.write("untracked.txt", "u\n");
    let c = Cancel::new();
    let staged = r.tree_diff(Version::Head, Version::Index, &c).unwrap();
    assert_eq!(staged.iter().map(|f| (f.path.as_str(), f.added, f.removed)).collect::<Vec<_>>(), [("keep.txt", 1, 4)]);
    let unstaged = r.tree_diff(Version::Index, Version::Worktree, &c).unwrap();
    let names: Vec<_> = unstaged.iter().map(|f| (f.path.as_str(), f.state)).collect();
    assert_eq!(names, [("keep.txt", FileState::Modified), ("untracked.txt", FileState::Added)]);
    let since = r.tree_diff(Version::Commit(Oid::from_hex(&c0).unwrap()), Version::Worktree, &c).unwrap();
    let names: Vec<_> = since.iter().map(|f| (f.path.as_str(), f.state)).collect();
    assert_eq!(
        names,
        [
            ("bin.dat", FileState::Added),
            ("gone.txt", FileState::Deleted),
            ("keep.txt", FileState::Modified),
            ("new.txt", FileState::Added),
            ("old.txt", FileState::Deleted),
            ("untracked.txt", FileState::Added),
        ]
    );
    let back = r.tree_diff(Version::Index, Version::Head, &c).unwrap();
    assert_eq!((back[0].added, back[0].removed), (4, 1));

    let fd = r.file_diff("keep.txt", None, Version::Index, Version::Worktree).unwrap();
    assert_eq!(fd.old.as_ref().unwrap().bytes(), b"staged\n");
    assert_eq!(fd.new.as_ref().unwrap().bytes(), b"worktree\n");
    assert_eq!(fd.hunks.len(), 1);
    let fd =
        r.file_diff("new.txt", Some("old.txt"), Version::Commit(Oid::from_hex(&c0).unwrap()), Version::Head).unwrap();
    assert!(fd.hunks.is_empty() && fd.old.is_some());
    let fd = r.file_diff("bin.dat", None, Version::Head, Version::Worktree).unwrap();
    assert!(fd.binary && fd.hunks.is_empty());
}

#[test]
fn file_history_follows_renames() {
    let t = repo_or_skip!();
    let body = "line one\nline two\nline three\nline four\n";
    t.write("first.txt", body);
    t.write("other.txt", "x\n");
    t.commit_all("create");
    t.write("other.txt", "y\n");
    t.commit_all("unrelated");
    t.git(&["mv", "first.txt", "second.txt"]);
    t.commit_all("rename");
    t.write("second.txt", format!("{body}line five\n"));
    t.commit_all("edit");
    let r = t.repo();
    let hist = r.file_history("second.txt", 0, 10, &Cancel::new()).unwrap();
    let got: Vec<_> = hist.rows.iter().map(|h| (h.commit.summary.as_str(), h.path.as_str())).collect();
    assert_eq!(got, [("edit", "second.txt"), ("rename", "second.txt"), ("create", "first.txt")]);
    assert!(!hist.more);
    let page = r.file_history("second.txt", 1, 1, &Cancel::new()).unwrap();
    assert_eq!(page.rows[0].commit.summary, "rename");
    assert!(page.more);
}

#[test]
fn file_history_through_a_merge() {
    let t = repo_or_skip!();
    t.write("f.txt", "1\n");
    t.commit_all("base");
    t.git(&["checkout", "-q", "-b", "side"]);
    t.write("f.txt", "1\nside\n");
    t.commit_all("side edit");
    t.git(&["checkout", "-q", "main"]);
    t.write("g.txt", "other\n");
    t.commit_all("main other file");
    t.git(&["merge", "-q", "--no-edit", "side"]);
    let r = t.repo();
    let hist = r.file_history("f.txt", 0, 10, &Cancel::new()).unwrap();
    let got: Vec<_> = hist.rows.iter().map(|h| h.commit.summary.as_str()).collect();
    let git: Vec<String> = t.git(&["log", "--format=%s", "--", "f.txt"]).lines().map(str::to_string).collect();
    assert_eq!(got, git);
}

#[test]
fn blame_streams_and_handles_buffers() {
    let t = repo_or_skip!();
    t.write("f.txt", "a\nb\nc\n");
    let c1 = t.commit_all("one");
    t.write("f.txt", "a\nB\nc\nd\n");
    let c2 = t.commit_all("two");
    let r = t.repo();
    let mut hunks = Vec::new();
    r.blame("f.txt", None, None, &Cancel::new(), &mut |h| hunks.push(h)).unwrap();
    hunks.sort_by_key(|h| h.start);
    let lines: Vec<(u32, u32, String)> = hunks.iter().map(|h| (h.start, h.len, h.commit.id.to_string())).collect();
    assert_eq!(lines, [(0, 1, c1.clone()), (1, 1, c2.clone()), (2, 1, c1.clone()), (3, 1, c2.clone())]);
    assert_eq!(hunks[0].commit.summary, "one");
    assert_eq!(hunks[0].commit.author.name, "Test");
    assert!(hunks[0].commit.boundary);
    assert_eq!(hunks[1].commit.previous.as_ref().unwrap().0.to_string(), c1);

    // an unsaved buffer: the new line is not committed yet
    let buffer = Text::new("a\nB\nnew\nc\nd\n");
    let mut hunks = Vec::new();
    r.blame("f.txt", None, Some(&buffer), &Cancel::new(), &mut |h| hunks.push(h)).unwrap();
    let new = hunks.iter().find(|h| h.start == 2).unwrap();
    assert!(new.commit.is_uncommitted());
    assert_eq!(hunks.iter().map(|h| h.len).sum::<u32>(), 5);

    // at an older revision
    let mut hunks = Vec::new();
    r.blame("f.txt", Some(Oid::from_hex(&c1).unwrap()), None, &Cancel::new(), &mut |h| hunks.push(h)).unwrap();
    assert!(hunks.iter().all(|h| h.commit.id.to_string() == c1));

    // untracked: an error, not a panic
    t.write("u.txt", "u\n");
    assert!(r.blame("u.txt", None, None, &Cancel::new(), &mut |_| {}).is_err());
}
