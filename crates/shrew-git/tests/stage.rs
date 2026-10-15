//! Partial staging, unstaging and discarding against the real index, including the awkward cases.
#![allow(clippy::single_range_in_vec_init)]

mod common;
use common::repo_or_skip;
use shrew_git::diff::{EolMode, Hunk, diff};
use shrew_git::patch::LineSel;
use shrew_git::*;

/// Hunks of index -> worktree for `path`, as the UI would see them.
fn unstaged_hunks(r: &Repo, path: &str) -> Vec<Hunk> {
    let base = r.index_blob(path).unwrap().map(|b| b.text).unwrap_or_default();
    let wt = r.worktree_file(path).unwrap().unwrap().0;
    diff(&base, &wt, EolMode::Normalize)
}

fn staged_hunks(r: &Repo, path: &str) -> Vec<Hunk> {
    let base = r.head_blob(path).unwrap().map(|b| b.text).unwrap_or_default();
    diff(&base, &r.index_blob(path).unwrap().unwrap().text, EolMode::Exact)
}

#[test]
fn stage_one_hunk_of_two() {
    let t = repo_or_skip!();
    let base: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    t.write("f.txt", &base);
    t.commit_all("base");
    let changed = base.replace("line 2\n", "LINE TWO\n").replace("line 18\n", "LINE EIGHTEEN\n");
    t.write("f.txt", &changed);
    let r = t.repo();
    let h = unstaged_hunks(&r, "f.txt");
    assert_eq!(h.len(), 2);
    r.stage_lines("f.txt", None, &LineSel::hunks(&h[1..]), Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), base.replace("line 18\n", "LINE EIGHTEEN\n"));
    assert_eq!(t.read("f.txt"), changed);
    // the remaining unstaged hunk is the first one
    let h2 = unstaged_hunks(&r, "f.txt");
    assert_eq!(h2, vec![h[0].clone()]);
    // and unstaging it again restores the index
    let sh = staged_hunks(&r, "f.txt");
    r.unstage_lines("f.txt", &LineSel::hunks(&sh), Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), base);
}

#[test]
fn adjacent_hunks_stage_independently() {
    let t = repo_or_skip!();
    // changes separated by a single unchanged line: their patch contexts overlap
    t.write("f.txt", "a\nb\nc\nd\ne\n");
    t.commit_all("base");
    t.write("f.txt", "A\nb\nC\nd\nE\n");
    let r = t.repo();
    let h = unstaged_hunks(&r, "f.txt");
    assert_eq!(h.len(), 3);
    r.stage_lines("f.txt", None, &LineSel::hunks(&h[1..2]), Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), "a\nb\nC\nd\ne\n");
    // stage the last while the middle is already staged (context differs from HEAD now)
    let h = unstaged_hunks(&r, "f.txt");
    assert_eq!(h.len(), 2);
    r.stage_lines("f.txt", None, &LineSel::hunks(&h[1..]), Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), "a\nb\nC\nd\nE\n");
    // unstage only the middle one
    let sh = staged_hunks(&r, "f.txt");
    assert_eq!(sh.len(), 2);
    r.unstage_lines("f.txt", &LineSel::hunks(&sh[..1]), Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), "a\nb\nc\nd\nE\n");
}

#[test]
fn stage_single_lines_of_a_hunk() {
    let t = repo_or_skip!();
    t.write("f.txt", "x\n1\n2\n3\ny\n");
    t.commit_all("base");
    t.write("f.txt", "x\none\ntwo\nthree\nfour\ny\n");
    let r = t.repo();
    let h = unstaged_hunks(&r, "f.txt");
    // select "two" (new line 2): replaces "2"
    r.stage_lines("f.txt", None, &LineSel::new_lines(&h, 2..3), Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), "x\n1\n3\ntwo\ny\n");
    // editor-style: the changes on lines 4..5 of the buffer ("four")
    r.stage_new_lines("f.txt", None, 4..5).unwrap();
    assert!(t.staged("f.txt").contains("four\n"));
}

#[test]
fn stage_from_unsaved_buffer() {
    let t = repo_or_skip!();
    t.write("f.txt", "a\nb\n");
    t.commit_all("base");
    let r = t.repo();
    let buffer = Text::new("a\nb\nfrom buffer\n");
    let base = r.index_blob("f.txt").unwrap().unwrap().text;
    let h = diff(&base, &buffer, EolMode::Normalize);
    r.stage_lines("f.txt", Some(&buffer), &LineSel::hunks(&h), Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), "a\nb\nfrom buffer\n");
    assert_eq!(t.read("f.txt"), "a\nb\n");
}

#[test]
fn no_newline_at_end_of_file() {
    let t = repo_or_skip!();
    t.write("f.txt", "a\nb");
    t.commit_all("base");
    // append two lines: the old last line gains a newline
    t.write("f.txt", "a\nb\nc\nd");
    let r = t.repo();
    let h = unstaged_hunks(&r, "f.txt");
    assert_eq!(h, vec![Hunk { old: 1..2, new: 1..4 }]);
    // stage only "c": b must get its newline, d stays out
    r.stage_lines("f.txt", None, &LineSel { old: vec![], new: vec![2..3] }, Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), "a\nb\nc\n");
    // stage the rest
    let h = unstaged_hunks(&r, "f.txt");
    r.stage_lines("f.txt", None, &LineSel::hunks(&h), Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), "a\nb\nc\nd");
    // remove the trailing newline-less line entirely
    t.write("f.txt", "a\nb\nc\n");
    let h = unstaged_hunks(&r, "f.txt");
    r.stage_lines("f.txt", None, &LineSel::hunks(&h), Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), "a\nb\nc\n");
    // and unstage that change again
    t.commit_all("c");
    t.write("f.txt", "a\nb\nc");
    t.git(&["add", "f.txt"]);
    let sh = staged_hunks(&r, "f.txt");
    r.unstage_lines("f.txt", &LineSel::hunks(&sh), Base::Unchecked).unwrap();
    assert_eq!(t.staged("f.txt"), "a\nb\nc\n");
}

#[test]
fn crlf_with_autocrlf() {
    let t = repo_or_skip!();
    t.git(&["config", "core.autocrlf", "true"]);
    t.write("w.txt", "one\r\ntwo\r\nthree\r\n");
    t.commit_all("base"); // stored as LF
    let r = t.repo();
    assert_eq!(r.index_blob("w.txt").unwrap().unwrap().text.bytes(), b"one\ntwo\nthree\n");
    t.write("w.txt", "one\r\nTWO\r\nthree\r\nfour\r\n");
    let h = unstaged_hunks(&r, "w.txt");
    assert_eq!(h.len(), 2, "only real changes, not every line");
    r.stage_lines("w.txt", None, &LineSel::hunks(&h[..1]), Base::Unchecked).unwrap();
    assert_eq!(t.staged("w.txt"), "one\nTWO\nthree\n");
    // git agrees nothing but "four" is left
    let d = t.git(&["diff", "--", "w.txt"]);
    assert!(d.contains("+four") && !d.contains("+TWO") && !d.contains("-two"), "{d}");
    // discarding the last line keeps CRLF in the worktree
    let h = unstaged_hunks(&r, "w.txt");
    r.discard_lines("w.txt", &LineSel::hunks(&h), Base::Unchecked).unwrap();
    assert_eq!(t.read("w.txt"), "one\r\nTWO\r\nthree\r\n");
}

#[test]
fn crlf_stored_in_repository() {
    let t = repo_or_skip!();
    t.write("dos.txt", "a\r\nb\r\nc\r\n");
    t.commit_all("base");
    t.write("dos.txt", "a\r\nB\r\nc\r\nd\r\n");
    let r = t.repo();
    let h = unstaged_hunks(&r, "dos.txt");
    r.stage_lines("dos.txt", None, &LineSel::hunks(&h[1..]), Base::Unchecked).unwrap();
    assert_eq!(t.staged("dos.txt"), "a\r\nb\r\nc\r\nd\r\n");
}

#[test]
fn new_file_partially_and_fully() {
    let t = repo_or_skip!();
    t.write("a.txt", "a\n");
    t.commit_all("base");
    t.write("new.txt", "1\n2\n3\n");
    let r = t.repo();
    let h = unstaged_hunks(&r, "new.txt");
    assert_eq!(h, vec![Hunk { old: 0..0, new: 0..3 }]);
    r.stage_lines("new.txt", None, &LineSel { old: vec![], new: vec![0..1, 2..3] }, Base::Unchecked).unwrap();
    assert_eq!(t.staged("new.txt"), "1\n3\n");
    // unstage every staged line of a file HEAD does not have: it leaves the index
    let sh = staged_hunks(&r, "new.txt");
    r.unstage_lines("new.txt", &LineSel::hunks(&sh), Base::Unchecked).unwrap();
    assert!(r.index_blob("new.txt").unwrap().is_none());
    assert_eq!(t.read("new.txt"), "1\n2\n3\n");
}

#[test]
fn discard_lines_in_worktree() {
    let t = repo_or_skip!();
    t.write("f.txt", "a\nb\nc\nd\ne\nf\ng\n");
    t.commit_all("base");
    t.write("f.txt", "A\nb\nc\nd\ne\nf\nG\n");
    let r = t.repo();
    let h = unstaged_hunks(&r, "f.txt");
    let text = r.discard_lines("f.txt", &LineSel::hunks(&h[..1]), Base::Unchecked).unwrap();
    assert_eq!(t.read("f.txt"), "a\nb\nc\nd\ne\nf\nG\n");
    assert_eq!(text.bytes(), b"a\nb\nc\nd\ne\nf\nG\n");
}

#[test]
fn stale_index_is_reported() {
    let t = repo_or_skip!();
    t.write("f.txt", "a\nb\n");
    t.commit_all("base");
    t.write("f.txt", "a\nB\n");
    let r = t.repo();
    let base = r.index_blob("f.txt").unwrap().unwrap();
    let h = diff(&base.text, &Text::new("a\nB\n"), EolMode::Normalize);
    // someone stages something else meanwhile; our selection was computed for the old index
    t.write("f.txt", "X\nb\n");
    t.git(&["add", "f.txt"]);
    t.write("f.txt", "a\nB\n");
    let err = r.stage_lines("f.txt", None, &LineSel::hunks(&h), Base::Blob(base.id)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Stale);
    assert_eq!(t.staged("f.txt"), "X\nb\n");
    let err = r.unstage_lines("f.txt", &LineSel::hunks(&h), Base::Absent).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Stale);
    // with the current base it works
    let now = r.index_blob("f.txt").unwrap().unwrap();
    let h = diff(&now.text, &Text::new("a\nB\n"), EolMode::Normalize);
    r.stage_lines("f.txt", None, &LineSel::hunks(&h), Base::Blob(now.id)).unwrap();
    assert_eq!(t.staged("f.txt"), "a\nB\n");
}

#[test]
fn whole_file_stage_unstage_discard_clean() {
    let t = repo_or_skip!();
    t.write("a.txt", "a\n");
    t.write("b.txt", "b\n");
    t.commit_all("base");
    t.write("a.txt", "A\n");
    std::fs::remove_file(t.abs("b.txt")).unwrap();
    t.write("u.txt", "u\n");
    let r = t.repo();
    r.stage(&["a.txt".into(), "b.txt".into(), "u.txt".into()]).unwrap();
    assert_eq!(t.short_status(&[]), ["A  u.txt", "D  b.txt", "M  a.txt"]);
    r.unstage(&["a.txt".into()]).unwrap();
    assert_eq!(t.short_status(&[]), [" M a.txt", "A  u.txt", "D  b.txt"]);
    r.unstage_all().unwrap();
    r.discard(&["a.txt".into(), "b.txt".into()]).unwrap();
    assert_eq!(t.short_status(&[]), ["?? u.txt"]);
    r.clean(&["u.txt".into()]).unwrap();
    assert!(t.short_status(&[]).is_empty());
    r.stage_all().unwrap();
}

#[test]
fn unstage_before_first_commit() {
    let t = repo_or_skip!();
    t.write("a.txt", "a\n");
    t.write("b.txt", "b\n");
    t.git(&["add", "."]);
    let r = t.repo();
    r.unstage(&["a.txt".into()]).unwrap();
    assert_eq!(t.short_status(&[]), ["?? a.txt", "A  b.txt"]);
    r.unstage_all().unwrap();
    assert_eq!(t.short_status(&[]), ["?? a.txt", "?? b.txt"]);
}

#[test]
fn unicode_and_quoted_paths() {
    let t = repo_or_skip!();
    let name = "dir é/we\"ird name.txt";
    if cfg!(windows) {
        return; // quotes are not valid in Windows file names
    }
    t.write(name, "a\nb\n");
    t.commit_all("base");
    t.write(name, "a\nB\nc\n");
    let r = t.repo();
    let h = unstaged_hunks(&r, name);
    r.stage_lines(name, None, &LineSel::hunks(&h), Base::Unchecked).unwrap();
    assert_eq!(r.index_blob(name).unwrap().unwrap().text.bytes(), b"a\nB\nc\n");
    let sh = staged_hunks(&r, name);
    r.unstage_lines(name, &LineSel::hunks(&sh), Base::Unchecked).unwrap();
    assert_eq!(r.index_blob(name).unwrap().unwrap().text.bytes(), b"a\nb\n");
}
