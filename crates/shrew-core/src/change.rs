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
    changes: Vec<Change>,
}

impl ChangeSet {
    /// Sorts the changes; overlapping ones are clipped so the set stays well-formed.
    pub fn new(mut changes: Vec<Change>) -> Self {
        changes.retain(|c| c.from != c.to || !c.text.is_empty());
        changes.sort_by_key(|c| (c.from, c.to));
        let mut out: Vec<Change> = Vec::with_capacity(changes.len());
        for mut c in changes {
            if let Some(last) = out.last() {
                if c.from < last.to {
                    c.from = last.to;
                    c.to = c.to.max(c.from);
                }
            }
            out.push(c);
        }
        ChangeSet { changes: out }
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
        let mut inverse = Vec::with_capacity(self.changes.len());
        let mut delta: isize = 0;
        for c in &self.changes {
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
        ChangeSet { changes: inverse }
    }

    /// Where `pos` (in old coordinates) ends up after this change set.
    pub fn map(&self, pos: usize, assoc: Assoc) -> usize {
        let mut delta: isize = 0;
        for c in &self.changes {
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
