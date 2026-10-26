//! The diff view: a tab showing two versions of a file, or of every changed file, side by side or unified.
//!
//! * Opened by the Git commands (`git_diff_file`, `:Gdiff [rev]`, `git_diff_staged`, `git_diff_all`,
//!   `git_diff_commit [rev]`, `git_diff_compare [a] [b]`) or by other parts of the app through
//!   [`State::open_diff`] with a [`DiffSpec`]. The tab is an ordinary buffer with a title
//!   ([`shrew_core::Document::title`]), so Ctrl+Tab, the buffer list and `:q` work as usual; this module keeps the
//!   view itself, keyed by that buffer's id.
//! * Every diff is computed by the [`shrew_git::GitService`]: `FileDiff` for versions git has, `Gutter` (diffed
//!   against the index or HEAD) when the working-tree side is open in a buffer, so edits show as you type, and
//!   `TreeDiff` / `CommitDetails` for the file list of a multi-file diff. Ignoring white space re-diffs on a
//!   short-lived thread. The UI thread only lays out rows (shrew-core's `diffview`) and draws the visible ones.
//! * Views that compare against the working tree or the index follow the repository: they reload when it changes
//!   (staging from the panel or a terminal, a commit), when the window gets the focus back, and after their own
//!   stage, unstage and revert actions.

mod draw;
mod input;
#[cfg(test)]
mod tests;
mod view;

pub use draw::draw;
pub use input::{drag, hover, key, press, release, row_under, wheel};
pub use view::DiffView;

use crate::state::State;
use shrew_core::diffview::{self as model, Layout};
use shrew_core::picker::{PickerData, PickerItem};
use shrew_core::{Choice, Command, Document};
use shrew_git::diff::{EolMode, Hunk, WordDiff};
use shrew_git::patch::LineSel;
use shrew_git::{Base, Event, FileTrack, LogQuery, Oid, Op, Reply, Request, Text, Ticket, Version};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use view::{Data, Live, Load, Section, Side, SideText};

/// A version of a file or of the whole tree, as the diff view names one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Rev {
    Head,
    Index,
    Worktree,
    Commit(Oid),
    /// Anything git understands (`main`, `v1.2`, `HEAD~3`, a hash), resolved in the background.
    Named(String),
    /// The first parent of a commit; nothing for a root commit.
    Parent(Box<Rev>),
    /// No file at all (the old side of a root commit).
    Empty,
}

impl Rev {
    pub fn label(&self) -> String {
        match self {
            Rev::Head => "HEAD".into(),
            Rev::Index => "index".into(),
            Rev::Worktree => "working tree".into(),
            Rev::Commit(o) => o.short(),
            Rev::Named(s) => s.clone(),
            Rev::Parent(r) => format!("{}^", r.label()),
            Rev::Empty => "nothing".into(),
        }
    }

    /// The service's version, for a resolved side (`Empty` has none).
    fn version(&self) -> Option<Version> {
        match self {
            Rev::Head => Some(Version::Head),
            Rev::Index => Some(Version::Index),
            Rev::Worktree => Some(Version::Worktree),
            Rev::Commit(o) => Some(Version::Commit(*o)),
            _ => None,
        }
    }

    fn resolved(&self) -> bool {
        !matches!(self, Rev::Named(_) | Rev::Parent(_))
    }

    /// Whether it can change without the view asking (the working tree, the index, a branch name).
    fn moves(&self) -> bool {
        !matches!(self, Rev::Commit(_) | Rev::Empty)
    }

    /// What `git log -1 <rev>` takes to name it.
    fn spec(&self) -> String {
        match self {
            Rev::Commit(o) => o.to_string(),
            Rev::Head => "HEAD".into(),
            r => r.label(),
        }
    }
}

/// What a diff tab compares.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiffSpec {
    /// The repository's workdir; found from `path` (or the active file, the project, the current folder) when
    /// `None`.
    pub repo: Option<PathBuf>,
    /// One file: absolute, or relative to `repo` (to the current folder when `repo` is `None`). `None` compares
    /// every changed file.
    pub path: Option<PathBuf>,
    /// The file's repository path on the old side, after a rename.
    pub old_path: Option<String>,
    pub left: Rev,
    pub right: Rev,
}

impl DiffSpec {
    /// One file between two versions.
    pub fn file(path: impl Into<PathBuf>, left: Rev, right: Rev) -> DiffSpec {
        DiffSpec { repo: None, path: Some(path.into()), old_path: None, left, right }
    }

    /// Every changed file between two versions.
    pub fn tree(left: Rev, right: Rev) -> DiffSpec {
        DiffSpec { repo: None, path: None, old_path: None, left, right }
    }

    /// What is not staged yet (index ↔ working tree).
    pub fn unstaged() -> DiffSpec {
        DiffSpec::tree(Rev::Index, Rev::Worktree)
    }

    /// What is staged (HEAD ↔ index).
    pub fn staged() -> DiffSpec {
        DiffSpec::tree(Rev::Head, Rev::Index)
    }

    /// A commit against its first parent.
    pub fn commit(rev: Rev) -> DiffSpec {
        DiffSpec::tree(Rev::Parent(Box::new(rev.clone())), rev)
    }

    #[allow(dead_code)] // for the panel and the log view, which know their repository
    pub fn in_repo(mut self, workdir: impl Into<PathBuf>) -> DiffSpec {
        self.repo = Some(workdir.into());
        self
    }

    /// The tab title.
    pub fn title(&self) -> String {
        let (l, r) = (self.left.label(), self.right.label());
        match (&self.path, &self.left, &self.right) {
            (Some(p), ..) => {
                let name = p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into());
                format!("diff: {name} ({l} ↔ {r})")
            }
            (None, Rev::Index, Rev::Worktree) => "diff: changes (index ↔ working tree)".into(),
            (None, Rev::Head, Rev::Index) => "diff: staged (HEAD ↔ index)".into(),
            (None, Rev::Parent(p), right) if **p == *right => format!("commit {}", right.label()),
            _ => format!("diff: {l} ↔ {r}"),
        }
    }
}

/// Multi-file diffs open every file when there are at most this many (and few enough lines); otherwise they open
/// collapsed and a file loads when it is opened.
const OPEN_FILES: usize = 40;
const OPEN_LINES: u32 = 20_000;
/// Commits listed in the commit picker.
const PICK_COMMITS: usize = 500;
/// Gutter requests use buffer ids with this bit set, so they never coalesce with the gutter's own.
const GUTTER_KEY: u64 = 1 << 62;

/// What a ticket is for.
#[derive(Clone, Debug)]
enum Wait {
    /// The repository of a view (by its tab id).
    Discover(u64),
    /// A side of a view (`right`: the new side) being resolved.
    Resolve {
        view: u64,
        right: bool,
    },
    /// The changed files of a multi-file view.
    Files(u64),
    File {
        view: u64,
        section: usize,
        generation: u64,
    },
    Gutter {
        view: u64,
        section: usize,
    },
    Op {
        view: u64,
        done: &'static str,
    },
    /// The repository a picker lists, then its list.
    PickRepo(Choice),
    Commits(PathBuf),
    Refs(PathBuf),
}

/// A white-space-ignoring diff done on a worker thread.
struct Job {
    view: u64,
    section: usize,
    generation: u64,
    hunks: Vec<Hunk>,
    words: Vec<(u32, WordDiff)>,
}

/// Every diff tab, by its buffer's id.
#[derive(Default)]
pub struct Views {
    pub views: HashMap<u64, DiffView>,
    next_id: u64,
    waiting: HashMap<Ticket, Wait>,
    jobs: Arc<Mutex<Vec<Job>>>,
    /// Re-diffs still running (headless screenshots wait for them).
    running: Arc<std::sync::atomic::AtomicUsize>,
    pub defaults: Defaults,
    /// The compare picker's first choice, while the second is picked.
    compare_first: Option<String>,
    /// The tab that was active after the last input.
    active: Option<u64>,
}

/// `[git]` settings for new views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Defaults {
    pub layout: Layout,
    pub context: u32,
    pub ignore_ws: bool,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults { layout: Layout::Split, context: shrew_config::DEFAULT_DIFF_CONTEXT, ignore_ws: false }
    }
}

impl Views {
    /// Takes `[git]` from the config (at startup and when it is reloaded).
    pub fn configure(&mut self, cfg: &shrew_config::GitConfig) {
        self.defaults = Defaults {
            layout: Layout::from_name(cfg.diff_layout()).unwrap_or_default(),
            context: cfg.diff_context(),
            ignore_ws: cfg.diff_ignore_whitespace(),
        };
    }

    /// The view of the active tab, if it is one.
    pub fn active<'a>(&'a self, ed: &shrew_core::Editor) -> Option<&'a DiffView> {
        self.views.get(&ed.buf().id).filter(|_| ed.buf().doc.title.is_some())
    }
}

/// Whether the active tab is a diff (it then draws and takes the keys instead of the text).
pub fn is_active(st: &State) -> bool {
    st.git.diff.active(&st.ed).is_some()
}

/// The status bar's position for a diff tab: which change the cursor is at, and its line.
pub fn position(st: &State) -> Option<String> {
    let v = st.git.diff.active(&st.ed)?;
    let Some(r) = v.rows.rows.get(v.cursor) else { return Some(v.message.clone().unwrap_or_default()) };
    let n = v.rows.starts.len();
    let k = v.rows.starts.partition_point(|&s| s as usize <= v.cursor);
    let line = match v.side {
        Side::Old => r.old().or(r.new_line()),
        Side::New => r.new_line().or(r.old()),
    };
    let at = if n == 0 { "no changes".to_string() } else { format!("change {}/{n}", k.max(1)) };
    Some(match line {
        Some(l) if matches!(r.kind, model::RowKind::Same | model::RowKind::Change) => format!("{at} · line {}", l + 1),
        _ => at,
    })
}

impl State {
    /// Opens a diff tab, or switches to the one showing the same thing (and refreshes it). The API for the rest of
    /// the app (the Source Control panel, the log view).
    #[allow(dead_code)]
    pub fn open_diff(&mut self, spec: DiffSpec) {
        open(self, spec);
    }
}

fn open(st: &mut State, mut spec: DiffSpec) {
    // a path relative to the repository or to the current folder
    if let Some(p) = &spec.path
        && p.is_relative()
    {
        spec.path = Some(match &spec.repo {
            Some(r) => r.join(p),
            None => st.ed.absolute(p),
        });
    }
    if let Some((&buf, _)) = st.git.diff.views.iter().find(|(_, v)| v.spec == spec)
        && let Some(i) = st.ed.buffers.iter().position(|b| b.id == buf)
    {
        st.ed.switch_to(i);
        refresh(st, buf);
        return;
    }
    // where to look for the repository, from the tab that was active before this one
    let at = spec.path.clone().unwrap_or_else(|| context_dir(st));
    let mut doc = Document::default();
    doc.title = Some(spec.title());
    let i = st.ed.add_buffer(doc);
    st.ed.switch_to(i);
    let buf = st.ed.buffers[i].id;
    let d = st.git.diff.defaults;
    st.git.diff.next_id += 1;
    let id = st.git.diff.next_id;
    let view = DiffView::new(id, buf, spec.clone(), d.layout, d.context, d.ignore_ws);
    st.git.diff.views.insert(buf, view);
    st.git.diff.active = Some(buf);
    match &spec.repo {
        Some(repo) => {
            let repo = repo.clone();
            watch(st, &repo);
            start(st, buf);
        }
        None => {
            let t = super::send(st, Request::Discover { path: at });
            st.git.diff.waiting.insert(t, Wait::Discover(buf));
        }
    }
}

/// Where a command without a file looks for the repository: the active file, a diff tab's repository, the
/// project, the current folder.
fn context_dir(st: &State) -> PathBuf {
    if let Some(p) = &st.ed.buf().doc.path {
        return p.clone();
    }
    if let Some(r) = st.git.diff.active(&st.ed).and_then(|v| v.repo.clone()) {
        return r;
    }
    st.ed.project_root().unwrap_or_else(|| st.ed.cwd.clone())
}

/// Resolves both sides, then loads.
fn start(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
    let Some(repo) = v.repo.clone() else { return };
    v.left = v.spec.left.resolved().then(|| v.spec.left.clone());
    v.right = v.spec.right.resolved().then(|| v.spec.right.clone());
    let todo: Vec<(bool, String)> = [(false, &v.spec.left), (true, &v.spec.right)]
        .into_iter()
        .filter(|(_, r)| !r.resolved())
        .map(|(right, r)| {
            let name = match r {
                Rev::Parent(inner) => inner.spec(),
                r => r.spec(),
            };
            (right, name)
        })
        .collect();
    if todo.is_empty() {
        return load(st, buf);
    }
    for (right, name) in todo {
        let query = LogQuery { revs: vec![name], limit: 1, ..LogQuery::default() };
        let t = super::send(st, Request::Log { repo: repo.clone(), query });
        st.git.diff.waiting.insert(t, Wait::Resolve { view: buf, right });
    }
}

/// Asks for the file list (multi-file) or the file.
fn load(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
    let (Some(repo), Some(left), Some(right)) = (v.repo.clone(), v.left.clone(), v.right.clone()) else { return };
    if v.multi {
        let req = match (&left, right.version()) {
            (Rev::Empty, Some(Version::Commit(id))) => Request::CommitDetails { repo, id },
            (l, Some(new)) => match l.version() {
                Some(old) => Request::TreeDiff { repo, old, new },
                None => return,
            },
            _ => return,
        };
        let t = super::send(st, req);
        st.git.diff.waiting.insert(t, Wait::Files(buf));
        return;
    }
    if v.sections.is_empty() {
        let path = v.spec.path.as_deref().and_then(|p| rel_path(&repo, p));
        let Some(path) = path else {
            v.message = Some("the file is not in this repository".into());
            return;
        };
        let mut s = Section::new(path);
        s.old_path = v.spec.old_path.clone();
        v.sections.push(s);
        v.stale = true;
    }
    load_section(st, buf, 0, true);
}

/// `path` relative to `workdir`, with `/` separators.
fn rel_path(workdir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(workdir).ok()?;
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The open buffer that stands for section `i`'s working-tree file, when the view compares it with the index or
/// HEAD (the gutter diff then runs on the buffer's text).
fn live_buffer(st: &State, v: &DiffView, i: usize) -> Option<usize> {
    if v.right != Some(Rev::Worktree) || !matches!(v.left, Some(Rev::Index | Rev::Head)) {
        return None;
    }
    let abs = v.repo.as_ref()?.join(&v.sections[i].path);
    st.ed.buffers.iter().position(|b| b.doc.path.as_deref() == Some(abs.as_path()) && b.doc.title.is_none())
}

/// Requests section `i` of a view (again). `force`: even when a live buffer has not changed.
fn load_section(st: &mut State, buf: u64, i: usize, force: bool) {
    let Some(v) = st.git.diff.views.get(&buf) else { return };
    let Some(s) = v.sections.get(i) else { return };
    let (Some(repo), Some(left), Some(right)) = (v.repo.clone(), v.left.clone(), v.right.clone()) else { return };
    let live = live_buffer(st, v, i).filter(|_| s.live.as_ref().is_none_or(|l| l.buffer != u64::MAX));
    let (path, old_path) = (s.path.clone(), s.old_path.clone());
    if let Some(bi) = live {
        let b = &st.ed.buffers[bi];
        let (bid, version, rope) = (b.id, b.doc.version, b.doc.rope.clone());
        let v = st.git.diff.views.get_mut(&buf).expect("checked above");
        let s = &mut v.sections[i];
        let l = s.live.get_or_insert(Live { buffer: bid, sent: None, snapshots: Vec::new() });
        if l.buffer != bid {
            *l = Live { buffer: bid, sent: None, snapshots: Vec::new() };
        }
        if !force && l.sent == Some(version) {
            return;
        }
        l.sent = Some(version);
        l.snapshots.retain(|(ver, _)| *ver != version);
        l.snapshots.push((version, rope.clone()));
        if l.snapshots.len() > 4 {
            l.snapshots.remove(0);
        }
        if s.data.is_none() {
            s.load = Load::Waiting;
        }
        let base = if left == Rev::Head { shrew_git::GutterBase::Head } else { shrew_git::GutterBase::Index };
        let key = GUTTER_KEY | (v.id << 20) | i as u64;
        let req = Request::Gutter { repo, path, buffer: key, version, text: rope, base };
        let t = super::send(st, req);
        let waiting = &mut st.git.diff.waiting;
        waiting.retain(|_, w| !matches!(w, Wait::Gutter { view, section } if *view == buf && *section == i));
        waiting.insert(t, Wait::Gutter { view: buf, section: i });
        return;
    }
    let v = st.git.diff.views.get_mut(&buf).expect("checked above");
    let s = &mut v.sections[i];
    s.live = None;
    s.generation += 1;
    s.load = Load::Waiting;
    if s.data.is_none() {
        s.note = Some("loading…".into());
        v.stale = true;
    }
    let generation = s.generation;
    let (Some(new), old) = (right.version(), left.version()) else { return };
    let req = Request::FileDiff { repo, path, old_path, old: old.unwrap_or(new), new };
    let t = super::send(st, req);
    st.git.diff.waiting.insert(t, Wait::File { view: buf, section: i, generation });
}

/// Loads a view again when what it shows may have moved.
fn refresh(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get(&buf) else { return };
    if v.repo.is_none() {
        return;
    }
    let moves = v.spec.left.moves() || v.spec.right.moves();
    if !moves {
        return;
    }
    let named = |r: &Rev| matches!(r, Rev::Named(_) | Rev::Parent(_));
    if named(&v.spec.left) || named(&v.spec.right) {
        return start(st, buf);
    }
    if v.multi {
        return load(st, buf);
    }
    load_section(st, buf, 0, true);
}

// ----- events -----

/// One event from the service.
pub fn event(st: &mut State, ev: &Event) {
    if let Reply::Gutter(Ok(g)) = &ev.reply
        && g.buffer & GUTTER_KEY != 0
    {
        let id = (g.buffer & !GUTTER_KEY) >> 20;
        let section = (g.buffer & 0xf_ffff) as usize;
        if let Some(t) = ev.ticket {
            st.git.diff.waiting.remove(&t);
        }
        if let Some((&buf, _)) = st.git.diff.views.iter().find(|(_, v)| v.id == id) {
            gutter(st, buf, section, g);
        }
        return;
    }
    let wait = ev.ticket.and_then(|t| st.git.diff.waiting.remove(&t));
    match (wait, &ev.reply) {
        (Some(Wait::Discover(buf)), Reply::Discovered { repo, .. }) => {
            let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
            match repo {
                Some(info) => {
                    v.repo = Some(info.workdir.clone());
                    watch(st, &info.workdir);
                    start(st, buf);
                }
                None => v.message = Some("not in a git repository".into()),
            }
        }
        (Some(Wait::Resolve { view, right }), Reply::Log(r)) => resolved(st, view, right, r),
        (Some(Wait::Files(buf)), Reply::TreeDiff(r)) => match r {
            Ok(files) => file_list(st, buf, files.to_vec()),
            Err(e) => fail(st, buf, e.to_string()),
        },
        (Some(Wait::Files(buf)), Reply::CommitDetails(r)) => match r {
            Ok(d) => file_list(st, buf, d.files.clone()),
            Err(e) => fail(st, buf, e.to_string()),
        },
        (Some(Wait::File { view, section, generation }), Reply::FileDiff(r)) => {
            file_diff(st, view, section, generation, r.as_ref().map(|d| d.as_ref()).map_err(|e| e.to_string()))
        }
        (Some(Wait::Gutter { view, section }), Reply::Gutter(Err(e))) => {
            if let Some(s) = st.git.diff.views.get_mut(&view).and_then(|v| v.sections.get_mut(section)) {
                s.note = Some(e.to_string());
                s.load = Load::Done;
            }
            stale(st, view);
        }
        (Some(Wait::Op { view, done }), Reply::OpDone(r)) => {
            match r {
                Ok(_) => st.ed.info(done),
                Err(e) => st.ed.error(e.to_string()),
            }
            if let Some(repo) = st.git.diff.views.get(&view).and_then(|v| v.repo.clone()) {
                refresh_repo(st, &repo);
            }
        }
        (Some(Wait::PickRepo(choice)), Reply::Discovered { repo, .. }) => match repo {
            Some(info) => list_for_picker(st, choice, info.workdir.clone()),
            None => st.ed.error("not in a git repository"),
        },
        (Some(Wait::Commits(repo)), Reply::Log(r)) => commit_picker(st, repo, r),
        (Some(Wait::Refs(repo)), Reply::Refs(r)) => ref_picker(st, repo, r),
        (None, Reply::Changed(c)) if c.index || c.head || c.refs => {
            if let Some(repo) = ev.repo.clone() {
                refresh_repo(st, &repo);
            }
        }
        // someone else's write (the panel, the gutter): the index or the files may have moved
        (None, Reply::OpDone(Ok(_))) => {
            if let Some(repo) = ev.repo.clone() {
                refresh_repo(st, &repo);
            }
        }
        _ => {}
    }
}

/// Starts watching a repository (its stamps are checked while the window has the focus).
fn watch(st: &mut State, repo: &Path) {
    st.git.watch(repo);
}

/// The repositories the diff tabs show (they stay watched while a tab is open).
pub fn repos(st: &State) -> Vec<PathBuf> {
    st.git.diff.views.values().filter_map(|v| v.repo.clone()).collect()
}

fn refresh_repo(st: &mut State, repo: &Path) {
    let bufs: Vec<u64> =
        st.git.diff.views.iter().filter(|(_, v)| v.repo.as_deref() == Some(repo)).map(|(&b, _)| b).collect();
    for b in bufs {
        refresh(st, b);
    }
}

fn stale(st: &mut State, buf: u64) {
    if let Some(v) = st.git.diff.views.get_mut(&buf) {
        v.stale = true;
    }
}

fn fail(st: &mut State, buf: u64, msg: String) {
    if let Some(v) = st.git.diff.views.get_mut(&buf) {
        v.message = Some(msg);
        v.stale = true;
    }
}

fn resolved(st: &mut State, buf: u64, right: bool, r: &shrew_git::Result<Arc<shrew_git::LogPage>>) {
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
    let rev = if right { &v.spec.right } else { &v.spec.left };
    let row = match r {
        Ok(page) => page.rows.first().map(|r| r.commit.clone()),
        Err(e) => {
            v.message = Some(format!("{}: {e}", rev.label()));
            return;
        }
    };
    let Some(c) = row else {
        v.message = Some(format!("{}: no such commit", rev.label()));
        return;
    };
    let side = match rev {
        Rev::Parent(_) => {
            v.summary.get_or_insert_with(|| c.summary.clone());
            c.parents.first().map_or(Rev::Empty, |p| Rev::Commit(*p))
        }
        _ => {
            if right && matches!(v.spec.left, Rev::Parent(_)) {
                v.summary = Some(c.summary.clone());
            }
            Rev::Commit(c.id)
        }
    };
    if right {
        v.right = Some(side);
    } else {
        v.left = Some(side);
    }
    if v.left.is_some() && v.right.is_some() {
        load(st, buf);
    }
}

/// The changed files of a multi-file view arrived: sections for them, the ones already loaded kept.
fn file_list(st: &mut State, buf: u64, files: Vec<shrew_git::FileChange>) {
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
    let mut old: HashMap<String, Section> = v.sections.drain(..).map(|s| (s.path.clone(), s)).collect();
    let lines: u32 = files.iter().map(|f| f.added + f.removed).sum();
    let open = files.len() <= OPEN_FILES && lines <= OPEN_LINES;
    for f in files {
        let mut s = old.remove(&f.path).unwrap_or_else(|| {
            let mut s = Section::new(f.path.clone());
            s.collapsed = !open;
            s
        });
        s.old_path = f.old_path;
        s.state = Some(f.state);
        s.binary = f.binary;
        if s.data.is_none() {
            s.stats = (f.added, f.removed);
        }
        if f.binary {
            s.note = Some("binary file".into());
            s.data = None;
            s.load = Load::Done;
        }
        v.sections.push(s);
    }
    v.message = v.sections.is_empty().then(|| "no changes".to_string());
    v.stale = true;
    let todo: Vec<usize> =
        (0..v.sections.len()).filter(|&i| !v.sections[i].collapsed && !v.sections[i].binary).collect();
    for i in todo {
        load_section(st, buf, i, true);
    }
}

fn file_diff(st: &mut State, buf: u64, i: usize, generation: u64, r: Result<&shrew_git::FileDiff, String>) {
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
    let empty_left = v.left == Some(Rev::Empty);
    let ignore_ws = v.ignore_ws;
    let Some(s) = v.sections.get_mut(i).filter(|s| s.generation == generation) else { return };
    v.stale = true;
    v.message = None;
    let d = match r {
        Ok(d) => d,
        Err(e) => {
            s.note = Some(e);
            s.load = Load::Done;
            return;
        }
    };
    if d.binary {
        s.binary = true;
        s.note = Some("binary file".into());
        s.data = None;
        s.load = Load::Done;
        return;
    }
    if d.old.is_none() && d.new.is_none() {
        s.note = Some("no such file in either version".into());
        s.load = Load::Done;
        return;
    }
    let new = d.new_text();
    let (old, hunks, words) = if empty_left {
        let n = new.len_lines() as u32;
        let hunks = if n > 0 { vec![Hunk { old: 0..0, new: 0..n }] } else { Vec::new() };
        (Text::default(), hunks, Vec::new())
    } else {
        (d.old_text(), d.hunks.clone(), d.words.clone())
    };
    s.set_data(Data {
        old: SideText::Bytes(old),
        new: SideText::Bytes(new),
        hunks,
        eol: d.eol,
        base: Base::Unchecked,
        version: None,
    });
    if ignore_ws {
        let input = ws_input(s);
        return spawn_ws(st, buf, i, input);
    }
    for (k, w) in words.into_iter().enumerate() {
        s.words.insert(k as u32, Arc::new(w));
    }
}

/// The inputs of a white-space-ignoring re-diff.
struct WsInput {
    old: Text,
    new: SideText,
    generation: u64,
}

fn ws_input(s: &Section) -> WsInput {
    let d = s.data.as_ref().expect("loaded");
    WsInput { old: d.old.to_text(), new: d.new.clone(), generation: s.generation }
}

/// A gutter diff of a live buffer arrived.
fn gutter(st: &mut State, buf: u64, i: usize, g: &shrew_git::Gutter) {
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
    let ignore_ws = v.ignore_ws;
    let Some(s) = v.sections.get_mut(i) else { return };
    let Some(live) = &s.live else { return };
    let Some(rope) = live.snapshots.iter().find(|(ver, _)| *ver == g.version).map(|(_, r)| r.clone()) else {
        return;
    };
    // an older answer than the one shown
    if s.data.as_ref().and_then(|d| d.version).is_some_and(|shown| shown > g.version) {
        return;
    }
    v.stale = true;
    v.message = None;
    let (old, hunks, base) = match g.file {
        FileTrack::Tracked => {
            let Some(b) = &g.base else { return };
            (b.text.clone(), g.hunks.clone(), Base::Blob(b.id))
        }
        FileTrack::Untracked => {
            let n = view::rope_lines(&rope) as u32;
            let hunks = if n > 0 { vec![Hunk { old: 0..0, new: 0..n }] } else { Vec::new() };
            (Text::default(), hunks, Base::Absent)
        }
        FileTrack::Conflicted => {
            s.note = Some("conflicted: resolve it first (the conflict markers are in the file)".into());
            s.load = Load::Done;
            return;
        }
        FileTrack::NotText => {
            s.note = Some("binary file".into());
            s.load = Load::Done;
            return;
        }
        FileTrack::TooLarge => {
            // too big to diff on every edit: the file on disk, once
            s.live = Some(Live { buffer: u64::MAX, sent: None, snapshots: Vec::new() });
            return load_section(st, buf, i, true);
        }
    };
    s.generation += 1;
    s.set_data(Data {
        old: SideText::Bytes(old),
        new: SideText::Rope(rope),
        hunks,
        eol: EolMode::Normalize,
        base,
        version: Some(g.version),
    });
    if ignore_ws {
        let input = ws_input(s);
        spawn_ws(st, buf, i, input);
    }
}

/// Re-diffs a section ignoring white space, on a thread of its own (a big file takes milliseconds).
fn spawn_ws(st: &State, buf: u64, section: usize, input: WsInput) {
    let jobs = st.git.diff.jobs.clone();
    let running = st.git.diff.running.clone();
    let wake = st.wake.clone();
    running.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let spawned = std::thread::Builder::new().name("shrew-diff".into()).spawn(move || {
        let new = input.new.to_text();
        let (hunks, words) = ignore_whitespace(&input.old, &new);
        jobs.lock().unwrap_or_else(|e| e.into_inner()).push(Job {
            view: buf,
            section,
            generation: input.generation,
            hunks,
            words,
        });
        running.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        wake();
    });
    if spawned.is_err() {
        st.git.diff.running.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// The changes between `old` and `new` when white space does not count (`git diff -w`): lines compare with every
/// space and tab removed, so the hunks keep the original line numbers. Word highlights come from the real texts.
pub fn ignore_whitespace(old: &Text, new: &Text) -> (Vec<Hunk>, Vec<(u32, WordDiff)>) {
    let squeeze = |t: &Text| {
        let mut out = Vec::with_capacity(t.bytes().len());
        for i in 0..t.len_lines() {
            out.extend(t.line_content(i).iter().filter(|b| !b.is_ascii_whitespace()));
            out.push(b'\n');
        }
        Text::new(out)
    };
    let hunks = shrew_git::diff::diff(&squeeze(old), &squeeze(new), EolMode::Exact);
    let words = hunks
        .iter()
        .enumerate()
        .map(|(k, h)| (k as u32, shrew_git::diff::word_diff(old, new, h)))
        .filter(|(_, w)| !w.is_empty())
        .collect();
    (hunks, words)
}

/// Picks up finished re-diffs and highlighting. True when something changed.
pub fn poll(st: &mut State) -> bool {
    let mut changed = false;
    let done: Vec<Job> = std::mem::take(&mut *st.git.diff.jobs.lock().unwrap_or_else(|e| e.into_inner()));
    for j in done {
        let Some(v) = st.git.diff.views.get_mut(&j.view) else { continue };
        let Some(s) = v.sections.get_mut(j.section).filter(|s| s.generation == j.generation) else { continue };
        let Some(d) = &mut s.data else { continue };
        d.hunks = j.hunks;
        s.stats = shrew_git::diff::stats(&d.hunks);
        s.words = j.words.into_iter().map(|(k, w)| (k, Arc::new(w))).collect();
        v.stale = true;
        changed = true;
    }
    #[cfg(feature = "syntax")]
    for v in st.git.diff.views.values_mut() {
        for s in &mut v.sections {
            for h in s.hl.iter_mut().flatten() {
                changed |= h.poll().is_some();
            }
        }
    }
    changed
}

/// Something is still loading (headless screenshots wait for it).
pub fn busy(st: &State) -> bool {
    let views = &st.git.diff;
    if !views.waiting.is_empty() || views.running.load(std::sync::atomic::Ordering::Acquire) > 0 {
        return true;
    }
    #[cfg(feature = "syntax")]
    {
        let syntax_on = st.ed.features.on(shrew_core::Feature::Syntax);
        if syntax_on
            && views.views.values().any(|v| v.sections.iter().any(|s| s.hl.iter().flatten().any(|h| !h.is_settled())))
        {
            return true;
        }
    }
    false
}

// ----- following the editor -----

/// After input: views whose tab closed go, the active one follows its live buffers and the editor's search.
pub fn after_input(st: &mut State) {
    let ids: Vec<u64> = st.ed.buffers.iter().filter(|b| b.doc.title.is_some()).map(|b| b.id).collect();
    let views = &mut st.git.diff;
    if views.views.len() > ids.len() || views.views.keys().any(|k| !ids.contains(k)) {
        views.views.retain(|k, _| ids.contains(k));
        views.waiting.retain(|_, w| match w {
            Wait::Discover(b) | Wait::Files(b) => ids.contains(b),
            Wait::Resolve { view, .. }
            | Wait::File { view, .. }
            | Wait::Gutter { view, .. }
            | Wait::Op { view, .. } => ids.contains(view),
            _ => true,
        });
    }
    let active = views.active(&st.ed).map(|v| v.buffer);
    let came_back = active.is_some() && active != views.active;
    views.active = active;
    let Some(buf) = active else { return };
    if came_back {
        adopt_buffers(st, buf);
        refresh(st, buf);
    } else {
        follow_buffers(st, buf);
    }
    input::follow_search(st, buf);
}

/// Live sections whose buffer changed send the new text; ones whose buffer closed fall back to the file.
fn follow_buffers(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get(&buf) else { return };
    let mut todo = Vec::new();
    for (i, s) in v.sections.iter().enumerate() {
        let Some(l) = &s.live else { continue };
        if l.buffer == u64::MAX {
            continue;
        }
        match st.ed.buffers.iter().find(|b| b.id == l.buffer) {
            Some(b) if l.sent != Some(b.doc.version) => todo.push(i),
            Some(_) => {}
            None => todo.push(i),
        }
    }
    for i in todo {
        load_section(st, buf, i, false);
    }
}

/// Files opened since the view loaded become live.
fn adopt_buffers(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
    for s in &mut v.sections {
        if s.live.as_ref().is_some_and(|l| l.buffer == u64::MAX) {
            s.live = None;
        }
    }
}

/// The window's focus: files may have changed while it was away.
pub fn focus(st: &mut State, focused: bool) {
    if focused && let Some(buf) = st.git.diff.active(&st.ed).map(|v| v.buffer) {
        refresh(st, buf);
    }
}

// ----- commands -----

pub fn command(st: &mut State, cmd: Command, arg: Option<String>) {
    let arg = arg.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());
    match cmd {
        Command::GitDiffFile | Command::GitDiffFileHead => {
            let Some(path) = st.ed.buf().doc.path.clone() else {
                return st.ed.error("no file to diff (open one first)");
            };
            let left = match arg.as_deref() {
                _ if cmd == Command::GitDiffFileHead => Rev::Head,
                None | Some(":0" | "index") => Rev::Index,
                Some("HEAD" | "@") => Rev::Head,
                Some(r) => Rev::Named(r.to_string()),
            };
            open(st, DiffSpec::file(path, left, Rev::Worktree));
        }
        Command::GitDiffAll => open(st, DiffSpec::unstaged()),
        Command::GitDiffStaged => open(st, DiffSpec::staged()),
        Command::GitDiffCommit => match arg {
            Some(rev) => open(st, DiffSpec::commit(Rev::Named(rev))),
            None => pick(st, Choice::GitCommit),
        },
        Command::GitDiffCompare => {
            let named = |s: &str| match s {
                "HEAD" | "@" => Rev::Head,
                "index" | ":0" => Rev::Index,
                "worktree" | "." => Rev::Worktree,
                s => Rev::Named(s.to_string()),
            };
            match arg.as_deref().map(|a| a.split_once("..").unwrap_or_else(|| a.split_once(' ').unwrap_or((a, "")))) {
                Some((a, "")) => open(st, DiffSpec::tree(named(a), Rev::Worktree)),
                Some((a, b)) => open(st, DiffSpec::tree(named(a.trim()), named(b.trim()))),
                None => {
                    st.git.diff.compare_first = None;
                    pick(st, Choice::GitRef);
                }
            }
        }
        Command::GitDiffToggleLayout => {
            let d = &mut st.git.diff.defaults;
            let next = match st.git.diff.views.get(&st.ed.buf().id) {
                Some(v) => other_layout(v.opts.layout),
                None => other_layout(d.layout),
            };
            set_layout(st, next);
        }
        _ => {}
    }
}

fn other_layout(l: Layout) -> Layout {
    match l {
        Layout::Split => Layout::Unified,
        Layout::Unified => Layout::Split,
    }
}

/// The active view's layout, which is also the default from now on (saved to `[git] diff_layout`).
pub fn set_layout(st: &mut State, layout: Layout) {
    if let Some(v) = st.git.diff.views.get_mut(&st.ed.buf().id) {
        v.opts.layout = layout;
        v.stale = true;
    }
    st.git.diff.defaults.layout = layout;
    crate::settings::save_with(st, |p| shrew_config::persist_value(p, "git", "diff_layout", layout.name()));
    st.ed.info(format!("diff layout: {}", layout.name()));
}

// ----- pickers -----

/// Opens the commit or ref list for the repository of the active file (or project).
fn pick(st: &mut State, choice: Choice) {
    let at = context_dir(st);
    let t = super::send(st, Request::Discover { path: at });
    st.git.diff.waiting.insert(t, Wait::PickRepo(choice));
}

fn list_for_picker(st: &mut State, choice: Choice, repo: PathBuf) {
    if choice == Choice::GitCommit {
        let query = LogQuery { limit: PICK_COMMITS, ..LogQuery::default() };
        let t = super::send(st, Request::Log { repo: repo.clone(), query });
        st.git.diff.waiting.insert(t, Wait::Commits(repo));
    } else {
        let t = super::send(st, Request::Refs { repo: repo.clone() });
        st.git.diff.waiting.insert(t, Wait::Refs(repo));
    }
}

fn item(label: String, detail: String, value: String) -> PickerItem {
    PickerItem { label, detail, data: PickerData::Value(value) }
}

fn commit_picker(st: &mut State, repo: PathBuf, r: &shrew_git::Result<Arc<shrew_git::LogPage>>) {
    let page = match r {
        Ok(p) => p,
        Err(e) => return st.ed.error(e.to_string()),
    };
    if page.rows.is_empty() {
        return st.ed.error("no commits yet");
    }
    let now = now_secs();
    let items = page
        .rows
        .iter()
        .map(|r| {
            let c = &r.commit;
            let refs: Vec<&str> = r.refs.iter().map(|x| x.name.as_str()).collect();
            let refs = if refs.is_empty() { String::new() } else { format!(" ({})", refs.join(", ")) };
            item(
                format!("{}  {}{refs}", c.id.short(), c.summary),
                format!("{} · {}", c.author.name, ago(now - c.author.time)),
                format!("{}\t{}", c.id, repo.display()),
            )
        })
        .collect();
    st.ed.open_choice(Choice::GitCommit, items, 0);
}

fn ref_picker(st: &mut State, repo: PathBuf, r: &shrew_git::Result<Arc<shrew_git::Refs>>) {
    let refs = match r {
        Ok(r) => r,
        Err(e) => return st.ed.error(e.to_string()),
    };
    let tag = |name: &str| format!("{name}\t{}", repo.display());
    let mut items = vec![
        item("HEAD".into(), "the current commit".into(), tag("HEAD")),
        item("working tree".into(), "the files as they are".into(), tag(".")),
        item("index".into(), "what is staged".into(), tag(":0")),
    ];
    for b in &refs.branches {
        let head = if b.is_head { " (current)" } else { "" };
        items.push(item(format!("{}{head}", b.name), format!("branch · {}", b.summary), tag(&b.name)));
    }
    for b in &refs.remote_branches {
        items.push(item(b.name.clone(), format!("remote branch · {}", b.summary), tag(&b.name)));
    }
    for t in &refs.tags {
        items.push(item(t.name.clone(), format!("tag · {}", t.target.short()), tag(&t.name)));
    }
    match &st.git.diff.compare_first {
        Some(first) => st.ed.info(format!("compare {first} with…")),
        None => st.ed.info("compare: pick the old side first"),
    }
    st.ed.open_choice(Choice::GitRef, items, 0);
}

/// A row picked in a git list (`value`), or a request to open one (`None`).
pub fn choice(st: &mut State, choice: Choice, value: Option<String>) {
    let Some(value) = value else { return pick(st, choice) };
    let (rev, repo) = value.split_once('\t').map_or((value.as_str(), None), |(a, b)| (a, Some(PathBuf::from(b))));
    let rev = rev.to_string();
    match choice {
        Choice::GitCommit => {
            let r = Oid::from_hex(&rev).map_or(Rev::Named(rev), Rev::Commit);
            let mut spec = DiffSpec::commit(r);
            spec.repo = repo;
            open(st, spec);
        }
        Choice::GitRef => match st.git.diff.compare_first.take() {
            None => {
                st.git.diff.compare_first = Some(rev);
                if let Some(repo) = repo {
                    list_for_picker(st, Choice::GitRef, repo);
                }
            }
            Some(first) => {
                let named = |s: &str| match s {
                    "HEAD" => Rev::Head,
                    ":0" => Rev::Index,
                    "." => Rev::Worktree,
                    s => Rev::Named(s.to_string()),
                };
                let mut spec = DiffSpec::tree(named(&first), named(&rev));
                spec.repo = repo;
                open(st, spec);
            }
        },
        _ => {}
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// "3 days ago".
fn ago(secs: i64) -> String {
    super::blame_inline::ago(secs)
}

// ----- actions -----

/// What to do with the selected changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Stage,
    Unstage,
    /// Stage when the view shows unstaged changes, unstage when it shows staged ones (fugitive's `-`).
    Toggle,
    Revert,
}

/// Stages, unstages or reverts the selected lines, or the change under the cursor.
pub fn act(st: &mut State, buf: u64, action: Action) {
    let Some(v) = st.git.diff.views.get(&buf) else { return };
    let can = v.actions();
    let action = match action {
        Action::Toggle if can.stage => Action::Stage,
        Action::Toggle if can.unstage => Action::Unstage,
        a => a,
    };
    let allowed = match action {
        Action::Stage => can.stage,
        Action::Unstage => can.unstage,
        Action::Revert => can.revert,
        Action::Toggle => false,
    };
    if !allowed {
        return st.ed.error(match action {
            Action::Stage | Action::Toggle => "stage from the unstaged diff (index ↔ working tree)",
            Action::Unstage => "unstage from the staged diff (HEAD ↔ index)",
            Action::Revert => "revert works on unstaged changes (index ↔ working tree) or on an open file",
        });
    }
    let Some(rows) = v.target_rows() else { return st.ed.error("no change here") };
    let picks = v.rows.selected_lines(rows);
    if picks.is_empty() {
        return st.ed.error("no change here");
    }
    let Some(repo) = v.repo.clone() else { return };
    let lines: u32 = picks.iter().map(|p| p.old.iter().chain(&p.new).map(|r| r.end - r.start).sum::<u32>()).sum();
    let what = if lines == 1 { "1 line".to_string() } else { format!("{lines} lines") };
    let mut ops: Vec<(Op, &'static str)> = Vec::new();
    let mut edits: Vec<(usize, LineSel)> = Vec::new();
    for model::Picked { file, old, new } in picks {
        let s = &v.sections[file as usize];
        let Some(d) = &s.data else { continue };
        let sel = LineSel { old, new };
        match action {
            Action::Stage => {
                let text = s.live.as_ref().filter(|l| l.buffer != u64::MAX).map(|_| d.new.to_text());
                ops.push((Op::StageLines { path: s.path.clone(), text, sel, base: d.base }, "staged"));
            }
            Action::Unstage => {
                ops.push((Op::UnstageLines { path: s.path.clone(), sel, base: Base::Unchecked }, "unstaged"));
            }
            Action::Revert if s.live.as_ref().is_some_and(|l| l.buffer != u64::MAX) => edits.push((file as usize, sel)),
            Action::Revert => {
                ops.push((Op::DiscardLines { path: s.path.clone(), sel, base: d.base }, "discarded"));
            }
            Action::Toggle => {}
        }
    }
    // reverting from the file on disk cannot be undone: X twice
    let v = st.git.diff.views.get_mut(&buf).expect("checked above");
    if action == Action::Revert && !ops.is_empty() && !v.armed {
        v.armed = true;
        return st.ed.error(format!("press X again to discard {what} from the file on disk (no undo)"));
    }
    v.armed = false;
    v.anchor = None;
    for (file, sel) in edits {
        revert_in_buffer(st, buf, file, &sel);
    }
    for (op, done) in ops {
        let done: &'static str = match (done, lines) {
            ("staged", 1) => "staged 1 line",
            ("staged", _) => "staged the selected lines",
            ("unstaged", 1) => "unstaged 1 line",
            ("unstaged", _) => "unstaged the selected lines",
            _ => "discarded",
        };
        let t = super::send(st, Request::Op { repo: repo.clone(), op });
        st.git.diff.waiting.insert(t, Wait::Op { view: buf, done });
    }
}

/// Puts the old text of the selected lines back into the open buffer: one undo step.
fn revert_in_buffer(st: &mut State, buf: u64, file: usize, sel: &LineSel) {
    let Some(v) = st.git.diff.views.get(&buf) else { return };
    let s = &v.sections[file];
    let (Some(d), Some(l)) = (&s.data, &s.live) else { return };
    let Some(bi) = st.ed.buffers.iter().position(|b| b.id == l.buffer) else { return };
    if d.version != Some(st.ed.buffers[bi].doc.version) {
        return st.ed.error("the diff is catching up with your edits: try again");
    }
    let (old, new) = (d.old.to_text(), d.new.to_text());
    let reverted = Text::new(shrew_git::patch::revert_selection(&old, &new, &d.hunks, sel, d.eol));
    // only the lines between the first and the last touched change differ
    let lines = |r: &Vec<std::ops::Range<u32>>| r.iter().map(|x| (x.start, x.end)).collect::<Vec<_>>();
    let mut touched: Vec<(u32, u32)> = Vec::new();
    for h in &d.hunks {
        let hit = lines(&sel.new).iter().any(|&(a, z)| a < h.new.end.max(h.new.start + 1) && z > h.new.start)
            || lines(&sel.old).iter().any(|&(a, z)| a < h.old.end && z > h.old.start);
        if hit {
            touched.push((h.new.start, h.new.end));
        }
    }
    let (Some(a), Some(z)) = (touched.iter().map(|t| t.0).min(), touched.iter().map(|t| t.1).max()) else { return };
    let n_new = new.len_lines() as u32;
    let n_rev = reverted.len_lines() as u32;
    let rz = n_rev.saturating_sub(n_new - z);
    let replacement = String::from_utf8_lossy(reverted.lines_bytes(a as usize..rz as usize)).into_owned();
    let b = &mut st.ed.buffers[bi];
    let rope = &b.doc.rope;
    let at =
        |line: u32| if line as usize >= rope.len_lines() { rope.len_chars() } else { rope.line_to_char(line as usize) };
    let (ca, cz) = (at(a), at(z));
    let cs = shrew_core::ChangeSet::single(shrew_core::Change::replace(ca, cz, replacement));
    b.apply(cs, None);
    b.commit();
    st.ed.info("reverted (u in the file undoes it)");
}

/// Opens the working-tree file at the line under the cursor.
pub fn open_file_at_cursor(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get(&buf) else { return };
    let Some(r) = v.rows.rows.get(v.cursor).copied() else { return };
    let (Some(repo), Some(s)) = (&v.repo, v.sections.get(r.file as usize)) else { return };
    let path = repo.join(&s.path);
    // the new side's line; for a removed line, where it was
    let line = match (r.kind, r.new_line(), r.hunk()) {
        (model::RowKind::File | model::RowKind::Note, ..) => None,
        (_, Some(l), _) => Some(l),
        (_, None, Some(h)) => s.data.as_ref().and_then(|d| d.hunks.get(h as usize)).map(|h| h.new.start),
        _ => None,
    };
    st.open_at(&path, Some(line.map_or(1, |l| l as usize + 1)), None);
}

/// Toggles white space being ignored in the active view (and for new views).
pub fn toggle_whitespace(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
    v.ignore_ws = !v.ignore_ws;
    let on = v.ignore_ws;
    st.git.diff.defaults.ignore_ws = on;
    crate::settings::save_with(st, |p| shrew_config::persist_value(p, "git", "diff_ignore_whitespace", on));
    st.ed.info(if on { "white space ignored" } else { "white space shown" });
    reload_all(st, buf);
}

/// Loads every loaded section of a view again (after a setting changed how they are diffed).
fn reload_all(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
    let todo: Vec<usize> = (0..v.sections.len()).filter(|&i| v.sections[i].load != Load::Idle).collect();
    for s in &mut v.sections {
        s.words.clear();
    }
    for i in todo {
        load_section(st, buf, i, true);
    }
}

/// A collapsed file was opened: load it if it never was.
pub fn ensure_loaded(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get(&buf) else { return };
    let todo: Vec<usize> = (0..v.sections.len())
        .filter(|&i| !v.sections[i].collapsed && v.sections[i].load == Load::Idle && !v.sections[i].binary)
        .collect();
    for i in todo {
        load_section(st, buf, i, true);
    }
}

/// Copies the cursor's (or the selection's) lines of the focused side.
pub fn yank(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get(&buf) else { return };
    let range = v.anchor.map_or(v.cursor..v.cursor + 1, |a| a.min(v.cursor)..a.max(v.cursor) + 1);
    let n = range.len();
    let text: Vec<String> = range.filter_map(|r| v.line(r, v.side).map(|l| l.into_owned())).collect();
    if text.is_empty() {
        return st.ed.error("nothing to copy on this side");
    }
    let mut s = text.join("\n");
    s.push('\n');
    st.ed.clipboard.set(s);
    st.ed.info(if n == 1 { "copied 1 line".to_string() } else { format!("copied {} lines", text.len()) });
    if let Some(v) = st.git.diff.views.get_mut(&buf) {
        v.anchor = None;
    }
}

/// Re-reads everything (`r`).
pub fn reload(st: &mut State, buf: u64) {
    if let Some(v) = st.git.diff.views.get_mut(&buf) {
        v.sections.iter_mut().for_each(|s| {
            if s.live.as_ref().is_some_and(|l| l.buffer == u64::MAX) {
                s.live = None;
            }
        });
    }
    let static_view = st.git.diff.views.get(&buf).is_some_and(|v| !v.spec.left.moves() && !v.spec.right.moves());
    if static_view {
        return start(st, buf);
    }
    refresh(st, buf);
}

/// Words changed in hunk `k` of a section, computed now if needed (only hunks on screen ask).
pub fn words(s: &mut Section, k: u32) -> Option<Arc<WordDiff>> {
    if let Some(w) = s.words.get(&k) {
        return Some(w.clone());
    }
    let d = s.data.as_ref()?;
    let h = d.hunks.get(k as usize)?;
    if h.old.is_empty() || h.new.is_empty() {
        return None;
    }
    // just the hunk's lines, so a live buffer is never copied whole
    let (old, new) = (d.old.lines_text(h.old.clone()), d.new.lines_text(h.new.clone()));
    let local = Hunk { old: 0..h.removed(), new: 0..h.added() };
    let mut w = shrew_git::diff::word_diff(&old, &new, &local);
    w.old.iter_mut().for_each(|sp| sp.line += h.old.start);
    w.new.iter_mut().for_each(|sp| sp.line += h.new.start);
    let w = Arc::new(w);
    s.words.insert(k, w.clone());
    Some(w)
}
