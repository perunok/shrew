//! Line diffs with imara-diff (Histogram, as Helix does), word-level diffs inside changed line pairs, and unified
//! diff rendering. Pure functions over [`Text`]: no repository involved, so the UI can use them directly.
//!
//! Line numbers are 0-based throughout, in git's line counting (see [`Text`]).

use crate::text::{Eol, Text, strip_eol};
use imara_diff::{Algorithm, Diff, InternedInput};
use std::ops::Range;

/// One change: lines `old` of the old text became lines `new` of the new text. A pure insertion has an empty
/// `old` range (positioned before old line `old.start`), a pure deletion an empty `new` range (the lines were
/// removed just before new line `new.start`, which can equal the new line count when the end was deleted).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Hunk {
    pub old: Range<u32>,
    pub new: Range<u32>,
}

/// What a hunk does, for gutter marks.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum HunkKind {
    Added,
    Modified,
    Deleted,
}

impl Hunk {
    pub fn kind(&self) -> HunkKind {
        match (self.old.is_empty(), self.new.is_empty()) {
            (true, _) => HunkKind::Added,
            (false, true) => HunkKind::Deleted,
            (false, false) => HunkKind::Modified,
        }
    }

    /// Lines added (new side length).
    pub fn added(&self) -> u32 {
        self.new.end - self.new.start
    }

    /// Lines removed (old side length).
    pub fn removed(&self) -> u32 {
        self.old.end - self.old.start
    }

    /// The same change seen from the other side.
    pub fn invert(&self) -> Hunk {
        Hunk { old: self.new.clone(), new: self.old.clone() }
    }

    /// Whether new line `line` is part of this hunk or is where its deletion is anchored.
    pub fn touches_new_line(&self, line: u32) -> bool {
        self.new.contains(&line) || (self.new.is_empty() && self.new.start == line)
    }
}

/// How line endings are compared.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum EolMode {
    /// Byte for byte, as `git diff` between two commits.
    #[default]
    Exact,
    /// When the two sides use different line endings throughout (one CRLF, the other LF, as with
    /// `core.autocrlf`), compare lines without their `\r`. Used between the worktree or a buffer and the index.
    Normalize,
}

/// Whether CRs are ignored for this pair of texts under `mode`.
pub(crate) fn strips_cr(old: &Text, new: &Text, mode: EolMode) -> bool {
    mode == EolMode::Normalize && old.line_ending() != new.line_ending()
}

/// A line as a diff token: its content and whether a newline ends it.
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
struct Line<'a> {
    content: &'a [u8],
    newline: bool,
}

impl AsRef<[u8]> for Line<'_> {
    fn as_ref(&self) -> &[u8] {
        self.content
    }
}

fn lines(text: &Text, strip_cr: bool) -> impl Iterator<Item = Line<'_>> {
    (0..text.len_lines()).map(move |i| {
        let line = text.line(i);
        let (content, eol) = strip_eol(line);
        match eol {
            Eol::None => Line { content, newline: false },
            Eol::CrLf if !strip_cr => Line { content: &line[..line.len() - 1], newline: true },
            _ => Line { content, newline: true },
        }
    })
}

/// The changes turning `old` into `new`, in order.
pub fn diff(old: &Text, new: &Text, eol: EolMode) -> Vec<Hunk> {
    let strip = strips_cr(old, new, eol);
    let mut input = InternedInput::default();
    input.reserve(old.len_lines() as u32, new.len_lines() as u32);
    input.update_before(lines(old, strip));
    input.update_after(lines(new, strip));
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    diff.hunks().map(|h| Hunk { old: h.before, new: h.after }).collect()
}

/// Lines added and removed over all `hunks`.
pub fn stats(hunks: &[Hunk]) -> (u32, u32) {
    hunks.iter().fold((0, 0), |(a, r), h| (a + h.added(), r + h.removed()))
}

/// A highlighted piece of one line: bytes `start..end` of line `line` (terminator excluded).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub line: u32,
    pub start: u32,
    pub end: u32,
}

/// The words that changed inside a modified hunk, on each side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WordDiff {
    pub old: Vec<Span>,
    pub new: Vec<Span>,
}

impl WordDiff {
    pub fn is_empty(&self) -> bool {
        self.old.is_empty() && self.new.is_empty()
    }
}

/// Hunks bigger than this (bytes, both sides) get no word diff.
const WORD_DIFF_MAX_BYTES: usize = 32 * 1024;

/// Word-level changes inside `hunk` (only for modified hunks; empty when the lines have too little in common to
/// make word highlights useful, or the hunk is huge).
pub fn word_diff(old: &Text, new: &Text, hunk: &Hunk) -> WordDiff {
    if hunk.kind() != HunkKind::Modified {
        return WordDiff::default();
    }
    let old_lines = hunk.old.start as usize..hunk.old.end as usize;
    let new_lines = hunk.new.start as usize..hunk.new.end as usize;
    let (a, b) = (old.lines_bytes(old_lines.clone()), new.lines_bytes(new_lines.clone()));
    if a.len() + b.len() > WORD_DIFF_MAX_BYTES {
        return WordDiff::default();
    }
    let (ta, tb) = (words(a), words(b));
    let mut input = InternedInput::default();
    input.update_before(ta.iter().map(|r| &a[r.clone()]));
    input.update_after(tb.iter().map(|r| &b[r.clone()]));
    let mut diff = Diff::compute(Algorithm::Myers, &input);
    diff.postprocess_no_heuristic(&input);

    let mut old_changed = Vec::new();
    let mut new_changed = Vec::new();
    for h in diff.hunks() {
        if !h.before.is_empty() {
            old_changed.push(ta[h.before.start as usize].start..ta[h.before.end as usize - 1].end);
        }
        if !h.after.is_empty() {
            new_changed.push(tb[h.after.start as usize].start..tb[h.after.end as usize - 1].end);
        }
    }
    // When most of the text changed, the whole lines are the change: word marks would only be noise.
    let mostly = |changed: &[Range<usize>], text: &[u8]| {
        let solid = |r: Range<usize>| text[r].iter().filter(|b| !b.is_ascii_whitespace()).count();
        let total = solid(0..text.len()).max(1);
        changed.iter().map(|r| solid(r.clone())).sum::<usize>() * 10 > total * 6
    };
    if mostly(&old_changed, a) && mostly(&new_changed, b) {
        return WordDiff::default();
    }
    WordDiff { old: spans(old, old_lines.start, &old_changed), new: spans(new, new_lines.start, &new_changed) }
}

/// Splits bytes into words (letters, digits, `_`, and any non-ASCII byte so UTF-8 is never cut), runs of spaces
/// and tabs, line breaks, and single punctuation bytes.
fn words(text: &[u8]) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |b: u8| {
        if b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80 {
            Class::Word
        } else if b == b' ' || b == b'\t' {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let c = class(text[i]);
        let mut j = i + 1;
        if c != Class::Other {
            while j < text.len() && class(text[j]) == c {
                j += 1;
            }
        }
        out.push(i..j);
        i = j;
    }
    out
}

/// Cuts byte ranges of a multi-line slice (starting at line `first`) into per-line spans without terminators.
fn spans(text: &Text, first: usize, ranges: &[Range<usize>]) -> Vec<Span> {
    let base = text.line_range(first..first).start;
    let mut out: Vec<Span> = Vec::new();
    let mut line = first;
    for r in ranges {
        let (start, end) = (base + r.start, base + r.end);
        while line < text.len_lines() && text.line_range(line..line + 1).end <= start {
            line += 1;
        }
        let mut l = line;
        while l < text.len_lines() {
            let lr = text.line_range(l..l + 1);
            if lr.start >= end {
                break;
            }
            let content_end = lr.start + text.line_content(l).len();
            let (s, e) = (start.max(lr.start), end.min(content_end));
            if s < e {
                let span = Span { line: l as u32, start: (s - lr.start) as u32, end: (e - lr.start) as u32 };
                match out.last_mut() {
                    Some(last) if last.line == span.line && last.end == span.start => last.end = span.end,
                    _ => out.push(span),
                }
            }
            l += 1;
        }
    }
    out
}

/// Hunks shown together with their surrounding context, as one `@@` section of a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// Old lines covered, context included.
    pub old: Range<u32>,
    /// New lines covered, context included.
    pub new: Range<u32>,
    /// Indices into the hunk list.
    pub hunks: Range<usize>,
}

/// Groups `hunks` with `context` lines around each, merging hunks whose context would touch (like `git diff -U`).
pub fn groups(hunks: &[Hunk], context: u32, old_len: u32) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for (i, h) in hunks.iter().enumerate() {
        let prev_end = if i == 0 { 0 } else { hunks[i - 1].old.end };
        let lead = context.min(h.old.start - prev_end);
        let trail = context.min(hunks.get(i + 1).map_or(old_len, |n| n.old.start) - h.old.end);
        let old = h.old.start - lead..h.old.end + trail;
        let new = h.new.start - lead..h.new.end + trail;
        match out.last_mut() {
            Some(g) if g.old.end >= old.start => {
                g.old.end = old.end;
                g.new.end = new.end;
                g.hunks.end = i + 1;
            }
            _ => out.push(Group { old, new, hunks: i..i + 1 }),
        }
    }
    out
}

/// The `@@` sections of a unified diff (no file headers), with `\ No newline at end of file` where needed.
/// Context lines come from `old`, so the output applies to `old` exactly.
pub fn unified(old: &Text, new: &Text, hunks: &[Hunk], context: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for g in groups(hunks, context, old.len_lines() as u32) {
        out.extend_from_slice(b"@@ -");
        range_header(&mut out, &g.old);
        out.extend_from_slice(b" +");
        range_header(&mut out, &g.new);
        out.extend_from_slice(b" @@\n");
        let mut o = g.old.start;
        for h in &hunks[g.hunks.clone()] {
            for i in o..h.old.start {
                push_line(&mut out, b' ', old.line(i as usize));
            }
            for i in h.old.clone() {
                push_line(&mut out, b'-', old.line(i as usize));
            }
            for i in h.new.clone() {
                push_line(&mut out, b'+', new.line(i as usize));
            }
            o = h.old.end;
        }
        for i in o..g.old.end {
            push_line(&mut out, b' ', old.line(i as usize));
        }
    }
    out
}

/// `start,len` with git's conventions: 1-based, `,1` omitted, and an empty range named by the line before it.
fn range_header(out: &mut Vec<u8>, r: &Range<u32>) {
    let len = r.end - r.start;
    let start = if len == 0 { r.start } else { r.start + 1 };
    if len == 1 {
        out.extend_from_slice(start.to_string().as_bytes());
    } else {
        out.extend_from_slice(format!("{start},{len}").as_bytes());
    }
}

fn push_line(out: &mut Vec<u8>, prefix: u8, line: &[u8]) {
    out.push(prefix);
    out.extend_from_slice(line);
    if line.last() != Some(&b'\n') {
        out.extend_from_slice(b"\n\\ No newline at end of file\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(a: &str, b: &str) -> Vec<Hunk> {
        diff(&Text::new(a), &Text::new(b), EolMode::Exact)
    }

    #[test]
    fn hunk_kinds() {
        let h = d("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(h, vec![Hunk { old: 1..2, new: 1..2 }, Hunk { old: 3..3, new: 3..4 }]);
        assert_eq!(h[0].kind(), HunkKind::Modified);
        assert_eq!(h[1].kind(), HunkKind::Added);
        let h = d("a\nb\nc\n", "a\nc\n");
        assert_eq!(h, vec![Hunk { old: 1..2, new: 1..1 }]);
        assert_eq!(h[0].kind(), HunkKind::Deleted);
        assert!(h[0].touches_new_line(1));
        assert_eq!(stats(&d("a\n", "b\nc\n")), (2, 1));
    }

    #[test]
    fn missing_final_newline_is_a_change() {
        assert_eq!(d("a\nb", "a\nb\n"), vec![Hunk { old: 1..2, new: 1..2 }]);
        assert!(d("a\nb", "a\nb").is_empty());
    }

    #[test]
    fn crlf_normalisation() {
        let (lf, crlf) = (Text::new("a\nb\nc\n"), Text::new("a\r\nB\r\nc\r\n"));
        assert_eq!(diff(&lf, &crlf, EolMode::Normalize), vec![Hunk { old: 1..2, new: 1..2 }]);
        assert_eq!(diff(&lf, &crlf, EolMode::Exact).len(), 1);
        assert_eq!(diff(&lf, &crlf, EolMode::Exact)[0], Hunk { old: 0..3, new: 0..3 });
        // same endings on both sides: a CR is content
        let mixed = Text::new("a\r\nb\nc\n");
        assert_eq!(diff(&lf, &mixed, EolMode::Normalize), vec![Hunk { old: 0..1, new: 0..1 }]);
    }

    #[test]
    fn unified_output_matches_git() {
        let old = Text::new("1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        let new = Text::new("1\n2\nthree\n4\n5\n6\n7\n8\n9\n10\n11");
        let hunks = diff(&old, &new, EolMode::Exact);
        let u = String::from_utf8(unified(&old, &new, &hunks, 3)).unwrap();
        assert_eq!(
            u,
            "@@ -1,6 +1,6 @@\n 1\n 2\n-3\n+three\n 4\n 5\n 6\n@@ -8,3 +8,4 @@\n 8\n 9\n 10\n+11\n\\ No newline at end of file\n"
        );
        let u = String::from_utf8(unified(&old, &new, &hunks, 0)).unwrap();
        assert_eq!(u, "@@ -3 +3 @@\n-3\n+three\n@@ -10,0 +11 @@\n+11\n\\ No newline at end of file\n");
        // close hunks share one section
        let u = unified(&old, &new, &hunks, 4);
        assert_eq!(u.iter().filter(|&&b| b == b'@').count(), 4);
    }

    #[test]
    fn unified_new_and_emptied_files() {
        let (empty, some) = (Text::new(""), Text::new("x\ny\n"));
        let u = unified(&empty, &some, &diff(&empty, &some, EolMode::Exact), 3);
        assert_eq!(u, b"@@ -0,0 +1,2 @@\n+x\n+y\n");
        let u = unified(&some, &empty, &diff(&some, &empty, EolMode::Exact), 3);
        assert_eq!(u, b"@@ -1,2 +0,0 @@\n-x\n-y\n");
    }

    #[test]
    fn word_level() {
        let old = Text::new("let x = foo(1, 2);\nkeep\n");
        let new = Text::new("let x = bar(1, 2, 3);\nkeep\n");
        let h = &diff(&old, &new, EolMode::Exact)[0];
        let w = word_diff(&old, &new, h);
        let pick = |t: &Text, s: &Span| {
            String::from_utf8(t.line(s.line as usize)[s.start as usize..s.end as usize].to_vec()).unwrap()
        };
        assert_eq!(w.old.iter().map(|s| pick(&old, s)).collect::<Vec<_>>(), ["foo"]);
        assert_eq!(w.new.iter().map(|s| pick(&new, s)).collect::<Vec<_>>(), ["bar", ", 3"]);
        // nothing in common: no word marks
        let (a, b) = (Text::new("alpha beta\n"), Text::new("gamma delta\n"));
        assert!(word_diff(&a, &b, &diff(&a, &b, EolMode::Exact)[0]).is_empty());
        // unicode stays whole
        let (a, b) = (Text::new("naïve café\n"), Text::new("naïve cafés\n"));
        let w = word_diff(&a, &b, &diff(&a, &b, EolMode::Exact)[0]);
        assert_eq!(w.new.iter().map(|s| pick(&b, s)).collect::<Vec<_>>(), ["cafés"]);
    }

    #[test]
    fn word_spans_cross_lines() {
        let old = Text::new("a b\nc d\n");
        let new = Text::new("a X\nY d\n");
        let w = word_diff(&old, &new, &diff(&old, &new, EolMode::Exact)[0]);
        assert_eq!(w.new, vec![Span { line: 0, start: 2, end: 3 }, Span { line: 1, start: 0, end: 1 }]);
    }
}
