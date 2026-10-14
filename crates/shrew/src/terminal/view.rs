//! Drawing the terminal panel: the tab strip with its buttons, and the active terminal's screen on the text grid.
//!
//! The screen is read under its lock only when it changed: the PTY thread marks it dirty, the UI marks a view
//! stale when it scrolls, selects or resizes. Then only the rows alacritty reports as damaged are turned into text
//! runs and background rects again; every other frame (the editor's cursor blinking, typing in a buffer) redraws
//! the panel from those cached rows without touching the terminal at all.

use super::session::{Palette, indexed};
use super::{Geom, Tab, Terminals, normal};
use crate::colors::{Colors, mix};
use crate::draw::Ui;
use crate::layout::Layout;
use alacritty_terminal::grid::{Dimensions, Row as GridRow};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors as TermColors;
use alacritty_terminal::term::{TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Rgb};
use shrew_render::{Color, Rect, Scene, TextStyle, char_columns, columns};
use std::ops::Range;
use std::sync::atomic::Ordering;

/// The theme's colors for cells (a program's own OSC 4/10/11 colors win over them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pal {
    pub ansi: [Color; 16],
    pub fg: Color,
    pub bg: Color,
    pub cursor: Color,
}

impl Pal {
    pub fn new(c: &Colors) -> Pal {
        Pal { ansi: c.ansi, fg: c.foreground, bg: c.background, cursor: c.cursor }
    }

    /// The same colors for answering programs' color queries.
    pub fn rgb(&self) -> Palette {
        let k = |c: Color| Rgb { r: c.r, g: c.g, b: c.b };
        let mut p = [k(self.fg); 19];
        for (i, c) in self.ansi.iter().enumerate() {
            p[i] = k(*c);
        }
        p[17] = k(self.bg);
        p[18] = k(self.cursor);
        p
    }
}

fn rgb(c: Rgb) -> Color {
    Color::rgb(c.r, c.g, c.b)
}

/// A cell color as drawn.
fn resolve(c: AnsiColor, tc: &TermColors, p: &Pal) -> Color {
    match c {
        AnsiColor::Spec(c) => rgb(c),
        AnsiColor::Indexed(i) => tc[i as usize]
            .map(rgb)
            .unwrap_or_else(|| if i < 16 { p.ansi[i as usize] } else { rgb(indexed(i, &p.rgb())) }),
        AnsiColor::Named(n) => {
            let i = n as usize;
            if let Some(c) = tc[i] {
                return rgb(c);
            }
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground => p.fg,
                NamedColor::Background => p.bg,
                NamedColor::Cursor => p.cursor,
                NamedColor::DimForeground => mix(p.fg, p.bg, 0.66),
                _ if i < 16 => p.ansi[i],
                // DimBlack..=DimWhite
                _ if (259..267).contains(&i) => mix(p.ansi[i - 259], p.bg, 0.66),
                _ => p.fg,
            }
        }
    }
}

/// Foreground and background of a cell after inverse, dim and hidden.
fn cell_colors(cell: &Cell, tc: &TermColors, p: &Pal) -> (Color, Color) {
    let mut fg = resolve(cell.fg, tc, p);
    let mut bg = resolve(cell.bg, tc, p);
    let f = cell.flags;
    if f.contains(Flags::DIM) {
        fg = mix(fg, bg, 0.66);
    }
    if f.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if f.contains(Flags::HIDDEN) {
        fg = bg;
    }
    (fg, bg)
}

/// Text drawn from one column on.
#[derive(Default)]
struct Seg {
    col: usize,
    text: String,
    spans: Vec<(Range<usize>, TextStyle)>,
}

/// One screen row, ready to draw.
#[derive(Default)]
struct Row {
    segs: Vec<Seg>,
    /// (first column, past the last, color) of backgrounds other than the default, and of strike-through lines.
    bgs: Vec<(usize, usize, Color)>,
    strikes: Vec<(usize, usize, Color)>,
}

fn push_run(runs: &mut Vec<(usize, usize, Color)>, a: usize, z: usize, c: Color) {
    match runs.last_mut() {
        Some(last) if last.1 == a && last.2 == c => last.1 = z,
        _ => runs.push((a, z, c)),
    }
}

impl Row {
    fn build(&mut self, row: &GridRow<Cell>, cols: usize, tc: &TermColors, p: &Pal) {
        self.segs.clear();
        self.bgs.clear();
        self.strikes.clear();
        let base = TextStyle::new(p.fg);
        let mut seg = Seg::default();
        // the span being extended: (byte where it starts, its style)
        let mut cur: Option<(usize, TextStyle)> = None;
        let mut col = 0;
        while col < cols {
            let cell = &row[Column(col)];
            let f = cell.flags;
            if f.contains(Flags::WIDE_CHAR_SPACER) {
                col += 1;
                continue;
            }
            let w = if f.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
            let (fg, bg) = cell_colors(cell, tc, p);
            if bg != p.bg {
                push_run(&mut self.bgs, col, col + w, bg);
            }
            if f.contains(Flags::STRIKEOUT) {
                push_run(&mut self.strikes, col, col + w, fg);
            }
            let ch = if f.contains(Flags::LEADING_WIDE_CHAR_SPACER) { ' ' } else { cell.c };
            let mut style = TextStyle::new(fg);
            style.bold = f.contains(Flags::BOLD);
            style.italic = f.contains(Flags::ITALIC);
            if f.intersects(Flags::ALL_UNDERLINES) {
                style.underline = Some(cell.underline_color().map_or(fg, |u| resolve(u, tc, p)));
            }
            if ch.is_control() || char_columns(ch) != w {
                // the renderer would give it another width than the terminal does: drawn on its own, so the rest
                // of the row stays on the grid
                self.finish(&mut seg, &mut cur, base, col + w);
                if !ch.is_control() && !ch.is_whitespace() && char_columns(ch) > 0 {
                    self.segs.push(Seg { col, text: ch.to_string(), spans: vec![(0..ch.len_utf8(), style)] });
                }
                col += w;
                continue;
            }
            let at = seg.text.len();
            if cur.is_none_or(|(_, s)| s != style) {
                if let Some((a, s)) = cur.take()
                    && s != base
                {
                    seg.spans.push((a..at, s));
                }
                cur = Some((at, style));
            }
            seg.text.push(ch);
            if let Some(zw) = cell.zerowidth() {
                seg.text.extend(zw.iter().filter(|c| !c.is_control()));
            }
            col += w;
        }
        self.finish(&mut seg, &mut cur, base, cols);
    }

    /// Closes the segment being built (dropping trailing blanks) and starts the next one at `next`.
    fn finish(&mut self, seg: &mut Seg, cur: &mut Option<(usize, TextStyle)>, base: TextStyle, next: usize) {
        let mut s = std::mem::take(seg);
        if let Some((a, st)) = cur.take()
            && st != base
        {
            s.spans.push((a..s.text.len(), st));
        }
        seg.col = next;
        let keep = s.text.trim_end_matches(' ').len();
        // an underlined blank still shows
        let keep = s.spans.iter().filter(|(_, st)| st.underline.is_some()).map(|(r, _)| r.end).fold(keep, usize::max);
        s.text.truncate(keep);
        if !s.text.is_empty() {
            self.segs.push(s);
        }
    }
}

/// The cursor as drawn: viewport row, column, shape, and the char under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorView {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub wide: bool,
    pub ch: char,
}

/// What the panel draws of one terminal, kept between frames.
#[derive(Default)]
pub struct View {
    rows: Vec<Row>,
    cols: usize,
    pal: Option<Pal>,
    pub cursor: Option<CursorView>,
    selection: Option<SelectionRange>,
    pub display_offset: usize,
    pub history: usize,
    pub mode: TermMode,
    /// Rebuild every row at the next frame: the UI scrolled, selected, moved the normal-mode cursor.
    pub stale: bool,
}

impl View {
    /// Brings the cached rows up to date with the screen, if anything changed.
    fn sync(&mut self, tab_dirty: bool, tab: &super::Tab, p: &Pal) {
        let rows = tab.session.size.rows;
        let cols = tab.session.size.cols;
        let shape_changed = self.rows.len() != rows || self.cols != cols || self.pal != Some(*p);
        if !tab_dirty && !self.stale && !shape_changed {
            return;
        }
        let mut term = tab.session.term.lock();
        let offset = term.grid().display_offset();
        let mut full = self.stale || shape_changed || offset != self.display_offset || offset != 0;
        let mut damaged: Vec<usize> = Vec::new();
        match term.damage() {
            TermDamage::Full => full = true,
            TermDamage::Partial(lines) => {
                if !full {
                    damaged.extend(lines.map(|l| l.line));
                }
            }
        }
        term.reset_damage();
        self.rows.resize_with(rows, Row::default);
        self.cols = cols;
        self.pal = Some(*p);
        self.stale = false;
        let grid = term.grid();
        let screen = grid.screen_lines().min(rows);
        let tc = *term.colors();
        let build = |r: usize, row: &mut Row| {
            let line = Line(r as i32 - offset as i32);
            row.build(&grid[line], cols.min(grid.columns()), &tc, p);
        };
        if full {
            for (r, row) in self.rows.iter_mut().enumerate().take(screen) {
                build(r, row);
            }
        } else {
            for r in damaged.into_iter().filter(|&r| r < screen) {
                build(r, &mut self.rows[r]);
            }
        }
        let content = term.renderable_content();
        let c = content.cursor;
        let row = c.point.line.0 + offset as i32;
        self.cursor = (c.shape != CursorShape::Hidden && (0..screen as i32).contains(&row)).then(|| {
            let cell = &grid[c.point];
            CursorView {
                row: row as usize,
                col: c.point.column.0,
                shape: c.shape,
                wide: cell.flags.contains(Flags::WIDE_CHAR),
                ch: cell.c,
            }
        });
        self.selection = content.selection;
        self.display_offset = offset;
        self.history = grid.history_size();
        self.mode = content.mode;
    }
}

// ----- drawing -----

/// Draws the panel (when it is open) and remembers its geometry for the mouse handlers.
pub fn draw(scene: &mut Scene, t: &mut Terminals, ui: &Ui, lay: &Layout, c: &Colors) {
    t.lay = Some(*lay);
    t.tab_hits.clear();
    if !t.visible || t.tabs.is_empty() || lay.panel.h < 1.0 {
        t.geom = None;
        return;
    }
    let g = Geom::new(lay);
    t.geom = Some(g);
    let size = g.size();
    let pal = Pal::new(c);
    if t.palette != Some(pal) {
        t.palette = Some(pal);
        for tab in &t.tabs {
            tab.session.set_palette(pal.rgb());
        }
    }
    for tab in &mut t.tabs {
        tab.session.resize(size);
    }
    let focused = t.focused && ui.focused;
    let tab = &mut t.tabs[t.active];
    let dirty = tab.session.shared.dirty.swap(false, Ordering::AcqRel);
    // the view borrows the tab immutably to read the screen
    let mut view = std::mem::take(&mut tab.view);
    view.sync(dirty, tab, &pal);
    tab.view = view;

    scene.rect(g.panel, c.background);
    strip(scene, t, &g, lay, c, focused);
    let tab = &t.tabs[t.active];
    body(scene, tab, &g, lay, c, ui, focused, t.hover.as_ref().filter(|h| h.tab == tab.id));
}

/// The tab strip: a tab per terminal, then the mode, the scroll position and the buttons on the right.
fn strip(scene: &mut Scene, t: &mut Terminals, g: &Geom, lay: &Layout, c: &Colors, focused: bool) {
    let s = g.strip;
    let line = lay.scale.round().max(1.0);
    scene.rect(s, c.panel_background);
    scene.rect(Rect::new(s.x, s.y, s.w, line), c.panel_border);
    // right side first: buttons, then status texts leftwards
    let hover = t.hover_button;
    for (r, which) in [(g.new, Button::New), (g.max, Button::Max), (g.close, Button::Close)] {
        if hover == Some(which) {
            scene.rounded_rect(r, (4.0 * lay.scale).round(), c.panel_selection, None);
        }
        icon(scene, r, which, t.maximized, lay, c);
    }
    let ty = lay.centered(s.y, s.h);
    let mut right = g.new.x - lay.pad * 2.0;
    let mut item = |scene: &mut Scene, text: &str, col: Color, badge: Option<Color>| {
        let w = columns(text) as f32 * lay.cw;
        match badge {
            Some(bg) => {
                let bw = w + 2.0 * lay.cw;
                right -= bw;
                let r = Rect::new(right.round(), s.y + lay.pad, bw.round(), s.h - 2.0 * lay.pad);
                scene.rounded_rect(r, (4.0 * lay.scale).round(), bg, None);
                scene.text((right + lay.cw).round(), ty, text, TextStyle::new(col).bold(), &[]);
            }
            None => {
                right -= w;
                scene.text(right.round(), ty, text, TextStyle::new(col), &[]);
            }
        }
        right -= 2.0 * lay.cw;
    };
    let tab = &t.tabs[t.active];
    let exited = tab.session.exited();
    if exited {
        let code = *tab.session.shared.exit_code.lock().unwrap_or_else(|e| e.into_inner());
        let msg = match code {
            Some(0) => "done · Enter closes".to_string(),
            Some(n) => format!("exit {n} · Enter closes"),
            None => "ended · Enter closes".to_string(),
        };
        item(scene, &msg, if code.is_some_and(|n| n != 0) { c.error } else { c.panel_muted }, None);
    }
    let v = &tab.view;
    if v.display_offset > 0 {
        item(scene, &format!("↑ {}/{}", v.display_offset, v.history), c.panel_muted, None);
    }
    if let Some((text, forward)) = &tab.normal.prompt {
        item(scene, &format!("{}{text}▏", if *forward { '/' } else { '?' }), c.panel_foreground, None);
    }
    if v.mode.contains(TermMode::VI) {
        item(scene, "NORMAL", c.mode_normal_fg, Some(c.mode_normal));
    } else if let Some(p) = t.pending {
        item(scene, &p.to_string(), c.accent_alt, None);
    }

    // tabs, clipped where the right side starts
    let clip = Rect::new(s.x, s.y, (right - s.x).max(0.0), s.h);
    scene.push_clip(clip);
    let radius = (6.0 * lay.scale).round();
    let mut x = s.x + lay.pad;
    for (i, tab) in t.tabs.iter_mut().enumerate() {
        // the title may come from /proc (the shell's directory): looked up at most once a second
        if tab.title.1.is_none_or(|at| at.elapsed() > std::time::Duration::from_secs(1)) {
            tab.title = (elide(&tab.session.title(), 32), Some(std::time::Instant::now()));
        }
        let label = &tab.title.0;
        let w = ((columns(label) + 4) as f32 * lay.cw).round();
        let r = Rect::new(x.round(), s.y + lay.pad, w, s.h - lay.pad);
        let active = i == t.active;
        if active {
            scene.rounded_rect(Rect::new(r.x, r.y, r.w, r.h + radius), radius, c.background, None);
            if focused {
                let bar = (2.0 * lay.scale).round().max(1.0);
                scene.rounded_rect(Rect::new(r.x + radius, r.y, r.w - 2.0 * radius, bar), bar / 2.0, c.accent, None);
            }
        }
        let ink = if active { c.panel_foreground } else { c.panel_muted };
        let lx = r.x + 2.0 * lay.cw;
        scene.text(lx.round(), ty + lay.pad / 2.0, label, TextStyle::new(ink), &[]);
        if tab.session.exited() {
            let dot =
                Rect::new((r.x + 0.75 * lay.cw).round(), (ty + lay.pad / 2.0 + lay.ch / 2.0 - 2.0).round(), 4.0, 4.0);
            scene.rounded_rect(dot, 2.0, c.panel_muted, None);
        }
        t.tab_hits.push((r, i));
        x += w + lay.pad / 2.0;
    }
    scene.pop_clip();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    New,
    Max,
    Close,
}

/// The strip's buttons, drawn from rects and glyphs so they match any font.
fn icon(scene: &mut Scene, r: Rect, which: Button, maximized: bool, lay: &Layout, c: &Colors) {
    let ink = c.panel_muted;
    let line = lay.scale.round().max(1.0);
    let cx = r.x + r.w / 2.0;
    let cy = r.y + r.h / 2.0;
    let k = (r.w * 0.22).round();
    match which {
        Button::New => {
            scene.rect(Rect::new((cx - k).round(), (cy - line / 2.0).round(), 2.0 * k, line), ink);
            scene.rect(Rect::new((cx - line / 2.0).round(), (cy - k).round(), line, 2.0 * k), ink);
        }
        Button::Max => {
            let outline = |scene: &mut Scene, x: f32, y: f32, s: f32| {
                let b = Rect::new(x.round(), y.round(), s.round(), s.round());
                scene.rounded_rect(b, line, Color::TRANSPARENT, Some((line, ink)));
            };
            if maximized {
                let s = 1.4 * k;
                outline(scene, cx - k + 0.6 * k, cy - k, s);
                scene.rect(
                    Rect::new((cx - k).round(), (cy - k + 0.6 * k).round(), s.round(), s.round()),
                    c.panel_background,
                );
                outline(scene, cx - k, cy - k + 0.6 * k, s);
            } else {
                outline(scene, cx - k, cy - k, 2.0 * k);
            }
        }
        Button::Close => {
            let ty = lay.centered(r.y, r.h);
            let x = (cx - lay.cw / 2.0).round();
            scene.text(x, ty, "×", TextStyle::new(ink), &[]);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn body(
    scene: &mut Scene,
    tab: &Tab,
    g: &Geom,
    lay: &Layout,
    c: &Colors,
    ui: &Ui,
    focused: bool,
    hover: Option<&super::Hover>,
) {
    let v = &tab.view;
    let (cw, ch) = (lay.cw, lay.ch);
    scene.push_clip(g.body);
    let cell = |col: usize, row: usize, w: usize| g.cell_rect(col, row, w);
    for (r, row) in v.rows.iter().enumerate() {
        for &(a, z, color) in &row.bgs {
            scene.rect(cell(a, r, z - a), color);
        }
    }
    // selection, the search match, the link under the pointer
    let off = v.display_offset as i32;
    let spans = |a: Point, z: Point, block: bool, f: &mut dyn FnMut(usize, usize, usize)| {
        for r in 0..v.rows.len() {
            let line = r as i32 - off;
            if line < a.line.0 || line > z.line.0 {
                continue;
            }
            let (ca, cz) = if block {
                (a.column.0.min(z.column.0), a.column.0.max(z.column.0))
            } else {
                (if line == a.line.0 { a.column.0 } else { 0 }, if line == z.line.0 { z.column.0 } else { v.cols - 1 })
            };
            if cz >= ca {
                f(r, ca, cz + 1);
            }
        }
    };
    if let Some(s) = &v.selection {
        let col = if focused { c.selection } else { c.selection_inactive };
        spans(s.start, s.end, s.is_block, &mut |r, a, z| scene.rect(cell(a, r, z - a), col));
    }
    if let Some(m) = &tab.normal.current {
        spans(*m.start(), *m.end(), false, &mut |r, a, z| {
            scene.rounded_rect(cell(a, r, z - a), (2.0 * lay.scale).round(), c.search_current, None)
        });
    }
    if let Some(h) = hover {
        let r = h.line.0 + off;
        if (0..v.rows.len() as i32).contains(&r) {
            let rect = cell(h.cols.start, r as usize, h.cols.len());
            let t = lay.scale.round().max(1.0);
            scene.rect(Rect::new(rect.x, rect.bottom() - t, rect.w, t), c.accent);
        }
    }
    let base = TextStyle::new(tab.view.pal.map_or(c.foreground, |p| p.fg));
    for (r, row) in v.rows.iter().enumerate() {
        let y = g.y0 + r as f32 * ch;
        for s in &row.segs {
            scene.text((g.x0 + s.col as f32 * cw).round(), y, &s.text, base, &s.spans);
        }
        for &(a, z, color) in &row.strikes {
            let rc = cell(a, r, z - a);
            scene.rect(Rect::new(rc.x, (rc.y + ch / 2.0).round(), rc.w, lay.scale.round().max(1.0)), color);
        }
    }
    // scrolled back: where in the history
    if v.display_offset > 0 && v.history > 0 {
        let track =
            Rect::new(g.body.right() - (8.0 * lay.scale).round(), g.y0, (4.0 * lay.scale).round(), g.rows as f32 * ch);
        let total = (v.history + g.rows) as f32;
        let h = (track.h * g.rows as f32 / total).max((16.0 * lay.scale).round()).min(track.h);
        let frac = 1.0 - v.display_offset as f32 / v.history as f32;
        let y = (track.y + frac * (track.h - h)).round();
        scene.rounded_rect(Rect::new(track.x, y, track.w, h), track.w / 2.0, c.scrollbar, None);
    }
    scene.pop_clip();

    // the cursor, above the text
    if let Some(cur) = v.cursor {
        scene.layer(1);
        scene.push_clip(g.body);
        let w = if cur.wide { 2 } else { 1 };
        let block = cell(cur.col, cur.row, w);
        let bar = (2.0 * lay.scale).round().max(1.0);
        let vi = v.mode.contains(TermMode::VI);
        let hollow = !focused || cur.shape == CursorShape::HollowBlock;
        if hollow {
            scene.rounded_rect(block, 0.0, Color::TRANSPARENT, Some((lay.scale.round().max(1.0), c.cursor)));
        } else if !ui.blink_off || vi {
            let shape = if vi { CursorShape::Block } else { cur.shape };
            match shape {
                CursorShape::Beam => scene.rect(Rect::new(block.x, block.y, bar, ch), c.cursor),
                CursorShape::Underline => scene.rect(Rect::new(block.x, block.bottom() - bar, block.w, bar), c.cursor),
                _ => {
                    scene.rect(block, c.cursor);
                    if !cur.ch.is_whitespace() && !cur.ch.is_control() {
                        let mut b = [0u8; 4];
                        scene.push_clip(block);
                        scene.text(block.x, block.y, cur.ch.encode_utf8(&mut b), TextStyle::new(c.cursor_text), &[]);
                        scene.pop_clip();
                    }
                }
            }
        }
        scene.pop_clip();
        scene.layer(0);
    }
}

/// `s` cut to `cols` columns with an ellipsis.
fn elide(s: &str, cols: usize) -> String {
    if columns(s) <= cols {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 1;
    for ch in s.chars() {
        w += char_columns(ch);
        if w > cols {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// The text of a screen row and the column each of its chars starts at (for finding links).
pub fn row_text(tab: &Tab, line: Line) -> (String, Vec<usize>) {
    let term = tab.session.term.lock();
    let grid = term.grid();
    let mut text = String::new();
    let mut cols = Vec::new();
    if line < grid.topmost_line() || line > grid.bottommost_line() {
        return (text, cols);
    }
    let row = &grid[line];
    for col in 0..grid.columns() {
        let cell = &row[Column(col)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        for _ in 0..cell.c.len_utf8() {
            cols.push(col);
        }
        text.push(cell.c);
    }
    (text, cols)
}

/// Exited-normal-mode check for a key handler: whether the screen of `tab` is in normal mode now.
pub fn in_normal(tab: &Tab) -> bool {
    normal::active(&tab.session.term.lock())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::session::{Listener, Shared, Size};
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    fn pal() -> Pal {
        let mut ansi = [Color::rgb(0, 0, 0); 16];
        for (i, a) in ansi.iter_mut().enumerate() {
            *a = Color::rgb(i as u8 * 10, 0, 0);
        }
        Pal { ansi, fg: Color::rgb(200, 200, 200), bg: Color::rgb(20, 20, 20), cursor: Color::rgb(255, 0, 0) }
    }

    fn row_of(bytes: &str) -> Row {
        let size = Size { cols: 12, rows: 2, cell_w: 8, cell_h: 16 };
        let mut t = Term::new(Config::default(), &size, Listener(Shared::detached()));
        Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new().advance(&mut t, bytes.as_bytes());
        let mut row = Row::default();
        row.build(&t.grid()[Line(0)], 12, t.colors(), &pal());
        row
    }

    #[test]
    fn colors_bold_and_backgrounds() {
        let p = pal();
        let row = row_of("ab\x1b[31;1mcd\x1b[0m \x1b[44mx\x1b[0m\x1b[38;2;1;2;3mz");
        assert_eq!(row.segs.len(), 1);
        let s = &row.segs[0];
        assert_eq!(s.text, "abcd xz");
        let red = TextStyle::new(p.ansi[1]).bold();
        assert_eq!(s.spans, vec![(2..4, red), (6..7, TextStyle::new(Color::rgb(1, 2, 3)))]);
        assert_eq!(row.bgs, vec![(5, 6, p.ansi[4])]);
    }

    #[test]
    fn inverse_and_256_colors() {
        let p = pal();
        let row = row_of("\x1b[7mi\x1b[0m\x1b[38;5;196mr");
        assert_eq!(row.bgs, vec![(0, 1, p.fg)]);
        assert_eq!(
            row.segs[0].spans,
            vec![(0..1, TextStyle::new(p.bg)), (1..2, TextStyle::new(Color::rgb(255, 0, 0)))]
        );
    }

    #[test]
    fn wide_chars_keep_the_grid() {
        let row = row_of("a日b");
        assert_eq!(row.segs.len(), 1);
        assert_eq!(row.segs[0].text, "a日b");
        // underlined blanks are kept, plain ones dropped
        let row = row_of("x\x1b[4m  \x1b[0m   ");
        assert_eq!(row.segs[0].text, "x  ");
    }
}
