//! One diff tab's state: what it compares, its files ("sections") with their texts and changes, the rows they lay
//! out as (shrew-core's [`model`]), the cursor and the scroll position. No drawing and no git requests here.

use super::{DiffSpec, Rev};
use crate::anim::Anim;
use shrew_core::diffview::{self as model, Layout, Options, Row, RowKind, Rows};
use shrew_git::diff::{EolMode, Hunk, WordDiff};
use shrew_git::ropey::Rope;
use shrew_git::{Base, FileState, Text};
use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

/// A side's text: bytes from git, or a snapshot of an open buffer (a live working-tree side).
#[derive(Clone)]
pub enum SideText {
    Bytes(Text),
    Rope(Rope),
}

impl Default for SideText {
    fn default() -> Self {
        SideText::Bytes(Text::default())
    }
}

impl SideText {
    /// Lines as git counts them: a final newline starts no extra line.
    pub fn len_lines(&self) -> usize {
        match self {
            SideText::Bytes(t) => t.len_lines(),
            SideText::Rope(r) => rope_lines(r),
        }
    }

    /// Line `i` without its line ending (lossy for bytes that are not UTF-8).
    pub fn line(&self, i: usize) -> Cow<'_, str> {
        match self {
            SideText::Bytes(t) if i < t.len_lines() => String::from_utf8_lossy(t.line_content(i)),
            SideText::Rope(r) if i < rope_lines(r) => {
                let mut s: Cow<str> = r.line(i).into();
                let trimmed = s.trim_end_matches(['\n', '\r']).len();
                if trimmed != s.len() {
                    s.to_mut().truncate(trimmed);
                }
                s
            }
            _ => Cow::Borrowed(""),
        }
    }

    /// Lines `range` (with their endings) as a text of their own, for word diffs and patches.
    pub fn lines_text(&self, range: Range<u32>) -> Text {
        match self {
            SideText::Bytes(t) => Text::new(t.hunk_bytes(&range).to_vec()),
            SideText::Rope(r) => {
                let n = rope_lines(r) as u32;
                let (a, z) = (range.start.min(n) as usize, range.end.min(n) as usize);
                let (ca, cz) = (r.line_to_char(a), if z >= r.len_lines() { r.len_chars() } else { r.line_to_char(z) });
                Text::new(r.slice(ca..cz).to_string().into_bytes())
            }
        }
    }

    /// The whole text (a copy for a rope).
    pub fn to_text(&self) -> Text {
        match self {
            SideText::Bytes(t) => t.clone(),
            SideText::Rope(r) => Text::from_rope(r),
        }
    }

    #[cfg(feature = "syntax")]
    pub fn bytes_len(&self) -> usize {
        match self {
            SideText::Bytes(t) => t.bytes().len(),
            SideText::Rope(r) => r.len_bytes(),
        }
    }
}

/// Lines of a rope in git's counting.
pub fn rope_lines(r: &Rope) -> usize {
    let n = r.len_lines();
    let chars = r.len_chars();
    if chars == 0 || r.char(chars - 1) == '\n' { n - 1 } else { n }
}

/// A file's texts and changes, once loaded.
pub struct Data {
    pub old: SideText,
    pub new: SideText,
    pub hunks: Vec<Hunk>,
    pub eol: EolMode,
    /// The staged version these hunks were computed from, for staging lines safely.
    pub base: Base,
    /// The buffer version the new side is (a live side).
    pub version: Option<u64>,
}

/// An open buffer standing in for the working-tree file: edits show in the diff as you type.
pub struct Live {
    /// The buffer's id.
    pub buffer: u64,
    /// The last version sent to the service, and the snapshots its answers refer to (newest last).
    pub sent: Option<u64>,
    pub snapshots: Vec<(u64, Rope)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    /// Nothing asked yet (a collapsed file in a multi-file diff is loaded when opened).
    Idle,
    Waiting,
    Done,
}

/// One file of the diff.
pub struct Section {
    /// Repository path on the new side.
    pub path: String,
    /// The old side's path after a rename.
    pub old_path: Option<String>,
    /// What happened to it (multi-file diffs).
    pub state: Option<FileState>,
    /// Lines added and removed: from the file list until loaded, then from the hunks.
    pub stats: (u32, u32),
    pub binary: bool,
    pub collapsed: bool,
    pub unfolded: BTreeSet<u32>,
    pub data: Option<Data>,
    /// Shown instead of lines: loading, binary, an error.
    pub note: Option<String>,
    pub load: Load,
    /// Bumped by every load and setting change; late results of older ones are dropped.
    pub generation: u64,
    /// Word-level changes per hunk, computed when first shown (or handed over with the file).
    pub words: HashMap<u32, Arc<WordDiff>>,
    pub live: Option<Live>,
    /// The two sides' syntax highlighters, made when the file is first on screen.
    #[cfg(feature = "syntax")]
    pub hl: [Option<shrew_syntax::Highlighter>; 2],
    #[cfg(feature = "syntax")]
    pub hl_tried: bool,
}

impl Section {
    pub fn new(path: String) -> Section {
        Section {
            path,
            old_path: None,
            state: None,
            stats: (0, 0),
            binary: false,
            collapsed: false,
            unfolded: BTreeSet::new(),
            data: None,
            note: None,
            load: Load::Idle,
            generation: 0,
            words: HashMap::new(),
            live: None,
            #[cfg(feature = "syntax")]
            hl: [None, None],
            #[cfg(feature = "syntax")]
            hl_tried: false,
        }
    }

    /// New data: the words and highlighters of the old one no longer fit.
    pub fn set_data(&mut self, data: Data) {
        let (mut added, mut removed) = (0, 0);
        for h in &data.hunks {
            added += h.added();
            removed += h.removed();
        }
        self.stats = (added, removed);
        self.words.clear();
        #[cfg(feature = "syntax")]
        {
            // a live side keeps using its buffer's highlighter; a static one is rebuilt for the new text
            let same_old = self.data.as_ref().is_some_and(|d| same_text(&d.old, &data.old));
            if !same_old {
                self.hl[0] = None;
            }
            let same_new = self.data.as_ref().is_some_and(|d| same_text(&d.new, &data.new));
            if !same_new {
                self.hl[1] = None;
            }
            if !(same_old && same_new) {
                self.hl_tried = false;
            }
        }
        self.data = Some(data);
        self.note = None;
        self.load = Load::Done;
    }
}

#[cfg(feature = "syntax")]
fn same_text(a: &SideText, b: &SideText) -> bool {
    match (a, b) {
        (SideText::Bytes(x), SideText::Bytes(y)) => x == y,
        (SideText::Rope(_), SideText::Rope(_)) => true,
        _ => false,
    }
}

/// Which side the cursor is on (split layout); Enter, `y` and searches start there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    Old,
    #[default]
    New,
}

/// Where things were drawn last frame, for the mouse.
#[derive(Clone, Debug, Default)]
pub struct Hits {
    /// The rows area and the first row drawn at its top (with the pixel offset of a smooth scroll).
    pub body: shrew_render::Rect,
    pub first: usize,
    pub y0: f32,
    pub row_h: f32,
    /// The old and new panes (unified: only `new` is set, spanning the text).
    pub old_pane: shrew_render::Rect,
    pub new_pane: shrew_render::Rect,
    pub scrollbar: shrew_render::Rect,
    pub buttons: Vec<(shrew_render::Rect, Button)>,
    /// Hunk actions in the center gutter: (rect, row of the hunk's first line).
    pub actions: Vec<(shrew_render::Rect, usize)>,
    /// Rows that fit on screen.
    pub page: usize,
}

/// The header bar's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Split,
    Unified,
    Whitespace,
    LessContext,
    MoreContext,
    Stage,
    Unstage,
    Revert,
    PrevChange,
    NextChange,
}

/// What can be done with the changes of a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Actions {
    pub stage: bool,
    pub unstage: bool,
    pub revert: bool,
}

pub struct DiffView {
    /// Names the view in gutter requests.
    pub id: u64,
    /// Its tab.
    pub buffer: u64,
    pub spec: DiffSpec,
    /// The repository's workdir, once discovered.
    pub repo: Option<PathBuf>,
    /// Both sides resolved (Head, Index, Worktree, Commit or Empty), once known.
    pub left: Option<Rev>,
    pub right: Option<Rev>,
    /// Every changed file rather than one.
    pub multi: bool,
    /// A commit's summary (commit diffs).
    pub summary: Option<String>,
    pub sections: Vec<Section>,
    pub rows: Rows,
    pub opts: Options,
    pub ignore_ws: bool,
    /// The rows no longer match the sections.
    pub stale: bool,
    pub cursor: usize,
    /// The other end of a line selection (`V`, a drag).
    pub anchor: Option<usize>,
    pub side: Side,
    /// First row and column shown.
    pub top: usize,
    pub left_col: usize,
    pub anim: Anim,
    /// Why there is nothing to show (not a repository, an error), or what is loading.
    pub message: Option<String>,
    /// A count typed before a motion, and a prefix key (`]`, `[`, `g`, `z`) waiting for the next one.
    pub count: Option<usize>,
    pub prefix: Option<char>,
    /// The editor's search pattern last acted on.
    pub search_seen: String,
    /// `X` once: the next `X` discards changes from the file on disk.
    pub armed: bool,
    pub hits: Hits,
    /// What a held mouse button drags, and the last click (row, when) for double clicks.
    pub grab: Option<Grab>,
    pub last_click: Option<(usize, std::time::Instant)>,
    /// Touchpad scrolling below one row or column, carried to the next event.
    pub wheel_acc: (f32, f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grab {
    /// Selecting rows from this one.
    Rows(usize),
    Scrollbar,
}

impl DiffView {
    pub fn new(id: u64, buffer: u64, spec: DiffSpec, layout: Layout, context: u32, ignore_ws: bool) -> DiffView {
        DiffView {
            id,
            buffer,
            multi: spec.path.is_none(),
            repo: spec.repo.clone(),
            spec,
            left: None,
            right: None,
            summary: None,
            sections: Vec::new(),
            rows: Rows::default(),
            opts: Options { layout, context },
            ignore_ws,
            stale: true,
            cursor: 0,
            anchor: None,
            side: Side::New,
            top: 0,
            left_col: 0,
            anim: Anim::default(),
            message: Some("loading…".into()),
            count: None,
            prefix: None,
            search_seen: String::new(),
            armed: false,
            hits: Hits::default(),
            grab: None,
            last_click: None,
            wheel_acc: (0.0, 0.0),
        }
    }

    /// What staging, unstaging and reverting mean here, if anything.
    pub fn actions(&self) -> Actions {
        let (l, r) = (self.left.as_ref(), self.right.as_ref());
        let live = self.sections.iter().any(|s| s.live.is_some());
        Actions {
            stage: matches!((l, r), (Some(Rev::Index), Some(Rev::Worktree))),
            unstage: matches!((l, r), (Some(Rev::Head), Some(Rev::Index))),
            revert: matches!((l, r), (Some(Rev::Index), Some(Rev::Worktree)))
                || (live && matches!((l, r), (Some(Rev::Head), Some(Rev::Worktree)))),
        }
    }

    /// Lays the sections out as rows again, keeping the cursor on its line.
    pub fn rebuild(&mut self) {
        let keep = self.rows.rows.get(self.cursor).copied();
        let top_keep = self.rows.rows.get(self.top).copied();
        let files: Vec<model::FileModel> = self
            .sections
            .iter()
            .map(|s| {
                let (changes, old_len, new_len, note) = match (&s.data, &s.note) {
                    (Some(d), None) => (
                        d.hunks.iter().map(|h| model::Change::new(h.old.clone(), h.new.clone())).collect(),
                        d.old.len_lines() as u32,
                        d.new.len_lines() as u32,
                        false,
                    ),
                    _ => (Vec::new(), 0, 0, true),
                };
                model::FileModel {
                    changes,
                    old_len,
                    new_len,
                    header: self.multi,
                    collapsed: self.multi && s.collapsed,
                    note,
                    unfolded: s.unfolded.clone(),
                }
            })
            .collect();
        self.rows = model::build(&files, self.opts);
        self.stale = false;
        let n = self.rows.len();
        self.cursor = keep.and_then(|r| self.relocate(r)).unwrap_or(self.cursor).min(n.saturating_sub(1));
        self.top = top_keep.and_then(|r| self.relocate(r)).unwrap_or(self.top).min(n.saturating_sub(1));
        if let Some(a) = self.anchor {
            self.anchor = Some(a.min(n.saturating_sub(1)));
        }
    }

    /// The row that now shows what `r` showed.
    fn relocate(&self, r: Row) -> Option<usize> {
        match r.kind {
            RowKind::File | RowKind::Note => self.rows.files.get(r.file as usize).map(|&f| f as usize),
            _ if r.new != model::NONE => self.rows.find(r.file, r.new, false),
            _ => self.rows.find(r.file, r.old, true),
        }
    }

    /// The text of a side of row `row`, if it has a line there.
    pub fn line(&self, row: usize, side: Side) -> Option<Cow<'_, str>> {
        let r = self.rows.rows.get(row)?;
        if !matches!(r.kind, RowKind::Same | RowKind::Change) {
            return None;
        }
        let d = self.sections.get(r.file as usize)?.data.as_ref()?;
        match side {
            Side::Old => r.old().map(|l| d.old.line(l as usize)),
            Side::New => r.new_line().map(|l| d.new.line(l as usize)),
        }
    }

    /// The rows the next action applies to: the selection, else the change under the cursor.
    pub fn target_rows(&self) -> Option<Range<usize>> {
        match self.anchor {
            Some(a) => Some(a.min(self.cursor)..a.max(self.cursor) + 1),
            None => self.rows.change_rows(self.cursor),
        }
    }

    // ----- moving -----

    /// Moves the cursor to `row` (clamped) and scrolls it into view with `margin` rows around it.
    pub fn goto(&mut self, row: usize, margin: usize) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        self.cursor = row.min(n - 1);
        self.reveal(margin);
    }

    pub fn reveal(&mut self, margin: usize) {
        let page = self.hits.page.max(1);
        let margin = margin.min(page.saturating_sub(1) / 2);
        if self.cursor < self.top + margin {
            self.top = self.cursor.saturating_sub(margin);
        } else if self.cursor + margin >= self.top + page {
            self.top = (self.cursor + margin + 1).saturating_sub(page);
        }
        self.clamp_top();
    }

    /// Scrolls by `rows` keeping the cursor on screen.
    pub fn scroll(&mut self, rows: isize, margin: usize) {
        self.top = (self.top as isize + rows).max(0) as usize;
        self.clamp_top();
        let page = self.hits.page.max(1);
        let margin = margin.min(page.saturating_sub(1) / 2);
        let lo = self.top + if self.top > 0 { margin } else { 0 };
        let hi = (self.top + page).saturating_sub(margin + 1).max(lo);
        self.cursor = self.cursor.clamp(lo, hi).min(self.rows.len().saturating_sub(1));
    }

    /// Scrolling stops once the last row is half way up the screen.
    pub fn clamp_top(&mut self) {
        let max = self.rows.len().saturating_sub(self.hits.page.max(1) / 2);
        self.top = self.top.min(max);
    }

    /// The change after (or before) the cursor, centred.
    pub fn jump_change(&mut self, forward: bool) -> bool {
        match self.rows.next_change(self.cursor, forward) {
            Some(r) => {
                self.cursor = r;
                self.center();
                true
            }
            None => false,
        }
    }

    pub fn jump_file(&mut self, forward: bool) -> bool {
        match self.rows.next_file(self.cursor, forward) {
            Some(r) => {
                self.cursor = r;
                self.top = r;
                self.clamp_top();
                true
            }
            None => false,
        }
    }

    pub fn center(&mut self) {
        let page = self.hits.page.max(1);
        self.top = self.cursor.saturating_sub(page / 3);
        self.clamp_top();
    }

    /// Opens or closes the fold or file header under the cursor. True when it was one.
    pub fn toggle_at_cursor(&mut self) -> bool {
        let Some(r) = self.rows.rows.get(self.cursor).copied() else { return false };
        let s = &mut self.sections[r.file as usize];
        match r.kind {
            RowKind::Fold => {
                s.unfolded.insert(r.old);
            }
            RowKind::File => s.collapsed = !s.collapsed,
            _ => return false,
        }
        self.stale = true;
        true
    }

    /// Context lines: `delta` more or fewer (everything past 99 is "all").
    pub fn change_context(&mut self, delta: i32) {
        let now = if self.opts.context == model::ALL { 100 } else { self.opts.context as i32 };
        let next = (now + delta).clamp(0, 100);
        self.opts.context = if next >= 100 { model::ALL } else { next as u32 };
        self.stale = true;
    }

    /// Opens every fold (`zR`: all context) or closes them all again (`zM`, back to `context` lines).
    pub fn unfold_all(&mut self, open: bool, context: u32) {
        for s in &mut self.sections {
            s.unfolded.clear();
        }
        self.opts.context = if open { model::ALL } else { context };
        self.stale = true;
    }

    pub fn total_stats(&self) -> (u32, u32) {
        self.sections.iter().fold((0, 0), |(a, r), s| (a + s.stats.0, r + s.stats.1))
    }
}
