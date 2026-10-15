//! Blame (`git_blame`, again to hide it): a column left of the text of the active file. Each run of lines from
//! one commit shows the commit's short id, author and age once, and a bar coloured by age down its side (the
//! newest commits in the accent, the oldest faded). A click on a run, or Enter in normal mode, opens that commit
//! in the log tab. The file is blamed as it is in the buffer (unsaved lines show as not committed yet); after an
//! edit the column dims until the next save blames it again.

use super::{Purpose, log, on};
use crate::colors::{Colors, mix};
use crate::draw::elide_end;
use crate::layout::{Layout, Scroll};
use crate::state::State;
use shrew_core::{Key, KeyPress, Mode, Mods, text};
use shrew_git::{BlameHunk, Oid, Request};
use shrew_render::{Rect, Scene, TextStyle, columns};
use std::path::PathBuf;

/// The column's width in cells.
pub const COLS: usize = 30;

pub struct Blame {
    pub buffer: u64,
    pub repo: PathBuf,
    pub path: String,
    /// The buffer version blamed.
    pub version: u64,
    /// Sorted by start line.
    pub hunks: Vec<BlameHunk>,
    pub done: bool,
    pub error: Option<String>,
    /// Oldest and newest commit times, for the age colours.
    range: (i64, i64),
}

#[derive(Default)]
pub struct Blames {
    list: Vec<Blame>,
    /// Runs drawn in the last frame: rect and commit.
    pub hits: Vec<(Rect, Oid)>,
}

impl Blames {
    pub fn get(&self, buffer: u64) -> Option<&Blame> {
        self.list.iter().find(|b| b.buffer == buffer)
    }

    pub fn add(&mut self, buffer: u64, hunks: Vec<BlameHunk>) {
        let Some(b) = self.list.iter_mut().find(|b| b.buffer == buffer) else { return };
        for h in &hunks {
            let t = h.commit.committer.time;
            if !h.commit.is_uncommitted() {
                b.range =
                    if b.hunks.is_empty() && b.range == (0, 0) { (t, t) } else { (b.range.0.min(t), b.range.1.max(t)) };
            }
        }
        b.hunks.extend(hunks);
        b.hunks.sort_by_key(|h| h.start);
    }

    pub fn done(&mut self, buffer: u64, r: Result<(), String>) {
        if let Some(b) = self.list.iter_mut().find(|b| b.buffer == buffer) {
            b.done = true;
            b.error = r.err();
        }
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }
}

impl Blame {
    /// The run holding `line`.
    pub fn at(&self, line: usize) -> Option<&BlameHunk> {
        let line = line as u32;
        let i = self.hunks.partition_point(|h| h.start + h.len <= line);
        self.hunks.get(i).filter(|h| h.start <= line && line < h.start + h.len)
    }

    /// 0 for the oldest commit, 1 for the newest.
    fn age(&self, time: i64) -> f32 {
        let (a, z) = self.range;
        if z <= a { 1.0 } else { ((time - a) as f32 / (z - a) as f32).clamp(0.0, 1.0) }
    }
}

fn active_blame(st: &State) -> Option<&Blame> {
    if !on(&st.ed) || log::active(st).is_some() {
        return None;
    }
    st.git.blame.get(st.ed.buf().id)
}

/// Turns blame on or off for the active file.
pub fn toggle(st: &mut State, repo: PathBuf) {
    let id = st.ed.buf().id;
    if st.git.blame.list.iter().any(|b| b.buffer == id) {
        st.git.blame.list.retain(|b| b.buffer != id);
        st.git.cancel_tickets(|p| !matches!(p, Purpose::Blame(b) if *b == id));
        return st.ed.info("blame off");
    }
    let Some(path) = super::active_rel(st) else {
        return st.ed.error("blame: the active buffer is not a file in this repository");
    };
    st.git.blame.list.push(Blame {
        buffer: id,
        repo,
        path,
        version: 0,
        hunks: Vec::new(),
        done: false,
        error: None,
        range: (0, 0),
    });
    request(st, id);
    st.ed.info("blame: click a commit (or Enter on a line) to open it; git_blame again hides it");
}

fn request(st: &mut State, id: u64) {
    let Some(b) = st.ed.buffers.iter().find(|b| b.id == id) else { return };
    let (version, dirty) = (b.doc.version, b.doc.dirty);
    let contents = dirty.then(|| b.doc.rope.clone());
    let Some(bl) = st.git.blame.list.iter_mut().find(|x| x.buffer == id) else { return };
    bl.version = version;
    bl.hunks.clear();
    bl.done = false;
    bl.error = None;
    bl.range = (0, 0);
    let req = Request::Blame { repo: bl.repo.clone(), path: bl.path.clone(), rev: None, contents };
    st.git.send(req, Purpose::Blame(id));
}

/// Drops blame of closed buffers and blames a saved file again.
pub fn after_input(st: &mut State) {
    if st.git.blame.list.is_empty() {
        return;
    }
    let ed = &st.ed;
    st.git.blame.list.retain(|b| ed.buffers.iter().any(|x| x.id == b.buffer && x.doc.path.is_some()));
    let again: Vec<u64> = st
        .git
        .blame
        .list
        .iter()
        .filter(|b| st.ed.buffers.iter().any(|x| x.id == b.buffer && !x.doc.dirty && x.doc.version != b.version))
        .map(|b| b.buffer)
        .collect();
    for id in again {
        request(st, id);
    }
}

/// A commit moved HEAD: blame again.
pub fn head_moved(st: &mut State) {
    let ids: Vec<u64> = st.git.blame.list.iter().map(|b| b.buffer).collect();
    for id in ids {
        request(st, id);
    }
}

/// The column, taken from the left of the text (after the sidebar and the terminal panel are placed).
pub fn carve(mut lay: Layout, st: &State) -> Layout {
    if active_blame(st).is_none() {
        return lay;
    }
    let w = (COLS as f32 * lay.cw).round();
    if lay.text.w - w < 30.0 * lay.cw {
        return lay;
    }
    lay.text.x += w;
    lay.text.w -= w;
    lay.cols = (((lay.text.w - lay.gutter_w - lay.scrollbar.w) / lay.cw).floor() as usize).max(1);
    lay
}

pub fn draw(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors, scroll: Scroll) {
    let Some(b) = active_blame(st) else { return };
    let (cw, ch) = (lay.cw, lay.ch);
    let w = (COLS as f32 * cw).round();
    let col = Rect::new(lay.text.x - w, lay.text.y, w, lay.text.h);
    if col.x < lay.sidebar.right() - 0.5 {
        return; // the text was too narrow to give it room
    }
    let doc = &st.ed.buf().doc;
    let stale = doc.version != b.version;
    scene.rect(col, mix(c.panel_background, c.background, 0.5));
    scene.rect(
        Rect::new(col.right() - lay.scale.round().max(1.0), col.y, lay.scale.round().max(1.0), col.h),
        c.panel_border,
    );
    scene.push_clip(col);
    let first = scroll.first_line();
    let last = text::last_line(&doc.rope);
    let rows = lay.rows + 2;
    if b.hunks.is_empty() {
        let msg = match &b.error {
            Some(e) => elide_end(e, COLS - 2).into_owned(),
            None => "blaming…".into(),
        };
        let col_c = if b.error.is_some() { c.error } else { c.panel_muted };
        scene.text(col.x + cw, scroll.line_y(lay, first.min(last)), &msg, TextStyle::new(col_c), &[]);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let bar = (3.0 * lay.scale).round().max(2.0);
    let mut hits = Vec::new();
    let mut line = first;
    while line <= last && line < first + rows {
        let Some(h) = b.at(line) else {
            line += 1;
            continue;
        };
        let end = ((h.start + h.len) as usize).min(last + 1).min(first + rows);
        let y0 = scroll.line_y(lay, line);
        let y1 = scroll.line_y(lay, end);
        let uncommitted = h.commit.is_uncommitted();
        let age = b.age(h.commit.committer.time);
        let faded = mix(c.panel_muted, c.background, 0.45);
        let tone = if uncommitted { c.warning } else { mix(c.accent, faded, 0.1 + 0.9 * age) };
        let tone = if stale { mix(tone, c.background, 0.5) } else { tone };
        scene.rect(Rect::new(col.x, y0, bar, y1 - y0), tone);
        if line as u32 == h.start || line == first {
            // a thin line between runs
            if line as u32 == h.start && line > 0 {
                scene.rect(
                    Rect::new(col.x + bar, y0, w - bar, lay.scale.round().max(1.0)),
                    mix(c.panel_border, c.background, 0.5),
                );
            }
            let fg = if stale { c.panel_muted } else { c.panel_foreground };
            if uncommitted {
                scene.text(col.x + bar + cw, y0, "not committed yet", TextStyle::new(c.panel_muted).italic(), &[]);
            } else {
                let id = h.commit.id.short();
                let ago = super::ago_short(h.commit.committer.time, now);
                let author_cols = COLS.saturating_sub(id.len() + ago.len() + 4);
                let author = elide_end(&h.commit.author.name, author_cols);
                scene.text(col.x + bar + cw, y0, &id, TextStyle::new(tone), &[]);
                let ax = col.x + bar + (id.len() + 2) as f32 * cw;
                scene.text(ax, y0, &author, TextStyle::new(fg), &[]);
                let gx = col.right() - (columns(&ago) + 1) as f32 * cw;
                scene.text(gx.round(), y0, &ago, TextStyle::new(c.panel_muted), &[]);
                // a second line: what the commit was about
                if line + 1 < end {
                    let summary = elide_end(&h.commit.summary, COLS - 2);
                    let sy = scroll.line_y(lay, line + 1);
                    scene.text(
                        col.x + bar + cw,
                        sy,
                        &summary,
                        TextStyle::new(mix(c.panel_muted, c.background, 0.7)),
                        &[],
                    );
                }
            }
        }
        if !uncommitted {
            hits.push((Rect::new(col.x, y0, w, (y1 - y0).max(ch)), h.commit.id));
        }
        line = end.max(line + 1);
    }
    scene.pop_clip();
    st.git.blame.hits = hits;
}

/// Opens a commit in the log tab, with its details showing.
pub fn open_commit(st: &mut State, repo: PathBuf, id: Oid) {
    log::open(st, repo, false);
    let view = st.ed.buf().id;
    log::show_commit(st, view, id);
}

pub fn press(st: &mut State, x: f32, y: f32) -> bool {
    let Some(&(_, id)) = st.git.blame.hits.iter().find(|(r, _)| r.contains(x, y)) else { return false };
    let Some(repo) = active_blame(st).map(|b| b.repo.clone()) else { return false };
    open_commit(st, repo, id);
    true
}

/// Enter in normal mode on a blamed line opens its commit.
pub fn key(st: &mut State, k: KeyPress) -> bool {
    let ed = &st.ed;
    if k.key != Key::Enter
        || k.mods != Mods::default()
        || ed.mode != Mode::Normal
        || !ed.vim()
        || ed.mapping_pending()
        || !ed.pending_keys().is_empty()
    {
        return false;
    }
    let Some(b) = active_blame(st) else { return false };
    let line = text::line_of(ed.buf().rope(), ed.buf().sel.primary().head);
    let Some(h) = b.at(line).filter(|h| !h.commit.is_uncommitted()) else { return false };
    let (repo, id) = (b.repo.clone(), h.commit.id);
    open_commit(st, repo, id);
    true
}
