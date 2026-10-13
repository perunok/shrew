//! A document (text + file + history) and a buffer (a document plus its cursors and viewport).

use crate::change::ChangeSet;
use crate::history::History;
use crate::selection::Selection;
use crate::text;
use ropey::Rope;
use std::io;
use std::path::{Path, PathBuf};

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
}

impl Default for Document {
    fn default() -> Self {
        Document::from_str("")
    }
}

impl Document {
    pub fn from_str(s: &str) -> Self {
        Document { rope: Rope::from_str(s), path: None, version: 0, dirty: false, crlf: false, history: History::default(), edits: Vec::new() }
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
        let path = self.path.clone().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name (use :w <path>)"))?;
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
        let tmp = path.with_file_name(format!(".{}.shrew-tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
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
        self.version += 1;
        self.dirty = true;
        self.edits.push(cs.clone());
        inv
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

    /// Closes the current undo group.
    pub fn commit(&mut self) {
        self.doc.history.set_open_after(&self.sel);
        self.doc.history.commit();
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.doc.history.undo() else { return false };
        for cs in &step.changes {
            self.doc.apply_raw(cs);
        }
        self.sel = step.selection.clamp(self.doc.len());
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.doc.history.redo() else { return false };
        for cs in &step.changes {
            self.doc.apply_raw(cs);
        }
        self.sel = step.selection.clamp(self.doc.len());
        true
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
        let col = text::col_of(self.rope(), self.sel.primary().head, tab_width);
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
