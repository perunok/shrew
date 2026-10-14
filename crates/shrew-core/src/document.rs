//! A document (text + file + history) and a buffer (a document plus its cursors and viewport).

use crate::change::ChangeSet;
use crate::history::History;
use crate::selection::Selection;
use crate::text;
use ropey::Rope;
use std::io;
use std::path::{Path, PathBuf};

/// Change sets kept for `take_edits` before the record restarts.
const MAX_PENDING_EDITS: usize = 512;
/// Change sets bigger than this are not kept (re-highlighting everything is cheaper than mapping them).
const MAX_RECORDED_CHANGES: usize = 4096;

pub struct Document {
    pub rope: Rope,
    pub path: Option<PathBuf>,
    /// Bumped on every change; caches key on it.
    pub version: u64,
    pub dirty: bool,
    crlf: bool,
    history: History,
    /// Change sets applied since the last `take_edits`, for incremental consumers (syntax).
    edits: Vec<ChangeSet>,
    /// The last column computed by [`Document::col_of`], kept valid across edits that do not touch the text
    /// before it, so the cursor column on a multi-megabyte line does not walk the line on every keystroke.
    col_cache: std::cell::Cell<Option<ColCache>>,
    /// Change sets since the editor last moved its marks (`ma`, the insert start) along with the text.
    mark_log: Vec<ChangeSet>,
}

#[derive(Clone, Copy, Debug)]
struct ColCache {
    version: u64,
    line_start: usize,
    pos: usize,
    col: usize,
    tab_width: usize,
    /// Every char between the line start and `pos` is one column wide.
    plain: bool,
}

impl Default for Document {
    fn default() -> Self {
        Document::from_str("")
    }
}

impl Document {
    #[allow(clippy::should_implement_trait)] // infallible, unlike FromStr
    pub fn from_str(s: &str) -> Self {
        Document {
            rope: Rope::from_str(s),
            path: None,
            version: 0,
            dirty: false,
            crlf: false,
            history: History::default(),
            edits: Vec::new(),
            col_cache: std::cell::Cell::new(None),
            mark_log: Vec::new(),
        }
    }

    /// Reads a file. Safe to call off the UI thread (the app loads big files in the background).
    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        let mut text = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        };
        let crlf = text.contains("\r\n");
        if crlf {
            text = text.replace("\r\n", "\n");
        }
        let mut doc = Document::from_str(&text);
        doc.crlf = crlf;
        doc.path = Some(path.to_path_buf());
        Ok(doc)
    }

    pub fn save(&mut self) -> io::Result<usize> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name (use :w <path>)"))?;
        self.save_as(&path)
    }

    /// Writes atomically (temp file + rename) so a crash never leaves half a file.
    pub fn save_as(&mut self, path: &Path) -> io::Result<usize> {
        let mut out = String::with_capacity(self.rope.len_bytes() + 16);
        for chunk in self.rope.chunks() {
            if self.crlf { out.push_str(&chunk.replace('\n', "\r\n")) } else { out.push_str(chunk) }
        }
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let tmp =
            path.with_file_name(format!(".{}.shrew-tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
        std::fs::write(&tmp, out.as_bytes())?;
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, path)?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(out.len())
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[scratch]".into())
    }

    /// Applies without touching history (undo/redo use this).
    fn apply_raw(&mut self, cs: &ChangeSet) -> ChangeSet {
        let inv = cs.apply(&mut self.rope);
        self.col_cache.set(self.col_cache.get().and_then(|c| c.after(cs, self.version)));
        self.version += 1;
        self.dirty = true;
        // Bounded: when nobody takes them (syntax off, a buffer in the background) or a change set is huge, the
        // record restarts. Consumers see fewer change sets than version steps and treat it as "all changed".
        if self.edits.len() >= MAX_PENDING_EDITS || cs.changes().len() > MAX_RECORDED_CHANGES {
            self.edits.clear();
        }
        if cs.changes().len() <= MAX_RECORDED_CHANGES {
            self.edits.push(cs.clone());
        }
        if self.mark_log.len() < MAX_PENDING_EDITS {
            self.mark_log.push(cs.clone()); // past that nobody is reading it (no keys typed): stop growing
        }
        inv
    }

    /// The change sets applied since the last call (the editor maps its marks through them after every key).
    pub(crate) fn take_mark_log(&mut self) -> Vec<ChangeSet> {
        std::mem::take(&mut self.mark_log)
    }

    /// The change sets applied since the last call, one per version step. When fewer come back than versions went
    /// by, some were dropped (see `apply_raw`): consumers must then treat the whole text as changed.
    /// Display column of `pos` (like [`text::col_of`], but remembering the last answer for the next call).
    pub fn col_of(&self, pos: usize, tab_width: usize) -> usize {
        let rope = &self.rope;
        let pos = pos.min(rope.len_chars());
        let ls = text::line_start(rope, text::line_of(rope, pos));
        let cached = self
            .col_cache
            .get()
            .filter(|c| c.version == self.version && c.line_start == ls && c.tab_width == tab_width);
        let (col, plain) = match cached {
            Some(c) if pos >= c.pos => {
                let (col, plain) = text::advance_cols_plain(rope.slice(c.pos..pos), c.col, tab_width);
                (col, plain && c.plain)
            }
            Some(c) if c.plain => return pos - ls,
            _ => text::advance_cols_plain(rope.slice(ls..pos), 0, tab_width),
        };
        self.col_cache.set(Some(ColCache { version: self.version, line_start: ls, pos, col, tab_width, plain }));
        col
    }

    pub fn take_edits(&mut self) -> Vec<ChangeSet> {
        std::mem::take(&mut self.edits)
    }

    pub fn len(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_chars() == 0
    }
}

impl ColCache {
    /// The cache after `cs` (applied at `version`): still good when every change is after `pos` or entirely before
    /// the line (then it only shifts).
    fn after(self, cs: &ChangeSet, version: u64) -> Option<ColCache> {
        if self.version != version {
            return None;
        }
        let mut delta: isize = 0;
        for c in cs.changes() {
            if c.from >= self.pos {
                break;
            }
            // must not touch the line break before the line
            if c.to >= self.line_start {
                return None;
            }
            delta += c.text.chars().count() as isize - (c.to - c.from) as isize;
        }
        let shift = |p: usize| (p as isize + delta) as usize;
        Some(ColCache { version: version + 1, line_start: shift(self.line_start), pos: shift(self.pos), ..self })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct View {
    /// First visible line.
    pub top: usize,
    /// First visible display column.
    pub left: usize,
    /// Visible text rows and columns (set by the UI from the window size).
    pub rows: usize,
    pub cols: usize,
}

/// Where an undo or redo changed the text, for vim's cursor placement.
pub(crate) struct HistoryJump {
    /// First changed position in the new text.
    pub top: usize,
    /// The cursor (line, column) when the undone/redone group started.
    pub start: (usize, usize),
    /// The group started with a single cursor.
    pub single: bool,
}

pub struct Buffer {
    pub id: u64,
    pub doc: Document,
    pub sel: Selection,
    pub view: View,
}

impl Buffer {
    pub fn new(id: u64, doc: Document) -> Self {
        Buffer { id, doc, sel: Selection::point(0), view: View { rows: 40, cols: 120, ..View::default() } }
    }

    #[inline]
    pub fn rope(&self) -> &Rope {
        &self.doc.rope
    }

    /// Applies an edit, maps (or replaces) the selection and records it for undo.
    pub fn apply(&mut self, cs: ChangeSet, after: Option<Selection>) {
        if cs.is_empty() {
            if let Some(s) = after {
                self.sel = s;
            }
            return;
        }
        let before = self.sel.clone();
        let inv = self.doc.apply_raw(&cs);
        let len = self.doc.len();
        self.sel = after.unwrap_or_else(|| before.map(&cs)).clamp(len);
        self.doc.history.record(cs, inv, &before, &self.sel);
    }

    /// Keeps edits in one undo group until the matching [`release_history`](Buffer::release_history).
    pub fn hold_history(&mut self) {
        self.doc.history.hold();
    }

    pub fn release_history(&mut self) {
        self.doc.history.set_open_after(&self.sel);
        self.doc.history.release();
    }

    /// Closes the current undo group.
    pub fn commit(&mut self) {
        self.doc.history.set_open_after(&self.sel);
        self.doc.history.commit();
    }

    pub fn undo(&mut self) -> bool {
        self.history_step(false).is_some()
    }

    pub fn redo(&mut self) -> bool {
        self.history_step(true).is_some()
    }

    /// Undoes or redoes one group, restores its selection and reports where the text changed.
    pub(crate) fn history_step(&mut self, redo: bool) -> Option<HistoryJump> {
        let step = if redo { self.doc.history.redo() } else { self.doc.history.undo() }?;
        // the cursor when the group started, as (line, column): measured on the text it belonged to, which is the
        // current text before a redo and the restored text after an undo
        let line_col = |rope: &Rope, sel: &Selection| {
            let h = sel.primary().head.min(rope.len_chars());
            let l = text::line_of(rope, h);
            (l, h - text::line_start(rope, l))
        };
        let mut start = redo.then(|| line_col(&self.doc.rope, &step.before));
        let mut top: Option<usize> = None;
        for cs in &step.changes {
            let inv = self.doc.apply_raw(cs);
            top = top.map(|t| cs.map(t, crate::change::Assoc::Before));
            if let (Some(c), Some(removed)) = (cs.changes().first(), inv.changes().first()) {
                // text that starts with a line break changes the lines after it
                let nl = c.text.starts_with('\n') && (removed.text.is_empty() || removed.text.starts_with('\n'));
                let p = removed.from + usize::from(nl);
                top = Some(top.map_or(p, |t| t.min(p)));
            }
        }
        if start.is_none() {
            start = Some(line_col(&self.doc.rope, &step.before));
        }
        self.sel = step.selection.clamp(self.doc.len());
        Some(HistoryJump {
            top: top.unwrap_or(0).min(self.doc.len()),
            start: start.unwrap_or_default(),
            single: step.before.len() == 1,
        })
    }

    pub fn cursor_line(&self) -> usize {
        text::line_of(self.rope(), self.sel.primary().head)
    }

    /// Scrolls so the primary cursor is visible with `scrolloff` lines of context.
    pub fn scroll_to_cursor(&mut self, scrolloff: usize, tab_width: usize) {
        let rows = self.view.rows.max(1);
        let so = scrolloff.min(rows.saturating_sub(1) / 2);
        let line = self.cursor_line();
        if line < self.view.top + so {
            self.view.top = line.saturating_sub(so);
        } else if line + so >= self.view.top + rows {
            self.view.top = (line + so + 1).saturating_sub(rows);
        }
        let last = text::last_line(self.rope());
        self.view.top = self.view.top.min(last);
        let col = self.doc.col_of(self.sel.primary().head, tab_width);
        let cols = self.view.cols.max(8);
        let margin = 4.min(cols / 4);
        if col < self.view.left + margin {
            self.view.left = col.saturating_sub(margin);
        } else if col + margin >= self.view.left + cols {
            self.view.left = col + margin + 1 - cols;
        }
    }

    pub fn center_on_cursor(&mut self) {
        self.view.top = self.cursor_line().saturating_sub(self.view.rows / 2);
    }

    pub fn reload(&mut self) -> io::Result<()> {
        let Some(path) = self.doc.path.clone() else { return Ok(()) };
        let fresh = Document::load(&path)?;
        let all = ChangeSet::single(crate::change::Change::replace(0, self.doc.len(), fresh.rope.to_string()));
        let head = self.sel.primary().head;
        self.apply(all, Some(Selection::point(head.min(fresh.rope.len_chars()))));
        self.commit();
        self.doc.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Change;

    #[test]
    fn column_cache_follows_edits() {
        let mut b = Buffer::new(1, Document::from_str("ab\n\tcd\nxyz"));
        let tw = 4;
        assert_eq!(b.doc.col_of(6, tw), 6);
        // typing after the cached point keeps it
        b.apply(ChangeSet::single(Change::insert(6, "é")), None);
        assert_eq!(b.doc.col_of(7, tw), 7);
        assert_eq!(b.doc.col_of(4, tw), 4);
        // an edit on an earlier line shifts it
        b.apply(ChangeSet::single(Change::insert(0, "zz")), None);
        assert_eq!(b.doc.col_of(9, tw), 7);
        assert_eq!(b.doc.col_of(9, tw), text::col_of(b.rope(), 9, tw));
        // an edit inside the line before it invalidates it
        b.apply(ChangeSet::single(Change::delete(5, 6)), None);
        assert_eq!(b.doc.col_of(8, tw), text::col_of(b.rope(), 8, tw));
        for p in 0..b.doc.len() {
            assert_eq!(b.doc.col_of(p, tw), text::col_of(b.rope(), p, tw), "pos {p}");
        }
    }
}
