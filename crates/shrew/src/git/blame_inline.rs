//! Inline blame (feature `git_blame_inline`): once the cursor has rested on a line for `[git] blame_delay`
//! (300 ms), who last changed it shows faintly after its end: "Ann, 3 days ago • Fix the parser"; lines not
//! committed yet show "You • Uncommitted changes". Nothing runs until the cursor rests in a tracked file; then the
//! whole buffer is blamed once (`git blame --incremental --contents -`, streamed, on the service's blame lane) and
//! kept for that version of the buffer, so moving around costs nothing. An edit asks again at the next rest.

use super::Place;
use crate::colors::Colors;
use crate::layout::{Layout, Scroll};
use crate::state::State;
use shrew_core::features::Feature;
use shrew_core::{Mode, text};
use shrew_git::{BlameCommit, BlameHunk, Request, Ticket};
use shrew_render::{Scene, TextStyle, columns};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime};

/// One buffer version's blame, line by line as it streams in.
struct FileBlame {
    version: u64,
    repo: PathBuf,
    ticket: Ticket,
    lines: Vec<Option<Arc<BlameCommit>>>,
    done: bool,
}

/// Where the cursor rests.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Rest {
    buffer: u64,
    version: u64,
    line: usize,
    since: Instant,
    /// The pause is over: show (and blame if needed).
    due: bool,
}

#[derive(Default)]
pub struct InlineBlame {
    files: HashMap<u64, FileBlame>,
    rest: Option<Rest>,
    /// `user.name` of each repository (to say "You"), read once with `git config`.
    users: HashMap<PathBuf, Option<String>>,
    asked_users: Option<Receiver<(PathBuf, Option<String>)>>,
    user_tx: Option<mpsc::Sender<(PathBuf, Option<String>)>>,
}

impl InlineBlame {
    pub fn clear(&mut self) {
        self.files.clear();
        self.rest = None;
    }

    /// HEAD moved: blames there are out of date.
    pub fn repo_changed(&mut self, repo: &Path) {
        self.files.retain(|_, f| f.repo != repo);
    }

    pub fn deadline(&self, delay: Duration) -> Option<Instant> {
        self.rest.filter(|r| !r.due).map(|r| r.since + delay)
    }

    pub fn busy(&self) -> bool {
        self.rest.is_some_and(|r| !r.due) || self.files.values().any(|f| !f.done)
    }

    pub fn on_hunks(&mut self, ticket: Option<Ticket>, hunks: Vec<BlameHunk>) {
        let Some(f) = self.files.values_mut().find(|f| Some(f.ticket) == ticket) else { return };
        for h in hunks {
            let (a, z) = (h.start as usize, (h.start + h.len) as usize);
            if z > f.lines.len() {
                f.lines.resize(z, None);
            }
            for slot in &mut f.lines[a..z] {
                *slot = Some(h.commit.clone());
            }
        }
    }

    pub fn on_done(&mut self, ticket: Option<Ticket>, r: shrew_git::Result<()>) {
        if let Some(f) = self.files.values_mut().find(|f| Some(f.ticket) == ticket) {
            f.done = true;
            if let Err(e) = r {
                log::debug!("inline blame: {e}");
            }
        }
    }

    /// The blame of the cursor line, when it is the line resting and its blame is in.
    fn current(&self, buffer: u64, version: u64, line: usize) -> Option<&Arc<BlameCommit>> {
        let r = self.rest?;
        if !r.due || (r.buffer, r.version, r.line) != (buffer, version, line) {
            return None;
        }
        let f = self.files.get(&buffer).filter(|f| f.version == version)?;
        f.lines.get(line)?.as_ref()
    }

    /// Reads `user.name` of `repo` on a short-lived thread, once per repository.
    fn ask_user(&mut self, repo: &Path, wake: crate::state::Waker) {
        if self.users.contains_key(repo) {
            return;
        }
        self.users.insert(repo.to_path_buf(), None);
        let tx = match &self.user_tx {
            Some(tx) => tx.clone(),
            None => {
                let (tx, rx) = mpsc::channel();
                self.asked_users = Some(rx);
                self.user_tx = Some(tx.clone());
                tx
            }
        };
        spawn_user(repo.to_path_buf(), tx, wake);
    }
}

fn spawn_user(repo: PathBuf, tx: mpsc::Sender<(PathBuf, Option<String>)>, wake: crate::state::Waker) {
    let _ = std::thread::Builder::new().name("shrew-git-user".into()).spawn(move || {
        let name = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["config", "--get", "user.name"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|n| !n.is_empty());
        let _ = tx.send((repo, name));
        wake();
    });
}

/// Watches where the cursor rests.
pub fn sync(st: &mut State, now: Instant) {
    if let Some(rx) = &st.git.blame.asked_users {
        while let Ok((repo, name)) = rx.try_recv() {
            st.git.blame.users.insert(repo, name);
        }
    }
    let ed = &st.ed;
    let b = ed.buf();
    let quiet = ed.features.on(Feature::GitBlameInline)
        && !ed.mode.is_visual()
        && b.sel.len() == 1
        && ed.prompt.is_none()
        && ed.picker.is_none()
        && !st.settings.is_open();
    let place = b.doc.path.clone().filter(|_| quiet).map(|p| st.git.place(&p));
    let Some(Place::In { repo, rel }) = place else {
        st.git.blame.rest = None;
        return;
    };
    // untracked and ignored files have nothing to blame
    let untracked = st.git.status.get(&repo).and_then(|s| s.get(&rel)).is_some_and(|e| e.untracked || e.ignored);
    if untracked {
        st.git.blame.rest = None;
        return;
    }
    let b = st.ed.buf();
    let (buffer, version, line) = (b.id, b.doc.version, b.cursor_line());
    let r = &mut st.git.blame.rest;
    if r.is_none_or(|r| (r.buffer, r.version, r.line) != (buffer, version, line)) {
        *r = Some(Rest { buffer, version, line, since: now, due: st.git.blame_delay.is_zero() });
    }
    let wake = st.wake.clone();
    st.git.blame.ask_user(&repo, wake);
    st.git.blame.files.retain(|id, _| st.ed.buffers.iter().any(|b| b.id == *id));
    if st.git.blame.rest.is_some_and(|r| r.due) {
        blame_if_needed(st, &repo, &rel);
    }
}

/// The pause is over: show the blame, asking for it first when this version has none.
pub fn tick(st: &mut State, now: Instant) -> bool {
    let delay = st.git.blame_delay;
    let Some(r) = st.git.blame.rest.as_mut().filter(|r| !r.due && r.since + delay <= now) else { return false };
    r.due = true;
    let place = st.ed.buf().doc.path.clone().map(|p| st.git.place(&p));
    if let Some(Place::In { repo, rel }) = place {
        blame_if_needed(st, &repo, &rel);
    }
    true
}

fn blame_if_needed(st: &mut State, repo: &Path, rel: &str) {
    let b = st.ed.buf();
    let (id, version) = (b.id, b.doc.version);
    if st.git.blame.files.get(&id).is_some_and(|f| f.version == version && f.repo == repo) {
        return;
    }
    let contents = Some(b.doc.rope.clone());
    let lines = b.doc.rope.len_lines();
    let ticket =
        st.git.link.send(Request::Blame { repo: repo.to_path_buf(), path: rel.to_string(), rev: None, contents });
    let f = FileBlame { version, repo: repo.to_path_buf(), ticket, lines: vec![None; lines], done: false };
    st.git.blame.files.insert(id, f);
}

/// "3 days ago".
pub fn ago(secs: i64) -> String {
    let s = secs.max(0);
    let (n, unit) = match s {
        0..60 => return "just now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86_400 => (s / 3600, "hour"),
        86_400..604_800 => (s / 86_400, "day"),
        604_800..2_629_800 => (s / 604_800, "week"),
        2_629_800..31_557_600 => (s / 2_629_800, "month"),
        _ => (s / 31_557_600, "year"),
    };
    if n == 1 && unit == "day" {
        return "yesterday".into();
    }
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// The text shown for a line's commit.
pub fn label(c: &BlameCommit, user: Option<&str>, now: i64) -> String {
    if c.is_uncommitted() {
        return "You • Uncommitted changes".into();
    }
    let who = if user.is_some_and(|u| u == c.author.name) { "You" } else { c.author.name.as_str() };
    format!("{who}, {} • {}", ago(now - c.author.time), c.summary)
}

/// The cursor line's blame at its end, faint, clipped to the text area.
pub fn draw(scene: &mut Scene, st: &State, lay: &Layout, c: &Colors, scroll: Scroll, first: usize, end: usize) {
    let ed = &st.ed;
    if (ed.mode == Mode::Insert && ed.vim()) || st.git.gutter.peek.is_some() {
        return; // typing, or looking at a change: no distraction
    }
    let b = ed.buf();
    let line = b.cursor_line();
    if !(first..end).contains(&line) {
        return;
    }
    let Some(commit) = st.git.blame.current(b.id, b.doc.version, line) else { return };
    let repo = match b.doc.path.as_deref().and_then(|p| st.git.file_place(p)) {
        Some(Place::In { repo, .. }) => repo,
        _ => return,
    };
    let user = st.git.blame.users.get(repo).and_then(|u| u.as_deref());
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let s = label(commit, user, now);
    let rope = b.rope();
    let tw = ed.settings.tab_width.max(1);
    let end_col = text::col_of(rope, text::line_end(rope, line), tw);
    let x = scroll.col_x(lay, end_col as f32 + 4.0);
    let room = ((lay.text_rect().right() - x) / lay.cw).floor();
    if room < 12.0 {
        return;
    }
    let s = crate::draw::elide_end(&s, room as usize - 1);
    if columns(&s) < 4 {
        return;
    }
    scene.text(x.round(), scroll.line_y(lay, line), &s, TextStyle::new(c.faint).italic(), &[]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use shrew_git::{Oid, Signature};

    #[test]
    fn relative_times() {
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(61), "1 minute ago");
        assert_eq!(ago(3 * 3600 + 5), "3 hours ago");
        assert_eq!(ago(86_400 + 10), "yesterday");
        assert_eq!(ago(3 * 86_400), "3 days ago");
        assert_eq!(ago(15 * 86_400), "2 weeks ago");
        assert_eq!(ago(100 * 86_400), "3 months ago");
        assert_eq!(ago(800 * 86_400), "2 years ago");
        assert_eq!(ago(-5), "just now", "a clock behind the commit");
    }

    #[test]
    fn labels() {
        let mut c = BlameCommit {
            id: Oid::from_hex("1111111111111111111111111111111111111111").unwrap(),
            author: Signature { name: "Ann".into(), email: "ann@x".into(), time: 1000, offset: 0 },
            summary: "Fix the parser".into(),
            ..Default::default()
        };
        assert_eq!(label(&c, None, 1000 + 3 * 86_400), "Ann, 3 days ago • Fix the parser");
        assert_eq!(label(&c, Some("Ann"), 1000 + 30), "You, just now • Fix the parser");
        c.id = Oid::ZERO;
        assert_eq!(label(&c, Some("Bob"), 0), "You • Uncommitted changes");
    }
}
