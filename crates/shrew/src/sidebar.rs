//! The sidebar: the project's file tree on the left (the model and its keys are `shrew_core::tree`). Drawing, and
//! what the mouse does there:
//!
//! - a click on a file opens it and gives it the keys (a double click does the same, once); a click on a folder
//!   opens or closes it, a click on its chevron only that (the selection stays), and a double click on a folder
//!   does not close it again;
//! - a middle click on a file opens it in a tab behind the current one;
//! - the row under the pointer is lightly highlighted, the wheel scrolls, the right edge drags to resize (the width
//!   is saved to config.toml as `[sidebar] width`);
//! - a right click opens the [`menu`] of the row (or of the empty part), see [`menu::menu`].

mod menu;
#[cfg(test)]
pub(crate) mod tests;

pub use menu::menu;

use crate::colors::{Colors, mix};
use crate::layout::Layout;
use crate::state::State;
use shrew_render::{Rect, Scene, TextStyle, columns};
use std::ops::Range;
use std::time::Instant;

/// Logical pixels.
pub const DEFAULT_WIDTH: f32 = 250.0;
pub const MIN_WIDTH: f32 = 120.0;
pub const MAX_WIDTH: f32 = 1200.0;

/// Clickable parts of the sidebar in the last frame.
#[derive(Default, Debug)]
pub struct Hits {
    pub area: Option<Rect>,
    /// The resize handle on the right edge.
    pub edge: Option<Rect>,
    /// Rows: rect, index into the tree's rows.
    pub rows: Vec<(Rect, usize)>,
    /// Where depth 0's chevron starts and how far each level indents (a row's chevron is two columns there).
    pub indent: (f32, f32),
}

/// The sidebar's width in pixels for this window (0 when it is off).
pub fn width(st: &State, window_w: f32, scale: f32) -> f32 {
    // ----- git: panel ----- Ctrl+Shift+G shows the Git tab even with the sidebar off
    #[cfg(feature = "git")]
    let git = crate::git::panel::draw::alone(st);
    #[cfg(not(feature = "git"))]
    let git = false;
    if !st.ed.sidebar_on() && !git {
        return 0.0;
    }
    (st.sidebar_width.clamp(MIN_WIDTH, MAX_WIDTH) * scale).min(window_w * 0.6).round()
}

pub fn draw(scene: &mut Scene, st: &mut State, hits: &mut Hits, lay: &Layout, c: &Colors) {
    let area = lay.sidebar;
    if area.w <= 0.0 {
        return;
    }
    let (cw, ch, pad, scale) = (lay.cw, lay.ch, lay.pad, lay.scale);
    let border = scale.round().max(1.0);
    scene.rect(area, c.panel_background);
    scene.rect(Rect::new(area.right() - border, area.y, border, area.h), c.panel_border);
    hits.area = Some(area);
    hits.edge = Some(Rect::new(area.right() - (3.0 * scale).round(), area.y, (6.0 * scale).round(), area.h));
    let inner = Rect::new(area.x, area.y, area.w - border, area.h);
    scene.push_clip(inner);
    // ----- git: panel ----- the Files | Git tabs in the header; the Git tab draws instead of the tree
    #[cfg(feature = "git")]
    if crate::git::panel::draw::sidebar(scene, st, lay, c) {
        scene.pop_clip();
        return;
    }
    let focused = st.ed.sidebar_focus;
    let active = st.ed.buf().doc.path.clone();
    let x0 = area.x + 3.0 * pad;
    let head_h = (ch + 3.0 * pad).round();
    // ----- git: file states -----
    #[cfg(feature = "git")]
    let deco = st.ed.tree.as_ref().and_then(|t| crate::git::status_ui::tree_deco(&st.git, &st.ed.features, &t.root));
    let Some(t) = st.ed.tree.as_mut() else {
        // no project: say how to get one
        let lines =
            [("No folder open", c.panel_foreground), ("srw <folder>", c.panel_muted), ("Ctrl+Alt+P", c.panel_muted)];
        for (i, (s, col)) in lines.iter().enumerate() {
            scene.text(x0, (area.y + head_h + i as f32 * ch).round(), s, TextStyle::new(*col), &[]);
        }
        scene.pop_clip();
        return;
    };
    // header: the project folder's name
    let name = t.root.file_name().map_or_else(|| t.root.display().to_string(), |n| n.to_string_lossy().to_uppercase());
    scene.text(x0, lay.centered(area.y, head_h), &name, TextStyle::new(c.panel_muted).bold(), &[]);
    let row_h = (ch + pad).round();
    let mut y = area.y + head_h;
    // the filter, while typing it or while it narrows the tree
    if t.filtering || !t.filter.is_empty() {
        let fy = lay.centered(y, row_h);
        scene.text(x0, fy, "/", TextStyle::new(c.accent).bold(), &[]);
        let fx = x0 + 2.0 * cw;
        scene.text(fx, fy, &t.filter, TextStyle::new(c.panel_foreground), &[]);
        if t.filtering && focused {
            let bar = (2.0 * scale).round().max(1.0);
            scene.rect(Rect::new((fx + columns(&t.filter) as f32 * cw).round(), fy, bar, ch), c.cursor);
        }
        y += row_h;
    }
    let visible = (((area.bottom() - y - pad) / row_h).floor() as usize).max(1);
    t.fit(visible);
    if t.rows.is_empty() {
        let msg = if t.filter.is_empty() { "empty folder" } else { "no match" };
        scene.text(x0, lay.centered(y, row_h), msg, TextStyle::new(c.panel_muted), &[]);
    }
    let radius = (4.0 * scale).round();
    let hl = TextStyle::new(c.fuzzy_match).bold();
    let mut spans: Vec<(Range<usize>, TextStyle)> = Vec::new();
    hits.indent = (x0, 2.0 * cw);
    // rows below the last one that fits are cut by the clip, and their hit rects with them
    let list = Rect::new(inner.x, y, inner.w, (inner.bottom() - y).max(0.0));
    for (i, row) in t.rows.iter().enumerate().skip(t.top).take(visible + 1) {
        let ry = (y + (i - t.top) as f32 * row_h).round();
        let rect = Rect::new(inner.x, ry, inner.w, row_h);
        let current = active.as_deref() == Some(row.path.as_path());
        let pill = Rect::new(inner.x + pad, ry, inner.w - 2.0 * pad, row_h);
        if i == t.selected {
            let bg = if focused { c.panel_selection } else { mix(c.panel_selection, c.panel_background, 0.5) };
            scene.rounded_rect(pill, radius, bg, None);
        } else if t.hover == Some(i) {
            scene.rounded_rect(pill, radius, crate::statusbar::hover_fill(c), None);
        }
        let ty = lay.centered(ry, row_h);
        let x = x0 + (row.depth * 2) as f32 * cw;
        if row.dir {
            let chevron = if row.open { "▾" } else { "▸" };
            scene.text(x, ty, chevron, TextStyle::new(c.panel_muted), &[]);
        }
        let fg = if current {
            c.accent
        } else if row.ignored {
            mix(c.panel_muted, c.panel_background, 0.7)
        } else if row.dir {
            c.panel_foreground
        } else {
            mix(c.panel_foreground, c.panel_muted, 0.8)
        };
        spans.clear();
        let mut it = row.name.char_indices().enumerate();
        for &ci in &row.hl {
            if let Some((_, (b, chr))) = it.by_ref().find(|(n, _)| *n == ci) {
                spans.push((b..b + chr.len_utf8(), hl));
            }
        }
        let mut style = TextStyle::new(fg);
        if current {
            style = style.bold();
        }
        // ----- git: the state colours the name; its letter goes at the right -----
        #[cfg(feature = "git")]
        let (style, name_end) = match deco.as_ref().and_then(|d| d.row(&row.path, row.dir, row.ignored, c)) {
            Some(m) => {
                (TextStyle { color: m.color, ..style }, crate::git::status_ui::draw_row_mark(scene, m, rect, ty, lay))
            }
            None => (style, rect.right()),
        };
        #[cfg(not(feature = "git"))]
        let name_end = rect.right();
        scene.push_clip(Rect::new(rect.x, rect.y, (name_end - rect.x).max(0.0), rect.h));
        scene.text(x + 2.0 * cw, ty, &row.name, style, &spans);
        scene.pop_clip();
        hits.rows.push((rect.intersect(&list), i));
    }
    scene.pop_clip();
}

/// The tree row at (`x`, `y`) in the last frame.
fn row_at(hits: &Hits, x: f32, y: f32) -> Option<usize> {
    if !hits.area.is_some_and(|a| a.contains(x, y)) {
        return None;
    }
    hits.rows.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1)
}

/// A left click at (`x`, `y`), at `now` (for double clicks). True when it was on the sidebar (taken).
pub fn press(st: &mut State, hits: &Hits, x: f32, y: f32, now: Instant) -> bool {
    if !hits.area.is_some_and(|a| a.contains(x, y)) {
        return false;
    }
    let Some(i) = row_at(hits, x, y) else {
        st.ed.sidebar_focus = st.ed.tree.is_some();
        st.after_input();
        return true;
    };
    let Some(t) = st.ed.tree.as_mut() else { return true };
    let Some(row) = t.rows.get(i) else { return true };
    let (dir, open, path, depth) = (row.dir, row.open, row.path.clone(), row.depth);
    // the second click of a double click: the first one did the work
    let double =
        t.last_click.is_some_and(|(at, j)| j == i && now.saturating_duration_since(at) < crate::mouse::DOUBLE_CLICK);
    t.last_click = if double { None } else { Some((now, i)) };
    let (x0, step) = hits.indent;
    let chevron = x >= x0 + depth as f32 * step - step / 4.0 && x < x0 + (depth as f32 + 1.0) * step;
    match (dir, double) {
        // a double click on a folder does not close it again; on a file, the file (already open) keeps the keys
        (true, true) => st.ed.sidebar_focus = true,
        (false, true) => st.ed.sidebar_focus = false,
        // the chevron folds the folder and leaves the selection alone
        (true, false) if chevron => {
            t.set_open(&path, !open);
            st.ed.sidebar_focus = true;
        }
        _ => st.ed.tree_click(i, false),
    }
    st.after_input();
    true
}

/// A middle click on a file opens it in a tab behind the current one. True when it was on a row.
pub fn middle(st: &mut State, hits: &Hits, x: f32, y: f32) -> bool {
    let Some(i) = row_at(hits, x, y) else { return false };
    let Some(row) = st.ed.tree.as_ref().and_then(|t| t.rows.get(i)) else { return false };
    if row.dir {
        return true;
    }
    let path = row.path.clone();
    let here = st.ed.buf().id;
    st.open_at(&path, None, None);
    if let Some(back) = st.ed.buffers.iter().position(|b| b.id == here) {
        st.ed.switch_to(back);
    }
    st.after_input();
    true
}

/// The pointer moved: the row under it is lit. True when that changed (draw again).
pub fn hover(st: &mut State, hits: &Hits, x: f32, y: f32) -> bool {
    // under a modal panel or an open menu the lit row stays as it was (the right-clicked one stays lit)
    if st.ed.picker.is_some() || st.ed.about.is_some() || st.settings.is_open() || st.menu_open() {
        return false;
    }
    let row = row_at(hits, x, y);
    st.ed.tree.as_mut().is_some_and(|t| std::mem::replace(&mut t.hover, row) != row)
}

/// The pointer left the window. True when a row was lit.
pub fn pointer_left(st: &mut State) -> bool {
    st.ed.tree.as_mut().is_some_and(|t| t.hover.take().is_some())
}

/// The wheel over the sidebar scrolls the tree. True when it was there.
pub fn wheel(st: &mut State, hits: &Hits, x: f32, y: f32, lines: f32) -> bool {
    if !hits.area.is_some_and(|a| a.contains(x, y)) {
        return false;
    }
    if let Some(t) = &mut st.ed.tree
        && lines.abs() >= 0.5
    {
        t.scroll(lines.round() as isize);
    }
    true
}

/// Dragging the edge to `x` (pixels): the new width in logical pixels.
pub fn drag_to(st: &mut State, x: f32, scale: f32) {
    st.sidebar_width = (x / scale.max(0.1)).clamp(MIN_WIDTH, MAX_WIDTH).round();
}
