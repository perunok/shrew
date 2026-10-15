//! The diff view's model, independent of git and of drawing: the changes between two texts laid out as rows.
//!
//! Each file contributes rows in order: a header (multi-file diffs), then its lines. Unchanged lines far from a
//! change fold into one row ("··· 42 unchanged lines ···") that opens on request. In the split layout a changed
//! block takes as many rows as its longer side, pairing its old and new lines row by row and leaving the shorter
//! side blank; the unified layout gives every removed line its row, then every added one. Rows are plain numbers
//! (20 bytes each), so a 100k-line file is a few megabytes and builds in about a millisecond.

use std::collections::BTreeSet;
use std::ops::Range;

/// No line on that side.
pub const NONE: u32 = u32::MAX;

/// Context lines that mean "show everything".
pub const ALL: u32 = u32::MAX;

/// One change: old lines `old` became new lines `new` (0-based; an empty range is a pure insertion or deletion
/// anchored before that line). The same shape as git's hunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub old: Range<u32>,
    pub new: Range<u32>,
}

impl Change {
    pub fn new(old: Range<u32>, new: Range<u32>) -> Change {
        Change { old, new }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Layout {
    /// Old on the left, new on the right, aligned.
    #[default]
    Split,
    /// One column: removed lines, then added ones.
    Unified,
}

impl Layout {
    pub fn name(self) -> &'static str {
        match self {
            Layout::Split => "split",
            Layout::Unified => "unified",
        }
    }

    pub fn from_name(s: &str) -> Option<Layout> {
        match s {
            "split" | "side-by-side" | "side_by_side" => Some(Layout::Split),
            "unified" | "inline" => Some(Layout::Unified),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// An unchanged line, on both sides.
    Same,
    /// A row of a change: a removed line, an added one, or (split) one of each.
    Change,
    /// Unchanged lines folded away.
    Fold,
    /// A file's header (multi-file diffs).
    File,
    /// A message instead of lines: loading, binary, an error. The app knows which.
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    /// The file it belongs to (index into the files given to [`build`]).
    pub file: u32,
    /// Line numbers, [`NONE`] when the side has no line here. A fold names its first lines.
    pub old: u32,
    pub new: u32,
    /// A change row's hunk index in its file; the number of lines a fold hides; 0 otherwise.
    pub n: u32,
}

impl Row {
    pub fn old(&self) -> Option<u32> {
        (self.old != NONE).then_some(self.old)
    }

    pub fn new_line(&self) -> Option<u32> {
        (self.new != NONE).then_some(self.new)
    }

    pub fn hunk(&self) -> Option<u32> {
        (self.kind == RowKind::Change).then_some(self.n)
    }
}

/// One file's changes and how it is shown.
#[derive(Clone, Debug, Default)]
pub struct FileModel {
    pub changes: Vec<Change>,
    pub old_len: u32,
    pub new_len: u32,
    /// Starts with a header row (multi-file diffs).
    pub header: bool,
    /// Only the header shows.
    pub collapsed: bool,
    /// One note row instead of lines (still loading, binary, failed).
    pub note: bool,
    /// Folds the user opened: a fold opens when it contains one of these old lines.
    pub unfolded: BTreeSet<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub layout: Layout,
    /// Unchanged lines kept around each change ([`ALL`]: nothing folds).
    pub context: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options { layout: Layout::Split, context: 3 }
    }
}

/// Built rows plus indices for navigation.
#[derive(Clone, Debug, Default)]
pub struct Rows {
    pub rows: Vec<Row>,
    /// The first row of every change, in order (all files).
    pub starts: Vec<u32>,
    /// Each file's first row (its header, or its first line).
    pub files: Vec<u32>,
}

impl Rows {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The first row of the next (or previous) change after `row`.
    pub fn next_change(&self, row: usize, forward: bool) -> Option<usize> {
        let row = row as u32;
        if forward {
            let i = self.starts.partition_point(|&s| s <= row);
            self.starts.get(i).map(|&s| s as usize)
        } else {
            // inside a change, the previous one is the change before the one the row belongs to
            let i = self.starts.partition_point(|&s| s < row);
            let mut i = i.checked_sub(1)?;
            let cur = self.rows.get(row as usize);
            if let Some(r) = cur
                && r.kind == RowKind::Change
                && self.rows[self.starts[i] as usize].file == r.file
                && self.rows[self.starts[i] as usize].n == r.n
            {
                i = i.checked_sub(1)?;
            }
            Some(self.starts[i] as usize)
        }
    }

    /// The first row of the next (or previous) file.
    pub fn next_file(&self, row: usize, forward: bool) -> Option<usize> {
        let row = row as u32;
        if forward {
            let i = self.files.partition_point(|&s| s <= row);
            self.files.get(i).map(|&s| s as usize)
        } else {
            let i = self.files.partition_point(|&s| s < row);
            self.files.get(i.checked_sub(1)?).map(|&s| s as usize)
        }
    }

    /// The rows of the change `row` is part of.
    pub fn change_rows(&self, row: usize) -> Option<Range<usize>> {
        let r = *self.rows.get(row)?;
        let hunk = r.hunk()?;
        let same = |x: &Row| x.kind == RowKind::Change && x.file == r.file && x.n == hunk;
        let mut a = row;
        while a > 0 && same(&self.rows[a - 1]) {
            a -= 1;
        }
        let mut z = row + 1;
        while z < self.rows.len() && same(&self.rows[z]) {
            z += 1;
        }
        Some(a..z)
    }

    /// The row showing `line` of `file` (on the new side, or the old side with `old`), or the nearest one before
    /// it when that line is folded away or gone. For keeping the cursor in place when the rows are rebuilt.
    pub fn find(&self, file: u32, line: u32, old: bool) -> Option<usize> {
        let first = *self.files.get(file as usize)? as usize;
        let end = self.files.get(file as usize + 1).map_or(self.rows.len(), |&e| e as usize);
        let mut best = None;
        for (i, r) in self.rows[first..end].iter().enumerate() {
            let l = if old { r.old } else { r.new };
            if l == NONE {
                best = best.or(Some(first + i));
                continue;
            }
            let span = if r.kind == RowKind::Fold { r.n.max(1) } else { 1 };
            if l <= line {
                best = Some(first + i);
                if line < l + span {
                    break;
                }
            } else {
                break;
            }
        }
        best.or_else(|| (first < end).then_some(first))
    }

    /// The changed lines of rows `range`, per file, each side as merged ranges.
    pub fn selected_lines(&self, range: Range<usize>) -> Vec<Picked> {
        let mut out: Vec<Picked> = Vec::new();
        let end = range.end.min(self.rows.len());
        for r in &self.rows[range.start.min(end)..end] {
            if r.kind != RowKind::Change {
                continue;
            }
            if out.last().is_none_or(|o| o.file != r.file) {
                out.push(Picked { file: r.file, old: Vec::new(), new: Vec::new() });
            }
            let entry = out.last_mut().expect("just pushed");
            for (line, ranges) in [(r.old, &mut entry.old), (r.new, &mut entry.new)] {
                if line == NONE {
                    continue;
                }
                match ranges.iter_mut().find(|x| x.end == line || x.start == line + 1) {
                    Some(x) if x.end == line => x.end += 1,
                    Some(x) => x.start = line,
                    None => ranges.push(line..line + 1),
                }
            }
        }
        for p in &mut out {
            p.old.sort_by_key(|r| r.start);
            p.new.sort_by_key(|r| r.start);
        }
        out
    }
}

/// Changed lines picked in one file: old (removed) and new (added) line ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picked {
    pub file: u32,
    pub old: Vec<Range<u32>>,
    pub new: Vec<Range<u32>>,
}

/// Lays out `files` as rows.
pub fn build(files: &[FileModel], opts: Options) -> Rows {
    let mut out = Rows::default();
    let estimate: usize = files.iter().map(|f| if f.collapsed { 1 } else { f.new_len.min(1 << 20) as usize }).sum();
    out.rows.reserve(estimate.min(1 << 20));
    for (i, f) in files.iter().enumerate() {
        let file = i as u32;
        out.files.push(out.rows.len() as u32);
        if f.header {
            out.rows.push(Row { kind: RowKind::File, file, old: NONE, new: NONE, n: 0 });
            if f.collapsed {
                continue;
            }
        }
        if f.note {
            out.rows.push(Row { kind: RowKind::Note, file, old: NONE, new: NONE, n: 0 });
            continue;
        }
        let (mut o, mut n) = (0u32, 0u32);
        let count = f.changes.len();
        for (k, c) in f.changes.iter().enumerate() {
            let gap = c.old.start.saturating_sub(o);
            unchanged(&mut out.rows, f, file, (o, n), gap, k > 0, true, opts.context);
            out.starts.push(out.rows.len() as u32);
            change(&mut out.rows, file, k as u32, c, opts.layout);
            (o, n) = (c.old.end, c.new.end);
        }
        let tail = f.old_len.saturating_sub(o).min(f.new_len.saturating_sub(n));
        unchanged(&mut out.rows, f, file, (o, n), tail, count > 0, false, opts.context);
    }
    out
}

/// `len` unchanged lines from (`old`, `new`): all of them, or `context` next to the changes around a fold.
#[allow(clippy::too_many_arguments)]
fn unchanged(
    rows: &mut Vec<Row>,
    f: &FileModel,
    file: u32,
    (old, new): (u32, u32),
    len: u32,
    after_change: bool,
    before_change: bool,
    context: u32,
) {
    let lead = if after_change { context.min(len) } else { 0 };
    let trail = if before_change { context.min(len - lead) } else { 0 };
    let hidden = len - lead - trail;
    let open = context == ALL || hidden <= 1 || f.unfolded.range(old..old + len).next().is_some();
    let same =
        |rows: &mut Vec<Row>, k: u32| rows.push(Row { kind: RowKind::Same, file, old: old + k, new: new + k, n: 0 });
    if open {
        (0..len).for_each(|k| same(rows, k));
        return;
    }
    (0..lead).for_each(|k| same(rows, k));
    rows.push(Row { kind: RowKind::Fold, file, old: old + lead, new: new + lead, n: hidden });
    (lead + hidden..len).for_each(|k| same(rows, k));
}

fn change(rows: &mut Vec<Row>, file: u32, hunk: u32, c: &Change, layout: Layout) {
    let row = |old: u32, new: u32| Row { kind: RowKind::Change, file, old, new, n: hunk };
    let (removed, added) = (c.old.end - c.old.start, c.new.end - c.new.start);
    match layout {
        Layout::Split => {
            for k in 0..removed.max(added) {
                let o = if k < removed { c.old.start + k } else { NONE };
                let n = if k < added { c.new.start + k } else { NONE };
                rows.push(row(o, n));
            }
        }
        Layout::Unified => {
            rows.extend(c.old.clone().map(|o| row(o, NONE)));
            rows.extend(c.new.clone().map(|n| row(NONE, n)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(changes: &[(Range<u32>, Range<u32>)], old_len: u32, new_len: u32) -> FileModel {
        FileModel {
            changes: changes.iter().map(|(o, n)| Change::new(o.clone(), n.clone())).collect(),
            old_len,
            new_len,
            ..FileModel::default()
        }
    }

    fn opts(layout: Layout, context: u32) -> Options {
        Options { layout, context }
    }

    /// Rows as "kind old new": `= 3 3`, `~ 4 -`, `fold 5 5 x10`.
    fn show(r: &Rows) -> Vec<String> {
        let n = |x: u32| if x == NONE { "-".to_string() } else { x.to_string() };
        r.rows
            .iter()
            .map(|r| match r.kind {
                RowKind::Same => format!("= {} {}", n(r.old), n(r.new)),
                RowKind::Change => format!("~ {} {}", n(r.old), n(r.new)),
                RowKind::Fold => format!("fold {} {} x{}", r.old, r.new, r.n),
                RowKind::File => format!("file {}", r.file),
                RowKind::Note => format!("note {}", r.file),
            })
            .collect()
    }

    #[test]
    fn split_pairs_lines_and_pads_the_shorter_side() {
        // old: a b c d, new: a X Y Z d (b c -> X Y Z)
        let f = file(&[(1..3, 1..4)], 4, 5);
        let r = build(&[f], opts(Layout::Split, ALL));
        assert_eq!(show(&r), ["= 0 0", "~ 1 1", "~ 2 2", "~ - 3", "= 3 4"]);
        assert_eq!(r.starts, [1]);
        // a pure deletion and a pure insertion
        let f = file(&[(1..2, 1..1), (3..3, 2..4)], 4, 5);
        let r = build(&[f], opts(Layout::Split, ALL));
        assert_eq!(show(&r), ["= 0 0", "~ 1 -", "= 2 1", "~ - 2", "~ - 3", "= 3 4"]);
    }

    #[test]
    fn unified_lists_removed_then_added() {
        let f = file(&[(1..3, 1..4)], 4, 5);
        let r = build(&[f], opts(Layout::Unified, ALL));
        assert_eq!(show(&r), ["= 0 0", "~ 1 -", "~ 2 -", "~ - 1", "~ - 2", "~ - 3", "= 3 4"]);
    }

    #[test]
    fn far_unchanged_lines_fold_and_open() {
        // 100 lines, line 50 changed
        let mut f = file(&[(50..51, 50..51)], 100, 100);
        let r = build(std::slice::from_ref(&f), opts(Layout::Split, 3));
        assert_eq!(
            show(&r),
            [
                "fold 0 0 x47",
                "= 47 47",
                "= 48 48",
                "= 49 49",
                "~ 50 50",
                "= 51 51",
                "= 52 52",
                "= 53 53",
                "fold 54 54 x46"
            ]
        );
        // opening the first fold shows its lines, the other stays
        f.unfolded.insert(10);
        let r = build(std::slice::from_ref(&f), opts(Layout::Split, 3));
        assert_eq!(r.len(), 50 + 1 + 3 + 1);
        assert_eq!(r.rows[0].kind, RowKind::Same);
        assert_eq!(r.rows.last().unwrap().kind, RowKind::Fold);
        // a gap of one hidden line is not worth a fold
        let f = file(&[(3..4, 3..4), (11..12, 11..12)], 20, 20);
        let r = build(&[f], opts(Layout::Split, 3));
        assert!(!r.rows[..12].iter().any(|r| r.kind == RowKind::Fold), "{:?}", show(&r));
        // identical texts: one fold
        let r = build(&[file(&[], 30, 30)], opts(Layout::Split, 3));
        assert_eq!(show(&r), ["fold 0 0 x30"]);
        assert!(build(&[file(&[], 0, 0)], opts(Layout::Split, 3)).is_empty());
    }

    #[test]
    fn files_have_headers_and_collapse() {
        let mut a = file(&[(0..1, 0..1)], 1, 1);
        a.header = true;
        let mut b = a.clone();
        b.collapsed = true;
        let mut c = file(&[], 0, 0);
        c.header = true;
        c.note = true;
        let r = build(&[a, b, c], opts(Layout::Split, 3));
        assert_eq!(show(&r), ["file 0", "~ 0 0", "file 1", "file 2", "note 2"]);
        assert_eq!(r.files, [0, 2, 3]);
        assert_eq!(r.starts, [1], "a collapsed file's changes are not stops");
        assert_eq!(r.next_file(0, true), Some(2));
        assert_eq!(r.next_file(3, false), Some(2));
        assert_eq!(r.next_file(0, false), None);
    }

    #[test]
    fn hunk_navigation() {
        let f = file(&[(2..4, 2..3), (10..10, 9..11), (20..21, 20..21)], 30, 30);
        let r = build(&[f], opts(Layout::Split, ALL));
        let starts: Vec<usize> = r.starts.iter().map(|&s| s as usize).collect();
        assert_eq!(r.next_change(0, true), Some(starts[0]));
        assert_eq!(r.next_change(starts[0], true), Some(starts[1]));
        assert_eq!(r.next_change(starts[0] + 1, true), Some(starts[1]), "from inside a change");
        assert_eq!(r.next_change(starts[2], true), None);
        assert_eq!(r.next_change(starts[2] + 3, false), Some(starts[2]));
        assert_eq!(r.next_change(starts[1] + 1, false), Some(starts[0]), "inside: the one before");
        assert_eq!(r.next_change(starts[0], false), None);
        assert_eq!(r.change_rows(starts[0] + 1), Some(starts[0]..starts[0] + 2));
        assert_eq!(r.change_rows(0), None);
    }

    #[test]
    fn find_keeps_the_cursor_on_its_line() {
        let f = file(&[(50..51, 50..52)], 100, 101);
        let r = build(&[f], opts(Layout::Split, 3));
        let at = r.find(0, 51, false).unwrap();
        assert_eq!(r.rows[at].new, 51);
        // a folded line maps to its fold
        let at = r.find(0, 10, false).unwrap();
        assert_eq!(r.rows[at].kind, RowKind::Fold);
        let at = r.find(0, 90, true).unwrap();
        assert_eq!(r.rows[at].kind, RowKind::Fold);
        assert_eq!(r.find(3, 0, false), None);
    }

    /// A pick in file 0, ranges given as (start, end).
    fn picked(old: &[(u32, u32)], new: &[(u32, u32)]) -> Picked {
        let r = |v: &[(u32, u32)]| v.iter().map(|&(a, z)| a..z).collect();
        Picked { file: 0, old: r(old), new: r(new) }
    }

    #[test]
    fn selection_to_lines() {
        let f = file(&[(1..3, 1..4), (6..7, 7..7)], 8, 8);
        let r = build(&[f], opts(Layout::Split, ALL));
        // rows: =0 0, ~1 1, ~2 2, ~- 3, =3 4, =4 5, =5 6, ~6 -, =7 7
        assert_eq!(r.selected_lines(2..4), vec![picked(&[(2, 3)], &[(2, 4)])]);
        assert_eq!(r.selected_lines(0..r.len()), vec![picked(&[(1, 3), (6, 7)], &[(1, 4)])]);
        assert!(r.selected_lines(4..6).is_empty());
        let r = build(&[file(&[(1..3, 1..4)], 4, 5)], opts(Layout::Unified, ALL));
        assert_eq!(r.selected_lines(2..5), vec![picked(&[(2, 3)], &[(1, 3)])]);
    }
}
