//! [`GitService`]: every git read and write off the UI thread.
//!
//! The UI sends [`Request`]s (each gets a [`Ticket`]) and, when the service calls its wake callback, drains
//! [`Event`]s with [`GitService::poll`]. Nothing runs until the first request: threads start lazily, one per
//! lane, and wait on a condition variable (no CPU) when idle. Lanes keep slow work from delaying fast work:
//!
//! | lane  | requests                                                       |
//! |-------|----------------------------------------------------------------|
//! | fast  | `Discover`, `Gutter`, `ConflictVersions`, `FileDiff`           |
//! | read  | `Status`, `Refs`, `Log`, `FileHistory`, `CommitDetails`, `TreeDiff`, refreshes |
//! | blame | `Blame` (a new blame of the same file cancels the running one) |
//! | write | `Op` (one at a time, in order)                                 |
//!
//! Pending requests coalesce: a `Gutter` for a buffer replaces the pending one for that buffer (only the
//! newest version is diffed and answered). `Status` and `Refs` per repository, `Discover` per path and `Blame`
//! per file coalesce too, but the one that runs answers every ticket it replaced, so separate parts of a UI
//! asking for the same thing each get their reply.
//!
//! **Change detection.** A repository becomes *watched* when its status is requested. While the window is
//! focused ([`GitService::set_focused`]) the read lane looks at a few file stamps of each watched repository
//! every [`ServiceOptions::poll_interval`] (no file is read), and on a change sends [`Reply::Changed`] and a fresh
//! status. Gaining focus, finishing an [`Op`] and [`GitService::refresh`] (call it after saving a file) re-run
//! the status. Unfocused, nothing runs at all.

use crate::blame::BlameHunk;
use crate::changes::{FileDiff, Version};
use crate::cli::Cancel;
use crate::diff::{EolMode, Hunk, diff};
use crate::error::{Error, ErrorKind, Result};
use crate::log::{CommitDetails, FileChange, HistoryPage, LogPage, LogQuery, Refs};
use crate::oid::Oid;
use crate::ops::{Base, CommitOptions, ConflictSide, FetchOptions, PullMode, PushOptions, StashOptions};
use crate::patch::LineSel;
use crate::repo::{Blob, Changes, MODE_EXEC, MODE_FILE, Repo, RepoInfo, Stamp};
use crate::status::{Status, StatusOptions, Untracked};
use crate::text::Text;
use ropey::Rope;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Called from a worker thread when events are waiting (post a user event to the event loop, then
/// [`GitService::poll`]). Coalesced: called once until the next poll.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Names a request; its events carry it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ticket(pub u64);

/// What the gutter compares a buffer with.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum GutterBase {
    /// The staged version: marks disappear as changes are staged (like VS Code and Sublime Merge).
    #[default]
    Index,
    /// The committed version.
    Head,
}

/// A write, run on the write lane through the `git` program (see the [`Repo`] method of the same name).
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Stage(Vec<String>),
    StageAll,
    Unstage(Vec<String>),
    UnstageAll,
    Discard(Vec<String>),
    /// Delete untracked files (ask the user first).
    Clean(Vec<String>),
    /// `text`: an unsaved buffer to stage from (the file on disk when `None`).
    StageLines {
        path: String,
        text: Option<Text>,
        sel: LineSel,
        base: Base,
    },
    UnstageLines {
        path: String,
        sel: LineSel,
        base: Base,
    },
    DiscardLines {
        path: String,
        sel: LineSel,
        base: Base,
    },
    Commit(CommitOptions),
    Switch(String),
    SwitchDetach(String),
    CreateBranch {
        name: String,
        start: Option<String>,
        switch: bool,
    },
    DeleteBranch {
        name: String,
        force: bool,
    },
    RenameBranch {
        old: String,
        new: String,
    },
    CreateTag {
        name: String,
        rev: Option<String>,
        message: Option<String>,
    },
    DeleteTag(String),
    StashPush(StashOptions),
    StashPop(usize),
    StashApply(usize),
    StashDrop(usize),
    TakeSide {
        path: String,
        side: ConflictSide,
        mark_resolved: bool,
    },
    MarkResolved(Vec<String>),
    AbortOperation,
    ContinueOperation,
    Fetch(FetchOptions),
    Pull(PullMode),
    Push(PushOptions),
}

impl Op {
    /// Whether it talks to a remote (slow; worth a progress display and a cancel button).
    pub fn is_network(&self) -> bool {
        matches!(self, Op::Fetch(_) | Op::Pull(_) | Op::Push(_))
    }
}

/// What a finished [`Op`] produced.
#[derive(Clone, Debug, PartialEq)]
pub enum OpOutput {
    Done,
    /// The new commit of [`Op::Commit`].
    Commit(Oid),
    /// git's summary of a fetch, pull or push.
    Text(String),
    /// The file's new content after [`Op::DiscardLines`] (reload an open buffer from it).
    File(Text),
}

impl Repo {
    /// Runs an [`Op`] here and now (the service does this on its write lane).
    pub fn run_op(&self, op: &Op, cancel: &Cancel, progress: &mut dyn FnMut(&str)) -> Result<OpOutput> {
        use OpOutput::Done;
        Ok(match op {
            Op::Stage(p) => self.stage(p).map(|_| Done)?,
            Op::StageAll => self.stage_all().map(|_| Done)?,
            Op::Unstage(p) => self.unstage(p).map(|_| Done)?,
            Op::UnstageAll => self.unstage_all().map(|_| Done)?,
            Op::Discard(p) => self.discard(p).map(|_| Done)?,
            Op::Clean(p) => self.clean(p).map(|_| Done)?,
            Op::StageLines { path, text, sel, base } => {
                self.stage_lines(path, text.as_ref(), sel, *base).map(|_| Done)?
            }
            Op::UnstageLines { path, sel, base } => self.unstage_lines(path, sel, *base).map(|_| Done)?,
            Op::DiscardLines { path, sel, base } => OpOutput::File(self.discard_lines(path, sel, *base)?),
            Op::Commit(o) => OpOutput::Commit(self.commit(o)?),
            Op::Switch(b) => self.switch(b).map(|_| Done)?,
            Op::SwitchDetach(r) => self.switch_detach(r).map(|_| Done)?,
            Op::CreateBranch { name, start, switch } => {
                self.create_branch(name, start.as_deref(), *switch).map(|_| Done)?
            }
            Op::DeleteBranch { name, force } => self.delete_branch(name, *force).map(|_| Done)?,
            Op::RenameBranch { old, new } => self.rename_branch(old, new).map(|_| Done)?,
            Op::CreateTag { name, rev, message } => {
                self.create_tag(name, rev.as_deref(), message.as_deref()).map(|_| Done)?
            }
            Op::DeleteTag(n) => self.delete_tag(n).map(|_| Done)?,
            Op::StashPush(o) => self.stash_push(o).map(|_| Done)?,
            Op::StashPop(i) => self.stash_pop(*i).map(|_| Done)?,
            Op::StashApply(i) => self.stash_apply(*i).map(|_| Done)?,
            Op::StashDrop(i) => self.stash_drop(*i).map(|_| Done)?,
            Op::TakeSide { path, side, mark_resolved } => self.take_side(path, *side, *mark_resolved).map(|_| Done)?,
            Op::MarkResolved(p) => self.mark_resolved(p).map(|_| Done)?,
            Op::AbortOperation => self.abort_operation().map(|_| Done)?,
            Op::ContinueOperation => self.continue_operation().map(|_| Done)?,
            Op::Fetch(o) => OpOutput::Text(self.fetch(o, cancel, progress)?),
            Op::Pull(m) => OpOutput::Text(self.pull(*m, cancel, progress)?),
            Op::Push(o) => OpOutput::Text(self.push(o, cancel, progress)?),
        })
    }
}

/// Something to do. `repo` is always the [`RepoInfo::workdir`] from [`Reply::Discovered`].
#[derive(Clone, Debug)]
pub enum Request {
    /// Find the repository of a file or folder.
    Discover {
        path: PathBuf,
    },
    /// Diff a buffer snapshot against its base version, for gutter marks. `buffer` is any id the UI uses for
    /// the buffer and `version` its edit counter (echoed back so stale results can be dropped).
    Gutter {
        repo: PathBuf,
        path: String,
        buffer: u64,
        version: u64,
        text: Rope,
        base: GutterBase,
    },
    /// The whole status; also starts watching the repository.
    Status {
        repo: PathBuf,
        options: StatusOptions,
    },
    Refs {
        repo: PathBuf,
    },
    Log {
        repo: PathBuf,
        query: LogQuery,
    },
    FileHistory {
        repo: PathBuf,
        path: String,
        skip: usize,
        limit: usize,
    },
    CommitDetails {
        repo: PathBuf,
        id: Oid,
    },
    /// One file between two versions (`old_path` is its name on the old side after a rename).
    FileDiff {
        repo: PathBuf,
        path: String,
        old_path: Option<String>,
        old: Version,
        new: Version,
    },
    TreeDiff {
        repo: PathBuf,
        old: Version,
        new: Version,
    },
    /// Blame `path` at `rev` (HEAD when `None`), or the given unsaved buffer contents.
    Blame {
        repo: PathBuf,
        path: String,
        rev: Option<Oid>,
        contents: Option<Rope>,
    },
    ConflictVersions {
        repo: PathBuf,
        path: String,
    },
    Op {
        repo: PathBuf,
        op: Op,
    },
}

impl Request {
    fn repo(&self) -> Option<&Path> {
        match self {
            Request::Discover { .. } => None,
            Request::Gutter { repo, .. }
            | Request::Status { repo, .. }
            | Request::Refs { repo }
            | Request::Log { repo, .. }
            | Request::FileHistory { repo, .. }
            | Request::CommitDetails { repo, .. }
            | Request::FileDiff { repo, .. }
            | Request::TreeDiff { repo, .. }
            | Request::Blame { repo, .. }
            | Request::ConflictVersions { repo, .. }
            | Request::Op { repo, .. } => Some(repo),
        }
    }
}

/// The base a gutter diff was made against.
#[derive(Clone, Debug, PartialEq)]
pub struct GutterBaseText {
    pub against: GutterBase,
    /// The blob id: pass it back as [`Base::Blob`] when staging lines from these hunks (index base).
    pub id: Oid,
    /// Show deleted lines or revert a hunk in the buffer from it.
    pub text: Text,
}

/// How the file relates to the repository.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FileTrack {
    Tracked,
    /// Not in the base version (untracked, or new in the index when diffing against HEAD): no hunks.
    Untracked,
    /// Conflicted: there is no single staged version to diff against.
    Conflicted,
    /// Binary, a symlink or a submodule: no line diff.
    NotText,
    /// Bigger than [`ServiceOptions::gutter_max_bytes`]: not diffed.
    TooLarge,
}

/// Gutter marks for one buffer version.
#[derive(Clone, Debug, PartialEq)]
pub struct Gutter {
    pub buffer: u64,
    pub version: u64,
    pub path: String,
    pub hunks: Vec<Hunk>,
    pub base: Option<GutterBaseText>,
    pub file: FileTrack,
    /// Time spent on the worker (diff included), for the curious.
    pub elapsed: Duration,
}

/// What came back.
#[derive(Clone, Debug)]
pub enum Reply {
    /// `repo` is `None` outside any repository.
    Discovered {
        path: PathBuf,
        repo: Option<RepoInfo>,
    },
    Gutter(Result<Gutter>),
    Status(Result<Arc<Status>>),
    /// A watched repository's HEAD, index, refs or config moved (a new status follows). Re-request gutters
    /// when `index` (or `head`, for a HEAD base) changed.
    Changed(Changes),
    Refs(Result<Arc<Refs>>),
    Log(Result<Arc<LogPage>>),
    FileHistory(Result<Arc<HistoryPage>>),
    CommitDetails(Result<Arc<CommitDetails>>),
    FileDiff(Result<Arc<FileDiff>>),
    TreeDiff(Result<Arc<Vec<FileChange>>>),
    /// Some blamed ranges (more may follow until `BlameDone`).
    Blame(Vec<BlameHunk>),
    BlameDone(Result<()>),
    ConflictVersions(Result<Arc<[Option<Blob>; 3]>>),
    /// A progress line of a running [`Op`] (`Receiving objects:  45% ...`); throttled.
    Progress(String),
    OpDone(Result<OpOutput>),
}

#[derive(Clone, Debug)]
pub struct Event {
    /// The request answered; `None` for [`Reply::Changed`] and refresh statuses the service started itself.
    pub ticket: Option<Ticket>,
    /// The repository's workdir (`None` for `Discovered`).
    pub repo: Option<PathBuf>,
    pub reply: Reply,
}

/// Service settings.
#[derive(Clone, Debug)]
pub struct ServiceOptions {
    /// How often watched repositories are checked while focused; `None` never polls.
    pub poll_interval: Option<Duration>,
    /// Buffers bigger than this get no gutter diff ([`FileTrack::TooLarge`]).
    pub gutter_max_bytes: usize,
}

impl Default for ServiceOptions {
    fn default() -> Self {
        ServiceOptions { poll_interval: Some(Duration::from_secs(3)), gutter_max_bytes: 8 * 1024 * 1024 }
    }
}

const FAST: usize = 0;
const READ: usize = 1;
const BLAME: usize = 2;
const WRITE: usize = 3;
const LANE_NAMES: [&str; 4] = ["git-fast", "git-read", "git-blame", "git-write"];

/// What makes two pending requests the same.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Key {
    Discover(PathBuf),
    Gutter(u64),
    Status(PathBuf),
    Refs(PathBuf),
    Blame(PathBuf, String),
    Refresh(Option<PathBuf>),
}

enum Work {
    Req(Request),
    /// Check stamps (and re-run status when `full` or changed) for one repository or all watched ones.
    Refresh {
        repo: Option<PathBuf>,
        full: bool,
    },
}

struct Job {
    ticket: Option<Ticket>,
    key: Option<Key>,
    work: Work,
    /// Tickets of coalesced requests this job answers as well.
    also: Vec<Ticket>,
}

#[derive(Default)]
struct LaneState {
    items: VecDeque<Job>,
    current: Option<(Option<Key>, Cancel, Option<Ticket>)>,
    started: bool,
}

#[derive(Default)]
struct Lane {
    state: Mutex<LaneState>,
    cv: Condvar,
}

struct Watch {
    stamp: Stamp,
    options: StatusOptions,
}

#[derive(Default)]
struct Outbox {
    events: Vec<Event>,
    woken: bool,
}

struct Shared {
    wake: Wake,
    options: ServiceOptions,
    out: Mutex<Outbox>,
    lanes: [Lane; 4],
    tickets: AtomicU64,
    focused: AtomicBool,
    shutdown: AtomicBool,
    /// Repositories by workdir.
    repos: Mutex<HashMap<PathBuf, RepoInfo>>,
    /// Discovery results by folder.
    discovered: Mutex<HashMap<PathBuf, Option<PathBuf>>>,
    watched: Mutex<HashMap<PathBuf, Watch>>,
    /// Bumped when a repository's config changed: lanes reopen their repositories.
    config_gen: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The background git service. Dropping it stops its threads (and kills a running `git`).
pub struct GitService {
    shared: Arc<Shared>,
}

impl GitService {
    /// Creates the service; no thread starts until the first request.
    pub fn new(wake: Wake) -> GitService {
        GitService::with_options(wake, ServiceOptions::default())
    }

    pub fn with_options(wake: Wake, options: ServiceOptions) -> GitService {
        GitService {
            shared: Arc::new(Shared {
                wake,
                options,
                out: Mutex::new(Outbox::default()),
                lanes: Default::default(),
                tickets: AtomicU64::new(1),
                focused: AtomicBool::new(true),
                shutdown: AtomicBool::new(false),
                repos: Mutex::new(HashMap::new()),
                discovered: Mutex::new(HashMap::new()),
                watched: Mutex::new(HashMap::new()),
                config_gen: AtomicU64::new(0),
            }),
        }
    }

    /// Queues a request.
    pub fn send(&self, req: Request) -> Ticket {
        let ticket = Ticket(self.shared.tickets.fetch_add(1, Ordering::Relaxed));
        let (lane, key) = match &req {
            Request::Discover { path } => (FAST, Some(Key::Discover(path.clone()))),
            Request::Gutter { buffer, .. } => (FAST, Some(Key::Gutter(*buffer))),
            Request::ConflictVersions { .. } | Request::FileDiff { .. } => (FAST, None),
            Request::Status { repo, .. } => (READ, Some(Key::Status(repo.clone()))),
            Request::Refs { repo } => (READ, Some(Key::Refs(repo.clone()))),
            Request::Log { .. }
            | Request::FileHistory { .. }
            | Request::CommitDetails { .. }
            | Request::TreeDiff { .. } => (READ, None),
            Request::Blame { repo, path, .. } => (BLAME, Some(Key::Blame(repo.clone(), path.clone()))),
            Request::Op { .. } => (WRITE, None),
        };
        Shared::push(&self.shared, lane, Job { ticket: Some(ticket), key, work: Work::Req(req), also: Vec::new() });
        ticket
    }

    /// Takes the events that arrived since the last poll (cheap; never blocks on git work).
    pub fn poll(&self) -> Vec<Event> {
        let mut out = lock(&self.shared.out);
        out.woken = false;
        std::mem::take(&mut out.events)
    }

    /// Cancels a request: dropped if still queued, stopped (git killed, walks interrupted) if running.
    pub fn cancel(&self, ticket: Ticket) {
        for lane in &self.shared.lanes {
            let mut st = lock(&lane.state);
            for j in st.items.iter_mut() {
                j.also.retain(|t| *t != ticket);
                // a request others were coalesced into still runs for them
                if j.ticket == Some(ticket) && !j.also.is_empty() {
                    j.ticket = Some(j.also.remove(0));
                }
            }
            st.items.retain(|j| j.ticket != Some(ticket));
            if let Some((_, c, Some(t))) = &st.current
                && *t == ticket
            {
                c.cancel();
            }
        }
    }

    /// Tell the service whether the window has focus: it only polls watched repositories while focused,
    /// and re-reads their status when focus comes back.
    pub fn set_focused(&self, focused: bool) {
        let was = self.shared.focused.swap(focused, Ordering::SeqCst);
        if focused && !was {
            lock(&self.shared.discovered).retain(|_, v| v.is_some()); // a folder may have become a repository
            if !lock(&self.shared.watched).is_empty() {
                Shared::push(
                    &self.shared,
                    READ,
                    Job {
                        ticket: None,
                        key: Some(Key::Refresh(None)),
                        work: Work::Refresh { repo: None, full: true },
                        also: Vec::new(),
                    },
                );
            }
        }
        self.shared.lanes[READ].cv.notify_all();
    }

    /// Re-reads the status of a watched repository (call after saving a file in it). Coalesced.
    pub fn refresh(&self, repo: &Path) {
        if lock(&self.shared.watched).contains_key(repo) {
            let repo = repo.to_path_buf();
            Shared::push(
                &self.shared,
                READ,
                Job {
                    ticket: None,
                    key: Some(Key::Refresh(Some(repo.clone()))),
                    work: Work::Refresh { repo: Some(repo), full: true },
                    also: Vec::new(),
                },
            );
        }
    }

    /// Stops watching a repository (no more polling or refreshes for it).
    pub fn unwatch(&self, repo: &Path) {
        lock(&self.shared.watched).remove(repo);
    }

    /// Whether any worker thread has been started (for tests and diagnostics).
    pub fn is_started(&self) -> bool {
        self.shared.lanes.iter().any(|l| lock(&l.state).started)
    }
}

impl Drop for GitService {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        for lane in &self.shared.lanes {
            let mut st = lock(&lane.state);
            st.items.clear();
            if let Some((_, c, _)) = &st.current {
                c.cancel();
            }
            drop(st);
            lane.cv.notify_all();
        }
    }
}

impl Shared {
    fn push(this: &Arc<Shared>, lane: usize, job: Job) {
        let l = &this.lanes[lane];
        let mut st = lock(&l.state);
        if let Some(key) = job.key.clone() {
            if let Some(i) = st.items.iter().position(|j| j.key.as_ref() == Some(&key)) {
                let mut job = job;
                if !matches!(key, Key::Gutter(_)) {
                    let old = &st.items[i];
                    job.also = old.ticket.into_iter().chain(old.also.iter().copied()).collect();
                }
                let old = std::mem::replace(&mut st.items[i], job);
                // a coalesced status keeps the widest options of both
                if let (Work::Req(Request::Status { options: old_o, .. }), Work::Req(Request::Status { options, .. })) =
                    (&old.work, &mut st.items[i].work)
                {
                    options.ignored |= old_o.ignored;
                    if old_o.untracked == Untracked::All || options.untracked == Untracked::None {
                        options.untracked = old_o.untracked;
                    }
                }
            } else {
                st.items.push_back(job);
            }
            // a new blame of the same file supersedes the running one
            if let (Key::Blame(..), Some((Some(k), c, _))) = (&key, &st.current)
                && *k == key
            {
                c.cancel();
            }
        } else {
            st.items.push_back(job);
        }
        if !st.started {
            st.started = true;
            let shared = this.clone();
            let spawned = std::thread::Builder::new().name(LANE_NAMES[lane].into()).spawn(move || shared.run(lane));
            if let Err(e) = spawned {
                log::error!("could not start the git thread: {e}");
                st.started = false;
            }
        }
        drop(st);
        l.cv.notify_one();
    }

    fn emit(&self, ticket: Option<Ticket>, repo: Option<PathBuf>, reply: Reply) {
        let mut out = lock(&self.out);
        out.events.push(Event { ticket, repo, reply });
        let wake = !out.woken;
        out.woken = true;
        drop(out);
        if wake {
            (self.wake)();
        }
    }

    /// The lane loop: take a job, run it, repeat; the read lane polls stamps while focused and idle.
    fn run(self: Arc<Self>, lane: usize) {
        let mut worker = Worker { shared: self.clone(), repos: HashMap::new(), config_seen: 0, bases: HashMap::new() };
        // when the next stamp poll is due: a deadline, so wakeups (new jobs, focus changes, spurious ones) do not
        // keep postponing it the way restarting a full-interval wait would
        let mut next_poll: Option<Instant> = None;
        loop {
            let (job, cancel) = {
                let l = &self.lanes[lane];
                let mut st = lock(&l.state);
                loop {
                    if self.shutdown.load(Ordering::SeqCst) {
                        return;
                    }
                    if let Some(job) = st.items.pop_front() {
                        let cancel = Cancel::new();
                        st.current = Some((job.key.clone(), cancel.clone(), job.ticket));
                        break (job, cancel);
                    }
                    let interval = self.options.poll_interval.filter(|_| {
                        lane == READ && self.focused.load(Ordering::SeqCst) && !lock(&self.watched).is_empty()
                    });
                    match interval {
                        Some(d) => {
                            let now = Instant::now();
                            let due = *next_poll.get_or_insert(now + d);
                            if now >= due {
                                // idle, focused and due: look at the stamps, then schedule the next look
                                next_poll = None;
                                drop(st);
                                worker.refresh(None, false);
                                st = lock(&l.state);
                                continue;
                            }
                            st = l.cv.wait_timeout(st, due - now).unwrap_or_else(|e| e.into_inner()).0;
                        }
                        None => {
                            next_poll = None;
                            st = l.cv.wait(st).unwrap_or_else(|e| e.into_inner());
                        }
                    }
                }
            };
            worker.handle(job, &cancel);
            lock(&self.lanes[lane].state).current = None;
        }
    }
}

/// One lane's thread-local state: its open repositories and the gutter base cache.
struct Worker {
    shared: Arc<Shared>,
    repos: HashMap<PathBuf, Repo>,
    config_seen: u64,
    /// Gutter bases by (repo, path, base): the blob id and its text.
    bases: HashMap<(PathBuf, String, GutterBase), (Oid, Text)>,
}

impl Worker {
    fn repo(&mut self, workdir: &Path) -> Result<Repo> {
        let generation = self.shared.config_gen.load(Ordering::SeqCst);
        if generation != self.config_seen {
            self.repos.clear();
            self.config_seen = generation;
        }
        if let Some(r) = self.repos.get(workdir) {
            return Ok(r.clone());
        }
        let info = lock(&self.shared.repos).get(workdir).cloned();
        let repo = match info {
            Some(info) => Repo::open(&info)?,
            None => {
                let r = Repo::discover(workdir)?.ok_or_else(|| {
                    Error::new(ErrorKind::NotARepo, format!("{} is not a git repository", workdir.display()))
                })?;
                lock(&self.shared.repos).insert(workdir.to_path_buf(), r.info().clone());
                r
            }
        };
        self.repos.insert(workdir.to_path_buf(), repo.clone());
        Ok(repo)
    }

    fn handle(&mut self, job: Job, cancel: &Cancel) {
        let (ticket, also) = (job.ticket, job.also);
        let req = match job.work {
            Work::Refresh { repo, full } => return self.refresh(repo, full),
            Work::Req(r) => r,
        };
        let shared = self.shared.clone();
        // the reply goes to the request's ticket and to every ticket coalesced into it
        let send = |repo: Option<PathBuf>, reply: Reply| {
            for t in &also {
                shared.emit(Some(*t), repo.clone(), reply.clone());
            }
            shared.emit(ticket, repo, reply);
        };
        if let Request::Discover { path } = req {
            let repo = self.discover(&path);
            return send(None, Reply::Discovered { path, repo });
        }
        let workdir = req.repo().expect("every other request names a repository").to_path_buf();
        let repo = match self.repo(&workdir) {
            Ok(r) => r,
            Err(e) => return send(Some(workdir), error_reply(&req, e)),
        };
        let emit = |reply: Reply| send(Some(workdir.clone()), reply);
        match req {
            Request::Discover { .. } => unreachable!(),
            Request::Gutter { path, buffer, version, text, base, .. } => {
                let r = self.gutter(&repo, &workdir, path, buffer, version, &text, base);
                emit(Reply::Gutter(r));
            }
            Request::Status { options, .. } => {
                let stamp = repo.stamp();
                let r = repo.status(&options, cancel).map(Arc::new);
                if r.is_ok() {
                    lock(&self.shared.watched).insert(workdir.clone(), Watch { stamp, options });
                }
                emit(Reply::Status(r));
            }
            Request::Refs { .. } => emit(Reply::Refs(repo.refs().map(Arc::new))),
            Request::Log { query, .. } => emit(Reply::Log(repo.log(&query, cancel).map(Arc::new))),
            Request::FileHistory { path, skip, limit, .. } => {
                emit(Reply::FileHistory(repo.file_history(&path, skip, limit, cancel).map(Arc::new)))
            }
            Request::CommitDetails { id, .. } => emit(Reply::CommitDetails(repo.commit_details(id).map(Arc::new))),
            Request::FileDiff { path, old_path, old, new, .. } => {
                emit(Reply::FileDiff(repo.file_diff(&path, old_path.as_deref(), old, new).map(Arc::new)))
            }
            Request::TreeDiff { old, new, .. } => emit(Reply::TreeDiff(repo.tree_diff(old, new, cancel).map(Arc::new))),
            Request::ConflictVersions { path, .. } => {
                emit(Reply::ConflictVersions(repo.conflict_versions(&path).map(Arc::new)))
            }
            Request::Blame { path, rev, contents, .. } => {
                let contents = contents.as_ref().map(Text::from_rope);
                let mut batch = Vec::new();
                let mut last = Instant::now();
                let r = repo.blame(&path, rev, contents.as_ref(), cancel, &mut |h| {
                    batch.push(h);
                    // first lines at once, then in batches so a big file does not flood the UI
                    if batch.len() >= 256 || last.elapsed() >= Duration::from_millis(50) {
                        emit(Reply::Blame(std::mem::take(&mut batch)));
                        last = Instant::now();
                    }
                });
                if !batch.is_empty() {
                    emit(Reply::Blame(batch));
                }
                emit(Reply::BlameDone(r));
            }
            Request::Op { op, .. } => {
                let mut pending: Option<String> = None;
                let mut last = Instant::now() - Duration::from_secs(1);
                let r = repo.run_op(&op, cancel, &mut |line| {
                    if last.elapsed() >= Duration::from_millis(50) {
                        emit(Reply::Progress(line.to_string()));
                        last = Instant::now();
                        pending = None;
                    } else {
                        pending = Some(line.to_string());
                    }
                });
                if let Some(line) = pending {
                    emit(Reply::Progress(line));
                }
                emit(Reply::OpDone(r));
                // the op moved HEAD, the index or the worktree: refresh what the UI shows
                Shared::push(
                    &self.shared,
                    READ,
                    Job {
                        ticket: None,
                        key: Some(Key::Refresh(Some(workdir.clone()))),
                        work: Work::Refresh { repo: Some(workdir.clone()), full: true },
                        also: Vec::new(),
                    },
                );
            }
        }
    }

    fn discover(&mut self, path: &Path) -> Option<RepoInfo> {
        let dir = if path.is_dir() { path.to_path_buf() } else { path.parent().unwrap_or(path).to_path_buf() };
        let known = lock(&self.shared.discovered).get(&dir).cloned();
        let workdir = match known {
            Some(w) => w,
            None => {
                let found = match Repo::discover(&dir) {
                    Ok(r) => r,
                    Err(e) => {
                        log::debug!("git discovery at {}: {e}", dir.display());
                        None
                    }
                };
                let w = found.map(|r| {
                    let info = r.info().clone();
                    self.repos.insert(info.workdir.clone(), r);
                    lock(&self.shared.repos).insert(info.workdir.clone(), info.clone());
                    info.workdir
                });
                lock(&self.shared.discovered).insert(dir, w.clone());
                w
            }
        };
        let info = lock(&self.shared.repos).get(&workdir?).cloned()?;
        // a path inside `.git` has no repository of its own
        let repo = self.repo(&info.workdir).ok()?;
        let is_root = crate::canonicalize(path).is_ok_and(|p| p == info.workdir);
        (is_root || repo.rel_path(path).is_some()).then_some(info)
    }

    #[allow(clippy::too_many_arguments)]
    fn gutter(
        &mut self,
        repo: &Repo,
        workdir: &Path,
        path: String,
        buffer: u64,
        version: u64,
        text: &Rope,
        base: GutterBase,
    ) -> Result<Gutter> {
        let start = Instant::now();
        let mut out = Gutter {
            buffer,
            version,
            path,
            hunks: Vec::new(),
            base: None,
            file: FileTrack::Tracked,
            elapsed: Duration::ZERO,
        };
        if text.len_bytes() > self.shared.options.gutter_max_bytes {
            out.file = FileTrack::TooLarge;
            return Ok(out);
        }
        let entry = match base {
            GutterBase::Index => repo.index_entry(&out.path)?,
            GutterBase::Head => repo.head_entry(&out.path)?,
        };
        let Some((id, mode)) = entry else {
            out.file = if base == GutterBase::Index && repo.is_conflicted(&out.path)? {
                FileTrack::Conflicted
            } else {
                FileTrack::Untracked
            };
            return Ok(out);
        };
        if mode != MODE_FILE && mode != MODE_EXEC {
            out.file = FileTrack::NotText;
            return Ok(out);
        }
        let key = (workdir.to_path_buf(), out.path.clone(), base);
        let base_text = match self.bases.get(&key) {
            Some((cached, t)) if *cached == id => t.clone(),
            _ => {
                let t = repo.blob_by_id(id)?;
                if self.bases.len() >= 64 {
                    self.bases.clear();
                }
                self.bases.insert(key, (id, t.clone()));
                t
            }
        };
        if base_text.is_binary() {
            out.file = FileTrack::NotText;
            return Ok(out);
        }
        let new = Text::from_rope(text);
        out.hunks = diff(&base_text, &new, EolMode::Normalize);
        out.base = Some(GutterBaseText { against: base, id, text: base_text });
        out.elapsed = start.elapsed();
        Ok(out)
    }

    /// Checks stamps of one or every watched repository; re-runs the status when `full` or something moved.
    fn refresh(&mut self, only: Option<PathBuf>, full: bool) {
        let targets: Vec<(PathBuf, Stamp, StatusOptions)> = lock(&self.shared.watched)
            .iter()
            .filter(|(w, _)| only.as_ref().is_none_or(|o| o == *w))
            .map(|(w, s)| (w.clone(), s.stamp.clone(), s.options.clone()))
            .collect();
        for (workdir, old, options) in targets {
            let info = lock(&self.shared.repos).get(&workdir).cloned();
            let Some(info) = info else { continue };
            let stamp = Stamp::read(&info);
            let changes = stamp.changes(&old);
            if changes.config {
                self.shared.config_gen.fetch_add(1, Ordering::SeqCst);
            }
            if changes.any() {
                self.shared.emit(None, Some(workdir.clone()), Reply::Changed(changes));
            }
            if !(full || changes.any()) {
                continue;
            }
            let Ok(repo) = self.repo(&workdir) else { continue };
            let r = repo.status(&options, &Cancel::new()).map(Arc::new);
            if let Some(w) = lock(&self.shared.watched).get_mut(&workdir) {
                w.stamp = stamp;
            }
            self.shared.emit(None, Some(workdir), Reply::Status(r));
        }
    }
}

/// The error reply matching a request's kind.
fn error_reply(req: &Request, e: Error) -> Reply {
    match req {
        Request::Discover { path } => Reply::Discovered { path: path.clone(), repo: None },
        Request::Gutter { .. } => Reply::Gutter(Err(e)),
        Request::Status { .. } => Reply::Status(Err(e)),
        Request::Refs { .. } => Reply::Refs(Err(e)),
        Request::Log { .. } => Reply::Log(Err(e)),
        Request::FileHistory { .. } => Reply::FileHistory(Err(e)),
        Request::CommitDetails { .. } => Reply::CommitDetails(Err(e)),
        Request::FileDiff { .. } => Reply::FileDiff(Err(e)),
        Request::TreeDiff { .. } => Reply::TreeDiff(Err(e)),
        Request::Blame { .. } => Reply::BlameDone(Err(e)),
        Request::ConflictVersions { .. } => Reply::ConflictVersions(Err(e)),
        Request::Op { .. } => Reply::OpDone(Err(e)),
    }
}
