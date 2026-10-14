//! Builds one frame: the editor state turned into a [`Scene`] of rects and grid text. Only visible lines are
//! touched, so a frame costs the same in a 10-line file and a 10-million-line one.
//!
//! Layers: 0 = editor (backgrounds, selections, text) and bars; 1 = cursors and the scrollbar (above text);
//! 2 = the picker, About and Settings overlays; 3 = what opens over Settings (its lists, the pickers).

use crate::anim::{Anim, Target};
use crate::colors::{Colors, mix};
use crate::layout::{Layout, Scroll};
use crate::state::{State, Tone};
use shrew_core::features::Feature;
use shrew_core::picker::{PickerKind, fuzzy_match};
use shrew_core::ropey::Rope;
use shrew_core::{CursorShape, Editor, Mode, Prompt, PromptKind, keymap, motion, search, text};
use shrew_render::{Color, Rect, Scene, TextStyle, char_columns, columns};
use std::ops::Range;
use std::time::Instant;

/// Clickable areas of the last frame, for the mouse handlers.
#[derive(Default, Debug)]
pub struct Hits {
    pub tabs: Vec<(Rect, usize)>,
    pub update: Option<Rect>,
    pub picker: Option<Rect>,
    /// Picker rows and the index into `matches` each shows.
    pub picker_rows: Vec<(Rect, usize)>,
    pub about: Option<Rect>,
    /// The Settings gear in the status bar.
    pub gear: Option<Rect>,
    /// The scrollbar track and the line count it maps to.
    pub scrollbar: Option<(Rect, usize)>,
}

/// UI state that lives between frames and is not the editor's.
#[derive(Default)]
pub struct Ui {
    pub anim: Anim,
    /// Smooth scrolling and the cursor glide (off for headless frames, which must show the final state).
    pub animate: bool,
    pub focused: bool,
    /// The cursor is in the "off" phase of blinking.
    pub blink_off: bool,
    /// Touchpad scrolling: fraction of a line past `view.top` (only while `view.top == frac_top`).
    pub frac: f32,
    pub frac_top: usize,
    pub picker_top: usize,
    pub hits: Hits,
    /// The primary cursor's rect (for the input method's candidate window).
    pub cursor_rect: Option<Rect>,
    /// Text being composed in an input method, drawn at the cursor.
    pub preedit: String,
    /// Bracket match for (buffer, version, position): searching can walk far, so it runs once per position.
    bracket: Option<BracketCache>,
    line: LineBuf,
}

impl Ui {
    pub fn new() -> Self {
        Ui { focused: true, animate: true, ..Ui::default() }
    }

    /// The scroll position the editor asks for (plus touchpad fraction).
    pub fn target_scroll(&self, ed: &Editor) -> Scroll {
        let v = ed.buf().view;
        let frac = if self.frac_top == v.top { self.frac } else { 0.0 };
        Scroll { top: v.top as f32 + frac, left: v.left as f32 }
    }
}

/// (buffer, version, cursor) -> the bracket pair found there.
type BracketCache = (u64, u64, usize, Option<(usize, usize)>);

/// Scratch for one line of text: the tab-expanded visible part and a map from source bytes to its bytes.
#[derive(Default)]
struct LineBuf {
    disp: String,
    map: Vec<u32>,
    spans: Vec<(Range<usize>, TextStyle)>,
}

/// Fills `scene` with the frame. Returns true while an animation needs more frames.
pub fn build(scene: &mut Scene, st: &mut State, ui: &mut Ui, lay: &Layout, now: Instant) -> bool {
    let c = st.colors;
    for b in &mut st.ed.buffers {
        b.view.rows = lay.rows;
        b.view.cols = lay.cols;
    }
    // where things should be, and where they are drawn this frame
    let tw = st.ed.settings.tab_width.max(1);
    let (buf_id, head) = (st.ed.buf().id, st.ed.buf().sel.primary().head);
    let cursor_target = {
        let doc = &st.ed.buf().doc;
        let h = head.min(doc.len());
        (text::line_of(&doc.rope, h) as f32, doc.col_of(h, tw) as f32)
    };
    // a different buffer, or a file loaded into the reused scratch buffer, appears without animation
    let buffer = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (buf_id, &st.ed.buf().doc.path).hash(&mut h);
        h.finish()
    };
    let target = Target {
        buffer,
        scroll: ui.target_scroll(&st.ed),
        cursor: cursor_target,
        rows: lay.rows,
        smooth_scroll: ui.animate && st.ed.features.on(Feature::SmoothScroll),
        cursor_animation: ui.animate && st.ed.features.on(Feature::CursorAnimation),
    };
    let animating = ui.anim.step(&target, now);
    let scroll = ui.anim.scroll;
    st.sync_syntax(scroll.first_line(), lay.rows + 2);

    scene.clear();
    scene.background(c.background);
    ui.hits = Hits::default();
    ui.cursor_rect = None;

    tabs(scene, st, ui, lay, &c);
    editor(scene, st, ui, lay, &c, scroll);
    status(scene, st, ui, lay, &c);
    cmdline(scene, &st.ed, st, lay, &c);
    crate::settings::draw(scene, st, lay, &c);
    if st.ed.picker.is_some() {
        picker(scene, &st.ed, ui, lay, &c);
    }
    if st.ed.about.is_some() {
        about(scene, &mut st.ed, ui, lay, &c);
    }
    animating
}

// ----- the text area -----

fn editor(scene: &mut Scene, st: &State, ui: &mut Ui, lay: &Layout, c: &Colors, scroll: Scroll) {
    let ed = &st.ed;
    let b = ed.buf();
    let rope = &b.doc.rope;
    let tw = ed.settings.tab_width.max(1);
    let f = |x| ed.features.on(x);
    let last = text::last_line(rope);
    let first = scroll.first_line().min(last);
    let rows = lay.rows + 1 + usize::from(scroll.y_offset(lay.ch) > 0.0);
    let end = (first + rows).min(last + 1); // exclusive
    let text_rect = lay.text_rect();
    let numbers = f(Feature::LineNumbers);
    let gutter = Rect::new(lay.text.x, lay.text.y, lay.gutter_w, lay.text.h);
    if c.gutter_background != c.background {
        scene.rect(gutter, c.gutter_background);
    }
    let head = b.sel.primary().head.min(rope.len_chars());
    let cur_line = text::line_of(rope, head);
    let insert_like = ed.mode == Mode::Insert || !ed.vim();
    let overlay = ed.prompt.is_some() || ed.picker.is_some() || ed.about.is_some() || st.settings.is_open();

    scene.push_clip(lay.text);
    // current line
    let has_selection = ed.mode.is_visual() || (insert_like && !b.sel.primary().is_empty());
    if f(Feature::CurrentLine) && !has_selection && (first..end).contains(&cur_line) {
        let y = scroll.line_y(lay, cur_line);
        scene.rect(Rect::new(lay.text.x, y, lay.text.w, lay.ch), c.current_line);
    }
    scene.push_clip(text_rect);
    let char_rect = |line: usize, ca: usize, cz: usize| {
        let x = scroll.col_x(lay, ca as f32);
        Rect::new(x, scroll.line_y(lay, line), (cz.max(ca) - ca) as f32 * lay.cw, lay.ch)
    };
    let radius = (2.0 * lay.scale).round();

    // search matches and other occurrences of the word under the cursor
    if f(Feature::SearchHighlight)
        && ed.search_visible
        && end > first
        && let Some(re) = &ed.search
    {
        for (a, z) in search::matches_in(rope, re, first, end - 1) {
            let current = ((a..z.max(a + 1)).contains(&head) && !overlay) || (overlay && a == head);
            let col = if current { c.search_current } else { c.search_match };
            span_rects(rope, a, z.max(a + 1), first, end, tw, &mut |l, ca, cz| {
                scene.rounded_rect(char_rect(l, ca, cz), radius, col, None)
            });
        }
    }
    if f(Feature::WordHighlight)
        && end > first
        && let Some((word, at)) = word_at_cursor(ed)
    {
        for l in first..end {
            for (a, z) in word_matches(rope, l, &word) {
                if a != at {
                    span_rects(rope, a, z, first, end, tw, &mut |l, ca, cz| {
                        scene.rounded_rect(char_rect(l, ca, cz), radius, c.word_highlight, None);
                    });
                }
            }
        }
    }
    // selections
    let sel_color = if ui.focused { c.selection } else { c.selection_inactive };
    let vis_a = text::line_start(rope, first);
    let vis_z = if end > last { rope.len_chars() } else { text::line_start(rope, end) };
    let ranges = b.sel.ranges();
    let start = ranges.partition_point(|r| r.to() + 1 < vis_a);
    for r in ranges[start..].iter().take_while(|r| r.from() <= vis_z) {
        if let Some((a, z)) = selection_span(ed.mode, rope, r.from(), r.to(), r.is_empty()) {
            span_rects(rope, a, z, first, end, tw, &mut |l, ca, cz| scene.rect(char_rect(l, ca, cz), sel_color));
        }
    }
    // matching bracket
    if f(Feature::BracketMatch)
        && !overlay
        && let Some((p, q)) = bracket_pair(ui, b.id, b.doc.version, rope, head, insert_like)
    {
        for x in [p, q] {
            let l = text::line_of(rope, x);
            if (first..end).contains(&l) {
                let ca = text::col_of(rope, x, tw);
                scene.rounded_rect(char_rect(l, ca, ca + 1), radius, c.bracket_match, None);
            }
        }
    }
    if f(Feature::IndentGuides) {
        indent_guides(scene, rope, first, end, cur_line, tw, lay, scroll, c);
    }

    // text
    let fg = TextStyle::new(c.foreground);
    let ws = f(Feature::Whitespace);
    #[cfg(feature = "syntax")]
    let syntax_on = f(Feature::Syntax);
    for l in first..end {
        let lb = &mut ui.line;
        let start_col = layout_line(lb, rope, l, tw, scroll.left.max(0.0) as usize, lay.cols + 2, ws);
        if lb.disp.is_empty() {
            continue;
        }
        lb.spans.clear();
        #[cfg(feature = "syntax")]
        if syntax_on && let Some(spans) = st.spans(b.id, l) {
            for s in spans {
                let (sa, sz) = (s.start as usize, s.end as usize);
                if sa >= lb.map.len() {
                    break;
                }
                let a = lb.map[sa] as usize;
                let z = lb.map.get(sz).map_or(lb.disp.len(), |&z| z as usize);
                if a < z {
                    let col = Color::rgb(s.fg.r, s.fg.g, s.fg.b);
                    let mut style = TextStyle::new(col);
                    style.bold = s.bold;
                    style.italic = s.italic;
                    style.underline = s.underline.then_some(col);
                    lb.spans.push((a..z, style));
                }
            }
        }
        if ws {
            mark_whitespace(lb, c.whitespace);
        }
        scene.text(scroll.col_x(lay, start_col as f32), scroll.line_y(lay, l), &lb.disp, fg, &lb.spans);
    }
    scene.pop_clip();

    // gutter
    if numbers {
        let rel = f(Feature::RelativeNumbers);
        let mut num = String::new();
        for l in first..end {
            num.clear();
            let n = if rel && l != cur_line { l.abs_diff(cur_line) } else { l + 1 };
            use std::fmt::Write;
            let _ = write!(num, "{n}");
            let x = lay.text.x + (1 + lay.digits.saturating_sub(num.len())) as f32 * lay.cw;
            let col = if l == cur_line { c.gutter_active } else { c.gutter_foreground };
            scene.text(x.round(), scroll.line_y(lay, l), &num, TextStyle::new(col), &[]);
        }
    }
    scene.pop_clip();

    if b.doc.path.is_none() && !b.doc.dirty && b.doc.is_empty() && ed.buffers.len() == 1 && st.loading.is_empty() {
        welcome(scene, ed, lay, c);
    }
    if let Some(l) = st.loading.first() {
        let msg = format!(
            "Loading {} ({})…",
            l.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            human_bytes(l.bytes)
        );
        let x = (lay.text.x + (lay.text.w - columns(&msg) as f32 * lay.cw) / 2.0).round();
        let y = (lay.text.y + lay.text.h / 2.0 - lay.ch).round();
        scene.text(x, y, &msg, TextStyle::new(c.panel_muted), &[]);
    }

    // cursors, above the text
    scene.layer(1);
    scene.push_clip(text_rect);
    cursors(scene, ed, ui, lay, c, scroll, first, end, vis_a, vis_z);
    if let Some(r) = ui.cursor_rect.filter(|_| !ui.preedit.is_empty()) {
        let w = columns(&ui.preedit) as f32 * lay.cw;
        scene.rect(Rect::new(r.x, r.y, w, lay.ch), c.background);
        scene.text(r.x, r.y, &ui.preedit, TextStyle::new(c.foreground).underline(c.accent), &[]);
    }
    scene.pop_clip();
    scrollbar(scene, ui, lay, c, scroll, last + 1);
    scene.layer(0);
}

/// Lays out the visible columns of `line` into `lb.disp` (tabs expanded, control chars shown as symbols) and fills
/// `lb.map` (source byte -> byte in `disp`). Returns the display column `disp` starts at.
fn layout_line(lb: &mut LineBuf, rope: &Rope, line: usize, tw: usize, left: usize, cols: usize, ws: bool) -> usize {
    lb.disp.clear();
    lb.map.clear();
    let slice = rope.line(line);
    let mut n = slice.len_chars();
    if n > 0 && slice.char(n - 1) == '\n' {
        n -= 1;
    }
    // trailing white space starts here (for the whitespace feature)
    let trailing = if ws {
        let mut it = slice.chars_at(n);
        let mut i = n;
        while let Some(ch) = it.prev() {
            if ch != ' ' && ch != '\t' {
                break;
            }
            i -= 1;
        }
        i
    } else {
        usize::MAX
    };
    let end_col = left + cols;
    let mut col = 0usize;
    let mut start: Option<usize> = None;
    for (i, ch) in slice.chars().take(n).enumerate() {
        let w = if ch == '\t' { tw - col % tw } else { char_columns(ch) };
        let bytes = ch.len_utf8();
        if left > 0 && start.is_none() && col + w <= left {
            lb.map.extend(std::iter::repeat_n(0, bytes));
            col += w;
            continue;
        }
        if col >= end_col {
            break;
        }
        start.get_or_insert(col);
        let at = lb.disp.len() as u32;
        lb.map.extend(std::iter::repeat_n(at, bytes));
        match ch {
            '\t' => {
                lb.disp.push(if ws { '→' } else { ' ' });
                lb.disp.extend(std::iter::repeat_n(' ', w - 1));
            }
            ' ' if i >= trailing => lb.disp.push('·'),
            '\u{7f}' => lb.disp.push('␡'),
            ch if (ch as u32) < 0x20 => lb.disp.push(char::from_u32(0x2400 + ch as u32).unwrap_or('?')),
            ch if ch.is_control() => lb.disp.push('\u{fffd}'),
            ch => lb.disp.push(ch),
        }
        col += w;
    }
    start.unwrap_or(col)
}

/// Colors the tab arrows and trailing-space dots `layout_line` put in.
fn mark_whitespace(lb: &mut LineBuf, col: Color) {
    let style = TextStyle::new(col);
    let mut extra = Vec::new();
    for (i, ch) in lb.disp.char_indices() {
        if ch == '→' || ch == '·' {
            extra.push((i..i + ch.len_utf8(), style));
        }
    }
    // a text that really contains these chars gets them dimmed too: harmless
    lb.spans.extend(extra);
}

/// Calls `f(line, from_col, to_col)` for each visible line piece of the char range `[a, z)`. A range that takes a
/// line break shows one extra cell there.
fn span_rects(
    rope: &Rope,
    a: usize,
    z: usize,
    first: usize,
    end: usize,
    tw: usize,
    f: &mut dyn FnMut(usize, usize, usize),
) {
    if z <= a {
        return;
    }
    let la = text::line_of(rope, a).max(first);
    let lz = text::line_of(rope, z.saturating_sub(1)).min(end.saturating_sub(1));
    for l in la..=lz {
        if l < first || l >= end {
            continue;
        }
        let ls = text::line_start(rope, l);
        let le = text::line_end(rope, l);
        let from = a.max(ls);
        let to = z.min(le);
        let ca = if from > ls { text::col_of(rope, from, tw) } else { 0 };
        let mut cz = if to > from { text::col_of(rope, to, tw) } else { ca };
        if z > le && le < rope.len_chars() {
            cz += 1; // the line break is selected
        }
        if cz > ca {
            f(l, ca, cz);
        }
    }
}

/// The highlighted `[from, to)` of one selection range, as shrew-core's `selection_spans` computes it.
fn selection_span(mode: Mode, rope: &Rope, from: usize, to: usize, empty: bool) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    match mode {
        Mode::Visual | Mode::VisualBlock => Some((from, text::grapheme_end(rope, to).min(len))),
        Mode::VisualLine => {
            let (a, z) = (text::line_of(rope, from), text::line_of(rope, to));
            let end = if z < text::last_line(rope) { text::line_start(rope, z + 1) } else { len };
            Some((text::line_start(rope, a), end))
        }
        _ => (!empty).then_some((from, to)),
    }
}

/// The pair of brackets to highlight: the one at the cursor (or just before it when typing) and its match.
fn bracket_pair(
    ui: &mut Ui,
    id: u64,
    version: u64,
    rope: &Rope,
    head: usize,
    insert_like: bool,
) -> Option<(usize, usize)> {
    if let Some((i, v, p, r)) = ui.bracket
        && (i, v, p) == (id, version, head)
    {
        return r;
    }
    let at = |p: usize| motion::matching_bracket(rope, p).map(|q| (p, q));
    let r = at(head).or_else(|| if insert_like && head > 0 { at(head - 1) } else { None });
    ui.bracket = Some((id, version, head, r));
    r
}

/// The word whose other occurrences get highlighted, and where this one starts.
fn word_at_cursor(ed: &Editor) -> Option<(String, usize)> {
    let b = ed.buf();
    let rope = &b.doc.rope;
    let r = b.sel.primary();
    if b.sel.len() > 1 && !ed.mode.is_visual() {
        return None;
    }
    let (a, z) = match ed.mode {
        Mode::Normal if ed.vim() => {
            let h = r.head.min(rope.len_chars());
            if !text::char_at(rope, h).is_some_and(text::is_word_char) {
                return None;
            }
            motion::word_object(rope, h, false, false)
        }
        Mode::Visual => (r.from(), text::grapheme_end(rope, r.to())),
        _ if !ed.vim() && !r.is_empty() => (r.from(), r.to()),
        _ if !ed.vim() => {
            let h = r.head.min(rope.len_chars());
            let p = if text::char_at(rope, h).is_some_and(text::is_word_char) {
                h
            } else if h > 0 && text::char_at(rope, h - 1).is_some_and(text::is_word_char) {
                h - 1
            } else {
                return None;
            };
            motion::word_object(rope, p, false, false)
        }
        _ => return None,
    };
    if z <= a || z - a > 64 {
        return None;
    }
    let w: String = rope.slice(a..z).chars().collect();
    // a whole word: the selection must not cut one
    let bounded = !text::char_at(rope, a.wrapping_sub(1)).is_some_and(text::is_word_char) || a == 0;
    let bounded = bounded && !text::char_at(rope, z).is_some_and(text::is_word_char);
    (bounded && w.chars().all(text::is_word_char)).then_some((w, a))
}

/// Whole-word occurrences of `word` on `line`, as char ranges.
fn word_matches(rope: &Rope, line: usize, word: &str) -> Vec<(usize, usize)> {
    let slice = rope.line(line);
    if slice.len_bytes() > 10_000 {
        return Vec::new();
    }
    let s: std::borrow::Cow<str> = slice.into();
    let ls = text::line_start(rope, line);
    let n = word.chars().count();
    let mut out = Vec::new();
    for (byte, _) in s.match_indices(word) {
        let before = s[..byte].chars().next_back();
        let after = s[byte + word.len()..].chars().next();
        if before.is_some_and(text::is_word_char) || after.is_some_and(text::is_word_char) {
            continue;
        }
        let a = ls + s[..byte].chars().count();
        out.push((a, a + n));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn indent_guides(
    scene: &mut Scene,
    rope: &Rope,
    first: usize,
    end: usize,
    cur: usize,
    tw: usize,
    lay: &Layout,
    scroll: Scroll,
    c: &Colors,
) {
    if end <= first {
        return;
    }
    let last = text::last_line(rope);
    // indent of each visible line; blank lines take the smaller of their neighbours'
    let indent = |l: usize| -> Option<usize> {
        let mut col = 0;
        for ch in rope.line(l).chars() {
            match ch {
                ' ' => col += 1,
                '\t' => col += tw - col % tw,
                '\n' | '\r' => return None,
                _ => return Some(col),
            }
        }
        None
    };
    let lo = first.saturating_sub(64);
    let hi = (end + 64).min(last + 1);
    let raw: Vec<Option<usize>> = (lo..hi).map(indent).collect();
    let mut levels = vec![0usize; raw.len()];
    let mut prev = None;
    let mut above = vec![None; raw.len()];
    for (i, r) in raw.iter().enumerate() {
        if r.is_some() {
            prev = *r;
        }
        above[i] = prev;
    }
    let mut next = None;
    for i in (0..raw.len()).rev() {
        if raw[i].is_some() {
            next = raw[i];
        }
        levels[i] = raw[i].unwrap_or_else(|| above[i].unwrap_or(0).min(next.unwrap_or(0)));
    }
    let level = |l: usize| levels[l - lo];
    // the guide of the block the cursor is in
    let active = (lo..hi).contains(&cur).then(|| level(cur)).filter(|&i| i > 0).map(|i| (i - 1) / tw * tw);
    let (mut act_a, mut act_z) = (cur, cur);
    if let Some(g) = active {
        while act_a > lo && level(act_a - 1) > g {
            act_a -= 1;
        }
        while act_z + 1 < hi && level(act_z + 1) > g {
            act_z += 1;
        }
    }
    let w = lay.scale.round().max(1.0);
    for l in first..end.min(hi) {
        let y = scroll.line_y(lay, l);
        let mut g = 0;
        while g < level(l) {
            let x = scroll.col_x(lay, g as f32).round();
            let on = active == Some(g) && (act_a..=act_z).contains(&l);
            scene.rect(Rect::new(x, y, w, lay.ch), if on { c.indent_guide_active } else { c.indent_guide });
            g += tw;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cursors(
    scene: &mut Scene,
    ed: &Editor,
    ui: &mut Ui,
    lay: &Layout,
    c: &Colors,
    scroll: Scroll,
    first: usize,
    end: usize,
    vis_a: usize,
    vis_z: usize,
) {
    let b = ed.buf();
    let rope = &b.doc.rope;
    let tw = ed.settings.tab_width.max(1);
    let shape = ed.cursor_shape();
    let hollow = !ui.focused || ed.prompt.is_some() || ed.picker.is_some() || ed.about.is_some();
    if ui.blink_off && !hollow {
        return;
    }
    let prim = b.sel.primary_index();
    let ranges = b.sel.ranges();
    let start = ranges.partition_point(|r| r.to() < vis_a);
    let bar = (2.0 * lay.scale).round().max(1.0);
    for (i, r) in ranges.iter().enumerate().skip(start).take_while(|(_, r)| r.from() <= vis_z) {
        let h = r.head.min(rope.len_chars());
        let line = text::line_of(rope, h);
        let (y, x) = if i == prim {
            // the animated position
            let (l, col) = ui.anim.cursor;
            let y = lay.text.y - scroll.y_offset(lay.ch) + (l - scroll.first_line() as f32) * lay.ch;
            (y.round(), scroll.col_x(lay, col))
        } else {
            if !(first..end).contains(&line) {
                continue;
            }
            (scroll.line_y(lay, line), scroll.col_x(lay, text::col_of(rope, h, tw) as f32))
        };
        let ch = text::char_at(rope, h).filter(|&ch| ch != '\n');
        let cells = match ch {
            Some('\t') | None => 1,
            Some(ch) => char_columns(ch).max(1),
        };
        let block = Rect::new(x, y, cells as f32 * lay.cw, lay.ch);
        let rect = match shape {
            CursorShape::Block => block,
            CursorShape::Bar => Rect::new(x.round(), y, bar, lay.ch),
            CursorShape::Underline => Rect::new(x, y + lay.ch - bar, block.w, bar),
        };
        if i == prim {
            ui.cursor_rect = Some(block);
        }
        let color = if i == prim { c.cursor } else { mix(c.cursor, c.background, 0.75) };
        if hollow {
            scene.rounded_rect(block, 0.0, Color::TRANSPARENT, Some((lay.scale.round().max(1.0), color)));
            continue;
        }
        scene.rect(rect, color);
        // the glyph under a block cursor, redrawn in the cursor-text color (clipped, so it follows the glide)
        if shape == CursorShape::Block
            && let Some(ch) = ch.filter(|&ch| ch != '\t' && !ch.is_control())
            && (first..end).contains(&line)
        {
            let gx = scroll.col_x(lay, text::col_of(rope, h, tw) as f32);
            let mut s = [0u8; 4];
            scene.push_clip(rect);
            scene.text(gx, scroll.line_y(lay, line), ch.encode_utf8(&mut s), TextStyle::new(c.cursor_text), &[]);
            scene.pop_clip();
        }
    }
}

fn scrollbar(scene: &mut Scene, ui: &mut Ui, lay: &Layout, c: &Colors, scroll: Scroll, total: usize) {
    if total <= lay.rows {
        return;
    }
    let track = lay.scrollbar;
    let min_h = (24.0 * lay.scale).round();
    let h = (track.h * lay.rows as f32 / (total + lay.rows) as f32).max(min_h).min(track.h);
    let frac = (scroll.top / (total.saturating_sub(1).max(1)) as f32).clamp(0.0, 1.0);
    let y = (track.y + frac * (track.h - h)).round();
    let w = (4.0 * lay.scale).round().max(2.0);
    let x = track.right() - w - (2.0 * lay.scale).round();
    scene.rounded_rect(Rect::new(x, y, w, h), w / 2.0, c.scrollbar, None);
    ui.hits.scrollbar = Some((track, total));
}

fn welcome(scene: &mut Scene, ed: &Editor, lay: &Layout, c: &Colors) {
    let vim = ed.vim();
    let title = format!("shrew {}", env!("CARGO_PKG_VERSION"));
    let mut lines: Vec<(&str, &str)> = vec![("Ctrl+P", "open a file"), ("Ctrl+Shift+P", "all commands")];
    if vim {
        lines.extend([(":e <file>", "edit a file"), ("i / Esc", "insert / normal mode"), (":q", "quit")]);
    } else {
        lines.extend([("Ctrl+S", "save"), ("Ctrl+Q", "quit")]);
    }
    lines.push((if vim { ":set novim" } else { ":set vim" }, if vim { "Sublime-style editing" } else { "vim keys" }));
    let key_w = lines.iter().map(|l| columns(l.0)).max().unwrap_or(0) + 3;
    let w = key_w + lines.iter().map(|l| columns(l.1)).max().unwrap_or(0);
    let h = lines.len() + 2;
    if w as f32 * lay.cw > lay.text.w || (h as f32 * lay.ch) > lay.text.h {
        return;
    }
    let x0 = (lay.text.x + (lay.text.w - w as f32 * lay.cw) / 2.0).round();
    let y0 = (lay.text.y + (lay.text.h - h as f32 * lay.ch) / 2.5).round();
    let tx = (lay.text.x + (lay.text.w - columns(&title) as f32 * lay.cw) / 2.0).round();
    scene.text(tx, y0, &title, TextStyle::new(c.accent).bold(), &[(5..title.len(), TextStyle::new(c.panel_muted))]);
    for (i, (k, d)) in lines.iter().enumerate() {
        let y = y0 + (i + 2) as f32 * lay.ch;
        scene.text(x0, y, k, TextStyle::new(c.panel_foreground), &[]);
        scene.text(x0 + key_w as f32 * lay.cw, y, d, TextStyle::new(c.panel_muted), &[]);
    }
}

// ----- bars -----

fn tabs(scene: &mut Scene, st: &State, ui: &mut Ui, lay: &Layout, c: &Colors) {
    if lay.tabs.h <= 0.0 {
        return;
    }
    let ed = &st.ed;
    scene.rect(lay.tabs, c.panel_background);
    scene.push_clip(lay.tabs);
    let r = (6.0 * lay.scale).round();
    let mut x = lay.pad;
    // keep the active tab in view
    let widths: Vec<f32> = ed
        .buffers
        .iter()
        .map(|b| (columns(&b.doc.display_name()) as f32 + 4.0 + if b.doc.dirty { 2.0 } else { 0.0 }) * lay.cw)
        .collect();
    let before: f32 = widths[..ed.active].iter().sum();
    let shift = (before + widths[ed.active] + 2.0 * lay.pad - lay.width).max(0.0);
    x -= shift;
    let ty = lay.centered(lay.tabs.y + lay.pad / 2.0, lay.tabs.h);
    for (i, b) in ed.buffers.iter().enumerate() {
        let w = widths[i];
        let tab = Rect::new(x.round(), lay.tabs.y + lay.pad, w.round(), lay.tabs.h - lay.pad);
        let active = i == ed.active;
        if active {
            // rounded on top only: the bottom corners hang below the clip
            scene.rounded_rect(Rect::new(tab.x, tab.y, tab.w, tab.h + r), r, c.background, None);
        }
        let name = b.doc.display_name();
        let fg = if active { c.panel_foreground } else { c.panel_muted };
        let tx = tab.x + 2.0 * lay.cw;
        scene.text(tx.round(), ty, &name, TextStyle::new(fg), &[]);
        if b.doc.dirty {
            scene.text((tx + (columns(&name) + 1) as f32 * lay.cw).round(), ty, "●", TextStyle::new(c.accent), &[]);
        }
        ui.hits.tabs.push((tab, i));
        x += w + lay.pad / 2.0;
    }
    scene.pop_clip();
}

fn status(scene: &mut Scene, st: &State, ui: &mut Ui, lay: &Layout, c: &Colors) {
    let ed = &st.ed;
    let b = ed.buf();
    let bar = lay.status;
    scene.rect(bar, c.panel_background);
    let y = lay.centered(bar.y, bar.h);
    let cw = lay.cw;
    let r = (4.0 * lay.scale).round();

    // right side first (it always shows), laid out from the right edge
    let mut right = bar.right() - lay.pad * 2.0;
    // the Settings gear (Ctrl+,) in the corner
    if ed.features.on(Feature::Settings) {
        let gw = 2.0 * cw;
        right -= gw;
        let open = st.settings.is_open();
        let r = Rect::new((right - cw / 2.0).round(), bar.y + lay.pad, (gw + cw).round(), bar.h - 2.0 * lay.pad);
        if open {
            scene.rounded_rect(r, r.h / 2.0, c.panel_selection, None);
        }
        let x = (right + (gw - columns(GEAR) as f32 * cw) / 2.0).round();
        scene.text(x, y, GEAR, TextStyle::new(if open { c.accent } else { c.panel_muted }), &[]);
        ui.hits.gear = Some(r);
        right -= 2.0 * cw;
    }
    let mut item = |scene: &mut Scene, s: &str, col: Color| {
        right -= columns(s) as f32 * cw;
        scene.text(right.round(), y, s, TextStyle::new(col), &[]);
        right -= 2.0 * cw;
    };
    let rope = b.rope();
    let head = b.sel.primary().head.min(rope.len_chars());
    let line = text::line_of(rope, head);
    let col = b.doc.col_of(head, ed.settings.tab_width.max(1));
    item(scene, &format!("{}:{}", line + 1, col + 1), c.panel_foreground);
    if let Some(lang) = st.language() {
        item(scene, &lang, c.panel_muted);
    }
    let n = b.sel.len();
    let p = b.sel.primary();
    if n > 1 {
        item(scene, &format!("{n} cursors"), c.accent_alt);
    } else if let Some((a, z)) = selection_span(ed.mode, rope, p.from(), p.to(), p.is_empty()) {
        let lines = text::line_of(rope, z.saturating_sub(1).max(a)) - text::line_of(rope, a) + 1;
        let s = if lines > 1 { format!("{lines} lines") } else { format!("{} selected", z - a) };
        item(scene, &s, c.panel_muted);
    }
    if st.mcp_running() {
        item(scene, "◆ MCP", c.accent_alt);
    }
    if let Some((label, tone)) = st.update_label() {
        let w = (columns(&label) as f32 + 2.0) * cw;
        right -= w;
        let pill = Rect::new(right.round(), bar.y + lay.pad, w.round(), bar.h - 2.0 * lay.pad);
        let (bg, fg) = match tone {
            Tone::Accent => (c.accent, c.accent_fg),
            Tone::Success => (c.success, c.success_fg),
            Tone::Error => (c.error, c.error_fg),
            Tone::Muted => (c.panel_selection, c.panel_foreground),
        };
        scene.rounded_rect(pill, pill.h / 2.0, bg, None);
        scene.text((right + cw).round(), y, &label, TextStyle::new(fg).bold(), &[]);
        ui.hits.update = Some(pill);
        right -= 2.0 * cw;
    }

    // left side: mode, file, pending keys, macro; clipped where the right side starts
    scene.push_clip(Rect::new(bar.x, bar.y, (right - bar.x).max(0.0), bar.h));
    let mut x = lay.pad * 2.0;
    if ed.vim() {
        let (label, col, ink) = match ed.mode {
            Mode::Normal => ("NORMAL", c.mode_normal, c.mode_normal_fg),
            Mode::Insert => ("INSERT", c.mode_insert, c.mode_insert_fg),
            m => (m.label(), c.mode_visual, c.mode_visual_fg),
        };
        let w = (columns(label) as f32 + 2.0) * cw;
        let badge = Rect::new(x.round(), bar.y + lay.pad, w.round(), bar.h - 2.0 * lay.pad);
        scene.rounded_rect(badge, r, col, None);
        scene.text((x + cw).round(), y, label, TextStyle::new(ink).bold(), &[]);
        x += w + cw;
    }
    let mut tail = String::new();
    if b.doc.dirty {
        tail.push_str(" ●");
    }
    let pending = format!("{}{}", ed.pending_keys(), keymap::keys_to_string(ed.keymap.held()));
    let rec = ed.recording_macro().map(|reg| format!("● recording @{reg}"));
    let extra = columns(&tail)
        + if pending.is_empty() { 0 } else { columns(&pending) + 2 }
        + rec.as_ref().map_or(0, |r| columns(r) + 2);
    // a long path keeps its end: the file name matters most
    let room = (((right - x) / cw) as usize).saturating_sub(extra + 2);
    let name = match &b.doc.path {
        Some(p) => shrew_core::editor::relative(p, &ed.cwd),
        None => b.doc.display_name(),
    };
    let name = elide_start(&name, room.max(8));
    scene.text(x.round(), y, &name, TextStyle::new(c.panel_foreground), &[]);
    x += columns(&name) as f32 * cw;
    if !tail.is_empty() {
        scene.text((x + cw).round(), y, "●", TextStyle::new(c.accent), &[]);
        x += 2.0 * cw;
    }
    x += 2.0 * cw;
    if !pending.is_empty() {
        scene.text(x.round(), y, &pending, TextStyle::new(c.accent_alt), &[]);
        x += (columns(&pending) + 2) as f32 * cw;
    }
    if let Some(rec) = rec {
        scene.text(x.round(), y, &rec, TextStyle::new(c.warning), &[]);
    }
    scene.pop_clip();
}

/// The status bar's Settings button.
const GEAR: &str = "⚙";

/// `s` cut to its last `cols` columns behind an ellipsis.
pub(crate) fn elide_start(s: &str, cols: usize) -> String {
    if columns(s) <= cols {
        return s.to_string();
    }
    let mut out: Vec<char> = Vec::new();
    let mut w = 1;
    for ch in s.chars().rev() {
        w += char_columns(ch);
        if w > cols {
            break;
        }
        out.push(ch);
    }
    std::iter::once('…').chain(out.into_iter().rev()).collect()
}

fn cmdline(scene: &mut Scene, ed: &Editor, st: &State, lay: &Layout, c: &Colors) {
    let bar = lay.cmdline;
    let y = lay.centered(bar.y, bar.h);
    let x = lay.pad * 2.0;
    scene.push_clip(bar);
    if let Some(p) = &ed.prompt {
        prompt(scene, p, x, y, lay, c);
    } else if let Some((msg, err)) = &ed.message {
        let msg = msg.lines().next().unwrap_or("");
        scene.text(x, y, msg, TextStyle::new(if *err { c.error } else { c.panel_foreground }), &[]);
    } else if !st.loading.is_empty() {
        scene.text(x, y, "loading…", TextStyle::new(c.panel_muted), &[]);
    }
    scene.pop_clip();
}

fn prompt(scene: &mut Scene, p: &Prompt, x: f32, y: f32, lay: &Layout, c: &Colors) {
    let prefix = match p.kind {
        PromptKind::Command => ":",
        PromptKind::Search { forward: true } => "/",
        PromptKind::Search { forward: false } => "?",
    };
    scene.text(x, y, prefix, TextStyle::new(c.accent).bold(), &[]);
    let tx = x + lay.cw;
    // keep the caret in view on a long line
    let before: String = p.text.chars().take(p.cursor).collect();
    let caret_col = columns(&before) as f32;
    let room = ((lay.width - tx) / lay.cw - 2.0).max(1.0);
    let shift = (caret_col - room).max(0.0).ceil();
    scene.text(tx - shift * lay.cw, y, &p.text, TextStyle::new(c.foreground), &[]);
    let bar = (2.0 * lay.scale).round().max(1.0);
    scene.rect(Rect::new((tx + (caret_col - shift) * lay.cw).round(), y, bar, lay.ch), c.cursor);
}

// ----- picker -----

fn picker(scene: &mut Scene, ed: &Editor, ui: &mut Ui, lay: &Layout, c: &Colors) {
    let Some(p) = &ed.picker else { return };
    scene.layer(3);
    let cw = lay.cw;
    let pad = lay.pad;
    let w = (lay.width * 0.6).clamp(60.0 * cw, 100.0 * cw).min(lay.width - 2.0 * cw).round();
    let row_h = (lay.ch + pad).round();
    let query_h = lay.ch + 3.0 * pad;
    let max_rows = (((lay.text.h * 0.75 - query_h) / row_h).floor() as usize).clamp(1, 14);
    let shown = p.matches.len().clamp(1, max_rows);
    let h = (query_h + 1.0 + pad + shown as f32 * row_h + pad).round();
    let x = ((lay.width - w) / 2.0).round();
    let y = (lay.text.y + 3.0 * pad).round();
    let panel = Rect::new(x, y, w, h);
    popup(scene, panel, lay, c);
    ui.hits.picker = Some(panel);
    let inner = Rect::new(x + 2.0 * pad, y, w - 4.0 * pad, h);
    scene.push_clip(inner);

    // query line
    let qy = lay.centered(y, query_h);
    let qx = inner.x + pad;
    scene.text(qx, qy, "›", TextStyle::new(c.accent).bold(), &[]);
    let tx = qx + 2.0 * cw;
    scene.text(tx, qy, &p.query, TextStyle::new(c.panel_foreground), &[]);
    let bar = (2.0 * lay.scale).round().max(1.0);
    scene.rect(Rect::new((tx + columns(&p.query) as f32 * cw).round(), qy, bar, lay.ch), c.cursor);
    let kind = match p.kind {
        PickerKind::Files => "files",
        PickerKind::Commands => "commands",
        PickerKind::Buffers => "buffers",
        PickerKind::Features => "features",
        PickerKind::Choice(choice) => choice.noun(),
    };
    let count = format!("{}{}/{} {kind}", if p.loading { "… " } else { "" }, p.matches.len(), p.items.len());
    let cx = inner.right() - pad - columns(&count) as f32 * cw;
    scene.text(cx.round(), qy, &count, TextStyle::new(c.panel_muted), &[]);
    let sep_y = (y + query_h).round();
    scene.rect(Rect::new(x, sep_y, w, lay.scale.round().max(1.0)), c.panel_border);

    // results
    if p.selected < ui.picker_top {
        ui.picker_top = p.selected;
    } else if p.selected >= ui.picker_top + max_rows {
        ui.picker_top = p.selected + 1 - max_rows;
    }
    ui.picker_top = ui.picker_top.min(p.matches.len().saturating_sub(max_rows));
    let ry0 = sep_y + 1.0 + pad;
    if p.matches.is_empty() {
        let msg = if p.loading { "searching…" } else { "no matches" };
        scene.text(inner.x + pad, lay.centered(ry0, row_h), msg, TextStyle::new(c.panel_muted), &[]);
    }
    let q = p.query.trim();
    let hl = TextStyle::new(c.fuzzy_match).bold();
    let mut spans: Vec<(Range<usize>, TextStyle)> = Vec::new();
    let room = ((inner.w - 2.0 * pad) / cw).max(0.0) as usize;
    let (label_cols, detail_x) = picker_columns(p, room);
    for (row, &mi) in p.matches.iter().enumerate().skip(ui.picker_top).take(max_rows) {
        let item = &p.items[mi as usize];
        let ry = ry0 + (row - ui.picker_top) as f32 * row_h;
        let rect = Rect::new(x + pad, ry, w - 2.0 * pad, row_h);
        if row == p.selected {
            scene.rounded_rect(rect, (5.0 * lay.scale).round(), c.panel_selection, None);
        }
        ui.hits.picker_rows.push((rect, row));
        let ty = lay.centered(ry, row_h);
        spans.clear();
        if !q.is_empty()
            && let Some((_, pos)) = fuzzy_match(q, &item.label)
        {
            let mut it = item.label.char_indices().enumerate();
            for &ci in &pos {
                if let Some((_, (b, ch))) = it.by_ref().find(|(i, _)| *i == ci) {
                    spans.push((b..b + ch.len_utf8(), hl));
                }
            }
        }
        let label_x = inner.x + pad;
        let fg = if row == p.selected { c.panel_foreground } else { mix(c.panel_foreground, c.panel_muted, 0.85) };
        let label = elide_end(&item.label, label_cols);
        // matches past the cut are gone with it
        let cut = matches!(label, std::borrow::Cow::Owned(_));
        let kept = if cut { label.len() - '…'.len_utf8() } else { label.len() };
        spans.retain(|(r, _)| r.end <= kept);
        scene.text(label_x, ty, &label, TextStyle::new(fg), &spans);
        // details all start at one column, after the longest label
        if let Some(dcol) = detail_x
            && !item.detail.is_empty()
        {
            let detail = elide_end(&item.detail, room - dcol);
            scene.text(label_x + dcol as f32 * cw, ty, &detail, TextStyle::new(c.panel_muted), &[]);
        }
    }
    scene.pop_clip();
    scene.layer(0);
}

/// A floating panel with a soft shadow, as the picker and the About panel use.
pub(crate) fn popup(scene: &mut Scene, panel: Rect, lay: &Layout, c: &Colors) {
    let Rect { x, y, w, h } = panel;
    let radius = (8.0 * lay.scale).round();
    for i in 1..=4 {
        let s = i as f32 * 2.0 * lay.scale;
        let a = if c.dark { 34 } else { 14 };
        scene.rounded_rect(
            Rect::new(x - s, y - s + 2.0 * s, w + 2.0 * s, h + 2.0 * s),
            radius + s,
            Color::rgba(0, 0, 0, a),
            None,
        );
    }
    scene.rounded_rect(panel, radius, c.popup, Some((lay.scale.round().max(1.0), c.popup_border)));
}

// ----- about -----

/// The About panel: version, manifesto, license, credits (see [`crate::about`]), centred over the text and scrolled
/// by `ed.about`.
fn about(scene: &mut Scene, ed: &mut Editor, ui: &mut Ui, lay: &Layout, c: &Colors) {
    let Some(top) = ed.about else { return };
    scene.layer(2);
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let w = ((76.0 * cw).min(lay.width - 2.0 * cw)).round().max(10.0 * cw);
    let cols = ((w - 7.0 * cw) / cw).floor().max(1.0) as usize;
    let lines = crate::about::lines(cols);
    let footer_h = ch + 3.0 * pad;
    let room = ((lay.text.h - 4.0 * pad - footer_h - 2.0 * ch) / ch).floor().max(1.0) as usize;
    let shown = lines.len().min(room);
    let top = top.min(lines.len() - shown);
    ed.about = Some(top);
    let h = (shown as f32 * ch + 2.0 * ch + footer_h).round();
    let x = ((lay.width - w) / 2.0).round();
    let y = (lay.text.y + ((lay.text.h - h) / 2.0).max(2.0 * pad)).round();
    let panel = Rect::new(x, y, w, h);
    popup(scene, panel, lay, c);
    ui.hits.about = Some(panel);

    let ink = |i: crate::about::Ink| {
        use crate::about::Ink;
        match i {
            Ink::Text => TextStyle::new(c.panel_foreground),
            Ink::Muted => TextStyle::new(c.panel_muted),
            Ink::Strong => TextStyle::new(c.panel_foreground).bold(),
            Ink::Accent => TextStyle::new(c.accent).bold(),
        }
    };
    let tx = x + 3.0 * cw;
    let body = Rect::new(x, y + ch, w, shown as f32 * ch);
    scene.push_clip(body);
    let mut spans = Vec::new();
    for (i, l) in lines.iter().skip(top).take(shown).enumerate() {
        let Some(base) = l.ink else { continue };
        spans.clear();
        spans.extend(l.spans.iter().map(|(r, k)| (r.clone(), ink(*k))));
        scene.text(tx, (body.y + i as f32 * ch).round(), &l.text, ink(base), &spans);
    }
    scene.pop_clip();
    // a thin scrollbar when it does not all fit
    if shown < lines.len() {
        let bw = (3.0 * lay.scale).round().max(2.0);
        let th = (body.h * shown as f32 / lines.len() as f32).max(ch);
        let ty = body.y + (body.h - th) * top as f32 / (lines.len() - shown) as f32;
        scene.rounded_rect(
            Rect::new(panel.right() - 2.0 * cw, ty.round(), bw, th.round()),
            bw / 2.0,
            c.scrollbar,
            None,
        );
    }
    let sep_y = (body.bottom() + ch).round();
    scene.rect(Rect::new(x, sep_y, w, lay.scale.round().max(1.0)), c.panel_border);
    let hint = if shown < lines.len() { "j/k scroll   Esc close" } else { "Esc close" };
    let fy = lay.centered(sep_y, footer_h);
    let hx = panel.right() - 3.0 * cw - columns(hint) as f32 * cw;
    let repo = crate::about::LINKS[0].1;
    if tx + (columns(repo) + 2) as f32 * cw < hx {
        scene.text(tx, fy, repo, TextStyle::new(c.panel_muted), &[]);
    }
    scene.text(hx.round(), fy, hint, TextStyle::new(c.panel_muted), &[]);
    scene.layer(0);
}

/// Gap between a picker's label and detail columns.
const DETAIL_GAP: usize = 3;

/// A picker's label width and the column its details start at (none when no item has one or there is no room), out
/// of `room` columns. The column follows the longest label (of every item, so it holds still while filtering), up
/// to half the row; longer labels are cut.
fn picker_columns(p: &shrew_core::picker::Picker, room: usize) -> (usize, Option<usize>) {
    // file lists can be huge and have no details
    if p.kind == PickerKind::Files || p.items.iter().all(|i| i.detail.is_empty()) {
        return (room, None);
    }
    let longest = p.items.iter().map(|i| columns(&i.label)).max().unwrap_or(0);
    let label = longest.min(room / 2);
    let dcol = label + DETAIL_GAP;
    if dcol + 4 > room { (room, None) } else { (label, Some(dcol)) }
}

/// `s` cut to `cols` columns, ending in an ellipsis when cut.
pub(crate) fn elide_end(s: &str, cols: usize) -> std::borrow::Cow<'_, str> {
    if columns(s) <= cols {
        return s.into();
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
    out.into()
}

fn human_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, left: usize, cols: usize, ws: bool) -> (String, Vec<u32>, usize) {
        let rope = Rope::from_str(text);
        let mut lb = LineBuf::default();
        let start = layout_line(&mut lb, &rope, 0, 4, left, cols, ws);
        (lb.disp, lb.map, start)
    }

    #[test]
    fn tabs_expand_and_bytes_map() {
        let (d, map, start) = line("\tfn é()\n", 0, 80, false);
        assert_eq!(d, "    fn é()");
        assert_eq!(start, 0);
        assert_eq!(map[0], 0); // the tab
        assert_eq!(map[1], 4); // 'f'
        assert_eq!(map[4], 7); // 'é' (2 bytes)
        assert_eq!(map[5], 7);
        assert_eq!(map[6], 9); // '('
        let (d, _, _) = line("a\tb", 0, 80, false);
        assert_eq!(d, "a   b");
    }

    #[test]
    fn horizontal_scroll_cuts_the_line() {
        let (d, map, start) = line("0123456789", 3, 4, false);
        assert_eq!(start, 3);
        assert_eq!(d, "3456");
        assert_eq!(map[0], 0);
        assert_eq!(map[3], 0);
        assert_eq!(map[5], 2);
        // a wide char straddling the left edge starts the text one column early
        let (d, _, start) = line("a日本", 2, 10, false);
        assert_eq!((d.as_str(), start), ("日本", 1));
    }

    #[test]
    fn whitespace_and_control_chars_are_visible() {
        let (d, _, _) = line("\tx  \n", 0, 80, true);
        assert_eq!(d, "→   x··");
        let (d, _, _) = line("a\u{0}b\u{7f}", 0, 80, false);
        assert_eq!(d, "a␀b␡");
    }

    #[test]
    fn spans_cover_line_pieces() {
        let rope = Rope::from_str("abc\ndef\nghi\n");
        let mut out = Vec::new();
        span_rects(&rope, 1, 6, 0, 3, 4, &mut |l, a, z| out.push((l, a, z)));
        // "bc" plus the line break, then "de"
        assert_eq!(out, vec![(0, 1, 4), (1, 0, 2)]);
        out.clear();
        span_rects(&rope, 1, 6, 1, 3, 4, &mut |l, a, z| out.push((l, a, z)));
        assert_eq!(out, vec![(1, 0, 2)], "lines above the view are skipped");
    }

    #[test]
    fn long_paths_keep_their_end() {
        assert_eq!(elide_start("src/main.rs", 20), "src/main.rs");
        assert_eq!(elide_start("crates/shrew/src/main.rs", 10), "…c/main.rs");
        assert_eq!(columns(&elide_start("日本語/日本語.rs", 7)), 6);
    }

    #[test]
    fn long_labels_lose_their_end() {
        assert_eq!(elide_end("Toggle line numbers", 30), "Toggle line numbers");
        assert_eq!(elide_end("Toggle line numbers", 8), "Toggle …");
        assert_eq!(columns(&elide_end("日本語の名前", 6)), 5);
    }

    #[test]
    fn picker_details_share_one_column() {
        use shrew_core::picker::{Picker, PickerData, PickerItem};
        let item = |label: &str, detail: &str| PickerItem {
            label: label.into(),
            detail: detail.into(),
            data: PickerData::Buffer(0),
        };
        let p = Picker::new(PickerKind::Buffers, vec![item("a.rs", "src/a.rs"), item("longer.rs", "")]);
        assert_eq!(picker_columns(&p, 80), (9, Some(12)));
        // a very long label takes at most half the row
        let p = Picker::new(PickerKind::Buffers, vec![item(&"x".repeat(100), "d")]);
        assert_eq!(picker_columns(&p, 80), (40, Some(43)));
        // no details: the label gets the row
        let p = Picker::new(PickerKind::Buffers, vec![item("a.rs", "")]);
        assert_eq!(picker_columns(&p, 80), (80, None));
    }

    #[test]
    fn whole_word_matches_only() {
        let rope = Rope::from_str("foo food foo_bar (foo)\n");
        assert_eq!(word_matches(&rope, 0, "foo"), vec![(0, 3), (18, 21)]);
    }

    #[test]
    fn visual_line_spans_whole_lines() {
        let rope = Rope::from_str("abc\ndef\nghi");
        assert_eq!(selection_span(Mode::VisualLine, &rope, 5, 5, true), Some((4, 8)));
        assert_eq!(selection_span(Mode::Visual, &rope, 1, 2, false), Some((1, 3)));
        assert_eq!(selection_span(Mode::Insert, &rope, 2, 2, true), None);
    }
}
