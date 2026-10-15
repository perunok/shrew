//! Timings for the performance targets. Ignored by default; run them optimised:
//!
//! ```text
//! cargo test -p shrew-git --release --test perf -- --ignored --nocapture
//! ```

mod common;
use shrew_git::diff::{EolMode, diff};
use shrew_git::ropey::Rope;
use shrew_git::*;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

fn service() -> (GitService, Receiver<()>) {
    let (tx, rx) = channel();
    let tx = std::sync::Mutex::new(tx);
    let wake: Wake = Arc::new(move || {
        let _ = tx.lock().unwrap().send(());
    });
    (GitService::with_options(wake, ServiceOptions { poll_interval: None, ..Default::default() }), rx)
}

/// Polls until `pick` returns something (panics after 30 s).
fn wait<T>(svc: &GitService, rx: &Receiver<()>, mut pick: impl FnMut(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for ev in svc.poll() {
            if let Some(t) = pick(ev) {
                return t;
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "timed out");
        let _ = rx.recv_timeout(left.min(Duration::from_millis(50)));
    }
}

fn percentile(sorted: &[Duration], p: usize) -> Duration {
    sorted[(sorted.len() * p / 100).min(sorted.len() - 1)]
}

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

#[test]
#[ignore]
fn status_of_a_20k_file_repo() {
    let Some(t) = common::TestRepo::new() else { return };
    for d in 0..200 {
        for f in 0..100 {
            t.write(&format!("dir{d:03}/file{f:03}.txt"), format!("content {d} {f}\n"));
        }
    }
    t.commit_all("20k files");
    for d in (0..200).step_by(10) {
        t.write(&format!("dir{d:03}/file000.txt"), "changed\n");
        t.write(&format!("dir{d:03}/new.txt"), "new\n");
    }
    let r = t.repo();
    let opts = StatusOptions::default();
    let (cold, _) = best_of(1, || r.status(&opts, &Cancel::new()).unwrap());
    let (ours, s) = best_of(5, || r.status(&opts, &Cancel::new()).unwrap());
    assert_eq!(s.entries.len(), 40);
    let (git, _) = best_of(5, || t.git(&["status", "--porcelain=v2"]));
    println!("status, 20k files, 40 changes: {ours:?} in-process, first call {cold:?} (git status: {git:?})");

    // through the service, as the app sees it: ask, and refresh after a save
    let (svc, rx) = service();
    let t0 = svc.send(Request::Discover { path: t.path().to_path_buf() });
    let info = wait(&svc, &rx, |e| match e.reply {
        Reply::Discovered { repo, .. } if e.ticket == Some(t0) => repo,
        _ => None,
    });
    let start = Instant::now();
    svc.send(Request::Status { repo: info.workdir.clone(), options: opts });
    wait(&svc, &rx, |e| matches!(e.reply, Reply::Status(Ok(_))).then_some(()));
    println!("status through the service, 20k files, first request to reply: {:?}", start.elapsed());
    let mut times = Vec::new();
    for i in 0..10 {
        t.write("dir001/file001.txt", format!("saved {i}\n"));
        let start = Instant::now();
        svc.refresh(&info.workdir);
        wait(&svc, &rx, |e| matches!(e.reply, Reply::Status(Ok(_))).then_some(()));
        times.push(start.elapsed());
    }
    times.sort();
    println!(
        "refresh after a save, 20k files: median {:?}, worst {:?}",
        percentile(&times, 50),
        times[times.len() - 1]
    );
}

#[test]
#[ignore]
fn gutter_latency_while_typing() {
    let Some(t) = common::TestRepo::new() else { return };
    let base: String = (0..10_000).map(|i| format!("    let value_{i} = compute({i}, \"some text\");\n")).collect();
    t.write("big.rs", &base);
    t.commit_all("base");
    let (svc, rx) = service();
    let t0 = svc.send(Request::Discover { path: t.abs("big.rs") });
    let info = wait(&svc, &rx, |e| match e.reply {
        Reply::Discovered { repo, .. } if e.ticket == Some(t0) => repo,
        _ => None,
    });
    let mut rope = Rope::from_str(&base);
    let mut version = 0;
    let send = |rope: &Rope, version: u64| {
        svc.send(Request::Gutter {
            repo: info.workdir.clone(),
            path: "big.rs".into(),
            buffer: 1,
            version,
            text: rope.clone(),
            base: GutterBase::Index,
        })
    };
    // the first request reads the base from the index
    let start = Instant::now();
    send(&rope, 0);
    wait(&svc, &rx, |e| matches!(e.reply, Reply::Gutter(Ok(_))).then_some(()));
    println!("gutter, 10k-line file, first request (reads the base): {:?}", start.elapsed());
    // one keystroke at a time, each waiting for its marks
    let mut times = Vec::new();
    let mut at = rope.line_to_char(5_000) + 8;
    for _ in 0..300 {
        rope.insert(at, "x");
        at += 1;
        version += 1;
        let start = Instant::now();
        send(&rope, version);
        wait(&svc, &rx, |e| match e.reply {
            Reply::Gutter(Ok(g)) if g.version == version => Some(()),
            _ => None,
        });
        times.push(start.elapsed());
    }
    times.sort();
    println!(
        "gutter latency per keystroke, 10k lines (send -> marks): median {:?}, p95 {:?}, worst {:?}",
        percentile(&times, 50),
        percentile(&times, 95),
        times[times.len() - 1]
    );
    assert!(percentile(&times, 95) < Duration::from_millis(20));
    // fast typing: 100 keystrokes sent back to back coalesce; only the last answer matters
    let start = Instant::now();
    for _ in 0..100 {
        rope.insert(at, "y");
        at += 1;
        version += 1;
        send(&rope, version);
    }
    wait(&svc, &rx, |e| match e.reply {
        Reply::Gutter(Ok(g)) if g.version == version => Some(()),
        _ => None,
    });
    println!("gutter, 100 keystrokes sent at once: last marks after {:?}", start.elapsed());
}
