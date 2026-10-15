//! Partial staging: pick lines out of a diff and build the text and the patch that apply only those.
//!
//! Staging a selection never edits a diff by hand. It computes the *target* (the index text with just the
//! selected changes applied, [`apply_selection`]), then diffs the current index text against that target and
//! applies the result with `git apply --cached`. The patch's context therefore always matches the index exactly,
//! even next to unselected changes, and git refuses it if the index moved in the meantime.

use crate::diff::{EolMode, Hunk, diff, strips_cr, unified};
use crate::text::{Eol, Text, strip_eol};
use std::borrow::Cow;
use std::ops::Range;

/// Lines picked from a diff: deleted lines by old line number, added lines by new line number (0-based).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineSel {
    pub old: Vec<Range<u32>>,
    pub new: Vec<Range<u32>>,
}

impl LineSel {
    /// Every line of these hunks.
    pub fn hunks<'a>(hunks: impl IntoIterator<Item = &'a Hunk>) -> LineSel {
        let mut sel = LineSel::default();
        for h in hunks {
            sel.old.push(h.old.clone());
            sel.new.push(h.new.clone());
        }
        sel
    }

    /// The changes on new lines `lines`, as selected in the editor: the added lines in the range, the deletions
    /// anchored in it, and for a modified hunk the deleted lines paired with the selected added lines (the k-th
    /// deleted line goes with the k-th added one; leftover deleted lines go with the last added line).
    pub fn new_lines(hunks: &[Hunk], lines: Range<u32>) -> LineSel {
        let mut sel = LineSel::default();
        let lines = lines.start..lines.end.max(lines.start + 1);
        for h in hunks {
            if h.new.is_empty() {
                if lines.contains(&h.new.start) || h.new.start == lines.end {
                    // a deletion is anchored between lines: take it when the line just above or below is selected
                    sel.old.push(h.old.clone());
                }
                continue;
            }
            let picked = h.new.start.max(lines.start)..h.new.end.min(lines.end);
            if picked.is_empty() {
                continue;
            }
            sel.new.push(picked.clone());
            let (first, last) = (picked.start - h.new.start, picked.end - h.new.start);
            let old_end = if picked.end == h.new.end { h.old.end } else { (h.old.start + last).min(h.old.end) };
            let old_start = (h.old.start + first).min(old_end);
            if old_start < old_end {
                sel.old.push(old_start..old_end);
            }
        }
        sel
    }

    pub fn is_empty(&self) -> bool {
        self.old.iter().all(|r| r.is_empty()) && self.new.iter().all(|r| r.is_empty())
    }

    pub(crate) fn has_old(&self, line: u32) -> bool {
        self.old.iter().any(|r| r.contains(&line))
    }

    pub(crate) fn has_new(&self, line: u32) -> bool {
        self.new.iter().any(|r| r.contains(&line))
    }

    /// The same selection seen from the other side (for reverting).
    pub fn invert(&self) -> LineSel {
        LineSel { old: self.new.clone(), new: self.old.clone() }
    }

    /// Whether this selects every change of `hunks`.
    pub fn covers(&self, hunks: &[Hunk]) -> bool {
        hunks.iter().all(|h| h.old.clone().all(|l| self.has_old(l)) && h.new.clone().all(|l| self.has_new(l)))
    }
}

/// `old` with the selected changes of `hunks` (the diff `old -> new`) applied. Unselected deletions stay,
/// unselected additions are left out. Under [`EolMode::Normalize`] lines taken from `new` get `old`'s line ending,
/// as git's `autocrlf` would store them.
pub fn apply_selection(old: &Text, new: &Text, hunks: &[Hunk], sel: &LineSel, eol: EolMode) -> Vec<u8> {
    let convert = strips_cr(old, new, eol).then(|| old.line_ending());
    let mut pieces: Vec<Cow<[u8]>> = Vec::with_capacity(old.len_lines() + 8);
    let mut at = 0u32;
    for h in hunks {
        for i in at..h.old.start {
            pieces.push(Cow::Borrowed(old.line(i as usize)));
        }
        for i in h.old.clone() {
            if !sel.has_old(i) {
                pieces.push(Cow::Borrowed(old.line(i as usize)));
            }
        }
        for i in h.new.clone() {
            if sel.has_new(i) {
                let line = new.line(i as usize);
                pieces.push(match convert {
                    Some(ending) if strip_eol(line).1 != Eol::None => {
                        let mut l = strip_eol(line).0.to_vec();
                        l.extend_from_slice(ending.as_bytes());
                        Cow::Owned(l)
                    }
                    _ => Cow::Borrowed(line),
                });
            }
        }
        at = h.old.end;
    }
    for i in at..old.len_lines() as u32 {
        pieces.push(Cow::Borrowed(old.line(i as usize)));
    }
    // A line without a newline can only be last: one that ended the file before is followed by more now.
    let ending = old.line_ending();
    let mut out = Vec::with_capacity(old.bytes().len() + 64);
    let n = pieces.len();
    for (k, p) in pieces.into_iter().enumerate() {
        out.extend_from_slice(&p);
        if k + 1 < n && p.last() != Some(&b'\n') {
            out.extend_from_slice(ending.as_bytes());
        }
    }
    out
}

/// `new` with the selected changes of `hunks` (the diff `old -> new`) undone: what discarding them leaves.
pub fn revert_selection(old: &Text, new: &Text, hunks: &[Hunk], sel: &LineSel, eol: EolMode) -> Vec<u8> {
    let inverted: Vec<Hunk> = hunks.iter().map(Hunk::invert).collect();
    apply_selection(new, old, &inverted, &sel.invert(), eol)
}

/// One side of a file patch: its text and git file mode (`0o100644`, `0o100755`, `0o120000`).
#[derive(Copy, Clone)]
pub struct Side<'a> {
    pub text: &'a Text,
    pub mode: u32,
}

/// A complete `git apply` patch for one file, `None` meaning the file does not exist on that side.
/// Returns `None` when there is nothing to change.
pub fn file_patch(path: &str, old: Option<Side>, new: Option<Side>, context: u32) -> Option<Vec<u8>> {
    let empty = Text::default();
    let (ot, nt) = (old.map_or(&empty, |s| s.text), new.map_or(&empty, |s| s.text));
    let hunks = diff(ot, nt, EolMode::Exact);
    let mode_change = matches!((old, new), (Some(o), Some(n)) if o.mode != n.mode);
    if hunks.is_empty() && !mode_change && old.is_some() == new.is_some() {
        return None;
    }
    let (a, b) = (quote_path("a/", path), quote_path("b/", path));
    let mut out = format!("diff --git {a} {b}\n").into_bytes();
    match (old, new) {
        (None, Some(n)) => out.extend_from_slice(format!("new file mode {:o}\n", n.mode).as_bytes()),
        (Some(o), None) => out.extend_from_slice(format!("deleted file mode {:o}\n", o.mode).as_bytes()),
        (Some(o), Some(n)) if mode_change => {
            out.extend_from_slice(format!("old mode {:o}\nnew mode {:o}\n", o.mode, n.mode).as_bytes())
        }
        _ => {}
    }
    if !hunks.is_empty() {
        let from = if old.is_some() { a } else { "/dev/null".into() };
        let to = if new.is_some() { b } else { "/dev/null".into() };
        out.extend_from_slice(format!("--- {from}\n+++ {to}\n").as_bytes());
        out.extend_from_slice(&unified(ot, nt, &hunks, context));
    }
    Some(out)
}

/// A path in a patch header, C-quoted the way git writes it when it holds special bytes.
pub(crate) fn quote_path(prefix: &str, path: &str) -> String {
    let needs = path.bytes().any(|b| b < 0x20 || b == b'"' || b == b'\\' || b >= 0x7f);
    if !needs {
        return format!("{prefix}{path}");
    }
    let mut s = String::from("\"");
    s.push_str(prefix);
    for b in path.bytes() {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\t' => s.push_str("\\t"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\{b:03o}")),
        }
    }
    s.push('"');
    s
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn sel_all(old: &str, new: &str) -> (Text, Text, Vec<Hunk>) {
        let (o, n) = (Text::new(old), Text::new(new));
        let h = diff(&o, &n, EolMode::Normalize);
        (o, n, h)
    }

    fn s(v: Vec<u8>) -> String {
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn whole_and_partial_selection() {
        let (o, n, h) = sel_all("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
        assert_eq!(h.len(), 2);
        assert_eq!(s(apply_selection(&o, &n, &h, &LineSel::hunks(&h), EolMode::Exact)), "a\nB\nc\nd\ne\n");
        assert_eq!(s(apply_selection(&o, &n, &h, &LineSel::hunks(&h[..1]), EolMode::Exact)), "a\nB\nc\nd\n");
        assert_eq!(s(apply_selection(&o, &n, &h, &LineSel::hunks(&h[1..]), EolMode::Exact)), "a\nb\nc\nd\ne\n");
        assert_eq!(s(revert_selection(&o, &n, &h, &LineSel::hunks(&h[1..]), EolMode::Exact)), "a\nB\nc\nd\n");
    }

    #[test]
    fn line_selection_in_a_modified_hunk() {
        let (o, n, h) = sel_all("x\n1\n2\n3\ny\n", "x\none\ntwo\nthree\nfour\ny\n");
        assert_eq!(h, vec![Hunk { old: 1..4, new: 1..5 }]);
        // select "two": replaces "2" only
        let sel = LineSel::new_lines(&h, 2..3);
        assert_eq!(sel, LineSel { old: vec![2..3], new: vec![2..3] });
        assert_eq!(s(apply_selection(&o, &n, &h, &sel, EolMode::Exact)), "x\n1\n3\ntwo\ny\n");
        // select "four": the extra added line, no deletion paired
        let sel = LineSel::new_lines(&h, 4..5);
        assert_eq!(sel, LineSel { old: vec![], new: vec![4..5] });
        // select the last two: "three" pairs with "3"
        let sel = LineSel::new_lines(&h, 3..5);
        assert_eq!(sel.old, vec![3..4]);
    }

    #[test]
    fn deletion_anchor_selection() {
        let (_, _, h) = sel_all("a\nb\nc\n", "a\nc\n");
        assert_eq!(LineSel::new_lines(&h, 1..2).old, vec![1..2]);
        assert_eq!(LineSel::new_lines(&h, 0..1).old, vec![1..2]);
        assert!(LineSel::new_lines(&h, 5..6).is_empty());
    }

    #[test]
    fn no_newline_at_eof_is_repaired() {
        // old ends without newline; staging only the appended line must not glue it to the old last line
        let (o, n, h) = sel_all("a\nb", "a\nb\nc\n");
        assert_eq!(h, vec![Hunk { old: 1..2, new: 1..3 }]);
        let sel = LineSel { old: vec![], new: vec![2..3] };
        assert_eq!(s(apply_selection(&o, &n, &h, &sel, EolMode::Exact)), "a\nb\nc\n");
    }

    #[test]
    fn crlf_lines_adopt_the_index_ending() {
        let (o, n, h) = sel_all("a\nb\n", "a\r\nB\r\nc\r\n");
        assert_eq!(h.len(), 1);
        let got = apply_selection(&o, &n, &h, &LineSel::hunks(&h), EolMode::Normalize);
        assert_eq!(s(got), "a\nB\nc\n");
        let back = revert_selection(&o, &n, &h, &LineSel::hunks(&h), EolMode::Normalize);
        assert_eq!(s(back), "a\r\nb\r\n");
    }

    #[test]
    fn patches() {
        let (o, n) = (Text::new("a\n"), Text::new("a\nb\n"));
        let p =
            file_patch("f.txt", Some(Side { text: &o, mode: 0o100644 }), Some(Side { text: &n, mode: 0o100644 }), 3);
        assert_eq!(s(p.unwrap()), "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1,2 @@\n a\n+b\n");
        let p = file_patch("new.txt", None, Some(Side { text: &n, mode: 0o100755 }), 3).unwrap();
        assert!(
            s(p).starts_with("diff --git a/new.txt b/new.txt\nnew file mode 100755\n--- /dev/null\n+++ b/new.txt\n")
        );
        assert!(
            file_patch("same", Some(Side { text: &o, mode: 0o100644 }), Some(Side { text: &o, mode: 0o100644 }), 3)
                .is_none()
        );
        assert_eq!(quote_path("a/", "é \"x\""), "\"a/\\303\\251 \\\"x\\\"\"");
    }
}
