//! Change markers (feature `git_gutter`): each line's state against the index (or HEAD, `[git] gutter_base`), live
//! while typing. The active buffer's rope (an O(1) snapshot) goes to the service a moment after the last edit
//! ([`DEBOUNCE`], newest wins); the markers are drawn from the last result, in the gutter (a bar for added and
//! changed lines, a triangle where lines were deleted), the minimap and the scrollbar. Files outside a repository,
//! untracked, ignored or binary ones cost nothing after the first answer.
//!
//! Hunk commands, at the cursor or on the selected lines (`ghs` / `ghu` / `ghr` / `ghp`, the palette, the peek's
//! buttons): stage and unstage go through `git apply --cached` (a stale index fails instead of staging the wrong
//! lines), revert is an ordinary edit of the buffer (`u` undoes it), and the peek shows the hunk's old and new lines
//! with the changed words marked, under the hunk; it follows the cursor from hunk to hunk and Esc closes it.

use super::{Hits, Place};
use crate::colors::{Colors, mix};
use crate::layout::{Layout, Scroll};
use crate::state::State;
use shrew_core::change::{Change, ChangeSet};
use shrew_core::features::Feature;
use shrew_core::git_hunks::{self as marks, HunkKind};
use shrew_core::{Command, Key, KeyPress, Mode, text};
use shrew_git::diff::{self, EolMode, Hunk};
use shrew_git::patch::{self, LineSel};
use shrew_git::{Base, FileDiff, FileTrack, Gutter, GutterBase, Op, OpOutput, Request, Text, Ticket, Version};
use shrew_render::{Color, Rect, Scene, TextStyle, char_columns, columns};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Quiet time after an edit before the buffer is diffed again (newer edits restart it).
pub const DEBOUNCE: Duration = Duration::from_millis(30);
/// While typing without a pause the markers still follow this often.
const MAX_WAIT: Duration = Duration::from_millis(150);
/// Lines the peek shows at most (the rest is counted).
const PEEK_ROWS: usize = 14;

/// What one buffer's markers came from.
struct Buf {
    file: PathBuf,
    repo: PathBuf,
    rel: String,
    base: GutterBase,
    result: Option<Gutter>,
    /// The version asked for and its ticket, until the answer comes.
    asked: Option<(u64, Ticket)>,
    /// A version whose diff failed (not asked again).
    failed: Option<u64>,
    /// The version the last sync saw.
    seen: u64,
    due: Option<Instant>,
    /// When the current run of edits started (for [`MAX_WAIT`]).
    since: Option<Instant>,
    /// The base moved (index or HEAD): ask again even for the same text.
    stale: bool,
}

impl Buf {
    /// A file with no line diff (untracked, binary, too big, conflicted) is not diffed again on every edit.
    fn passive(&self) -> bool {
        self.result.as_ref().is_some_and(|r| r.file != FileTrack::Tracked)
    }
}

/// A peek button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeekButton {
    Stage,
    Revert,
    Close,
}

/// The open hunk peek: the hunk shown on `line` of buffer `buffer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peek {
    pub buffer: u64,
    pub line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Stage,
    Unstage,
}

/// A stage or unstage waiting for the staged diff (HEAD -> index) of its file.
struct Waiting {
    ticket: Ticket,
    act: Act,
    target: Target,
}

/// What a hunk command acts on.
#[derive(Clone, Debug)]
struct Target {
    buffer: u64,
    repo: PathBuf,
    rel: String,
    lines: std::ops::Range<usize>,
    selected: bool,
}

#[derive(Default)]
pub struct Gutters {
    bufs: HashMap<u64, Buf>,
    pub peek: Option<Peek>,
    waiting: Vec<Waiting>,
    /// Ops sent from here, with what to say when they are done.
    ops: HashMap<Ticket, String>,
}

impl Gutters {
    pub fn clear(&mut self) {
        self.bufs.clear();
        self.peek = None;
    }

    /// `[git] gutter_base` changed: every buffer starts over.
    pub fn base_changed(&mut self) {
        self.bufs.clear();
    }

    /// The index or HEAD of `repo` moved: its buffers are diffed again.
    pub fn repo_changed(&mut self, repo: &std::path::Path, head: bool) {
        for b in self.bufs.values_mut().filter(|b| b.repo == repo) {
            if b.base == GutterBase::Index || head {
                b.stale = true;
                b.failed = None;
            }
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.bufs.values().filter_map(|b| b.due).min()
    }

    pub fn busy(&self) -> bool {
        self.bufs.values().any(|b| b.asked.is_some() || b.due.is_some())
            || !self.waiting.is_empty()
            || !self.ops.is_empty()
    }

    /// The active buffer's last result (its hunks match [`shrew_core::Editor::hunks`]).
    fn result(&self, id: u64) -> Option<&Gutter> {
        self.bufs.get(&id)?.result.as_ref()
    }
}

fn to_marks(hunks: &[Hunk]) -> Vec<marks::Hunk> {
    hunks.iter().map(|h| marks::Hunk { old: h.old.clone(), new: h.new.clone() }).collect()
}

/// Asks for the active buffer's markers when they are missing or stale, debounced after edits.
pub fn sync(st: &mut State, now: Instant, saved: &[u64]) {
    let ids: HashSet<u64> = st.ed.buffers.iter().map(|b| b.id).collect();
    st.git.gutter.bufs.retain(|id, _| ids.contains(id));
    st.ed.git_hunks.retain(|id, _| ids.contains(id));
    if !st.ed.features.on(Feature::GitGutter) {
        st.git.gutter.peek = None;
        return;
    }
    let base = if st.git.head_base { GutterBase::Head } else { GutterBase::Index };
    let (id, version, path) = {
        let b = st.ed.buf();
        (b.id, b.doc.version, b.doc.path.clone())
    };
    let place = path.as_ref().map_or(Place::Outside, |p| st.git.place(p));
    let Place::In { repo, rel } = place else {
        if place == Place::Outside {
            st.git.gutter.bufs.remove(&id);
            st.ed.git_hunks.remove(&id);
        }
        st.git.gutter.peek = None;
        return;
    };
    let path = path.expect("a file in a repository has a path");
    let g = &mut st.git.gutter;
    let b = g.bufs.entry(id).or_insert_with(|| Buf {
        file: path.clone(),
        repo: repo.clone(),
        rel: rel.clone(),
        base,
        result: None,
        asked: None,
        failed: None,
        seen: version,
        due: None,
        since: None,
        stale: false,
    });
    if b.file != path || b.base != base {
        *b = Buf {
            file: path,
            repo,
            rel,
            base,
            result: None,
            asked: None,
            failed: None,
            seen: version,
            due: None,
            since: None,
            stale: false,
        };
        st.ed.git_hunks.remove(&id);
    }
    if saved.contains(&id) && b.passive() {
        b.stale = true; // a file that was too big, or not text, may be one now
    }
    let first = b.result.is_none() && b.asked.is_none() && b.failed != Some(version);
    if first || (b.stale && b.asked.is_none()) {
        b.due = Some(now);
    } else if version != b.seen && !b.passive() {
        let since = *b.since.get_or_insert(now);
        b.due = Some((now + DEBOUNCE).min(since + MAX_WAIT));
    }
    b.seen = version;
    if b.due.is_some_and(|d| d <= now) {
        send(st, id);
    }
    follow_peek(st);
}

/// Sends the diff request for buffer `id` (its current text).
fn send(st: &mut State, id: u64) {
    let Some(buf) = st.ed.buffers.iter().find(|b| b.id == id) else { return };
    let (version, rope) = (buf.doc.version, buf.doc.rope.clone());
    let super::Git { link, gutter, .. } = &mut st.git;
    let Some(b) = gutter.bufs.get_mut(&id) else { return };
    let req =
        Request::Gutter { repo: b.repo.clone(), path: b.rel.clone(), buffer: id, version, text: rope, base: b.base };
    b.asked = Some((version, link.send(req)));
    (b.due, b.since, b.stale) = (None, None, false);
}

/// Sends what is due. True when it sent something.
pub fn tick(st: &mut State, now: Instant) -> bool {
    let due: Vec<u64> =
        st.git.gutter.bufs.iter().filter(|(_, b)| b.due.is_some_and(|d| d <= now)).map(|(id, _)| *id).collect();
    for &id in &due {
        send(st, id);
    }
    !due.is_empty()
}

/// A diff came back: its hunks become the buffer's markers (an answer older than the one shown is dropped).
pub fn on_gutter(st: &mut State, ticket: Option<Ticket>, r: shrew_git::Result<Gutter>) {
    let g = &mut st.git.gutter;
    match r {
        Ok(res) => {
            let Some(b) = g.bufs.get_mut(&res.buffer) else { return };
            if b.asked.is_some_and(|(_, t)| Some(t) == ticket) {
                b.asked = None;
            }
            if b.result.as_ref().is_some_and(|r| r.version > res.version) || res.path != b.rel {
                return;
            }
            st.ed.git_hunks.insert(res.buffer, to_marks(&res.hunks));
            b.result = Some(res);
        }
        Err(e) => {
            if let Some(b) = g.bufs.values_mut().find(|b| b.asked.is_some_and(|(_, t)| Some(t) == ticket)) {
                b.failed = b.asked.take().map(|(v, _)| v);
            }
            log::debug!("git markers: {e}");
        }
    }
}

/// The peek stays on the hunk at the cursor: it follows `]c`, and closes when the cursor leaves the hunks.
fn follow_peek(st: &mut State) {
    let Some(p) = st.git.gutter.peek else { return };
    let b = st.ed.buf();
    let last = text::last_line(b.rope());
    let line = b.cursor_line();
    let hunks = st.ed.hunks();
    st.git.gutter.peek = if p.buffer != b.id {
        None
    } else {
        match marks::at_line(hunks, line, last) {
            Some(i) => Some(Peek { buffer: p.buffer, line: hunks[i].line(last) }),
            // the markers are being recomputed after an edit: keep it until they are back
            None if st.git.gutter.bufs.get(&p.buffer).is_some_and(|b| b.asked.is_some() || b.due.is_some()) => Some(p),
            None => None,
        }
    };
}

// ----- commands -----

/// [`shrew_core::Event::Git`].
pub fn command(st: &mut State, cmd: Command) {
    match cmd {
        Command::GitPreviewHunk => toggle_peek(st),
        Command::GitStageHunk => start(st, Act::Stage),
        Command::GitUnstageHunk => start(st, Act::Unstage),
        Command::GitRevertHunk => revert(st),
        Command::GitGutterBase => toggle_base(st),
        _ => {}
    }
}

fn toggle_base(st: &mut State) {
    st.git.head_base = !st.git.head_base;
    st.git.gutter.base_changed();
    let v = if st.git.head_base { "head" } else { "index" };
    if let Some(p) = &st.config_path
        && let Err(e) = shrew_config::persist_value(p, "git", "gutter_base", v)
    {
        st.ed.error(format!("could not save [git] gutter_base to {}: {e}", p.display()));
    } else {
        st.ed.info(if st.git.head_base {
            "change markers compare with HEAD (the last commit)"
        } else {
            "change markers compare with the index (staged changes drop out)"
        });
    }
    super::sync(st);
}

/// The active buffer as a command target, or why not.
fn target(st: &mut State) -> Option<Target> {
    let b = st.ed.buf();
    let id = b.id;
    let Some(path) = b.doc.path.clone() else {
        st.ed.error("not a file");
        return None;
    };
    match st.git.place(&path) {
        Place::In { repo, rel } => {
            let (lines, selected) = st.ed.git_target_lines();
            Some(Target { buffer: id, repo, rel, lines, selected })
        }
        Place::Unknown => {
            st.ed.info("looking for the git repository…");
            None
        }
        Place::Outside => {
            st.ed.error("not in a git repository");
            None
        }
    }
}

/// The hunks of `hunks` a command at `t` acts on: those on the selected lines, else the one at the cursor line.
fn selection(hunks: &[Hunk], t: &Target, last: usize) -> LineSel {
    if t.selected {
        return LineSel::new_lines(hunks, t.lines.start as u32..t.lines.end as u32);
    }
    let m = to_marks(hunks);
    match marks::at_line(&m, t.lines.start, last) {
        Some(i) => LineSel::hunks([&hunks[i]]),
        None => LineSel::default(),
    }
}

fn leave_visual(st: &mut State) {
    if st.ed.mode.is_visual() {
        st.ed.set_mode(Mode::Normal);
    }
}

/// Stage or unstage: right away from the markers' index text when there is one, else after the staged diff.
fn start(st: &mut State, act: Act) {
    let Some(t) = target(st) else { return };
    let ready = st
        .git
        .gutter
        .result(t.buffer)
        .filter(|r| r.path == t.rel && r.base.as_ref().is_none_or(|b| b.against == GutterBase::Index));
    if act == Act::Stage
        && let Some(r) = ready
        && matches!(r.file, FileTrack::Tracked | FileTrack::Untracked)
    {
        let index = r.base.as_ref().map(|b| (b.text.clone(), Base::Blob(b.id)));
        return stage_now(st, &t, index);
    }
    let req = Request::FileDiff {
        repo: t.repo.clone(),
        path: t.rel.clone(),
        old_path: None,
        old: Version::Head,
        new: Version::Index,
    };
    let ticket = st.git.link.send(req);
    st.git.gutter.waiting.push(Waiting { ticket, act, target: t });
    leave_visual(st);
}

/// The staged diff for a waiting stage or unstage arrived.
pub fn on_file_diff(st: &mut State, ticket: Option<Ticket>, r: shrew_git::Result<Arc<FileDiff>>) {
    let Some(i) = st.git.gutter.waiting.iter().position(|w| Some(w.ticket) == ticket) else { return };
    let w = st.git.gutter.waiting.remove(i);
    if !st.ed.buffers.iter().any(|b| b.id == w.target.buffer) {
        return;
    }
    let fd = match r {
        Ok(fd) => fd,
        Err(e) => return st.ed.error(format!("git: {e}")),
    };
    if fd.binary {
        return st.ed.error("a binary file: stage or unstage it whole");
    }
    match w.act {
        Act::Stage => {
            let index = fd.new.clone().map(|t| (t, Base::Unchecked));
            stage_now(st, &w.target, index)
        }
        Act::Unstage => unstage_now(st, &w.target, &fd),
    }
}

/// The buffer's text for buffer `id`, and its last line.
fn buffer_text(st: &State, id: u64) -> Option<(Text, usize)> {
    let b = st.ed.buffers.iter().find(|b| b.id == id)?;
    Some((Text::from_rope(&b.doc.rope), text::last_line(&b.doc.rope)))
}

/// Stages the target's lines of the diff index -> buffer (`index`: the staged text and its blob, `None` when the
/// file is not in the index).
fn stage_now(st: &mut State, t: &Target, index: Option<(Text, Base)>) {
    let Some((new, last)) = buffer_text(st, t.buffer) else { return };
    let (old, base) = index.unwrap_or((Text::default(), Base::Absent));
    let hunks = diff::diff(&old, &new, EolMode::Normalize);
    let sel = selection(&hunks, t, last);
    leave_visual(st);
    if sel.is_empty() {
        return st.ed.info(if t.selected { "no unstaged changes on these lines" } else { "no unstaged change here" });
    }
    let n = count(&sel);
    let op = Op::StageLines { path: t.rel.clone(), text: Some(new), sel, base };
    let ticket = st.git.link.send(Request::Op { repo: t.repo.clone(), op });
    st.git.gutter.ops.insert(ticket, format!("staged {n}"));
}

/// Unstages the target's lines from the staged diff HEAD -> index. Buffer lines become index lines through the
/// diff index -> buffer: lines inside an unstaged change take its index side.
fn unstage_now(st: &mut State, t: &Target, fd: &FileDiff) {
    let Some(index) = fd.new.clone() else { return st.ed.info("nothing staged in this file") };
    let Some((new, _)) = buffer_text(st, t.buffer) else { return };
    let head = fd.old_text();
    let staged = diff::diff(&head, &index, EolMode::Exact);
    let unstaged = diff::diff(&index, &new, EolMode::Normalize);
    let (a, z) =
        (to_index_line(&unstaged, t.lines.start as u32, false), to_index_line(&unstaged, t.lines.end as u32, true));
    let sel = if t.selected {
        LineSel::new_lines(&staged, a..z.max(a + 1))
    } else {
        let m = to_marks(&staged);
        match marks::at_line(&m, a as usize, index.len_lines()) {
            Some(i) => LineSel::hunks([&staged[i]]),
            None => LineSel::default(),
        }
    };
    if sel.is_empty() {
        return st.ed.info(if t.selected { "nothing staged on these lines" } else { "no staged change here" });
    }
    let n = count(&sel);
    let ticket = st.git.link.send(Request::Op {
        repo: t.repo.clone(),
        op: Op::UnstageLines { path: t.rel.clone(), sel, base: Base::Unchecked },
    });
    st.git.gutter.ops.insert(ticket, format!("unstaged {n}"));
}

/// Buffer line `line` as a line of the index text, through `hunks` (index -> buffer). A line inside a changed
/// block maps to the block's start (or its end, for the end of a range).
fn to_index_line(hunks: &[Hunk], line: u32, end: bool) -> u32 {
    let mut delta: i64 = 0;
    for h in hunks {
        if line < h.new.start || (end && line == h.new.start) {
            break;
        }
        if line < h.new.end {
            return if end { h.old.end } else { h.old.start };
        }
        delta = h.old.end as i64 - h.new.end as i64;
    }
    (line as i64 + delta).max(0) as u32
}

/// "3 lines" for a message.
fn count(sel: &LineSel) -> String {
    let n: u32 = sel.new.iter().chain(&sel.old).map(|r| r.end - r.start).sum();
    if n == 1 { "1 line".into() } else { format!("{n} lines") }
}

/// An op from here finished.
pub fn on_op_done(st: &mut State, ticket: Option<Ticket>, r: shrew_git::Result<OpOutput>) {
    let Some(what) = ticket.and_then(|t| st.git.gutter.ops.remove(&t)) else { return };
    match r {
        Ok(_) => st.ed.info(what),
        Err(e) if e.kind == shrew_git::ErrorKind::Stale => {
            st.ed.error("the index changed meanwhile: nothing was staged, try again")
        }
        Err(e) => st.ed.error(format!("git: {}", e.message.lines().next().unwrap_or(""))),
    }
}

/// Puts the base version back for the target's lines, as one undoable edit.
fn revert(st: &mut State) {
    let Some(t) = target(st) else { return };
    let Some(base) = st.git.gutter.result(t.buffer).filter(|r| r.path == t.rel).and_then(|r| r.base.clone()) else {
        return st.ed.error("nothing to revert to: the file is new, or its markers are not ready");
    };
    let Some((new, last)) = buffer_text(st, t.buffer) else { return };
    let hunks = diff::diff(&base.text, &new, EolMode::Normalize);
    let sel = selection(&hunks, &t, last);
    leave_visual(st);
    if sel.is_empty() {
        return st.ed.info(if t.selected { "no changes on these lines" } else { "no change here" });
    }
    let target = patch::revert_selection(&base.text, &new, &hunks, &sel, EolMode::Normalize);
    let target = String::from_utf8_lossy(&target).replace("\r\n", "\n");
    let n = count(&sel);
    let b = st.ed.buf_mut();
    if let Some(change) = minimal_change(b.rope(), &target) {
        b.apply(ChangeSet::single(change), None);
        b.commit();
    }
    st.git.gutter.peek = None;
    st.ed.info(format!("reverted {n} (u undoes it)"));
}

/// One change turning `rope` into `target`: the differing middle between their common start and end.
fn minimal_change(rope: &shrew_core::ropey::Rope, target: &str) -> Option<Change> {
    let cur = rope.to_string();
    if cur == target {
        return None;
    }
    let (a, b) = (cur.as_bytes(), target.as_bytes());
    let mut pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !target.is_char_boundary(pre) {
        pre -= 1;
    }
    let room = a.len().min(b.len()) - pre;
    let mut suf = a.iter().rev().zip(b.iter().rev()).take(room).take_while(|(x, y)| x == y).count();
    while !target.is_char_boundary(b.len() - suf) {
        suf -= 1;
    }
    let from = rope.byte_to_char(pre);
    let to = rope.byte_to_char(a.len() - suf);
    Some(Change::replace(from, to, &target[pre..b.len() - suf]))
}

// ----- the peek -----

fn toggle_peek(st: &mut State) {
    if st.git.gutter.peek.take().is_some() {
        return;
    }
    let b = st.ed.buf();
    let (id, line, last) = (b.id, b.cursor_line(), text::last_line(b.rope()));
    let hunks = st.ed.hunks();
    match marks::at_line(hunks, line, last) {
        Some(i) => st.git.gutter.peek = Some(Peek { buffer: id, line: hunks[i].line(last) }),
        None => st.ed.info("no change on this line (]c goes to the next one)"),
    }
}

/// Esc closes the peek (unless a prompt, picker or menu has the keys, or a vim command is half typed).
pub fn key(st: &mut State, k: KeyPress) -> bool {
    let ed = &st.ed;
    let free = ed.prompt.is_none() && ed.picker.is_none() && ed.about.is_none() && !ed.sidebar_focus;
    if st.git.gutter.peek.is_some()
        && k.key == Key::Esc
        && k.mods == Default::default()
        && free
        && ed.pending_keys().is_empty()
        && !ed.mode.is_visual()
    {
        st.git.gutter.peek = None;
        return true;
    }
    false
}

/// Clicks: the peek's buttons, then a marker in the gutter (opens the peek on its hunk).
pub fn press(st: &mut State, hits: &Hits, lay: &Layout, scroll: Scroll, x: f32, y: f32) -> bool {
    if let Some(&(_, b)) = hits.peek_buttons.iter().find(|(r, _)| r.contains(x, y)) {
        match b {
            PeekButton::Stage => start(st, Act::Stage),
            PeekButton::Revert => revert(st),
            PeekButton::Close => st.git.gutter.peek = None,
        }
        st.after_input();
        return true;
    }
    if hits.peek.is_some_and(|r| r.contains(x, y)) {
        return true; // a click inside the peek stays there
    }
    let hunks = st.ed.hunks();
    if hunks.is_empty() || !lay.text.contains(x, y) || st.ed.picker.is_some() || st.settings.is_open() {
        return false;
    }
    let (mx, mw) = marker_x(lay, st.ed.features.on(Feature::LineNumbers));
    if x < mx - lay.cw * 0.4 || x > mx + mw + lay.cw * 0.4 {
        return false;
    }
    let b = st.ed.buf();
    let last = text::last_line(b.rope());
    let line = scroll.line_at(lay, y);
    // a deletion's triangle sits on the line boundary: the line just below it counts too
    let line = if line < 0 { return false } else { line as usize };
    let Some(i) = marks::at_line(hunks, line.min(last), last)
        .or_else(|| hunks.iter().position(|h| h.kind() == HunkKind::Deleted && h.new.start as usize == line))
    else {
        return false;
    };
    let target = hunks[i].line(last);
    let id = b.id;
    let open = st.git.gutter.peek == Some(Peek { buffer: id, line: target });
    if b.cursor_line() != target {
        // the cursor goes to the hunk without scrolling (it is on screen: it was clicked)
        if st.ed.mode.is_visual() {
            st.ed.set_mode(Mode::Normal);
        }
        let b = st.ed.buf_mut();
        b.sel = shrew_core::Selection::point(text::first_non_blank(&b.doc.rope, target));
    }
    st.ed.sidebar_focus = false;
    st.git.gutter.peek = (!open).then_some(Peek { buffer: id, line: target });
    st.after_input();
    true
}

// ----- drawing -----

/// The marker column: x and width (right of the line numbers, or in the narrow gutter without them).
fn marker_x(lay: &Layout, numbers: bool) -> (f32, f32) {
    let w = (3.0 * lay.scale).round().max(2.0);
    let x = if numbers { lay.text.x + (lay.digits as f32 + 1.35) * lay.cw } else { lay.text.x + 0.45 * lay.cw };
    (x.round(), w)
}

fn kind_color(k: HunkKind, c: &Colors) -> Color {
    match k {
        HunkKind::Added => c.git_added,
        HunkKind::Modified => c.git_modified,
        HunkKind::Deleted => c.git_deleted,
    }
}

/// A right-pointing triangle centred on `cy`, `h` tall, its flat side at `x`, from one-pixel slices.
fn triangle(scene: &mut Scene, x: f32, cy: f32, h: f32, w: f32, col: Color) {
    let n = h.round().max(3.0) as i32;
    let top = (cy - n as f32 / 2.0).round();
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        let len = (w * (1.0 - (2.0 * t - 1.0).abs())).max(1.0);
        scene.rect(Rect::new(x, top + i as f32, len.round().max(1.0), 1.0), col);
    }
}

/// The markers of the visible lines `[first, end)`, in the gutter (inside its clip).
pub fn draw_marks(scene: &mut Scene, st: &State, lay: &Layout, c: &Colors, scroll: Scroll, first: usize, end: usize) {
    let hunks = st.ed.hunks();
    if hunks.is_empty() {
        return;
    }
    let last = text::last_line(st.ed.buf().rope());
    let (x, w) = marker_x(lay, st.ed.features.on(Feature::LineNumbers));
    let gap = lay.scale.round().max(1.0);
    let from = hunks.partition_point(|h| h.lines(last).end <= first);
    for h in hunks[from..].iter().take_while(|h| h.lines(last).start < end) {
        let col = kind_color(h.kind(), c);
        if h.kind() == HunkKind::Deleted {
            let start = h.new.start as usize;
            let y = if start > last { scroll.line_y(lay, last) + lay.ch } else { scroll.line_y(lay, start) };
            triangle(scene, x - gap, y, (lay.ch * 0.5).max(6.0 * lay.scale), w * 2.2, col);
            continue;
        }
        let l = h.lines(last);
        let (ya, yz) = (scroll.line_y(lay, l.start), scroll.line_y(lay, l.end));
        scene.rect(Rect::new(x, ya + gap, w, (yz - ya - 2.0 * gap).max(gap)), col);
    }
}

/// Marks along the scrollbar track: where the changes are in the whole file.
pub fn draw_scrollbar(scene: &mut Scene, st: &State, lay: &Layout, c: &Colors, total: usize) {
    let hunks = st.ed.hunks();
    if hunks.is_empty() || total <= lay.rows {
        return;
    }
    let track = lay.scrollbar;
    let last = total - 1;
    let w = (3.0 * lay.scale).round().max(2.0);
    let min_h = (2.0 * lay.scale).round().max(1.0);
    let mut prev: Option<(f32, Color)> = None;
    for h in hunks {
        let l = h.lines(last);
        let y = (track.y + l.start as f32 / total as f32 * track.h).round();
        let hh = ((l.end - l.start) as f32 / total as f32 * track.h).round().max(min_h);
        let col = kind_color(h.kind(), c);
        // thousands of hunks: one rect per pixel row and colour
        if prev == Some((y, col)) {
            continue;
        }
        prev = Some((y, col));
        scene.rect(Rect::new(track.x + lay.scale.round(), y, w, hh), col);
    }
}

/// Marks at the minimap's left edge.
pub fn draw_minimap(scene: &mut Scene, st: &State, g: &crate::minimap::Geometry, lay: &Layout, c: &Colors) {
    let hunks = st.ed.hunks();
    if hunks.is_empty() {
        return;
    }
    let shown = g.lines();
    let last = g.total.saturating_sub(1);
    let w = (2.0 * lay.scale).round().max(1.0);
    let from = hunks.partition_point(|h| h.lines(last).end <= shown.start);
    for h in hunks[from..].iter().take_while(|h| h.lines(last).start < shown.end) {
        let l = h.lines(last);
        let col = kind_color(h.kind(), c);
        let (y, hh) = if h.kind() == HunkKind::Deleted {
            let y = g.line_y(h.new.start as f32);
            ((y - g.line_px / 2.0).round(), g.line_px.max(1.0))
        } else {
            let y = g.line_y(l.start as f32);
            (y.round(), (g.line_y(l.end as f32) - y).round().max(1.0))
        };
        scene.rect(Rect::new(g.rect.x, y, w, hh), col);
    }
}

/// One row of the peek: a deleted or added line, tab-expanded, with its changed words as display columns.
struct Row {
    added: bool,
    text: String,
    words: Vec<(usize, usize)>,
}

/// `line` with tabs expanded, and the display column of each byte (plus one past the end).
fn expand(line: &str, tw: usize) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(line.len());
    let mut cols = Vec::with_capacity(line.len() + 1);
    let mut col = 0;
    for ch in line.chars() {
        let w = if ch == '\t' { tw - col % tw } else { char_columns(ch) };
        cols.extend(std::iter::repeat_n(col, ch.len_utf8()));
        match ch {
            '\t' => out.extend(std::iter::repeat_n(' ', w)),
            c if c.is_control() => out.push('\u{fffd}'),
            c => out.push(c),
        }
        col += w;
    }
    cols.push(col);
    (out, cols)
}

/// The peek's rows for `hunk` of the buffer against `base`: removed lines, then added ones.
fn peek_rows(base: &Text, rope: &shrew_core::ropey::Rope, hunk: &Hunk, tw: usize) -> Vec<Row> {
    // small texts holding just the hunk's lines, so the word diff never touches the whole file
    let old = Text::new(base.hunk_bytes(&hunk.old).to_vec());
    let mut new_bytes = Vec::new();
    for l in hunk.new.clone() {
        if (l as usize) < rope.len_lines() {
            new_bytes.extend(rope.line(l as usize).bytes());
        }
    }
    let new = Text::new(new_bytes);
    let local = Hunk { old: 0..old.len_lines() as u32, new: 0..new.len_lines() as u32 };
    let words =
        if local.old.len() + local.new.len() <= 400 { diff::word_diff(&old, &new, &local) } else { Default::default() };
    let mut rows = Vec::new();
    for (side, t, spans) in [(false, &old, &words.old), (true, &new, &words.new)] {
        for i in 0..t.len_lines() {
            let s = String::from_utf8_lossy(t.line_content(i));
            let (text, cols) = expand(&s, tw);
            let w = spans
                .iter()
                .filter(|sp| sp.line as usize == i)
                .map(|sp| (cols[(sp.start as usize).min(cols.len() - 1)], cols[(sp.end as usize).min(cols.len() - 1)]))
                .collect();
            rows.push(Row { added: side, text, words: w });
        }
    }
    rows
}

/// The hunk peek under its hunk (above it when there is no room), in layer 1 over the text.
pub fn draw_peek(scene: &mut Scene, st: &State, hits: &mut Hits, lay: &Layout, c: &Colors, scroll: Scroll) {
    let Some(p) = st.git.gutter.peek else { return };
    let b = st.ed.buf();
    if p.buffer != b.id {
        return;
    }
    let Some(res) = st.git.gutter.result(b.id) else { return };
    let Some(base) = &res.base else { return };
    let rope = b.rope();
    let last = text::last_line(rope);
    let m = st.ed.hunks();
    let Some(i) = marks::at_line(m, p.line, last) else { return };
    let Some(hunk) = res.hunks.get(i) else { return };
    let tw = st.ed.settings.tab_width.max(1);
    let rows = peek_rows(&base.text, rope, hunk, tw);
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let shown = rows.len().min(PEEK_ROWS);
    let more = rows.len() - shown;
    let head_h = (ch + 2.0 * pad).round();
    let h = (head_h + (shown + usize::from(more > 0)) as f32 * ch + pad).round();
    let area = lay.text_rect();
    let x = (area.x + cw).round();
    let w = (area.w - 2.0 * cw).min(140.0 * cw).round();
    if w < 20.0 * cw {
        return;
    }
    let lines = m[i].lines(last);
    let below = scroll.line_y(lay, lines.end) + pad / 2.0;
    let above = scroll.line_y(lay, lines.start) - h - pad / 2.0;
    let y = if below + h <= lay.text.bottom() || above < lay.text.y { below } else { above };
    let y = y.max(lay.text.y + pad).round();
    let panel = Rect::new(x, y, w, h);
    scene.push_clip(lay.text);
    crate::draw::popup(scene, panel, lay, c);
    hits.peek = Some(panel);
    scene.push_clip(Rect::new(x + 1.0, y + 1.0, w - 2.0, h - 2.0));

    // header: which change, its size, the buttons
    let ty = lay.centered(y, head_h);
    let what = match m[i].kind() {
        HunkKind::Added => "Added",
        HunkKind::Modified => "Changed",
        HunkKind::Deleted => "Deleted",
    };
    let title = format!("{what} · {} of {}", i + 1, m.len());
    let mut tx = x + 2.0 * cw;
    scene.text(tx, ty, &title, TextStyle::new(c.panel_foreground).bold(), &[]);
    tx += (columns(&title) + 2) as f32 * cw;
    for (s, col) in [(format!("−{}", hunk.removed()), c.git_deleted), (format!("+{}", hunk.added()), c.git_added)] {
        if s.len() > 2 || !s.ends_with('0') {
            scene.text(tx, ty, &s, TextStyle::new(col), &[]);
            tx += (columns(&s) + 1) as f32 * cw;
        }
    }
    let mut bx = x + w - cw;
    let radius = (4.0 * lay.scale).round();
    let bh = (ch + pad).round();
    let by = (y + (head_h - bh) / 2.0).round();
    let against = if res.base.as_ref().is_some_and(|b| b.against == GutterBase::Head) { "HEAD" } else { "index" };
    for (label, button) in [("×", PeekButton::Close), ("Revert", PeekButton::Revert), ("Stage", PeekButton::Stage)] {
        let bw = ((columns(label) + 2) as f32 * cw).round();
        bx -= bw;
        let r = Rect::new(bx, by, bw, bh);
        if button != PeekButton::Close {
            scene.rounded_rect(r, radius, c.panel_selection, None);
        }
        let ink = if button == PeekButton::Close { c.panel_muted } else { c.panel_foreground };
        scene.text((bx + cw).round(), ty, label, TextStyle::new(ink), &[]);
        hits.peek_buttons.push((r, button));
        bx -= cw / 2.0;
    }
    let hint = format!("vs {against} · ghs ghr · Esc");
    let hx = bx - (columns(&hint) + 1) as f32 * cw;
    if hx > tx + cw {
        scene.text(hx.round(), ty, &hint, TextStyle::new(c.panel_muted), &[]);
    }

    // the lines
    let body_x = x + 1.0;
    let body_w = w - 2.0;
    let sign_x = x + cw;
    let text_x = x + 3.0 * cw;
    let cols = ((w - 4.0 * cw) / cw).max(1.0) as usize;
    for (n, row) in rows.iter().take(shown).enumerate() {
        let ry = y + head_h + n as f32 * ch;
        let (bg, word, sign, sign_col) = if row.added {
            (c.diff_add_bg, c.diff_add_word, "+", c.git_added)
        } else {
            (c.diff_del_bg, c.diff_del_word, "−", c.git_deleted)
        };
        scene.rect(Rect::new(body_x, ry, body_w, ch), bg);
        for &(a, z) in &row.words {
            let (a, z) = (a.min(cols), z.min(cols));
            if z > a {
                scene.rounded_rect(
                    Rect::new(text_x + a as f32 * cw, ry, (z - a) as f32 * cw, ch),
                    radius / 2.0,
                    word,
                    None,
                );
            }
        }
        scene.text(sign_x, ry, sign, TextStyle::new(sign_col).bold(), &[]);
        let t = crate::draw::elide_end(&row.text, cols);
        scene.text(text_x, ry, &t, TextStyle::new(c.foreground), &[]);
    }
    if more > 0 {
        let ry = y + head_h + shown as f32 * ch;
        let s = format!("… {more} more line{}", if more == 1 { "" } else { "s" });
        scene.text(text_x, ry, &s, TextStyle::new(mix(c.panel_muted, c.popup, 0.8)), &[]);
    }
    scene.pop_clip();
    scene.pop_clip();
}

#[cfg(test)]
mod tests {
    use super::*;
    use shrew_core::KeyPress;

    fn h(old: std::ops::Range<u32>, new: std::ops::Range<u32>) -> Hunk {
        Hunk { old, new }
    }

    #[test]
    fn buffer_lines_map_to_index_lines() {
        // index -> buffer: 2 lines added at 3, line 10 changed into 3 lines, 2 lines deleted before buffer line 20
        let hunks = [h(3..3, 3..5), h(8..9, 10..13), h(14..16, 19..19)];
        assert_eq!(to_index_line(&hunks, 0, false), 0);
        assert_eq!(to_index_line(&hunks, 3, false), 3, "an added line: where it would go");
        assert_eq!(to_index_line(&hunks, 4, true), 3);
        assert_eq!(to_index_line(&hunks, 5, false), 3);
        assert_eq!(to_index_line(&hunks, 9, false), 7);
        assert_eq!(to_index_line(&hunks, 11, false), 8, "inside a change: its start");
        assert_eq!(to_index_line(&hunks, 11, true), 9, "or its end");
        assert_eq!(to_index_line(&hunks, 13, false), 9);
        assert_eq!(to_index_line(&hunks, 19, false), 16, "after the deletion");
        assert_eq!(to_index_line(&[], 7, false), 7);
    }

    #[test]
    fn reverting_is_a_minimal_edit() {
        let rope = shrew_core::ropey::Rope::from_str("a\nnew é\nb\nc\n");
        let c = minimal_change(&rope, "a\nold é\nb\nc\n").unwrap();
        assert_eq!((c.from, c.to, c.text.as_str()), (2, 5, "old"));
        let c = minimal_change(&rope, "a\nb\nc\n").unwrap();
        let mut r = rope.clone();
        ChangeSet::single(c).apply(&mut r);
        assert_eq!(r.to_string(), "a\nb\nc\n");
        // multi-byte chars at the edges stay whole
        let rope = shrew_core::ropey::Rope::from_str("xé\n");
        let c = minimal_change(&rope, "xè\n").unwrap();
        let mut r = rope.clone();
        ChangeSet::single(c).apply(&mut r);
        assert_eq!(r.to_string(), "xè\n");
        assert!(minimal_change(&rope, "xé\n").is_none());
    }

    #[test]
    fn peek_rows_mark_the_changed_words() {
        let base = Text::new(b"one\nlet x = 1;\nthree\n".to_vec());
        let rope = shrew_core::ropey::Rope::from_str("one\nlet x = 2;\n\tadded\nthree\n");
        let hunks = diff::diff(&base, &Text::from_rope(&rope), EolMode::Normalize);
        assert_eq!(hunks, vec![h(1..2, 1..3)]);
        let rows = peek_rows(&base, &rope, &hunks[0], 4);
        assert_eq!(rows.len(), 3);
        assert!(!rows[0].added && rows[1].added && rows[2].added);
        assert_eq!(rows[0].text, "let x = 1;");
        assert_eq!(rows[2].text, "    added", "tabs expanded");
        assert_eq!(rows[0].words, vec![(8, 9)], "the 1");
        assert_eq!(rows[1].words, vec![(8, 9)], "the 2");
    }

    /// Edits wait for a quiet moment (newest wins), but not longer than MAX_WAIT while typing on.
    #[test]
    fn edits_are_debounced() {
        let mut st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
        let path = PathBuf::from("/nowhere/repo/x.txt");
        st.git.files.insert(path.clone(), Place::In { repo: "/nowhere/repo".into(), rel: "x.txt".into() });
        st.ed.buf_mut().doc.path = Some(path);
        st.after_input();
        let id = st.ed.buf().id;
        let b = &st.git.gutter.bufs[&id];
        assert!(b.asked.is_some() && b.due.is_none(), "a new file is diffed at once");
        // its answer
        let b = st.git.gutter.bufs.get_mut(&id).unwrap();
        b.asked = None;
        b.result = Some(Gutter {
            buffer: id,
            version: 0,
            path: "x.txt".into(),
            hunks: Vec::new(),
            base: None,
            file: FileTrack::Tracked,
            elapsed: Duration::ZERO,
        });
        let t0 = Instant::now();
        for k in KeyPress::parse_seq("iab") {
            st.key(k);
        }
        let b = &st.git.gutter.bufs[&id];
        assert!(b.asked.is_none(), "typing does not diff on every key");
        let due = b.due.expect("a diff is due");
        assert!(due > t0 && due <= Instant::now() + DEBOUNCE);
        assert!(!tick(&mut st, t0), "not yet");
        assert!(tick(&mut st, due + Duration::from_millis(1)));
        let b = &st.git.gutter.bufs[&id];
        assert_eq!(b.asked.map(|a| a.0), Some(st.ed.buf().doc.version), "the newest version");
        assert!(b.due.is_none());
        // typing on and on: still a diff every MAX_WAIT
        st.git.gutter.bufs.get_mut(&id).unwrap().asked = None;
        let start = Instant::now();
        st.key(KeyPress::ch('c'));
        let since = st.git.gutter.bufs[&id].since.unwrap();
        assert!(since >= start);
        for _ in 0..5 {
            st.key(KeyPress::ch('d'));
        }
        assert!(st.git.gutter.bufs[&id].due.unwrap() <= since + MAX_WAIT);
    }

    #[test]
    fn expanding_tracks_columns() {
        let (s, cols) = expand("\tab日c", 4);
        assert_eq!(s, "    ab日c");
        assert_eq!(cols, vec![0, 4, 5, 6, 6, 6, 8, 9]);
    }
}
