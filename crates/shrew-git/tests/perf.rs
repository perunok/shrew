//! Timings for the performance targets. Ignored by default; run them optimised:
//!
//! ```text
//! cargo test -p shrew-git --release --test perf -- --ignored --nocapture
//! ```

mod common;
use shrew_git::diff::{EolMode, diff};
use shrew_git::ropey::Rope;
use shrew_git::*;
use std::time::{Duration, Instant};

fn best_of<T>(n: usize, mut f: impl FnMut() -> T) -> (Duration, T) {
    let mut best = Duration::MAX;
    let mut out = None;
    for _ in 0..n {
        let t = Instant::now();
        let r = f();
        best = best.min(t.elapsed());
        out = Some(r);
    }
    (best, out.unwrap())
}

#[test]
#[ignore]
fn gutter_diff_10k_lines() {
    let base: String = (0..10_000).map(|i| format!("    let value_{i} = compute({i}, \"some text\");\n")).collect();
    let base_text = Text::new(base.clone());
    let mut rope = Rope::from_str(&base);
    // an edit in the middle, as typing produces
    let at = rope.line_to_char(5_000) + 8;
    rope.insert(at, "edited_");
    let (t, hunks) = best_of(20, || diff(&base_text, &Text::from_rope(&rope), EolMode::Normalize));
    assert_eq!(hunks.len(), 1);
    println!("gutter diff, 10k lines, one edit: {t:?} (snapshot + split + diff)");
    // scattered edits
    for i in (0..10_000).step_by(100) {
        let at = rope.line_to_char(i);
        rope.insert(at, "x");
    }
    let (t, hunks) = best_of(20, || diff(&base_text, &Text::from_rope(&rope), EolMode::Normalize));
    println!("gutter diff, 10k lines, {} hunks: {t:?}", hunks.len());
    assert!(t < Duration::from_millis(20));
}

#[test]
#[ignore]
fn status_of_a_10k_file_repo() {
    let Some(t) = common::TestRepo::new() else { return };
    for d in 0..100 {
        for f in 0..100 {
            t.write(&format!("dir{d:03}/file{f:03}.txt"), format!("content {d} {f}\n"));
        }
    }
    t.commit_all("10k files");
    for d in (0..100).step_by(10) {
        t.write(&format!("dir{d:03}/file000.txt"), "changed\n");
        t.write(&format!("dir{d:03}/new.txt"), "new\n");
    }
    let r = t.repo();
    let opts = StatusOptions::default();
    let (ours, s) = best_of(5, || r.status(&opts, &Cancel::new()).unwrap());
    assert_eq!(s.entries.len(), 20);
    let (git, _) = best_of(5, || t.git(&["status", "--porcelain=v2"]));
    println!("status, 10k files, 20 changes: {ours:?} in-process (git status: {git:?})");
}

#[test]
#[ignore]
fn status_and_log_of_this_repository() {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(r) = Repo::discover(here).unwrap() else { return };
    let (t, s) = best_of(5, || r.status(&StatusOptions::default(), &Cancel::new()).unwrap());
    println!("status of shrew's own repository: {t:?} ({} entries)", s.entries.len());
    let (t, page) = best_of(5, || r.log(&LogQuery { limit: 200, ..Default::default() }, &Cancel::new()).unwrap());
    println!("log first page of shrew's repository: {t:?} ({} rows)", page.rows.len());
}

#[test]
#[ignore]
fn log_first_page_of_20k_commits() {
    let Some(t) = common::TestRepo::new() else { return };
    // fast-import: 20k commits on a main line with a side branch merged every 50 commits
    let mut stream = String::new();
    let mut mark = 0;
    let mut prev_main = 0;
    for i in 0..20_000u32 {
        mark += 1;
        let parent = if prev_main > 0 { format!("from :{prev_main}\n") } else { String::new() };
        let content = format!("{i}\n");
        stream.push_str(&format!(
            "commit refs/heads/main\nmark :{mark}\ncommitter T <t@x> {} +0000\ndata {}\nc{i}\n{parent}M 644 inline f{}.txt\ndata {}\n{content}\n",
            1_600_000_000 + i * 60,
            format!("c{i}").len(),
            i % 100,
            content.len()
        ));
        prev_main = mark;
    }
    let mut child = std::process::Command::new("git")
        .current_dir(t.path())
        .args(["fast-import", "--quiet"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(stream.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
    t.git(&["reset", "-q", "--hard", "main"]);
    let r = t.repo();
    let q = LogQuery { limit: 200, ..Default::default() };
    let (cold, _) = best_of(1, || r.log(&q, &Cancel::new()).unwrap());
    let (warm, page) = best_of(5, || r.log(&q, &Cancel::new()).unwrap());
    assert_eq!(page.rows.len(), 200);
    println!("log first page, 20k commits, no commit-graph: {warm:?} (first call {cold:?})");
    t.git(&["commit-graph", "write", "--reachable"]);
    let r = t.repo();
    let (warm, _) = best_of(5, || r.log(&q, &Cancel::new()).unwrap());
    println!("log first page, 20k commits, with commit-graph: {warm:?}");
    let deep = LogQuery { skip: 10_000, limit: 200, ..Default::default() };
    let (t2, page) = best_of(3, || r.log(&deep, &Cancel::new()).unwrap());
    assert_eq!(page.rows.len(), 200);
    println!("log page at row 10,000: {t2:?}");
}

#[test]
#[ignore]
fn blame_first_chunk_of_a_long_history() {
    let Some(t) = common::TestRepo::new() else { return };
    let mut lines: Vec<String> = (0..5000).map(|i| format!("line {i}\n")).collect();
    t.write("big.txt", lines.concat());
    t.commit_all("base");
    for c in 0..200 {
        for k in 0..10 {
            let i = (c * 37 + k * 499) % 5000;
            lines[i] = format!("line {i} edited in {c}\n");
        }
        t.write("big.txt", lines.concat());
        t.commit_all(&format!("edit {c}"));
    }
    let r = t.repo();
    let start = Instant::now();
    let mut first = None;
    let mut total = 0;
    r.blame("big.txt", None, None, &Cancel::new(), &mut |h| {
        first.get_or_insert_with(|| start.elapsed());
        total += h.len;
    })
    .unwrap();
    println!("blame, 5000 lines, 200 commits: first chunk after {:?}, all after {:?}", first.unwrap(), start.elapsed());
    assert_eq!(total, 5000);
}
