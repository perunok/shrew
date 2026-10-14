//! Multi-cursor selections (Sublime/Helix style): every edit and motion applies to all ranges.

use crate::change::{Assoc, ChangeSet};
use smallvec::{SmallVec, smallvec};

/// One cursor. `head` is where the cursor is, `anchor` the other end of the selection. Empty when equal.
/// `col` remembers the display column for vertical moves (the "sticky" column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub anchor: usize,
    pub head: usize,
    pub col: Option<usize>,
}

impl Range {
    pub fn new(anchor: usize, head: usize) -> Self {
        Range { anchor, head, col: None }
    }
    pub fn point(pos: usize) -> Self {
        Range::new(pos, pos)
    }
    #[inline]
    pub fn from(&self) -> usize {
        self.anchor.min(self.head)
    }
    #[inline]
    pub fn to(&self) -> usize {
        self.anchor.max(self.head)
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
    pub fn len(&self) -> usize {
        self.to() - self.from()
    }
    pub fn forward(&self) -> bool {
        self.head >= self.anchor
    }
    pub fn map(&self, cs: &ChangeSet) -> Range {
        if self.is_empty() {
            let p = cs.map(self.head, Assoc::After);
            return Range { anchor: p, head: p, col: self.col };
        }
        // the selection grows with text typed at its edges only on the outside
        let (a_assoc, h_assoc) =
            if self.forward() { (Assoc::After, Assoc::Before) } else { (Assoc::Before, Assoc::After) };
        Range { anchor: cs.map(self.anchor, a_assoc), head: cs.map(self.head, h_assoc), col: self.col }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    ranges: SmallVec<[Range; 1]>,
    primary: usize,
}

impl Selection {
    pub fn point(pos: usize) -> Self {
        Selection { ranges: smallvec![Range::point(pos)], primary: 0 }
    }

    pub fn single(r: Range) -> Self {
        Selection { ranges: smallvec![r], primary: 0 }
    }

    /// Builds a normalised selection: sorted, overlapping ranges merged. `primary` indexes into `ranges`.
    pub fn new(ranges: impl IntoIterator<Item = Range>, primary: usize) -> Self {
        let ranges: SmallVec<[Range; 1]> = ranges.into_iter().collect();
        assert!(!ranges.is_empty(), "a selection needs at least one range");
        let primary = primary.min(ranges.len() - 1);
        let mut s = Selection { ranges, primary };
        s.normalize();
        s
    }

    fn normalize(&mut self) {
        if self.ranges.len() == 1 {
            return;
        }
        let prim = self.ranges[self.primary];
        let mut idx: SmallVec<[(Range, bool); 4]> =
            self.ranges.iter().enumerate().map(|(i, r)| (*r, i == self.primary)).collect();
        idx.sort_by_key(|(r, _)| (r.from(), r.to()));
        let mut out: SmallVec<[Range; 1]> = SmallVec::new();
        let mut primary = 0;
        for (r, is_p) in idx {
            if let Some(last) = out.last_mut() {
                let overlaps = r.from() < last.to()
                    || (r.from() == last.from() && (r.is_empty() || last.is_empty()) && r.from() == last.to());
                if overlaps || (r.from() == last.from() && r.to() == last.to()) {
                    let from = last.from().min(r.from());
                    let to = last.to().max(r.to());
                    *last = if last.forward() { Range::new(from, to) } else { Range::new(to, from) };
                    if is_p {
                        primary = out.len() - 1;
                    }
                    continue;
                }
            }
            if is_p {
                primary = out.len();
            }
            out.push(r);
        }
        let _ = prim;
        self.ranges = out;
        self.primary = primary;
    }

    #[inline]
    pub fn ranges(&self) -> &[Range] {
        &self.ranges
    }
    #[inline]
    pub fn primary(&self) -> Range {
        self.ranges[self.primary]
    }
    #[inline]
    pub fn primary_index(&self) -> usize {
        self.primary
    }
    pub fn len(&self) -> usize {
        self.ranges.len()
    }
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Applies `f` to every range and re-normalises.
    pub fn transform(&self, mut f: impl FnMut(Range) -> Range) -> Selection {
        Selection::new(self.ranges.iter().map(|r| f(*r)), self.primary)
    }

    pub fn map(&self, cs: &ChangeSet) -> Selection {
        if self.ranges.len() < 32 {
            return self.transform(|r| r.map(cs));
        }
        // many cursors: map every end in one sorted pass instead of scanning the changes per cursor
        let mut ends: Vec<(usize, Assoc)> = Vec::with_capacity(self.ranges.len() * 2);
        let mut order: Vec<usize> = Vec::with_capacity(self.ranges.len() * 2);
        for r in &self.ranges {
            let (a, h) = if r.is_empty() {
                (Assoc::After, Assoc::After)
            } else if r.forward() {
                (Assoc::After, Assoc::Before)
            } else {
                (Assoc::Before, Assoc::After)
            };
            ends.push((r.anchor, a));
            ends.push((r.head, h));
        }
        order.extend(0..ends.len());
        order.sort_by_key(|&i| ends[i].0);
        let mut sorted: Vec<(usize, Assoc)> = order.iter().map(|&i| ends[i]).collect();
        cs.map_sorted(&mut sorted);
        for (k, &i) in order.iter().enumerate() {
            ends[i].0 = sorted[k].0;
        }
        Selection::new(
            self.ranges.iter().enumerate().map(|(i, r)| Range {
                anchor: ends[2 * i].0,
                head: ends[2 * i + 1].0,
                col: r.col,
            }),
            self.primary,
        )
    }

    pub fn push(&self, r: Range) -> Selection {
        let mut ranges = self.ranges.clone();
        ranges.push(r);
        let p = ranges.len() - 1;
        Selection::new(ranges, p)
    }

    pub fn only_primary(&self) -> Selection {
        Selection::single(self.primary())
    }

    pub fn collapse(&self) -> Selection {
        self.transform(|r| Range { anchor: r.head, head: r.head, col: r.col })
    }

    pub fn clamp(&self, len: usize) -> Selection {
        self.transform(|r| Range { anchor: r.anchor.min(len), head: r.head.min(len), col: r.col })
    }

    pub fn remove(&self, i: usize) -> Selection {
        if self.ranges.len() == 1 {
            return self.clone();
        }
        let mut ranges = self.ranges.clone();
        ranges.remove(i);
        let p = if self.primary >= i { self.primary.saturating_sub(1) } else { self.primary };
        Selection::new(ranges, p)
    }

    pub fn with_primary(mut self, i: usize) -> Selection {
        self.primary = i.min(self.ranges.len() - 1);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_overlaps_and_keeps_primary() {
        let s = Selection::new([Range::new(5, 8), Range::new(0, 2), Range::new(7, 10), Range::point(20)], 2);
        assert_eq!(s.ranges(), &[Range::new(0, 2), Range::new(5, 10), Range::point(20)]);
        assert_eq!(s.primary_index(), 1);
        let s = Selection::new([Range::point(3), Range::point(3)], 1);
        assert_eq!(s.len(), 1);
    }
}
