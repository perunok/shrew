//! The buffer tabs: their row (or the middle of the custom title bar), drawn and handled with the mouse.
//!
//! - Layout: every tab has a width between [`MIN_LABEL`] and [`MAX_LABEL`] columns of label (longer names end in
//!   `…`), plus room for its close button. When they do not fit, the strip scrolls (the wheel, either direction,
//!   smoothly), fades at a clipped edge and shows a tab list button (the buffer picker). The active tab is kept in
//!   view whenever it changes. Only what is visible has a hit rect.
//! - Look: the active tab has the editor's background and an accent line on top; a hovered one is highlighted. The
//!   active and the hovered tab show a close button; an unsaved tab shows a dot there, which turns into the close
//!   button under the pointer. Two files of the same name show the shortest parent path that tells them apart, in
//!   muted text (`mod.rs · git/log`).
//! - Mouse: a press switches to a tab; the middle button or the close button closes it (an unsaved buffer asks to
//!   save, discard or cancel on the command line: shrew-core's `tabs`); dragging a tab along the row reorders it
//!   (Esc cancels); a double click on the empty part of a row of its own opens a scratch buffer (in the custom title
//!   bar the empty part moves and maximizes the window instead); a right click opens [`menu`].
//!
//! Cost: labels are recomputed only when a buffer's name changes; a frame fills reused vectors; hover changes
//! redraw once; the strip animates (redraws continuously) only while it scrolls.

use crate::colors::{Colors, mix};
use crate::context_menu::{Action, AppFn, ContextMenu, MenuItem, Run};
use crate::draw::{Ui, elide_end, elide_start, line};
use crate::layout::Layout;
use crate::mouse::Target;
use crate::state::State;
use shrew_core::document::Buffer;
use shrew_core::features::Feature;
use shrew_core::{Command, Key, KeyPress};
use shrew_render::{Color, Rect, Scene, TextStyle, columns};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Label columns of a tab: at least this many (short names get a comfortable tab), at most this many (`…`).
pub const MIN_LABEL: usize = 5;
pub const MAX_LABEL: usize = 28;
/// Columns around the label: padding on the left, the close button's slot and padding on the right.
const PAD_LEFT: f32 = 1.5;
const CLOSE_SLOT: f32 = 2.5;
const PAD_RIGHT: f32 = 0.5;
/// Between a name and the parent path that tells it apart.
const DIR_SEP: &str = " · ";
/// One wheel notch scrolls the strip this many columns.
const WHEEL_COLS: f32 = 8.0;
/// The fade at a clipped edge (logical pixels) and the scroll ease's time constant.
const FADE: f32 = 28.0;
const SCROLL_TAU: f32 = 0.05;

/// One buffer's label, kept while its name stays the same.
#[derive(Clone, Debug, Default)]
struct Label {
    id: u64,
    path: Option<PathBuf>,
    title: Option<String>,
    /// The name and the parent path that tells it apart from a namesake (empty when it has none), both already cut
    /// to [`MAX_LABEL`].
    name: String,
    dir: String,
    /// Columns of the whole label.
    cols: usize,
}

/// A tab as drawn in the last frame (all of it, even the part scrolled out of view).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub index: usize,
    pub rect: Rect,
    pub close: Rect,
}

/// What the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hover {
    /// A tab (its index), and whether its close button.
    Tab(usize, bool),
    /// The tab list button.
    List,
}

/// A tab held with the left button: it moves once the pointer has gone [`crate::mouse::DRAG_SLOP`].
#[derive(Clone, Copy, Debug)]
struct Drag {
    id: u64,
    from: (f32, f32),
    /// Where the pointer is and where it holds the tab (from the tab's left edge).
    x: f32,
    grab: f32,
    moving: bool,
    /// The position the tab goes to when dropped (an index into the row without it).
    to: usize,
}

/// The tab row's state between frames.
#[derive(Default)]
pub struct Strip {
    /// Scroll offset drawn and the one it eases to (pixels).
    offset: f32,
    target: f32,
    last_frame: Option<Instant>,
    /// (active buffer, its index, buffer count, visible width) when the active tab was last brought into view.
    seen: Option<(u64, usize, usize, u32)>,
    labels: Vec<Label>,
    /// Each tab's width and left edge (from the content's start, which is the strip's left edge unscrolled).
    widths: Vec<f32>,
    lefts: Vec<f32>,
    /// This frame's tabs, the visible width (without the list button) and the list button.
    pub placed: Vec<Placed>,
    pub area: Rect,
    pub view: Rect,
    pub list: Option<Rect>,
    pub hover: Option<Hover>,
    /// The close button pressed (its buffer): it closes when released over it.
    pressed_close: Option<u64>,
    drag: Option<Drag>,
    last_click: Option<(Instant, (f32, f32))>,
    /// Still scrolling: the frame asks for another.
    pub animating: bool,
}

impl Strip {
    /// Forgets the last frame's tabs: a frame that shows none (one buffer, no tab item in the title bar) leaves no
    /// hit rects behind.
    pub fn begin_frame(&mut self) {
        self.placed.clear();
        self.list = None;
        self.animating = false;
        self.area = Rect::default();
        self.view = Rect::default();
    }

    /// The tab being dragged along the row, once it moved (its buffer id and where it would drop).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dragging(&self) -> Option<(u64, usize)> {
        self.drag.filter(|d| d.moving).map(|d| (d.id, d.to))
    }

    /// Brings the labels up to date with the buffers (only when a name changed).
    fn sync_labels(&mut self, buffers: &[Buffer]) {
        let same = self.labels.len() == buffers.len()
            && self
                .labels
                .iter()
                .zip(buffers)
                .all(|(l, b)| l.id == b.id && l.path == b.doc.path && l.title == b.doc.title);
        if !same {
            self.labels = labels(buffers);
        }
    }
}

/// Labels for `buffers`: display names, and for namesakes the shortest parent path that tells them apart.
fn labels(buffers: &[Buffer]) -> Vec<Label> {
    let names: Vec<String> = buffers.iter().map(|b| b.doc.display_name()).collect();
    let files: Vec<Option<&Path>> =
        buffers.iter().map(|b| b.doc.path.as_deref().filter(|_| b.doc.title.is_none())).collect();
    let dirs = disambiguate(&names, &files);
    buffers
        .iter()
        .zip(names)
        .zip(dirs)
        .map(|((b, name), dir)| {
            let name = elide_end(&name, MAX_LABEL).into_owned();
            let room = MAX_LABEL.saturating_sub(columns(&name) + columns(DIR_SEP));
            // the end of the path is the part that differs
            let dir = if dir.is_empty() || room < 4 { String::new() } else { elide_start(&dir, room) };
            let cols = columns(&name) + if dir.is_empty() { 0 } else { columns(DIR_SEP) + columns(&dir) };
            Label { id: b.id, path: b.doc.path.clone(), title: b.doc.title.clone(), name, dir, cols }
        })
        .collect()
}

/// For each tab, the parent path that tells its file apart from others of the same name (`git/log` for
/// `.../git/log/mod.rs` next to `.../git/diffview/mod.rs`), or "" when its name is unique. `files[i]` is `None` for
/// a tab that is not a file (a scratch buffer, a diff).
pub fn disambiguate(names: &[String], files: &[Option<&Path>]) -> Vec<String> {
    let parents = |p: &Path| -> Vec<String> {
        let mut v: Vec<String> = p
            .parent()
            .map(|d| d.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        v.retain(|c| c != "/" && c != "\\");
        v
    };
    // the last `k` folders, outermost first
    let suffix = |dirs: &[String], k: usize| dirs[dirs.len().saturating_sub(k)..].join("/");
    let mut out = vec![String::new(); names.len()];
    for (i, f) in files.iter().enumerate() {
        let Some(p) = f else { continue };
        let twins: Vec<Vec<String>> = files
            .iter()
            .enumerate()
            .filter(|&(j, q)| j != i && names[j] == names[i] && q.is_some_and(|q| q != *p))
            .filter_map(|(_, q)| q.map(parents))
            .collect();
        if twins.is_empty() {
            continue;
        }
        let mine = parents(p);
        let k = (1..=mine.len()).find(|&k| twins.iter().all(|t| suffix(t, k) != suffix(&mine, k)));
        out[i] = suffix(&mine, k.unwrap_or(mine.len()));
    }
    out
}

/// A tab's width for a label of `cols` columns.
pub fn tab_width(cols: usize, cw: f32) -> f32 {
    ((PAD_LEFT + cols.clamp(MIN_LABEL, MAX_LABEL) as f32 + CLOSE_SLOT + PAD_RIGHT) * cw).round()
}

/// The gap between tabs and the room before the first one.
fn gap(lay: &Layout) -> f32 {
    (2.0 * lay.scale).round().max(1.0)
}

/// Left edges of the tabs (from the content's start) and the content's whole width, for tabs `widths` wide.
pub fn offsets(widths: &[f32], lead: f32, gap: f32, out: &mut Vec<f32>) -> f32 {
    out.clear();
    let mut x = lead;
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            x += gap;
        }
        out.push(x);
        x += w;
    }
    x + lead
}

/// The scroll offset that shows `[a, b)` (content pixels) whole in a `view` px wide window at `offset`, with
/// `margin` to spare, within `[0, max]`.
pub fn reveal(offset: f32, a: f32, b: f32, view: f32, margin: f32, max: f32) -> f32 {
    let mut o = offset;
    if b + margin > o + view {
        o = b + margin - view;
    }
    if a - margin < o {
        o = a - margin;
    }
    o.clamp(0.0, max.max(0.0))
}

fn approach(x: &mut f32, target: f32, dt: f32) {
    *x += (target - *x) * (1.0 - (-dt / SCROLL_TAU).exp());
    if (target - *x).abs() < 0.5 {
        *x = target;
    }
}

// ----- drawing -----

/// Draws the tabs in `area` (their own row, or the room the custom title bar leaves them) and records where they
/// went.
pub fn draw(scene: &mut Scene, st: &mut State, ui: &mut Ui, lay: &Layout, c: &Colors, area: Rect) {
    let strip = &mut st.tabs;
    strip.begin_frame();
    strip.area = area;
    if area.h <= 0.0 || area.w <= 0.0 {
        return;
    }
    let ed = &st.ed;
    strip.sync_labels(&ed.buffers);
    scene.rect(area, c.panel_background);
    let (cw, s) = (lay.cw, lay.scale);
    let (lead, gap) = (lay.pad, gap(lay));
    strip.widths.clear();
    strip.widths.extend(strip.labels.iter().map(|l| tab_width(l.cols, cw)));
    let content = offsets(&strip.widths, lead, gap, &mut strip.lefts);
    let lefts = &strip.lefts;
    // too many: the strip scrolls and a button lists them all
    let overflow = content > area.w + 0.5;
    let list_w = if overflow { (area.h * 0.9).round().min(area.w / 3.0) } else { 0.0 };
    let view = Rect::new(area.x, area.y, (area.w - list_w).max(0.0), area.h);
    strip.view = view;
    let max = (content - view.w).max(0.0);
    let fade = (FADE * s).round().min(view.w / 4.0);
    // the active tab comes into view when it (or the row) changed
    let n = ed.buffers.len();
    let seen = (ed.buf().id, ed.active, n, view.w as u32);
    if strip.seen != Some(seen) && strip.drag.is_none_or(|d| !d.moving) {
        strip.seen = Some(seen);
        let (a, w) = (lefts[ed.active], strip.widths[ed.active]);
        // clear of the fade, with a little of the next tab showing past it
        strip.target = reveal(strip.target, a, a + w, view.w, fade + cw, max);
    }
    strip.target = strip.target.clamp(0.0, max);
    let now = Instant::now();
    let dt = strip.last_frame.map_or(1.0 / 120.0, |t| now.saturating_duration_since(t).as_secs_f32()).min(1.0 / 30.0);
    strip.last_frame = Some(now);
    if ui.animate && ed.features.on(Feature::SmoothScroll) {
        approach(&mut strip.offset, strip.target, dt);
    } else {
        strip.offset = strip.target;
    }
    strip.offset = strip.offset.clamp(0.0, max);
    strip.animating = strip.offset != strip.target;
    let origin = view.x - strip.offset.round();

    // a right-clicked tab looks hovered while its menu is open
    let menu_tab = match st.menu.as_ref().map(|m| m.target) {
        Some(Target::Tab(i)) => Some(i),
        _ => None,
    };
    let hover = strip.hover;
    let drag = strip.drag.filter(|d| d.moving);
    let dragged = drag.and_then(|d| ed.buffer_index(d.id));
    let r = (6.0 * s).round();
    let line_w = (2.0 * s).round().max(1.0);
    scene.push_clip(view);
    for (i, b) in ed.buffers.iter().enumerate() {
        let x = origin + lefts[i];
        let w = strip.widths[i];
        let tab = Rect::new(x.round(), area.y + lay.pad, w, area.h - lay.pad);
        if tab.right() < view.x || tab.x > view.right() {
            continue; // scrolled away: not drawn, no hit rect
        }
        let active = i == ed.active;
        let hovered = matches!(hover, Some(Hover::Tab(h, _)) if h == i) || menu_tab == Some(i);
        let close = close_rect(tab, lay);
        if Some(i) == dragged {
            // its place while it is carried: an outline
            let ghost = Rect::new(tab.x, tab.y, tab.w, tab.h + r);
            scene.rounded_rect(ghost, r, Color::TRANSPARENT, Some((s.round().max(1.0), c.panel_border)));
        } else {
            draw_tab(scene, b, &strip.labels[i], tab, close, active, hovered, hover, lay, c);
            // a hairline between two plain tabs
            let next_plain = i + 1 < n && i + 1 != ed.active && !matches!(hover, Some(Hover::Tab(h, _)) if h == i + 1);
            if !active && !hovered && next_plain {
                let hgt = (tab.h * 0.45).round();
                let sep = Rect::new(tab.right() + (gap / 2.0).floor(), tab.y + (tab.h - hgt) / 2.0, 1.0, hgt);
                scene.rect(sep, c.panel_border);
            }
        }
        let vis = tab.intersect(&view);
        if vis.w > 0.0 {
            ui.hits.tabs.push((vis, i));
            strip.placed.push(Placed { index: i, rect: tab, close: close.intersect(&view) });
        }
    }
    // the dragged tab under the pointer, over the others; a line where it would go
    if let (Some(d), Some(i)) = (drag, dragged) {
        scene.layer(1);
        let mut others = (0..n).filter(|&j| j != i);
        let at = match others.clone().nth(d.to) {
            Some(j) => origin + lefts[j] - gap / 2.0,
            None => others.next_back().map_or(origin + lead, |j| origin + lefts[j] + strip.widths[j] + gap / 2.0),
        };
        let w = strip.widths[i];
        let x = (d.x - d.grab).clamp(view.x, (view.right() - w).max(view.x));
        let tab = Rect::new(x.round(), area.y + lay.pad, w, area.h - lay.pad);
        let lifted = Rect::new(tab.x, tab.y, tab.w, tab.h + r);
        // see-through, so the tabs it passes over (and the drop line) show
        let bg = mix(c.panel_selection, c.background, 0.5).with_alpha(215);
        scene.rounded_rect(lifted, r, bg, Some((1.0, c.panel_border)));
        let b = &ed.buffers[i];
        draw_label(scene, b, &strip.labels[i], tab, i == ed.active, true, lay, c);
        scene.layer(2);
        let mark = Rect::new((at - line_w / 2.0).round(), area.y + lay.pad, line_w, area.h - lay.pad);
        scene.rect(mark, c.accent);
        scene.layer(0);
    }
    // fades where tabs go on past the edge
    if overflow {
        scene.layer(1);
        if strip.offset > 0.5 {
            fade_edge(scene, Rect::new(view.x, area.y, fade, area.h), true, c.panel_background);
        }
        if strip.offset < max - 0.5 {
            fade_edge(scene, Rect::new(view.right() - fade, area.y, fade, area.h), false, c.panel_background);
        }
        scene.layer(0);
    }
    scene.pop_clip();
    if overflow {
        let b = Rect::new(view.right(), area.y + lay.pad, list_w, area.h - lay.pad);
        list_button(scene, b, hover == Some(Hover::List) || st.ed.picker.is_some(), lay, c);
        strip.list = Some(b);
        ui.hits.tab_list = Some(b);
    }
}

/// The close button's square at the right of `tab`.
fn close_rect(tab: Rect, lay: &Layout) -> Rect {
    let size = (lay.ch * 0.95).round().min(tab.h - 2.0);
    let cx = tab.right() - (PAD_RIGHT + CLOSE_SLOT / 2.0) * lay.cw;
    let cy = tab.y + tab.h / 2.0;
    Rect::new((cx - size / 2.0).round(), (cy - size / 2.0).round(), size, size)
}

#[allow(clippy::too_many_arguments)]
fn draw_tab(
    scene: &mut Scene,
    b: &Buffer,
    label: &Label,
    tab: Rect,
    close: Rect,
    active: bool,
    hovered: bool,
    hover: Option<Hover>,
    lay: &Layout,
    c: &Colors,
) {
    let s = lay.scale;
    let r = (6.0 * s).round();
    // rounded on top only: the bottom corners hang below the clip
    let body = Rect::new(tab.x, tab.y, tab.w, tab.h + r);
    if active {
        scene.rounded_rect(body, r, c.background, None);
        // the accent along its top edge, inside the rounding
        let t = (2.0 * s).round().max(1.0);
        scene.rounded_rect(Rect::new(tab.x + r * 0.6, tab.y, tab.w - r * 1.2, t), t / 2.0, c.accent, None);
    } else if hovered {
        scene.rounded_rect(body, r, mix(c.panel_selection, c.panel_background, 0.55), None);
    }
    draw_label(scene, b, label, tab, active, hovered, lay, c);
    // the close button on the active and the hovered tab; an unsaved one shows a dot there until hovered
    let on_close = matches!(hover, Some(Hover::Tab(_, true))) && hovered;
    let (cx, cy) = (close.x + close.w / 2.0, close.y + close.h / 2.0);
    if b.doc.dirty && !hovered {
        let d = (7.0 * s).round();
        let dot = Rect::new((cx - d / 2.0).round(), (cy - d / 2.0).round(), d, d);
        scene.rounded_rect(dot, d / 2.0, if active { c.accent } else { mix(c.accent, c.panel_muted, 0.6) }, None);
    } else if active || hovered {
        if on_close {
            scene.rounded_rect(close, (4.0 * s).round(), c.panel_selection, None);
        }
        let k = 3.6 * s;
        let ink = if on_close || active { c.panel_foreground } else { c.panel_muted };
        line(scene, (cx - k, cy - k), (cx + k, cy + k), 1.3 * s, ink);
        line(scene, (cx - k, cy + k), (cx + k, cy - k), 1.3 * s, ink);
    }
}

/// The name (and the muted parent path of a namesake), clipped to the tab left of its close button.
#[allow(clippy::too_many_arguments)]
fn draw_label(
    scene: &mut Scene,
    b: &Buffer,
    l: &Label,
    tab: Rect,
    active: bool,
    hovered: bool,
    lay: &Layout,
    c: &Colors,
) {
    let cw = lay.cw;
    let tx = (tab.x + PAD_LEFT * cw).round();
    let ty = lay.centered(tab.y, tab.h);
    let fg = if active || hovered { c.panel_foreground } else { c.panel_muted };
    let room = Rect::new(tab.x, tab.y, (tab.w - (CLOSE_SLOT + PAD_RIGHT) * cw).max(0.0), tab.h);
    scene.push_clip(room);
    // an unsaved tab's name reads a little brighter, as in VS Code
    let style = if b.doc.dirty && !active {
        TextStyle::new(mix(c.panel_foreground, c.panel_muted, 0.7))
    } else {
        TextStyle::new(fg)
    };
    scene.text(tx, ty, &l.name, style, &[]);
    if !l.dir.is_empty() {
        let x = tx + (columns(&l.name) as f32) * cw;
        let dim = mix(c.panel_muted, c.panel_background, 0.75);
        scene.text(x, ty, DIR_SEP, TextStyle::new(dim), &[]);
        scene.text(x + columns(DIR_SEP) as f32 * cw, ty, &l.dir, TextStyle::new(dim), &[]);
    }
    scene.pop_clip();
}

/// `color` fading to transparent across `r`, towards the inside of the strip (`left`: from its left edge).
fn fade_edge(scene: &mut Scene, r: Rect, left: bool, color: Color) {
    const STEPS: usize = 8;
    let w = r.w / STEPS as f32;
    for k in 0..STEPS {
        let a = (255.0 * (1.0 - k as f32 / STEPS as f32)) as u8;
        let x = if left { r.x + k as f32 * w } else { r.right() - (k + 1) as f32 * w };
        scene.rect(Rect::new(x.floor(), r.y, w.ceil(), r.h), color.with_alpha(a));
    }
}

/// The tab list button: a chevron, highlighted while hovered or while its list is open.
fn list_button(scene: &mut Scene, r: Rect, on: bool, lay: &Layout, c: &Colors) {
    let s = lay.scale;
    let pill = Rect::new(r.x + 2.0 * s, r.y + 1.0 * s, r.w - 4.0 * s, r.h - 2.0 * lay.pad);
    if on {
        scene.rounded_rect(pill, (5.0 * s).round(), c.panel_selection, None);
    }
    let (cx, cy, k) = ((pill.x + pill.w / 2.0).round(), (pill.y + pill.h / 2.0).round(), 3.5 * s);
    let ink = if on { c.panel_foreground } else { c.panel_muted };
    line(scene, (cx - k, cy - k / 2.0), (cx, cy + k / 2.0), 1.3 * s, ink);
    line(scene, (cx, cy + k / 2.0), (cx + k, cy - k / 2.0), 1.3 * s, ink);
}

// ----- the mouse -----

/// What is at (`x`, `y`) in the last frame's strip.
fn hover_at(st: &State, x: f32, y: f32) -> Option<Hover> {
    let strip = &st.tabs;
    if strip.list.is_some_and(|r| r.contains(x, y)) {
        return Some(Hover::List);
    }
    if !strip.view.contains(x, y) {
        return None;
    }
    let p = strip.placed.iter().find(|p| p.rect.intersect(&strip.view).contains(x, y))?;
    // a hovered tab always shows its close button
    Some(Hover::Tab(p.index, p.close.contains(x, y)))
}

/// The pointer moved: the hovered tab, a tab drag. True when the strip must be drawn again.
pub fn moved(st: &mut State, lay: &Layout, x: f32, y: f32) -> bool {
    let mut redraw = false;
    if let Some(mut d) = st.tabs.drag {
        if !d.moving && (d.from.0 - x).abs() > crate::mouse::DRAG_SLOP * lay.scale {
            d.moving = true;
        }
        if d.moving {
            d.x = x;
            let w = st.ed.buffer_index(d.id).and_then(|i| st.tabs.widths.get(i)).copied().unwrap_or(0.0);
            d.to = drop_slot(st, d.id, x - d.grab + w / 2.0);
            redraw = true;
        }
        st.tabs.drag = Some(d);
    }
    // nothing is hovered under a menu, the picker, or a tab being carried
    let carrying = st.tabs.drag.is_some_and(|d| d.moving);
    let h = if st.menu_open() || st.ed.picker.is_some() || carrying { None } else { hover_at(st, x, y) };
    if h != st.tabs.hover {
        st.tabs.hover = h;
        redraw = true;
    }
    redraw
}

/// Where a tab whose middle is at `x` would drop: the number of other tabs whose middle is left of it.
fn drop_slot(st: &State, id: u64, x: f32) -> usize {
    let strip = &st.tabs;
    let origin = strip.view.x - strip.offset.round();
    let mids = strip.lefts.iter().zip(&strip.widths).map(|(l, w)| origin + l + w / 2.0);
    st.ed.buffers.iter().zip(mids).filter(|(b, mid)| b.id != id && *mid < x).count()
}

/// The pointer left the window.
pub fn left(st: &mut State) -> bool {
    st.tabs.hover.take().is_some()
}

/// A left press: on a tab it switches to it (a drag may follow), on its close button it closes on release, on the
/// list button the buffer list opens, a double click on the empty part of a row of its own opens a scratch buffer.
/// True when the strip took it.
pub fn press(st: &mut State, lay: &Layout, x: f32, y: f32, now: Instant) -> bool {
    match hover_at(st, x, y) {
        Some(Hover::List) => {
            st.ed.run(Command::SwitchBuffer);
            st.after_input();
            true
        }
        Some(Hover::Tab(i, close)) => {
            let id = st.ed.buffers[i].id;
            if close {
                st.tabs.pressed_close = Some(id);
                return true;
            }
            st.ed.sidebar_focus = false;
            st.ed.switch_to(i);
            let grab = st.tabs.placed.iter().find(|p| p.index == i).map_or(0.0, |p| x - p.rect.x);
            st.tabs.drag = Some(Drag { id, from: (x, y), x, grab, moving: false, to: i });
            st.after_input();
            true
        }
        None => {
            // the empty part of the row (the custom title bar handles its own before this)
            if lay.top_bar || !lay.tabs.contains(x, y) || lay.tabs.h <= 0.0 {
                return false;
            }
            let double = st.tabs.last_click.is_some_and(|(t, (px, py))| {
                now.saturating_duration_since(t) < crate::mouse::DOUBLE_CLICK
                    && (px - x).hypot(py - y) < crate::mouse::DOUBLE_CLICK_PX * lay.scale
            });
            st.tabs.last_click = if double { None } else { Some((now, (x, y))) };
            if double {
                st.ed.run(Command::NewFile);
                st.after_input();
            }
            true
        }
    }
}

/// A left release: a close button closes its tab when released over it, a dragged tab drops. True when the strip
/// had the button.
pub fn release(st: &mut State, x: f32, y: f32) -> bool {
    let drag = st.tabs.drag.take();
    if let Some(id) = st.tabs.pressed_close.take() {
        if let Some(Hover::Tab(i, true)) = hover_at(st, x, y)
            && st.ed.buffers[i].id == id
        {
            close(st, id);
        }
        return true;
    }
    match drag {
        Some(d) if d.moving => {
            if let Some(from) = st.ed.buffer_index(d.id) {
                st.ed.move_buffer(from, d.to);
            }
            // the tab under the pointer is a different one now
            st.tabs.hover = hover_at(st, x, y);
            st.after_input();
            true
        }
        Some(_) => true,
        None => false,
    }
}

/// A middle press on a tab closes it. True when it was on one.
pub fn middle(st: &mut State, x: f32, y: f32) -> bool {
    match hover_at(st, x, y) {
        Some(Hover::Tab(i, _)) => {
            let id = st.ed.buffers[i].id;
            close(st, id);
            true
        }
        _ => false,
    }
}

/// Closes the buffer `id` (one with unsaved changes asks first).
fn close(st: &mut State, id: u64) {
    st.ed.message = None;
    st.ed.tab_command(Command::CloseBuffer, id);
    st.tabs.hover = None;
    st.after_input();
}

/// The wheel over the strip scrolls it sideways (`px`: pixels, positive to the right). True when the pointer is
/// over it (the wheel then scrolls nothing else).
pub fn wheel(st: &mut State, x: f32, y: f32, px: f32) -> bool {
    let strip = &mut st.tabs;
    if !strip.area.contains(x, y) || strip.area.h <= 0.0 {
        return false;
    }
    strip.target += px;
    true
}

/// Pixels a wheel event scrolls the strip: notches times [`WHEEL_COLS`] columns, a touchpad's pixels as they come.
pub fn wheel_px(delta: crate::mouse::Wheel, lay: &Layout) -> f32 {
    match delta {
        // winit: positive is up or left; down and right scroll on along the row
        crate::mouse::Wheel::Lines(dx, dy) => -(dx + dy) * WHEEL_COLS * lay.cw,
        crate::mouse::Wheel::Pixels(dx, dy) => -(dx + dy),
    }
}

/// Esc while a tab is dragged puts it back. True when it took the key.
pub fn key(st: &mut State, k: KeyPress) -> bool {
    if k.key == Key::Esc && st.tabs.drag.is_some_and(|d| d.moving) {
        st.tabs.drag = None;
        return true;
    }
    false
}

// ----- the right-click menu -----

/// The menu of tab `i`: closing it and others, its file's path, where it is, reopening a closed one. Commands show
/// the chord that runs them (on the active tab).
pub fn menu(st: &State, i: usize) -> Option<ContextMenu> {
    let ed = &st.ed;
    let b = ed.buffers.get(i)?;
    let id = b.id as usize;
    let file = b.doc.path.is_some();
    let n = ed.buffers.len();
    // a command's label and chord, run on this tab
    let on = |cmd: Command, f: AppFn| Action { run: Run::App(f, id), ..Action::command(ed, cmd) };
    let sublime = !ed.vim();
    let mut items: Vec<MenuItem> = vec![
        on(Command::CloseBuffer, |st, _, id| tab(st, Command::CloseBuffer, id)).label("Close").into(),
        on(Command::CloseOthers, |st, _, id| tab(st, Command::CloseOthers, id)).enabled(n > 1).into(),
        on(Command::CloseRight, |st, _, id| tab(st, Command::CloseRight, id)).enabled(i + 1 < n).into(),
        on(Command::CloseSaved, |st, _, id| tab(st, Command::CloseSaved, id))
            .hint(sublime.then(|| "Ctrl+K U".to_string()))
            .enabled(ed.buffers.iter().any(|b| !b.doc.dirty))
            .into(),
        on(Command::CloseAll, |st, _, id| tab(st, Command::CloseAll, id))
            .hint(sublime.then(|| "Ctrl+K W".to_string()))
            .into(),
        MenuItem::Separator,
        on(Command::CopyPath, |st, _, id| tab(st, Command::CopyPath, id)).enabled(file).into(),
        on(Command::CopyRelativePath, |st, _, id| tab(st, Command::CopyRelativePath, id)).enabled(file).into(),
    ];
    if ed.features.available(Feature::Sidebar) {
        items
            .push(on(Command::RevealInSidebar, |st, _, id| tab(st, Command::RevealInSidebar, id)).enabled(file).into());
    }
    items.push(on(Command::RevealInFileManager, |st, _, id| reveal_tab(st, id as u64)).enabled(file).into());
    #[cfg(feature = "terminal")]
    if ed.features.on(Feature::Terminal) {
        items.push(
            Action::app_with("Open in Terminal", |st, _, id| terminal_at(st, id as u64), id).enabled(file).into(),
        );
    }
    items.push(MenuItem::Separator);
    items.push(Action::command(ed, Command::ReopenClosed).label("Reopen Closed").into());
    Some(ContextMenu::new(Target::Tab(i), items))
}

/// The menu of the empty part of a row of tabs of its own.
pub fn strip_menu(st: &State) -> Option<ContextMenu> {
    let spec = ["new|New Scratch Buffer", "-", "close_saved|Close Saved", "close_all|Close All"];
    let mut items = crate::context_menu::items_from_spec(&st.ed, &spec);
    if !st.ed.vim() {
        for e in &mut items {
            if let MenuItem::Action(a) = e {
                match a.label.as_str() {
                    "Close All" => a.hint = Some("Ctrl+K W".into()),
                    "Close Saved" => a.hint = Some("Ctrl+K U".into()),
                    _ => {}
                }
            }
        }
    }
    items.push(MenuItem::Separator);
    items.push(Action::command(&st.ed, Command::ReopenClosed).label("Reopen Closed").into());
    Some(ContextMenu::new(Target::TabStrip, items))
}

/// A tab command for the buffer with id `id`.
fn tab(st: &mut State, cmd: Command, id: usize) {
    st.ed.tab_command(cmd, id as u64);
}

/// [`shrew_core::Event::App`] commands this module carries out.
pub fn is_app_command(cmd: Command) -> bool {
    cmd == Command::RevealInFileManager
}

pub fn app_command(st: &mut State, cmd: Command) {
    if cmd == Command::RevealInFileManager {
        let id = st.ed.buf().id;
        reveal_tab(st, id);
    }
}

/// Shows buffer `id`'s file in the desktop's file manager.
fn reveal_tab(st: &mut State, id: u64) {
    let Some(i) = st.ed.buffer_index(id) else { return };
    let Some(path) = st.ed.file_of(i) else { return };
    match reveal_in_file_manager(&path) {
        Ok(()) => st.ed.info(format!("showing {}", path.display())),
        Err(e) => st.ed.error(format!("cannot open the file manager: {e}")),
    }
}

/// Opens the folder holding `path` in the file manager (`xdg-open` on the folder; `open -R` and `explorer
/// /select,` select the file itself), detached.
pub fn reveal_in_file_manager(path: &Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("explorer");
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path.as_os_str());
        c.arg(arg);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(dir);
        c
    };
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    // reap it without blocking the UI
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// A new terminal in the folder of buffer `id`'s file.
#[cfg(feature = "terminal")]
fn terminal_at(st: &mut State, id: u64) {
    let Some(i) = st.ed.buffer_index(id) else { return };
    let Some(path) = st.ed.file_of(i) else { return };
    let dir = path.parent().map_or_else(|| st.ed.work_dir(), Path::to_path_buf);
    st.open_terminal_in(None, dir);
}

#[cfg(test)]
mod tests;
