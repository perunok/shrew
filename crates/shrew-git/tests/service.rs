//! The background service: lazy start, coalescing, watching, ops, blame streaming and cancellation.

mod common;
use common::repo_or_skip;
use shrew_git::diff::HunkKind;
use shrew_git::ropey::Rope;
use shrew_git::*;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

fn service(poll: Option<Duration>) -> (GitService, Receiver<()>) {
    let (tx, rx) = channel();
    let tx = std::sync::Mutex::new(tx);
    let wake: Wake = Arc::new(move || {
        let _ = tx.lock().unwrap().send(());
    });
    (GitService::with_options(wake, ServiceOptions { poll_interval: poll, ..Default::default() }), rx)
}

/// Polls events until `pick` returns something, or panics after a while.
fn wait<T>(svc: &GitService, rx: &Receiver<()>, mut pick: impl FnMut(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        for ev in svc.poll() {
            if let Some(t) = pick(ev) {
                return t;
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "timed out waiting for an event");
        let _ = rx.recv_timeout(left.min(Duration::from_millis(200)));
    }
}

fn discover(svc: &GitService, rx: &Receiver<()>, path: &std::path::Path) -> Option<RepoInfo> {
    let t = svc.send(Request::Discover { path: path.to_path_buf() });
    wait(svc, rx, |e| match e.reply {
        Reply::Discovered { repo, .. } if e.ticket == Some(t) => Some(repo),
        _ => None,
    })
}

#[test]
fn lazy_and_outside_a_repository() {
    let (svc, rx) = service(None);
    assert!(!svc.is_started());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.txt"), "x").unwrap();
    assert!(discover(&svc, &rx, &dir.path().join("x.txt")).is_none());
    assert!(svc.is_started());
}

#[test]
fn gutter_hunks_and_coalescing() {
    let t = repo_or_skip!();
    t.write("f.txt", "a\nb\nc\nd\n");
    t.commit_all("base");
    let (svc, rx) = service(None);
    let info = discover(&svc, &rx, &t.abs("f.txt")).unwrap();
    let send = |version: u64, text: &str, base: GutterBase| {
        svc.send(Request::Gutter {
            repo: info.workdir.clone(),
            path: "f.txt".into(),
            buffer: 7,
            version,
            text: Rope::from_str(text),
            base,
        })
    };
    send(1, "a\nB\nc\nd\nnew\n", GutterBase::Index);
    let g = wait(&svc, &rx, |e| match e.reply {
        Reply::Gutter(Ok(g)) => Some(g),
        Reply::Gutter(Err(e)) => panic!("{e}"),
        _ => None,
    });
    assert_eq!(g.version, 1);
    assert_eq!(g.file, FileTrack::Tracked);
    let kinds: Vec<_> = g.hunks.iter().map(|h| h.kind()).collect();
    assert_eq!(kinds, [HunkKind::Modified, HunkKind::Added]);
    let base = g.base.unwrap();
    assert_eq!(base.text.bytes(), b"a\nb\nc\nd\n");
    assert_eq!(base.id, t.repo().index_entry("f.txt").unwrap().unwrap().0);

    // a burst of edits: only some are diffed, the newest always is
    for v in 2..=200 {
        send(v, &format!("a\nb\nc\nd\n{v}\n"), GutterBase::Index);
    }
    let mut seen = 0;
    let last = wait(&svc, &rx, |e| match e.reply {
        Reply::Gutter(Ok(g)) => {
            seen += 1;
            (g.version == 200).then_some(g)
        }
        _ => None,
    });
    assert_eq!(last.hunks.len(), 1);
    assert!(seen <= 199);

    // against HEAD, after staging: index base shows nothing, HEAD base still shows the change
    t.write("f.txt", "a\nB\nc\nd\n");
    t.git(&["add", "f.txt"]);
    send(300, "a\nB\nc\nd\n", GutterBase::Index);
    let g = wait(&svc, &rx, |e| match e.reply {
        Reply::Gutter(Ok(g)) if g.version == 300 => Some(g),
        _ => None,
    });
    assert!(g.hunks.is_empty());
    send(301, "a\nB\nc\nd\n", GutterBase::Head);
    let g = wait(&svc, &rx, |e| match e.reply {
        Reply::Gutter(Ok(g)) if g.version == 301 => Some(g),
        _ => None,
    });
    assert_eq!(g.hunks.len(), 1);

    // an untracked file has no hunks
    t.write("u.txt", "u\n");
    svc.send(Request::Gutter {
        repo: info.workdir.clone(),
        path: "u.txt".into(),
        buffer: 8,
        version: 1,
        text: Rope::from_str("u\n"),
        base: GutterBase::Index,
    });
    let g = wait(&svc, &rx, |e| match e.reply {
        Reply::Gutter(Ok(g)) if g.buffer == 8 => Some(g),
        _ => None,
    });
    assert_eq!(g.file, FileTrack::Untracked);
}

#[test]
fn coalesced_requests_answer_every_ticket() {
    let t = repo_or_skip!();
    t.write("f.txt", "a\n");
    t.commit_all("base");
    let (svc, rx) = service(None);
    let info = discover(&svc, &rx, &t.abs("f.txt")).unwrap();
    // a slow log keeps the read lane busy while the same status and refs are asked for twice
    let log = svc.send(Request::Log { repo: info.workdir.clone(), query: LogQuery::default() });
    let status =
        |svc: &GitService| svc.send(Request::Status { repo: info.workdir.clone(), options: StatusOptions::default() });
    let mut open: std::collections::HashSet<Ticket> = [
        log,
        status(&svc),
        status(&svc),
        svc.send(Request::Refs { repo: info.workdir.clone() }),
        svc.send(Request::Refs { repo: info.workdir.clone() }),
        svc.send(Request::Discover { path: t.abs("f.txt") }),
        svc.send(Request::Discover { path: t.abs("f.txt") }),
    ]
    .into();
    let cancelled = status(&svc);
    svc.cancel(cancelled);
    wait(&svc, &rx, |e| {
        assert_ne!(e.ticket, Some(cancelled), "a cancelled ticket is not answered");
        if let Some(t) = e.ticket {
            open.remove(&t);
        }
        open.is_empty().then_some(())
    });
}

#[test]
fn watching_follows_focus() {
    let t = repo_or_skip!();
    t.write("f.txt", "a\n");
    t.commit_all("base");
    let (svc, rx) = service(Some(Duration::from_millis(100)));
    let info = discover(&svc, &rx, t.path()).unwrap();
    svc.send(Request::Status { repo: info.workdir.clone(), options: StatusOptions::default() });
    let s = wait(&svc, &rx, |e| match e.reply {
        Reply::Status(Ok(s)) => Some(s),
        _ => None,
    });
    assert!(s.is_clean());

    // unfocused: changes go unnoticed, nothing runs
    svc.set_focused(false);
    std::thread::sleep(Duration::from_millis(50));
    t.write("f.txt", "b\n");
    t.git(&["add", "f.txt"]);
    std::thread::sleep(Duration::from_millis(400));
    assert!(svc.poll().is_empty());

    // focus comes back: a full refresh, and the index change is reported
    svc.set_focused(true);
    let mut changed = None;
    let s = wait(&svc, &rx, |e| match e.reply {
        Reply::Changed(c) => {
            changed = Some(c);
            None
        }
        Reply::Status(Ok(s)) => Some(s),
        _ => None,
    });
    assert_eq!(s.staged().count(), 1);
    assert!(changed.is_some_and(|c| c.index));

    // focused: a commit made elsewhere is picked up by polling
    std::thread::sleep(Duration::from_millis(30));
    t.git(&["commit", "-q", "-m", "elsewhere"]);
    let c = wait(&svc, &rx, |e| match e.reply {
        Reply::Changed(c) => Some(c),
        _ => None,
    });
    assert!(c.head);
    let s = wait(&svc, &rx, |e| match e.reply {
        Reply::Status(Ok(s)) => Some(s),
        _ => None,
    });
    assert!(s.is_clean());
    assert_eq!(s.head.summary.as_deref(), Some("elsewhere"));
}

/// The read lane polls on a schedule: requests arriving more often than the poll interval (or any other wakeup)
/// must not keep postponing the look at the stamps, or an outside commit would go unnoticed while shrew is busy.
#[test]
fn a_busy_read_lane_still_polls() {
    let t = repo_or_skip!();
    t.write("f.txt", "a\n");
    t.commit_all("base");
    let (svc, rx) = service(Some(Duration::from_millis(300)));
    let info = discover(&svc, &rx, t.path()).unwrap();
    svc.send(Request::Status { repo: info.workdir.clone(), options: StatusOptions::default() });
    wait(&svc, &rx, |e| matches!(e.reply, Reply::Status(Ok(_))).then_some(()));
    t.write("f.txt", "b\n");
    t.git(&["commit", "-qam", "elsewhere"]);
    // a request every 50 ms keeps waking the lane, well inside the 300 ms interval
    let start = Instant::now();
    let mut changed = false;
    while !changed && start.elapsed() < Duration::from_secs(10) {
        svc.send(Request::Refs { repo: info.workdir.clone() });
        std::thread::sleep(Duration::from_millis(50));
        changed = svc.poll().into_iter().any(|e| matches!(e.reply, Reply::Changed(c) if c.head));
    }
    let _ = &rx;
    assert!(changed, "the commit made elsewhere was noticed while requests kept coming");
}

#[test]
fn ops_refresh_status_and_report() {
    let t = repo_or_skip!();
    t.write("f.txt", "a\n");
    t.commit_all("base");
    let (svc, rx) = service(None);
    let info = discover(&svc, &rx, t.path()).unwrap();
    let repo = info.workdir.clone();
    svc.send(Request::Status { repo: repo.clone(), options: StatusOptions::default() });
    wait(&svc, &rx, |e| matches!(e.reply, Reply::Status(_)).then_some(()));

    t.write("f.txt", "b\n");
    svc.send(Request::Op { repo: repo.clone(), op: Op::Stage(vec!["f.txt".into()]) });
    let tk = svc.send(Request::Op {
        repo: repo.clone(),
        op: Op::Commit(CommitOptions { message: "via service".into(), ..Default::default() }),
    });
    let id = wait(&svc, &rx, |e| match e.reply {
        Reply::OpDone(Ok(OpOutput::Commit(id))) if e.ticket == Some(tk) => Some(id),
        Reply::OpDone(Err(err)) => panic!("{err}"),
        _ => None,
    });
    assert_eq!(id.to_string(), t.git(&["rev-parse", "HEAD"]).trim());
    let s = wait(&svc, &rx, |e| match e.reply {
        Reply::Status(Ok(s)) if s.head.oid == Some(id) => Some(s),
        _ => None,
    });
    assert!(s.is_clean());

    // a failing op carries git's message
    let tk = svc.send(Request::Op { repo: repo.clone(), op: Op::Switch("no-such-branch".into()) });
    let err = wait(&svc, &rx, |e| match e.reply {
        Reply::OpDone(Err(err)) if e.ticket == Some(tk) => Some(err),
        _ => None,
    });
    assert!(err.message.contains("no-such-branch"), "{}", err.message);

    // reads
    svc.send(Request::Refs { repo: repo.clone() });
    let refs = wait(&svc, &rx, |e| match e.reply {
        Reply::Refs(Ok(r)) => Some(r),
        _ => None,
    });
    assert_eq!(refs.branches.len(), 1);
    svc.send(Request::Log { repo: repo.clone(), query: LogQuery::default() });
    let log = wait(&svc, &rx, |e| match e.reply {
        Reply::Log(Ok(l)) => Some(l),
        _ => None,
    });
    assert_eq!(log.rows.len(), 2);
    svc.send(Request::CommitDetails { repo: repo.clone(), id });
    let d = wait(&svc, &rx, |e| match e.reply {
        Reply::CommitDetails(Ok(d)) => Some(d),
        _ => None,
    });
    assert_eq!(d.files.len(), 1);
    svc.send(Request::FileDiff {
        repo: repo.clone(),
        path: "f.txt".into(),
        old_path: None,
        old: Version::Commit(log.rows[1].commit.id),
        new: Version::Head,
    });
    let fd = wait(&svc, &rx, |e| match e.reply {
        Reply::FileDiff(Ok(d)) => Some(d),
        _ => None,
    });
    assert_eq!(fd.hunks.len(), 1);
}

#[test]
fn blame_streams_through_the_service() {
    let t = repo_or_skip!();
    let text: String = (0..2000).map(|i| format!("line {i}\n")).collect();
    t.write("big.txt", &text);
    t.commit_all("big");
    let (svc, rx) = service(None);
    let info = discover(&svc, &rx, t.path()).unwrap();
    let tk = svc.send(Request::Blame { repo: info.workdir.clone(), path: "big.txt".into(), rev: None, contents: None });
    let mut lines = 0;
    wait(&svc, &rx, |e| match e.reply {
        Reply::Blame(h) => {
            lines += h.iter().map(|h| h.len).sum::<u32>();
            None
        }
        Reply::BlameDone(r) if e.ticket == Some(tk) => {
            r.unwrap();
            Some(())
        }
        _ => None,
    });
    assert_eq!(lines, 2000);
}

#[cfg(unix)]
#[test]
fn cancel_a_running_fetch() {
    let t = repo_or_skip!();
    t.write("a.txt", "a\n");
    t.commit_all("base");
    let remote = tempfile::tempdir().unwrap();
    common::git_in(remote.path(), &["init", "-q", "--bare"]);
    t.git(&["remote", "add", "slow", remote.path().to_str().unwrap()]);
    t.git(&["config", "remote.slow.uploadpack", "sleep 30; git-upload-pack"]);
    let (svc, rx) = service(None);
    let info = discover(&svc, &rx, t.path()).unwrap();
    let tk = svc.send(Request::Op {
        repo: info.workdir.clone(),
        op: Op::Fetch(FetchOptions { remote: Some("slow".into()), ..Default::default() }),
    });
    std::thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    svc.cancel(tk);
    let err = wait(&svc, &rx, |e| match e.reply {
        Reply::OpDone(r) if e.ticket == Some(tk) => Some(r.unwrap_err()),
        _ => None,
    });
    assert_eq!(err.kind, ErrorKind::Cancelled);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn unknown_repository_is_an_error_not_a_hang() {
    let (svc, rx) = service(None);
    let dir = tempfile::tempdir().unwrap();
    svc.send(Request::Status { repo: dir.path().to_path_buf(), options: StatusOptions::default() });
    let e = wait(&svc, &rx, |e| match e.reply {
        Reply::Status(r) => Some(r.unwrap_err()),
        _ => None,
    });
    assert_eq!(e.kind, ErrorKind::NotARepo);
}
