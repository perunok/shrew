//! Git integration (cargo feature `git`): the app's side of [`shrew_git`]. [`Git`] owns the one
//! [`GitService`] (made on first use, so no thread starts before a git feature is used), the repository the
//! active file or project belongs to and its latest status, and routes the service's events to the parts.
//!
//! - [`panel`]: the Source Control panel, the sidebar's Git tab (feature `git_panel`, Ctrl+Shift+G).
//! - [`pickers`]: branches, stashes and changed files in the picker.
//! - [`ops`]: running writes (stage, commit, fetch, pull, push...) with a progress toast and cancel.
//! - [`log`]: the log and file history tab: commit graph, refs, paging, search, commit details.
//! - [`blame_view`]: blame as a column left of the text, per line group.
//! - [`conflict`]: merge conflict regions and their actions in the text.
//! - [`ask`] / [`textbox`]: the git prompt on the command line and the text fields.
//!
//! Costs: outside a repository, or with the features off, nothing runs. In one, the service's threads sleep until
//! asked; the status is re-read when the window gets the focus back, after a write and after a save, and the
//! service looks at a few file stamps every 3 s while the window is focused (see [`shrew_git::service`]).

pub mod ask;
pub mod blame_view;
pub mod conflict;
pub mod log;
pub mod ops;
pub mod panel;
pub mod pickers;
pub mod textbox;

use crate::colors::Colors;
use crate::draw::Ui;
use crate::layout::{Layout, Scroll};
use crate::state::{State, Waker};
use ask::{Answer, Ask, Then};
use panel::{Action, Focus};
use shrew_core::features::Feature;
use shrew_core::picker::{Choice, PickerKind};
use shrew_core::{Command, Editor, Key, KeyPress, Mode};
use shrew_git::{
    CommitOptions, Event, FetchOptions, GitService, Op, OpOutput, PullMode, PushOptions, Refs, Reply, RepoInfo,
    Request, Status, StatusOptions, Ticket,
};
use shrew_render::Scene;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a request was for, so its answer goes to the right part.
#[derive(Clone, Debug)]
enum Purpose {
    Status,
    /// Refs for a picker (or just to keep).
    Refs(Option<Choice>),
    /// A page of the log or file history shown in the view of this buffer.
    Log(u64),
    /// Commit details for the view of this buffer.
    Details(u64),
    /// The last commit's message, to amend it.
    AmendMessage,
    /// Blame of this buffer.
    Blame(u64),
    Op(Op),
}

pub struct Git {
    service: Option<GitService>,
    wake: Waker,
    /// Folders looked up, and their repository (`None`: not in one).
    known: HashMap<PathBuf, Option<RepoInfo>>,
    asked: HashSet<PathBuf>,
    /// The folder `repo` was looked up for.
    target: Option<PathBuf>,
    /// The repository the panel, pickers and commands work on.
    pub repo: Option<RepoInfo>,
    pub status: Option<Arc<Status>>,
    pub status_error: Option<String>,
    status_pending: bool,
    pub refs: Option<Arc<Refs>>,
    tickets: HashMap<Ticket, Purpose>,
    /// Commands waiting for the repository to be found.
    waiting: Vec<Command>,
    /// The version of each open file last seen saved, to re-read the status after a save.
    saved: HashMap<u64, u64>,
    /// The changed-files list opens when the status it waits for comes.
    changes_pending: bool,
    // ----- git: panel -----
    pub panel: panel::Panel,
    pub ask: Option<Ask>,
    pub ops: ops::Ops,
    pub views: log::Views,
    pub blame: blame_view::Blames,
    pub conflicts: conflict::Conflicts,
}

impl Git {
    pub fn new(wake: Waker) -> Git {
        Git {
            service: None,
            wake,
            known: HashMap::new(),
            asked: HashSet::new(),
            target: None,
            repo: None,
            status: None,
            status_error: None,
            status_pending: false,
            refs: None,
            tickets: HashMap::new(),
            waiting: Vec::new(),
            saved: HashMap::new(),
            changes_pending: false,
            panel: Default::default(),
            ask: None,
            ops: Default::default(),
            views: Default::default(),
            blame: Default::default(),
            conflicts: Default::default(),
        }
    }

    /// The service, started on first use.
    fn service(&mut self) -> &GitService {
        let wake = self.wake.clone();
        self.service.get_or_insert_with(|| GitService::new(Arc::new(move || wake())))
    }

    fn send(&mut self, req: Request, mut purpose: Purpose) -> Ticket {
        // the service answers only the newest of a repository's pending status or refs requests (and of a file's
        // blame): the older tickets are forgotten, a list waiting for refs carries over
        let same = |p: &Purpose| match (p, &purpose) {
            (Purpose::Status, Purpose::Status) | (Purpose::Refs(_), Purpose::Refs(_)) => true,
            (Purpose::Blame(a), Purpose::Blame(b)) => a == b,
            _ => false,
        };
        let older: Vec<Ticket> = self.tickets.iter().filter(|(_, p)| same(p)).map(|(t, _)| *t).collect();
        for t in older {
            if let Some(Purpose::Refs(Some(c))) = self.tickets.remove(&t)
                && let Purpose::Refs(none @ None) = &mut purpose
            {
                *none = Some(c);
            }
        }
        let t = self.service().send(req);
        self.tickets.insert(t, purpose);
        t
    }

    pub fn workdir(&self) -> Option<&Path> {
        self.repo.as_ref().map(|r| r.workdir.as_path())
    }

    /// A file's repository path, when it is in the current repository.
    pub fn rel_path(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(self.workdir()?).ok()?;
        let s = rel.to_str()?.replace('\\', "/");
        (!s.is_empty()).then_some(s)
    }

    /// Background work the picture waits for (headless screenshots).
    pub fn busy(&self) -> bool {
        !self.tickets.is_empty() || !self.asked.is_empty() || self.status_pending
    }

    fn cancel_tickets(&mut self, keep: impl Fn(&Purpose) -> bool) {
        let drop: Vec<Ticket> = self.tickets.iter().filter(|(_, p)| !keep(p)).map(|(t, _)| *t).collect();
        for t in drop {
            self.tickets.remove(&t);
            if let Some(s) = &self.service {
                s.cancel(t);
            }
        }
    }
}

/// True when the git panel feature is on.
pub fn on(ed: &Editor) -> bool {
    ed.features.on(Feature::GitPanel)
}

/// The keys go to the panel or the git prompt (the text draws as unfocused).
pub fn has_focus(st: &State) -> bool {
    st.git.ask.is_some() || (st.git.panel.focused && panel::draw::shown(st))
}

/// `]x` / `[x` jump between merge conflicts in vim's normal mode, unless you mapped them yourself.
pub fn default_mappings(ed: &mut Editor) {
    use shrew_core::{MapMode, Rhs};
    for (lhs, cmd) in [("]x", Command::GitNextConflict), ("[x", Command::GitPrevConflict)] {
        let keys = KeyPress::parse_seq(lhs);
        if ed.keymap.get(MapMode::Normal, &keys).is_none() {
            ed.keymap.map(MapMode::Normal, keys, Rhs::Command(cmd));
        }
    }
}

// ----- repository -----

/// The folder the panel and commands work for: the active file's, else the current repository's (a log tab),
/// else the project's or the working directory.
fn target_dir(st: &State) -> PathBuf {
    let b = st.ed.buf();
    if let Some(dir) = b.doc.path.as_deref().and_then(Path::parent) {
        return dir.to_path_buf();
    }
    if let Some(v) = st.git.views.get(b.id) {
        return v.repo.clone();
    }
    if let Some(r) = &st.git.repo {
        return r.workdir.clone();
    }
    st.ed.project_root().unwrap_or_else(|| st.ed.cwd.clone())
}

/// Looks the target folder up (once per folder) and switches the repository when it changed.
fn sync_repo(st: &mut State) {
    let dir = target_dir(st);
    if st.git.target.as_ref() == Some(&dir) {
        return;
    }
    st.git.target = Some(dir.clone());
    match st.git.known.get(&dir).cloned() {
        Some(repo) => set_repo(st, repo),
        None => {
            if st.git.asked.insert(dir.clone()) {
                st.git.service().send(Request::Discover { path: dir });
            }
        }
    }
}

fn set_repo(st: &mut State, repo: Option<RepoInfo>) {
    if st.git.repo == repo {
        return;
    }
    let old = std::mem::replace(&mut st.git.repo, repo);
    st.git.status = None;
    st.git.status_error = None;
    st.git.refs = None;
    st.git.panel.set_status(&Status::default());
    if let Some(old) = old
        && let Some(s) = &st.git.service
        && !st.ed.buffers.iter().any(|b| b.doc.path.as_ref().is_some_and(|p| p.starts_with(&old.workdir)))
    {
        s.unwatch(&old.workdir);
    }
    if st.git.repo.is_some() {
        request_status(st);
        request_refs(st);
    }
}

fn request_status(st: &mut State) {
    let Some(repo) = st.git.workdir().map(Path::to_path_buf) else { return };
    st.git.status_pending = true;
    st.git.send(Request::Status { repo, options: StatusOptions::default() }, Purpose::Status);
}

/// The repository's workdir for `cmd`, or `None` after saying why (or while it is being looked up: the command
/// runs once it is found).
fn repo_for(st: &mut State, cmd: Command) -> Option<PathBuf> {
    sync_repo(st);
    if let Some(r) = &st.git.repo {
        return Some(r.workdir.clone());
    }
    let dir = st.git.target.clone()?;
    match st.git.known.get(&dir) {
        Some(None) => st.ed.error(format!("not a git repository: {}", dir.display())),
        _ => {
            if !st.git.waiting.contains(&cmd) {
                st.git.waiting.push(cmd);
            }
        }
    }
    None
}

fn discovered(st: &mut State, path: PathBuf, repo: Option<RepoInfo>) {
    st.git.asked.remove(&path);
    st.git.known.insert(path.clone(), repo.clone());
    if st.git.target.as_ref() != Some(&path) {
        return;
    }
    set_repo(st, repo);
    let waiting = std::mem::take(&mut st.git.waiting);
    if st.git.repo.is_none() {
        if !waiting.is_empty() {
            st.ed.error(format!("not a git repository: {}", path.display()));
        }
        return;
    }
    for cmd in waiting {
        command(st, cmd);
    }
}

// ----- events -----

/// Takes what the service finished. True when something may look different.
pub fn poll(st: &mut State) -> bool {
    let Some(s) = &st.git.service else { return false };
    let events = s.poll();
    if events.is_empty() {
        return false;
    }
    for ev in events {
        event(st, ev);
    }
    true
}

fn event(st: &mut State, ev: Event) {
    let purpose = ev.ticket.and_then(|t| st.git.tickets.get(&t).cloned());
    let last = !matches!(ev.reply, Reply::Blame(_) | Reply::Progress(_));
    if last && let Some(t) = ev.ticket {
        st.git.tickets.remove(&t);
    }
    let current = ev.repo.is_some() && ev.repo.as_deref() == st.git.workdir();
    match ev.reply {
        Reply::Discovered { path, repo } => discovered(st, path, repo),
        Reply::Status(r) if current => {
            st.git.status_pending = false;
            match r {
                Ok(s) => {
                    st.git.panel.set_status(&s);
                    st.git.status = Some(s);
                    st.git.status_error = None;
                    pickers::status_changed(st);
                }
                Err(e) => st.git.status_error = Some(e.message),
            }
        }
        Reply::Changed(c) if current => {
            if c.head || c.refs || c.config {
                st.git.refs = None;
                if panel::draw::shown(st) {
                    request_refs(st);
                }
            }
            if c.head || c.refs {
                log::repo_moved(st);
            }
            if c.head {
                blame_view::head_moved(st);
            }
        }
        Reply::Refs(r) if current => match r {
            Ok(refs) => {
                st.git.refs = Some(refs);
                if let Some(Purpose::Refs(Some(choice))) = purpose {
                    pickers::fill(st, choice);
                }
            }
            Err(e) => st.ed.error(format!("git: {}", e.message)),
        },
        Reply::Log(r) => {
            if let Some(Purpose::Log(id)) = purpose {
                log::page(st, id, r.map(|p| log::Page::from_log(&p)));
            }
        }
        Reply::FileHistory(r) => {
            if let Some(Purpose::Log(id)) = purpose {
                log::page(st, id, r.map(|p| log::Page::from_history(&p)));
            }
        }
        Reply::CommitDetails(r) => match (purpose, r) {
            (Some(Purpose::Details(id)), r) => log::details(st, id, r),
            (Some(Purpose::AmendMessage), Ok(d)) => {
                st.git.panel.message.set(d.message.trim_end());
                st.git.panel.focus = Focus::Message;
                st.ed.info("the last commit's message: edit it, then Ctrl+Shift+Enter amends");
            }
            (_, Err(e)) => st.ed.error(format!("git: {}", e.message)),
            _ => {}
        },
        Reply::Blame(hunks) => {
            if let Some(Purpose::Blame(id)) = purpose {
                st.git.blame.add(id, hunks);
            }
        }
        Reply::BlameDone(r) => {
            if let Some(Purpose::Blame(id)) = purpose {
                st.git.blame.done(id, r.map_err(|e| e.message));
            }
        }
        Reply::Progress(line) => {
            if let Some(t) = ev.ticket {
                st.git.ops.progress(t, line);
            }
        }
        Reply::OpDone(r) => {
            if let (Some(t), Some(Purpose::Op(op))) = (ev.ticket, purpose) {
                // a watched repository's status follows every op
                st.git.status_pending |= current && st.git.status.is_some();
                op_done(st, t, op, r);
            }
        }
        _ => {}
    }
}

// ----- writes -----

fn send_op(st: &mut State, repo: PathBuf, op: Op) {
    let label = ops::label(&op);
    let network = op.is_network();
    let t = st.git.send(Request::Op { repo, op: op.clone() }, Purpose::Op(op));
    st.git.ops.started(t, label, network);
}

fn op_done(st: &mut State, ticket: Ticket, op: Op, r: shrew_git::Result<OpOutput>) {
    st.git.ops.finished(ticket);
    match r {
        Ok(out) => {
            let msg = match (&op, out) {
                (Op::Commit(o), OpOutput::Commit(id)) => {
                    st.git.panel.message.clear();
                    let subject = o.message.lines().next().unwrap_or("").to_string();
                    format!("{} {}: {subject}", if o.amend { "amended" } else { "committed" }, id.short())
                }
                (_, OpOutput::Text(t)) => {
                    st.git.ops.last_output = t.clone();
                    ops::summary(&op, &t)
                }
                (_, OpOutput::File(_)) => "discarded".into(),
                _ => ops::done_message(&op),
            };
            if !msg.is_empty() {
                st.ed.info(msg);
            }
        }
        Err(e) if e.kind == shrew_git::ErrorKind::Cancelled => st.ed.info(format!("{} cancelled", ops::label(&op))),
        Err(e) => {
            if let Op::DeleteBranch { name, force: false } = &op
                && e.message.contains("not fully merged")
            {
                st.git.ask = Some(Ask::confirm(
                    format!("{name} is not merged anywhere: delete it anyway?"),
                    Then::DeleteBranch { name: name.clone(), force: true },
                ));
                return;
            }
            st.git.ops.failed(&op, &e.message);
            st.ed.error(format!("git {}: {}", ops::verb(&op), e.message.lines().next().unwrap_or("failed")));
        }
    }
}

/// Answers the git prompt.
fn answer(st: &mut State, then: Then, text: String, alt: bool) {
    let cmd = Command::GitRefresh;
    if matches!(then, Then::LogAuthor(_) | Then::LogPath(_)) {
        return log::filter(st, then, text);
    }
    let Some(repo) = repo_for(st, cmd) else { return };
    let op = match then {
        Then::Op(op) => op,
        Then::DeleteBranch { name, force } => Op::DeleteBranch { name, force },
        Then::DropStash(i) => Op::StashDrop(i),
        Then::NewBranch { start } => Op::CreateBranch { name: text, start, switch: true },
        Then::StashPush => Op::StashPush(shrew_git::StashOptions {
            message: (!text.is_empty()).then_some(text),
            include_untracked: alt,
            ..Default::default()
        }),
        Then::LogAuthor(_) | Then::LogPath(_) => return,
    };
    send_op(st, repo, op);
}

// ----- commands -----

/// Runs a `git_*` command ([`shrew_core::Event::App`]).
pub fn command(st: &mut State, cmd: Command) {
    use Command::*;
    match cmd {
        GitPanel => return toggle_panel(st),
        GitCancel => return cancel(st),
        GitOutput => return ops::show_output(st),
        GitConflictCompare => return conflict::compare(st),
        _ => {}
    }
    let Some(repo) = repo_for(st, cmd) else { return };
    match cmd {
        GitRefresh => {
            if let Some(s) = &st.git.service {
                s.refresh(&repo);
            }
            request_status(st);
            st.git.refs = None;
        }
        GitCommit | GitAmend => commit(st, repo, cmd == GitAmend),
        GitStage | GitUnstage | GitDiscard | GitOpenChange | GitOpenFile => file_command(st, repo, cmd),
        GitStageAll => send_op(st, repo, Op::StageAll),
        GitUnstageAll => send_op(st, repo, Op::UnstageAll),
        GitBranches => pickers::open(st, repo, Choice::GitBranches),
        GitStashes => pickers::open(st, repo, Choice::GitStashes),
        GitChangedFiles => pickers::open(st, repo, Choice::GitChanges),
        GitNewBranch => st.git.ask = Some(Ask::text("New branch:", "", Then::NewBranch { start: None })),
        GitStash => st.git.ask = Some(Ask::text("Stash message:", "", Then::StashPush)),
        GitFetch => send_op(st, repo, Op::Fetch(FetchOptions::default())),
        GitPull => send_op(st, repo, Op::Pull(PullMode::FastForwardOnly)),
        GitPullRebase => send_op(st, repo, Op::Pull(PullMode::Rebase)),
        GitPush => send_op(st, repo, Op::Push(PushOptions::default())),
        GitLog | GitLogAll => log::open(st, repo, cmd == GitLogAll),
        GitFileHistory => match panel_file(st).or_else(|| active_rel(st)) {
            Some(rel) => log::open_history(st, repo, rel),
            None => st.ed.error("file history: the active buffer is not a file in this repository"),
        },
        GitBlame => blame_view::toggle(st, repo),
        GitMarkResolved => mark_resolved(st, repo),
        GitAbort => {
            let what = st.git.status.as_ref().map_or("operation", |s| ops::state_noun(s.head.state));
            st.git.ask = Some(Ask::confirm(format!("Abort the {what}?"), Then::Op(Op::AbortOperation)));
        }
        GitContinue => send_op(st, repo, Op::ContinueOperation),
        _ => {}
    }
}

/// The file selected in the focused panel (its context menu's commands act on it).
fn panel_file(st: &State) -> Option<String> {
    let p = &st.git.panel;
    (p.focused && panel::draw::shown(st)).then(|| p.current_file().map(|f| f.path.clone())).flatten()
}

/// The active buffer's repository path.
fn active_rel(st: &State) -> Option<String> {
    st.git.rel_path(st.ed.buf().doc.path.as_deref()?)
}

/// Stage, unstage, discard or open: the panel's selected row when the panel shows, else the active file.
fn file_command(st: &mut State, repo: PathBuf, cmd: Command) {
    use Command::*;
    let p = &st.git.panel;
    if panel::draw::shown(st) && !p.rows.is_empty() {
        let i = p.selected;
        let action = match cmd {
            GitStage => p.stage_action(i),
            GitUnstage => p.unstage_action(i),
            GitDiscard => p.discard_action(i),
            GitOpenChange => p.current_file().map(|f| Action::OpenChange(f.clone(), false)),
            _ => p.current_file().map(|f| Action::OpenFile(f.path.clone(), false)),
        };
        if let Some(a) = action {
            return perform(st, a);
        }
    }
    let Some(rel) = active_rel(st) else {
        return st.ed.error("not a file in this repository (select one in the Source Control panel)");
    };
    let dirty = st.ed.buf().doc.dirty;
    match cmd {
        GitStage => {
            if dirty {
                st.ed.info("staged the file as saved (it has unsaved changes)");
            }
            send_op(st, repo, Op::Stage(vec![rel]))
        }
        GitUnstage => send_op(st, repo, Op::Unstage(vec![rel])),
        GitDiscard => {
            st.git.ask = Some(Ask::confirm(format!("Discard changes to {rel}?"), Then::Op(Op::Discard(vec![rel]))))
        }
        _ => {}
    }
}

fn mark_resolved(st: &mut State, repo: PathBuf) {
    let from_panel = panel::draw::shown(st)
        && st.git.panel.focused
        && st.git.panel.current_file().is_some_and(|f| f.section == panel::Section::Merge);
    let rel = if from_panel { st.git.panel.current_file().map(|f| f.path.clone()) } else { active_rel(st) };
    let Some(rel) = rel else { return st.ed.error("mark resolved: no conflicted file here") };
    if !from_panel {
        if !shrew_core::scm::conflicts(st.ed.buf().rope()).is_empty() {
            return st.ed.error("this file still has conflict markers: resolve them first");
        }
        if st.ed.buf().doc.dirty {
            st.ed.run(Command::Save);
            if st.ed.buf().doc.dirty {
                return; // saving failed and said why
            }
        }
    }
    send_op(st, repo, Op::MarkResolved(vec![rel]));
}

fn commit(st: &mut State, repo: PathBuf, amend: bool) {
    let p = &mut st.git.panel;
    if p.message.is_blank() {
        show_panel(st, Focus::Message);
        if amend {
            match st.git.status.as_ref().and_then(|s| s.head.oid) {
                Some(id) => {
                    st.git.send(Request::CommitDetails { repo, id }, Purpose::AmendMessage);
                }
                None => st.ed.error("nothing to amend: there is no commit yet"),
            }
        } else {
            st.ed.error("write a commit message first (Ctrl+Enter commits)");
        }
        return;
    }
    let staged = st.git.status.as_ref().is_none_or(|s| s.staged().next().is_some());
    if !staged && !amend {
        return st.ed.error("nothing staged: stage changes first (s on a file, S for all)");
    }
    let message = st.git.panel.message.text.trim_end().to_string() + "\n";
    send_op(st, repo, Op::Commit(CommitOptions { message, amend, ..Default::default() }));
}

fn cancel(st: &mut State) {
    match st.git.ops.running.as_ref().map(|r| r.ticket) {
        Some(t) => {
            if let Some(s) = &st.git.service {
                s.cancel(t);
            }
            st.ed.info("cancelling…");
        }
        None => st.ed.info("no git operation is running"),
    }
}

/// Carries out what the panel asked for.
pub fn perform(st: &mut State, a: Action) {
    match a {
        Action::Op(op) => {
            if let Some(repo) = repo_for(st, Command::GitRefresh) {
                send_op(st, repo, op);
            }
        }
        Action::Ask(q, op) => st.git.ask = Some(Ask::confirm(q, Then::Op(op))),
        Action::OpenChange(f, keep) => open_change(st, &f, keep),
        Action::OpenFile(path, keep) => open_file(st, &path, keep),
        Action::Commit { amend } => command(st, if amend { Command::GitAmend } else { Command::GitCommit }),
        Action::Run(cmd) => {
            st.ed.run(cmd);
            st.process_events();
        }
        Action::Pass(k) => {
            st.git.panel.focused = false;
            st.ed.handle_key(k);
        }
        Action::Paste => {
            if let Some(s) = st.ed.clipboard.get() {
                st.git.panel.message.insert(&s);
            }
        }
        Action::Unfocus => st.git.panel.focused = false,
        Action::Hide => hide_panel(st),
    }
}

/// Opens a changed file's diff. The diff view, when this build has one, takes over here (its command acts on the
/// active file); without it the file opens. A deleted file has nothing to open.
fn open_change(st: &mut State, f: &panel::File, keep: bool) {
    if f.mark == shrew_git::Mark::Deleted && f.section != panel::Section::Staged {
        return st.ed.info(format!("{} is deleted (u restores nothing: x discards the deletion)", f.path));
    }
    open_file(st, &f.path, keep);
    if let Some(diff) = Command::from_name("git_diff_file") {
        st.ed.run(diff);
        st.process_events();
    }
}

fn open_file(st: &mut State, rel: &str, keep: bool) {
    let Some(wd) = st.git.workdir() else { return };
    let path = wd.join(rel);
    if path.is_dir() {
        return st.ed.info(format!("{rel} is a folder"));
    }
    st.open_at(&path, None, None);
    if !keep {
        st.git.panel.focused = false;
    }
}

// ----- the panel's visibility and focus -----

/// Ctrl+Shift+G: shows the Git tab and gives it the keys; again, gives them back.
fn toggle_panel(st: &mut State) {
    let p = &st.git.panel;
    if panel::draw::shown(st) && p.focused {
        st.git.panel.focused = false;
        return;
    }
    show_panel(st, Focus::List);
}

pub fn show_panel(st: &mut State, focus: Focus) {
    let p = &mut st.git.panel;
    p.active = true;
    p.focused = true;
    p.focus = focus;
    p.follow = true;
    if !st.ed.sidebar_on() {
        st.git.panel.alone = true;
    }
    st.ed.sidebar_focus = false;
    #[cfg(feature = "terminal")]
    st.terminals.set_focus(false);
    sync_repo(st);
    if st.git.repo.is_some() && st.git.status.is_none() && !st.git.status_pending {
        request_status(st);
    }
    request_refs(st);
}

/// Reads the refs (branches, remotes) when they are not known: the panel's sync button needs a remote.
fn request_refs(st: &mut State) {
    if st.git.refs.is_some() || st.git.tickets.values().any(|p| matches!(p, Purpose::Refs(_))) {
        return;
    }
    if let Some(repo) = st.git.workdir().map(Path::to_path_buf) {
        st.git.send(Request::Refs { repo }, Purpose::Refs(None));
    }
}

fn hide_panel(st: &mut State) {
    let p = &mut st.git.panel;
    p.focused = false;
    p.active = false;
    p.alone = false;
}

/// The Files tab: back to the tree (turning the sidebar on when the panel showed alone).
pub fn show_files(st: &mut State) {
    let alone = st.git.panel.alone;
    hide_panel(st);
    if alone && !st.ed.sidebar_on() {
        st.ed.run(Command::ToggleSidebar);
        st.process_events();
    }
}

// ----- input -----

/// A key, before the editor sees it: the git prompt, the git pickers' own keys, the focused panel, a log tab, a
/// blame click. True when taken.
pub fn key(st: &mut State, k: KeyPress) -> bool {
    if let Some(a) = &mut st.git.ask {
        match a.key(k) {
            Answer::Waiting => {}
            Answer::Cancel => st.git.ask = None,
            Answer::Yes { text, alt } => {
                let then = st.git.ask.take().expect("open").then;
                answer(st, then, text, alt);
            }
        }
        return true;
    }
    let ed = &st.ed;
    let overlay = ed.prompt.is_some() || ed.about.is_some() || st.settings.is_open();
    if k.key == Key::Esc
        && st.git.ops.toast_error()
        && ed.picker.is_none()
        && !overlay
        && !(ed.vim() && ed.mode != Mode::Normal)
    {
        st.git.ops.close_toast();
        return true;
    }
    if let Some(p) = &ed.picker {
        if let PickerKind::Choice(c) = p.kind
            && c.is_git()
        {
            return pickers::key(st, c, k);
        }
        return false;
    }
    if overlay || !on(&st.ed) {
        return false;
    }
    if st.git.panel.focused && panel::draw::shown(st) {
        if let Some(a) = st.git.panel.key(k) {
            perform(st, a);
        }
        return true;
    }
    if log::key(st, k) {
        return true;
    }
    blame_view::key(st, k)
}

/// Text from an input method: into the commit message or the git prompt. True when taken.
pub fn text(st: &mut State, s: &str) -> bool {
    if let Some(Ask { input: Some(b), .. }) = &mut st.git.ask {
        b.insert(s);
        return true;
    }
    if st.git.panel.focused && st.git.panel.focus == Focus::Message && panel::draw::shown(st) {
        st.git.panel.message.insert(s);
        return true;
    }
    false
}

/// Where typed text goes is a git field (the input method should be on).
pub fn typing(st: &State) -> bool {
    matches!(st.git.ask, Some(Ask { input: Some(_), .. }))
        || (st.git.panel.focused && st.git.panel.focus == Focus::Message && panel::draw::shown(st))
}

/// After every input: follow the active file's repository while something shows it, re-read the status after a
/// save, keep the parts in step with the buffers.
pub fn after_input(st: &mut State) {
    if !on(&st.ed) {
        return;
    }
    // with the sidebar on, the panel is its tab (Ctrl+B then hides both)
    if st.ed.sidebar_on() {
        st.git.panel.alone = false;
    }
    // the tree took the keys (Ctrl+Shift+E): its tab shows
    if st.ed.sidebar_focus && st.git.panel.active {
        hide_panel(st);
    }
    if st.git.panel.focused && !panel::draw::shown(st) {
        st.git.panel.focused = false;
    }
    if panel::draw::shown(st) {
        sync_repo(st);
    }
    saved_files(st);
    st.git.views.prune(&st.ed);
    blame_view::after_input(st);
}

/// Re-reads the status when a file of the repository was saved.
fn saved_files(st: &mut State) {
    let (Some(repo), Some(svc)) = (&st.git.repo, &st.git.service) else { return };
    let mut changed = false;
    for b in &st.ed.buffers {
        let Some(p) = &b.doc.path else { continue };
        if b.doc.dirty || !p.starts_with(&repo.workdir) {
            continue;
        }
        let v = st.git.saved.entry(b.id).or_insert(b.doc.version);
        if *v != b.doc.version {
            *v = b.doc.version;
            changed = true;
        }
    }
    if changed {
        svc.refresh(&repo.workdir);
    }
}

/// The window gained or lost the focus: the service polls only while focused.
pub fn window_focus(st: &mut State, focused: bool) {
    if let Some(s) = &st.git.service {
        s.set_focused(focused);
    }
}

/// The feature was switched: off hides everything and drops what was asked.
pub fn feature_changed(st: &mut State) {
    if on(&st.ed) {
        return;
    }
    hide_panel(st);
    st.git.ask = None;
    st.git.blame.clear();
    st.git.cancel_tickets(|p| matches!(p, Purpose::Op(_)));
    let views: Vec<u64> = st.git.views.ids();
    for id in views {
        if let Some(i) = st.ed.buffers.iter().position(|b| b.id == id) {
            st.ed.switch_to(i);
            st.ed.run(Command::CloseBuffer);
        }
    }
}

/// `srw --screenshot` waits for these.
pub fn busy(st: &State) -> bool {
    st.git.busy()
}

/// Enter in a git picker: `value` is what the row stands for.
pub fn choose(st: &mut State, choice: Choice, value: String) {
    pickers::choose(st, choice, value);
}

// ----- drawing and the mouse -----

/// Forgets last frame's clickable areas.
pub fn begin_frame(st: &mut State) {
    st.git.panel.hits = Default::default();
    st.git.views.clear_hits();
    st.git.blame.hits.clear();
    st.git.conflicts.hits.clear();
    st.git.ops.hits = Default::default();
}

/// Takes the blame column out of the text area.
pub fn carve(lay: Layout, st: &State) -> Layout {
    blame_view::carve(lay, st)
}

/// A log tab draws itself instead of text. True when the active buffer is one.
pub fn draw_view(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors) -> bool {
    log::draw::draw(scene, st, lay, c)
}

/// Conflict regions and the blame column over the text.
pub fn draw_decorations(scene: &mut Scene, st: &mut State, ui: &Ui, lay: &Layout, c: &Colors, scroll: Scroll) {
    if !on(&st.ed) {
        return;
    }
    conflict::draw(scene, st, ui, lay, c, scroll);
    blame_view::draw(scene, st, lay, c, scroll);
}

/// The git prompt and the progress or error toast.
pub fn draw_overlays(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors) {
    ops::draw(scene, st, lay, c);
    if let Some(a) = &st.git.ask {
        a.draw(scene, lay, c);
    }
}

/// A left click. True when git took it.
pub fn press(st: &mut State, x: f32, y: f32) -> bool {
    if ops::press(st, x, y) || conflict::press(st, x, y) || blame_view::press(st, x, y) {
        return true;
    }
    if panel::draw::press(st, x, y) {
        return true;
    }
    if st.git.panel.focused {
        st.git.panel.focused = false;
    }
    log::press(st, x, y)
}

/// A right click: the panel's context menu. True when taken.
pub fn right_press(st: &mut State, x: f32, y: f32) -> bool {
    panel::draw::right_press(st, x, y)
}

/// The wheel over the panel or a log tab. True when taken.
pub fn wheel(st: &mut State, x: f32, y: f32, lines: f32) -> bool {
    panel::draw::wheel(st, x, y, lines) || log::wheel(st, x, y, lines)
}

/// The pointer moved: hover states. (redraw, pointer over something clickable)
pub fn hover(st: &mut State, x: f32, y: f32) -> (bool, bool) {
    let (a, pa) = panel::draw::hover(st, x, y);
    let (b, pb) = log::hover(st, x, y);
    let pc = st.git.conflicts.hits.iter().any(|h| h.0.contains(x, y))
        || st.git.blame.hits.iter().any(|h| h.0.contains(x, y))
        || st.git.ops.hits.close.is_some_and(|r| r.contains(x, y));
    (a || b, pa || pb || pc)
}

// ----- small helpers -----

/// "3 min ago", "2 days ago", "5 years ago" from a commit time (seconds since the epoch).
pub fn ago(time: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    ago_at(time, now)
}

pub fn ago_at(time: i64, now: i64) -> String {
    let s = (now - time).max(0);
    let (n, unit) = match s {
        s if s < 60 => return "just now".into(),
        s if s < 3600 => (s / 60, "min"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 7 * 86_400 => (s / 86_400, "day"),
        s if s < 30 * 86_400 => (s / (7 * 86_400), "week"),
        s if s < 365 * 86_400 => (s / (30 * 86_400), "month"),
        s => (s / (365 * 86_400), "year"),
    };
    let plural = if n == 1 || unit == "min" { "" } else { "s" };
    format!("{n} {unit}{plural} ago")
}

/// "3mo", "2y": the blame column's short age.
pub fn ago_short(time: i64, now: i64) -> String {
    let s = (now - time).max(0);
    match s {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s if s < 30 * 86_400 => format!("{}d", s / 86_400),
        s if s < 365 * 86_400 => format!("{}mo", s / (30 * 86_400)),
        s => format!("{}y", s / (365 * 86_400)),
    }
}

#[cfg(test)]
mod tests;
