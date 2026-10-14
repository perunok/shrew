//! Vim's jump list: where big moves (`G`, `/`, `%`, `:42`, marks, opening another file) started from, walked with
//! Ctrl+O (older) and Ctrl+I / Tab (newer). Positions are lines and columns, so edits never leave one past the end.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jump {
    pub buffer: u64,
    /// To reopen the file when its buffer was closed.
    pub path: Option<PathBuf>,
    pub line: usize,
    pub col: usize,
}

impl Jump {
    fn same_place(&self, o: &Jump) -> bool {
        self.line == o.line && (self.buffer == o.buffer || (self.path.is_some() && self.path == o.path))
    }
}

#[derive(Clone, Debug, Default)]
pub struct JumpList {
    list: Vec<Jump>,
    /// The entry Ctrl+O / Ctrl+I last went to; `list.len()` when not walking.
    idx: usize,
}

impl JumpList {
    pub const MAX: usize = 100;

    /// Records the place a jump starts from. Like vim, an older entry for the same line goes, and walking starts
    /// over from the newest.
    pub fn push(&mut self, j: Jump) {
        self.list.retain(|o| !o.same_place(&j));
        self.list.push(j);
        if self.list.len() > Self::MAX {
            self.list.remove(0);
        }
        self.idx = self.list.len();
    }

    /// Ctrl+O from `here`: the previous entry. The first step back remembers `here`, so Ctrl+I returns to it.
    pub fn back(&mut self, here: Jump) -> Option<Jump> {
        if self.idx >= self.list.len() {
            self.push(here.clone());
            self.idx = self.list.len() - 1;
        }
        // skip entries for the line we are on
        while self.idx > 0 {
            self.idx -= 1;
            if !self.list[self.idx].same_place(&here) {
                return Some(self.list[self.idx].clone());
            }
        }
        None
    }

    /// Ctrl+I: the next entry, if Ctrl+O went back before.
    pub fn forward(&mut self) -> Option<Jump> {
        if self.idx + 1 < self.list.len() {
            self.idx += 1;
            return Some(self.list[self.idx].clone());
        }
        None
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: usize) -> Jump {
        Jump { buffer: 1, path: None, line, col: 0 }
    }

    #[test]
    fn back_and_forth() {
        let mut j = JumpList::default();
        j.push(at(1));
        j.push(at(10));
        // now on line 20
        assert_eq!(j.back(at(20)), Some(at(10)));
        assert_eq!(j.back(at(10)), Some(at(1)));
        assert_eq!(j.back(at(1)), None);
        assert_eq!(j.forward(), Some(at(10)));
        assert_eq!(j.forward(), Some(at(20)), "the place Ctrl+O started from");
        assert_eq!(j.forward(), None);
    }

    #[test]
    fn same_line_is_kept_once_and_new_jumps_reset() {
        let mut j = JumpList::default();
        j.push(at(5));
        j.push(at(8));
        j.push(at(5));
        assert_eq!(j.len(), 2);
        assert_eq!(j.back(at(9)), Some(at(5)));
        j.push(at(30));
        assert_eq!(j.forward(), None, "a new jump ends the walk");
        assert_eq!(j.back(at(31)), Some(at(30)));
    }

    #[test]
    fn other_files() {
        let mut j = JumpList::default();
        j.push(Jump { buffer: 1, path: Some("/a".into()), line: 3, col: 2 });
        let back = j.back(Jump { buffer: 2, path: Some("/b".into()), line: 3, col: 0 }).unwrap();
        assert_eq!((back.buffer, back.line, back.col), (1, 3, 2));
    }
}
