//! Edits as change sets: sorted, non-overlapping replacements in the coordinates of the text *before* the edit.
//! Applying one returns its inverse, which is what undo stores.

use ropey::Rope;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub from: usize,
    pub to: usize,
    pub text: String,
}

impl Change {
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Change { from: at, to: at, text: text.into() }
    }
    pub fn delete(from: usize, to: usize) -> Self {
        Change { from, to, text: String::new() }
    }
    pub fn replace(from: usize, to: usize, text: impl Into<String>) -> Self {
        Change { from, to, text: text.into() }
    }
    fn ins_len(&self) -> usize {
        self.text.chars().count()
    }
}

/// Which side of text inserted exactly at a position a mapped position ends up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assoc {
    Before,
    After,
}

/// A position-level summary of an edit, in the coordinates of the text before it (used by syntax re-parsing and
/// caches that only need to know which region moved).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditSpan {
    pub from: usize,
    pub old_to: usize,
    pub new_to: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    /// Shared: history, the syntax edit log and undo steps hold the same change sets without copying them.
    changes: std::sync::Arc<Vec<Change>>,
}

impl ChangeSet {
    /// Sorts the changes; overlapping ones are clipped so the set stays well-formed.
    pub fn new(mut changes: Vec<Change>) -> Self {
        changes.retain(|c| c.from != c.to || !c.text.is_empty());
        changes.sort_by_key(|c| (c.from, c.to));
        let mut out: Vec<Change> = Vec::with_capacity(changes.len());
        for mut c in changes {
            if let Some(last) = out.last()
                && c.from < last.to
            {
                c.from = last.to;
                c.to = c.to.max(c.from);
            }
            out.push(c);
        }
        ChangeSet { changes: std::sync::Arc::new(out) }
    }

    pub fn single(c: Change) -> Self {
        ChangeSet::new(vec![c])
    }

    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Applies to `rope` and returns the inverse change set (in the coordinates of the new text).
    pub fn apply(&self, rope: &mut Rope) -> ChangeSet {
        if self.changes.len() >= REBUILD_AT {
            return self.apply_rebuild(rope);
        }
        let mut inverse = Vec::with_capacity(self.changes.len());
        let mut delta: isize = 0;
        for c in self.changes.iter() {
            let removed = rope.slice(c.from..c.to).to_string();
            let from = (c.from as isize + delta) as usize;
            let ins = c.ins_len();
            inverse.push(Change { from, to: from + ins, text: removed });
            delta += ins as isize - (c.to - c.from) as isize;
        }
        for c in self.changes.iter().rev() {
            if c.to > c.from {
                rope.remove(c.from..c.to);
            }
            if !c.text.is_empty() {
                rope.insert(c.from, &c.text);
            }
        }
        ChangeSet { changes: std::sync::Arc::new(inverse) }
    }

    /// Thousands of edits (a `:%s`, typing over every occurrence of a word) cost a rope insert/remove each; past
    /// [`REBUILD_AT`] it is much faster to stream the text once into a new rope.
    fn apply_rebuild(&self, rope: &mut Rope) -> ChangeSet {
        let delta: isize = self.changes.iter().map(|c| c.text.len() as isize - (c.to - c.from) as isize).sum();
        let mut out = String::with_capacity((rope.len_bytes() as isize + delta.max(0)) as usize);
        let mut inverse = Vec::with_capacity(self.changes.len());
        let mut src = CharStream::new(rope);
        let mut shift: isize = 0;
        for c in self.changes.iter() {
            src.copy_to(c.from, &mut out);
            let mut removed = String::new();
            src.copy_to(c.to, &mut removed);
            let from = (c.from as isize + shift) as usize;
            let ins = c.ins_len();
            inverse.push(Change { from, to: from + ins, text: removed });
            shift += ins as isize - (c.to - c.from) as isize;
            out.push_str(&c.text);
        }
        src.copy_to(rope.len_chars(), &mut out);
        *rope = Rope::from_str(&out);
        ChangeSet { changes: std::sync::Arc::new(inverse) }
    }

    /// Maps many positions at once; `items` must be sorted by position. One pass over the changes instead of one
    /// per position (selections with thousands of cursors).
    pub fn map_sorted(&self, items: &mut [(usize, Assoc)]) {
        let mut i = 0;
        let mut delta: isize = 0;
        for (pos, assoc) in items.iter_mut() {
            // changes entirely before `pos`, whatever the association
            while let Some(c) = self.changes.get(i) {
                let before = if c.from == c.to { c.from < *pos } else { c.to <= *pos };
                if !before {
                    break;
                }
                delta += c.ins_len() as isize - (c.to - c.from) as isize;
                i += 1;
            }
            let mut d = delta;
            let mut out = None;
            for c in &self.changes[i..] {
                if c.from > *pos {
                    break;
                }
                let ins = c.ins_len() as isize;
                if c.from == c.to {
                    if *assoc == Assoc::After {
                        d += ins;
                        continue;
                    }
                    break;
                }
                // a replacement starting at or before `pos` and ending after it: snap to its start
                out = Some((c.from as isize + d) as usize);
                break;
            }
            *pos = out.unwrap_or((*pos as isize + d) as usize);
        }
    }

    /// Where `pos` (in old coordinates) ends up after this change set.
    pub fn map(&self, pos: usize, assoc: Assoc) -> usize {
        let mut delta: isize = 0;
        for c in self.changes.iter() {
            if c.from > pos {
                break;
            }
            let ins = c.ins_len() as isize;
            if c.from == c.to {
                // pure insertion; at pos only `After` moves past it
                if c.from < pos || assoc == Assoc::After {
                    delta += ins;
                    continue;
                }
                break;
            }
            if c.to <= pos {
                delta += ins - (c.to - c.from) as isize;
                continue;
            }
            // strictly inside a replaced range: snap to its start
            return (c.from as isize + delta) as usize;
        }
        (pos as isize + delta) as usize
    }

    /// Position-level spans (old coordinates), for incremental consumers.
    pub fn spans(&self) -> impl Iterator<Item = EditSpan> + '_ {
        self.changes.iter().map(|c| EditSpan { from: c.from, old_to: c.to, new_to: c.from + c.ins_len() })
    }
}

/// Change sets with at least this many changes rebuild the rope instead of editing it in place.
const REBUILD_AT: usize = 2048;

/// Copies a rope's text out in order, by char position, a chunk at a time.
struct CharStream<'a> {
    chunks: ropey::iter::Chunks<'a>,
    /// Rest of the current chunk.
    cur: &'a str,
    /// Char position of the start of `cur`.
    pos: usize,
}

impl<'a> CharStream<'a> {
    fn new(rope: &'a Rope) -> Self {
        CharStream { chunks: rope.chunks(), cur: "", pos: 0 }
    }

    /// Appends the text from the current position up to char `to`.
    fn copy_to(&mut self, to: usize, out: &mut String) {
        while self.pos < to {
            if self.cur.is_empty() {
                match self.chunks.next() {
                    Some(c) => self.cur = c,
                    None => return,
                }
                continue;
            }
            let want = to - self.pos;
            // byte length of the first `want` chars of `cur` (all of it when shorter)
            let (bytes, chars) = if self.cur.len() <= want && self.cur.is_ascii() {
                (self.cur.len(), self.cur.len())
            } else {
                match self.cur.char_indices().nth(want) {
                    Some((b, _)) => (b, want),
                    None => (self.cur.len(), self.cur.chars().count()),
                }
            };
            out.push_str(&self.cur[..bytes]);
            self.cur = &self.cur[bytes..];
            self.pos += chars;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_and_invert() {
        let mut r = Rope::from_str("hello world");
        let cs = ChangeSet::new(vec![Change::replace(6, 11, "shrew"), Change::insert(0, ">> "), Change::delete(4, 5)]);
        let inv = cs.apply(&mut r);
        assert_eq!(r.to_string(), ">> hell shrew");
        inv.apply(&mut r);
        assert_eq!(r.to_string(), "hello world");
    }

    #[test]
    fn rebuild_matches_in_place_edits() {
        let text: String = (0..5000).map(|i| format!("l{i} é\n")).collect();
        let changes: Vec<Change> = (0..3000).map(|i| Change::replace(i * 7, i * 7 + 2, format!("<{i}>"))).collect();
        let cs = ChangeSet::new(changes.clone());
        assert!(cs.changes().len() >= REBUILD_AT);
        let mut a = Rope::from_str(&text);
        let inv = cs.apply(&mut a);
        // the same edits one by one, below the threshold
        let mut b = Rope::from_str(&text);
        for chunk in changes.chunks(1000).rev() {
            ChangeSet::new(chunk.to_vec()).apply(&mut b);
        }
        assert_eq!(a, b);
        inv.apply(&mut a);
        assert_eq!(a.to_string(), text);
    }

    #[test]
    fn map_sorted_matches_map() {
        let cs = ChangeSet::new(vec![Change::insert(2, "xx"), Change::delete(5, 7), Change::replace(9, 10, "abc")]);
        let mut items: Vec<(usize, Assoc)> = (0..12).flat_map(|p| [(p, Assoc::Before), (p, Assoc::After)]).collect();
        let want: Vec<usize> = items.iter().map(|&(p, a)| cs.map(p, a)).collect();
        cs.map_sorted(&mut items);
        assert_eq!(items.iter().map(|i| i.0).collect::<Vec<_>>(), want);
    }

    #[test]
    fn mapping() {
        let cs = ChangeSet::new(vec![Change::insert(2, "xx"), Change::delete(5, 7)]);
        assert_eq!(cs.map(0, Assoc::After), 0);
        assert_eq!(cs.map(2, Assoc::Before), 2);
        assert_eq!(cs.map(2, Assoc::After), 4);
        assert_eq!(cs.map(4, Assoc::After), 6);
        assert_eq!(cs.map(6, Assoc::After), 7); // inside the deletion -> its start
        assert_eq!(cs.map(7, Assoc::After), 7); // end of the deletion
        assert_eq!(cs.map(9, Assoc::After), 9);
    }
}
