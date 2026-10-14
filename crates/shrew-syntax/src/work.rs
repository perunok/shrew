//! The worker's side of one buffer: a rope snapshot, the parser states and the highlighting pass.
//!
//! Highlighted text is kept as *runs*: line ranges produced by one forward pass, each holding parser checkpoints
//! (every [`CHECKPOINT_EVERY`] lines and at its end, so a pass can resume there). The run starting at line 0 is
//! exact. An edit cuts runs at the first changed line; the lines after the edit keep their spans as a *tentative*
//! run. When the re-highlighting pass reaches one of its checkpoints with an identical parser state, the rest of
//! that run is known to be correct and is adopted without parsing it again: typing in a 20k-line file re-parses a
//! few lines, not the rest of the file.
//!
//! When the viewport is far past the highlighted text (opening a file at its end, jumping), a *guessed* run starts
//! from a fresh parser state at a top-level-looking line a little above it, so the screen gets colors within
//! milliseconds; the exact pass replaces (or, on a matching checkpoint, adopts) it later. Files over
//! [`WINDOWED_LINES`] lines or [`WINDOWED_BYTES`] bytes only ever get guessed runs around the viewport (as Sublime
//! does), and forget them as the viewport moves away.

use crate::detect::{self, LangHint};
use crate::theme::{Rgb, Span, Theme};
use ropey::Rope;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};
use syntect::highlighting::{FontStyle, HighlightState, Highlighter as SynHighlighter, RangedHighlightIterator};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

pub(crate) const CHECKPOINT_EVERY: usize = 64;
/// Lines longer than this (in chars) are drawn plain and skipped by the parser.
pub const MAX_LINE_CHARS: usize = 20_000;
/// Files bigger than this (lines or bytes) are only highlighted around the viewport.
pub const WINDOWED_LINES: usize = 100_000;
pub const WINDOWED_BYTES: usize = 8 << 20;
/// Start from a guessed state rather than parse more than this many lines to reach the viewport (~20 ms).
const FAR: usize = 500;
/// Lines parsed above the viewport when starting from a guess.
const CONTEXT: usize = 200;
/// Windowed mode: highlighted lines kept on each side of the viewport.
const KEEP: usize = 5_000;
/// Time slice of one step; between steps the worker picks up edits and viewport changes.
const SLICE: Duration = Duration::from_millis(3);

/// Which lines an edit touched: `0..first` are unchanged, and so are the last `suffix` lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Inval {
    pub first: usize,
    pub suffix: usize,
    pub old_lines: usize,
    pub new_lines: usize,
}

impl Inval {
    /// `self` followed by `next`.
    pub fn then(self, next: Inval) -> Inval {
        Inval {
            first: self.first.min(next.first),
            suffix: self.suffix.min(next.suffix),
            old_lines: self.old_lines,
            new_lines: next.new_lines,
        }
    }

    /// First old line of the unchanged suffix.
    pub fn old_suffix(&self) -> usize {
        self.old_lines - self.suffix
    }

    pub fn new_suffix(&self) -> usize {
        self.new_lines - self.suffix
    }

    /// Where an old line ends up, `None` if it was rewritten.
    pub fn map(&self, line: usize) -> Option<usize> {
        if line < self.first {
            Some(line)
        } else if line >= self.old_suffix() {
            Some(line - self.old_suffix() + self.new_suffix())
        } else {
            None
        }
    }

    /// The parts of an old line range that survive, in new coordinates (offset into the range, new range).
    pub fn map_range(&self, r: Range<usize>) -> impl Iterator<Item = (usize, Range<usize>)> {
        let head = r.start..r.end.min(self.first);
        let tail = r.start.max(self.old_suffix())..r.end;
        let shift = |x: usize| x - self.old_suffix() + self.new_suffix();
        let start = r.start;
        [
            (!head.is_empty()).then(|| (0, head.clone())),
            (!tail.is_empty()).then(|| (tail.start - start, shift(tail.start)..shift(tail.end))),
        ]
        .into_iter()
        .flatten()
    }
}

/// Results for the UI, in the coordinates of the latest text the worker has seen.
#[derive(Debug)]
pub(crate) enum Update {
    Lines { start: usize, spans: Vec<Box<[Span]>> },
    Clear(Range<usize>),
    Syntax(Option<String>),
}

pub(crate) fn map_updates(out: &mut Vec<Update>, inv: &Inval) {
    let old = std::mem::take(out);
    for u in old {
        match u {
            Update::Lines { start, mut spans } => {
                let parts: Vec<_> = inv.map_range(start..start + spans.len()).collect();
                for (offset, r) in parts.into_iter().rev() {
                    let piece = spans.split_off(offset);
                    out.push(Update::Lines { start: r.start, spans: piece.into_iter().take(r.len()).collect() });
                }
            }
            Update::Clear(r) => out.extend(inv.map_range(r).map(|(_, r)| Update::Clear(r))),
            s @ Update::Syntax(_) => out.push(s),
        }
    }
}

#[derive(Clone, Debug)]
struct State {
    parse: ParseState,
    hl: HighlightState,
}

impl State {
    fn same(&self, other: &State) -> bool {
        self.hl.path == other.hl.path && self.parse == other.parse
    }
}

struct Ckpt {
    line: usize,
    state: State,
    /// Only there because a step ended here; replaced when the run grows.
    temp: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Starts at line 0: states are exact.
    Exact,
    /// Starts from a guessed (fresh) state near the viewport.
    Anchored,
    /// Survived an edit; correct if the state at its start still matches. Never extended, only adopted.
    Tentative,
}

struct Run {
    start: usize,
    end: usize,
    kind: Kind,
    /// Sorted; the last one is at `end` (none for an empty run, which starts from a fresh state).
    ckpts: Vec<Ckpt>,
}

/// What a step should work on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pass {
    Visible,
    Background,
}

#[derive(Default)]
pub(crate) struct Work {
    rope: Rope,
    pub version: u64,
    syntax: Option<usize>,
    syntax_name: Option<String>,
    resolved: bool,
    hint: Option<LangHint>,
    /// Generation of the grammar set `syntax` indexes into.
    set_gen: u64,
    theme: Option<(Arc<Theme>, u64)>,
    runs: Vec<Run>,
    /// First visible line and row count; rows = 0 means not on screen.
    viewport: (usize, usize),
    pub out: Vec<Update>,
    buf: String,
}

/// Inputs from the UI thread (see `Inbox` in `engine`).
pub(crate) struct Input<'a> {
    pub text: Option<(Rope, u64)>,
    pub inval: Option<Inval>,
    pub hint: Option<LangHint>,
    pub viewport: (usize, usize),
    pub theme: (&'a Arc<Theme>, u64),
    pub set_gen: u64,
}

impl Work {
    pub fn ingest(&mut self, input: Input, ss: &SyntaxSet) {
        if let Some((rope, version)) = input.text {
            self.rope = rope;
            self.version = version;
        }
        if let Some(inv) = input.inval {
            self.apply_inval(&inv);
        }
        let new_set = self.set_gen != input.set_gen;
        if new_set {
            // parser states point into the old set's contexts
            self.set_gen = input.set_gen;
            self.runs.clear();
        }
        if input.hint.is_some() || (new_set && self.hint.is_some()) {
            if let Some(hint) = input.hint {
                self.hint = Some(hint);
            }
            let syntax = detect::resolve(ss, self.hint.as_ref().expect("just set"));
            let name = syntax.map(|i| ss.syntaxes()[i].name.clone());
            self.syntax = syntax;
            if !self.resolved || name != self.syntax_name {
                if self.resolved {
                    self.out.clear();
                    self.out.push(Update::Clear(0..self.rope.len_lines()));
                }
                self.resolved = true;
                self.runs.clear();
                self.out.push(Update::Syntax(name.clone()));
                self.syntax_name = name;
            }
        }
        let (theme, generation) = input.theme;
        if self.theme.as_ref().is_none_or(|(_, g)| *g != generation) {
            // highlight states embed styles: start over (old colors stay on screen until replaced)
            self.theme = Some((theme.clone(), generation));
            self.runs.clear();
        }
        self.viewport = input.viewport;
        if self.windowed() {
            self.evict();
        }
    }

    fn windowed(&self) -> bool {
        self.rope.len_lines() > WINDOWED_LINES || self.rope.len_bytes() > WINDOWED_BYTES
    }

    fn apply_inval(&mut self, inv: &Inval) {
        let first = inv.first;
        let old_suffix = inv.old_suffix();
        let mut runs = Vec::with_capacity(self.runs.len() + 1);
        for mut run in self.runs.drain(..) {
            if run.end <= first {
                runs.push(run);
                continue;
            }
            let mut tail = None;
            if run.end > old_suffix {
                let from = run.start.max(old_suffix);
                let keep = run.ckpts.iter().position(|c| c.line >= from).unwrap_or(run.ckpts.len());
                let ckpts: Vec<Ckpt> = run
                    .ckpts
                    .drain(keep..)
                    .map(|mut c| {
                        c.line = inv.map(c.line).expect("in the suffix");
                        c
                    })
                    .collect();
                let kind = if run.kind == Kind::Anchored && run.start >= old_suffix {
                    Kind::Anchored
                } else {
                    Kind::Tentative
                };
                tail = Some(Run {
                    start: inv.map(from).expect("in the suffix"),
                    end: inv.map(run.end).unwrap(),
                    kind,
                    ckpts,
                });
            }
            if run.start <= first {
                let keep = run.ckpts.iter().rposition(|c| c.line <= first);
                match keep {
                    Some(k) => {
                        run.ckpts.truncate(k + 1);
                        run.end = run.ckpts[k].line;
                        runs.push(run);
                    }
                    None if run.kind != Kind::Tentative => {
                        let kind = if run.start == 0 { Kind::Exact } else { Kind::Anchored };
                        runs.push(Run { start: run.start, end: run.start, kind, ckpts: Vec::new() });
                    }
                    None => {}
                }
            }
            runs.extend(tail);
        }
        self.runs = runs;
        map_updates(&mut self.out, inv);
    }

    /// Whether a run counts as done. Guesses (anchored runs) are good enough to show, and all a big file gets;
    /// otherwise the exact pass must reach everything.
    fn counts(kind: Kind, guesses_ok: bool) -> bool {
        if guesses_ok { kind != Kind::Tentative } else { kind == Kind::Exact }
    }

    /// First line of `range` not covered by a run that counts.
    fn first_gap(&self, range: Range<usize>, guesses_ok: bool) -> Option<usize> {
        let mut l = range.start;
        for r in self.runs.iter().filter(|r| Self::counts(r.kind, guesses_ok)) {
            if r.start > l {
                break;
            }
            l = l.max(r.end);
        }
        (l < range.end).then_some(l)
    }

    /// Lines worth highlighting first: the viewport plus a screen of look-ahead.
    fn visible(&self) -> Range<usize> {
        let (top, rows) = self.viewport;
        let n = self.rope.len_lines();
        if rows == 0 {
            return 0..0;
        }
        top.min(n)..(top + 2 * rows).min(n)
    }

    /// The line to highlight next and how far this pass should go.
    fn target(&self, pass: Pass) -> Option<(usize, usize)> {
        self.syntax?;
        match pass {
            Pass::Visible => {
                let vis = self.visible();
                self.first_gap(vis.clone(), true).map(|l| (l, vis.end))
            }
            Pass::Background if self.windowed() => None,
            Pass::Background => {
                let n = self.rope.len_lines();
                self.first_gap(0..n, false).map(|l| (l, n))
            }
        }
    }

    pub fn settled(&self) -> bool {
        self.target(Pass::Visible).is_none() && self.target(Pass::Background).is_none()
    }

    /// Lines the UI should repaint for: does `out` touch the screen?
    pub fn out_visible(&self) -> bool {
        let vis = self.visible();
        self.out.iter().any(|u| match u {
            Update::Lines { start, spans } => *start < vis.end && start + spans.len() > vis.start,
            Update::Clear(_) | Update::Syntax(_) => true,
        })
    }

    /// Does one time slice of work for `pass`; false if there was nothing to do.
    pub fn step(&mut self, pass: Pass, ss: &SyntaxSet) -> bool {
        let Some((gap, until)) = self.target(pass) else { return false };
        let guesses_ok = pass == Pass::Visible || self.windowed();
        // the run to extend: the closest one that counts before the gap
        let prev = self.runs.iter().rposition(|r| {
            r.end <= gap && if guesses_ok { r.kind != Kind::Tentative } else { r.kind == Kind::Exact && r.start == 0 }
        });
        let prev_end = prev.map_or(0, |i| self.runs[i].end);
        // lines after an edit still show their old colors: verify them in order (it usually converges within a few
        // lines). Text never highlighted far from anything: start from a guess near it rather than parse up to it.
        let tentative = self.runs.iter().any(|r| r.kind == Kind::Tentative && r.start <= gap && gap < r.end);
        let i = match prev {
            Some(i) if !guesses_ok || tentative || gap - prev_end <= FAR => i,
            None if !guesses_ok || tentative || gap <= FAR => self.new_run_at(0),
            _ => {
                let at = self.anchor_line(gap, prev_end);
                self.new_run_at(at)
            }
        };
        self.extend(i, until, ss);
        true
    }

    /// Where to start a guessed run for `gap`: a top-level-looking line (unindented, after a blank one) a little
    /// above it, so the fresh parser state is likely right.
    fn anchor_line(&self, gap: usize, floor: usize) -> usize {
        let lo = gap.saturating_sub(2 * CONTEXT).max(floor);
        let blank = |l: usize| self.rope.line(l).chars().all(char::is_whitespace);
        let top_level =
            |l: usize| self.rope.line(l).chars().next().is_some_and(|c| !c.is_whitespace() && c != '}' && c != ')');
        (lo.max(1)..=gap.saturating_sub(CONTEXT / 8))
            .rev()
            .find(|&l| top_level(l) && blank(l - 1))
            .unwrap_or_else(|| gap.saturating_sub(CONTEXT).max(floor))
    }

    /// Inserts an empty run (fresh parser state) at `start`, cutting whatever overlaps it.
    fn new_run_at(&mut self, start: usize) -> usize {
        let mut runs = Vec::with_capacity(self.runs.len() + 1);
        for mut run in self.runs.drain(..) {
            if run.start >= start || run.end <= start {
                if run.start != start {
                    runs.push(run);
                }
                continue;
            }
            if let Some(k) = run.ckpts.iter().rposition(|c| c.line <= start) {
                run.ckpts.truncate(k + 1);
                run.end = run.ckpts[k].line;
                runs.push(run);
            } else if run.kind != Kind::Tentative {
                run.ckpts.clear();
                run.end = run.start;
                runs.push(run);
            }
        }
        let kind = if start == 0 { Kind::Exact } else { Kind::Anchored };
        let i = runs.partition_point(|r| r.start < start);
        runs.insert(i, Run { start, end: start, kind, ckpts: Vec::new() });
        self.runs = runs;
        i
    }

    /// Windowed mode: forget highlighting far from the viewport.
    fn evict(&mut self) {
        let (top, rows) = self.viewport;
        if rows == 0 {
            return;
        }
        let lo = top.saturating_sub(KEEP);
        let hi = top + rows + KEEP;
        let mut out = Vec::new();
        self.runs.retain_mut(|run| {
            if run.end <= lo || run.start >= hi {
                out.push(run.start..run.end);
                return false;
            }
            if run.start < lo {
                run.ckpts.retain(|c| c.line >= lo);
                if run.ckpts.is_empty() {
                    out.push(run.start..run.end);
                    return false;
                }
                out.push(run.start..lo);
                run.start = lo;
            }
            if run.end > hi {
                if let Some(k) = run.ckpts.iter().rposition(|c| c.line <= hi && c.line >= run.start) {
                    out.push(run.ckpts[k].line..run.end);
                    run.ckpts.truncate(k + 1);
                    run.end = run.ckpts[k].line;
                }
            }
            true
        });
        self.out.extend(out.into_iter().filter(|r| !r.is_empty()).map(Update::Clear));
    }

    fn fresh_state(&self, ss: &SyntaxSet, hl: &SynHighlighter) -> State {
        let syntax = &ss.syntaxes()[self.syntax.expect("highlighting needs a syntax")];
        State { parse: ParseState::new(syntax), hl: HighlightState::new(hl, ScopeStack::new()) }
    }

    /// Highlights forward from the end of run `i` until `until`, the end of the text or the time slice runs out.
    fn extend(&mut self, i: usize, until: usize, ss: &SyntaxSet) {
        let (theme, _) = self.theme.clone().expect("theme is set on ingest");
        let hl = SynHighlighter::new(theme.syntect());
        let default = hl.get_default();
        let bg = theme.palette.background;
        let mut state = match self.runs[i].ckpts.last() {
            Some(c) if c.temp => self.runs[i].ckpts.pop().unwrap().state,
            Some(c) => c.state.clone(),
            None => self.fresh_state(ss, &hl),
        };
        let n = self.rope.len_lines();
        let rope = self.rope.clone();
        let mut l = self.runs[i].end;
        let mut batch_start = l;
        let mut batch: Vec<Box<[Span]>> = Vec::new();
        let mut lines = rope.lines_at(l);
        let mut last_ckpt = self.runs[i].ckpts.last().map_or(self.runs[i].start, |c| c.line);
        let deadline = Instant::now() + SLICE;
        while l < n && l < until {
            if l % 16 == 0 && Instant::now() >= deadline {
                break;
            }
            if let Some(next) = self.runs.get_mut(i + 1).filter(|r| r.start <= l) {
                if let Some(pos) =
                    next.ckpts.iter().position(|c| c.line == l).filter(|&p| next.ckpts[p].state.same(&state))
                {
                    // converged: the rest of `next` is exactly what this pass would produce
                    let next = self.runs.remove(i + 1);
                    let run = &mut self.runs[i];
                    let Some(end) = next.ckpts.last().filter(|c| c.line == next.end) else {
                        debug_assert!(false, "runs end with a checkpoint");
                        continue;
                    };
                    state = end.state.clone();
                    run.ckpts.extend(next.ckpts.into_iter().skip(pos + 1));
                    run.end = next.end;
                    last_ckpt = run.end;
                    flush(&mut self.out, &mut batch, batch_start);
                    l = run.end;
                    batch_start = l;
                    lines = rope.lines_at(l);
                    continue;
                }
                next.ckpts.retain(|c| c.line > l);
                next.start = l + 1;
                if next.start >= next.end || next.ckpts.is_empty() {
                    self.runs.remove(i + 1);
                }
            }
            let line = lines.next().expect("l < len_lines");
            batch.push(highlight_line(line, &mut state, ss, &hl, default.foreground, bg, &mut self.buf));
            l += 1;
            self.runs[i].end = l;
            if l - last_ckpt >= CHECKPOINT_EVERY {
                self.runs[i].ckpts.push(Ckpt { line: l, state: state.clone(), temp: false });
                last_ckpt = l;
            }
        }
        flush(&mut self.out, &mut batch, batch_start);
        let run = &mut self.runs[i];
        if run.ckpts.last().is_none_or(|c| c.line != run.end) {
            run.ckpts.push(Ckpt { line: run.end, state, temp: true });
        }
    }
}

fn flush(out: &mut Vec<Update>, batch: &mut Vec<Box<[Span]>>, start: usize) {
    if !batch.is_empty() {
        out.push(Update::Lines { start, spans: std::mem::take(batch) });
    }
}

/// Parses one line (with its `\n`) and turns the styles into spans, skipping default-colored text.
fn highlight_line(
    line: ropey::RopeSlice,
    state: &mut State,
    ss: &SyntaxSet,
    hl: &SynHighlighter,
    default_fg: syntect::highlighting::Color,
    bg: Rgb,
    buf: &mut String,
) -> Box<[Span]> {
    if line.len_chars() > MAX_LINE_CHARS {
        return Box::default();
    }
    let text: &str = match line.as_str() {
        Some(s) => s,
        None => {
            buf.clear();
            buf.extend(line.chunks());
            buf
        }
    };
    let content = text.trim_end_matches(['\n', '\r']).len();
    let Ok(ops) = state.parse.parse_line(text, ss) else { return Box::default() };
    let mut spans: Vec<Span> = Vec::new();
    for (style, _, r) in RangedHighlightIterator::new(&mut state.hl, &ops, text, hl) {
        let end = r.end.min(content);
        if r.start >= end || (style.foreground == default_fg && style.font_style.is_empty()) {
            continue;
        }
        let fs = style.font_style;
        let span = Span {
            start: r.start as u32,
            end: end as u32,
            fg: Rgb::from_syntect(style.foreground, bg),
            bold: fs.contains(FontStyle::BOLD),
            italic: fs.contains(FontStyle::ITALIC),
            underline: fs.contains(FontStyle::UNDERLINE),
        };
        match spans.last_mut() {
            Some(last)
                if last.end == span.start
                    && (last.fg, last.bold, last.italic, last.underline)
                        == (span.fg, span.bold, span.italic, span.underline) =>
            {
                last.end = span.end
            }
            _ => spans.push(span),
        }
    }
    spans.into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inval_mapping() {
        // lines 3..5 of 10 replaced by 4 lines -> 11 lines, last 5 unchanged
        let inv = Inval { first: 3, suffix: 5, old_lines: 10, new_lines: 11 };
        assert_eq!(inv.map(2), Some(2));
        assert_eq!(inv.map(3), None);
        assert_eq!(inv.map(4), None);
        assert_eq!(inv.map(5), Some(6));
        assert_eq!(inv.map(9), Some(10));
        let parts: Vec<_> = inv.map_range(0..10).collect();
        assert_eq!(parts, vec![(0, 0..3), (5, 6..11)]);
        let then = inv.then(Inval { first: 1, suffix: 8, old_lines: 11, new_lines: 11 });
        assert_eq!(then, Inval { first: 1, suffix: 5, old_lines: 10, new_lines: 11 });
    }

    #[test]
    fn map_updates_splits() {
        let inv = Inval { first: 3, suffix: 5, old_lines: 10, new_lines: 11 };
        let sp = |n: u32| -> Box<[Span]> {
            vec![Span { start: 0, end: n, fg: Rgb::default(), bold: false, italic: false, underline: false }].into()
        };
        let mut out = vec![Update::Lines { start: 1, spans: (1..8).map(sp).collect() }, Update::Clear(4..6)];
        map_updates(&mut out, &inv);
        let got: Vec<String> = out
            .iter()
            .map(|u| match u {
                Update::Lines { start, spans } => {
                    format!("L{start}:{:?}", spans.iter().map(|s| s[0].end).collect::<Vec<_>>())
                }
                Update::Clear(r) => format!("C{r:?}"),
                Update::Syntax(_) => "S".into(),
            })
            .collect();
        // old lines 1..8 hold spans 1..=7: lines 1,2 keep; old 5,6,7 -> new 6,7,8
        assert_eq!(got, vec!["L6:[5, 6, 7]", "L1:[1, 2]", "C6..7"]);
    }
}
