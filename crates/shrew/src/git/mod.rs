//! Git in the editor (cargo feature `git`). One [`GitService`] (shrew-git) does every read and write off the UI
//! thread: this module owns it, finds the repository of each file and of the sidebar's project, keeps the latest
//! status of each repository in use, and hands every reply to the part it belongs to:
//!
//! - [`gutter`]: change markers (gutter, minimap, scrollbar), the hunk peek, staging, unstaging and reverting hunks
//!   or selected lines (feature `git_gutter`).
//! - [`blame_inline`]: who last changed the cursor line, at its end after a pause (feature `git_blame_inline`).
//! - [`status_ui`]: the branch in the status bar and title bar, file states in the sidebar (feature `git_status`).
//!
//! Cost: nothing starts before the first file or project is looked up (one idle thread then), and nothing more
//! runs for files outside a repository. The service watches the repositories in use only while the window has the
//! focus, and the UI thread never waits on it: replies arrive through the wake callback and [`poll`].

pub mod blame_inline;
pub mod gutter;
pub mod status_ui;
#[cfg(test)]
mod tests;

use crate::state::{State, Waker};
use shrew_core::Command;
use shrew_core::features::Feature;
use shrew_git::{GitService, Reply, RepoInfo, Request, Status, StatusOptions, Ticket};
use shrew_render::Rect;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Clickable git parts of the last frame.
#[derive(Default, Debug)]
pub struct Hits {
    /// The branch in the status bar.
    pub branch: Option<Rect>,
    /// The hunk peek and its buttons.
    pub peek: Option<Rect>,
    pub peek_buttons: Vec<(Rect, gutter::PeekButton)>,
}

/// The service, started on first use.
pub struct Link {
    wake: Waker,
    service: Option<GitService>,
    focused: bool,
}

impl Link {
    pub fn send(&mut self, req: Request) -> Ticket {
        let focused = self.focused;
        let wake = self.wake.clone();
        let s = self.service.get_or_insert_with(|| {
            let s = GitService::new(wake);
            s.set_focused(focused);
            s
        });
        s.send(req)
    }
}

/// Where a file is, git-wise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// Still being found out.
    Unknown,
    Outside,
    /// In the repository `repo` (its workdir) as `rel` (`src/main.rs`).
    In {
        repo: PathBuf,
        rel: String,
    },
}

pub struct Git {
    pub link: Link,
    /// Folder -> its repository's workdir (`None`: in none), as discovered.
    dirs: HashMap<PathBuf, Option<PathBuf>>,
    asking: HashSet<PathBuf>,
    pub repos: HashMap<PathBuf, RepoInfo>,
    /// The latest status of each repository in use (the service sends a new one whenever it changes).
    pub status: HashMap<PathBuf, Arc<Status>>,
    watching: HashSet<PathBuf>,
    /// File -> where it is, once its folder is known.
    files: HashMap<PathBuf, Place>,
    /// Each buffer's dirty flag last seen: a save refreshes its repository's status.
    dirty: HashMap<u64, bool>,
    pub gutter: gutter::Gutters,
    pub blame: blame_inline::InlineBlame,
    pub status_ui: status_ui::StatusUi,
    /// `[git] gutter_base = "head"`.
    pub head_base: bool,
    pub blame_delay: Duration,
}

impl Git {
    pub fn new(wake: Waker, cfg: &shrew_config::GitConfig) -> Git {
        let mut g = Git {
            link: Link { wake, service: None, focused: true },
            dirs: HashMap::new(),
            asking: HashSet::new(),
            repos: HashMap::new(),
            status: HashMap::new(),
            watching: HashSet::new(),
            files: HashMap::new(),
            dirty: HashMap::new(),
            gutter: Default::default(),
            blame: Default::default(),
            status_ui: Default::default(),
            head_base: false,
            blame_delay: Duration::ZERO,
        };
        g.configure(cfg);
        g
    }

    /// `[git]` from config.toml.
    pub fn configure(&mut self, cfg: &shrew_config::GitConfig) {
        if self.head_base != cfg.gutter_head() {
            self.head_base = cfg.gutter_head();
            self.gutter.base_changed();
        }
        self.blame_delay = Duration::from_millis(cfg.blame_delay());
    }

    /// Window focus: the service only watches repositories while focused, and re-reads them when it comes back.
    pub fn set_focused(&mut self, focused: bool) {
        self.link.focused = focused;
        if let Some(s) = &self.link.service {
            s.set_focused(focused);
        }
        if focused {
            // a folder may have become a repository meanwhile
            self.dirs.retain(|_, w| w.is_some());
            self.files.retain(|_, p| matches!(p, Place::In { .. }));
        }
    }

    /// Where `file` is; asks the service about its folder the first time.
    pub fn place(&mut self, file: &Path) -> Place {
        if let Some(p) = self.files.get(file) {
            return p.clone();
        }
        let Some(dir) = file.parent() else { return Place::Outside };
        let place = match self.dir_repo(dir) {
            None => return Place::Unknown,
            Some(None) => Place::Outside,
            Some(Some(repo)) => match rel_path(&repo, file) {
                Some(rel) => Place::In { repo, rel },
                None => Place::Outside,
            },
        };
        self.files.insert(file.to_path_buf(), place.clone());
        place
    }

    /// The repository `dir` is in: `None` while it is being found out, `Some(None)` outside any.
    pub fn dir_repo(&mut self, dir: &Path) -> Option<Option<PathBuf>> {
        if let Some(w) = self.dirs.get(dir) {
            return Some(w.clone());
        }
        if self.asking.insert(dir.to_path_buf()) {
            self.link.send(Request::Discover { path: dir.to_path_buf() });
        }
        None
    }

    /// Starts watching `repo` (its status comes now and whenever it changes).
    fn watch(&mut self, repo: &Path) {
        if self.watching.insert(repo.to_path_buf()) {
            let options = StatusOptions::default();
            self.link.send(Request::Status { repo: repo.to_path_buf(), options });
        }
    }

    /// The repositories in use: the active buffer's, the open buffers' and the sidebar project's.
    fn unwatch_except(&mut self, used: &HashSet<PathBuf>) {
        let gone: Vec<PathBuf> = self.watching.iter().filter(|r| !used.contains(*r)).cloned().collect();
        for r in gone {
            self.watching.remove(&r);
            self.status.remove(&r);
            if let Some(s) = &self.link.service {
                s.unwatch(&r);
            }
        }
    }

    pub fn status_of(&self, repo: &Path) -> Option<&Arc<Status>> {
        self.status.get(repo)
    }

    /// Where `file` is, if it was looked up already (no request).
    pub fn file_place(&self, file: &Path) -> Option<&Place> {
        self.files.get(file)
    }

    /// The repository of `dir`, if discovery already said (no request).
    pub fn known_dir_repo(&self, dir: &Path) -> Option<PathBuf> {
        self.dirs.get(dir).cloned().flatten()
    }
}

/// `file` relative to the canonical `workdir`, `/`-separated (its folder is resolved through symlinks once).
fn rel_path(workdir: &Path, file: &Path) -> Option<String> {
    let dir = std::fs::canonicalize(file.parent()?).ok()?;
    let full = dir.join(file.file_name()?);
    let rel = full.strip_prefix(workdir).ok()?;
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    let rel = parts?.join("/");
    (!rel.is_empty() && !rel.starts_with(".git/") && rel != ".git").then_some(rel)
}

/// Whether anything git is switched on.
pub fn any_on(st: &State) -> bool {
    let f = &st.ed.features;
    f.on(Feature::GitGutter) || f.on(Feature::GitBlameInline) || f.on(Feature::GitStatus)
}

/// After input, a wake or a timer: looks up what the screen shows and asks for what is missing or stale.
pub fn sync(st: &mut State) {
    if !any_on(st) {
        if !st.ed.git_hunks.is_empty() {
            st.ed.git_hunks.clear();
        }
        st.git.gutter.peek = None;
        return;
    }
    let now = Instant::now();
    let mut used = HashSet::new();
    // saves: the status changed (the service also notices within its poll interval, this is at once)
    let mut saved = Vec::new();
    for b in &st.ed.buffers {
        let was = st.git.dirty.insert(b.id, b.doc.dirty);
        if was == Some(true) && !b.doc.dirty {
            saved.push(b.id);
        }
    }
    st.git.dirty.retain(|id, _| st.ed.buffers.iter().any(|b| b.id == *id));
    // every open file's repository stays watched
    let files: Vec<(u64, PathBuf)> =
        st.ed.buffers.iter().filter_map(|b| b.doc.path.clone().map(|p| (b.id, p))).collect();
    for (id, path) in &files {
        if let Place::In { repo, .. } = st.git.place(path) {
            if saved.contains(id)
                && let Some(s) = &st.git.link.service
            {
                s.refresh(&repo);
            }
            used.insert(repo);
        }
    }
    // the sidebar's project
    if st.ed.features.on(Feature::GitStatus)
        && st.ed.sidebar_on()
        && let Some(root) = st.ed.tree.as_ref().map(|t| t.root.clone())
        && let Some(Some(repo)) = st.git.dir_repo(&root)
    {
        used.insert(repo);
    }
    for r in &used {
        st.git.watch(r);
    }
    st.git.unwatch_except(&used);
    gutter::sync(st, now, &saved);
    blame_inline::sync(st, now);
}

/// Takes the service's replies (on a wake). True when something on screen may have changed.
pub fn poll(st: &mut State) -> bool {
    let events = match &st.git.link.service {
        Some(s) => s.poll(),
        None => Vec::new(),
    };
    let mut changed = false;
    for ev in events {
        let repo = ev.repo.clone();
        changed |= !matches!(ev.reply, Reply::Progress(_));
        match ev.reply {
            Reply::Discovered { path, repo } => {
                st.git.asking.remove(&path);
                let workdir = repo.as_ref().map(|r| r.workdir.clone());
                if let Some(info) = repo {
                    st.git.repos.insert(info.workdir.clone(), info);
                }
                st.git.dirs.insert(path, workdir);
            }
            Reply::Status(Ok(s)) => {
                if let Some(r) = repo
                    && st.git.watching.contains(&r)
                {
                    st.git.status.insert(r, s);
                    st.git.status_ui.invalidate();
                }
            }
            Reply::Status(Err(e)) => log::debug!("git status: {e}"),
            Reply::Changed(c) => {
                if let Some(r) = repo {
                    if c.index || c.head {
                        st.git.gutter.repo_changed(&r, c.head);
                    }
                    if c.head {
                        st.git.blame.repo_changed(&r);
                    }
                }
            }
            Reply::Gutter(r) => gutter::on_gutter(st, ev.ticket, r),
            Reply::FileDiff(r) => gutter::on_file_diff(st, ev.ticket, r),
            Reply::OpDone(r) => gutter::on_op_done(st, ev.ticket, r),
            Reply::Blame(hunks) => st.git.blame.on_hunks(ev.ticket, hunks),
            Reply::BlameDone(r) => st.git.blame.on_done(ev.ticket, r),
            _ => {}
        }
    }
    changed |= tick(st, Instant::now());
    if changed {
        sync(st);
    }
    changed
}

/// Runs the timers that are due (the gutter's debounce, the blame's pause). True when it did something.
pub fn tick(st: &mut State, now: Instant) -> bool {
    let a = gutter::tick(st, now);
    let b = blame_inline::tick(st, now);
    a || b
}

/// The next timer, for the event loop.
pub fn deadline(st: &State) -> Option<Instant> {
    [st.git.gutter.deadline(), st.git.blame.deadline(st.git.blame_delay)].into_iter().flatten().min()
}

/// Work in flight or timers pending (`--screenshot` waits for them).
pub fn busy(st: &State) -> bool {
    any_on(st)
        && (!st.git.asking.is_empty()
            || st.git.gutter.busy()
            || st.git.blame.busy()
            || st.git.watching.iter().any(|r| !st.git.status.contains_key(r)))
}

/// `--keys`: waits (a few seconds at most) until discovery, the markers and hunk commands are done, so the next
/// key sees them.
pub fn settle_markers(st: &mut State) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while any_on(st) && (!st.git.asking.is_empty() || st.git.gutter.busy()) && Instant::now() < deadline {
        st.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// [`shrew_core::Event::Git`]: a hunk command.
pub fn command(st: &mut State, cmd: Command) {
    gutter::command(st, cmd);
}

/// A feature was switched: what it showed goes away at once, or comes back on the next sync.
pub fn feature_changed(st: &mut State, f: Feature) {
    match f {
        Feature::GitGutter if !st.ed.features.on(f) => {
            st.ed.git_hunks.clear();
            st.git.gutter.clear();
        }
        Feature::GitBlameInline if !st.ed.features.on(f) => st.git.blame.clear(),
        _ => {}
    }
    sync(st);
}

/// A key the git parts take before the editor (Esc closes the hunk peek). True when taken.
pub fn key(st: &mut State, k: shrew_core::KeyPress) -> bool {
    gutter::key(st, k)
}

/// A left click: the peek's buttons, a gutter marker, the status bar's branch. True when taken.
pub fn press(
    st: &mut State,
    hits: &Hits,
    lay: &crate::layout::Layout,
    scroll: crate::layout::Scroll,
    x: f32,
    y: f32,
) -> bool {
    if hits.branch.is_some_and(|r| r.contains(x, y)) {
        status_ui::branch_clicked(st);
        return true;
    }
    gutter::press(st, hits, lay, scroll, x, y)
}
