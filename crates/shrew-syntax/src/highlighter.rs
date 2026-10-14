//! The UI thread's side of one buffer: a per-line span cache that drawing reads without locks, kept in step with
//! the document by mapping each edit onto it (lines above an edit keep their colors, lines below move with it).

use crate::detect::LangHint;
use crate::engine::{Engine, Inbox, Job, Syntaxes, lock};
use crate::theme::Span;
use crate::work::{Inval, Update};
use ropey::Rope;
use shrew_core::{ChangeSet, Document};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Edited lines keep their old (clamped) spans until re-highlighted, so typing never flickers; past this many
/// rewritten lines they are dropped instead.
const CARRY_OVER: usize = 256;

pub struct Highlighter {
    engine: Arc<Engine>,
    job: Arc<Job>,
    lines: Vec<Option<Box<[Span]>>>,
    len_chars: usize,
    version: u64,
    viewport: (usize, usize),
    syntax: Option<String>,
    settled: bool,
}

impl Highlighter {
    /// A highlighter for `rope` at `version` (see also [`Syntaxes::highlighter`]).
    pub fn new(syntaxes: &Syntaxes, rope: &Rope, version: u64, path: Option<&Path>) -> Highlighter {
        let hint = LangHint::new(path, &first_line(rope));
        let viewport = (0, 60);
        let job = Arc::new(Job::new(Inbox {
            text: Some((rope.clone(), version)),
            hint: Some(hint),
            viewport,
            ..Inbox::default()
        }));
        let engine = syntaxes.engine().clone();
        engine.add(job.clone());
        Highlighter {
            engine,
            job,
            lines: vec![None; rope.len_lines()],
            len_chars: rope.len_chars(),
            version,
            viewport,
            syntax: None,
            settled: false,
        }
    }

    /// Spans for a line, `None` while it is not highlighted yet (draw it plain). Byte ranges within the line's
    /// text without its line ending; always on char boundaries and within the line, even right after an edit.
    #[inline]
    pub fn spans_for_line(&self, line: usize) -> Option<&[Span]> {
        self.lines.get(line)?.as_deref()
    }

    /// Takes the document's pending edits and hands the new text to the worker. Cheap; call after every change
    /// (or once per frame). If something else consumes `take_edits`, use [`Highlighter::apply_edits`].
    pub fn sync(&mut self, doc: &mut Document) {
        let edits = doc.take_edits();
        self.apply_edits(&doc.rope, doc.version, &edits);
    }

    /// Like [`Highlighter::sync`] with the edits taken by the caller: `edits` are the change sets that led to
    /// `version`, one per version step as shrew-core's `Document` records them (older ones are ignored; if some
    /// are missing everything is re-highlighted).
    pub fn apply_edits(&mut self, rope: &Rope, version: u64, edits: &[ChangeSet]) {
        if version == self.version {
            return;
        }
        let needed = version.wrapping_sub(self.version) as usize;
        let inv = if version > self.version && edits.len() >= needed {
            self.inval_for(&edits[edits.len() - needed..], rope)
        } else {
            None
        };
        let inv =
            inv.unwrap_or(Inval { first: 0, suffix: 0, old_lines: self.lines.len(), new_lines: rope.len_lines() });
        let out = {
            let mut inbox = lock(&self.job.inbox);
            let out = std::mem::take(&mut inbox.out);
            inbox.woken = false;
            inbox.text = Some((rope.clone(), version));
            inbox.inval = Some(match inbox.inval {
                Some(prev) => prev.then(inv),
                None => inv,
            });
            inbox.settled = None;
            out
        };
        // results published before this edit are in the old coordinates: apply them, then map
        self.apply_updates(out);
        self.remap(&inv, rope);
        self.version = version;
        self.len_chars = rope.len_chars();
        self.settled = false;
        self.engine.signal();
    }

    /// First changed line and unchanged suffix lines of a sequence of change sets (`None` if they do not fit).
    fn inval_for(&self, edits: &[ChangeSet], rope: &Rope) -> Option<Inval> {
        let mut len = self.len_chars;
        let (mut prefix, mut suffix) = (usize::MAX, usize::MAX);
        for cs in edits {
            let mut delta: isize = 0;
            let mut end = 0;
            for (k, sp) in cs.spans().enumerate() {
                if k == 0 {
                    prefix = prefix.min(sp.from);
                }
                delta += sp.new_to as isize - sp.old_to as isize;
                end = (sp.old_to as isize + delta) as usize;
            }
            let new_len = (len as isize + delta) as usize;
            if !cs.is_empty() {
                suffix = suffix.min(new_len.checked_sub(end)?);
            }
            len = new_len;
        }
        if len != rope.len_chars() {
            return None;
        }
        let n = rope.len_lines();
        let first = rope.char_to_line(prefix.min(len));
        let suffix = if suffix == usize::MAX { n } else { n - (rope.char_to_line(len - suffix.min(len)) + 1) };
        let old_lines = self.lines.len();
        let suffix = suffix.min(n - first).min(old_lines.saturating_sub(first));
        Some(Inval { first: first.min(old_lines), suffix, old_lines, new_lines: n })
    }

    /// Moves the cached spans through an edit.
    fn remap(&mut self, inv: &Inval, rope: &Rope) {
        let (old_mid, new_mid) = (inv.first..inv.old_suffix(), inv.first..inv.new_suffix());
        if old_mid.len() == new_mid.len() {
            for l in new_mid {
                let stale = self.lines[l].take();
                self.lines[l] = if l - inv.first < CARRY_OVER { clamp(stale, l, rope) } else { None };
            }
            return;
        }
        let old: Vec<_> = self.lines.drain(old_mid.clone()).collect();
        let mut old = old.into_iter();
        let fresh: Vec<_> = new_mid
            .clone()
            .map(|l| if l - inv.first < CARRY_OVER { clamp(old.next().flatten(), l, rope) } else { None })
            .collect();
        self.lines.splice(inv.first..inv.first, fresh);
        debug_assert_eq!(self.lines.len(), inv.new_lines);
    }

    fn apply_updates(&mut self, out: Vec<Update>) -> Option<Range<usize>> {
        let mut changed: Option<Range<usize>> = None;
        let mut touch = |r: Range<usize>| {
            changed = Some(match changed.take() {
                Some(c) => c.start.min(r.start)..c.end.max(r.end),
                None => r,
            })
        };
        for u in out {
            match u {
                Update::Lines { start, spans } => {
                    let end = (start + spans.len()).min(self.lines.len());
                    touch(start..end);
                    for (slot, s) in self.lines[start.min(end)..end].iter_mut().zip(spans) {
                        *slot = Some(s);
                    }
                }
                Update::Clear(r) => {
                    let r = r.start.min(self.lines.len())..r.end.min(self.lines.len());
                    touch(r.clone());
                    self.lines[r].fill(None);
                }
                Update::Syntax(name) => {
                    self.syntax = name;
                    touch(0..self.lines.len());
                }
            }
        }
        changed
    }

    /// Picks up finished highlighting. Returns the lines that changed (redraw if they are on screen).
    pub fn poll(&mut self) -> Option<Range<usize>> {
        let (out, settled) = {
            let mut inbox = lock(&self.job.inbox);
            inbox.woken = false;
            (std::mem::take(&mut inbox.out), inbox.settled == Some(self.version))
        };
        self.settled = settled;
        self.apply_updates(out)
    }

    /// The lines on screen (`rows = 0` for a buffer that is not visible): they are highlighted first.
    pub fn set_viewport(&mut self, top: usize, rows: usize) {
        if self.viewport == (top, rows) {
            return;
        }
        self.viewport = (top, rows);
        let mut inbox = lock(&self.job.inbox);
        inbox.viewport = (top, rows);
        inbox.settled = None;
        drop(inbox);
        self.settled = false;
        self.engine.signal();
    }

    /// Forces a grammar: a shrew lang name (`python`), a grammar name (`Rust`) or an extension (`rs`); `text` for
    /// none.
    pub fn set_language(&mut self, name: &str) {
        self.set_hint(LangHint::named(name));
    }

    /// Picks the grammar again from a (new) path and the text's first line, e.g. after "save as".
    pub fn detect_language(&mut self, path: Option<&Path>, rope: &Rope) {
        self.set_hint(LangHint::new(path, &first_line(rope)));
    }

    fn set_hint(&mut self, hint: LangHint) {
        let mut inbox = lock(&self.job.inbox);
        inbox.hint = Some(hint);
        inbox.settled = None;
        drop(inbox);
        self.settled = false;
        self.engine.signal();
    }

    /// The grammar in use (`None` for plain text or before it is known).
    pub fn syntax_name(&self) -> Option<&str> {
        self.syntax.as_deref()
    }

    /// Everything that will be highlighted is (as of the last [`Highlighter::poll`]).
    pub fn is_settled(&self) -> bool {
        self.settled
    }

    /// The document version the cache matches.
    pub fn version(&self) -> u64 {
        self.version
    }
}

impl Drop for Highlighter {
    fn drop(&mut self) {
        self.job.closed.store(true, Ordering::Release);
        self.engine.signal();
    }
}

fn first_line(rope: &Rope) -> String {
    if rope.len_lines() == 0 {
        return String::new();
    }
    rope.line(0).chars().take(256).filter(|&c| c != '\n' && c != '\r').collect()
}

/// Fits stale spans to a line's new text: cut at its end and snapped to char boundaries.
fn clamp(spans: Option<Box<[Span]>>, line: usize, rope: &Rope) -> Option<Box<[Span]>> {
    let spans = spans?;
    let text = rope.line(line);
    let mut len = text.len_bytes();
    for c in text.chars_at(text.len_chars()).reversed() {
        if c != '\n' && c != '\r' {
            break;
        }
        len -= 1;
    }
    let floor = |b: usize| text.char_to_byte(text.byte_to_char(b.min(len)));
    let fitted: Box<[Span]> = spans
        .iter()
        .filter_map(|s| {
            let (start, end) = (floor(s.start as usize), floor(s.end as usize));
            (start < end).then(|| Span { start: start as u32, end: end as u32, ..*s })
        })
        .collect();
    Some(fitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_to_boundaries() {
        let rope = Rope::from_str("héllo\nx\n");
        let s = |a, b| Span { start: a, end: b, fg: Default::default(), bold: false, italic: false, underline: false };
        let spans: Box<[Span]> = vec![s(0, 2), s(2, 4), s(4, 40)].into();
        let got = clamp(Some(spans.clone()), 0, &rope).unwrap();
        // byte 2 is inside 'é' (bytes 1..3): floored to 1
        assert_eq!(got.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(), vec![(0, 1), (1, 4), (4, 6)]);
        let got = clamp(Some(spans), 1, &rope).unwrap();
        assert_eq!(got.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(), vec![(0, 1)]);
    }
}
