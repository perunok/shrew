//! The minimap: a Sublime-style overview column at the right edge of the editor (the `minimap` feature).
//!
//! It draws no glyphs. Each line becomes thin blocks, one per run of non-blank text of one color (the syntax colors
//! once the highlighter has the line, the text color before), [`LINE_PX`] logical pixels per line and
//! [`COL_PX`] per column, cut at the minimap's width. On top go the current line, cursors, search matches and a
//! translucent box over the lines the editor shows. A file longer than the minimap scrolls through it in
//! proportion to the editor, as in Sublime, so the box travels from the top to the bottom exactly once.
//!
//! Cost: only the lines on show (plus a margin, so scrolling rarely rebuilds) are read, never the whole file. The
//! blocks are cached per (buffer, version, syntax generation, colors, search, line window), so a frame that changes
//! none of them only copies a few thousand rects into the scene. Off, it is a zero-width rect and an empty cache.
//!
//! Mouse ([`press`], [`drag`], [`hover`]): a click centres the editor on the line under the pointer, dragging (the
//! box or anywhere) scrolls continuously with the box following the pointer, and the box brightens under the
//! pointer. The wheel over it scrolls the editor like anywhere else. It never takes the keyboard focus.

use crate::colors::{Colors, mix};
use crate::draw::Ui;
use crate::layout::{Layout, Scroll};
use crate::state::State;
use shrew_core::features::Feature;
use shrew_core::ropey::{Rope, RopeSlice};
use shrew_core::{search, text};
use shrew_render::{Color, Rect, Scene, char_columns};
use std::ops::Range;
use std::time::{Duration, Instant};

/// The text keeps at least this many columns: a narrower window hides the minimap.
pub const MIN_TEXT_COLS: usize = 50;
/// Height of a line and width of a column, in logical pixels.
const LINE_PX: f32 = 2.0;
const COL_PX: f32 = 1.0;
/// Room between the text and the first column, in logical pixels.
const INSET: f32 = 4.0;
/// How strongly text colors show over the background (Sublime's minimap is a little faded too).
const INK: f32 = 0.75;
/// Lines cached beyond each end of the shown ones, as a fraction (1/n) of the shown lines.
const MARGIN: usize = 8;
/// Search marks per rebuild, at most (a search for `e` in a dense file).
const MAX_MARKS: usize = 4096;
/// Alpha of the viewport box, normally and under the pointer.
const BOX_ALPHA: u8 = 0x1c;
const BOX_ALPHA_HOVER: u8 = 0x30;

/// Logical width the minimap asks [`Layout::with_minimap`] for: the configured width, or 0 when the feature is off
/// or the buffer is empty (nothing to map, and the welcome screen stays centred).
pub fn width(st: &State) -> f32 {
    let on = st.ed.features.on(Feature::Minimap) && st.ed.buf().doc.rope.len_chars() > 0;
    if on { st.minimap_width } else { 0.0 }
}

/// Where lines go in the minimap. Drawing and hit testing both build one, from the drawn scroll position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub rect: Rect,
    /// Physical pixels per line (whole) and per column.
    pub line_px: f32,
    pub col_px: f32,
    /// Pixels left of column 0.
    pub inset: f32,
    /// Lines in the buffer, text rows on screen.
    pub total: usize,
    pub rows: usize,
    /// How far the content is scrolled up, in whole pixels.
    pub offset: f32,
}

impl Geometry {
    /// `top`: the editor's (drawn, possibly fractional) first line.
    pub fn new(rect: Rect, scale: f32, total: usize, rows: usize, top: f32) -> Geometry {
        let line_px = (LINE_PX * scale).round().max(1.0);
        let col_px = (COL_PX * scale).max(0.5);
        let inset = (INSET * scale).round();
        let mut g = Geometry { rect, line_px, col_px, inset, total, rows, offset: 0.0 };
        let spare = (total as f32 - g.capacity()).max(0.0);
        g.offset = ((g.ratio() * top.max(0.0)).min(spare) * line_px).round();
        g
    }

    /// Lines that fit.
    pub fn capacity(&self) -> f32 {
        self.rect.h / self.line_px
    }

    /// Minimap lines scrolled per editor line: 0 while the whole file fits, else such that the viewport box reaches
    /// the bottom just as the editor shows the last line.
    fn ratio(&self) -> f32 {
        let spare = self.total as f32 - self.capacity();
        if spare <= 0.0 {
            return 0.0;
        }
        (spare / self.total.saturating_sub(self.rows).max(1) as f32).min(1.0)
    }

    /// Top of `line` (may be fractional) on screen.
    pub fn line_y(&self, line: f32) -> f32 {
        self.rect.y + line * self.line_px - self.offset
    }

    /// The (fractional) line under `y`.
    pub fn line_at(&self, y: f32) -> f32 {
        (y - self.rect.y + self.offset) / self.line_px
    }

    /// The lines with at least a pixel on screen, `[first, end)`.
    pub fn lines(&self) -> Range<usize> {
        let first = (self.offset / self.line_px).floor() as usize;
        let end = ((self.offset + self.rect.h) / self.line_px).ceil() as usize;
        first.min(self.total)..end.min(self.total)
    }

    /// The box over what the editor shows when its first line is `top` (no lower than the last line).
    pub fn viewport(&self, top: f32) -> Rect {
        let y = self.line_y(top).round();
        let lines = (self.rows as f32).min(self.total as f32 - top).max(1.0);
        Rect::new(self.rect.x, y, self.rect.w, (lines * self.line_px).round())
    }

    /// The furthest a click or drag scrolls: the last line at the bottom of the screen.
    fn max_top(&self) -> f32 {
        self.total.saturating_sub(self.rows) as f32
    }

    /// Editor lines one pixel of dragging scrolls, so that the box follows the pointer.
    pub fn lines_per_px(&self) -> f32 {
        1.0 / ((1.0 - self.ratio()) * self.line_px).max(1e-3)
    }

    /// Columns that fit.
    pub fn cols(&self) -> usize {
        ((self.rect.w - self.inset).max(0.0) / self.col_px).floor() as usize
    }

    /// The pixel span of columns `[col, col + len)`, whole pixels, at least one wide.
    fn x_span(&self, col: usize, len: usize) -> (f32, f32) {
        let x = self.rect.x + self.inset;
        let a = (x + col as f32 * self.col_px).round();
        let z = (x + (col + len) as f32 * self.col_px).round();
        (a, (z - a).max(1.0))
    }
}

/// A run of same-colored text on one line, in display columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub line: u32,
    pub col: u16,
    pub len: u16,
    pub color: Color,
}

/// What the cached blocks were built from: when any of it changes they are rebuilt.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    buffer: u64,
    version: u64,
    tab_width: usize,
    cols: usize,
    colors: [Color; 3],
    /// The search pattern whose matches are marked.
    search: Option<String>,
}

struct Cache {
    key: Key,
    /// The highlighter's span generation the blocks match (none without highlighting): when it moves on, only the
    /// lines it changed are redone.
    syntax: Option<u64>,
    lines: Range<usize>,
    blocks: Vec<Block>,
    marks: Vec<Block>,
}

/// The minimap's state between frames (lives in [`Ui`]).
#[derive(Default)]
pub struct Minimap {
    cache: Option<Cache>,
    /// The pointer is over the minimap.
    pub hover: bool,
    /// A drag in progress: the pointer's y and the editor's first line when it started.
    grab: Option<(f32, f32)>,
    /// Spans of one line in minimap colors (scratch).
    spans: Vec<(u32, u32, Color)>,
    /// Rebuilds so far and how long the last one took (`--stats`, tests).
    pub builds: u64,
    pub last_build: Duration,
}

/// Draws the minimap into `lay.minimap` (nothing when it is empty).
pub fn draw(scene: &mut Scene, st: &State, ui: &mut Ui, lay: &Layout, c: &Colors, scroll: Scroll) {
    if lay.minimap.is_empty() {
        ui.minimap.cache = None;
        ui.minimap.hover = false;
        return;
    }
    let ed = &st.ed;
    let b = ed.buf();
    let rope = &b.doc.rope;
    let tw = ed.settings.tab_width.max(1);
    let g = Geometry::new(lay.minimap, lay.scale, text::last_line(rope) + 1, lay.rows, scroll.top);
    let shown = g.lines();
    let search = ed
        .search
        .as_ref()
        .filter(|_| ed.search_visible && ed.features.on(Feature::SearchHighlight))
        .map(|re| re.as_str().to_string());
    #[cfg(feature = "syntax")]
    let syntax = st.highlighters.get(&b.id).filter(|_| ed.features.on(Feature::Syntax)).map(|h| h.generation());
    #[cfg(not(feature = "syntax"))]
    let syntax = None;
    let key = Key {
        buffer: b.id,
        version: b.doc.version,
        tab_width: tw,
        cols: g.cols().min(u16::MAX as usize),
        colors: [c.foreground, c.background, c.warning],
        search,
    };
    let mm = &mut ui.minimap;
    mm.update(st, key, syntax, shown.clone(), g.total, c);
    let Some(cache) = &mm.cache else { return };

    scene.push_clip(g.rect);
    // a hairline between lines where there are pixels to spare (scale 2: 3 px of 4)
    let block_h = (g.line_px * 0.75).round().max(1.0);
    // the current line and other cursor lines, behind the text
    let head = b.sel.primary().head.min(rope.len_chars());
    let cur_line = text::line_of(rope, head);
    if shown.contains(&cur_line) {
        scene.rect(Rect::new(g.rect.x, g.line_y(cur_line as f32), g.rect.w, g.line_px), c.current_line);
    }
    // selected lines, as whole rows
    let (a, z) = (text::line_start(rope, shown.start), text::line_start(rope, shown.end));
    let ranges = b.sel.ranges();
    let start = ranges.partition_point(|r| r.to() < a);
    let visual = ed.mode.is_visual();
    for r in ranges[start..].iter().take_while(|r| r.from() <= z).take(1024) {
        if r.is_empty() && !visual {
            continue;
        }
        let la = text::line_of(rope, r.from()).max(shown.start);
        let lz = text::line_of(rope, r.to().saturating_sub(usize::from(!visual && r.to() > r.from())))
            .min(shown.end.saturating_sub(1));
        if lz >= la {
            let y = g.line_y(la as f32);
            scene.rect(Rect::new(g.rect.x, y, g.rect.w, g.line_y(lz as f32 + 1.0) - y), c.selection);
        }
    }
    let blocks = &cache.blocks;
    let from = blocks.partition_point(|k| (k.line as usize) < shown.start);
    for k in blocks[from..].iter().take_while(|k| (k.line as usize) < shown.end) {
        let (x, w) = g.x_span(k.col as usize, k.len as usize);
        scene.rect(Rect::new(x, g.line_y(k.line as f32), w, block_h), k.color);
    }
    let from = cache.marks.partition_point(|k| (k.line as usize) < shown.start);
    for k in cache.marks[from..].iter().take_while(|k| (k.line as usize) < shown.end) {
        let (x, w) = g.x_span(k.col as usize, k.len as usize);
        scene.rect(Rect::new(x, g.line_y(k.line as f32), w, block_h), k.color);
    }
    // cursors: a short tick where each one is
    let tick = (2.0 * lay.scale).round().max(1.0);
    for r in ranges[start..].iter().take_while(|r| r.from() <= z).take(1024) {
        let h = r.head.min(rope.len_chars());
        let l = text::line_of(rope, h);
        if !shown.contains(&l) {
            continue;
        }
        let col = text::col_of(rope, h, tw).min(g.cols().saturating_sub(1));
        let (x, _) = g.x_span(col, 1);
        scene.rect(Rect::new(x, g.line_y(l as f32), tick, g.line_px), c.cursor);
    }
    // ----- git: change marks at the left edge -----
    #[cfg(feature = "git")]
    crate::git::gutter::draw_minimap(scene, st, &g, lay, c);
    // what the editor shows
    let alpha = if mm.hover { BOX_ALPHA_HOVER } else { BOX_ALPHA };
    scene.rect(g.viewport(scroll.top), c.foreground.with_alpha(alpha));
    scene.pop_clip();
}

impl Minimap {
    /// Brings the cache up to date for showing `shown`, doing as little as it can: nothing when nothing changed, the
    /// lines new highlighting arrived for, the lines a scroll brought in, or everything after an edit.
    fn update(&mut self, st: &State, key: Key, syntax: Option<u64>, shown: Range<usize>, total: usize, c: &Colors) {
        let t = Instant::now();
        let margin = shown.len() / MARGIN + 1;
        let want = shown.start.saturating_sub(margin)..(shown.end + margin).min(total);
        let ink = Ink { fg: mix(c.foreground, c.background, INK), bg: c.background };
        let mut cache = match self.cache.take() {
            Some(cache) if cache.key == key => cache,
            old => {
                let mut cache = old.unwrap_or_else(|| Cache {
                    key: key.clone(),
                    syntax,
                    lines: 0..0,
                    blocks: Vec::new(),
                    marks: Vec::new(),
                });
                cache.key = key;
                cache.syntax = syntax;
                cache.lines = 0..0;
                cache.blocks.clear();
                cache
            }
        };
        let mut work = false;
        // new highlighting: redo just the lines it touched
        if cache.syntax != syntax {
            let changed = match (cache.syntax, syntax) {
                (Some(old), Some(_)) => changed_since(st, old),
                _ => None,
            };
            let r = changed.unwrap_or(0..usize::MAX);
            let r = r.start.max(cache.lines.start)..r.end.min(cache.lines.end);
            if !r.is_empty() {
                let at = cache.blocks.partition_point(|k| (k.line as usize) < r.start);
                let to = cache.blocks.partition_point(|k| (k.line as usize) < r.end);
                let mut fresh = Vec::new();
                build_lines(st, &cache.key, &ink, r, &mut self.spans, &mut fresh);
                cache.blocks.splice(at..to, fresh);
                work = true;
            }
            cache.syntax = syntax;
        }
        // scrolled out of the cached window (or nothing built yet): keep what still shows, build the rest
        let have = cache.lines.clone();
        if have.is_empty() || shown.start < have.start || shown.end > have.end {
            let keep = want.start.max(have.start)..want.end.min(have.end);
            if keep.is_empty() {
                cache.blocks.clear();
                build_lines(st, &cache.key, &ink, want.clone(), &mut self.spans, &mut cache.blocks);
            } else {
                let to = cache.blocks.partition_point(|k| (k.line as usize) < keep.end);
                cache.blocks.truncate(to);
                let at = cache.blocks.partition_point(|k| (k.line as usize) < keep.start);
                cache.blocks.drain(..at);
                let mut above = Vec::new();
                build_lines(st, &cache.key, &ink, want.start..keep.start, &mut self.spans, &mut above);
                cache.blocks.splice(0..0, above);
                build_lines(st, &cache.key, &ink, keep.end..want.end, &mut self.spans, &mut cache.blocks);
            }
            cache.lines = want;
            work = true;
        }
        // search matches cost little next to the blocks: redone whenever anything was
        if work {
            cache.marks.clear();
            let ed = &st.ed;
            if cache.key.search.is_some()
                && let Some(re) = &ed.search
                && !cache.lines.is_empty()
            {
                let rope = ed.buf().rope();
                let found = search::matches_in(rope, re, cache.lines.start, cache.lines.end - 1);
                let color = mix(c.warning, c.background, 0.85);
                let (tw, cols) = (cache.key.tab_width, cache.key.cols);
                marks(rope, &found, cache.lines.clone(), tw, cols, color, &mut cache.marks);
            }
            self.builds += 1;
            self.last_build = t.elapsed();
        }
        self.cache = Some(cache);
    }
}

/// Text colors as the minimap draws them.
struct Ink {
    fg: Color,
    #[cfg_attr(not(feature = "syntax"), allow(dead_code))]
    bg: Color,
}

/// Appends the blocks of `lines` (of the active buffer) to `out`.
fn build_lines(
    st: &State,
    key: &Key,
    ink: &Ink,
    lines: Range<usize>,
    spans: &mut Vec<(u32, u32, Color)>,
    out: &mut Vec<Block>,
) {
    if lines.is_empty() {
        return;
    }
    let b = st.ed.buf();
    for (i, line) in b.rope().lines_at(lines.start).take(lines.len()).enumerate() {
        let l = lines.start + i;
        spans.clear();
        #[cfg(feature = "syntax")]
        if st.ed.features.on(Feature::Syntax)
            && let Some(ss) = st.spans(b.id, l)
        {
            let bg = ink.bg;
            spans.extend(ss.iter().map(|s| (s.start, s.end, mix(Color::rgb(s.fg.r, s.fg.g, s.fg.b), bg, INK))));
        }
        line_blocks(line, spans, key.tab_width, key.cols, ink.fg, &mut |col, len, color| {
            out.push(Block { line: l as u32, col: col as u16, len: len as u16, color });
        });
    }
}

/// The lines whose highlighting changed after `generation`, if the highlighter still knows.
#[cfg(feature = "syntax")]
fn changed_since(st: &State, generation: u64) -> Option<Range<usize>> {
    st.highlighters.get(&st.ed.buf().id)?.changed_since(generation)
}

#[cfg(not(feature = "syntax"))]
fn changed_since(_: &State, _: u64) -> Option<Range<usize>> {
    None
}

/// Splits one line into blocks: runs of non-blank chars of one color, in display columns (tabs expanded), cut at
/// `cols`. `spans` are (byte start, byte end, color), sorted and disjoint; text outside them is `fg`. Calls
/// `out(col, len, color)` for each block, left to right.
pub fn line_blocks(
    line: RopeSlice,
    spans: &[(u32, u32, Color)],
    tw: usize,
    cols: usize,
    fg: Color,
    out: &mut dyn FnMut(usize, usize, Color),
) {
    let (mut col, mut base, mut si) = (0usize, 0usize, 0usize);
    let mut run: Option<(usize, Color)> = None;
    let mut flush = |run: &mut Option<(usize, Color)>, end: usize| {
        if let Some((a, color)) = run.take() {
            let end = end.min(cols);
            if end > a {
                out(a, end - a, color);
            }
        }
    };
    // bytes of the rope's chunks, decoding only non-ASCII: several times faster than `RopeSlice::chars`
    'chunks: for chunk in line.chunks() {
        let bytes = chunk.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if col >= cols {
                break 'chunks;
            }
            let b = bytes[i];
            let (blank, w, len) = if b < 0x80 {
                if b == b'\n' || b == b'\r' {
                    break 'chunks;
                }
                (b.is_ascii_whitespace(), if b == b'\t' { tw - col % tw } else { 1 }, 1)
            } else {
                let ch = chunk[i..].chars().next().unwrap_or(' ');
                (ch.is_whitespace(), char_columns(ch), ch.len_utf8())
            };
            if blank {
                flush(&mut run, col);
            } else {
                let at = base + i;
                while si < spans.len() && spans[si].1 as usize <= at {
                    si += 1;
                }
                let color = match spans.get(si) {
                    Some(s) if s.0 as usize <= at => s.2,
                    _ => fg,
                };
                if run.is_none_or(|(_, c)| c != color) {
                    flush(&mut run, col);
                    run = Some((col, color));
                }
            }
            col += w;
            i += len;
        }
        base += chunk.len();
    }
    flush(&mut run, col);
}

/// Search matches (char ranges) on `lines` as blocks of `color`, at least a column wide.
fn marks(
    rope: &Rope,
    found: &[(usize, usize)],
    lines: Range<usize>,
    tw: usize,
    cols: usize,
    color: Color,
    out: &mut Vec<Block>,
) {
    let last = cols.saturating_sub(1);
    for &(a, z) in found.iter().take(MAX_MARKS) {
        let l = text::line_of(rope, a);
        if !lines.contains(&l) {
            continue;
        }
        let end = z.max(a + 1).min(text::line_end(rope, l).max(a + 1));
        let ca = text::col_of(rope, a, tw).min(last);
        let cz = text::col_of(rope, end.min(rope.len_chars()), tw).clamp(ca + 1, cols.max(ca + 1));
        out.push(Block { line: l as u32, col: ca as u16, len: (cz - ca) as u16, color });
    }
}

// ----- mouse -----

/// The geometry the last frame was drawn with (none when the minimap is hidden).
fn geometry(st: &State, ui: &Ui, lay: &Layout) -> Option<Geometry> {
    if lay.minimap.is_empty() {
        return None;
    }
    let total = text::last_line(st.ed.buf().rope()) + 1;
    Some(Geometry::new(lay.minimap, lay.scale, total, lay.rows, ui.anim.scroll.top))
}

/// The editor's first line after a click at `y`: unchanged on the viewport box (a drag is starting), else the line
/// under the pointer centred.
pub fn click_target(g: &Geometry, drawn_top: f32, view_top: f32, y: f32) -> f32 {
    if g.viewport(drawn_top).contains(g.rect.x, y) {
        return view_top;
    }
    (g.line_at(y).floor() - (g.rows / 2) as f32).min(g.max_top()).max(0.0)
}

/// The editor's first line while dragging from `grab` (pointer y, first line at the start) to `y`.
pub fn drag_target(g: &Geometry, grab: (f32, f32), y: f32) -> f32 {
    (grab.1 + (y - grab.0) * g.lines_per_px()).min(g.max_top().max(grab.1)).max(0.0)
}

/// A left press at (`x`, `y`). True when it hit the minimap: the app then sends pointer moves to [`drag`].
pub fn press(st: &mut State, ui: &mut Ui, lay: &Layout, x: f32, y: f32) -> bool {
    let Some(g) = geometry(st, ui, lay).filter(|g| g.rect.contains(x, y)) else { return false };
    let view_top = st.ed.buf().view.top as f32;
    scroll_to(st, ui, click_target(&g, ui.anim.scroll.top, view_top, y));
    ui.minimap.grab = Some((y, st.ed.buf().view.top as f32));
    true
}

/// The pointer moved to `y` while dragging. True when the editor scrolled.
pub fn drag(st: &mut State, ui: &mut Ui, lay: &Layout, y: f32) -> bool {
    let (Some(g), Some(grab)) = (geometry(st, ui, lay), ui.minimap.grab) else { return false };
    scroll_to(st, ui, drag_target(&g, grab, y))
}

/// The pointer is at `at` (none: it left the window). True when the hover look changed (redraw).
pub fn hover(ui: &mut Ui, lay: &Layout, at: Option<(f32, f32)>) -> bool {
    let over = at.is_some_and(|(x, y)| lay.minimap.contains(x, y));
    std::mem::replace(&mut ui.minimap.hover, over) != over
}

fn scroll_to(st: &mut State, ui: &mut Ui, top: f32) -> bool {
    let cur = st.ed.buf().view.top as isize;
    let to = top.round() as isize;
    ui.frac = 0.0;
    if to == cur {
        return false;
    }
    st.ed.scroll(to - cur, 0);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const FG: Color = Color::rgb(200, 200, 200);
    const KW: Color = Color::rgb(255, 0, 0);

    fn blocks(text: &str, spans: &[(u32, u32, Color)], cols: usize) -> Vec<(usize, usize, Color)> {
        let rope = Rope::from_str(text);
        let mut out = Vec::new();
        line_blocks(rope.line(0), spans, 4, cols, FG, &mut |a, n, c| out.push((a, n, c)));
        out
    }

    #[test]
    fn runs_split_at_blanks_and_colors() {
        // "fn main() {}" with `fn` as a keyword
        let got = blocks("fn main() {}\n", &[(0, 2, KW)], 100);
        assert_eq!(got, vec![(0, 2, KW), (3, 6, FG), (10, 2, FG)]);
        // tabs expand; leading and trailing blanks make no blocks
        let got = blocks("\tx = 1  \n", &[], 100);
        assert_eq!(got, vec![(4, 1, FG), (6, 1, FG), (8, 1, FG)]);
        assert!(blocks("   \n", &[], 100).is_empty());
        assert!(blocks("", &[], 100).is_empty());
        // adjacent spans of one color merge, a color change splits
        let got = blocks("abcdef", &[(0, 2, KW), (2, 4, KW), (4, 6, FG)], 100);
        assert_eq!(got, vec![(0, 4, KW), (4, 2, FG)]);
    }

    #[test]
    fn long_lines_are_cut_and_wide_chars_count_double() {
        let long = "x".repeat(10_000);
        assert_eq!(blocks(&long, &[], 80), vec![(0, 80, FG)]);
        assert_eq!(blocks("a 日本 b", &[], 100), vec![(0, 1, FG), (2, 4, FG), (7, 1, FG)]);
        // spans index bytes: the keyword after a 3-byte char
        assert_eq!(blocks("日 if", &[(4, 6, KW)], 100), vec![(0, 2, FG), (3, 2, KW)]);
        // a block running into the edge is cut there
        assert_eq!(blocks("ab cdef", &[], 5), vec![(0, 2, FG), (3, 2, FG)]);
    }

    fn layout(st: &State, size: (u32, u32)) -> Layout {
        let m = shrew_render::Metrics {
            font_size: 15.0,
            cell_width: 9.0,
            cell_height: 21.0,
            baseline: 16.0,
            ascent: 14.0,
            descent: 4.0,
            underline_offset: 18.0,
            underline_thickness: 1.0,
        };
        Layout::new(&m, size, 1.0, st.ed.buf().rope().len_lines(), true, false).with_minimap(width(st))
    }

    #[test]
    fn unchanged_frames_reuse_the_blocks() {
        let mut st = State::new(shrew_config::Loaded::default(), std::sync::Arc::new(|| {}), true);
        let text: String = (0..3000).map(|i| format!("    let x{i} = foo(bar, {i}); // note\n")).collect();
        st.ed.install_document(shrew_core::Document::from_str(&text));
        st.process_events();
        let mut ui = Ui::new();
        ui.animate = false;
        let mut scene = Scene::new();
        let lay = layout(&st, (1280, 800));
        assert!(!lay.minimap.is_empty());
        assert_eq!(lay.minimap.right(), lay.scrollbar.x);
        assert_eq!(lay.text.right(), lay.minimap.x);
        let mut frame = |st: &mut State, ui: &mut Ui| crate::draw::build(&mut scene, st, ui, &lay, Instant::now());
        frame(&mut st, &mut ui);
        let builds = ui.minimap.builds;
        assert!(builds >= 1);
        let cache = ui.minimap.cache.as_ref().unwrap();
        assert!(cache.blocks.len() > 1000, "{}", cache.blocks.len());
        for _ in 0..5 {
            frame(&mut st, &mut ui);
        }
        assert_eq!(ui.minimap.builds, builds, "nothing changed: nothing rebuilt");
        // scrolling a little stays in the cached window; an edit rebuilds
        st.ed.scroll(3, 0);
        frame(&mut st, &mut ui);
        assert_eq!(ui.minimap.builds, builds);
        st.key(shrew_core::KeyPress::parse("x").unwrap());
        frame(&mut st, &mut ui);
        assert_eq!(ui.minimap.builds, builds + 1);
        // scrolling past the cached window builds only the lines it brings in, ending where a full build does
        st.ed.scroll(200, 0);
        frame(&mut st, &mut ui);
        assert_eq!(ui.minimap.builds, builds + 2);
        let shifted = ui.minimap.cache.as_ref().map(|c| (c.lines.clone(), c.blocks.clone())).unwrap();
        ui.minimap.cache = None;
        frame(&mut st, &mut ui);
        let full = ui.minimap.cache.as_ref().map(|c| (c.lines.clone(), c.blocks.clone())).unwrap();
        assert!(shifted.0.start > 0);
        assert_eq!(shifted, full);
        // off: hidden and the cache dropped
        st.key(shrew_core::KeyPress::parse(":").unwrap());
        for k in shrew_core::KeyPress::parse_seq("set nominimap<CR>") {
            st.key(k);
        }
        let lay = layout(&st, (1280, 800));
        assert!(lay.minimap.is_empty());
        crate::draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
        assert!(ui.minimap.cache.is_none());
        // a narrow window has no room for it
        st.ed.set_feature(Feature::Minimap, true);
        assert!(layout(&st, (500, 800)).minimap.is_empty());
    }

    #[test]
    fn mouse_scrolls_the_editor() {
        let mut st = State::new(shrew_config::Loaded::default(), std::sync::Arc::new(|| {}), true);
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        st.ed.install_document(shrew_core::Document::from_str(&text));
        st.process_events();
        let mut ui = Ui::new();
        ui.animate = false;
        let mut scene = Scene::new();
        let lay = layout(&st, (1280, 800));
        crate::draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
        let g = geometry(&st, &ui, &lay).unwrap();
        let rows = lay.rows as f32;
        let x = lay.minimap.x + 10.0;
        assert!(!press(&mut st, &mut ui, &lay, lay.text.x + 50.0, 100.0), "not on the minimap");
        // a click on line 80 centres it
        assert!(press(&mut st, &mut ui, &lay, x, g.line_y(80.0) + 1.0));
        let top = st.ed.buf().view.top as f32;
        assert_eq!(top, (80.0 - (lay.rows / 2) as f32).min(100.0 - rows).max(0.0));
        // dragging back up 20 px scrolls 10 lines (2 px a line, the whole file fits)
        let y0 = g.line_y(80.0) + 1.0;
        assert!(drag(&mut st, &mut ui, &lay, y0 - 20.0));
        assert_eq!(st.ed.buf().view.top as f32, (top - 10.0).max(0.0));
        assert!(hover(&mut ui, &lay, Some((x, 50.0))));
        assert!(!hover(&mut ui, &lay, Some((x, 51.0))), "no change, no redraw");
        assert!(hover(&mut ui, &lay, None));
    }

    #[cfg(feature = "syntax")]
    #[test]
    fn highlighting_arrives_line_by_line() {
        let mut st = State::new(shrew_config::Loaded::default(), std::sync::Arc::new(|| {}), true);
        st.ed.run(shrew_core::Command::ToggleSyntax); // headless starts without it: off, then on
        st.ed.run(shrew_core::Command::ToggleSyntax);
        st.process_events();
        st.open_at(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/draw.rs")), Some(400), None);
        st.process_events();
        assert!(st.syntax.as_ref().unwrap().wait_ready(Duration::from_secs(10)));
        let mut ui = Ui::new();
        ui.animate = false;
        let mut scene = Scene::new();
        let lay = layout(&st, (1280, 800));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            st.poll();
            crate::draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
            if st.highlighters.values().all(|h| h.is_settled()) || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let colors = |ui: &Ui| ui.minimap.cache.as_ref().map(|c| c.blocks.clone()).unwrap();
        let incremental = colors(&ui);
        assert!(incremental.iter().any(|k| k.color != incremental[0].color), "highlighted");
        ui.minimap.cache = None;
        crate::draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
        assert_eq!(incremental, colors(&ui), "the same blocks as building from scratch");
    }

    /// `cargo test --release -p shrew rebuild_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn rebuild_cost() {
        let mut st = State::new(shrew_config::Loaded::default(), std::sync::Arc::new(|| {}), true);
        st.open_at(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../shrew-core/src/editor.rs")),
            None,
            None,
        );
        st.process_events();
        let mut ui = Ui::new();
        ui.animate = false;
        let mut scene = Scene::new();
        let lay = layout(&st, (1280, 800));
        let mut times = Vec::new();
        for i in 0..200 {
            st.ed.buf_mut().view.top = i * 10;
            ui.minimap.cache = None;
            crate::draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
            times.push(ui.minimap.last_build);
        }
        times.sort();
        let t = Instant::now();
        crate::draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
        let cached = t.elapsed();
        let n = ui.minimap.cache.as_ref().map_or(0, |c| c.blocks.len());
        eprintln!("rebuild median {:?} max {:?}; whole cached frame {cached:?}; {n} blocks", times[100], times[199]);
        let rope = st.ed.buf().rope().clone();
        let best = |f: &mut dyn FnMut()| {
            (0..50)
                .map(|_| {
                    let t = Instant::now();
                    f();
                    t.elapsed()
                })
                .min()
                .unwrap()
        };
        let blocks = best(&mut || {
            for line in rope.lines_at(1000).take(600) {
                line_blocks(line, &[], 4, 100, FG, &mut |_, _, _| {});
            }
        });
        let chars = best(&mut || {
            let mut c = 0;
            for line in rope.lines_at(1000).take(600) {
                c += line.chars().count();
            }
            assert!(c > 0);
        });
        let c = st.colors;
        let scroll = ui.anim.scroll;
        let cached = best(&mut || {
            scene.clear();
            draw(&mut scene, &st, &mut ui, &lay, &c, scroll);
        });
        assert_eq!(ui.minimap.builds, 200, "drawing from the cache rebuilds nothing");
        eprintln!("cached minimap draw {cached:?}");
        let flat: String = rope.lines_at(1000).take(600).map(|l| l.to_string()).collect();
        let plain = best(&mut || {
            let n = flat.char_indices().filter(|c| !c.1.is_whitespace()).count();
            assert!(n > 0);
        });
        let lines_only = best(&mut || assert_eq!(rope.lines_at(1000).take(600).count(), 600));
        eprintln!(
            "600 lines: blocks {blocks:?}, counting chars {chars:?}, plain string {plain:?}, lines {lines_only:?} \
             (best of 50)"
        );
    }

    fn geo(total: usize, top: f32) -> Geometry {
        // 200 px tall at scale 1: 100 lines fit; 40 rows on screen
        Geometry::new(Rect::new(500.0, 30.0, 100.0, 200.0), 1.0, total, 40, top)
    }

    #[test]
    fn short_files_do_not_scroll() {
        let g = geo(60, 10.0);
        assert_eq!(g.offset, 0.0);
        assert_eq!(g.lines(), 0..60);
        assert_eq!(g.line_y(3.0), 36.0);
        assert_eq!(g.line_at(36.5).floor(), 3.0);
        assert_eq!(g.viewport(10.0), Rect::new(500.0, 50.0, 100.0, 80.0));
        assert_eq!(g.lines_per_px(), 0.5, "the box follows the pointer: 2 px per line");
    }

    #[test]
    fn long_files_scroll_in_proportion() {
        // 1040 lines: the editor scrolls 0..=1000, the minimap 0..=940
        let top = geo(1040, 0.0);
        assert_eq!(top.lines(), 0..100);
        assert_eq!(top.viewport(0.0).y, 30.0);
        let end = geo(1040, 1000.0);
        assert_eq!(end.offset, 940.0 * 2.0);
        assert_eq!(end.lines(), 940..1040);
        // the box ends at the bottom when the editor shows the last line
        assert_eq!(end.viewport(1000.0).bottom(), 230.0);
        let mid = geo(1040, 500.0);
        assert_eq!(mid.offset, 940.0);
        // every line drawn maps back to itself
        for l in mid.lines() {
            let y = mid.line_y(l as f32);
            assert_eq!(mid.line_at(y + 0.5).floor() as usize, l);
            assert_eq!(mid.line_at(y + mid.line_px - 0.5).floor() as usize, l);
        }
        // dragging the box from top to bottom (160 px of travel) scrolls the whole file
        let travel = 200.0 - 80.0;
        assert!((travel * mid.lines_per_px() - 1000.0).abs() < 0.5, "{}", mid.lines_per_px());
    }

    #[test]
    fn clicks_centre_and_drags_follow() {
        // a click on line 50 (below the box) centres it: 40 rows, so 30 is the new top
        let long = geo(200, 0.0);
        assert_eq!(click_target(&long, 0.0, 0.0, long.line_y(50.0) + 1.0), 30.0);
        // but never further than the last line at the bottom of the screen
        let g = geo(60, 0.0);
        assert_eq!(click_target(&g, 0.0, 0.0, g.line_y(55.0)), 20.0);
        assert_eq!(click_target(&g, 0.0, 0.0, g.line_y(90.0)), 20.0, "below the text");
        // a click near the top never scrolls above the first line
        let g2 = geo(60, 15.0);
        assert_eq!(click_target(&g2, 15.0, 15.0, g2.line_y(2.0)), 0.0);
        // a click on the box keeps the view (a drag starts)
        assert_eq!(click_target(&g, 0.0, 0.0, g.line_y(5.0)), 0.0);
        // dragging 20 px down from there moves 10 lines
        assert_eq!(drag_target(&g, (g.line_y(5.0), 0.0), g.line_y(5.0) + 20.0), 10.0);
        assert_eq!(drag_target(&g, (100.0, 3.0), 0.0), 0.0, "clamped at the top");
        assert_eq!(drag_target(&g, (0.0, 3.0), 500.0), 20.0, "and at the bottom");
        // scale 2: twice the pixels per line, same lines
        let g = Geometry::new(Rect::new(1000.0, 60.0, 200.0, 400.0), 2.0, 200, 40, 0.0);
        assert_eq!(g.line_px, 4.0);
        assert_eq!(g.cols(), 96);
        assert_eq!(click_target(&g, 0.0, 0.0, g.line_y(50.0) + 1.0), 30.0);
    }
}
