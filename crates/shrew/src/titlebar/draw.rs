//! Drawing the title bar (layer 0, the tab row) and its menus (layer 3, above every overlay). Icons are drawn from
//! rects and circles rather than font glyphs, so they look the same in every font; colors are theme tokens only.

use super::items::{self, Item};
use super::menu::{Entry, MenuHits};
use super::{on, own_window_buttons, project_name, shown};
use crate::colors::{Colors, mix};
use crate::draw::{Ui, popup, tabs_in};
use crate::layout::Layout;
use crate::state::State;
use shrew_core::Editor;
use shrew_render::{Color, Rect, Scene, TextStyle, columns};

/// Room the macOS traffic lights take at the bar's left end (logical pixels).
const TRAFFIC_LIGHTS: f32 = 76.0;

/// Draws the bar with the tabs in it. False when the custom title bar is off (the app draws the plain tab row).
pub fn draw(scene: &mut Scene, st: &mut State, ui: &mut Ui, lay: &Layout, c: &Colors) -> bool {
    if !on(&st.ed) {
        st.titlebar.placed.clear();
        st.titlebar.bar = Rect::default();
        return false;
    }
    let bar = lay.tabs;
    st.titlebar.bar = bar;
    scene.rect(bar, c.panel_background);
    let inner = if cfg!(target_os = "macos") {
        let l = (TRAFFIC_LIGHTS * lay.scale).round();
        Rect::new(bar.x + l, bar.y, (bar.w - l).max(0.0), bar.h)
    } else {
        bar
    };
    let project = project_label(&st.ed);
    let button = bar.h.round();
    let width = |i: Item| match i {
        Item::Project => ((columns(&project) + 3) as f32 * lay.cw).round(),
        i if i.is_window_button() => (bar.h * 1.4).round(),
        _ => button,
    };
    let placed = items::place(&st.titlebar.layout, inner, shown, width, (lay.pad / 2.0).round());
    let tb = &st.titlebar;
    let menu_open = tb.menu.as_ref().is_some_and(|m| m.at.is_none());
    for &(item, r) in &placed {
        if item == Item::Tabs {
            continue;
        }
        let hover = tb.hover == Some(item);
        let pressed = hover && tb.pressed == Some(item);
        let active = (item == Item::Menu && menu_open) || pressed;
        let ink = if hover || active { c.panel_foreground } else { c.panel_muted };
        if item.is_window_button() {
            let (bg, ink) = match (item, hover, pressed) {
                (Item::Close, true, false) => (Some(c.error), c.error_fg),
                (Item::Close, true, true) => (Some(mix(c.error, c.panel_background, 0.75)), c.error_fg),
                (_, true, false) => (Some(c.panel_selection), ink),
                (_, true, true) => (Some(c.panel_border), ink),
                _ => (None, ink),
            };
            if let Some(bg) = bg {
                scene.rect(r, bg);
            }
            icon(scene, item, r, lay, ink, tb.maximized);
            continue;
        }
        // other items: a rounded hover pill inside the bar
        let pill = Rect::new(r.x, r.y + lay.pad, r.w, r.h - 2.0 * lay.pad);
        if active {
            scene.rounded_rect(pill, (5.0 * lay.scale).round(), c.panel_border, None);
        } else if hover {
            scene.rounded_rect(pill, (5.0 * lay.scale).round(), c.panel_selection, None);
        }
        if item == Item::Project {
            let y = lay.centered(r.y, r.h);
            scene.text((r.x + 1.5 * lay.cw).round(), y, &project, TextStyle::new(c.panel_foreground).bold(), &[]);
        } else {
            icon(scene, item, r, lay, ink, false);
        }
    }
    // the tabs: they hang from the bar's bottom edge, as in their own row
    if let Some(&(_, r)) = placed.iter().find(|p| p.0 == Item::Tabs) {
        tabs_in(scene, st, ui, lay, c, r);
    }
    st.titlebar.placed = placed;
    // without the system's frame (and its shadow) a hairline marks the window's edge, over everything but the menus
    if own_window_buttons() && !st.titlebar.maximized {
        scene.layer(4);
        let edge = Rect::new(0.0, 0.0, lay.width, lay.height);
        scene.rounded_rect(edge, 0.0, Color::TRANSPARENT, Some((lay.scale.round().max(1.0), c.panel_border)));
        scene.layer(0);
    }
    true
}

/// The project item's text: the project's name, `~` at home.
fn project_label(ed: &Editor) -> String {
    project_name(ed).unwrap_or_else(|| if ed.cwd.parent().is_some() { "~".into() } else { "/".into() })
}

/// A line from `a` to `b`, `w` thick, as a row of overlapping dots (the renderer draws antialiased rounded rects,
/// not lines). Opaque colors only.
fn line(scene: &mut Scene, a: (f32, f32), b: (f32, f32), w: f32, col: Color) {
    let len = (b.0 - a.0).hypot(b.1 - a.1);
    let n = (len / (w * 0.3)).ceil().max(1.0) as usize;
    for k in 0..=n {
        let t = k as f32 / n as f32;
        let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        scene.rounded_rect(Rect::new(x - w / 2.0, y - w / 2.0, w, w), w / 2.0, col, None);
    }
}

fn ring(scene: &mut Scene, cx: f32, cy: f32, d: f32, w: f32, col: Color) {
    scene.rounded_rect(Rect::new(cx - d / 2.0, cy - d / 2.0, d, d), d / 2.0, Color::TRANSPARENT, Some((w, col)));
}

/// An item's icon, centered in `r`.
fn icon(scene: &mut Scene, item: Item, r: Rect, lay: &Layout, ink: Color, maximized: bool) {
    let s = lay.scale;
    let (cx, cy) = ((r.x + r.w / 2.0).round(), (r.y + r.h / 2.0).round());
    let t = (1.0 * s).round().max(1.0); // hairline
    let lw = 1.4 * s; // stroked lines
    let z = (14.0 * s).round(); // icon box
    let (x0, y0) = (cx - z / 2.0, cy - z / 2.0);
    match item {
        Item::Menu => {
            let w = z;
            for k in [-1.0, 0.0, 1.0] {
                let y = (cy + k * 4.0 * s - t / 2.0).round();
                scene.rounded_rect(Rect::new(x0, y, w, t.max(1.5 * s).round()), t, ink, None);
            }
        }
        Item::Sidebar => {
            let b = Rect::new(x0, (cy - 6.0 * s).round(), z, (12.0 * s).round());
            scene.rounded_rect(b, 2.0 * s, Color::TRANSPARENT, Some((t, ink)));
            scene.rect(Rect::new(b.x, b.y, (z * 0.36).round(), b.h), ink);
        }
        Item::Terminal => {
            let b = Rect::new(x0, (cy - 6.0 * s).round(), z, (12.0 * s).round());
            scene.rounded_rect(b, 2.0 * s, Color::TRANSPARENT, Some((t, ink)));
            let (px, py) = (b.x + 3.5 * s, b.y + 3.5 * s);
            line(scene, (px, py), (px + 2.5 * s, py + 2.5 * s), lw, ink);
            line(scene, (px + 2.5 * s, py + 2.5 * s), (px, py + 5.0 * s), lw, ink);
            scene.rect(Rect::new((px + 4.5 * s).round(), (py + 4.5 * s).round(), (4.0 * s).round(), t), ink);
        }
        Item::Files => {
            let d = 9.0 * s;
            let (rx, ry) = (x0 + d / 2.0 + 0.5 * s, y0 + d / 2.0 + 0.5 * s);
            ring(scene, rx, ry, d, lw, ink);
            let o = d / 2.0 * std::f32::consts::FRAC_1_SQRT_2;
            line(scene, (rx + o, ry + o), (x0 + z - 1.0 * s, y0 + z - 1.0 * s), lw * 1.2, ink);
        }
        Item::Settings => {
            // a cog: a ring with eight teeth
            ring(scene, cx, cy, 8.0 * s, 2.0 * s, ink);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let (dx, dy) = (a.cos(), a.sin());
                line(
                    scene,
                    (cx + dx * 4.5 * s, cy + dy * 4.5 * s),
                    (cx + dx * 6.2 * s, cy + dy * 6.2 * s),
                    2.4 * s,
                    ink,
                );
            }
        }
        Item::About => {
            ring(scene, cx, cy, z, t.max(1.2 * s), ink);
            let y = (cy - lay.ch / 2.0).round();
            scene.text((cx - lay.cw / 2.0).round(), y, "?", TextStyle::new(ink).bold(), &[]);
        }
        Item::Minimize => {
            let w = (10.0 * s).round();
            scene.rect(Rect::new(cx - w / 2.0, cy, w, t), ink);
        }
        Item::Maximize => {
            let w = (10.0 * s).round();
            if maximized {
                // restore: a window behind another
                let (o, q) = ((2.0 * s).round(), w - (2.0 * s).round());
                let back = Rect::new(cx - w / 2.0 + o, cy - w / 2.0, q, q);
                scene.rect(Rect::new(back.x, back.y, back.w, t), ink);
                scene.rect(Rect::new(back.right() - t, back.y, t, back.h), ink);
                let front = Rect::new(cx - w / 2.0, cy - w / 2.0 + o, q, q);
                scene.rounded_rect(front, 1.0 * s, Color::TRANSPARENT, Some((t, ink)));
            } else {
                let r = Rect::new(cx - w / 2.0, cy - w / 2.0, w, w);
                scene.rounded_rect(r, 1.0 * s, Color::TRANSPARENT, Some((t, ink)));
            }
        }
        Item::Close => {
            let h = 5.0 * s;
            line(scene, (cx - h, cy - h), (cx + h, cy + h), 1.3 * s, ink);
            line(scene, (cx - h, cy + h), (cx + h, cy - h), 1.3 * s, ink);
        }
        Item::Project | Item::Tabs => {}
    }
}

/// The open menu bar or right-click menu, above everything else.
pub fn draw_menus(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors) {
    let Some(m) = &mut st.titlebar.menu else { return };
    scene.layer(4);
    let (cw, pad) = (lay.cw, lay.pad);
    let row_h = (lay.ch + 1.5 * pad).round();
    let sep_h = (2.0 * pad + 1.0).round();
    let mut hits = MenuHits::default();
    let below = st.titlebar.bar.bottom().max(lay.tabs.bottom());
    // the list of menus, under the menu button
    let mut sub_at = (m.anchor_x, below + pad);
    if m.at.is_none() {
        let cols = m.menus.iter().map(|x| columns(x.title)).max().unwrap_or(0) + 7;
        let w = (cols as f32 * cw).round();
        let h = (m.menus.len() as f32 * row_h + 2.0 * pad).round();
        let x = m.anchor_x.clamp(0.0, (lay.width - w).max(0.0)).round();
        let panel = Rect::new(x, (below + pad / 2.0).round(), w, h);
        popup(scene, panel, lay, c);
        hits.panels.push(panel);
        for (i, menu) in m.menus.iter().enumerate() {
            let row = Rect::new(panel.x + pad, panel.y + pad + i as f32 * row_h, panel.w - 2.0 * pad, row_h);
            if i == m.open {
                let bg = if m.item.is_none() { c.panel_selection } else { mix(c.panel_selection, c.popup, 0.5) };
                scene.rounded_rect(row, (5.0 * lay.scale).round(), bg, None);
                sub_at = (panel.right() + pad / 2.0, row.y - pad);
            }
            let y = lay.centered(row.y, row.h);
            // the mnemonic letter underlined, as in a system menu bar
            let fg = TextStyle::new(c.panel_foreground);
            let u = menu.title.char_indices().find(|(_, ch)| ch.to_ascii_lowercase() == menu.mnemonic);
            let spans: Vec<_> =
                u.map(|(b, ch)| (b..b + ch.len_utf8(), fg.underline(c.panel_foreground))).into_iter().collect();
            scene.text((row.x + cw).round(), y, menu.title, fg, &spans);
            let chev = Rect::new(row.right() - 2.0 * cw, row.y, cw, row.h);
            let (hx, hy) = (chev.x + cw * 0.35, (chev.y + chev.h / 2.0).round());
            let k = 3.0 * lay.scale;
            line(scene, (hx, hy - k), (hx + k, hy), 1.2 * lay.scale, c.panel_muted);
            line(scene, (hx + k, hy), (hx, hy + k), 1.2 * lay.scale, c.panel_muted);
            hits.roots.push((row, i));
        }
    } else if let Some(at) = m.at {
        sub_at = at;
    }
    // the open menu's entries
    let entries = m.entries();
    let label_cols =
        entries.iter().map(|e| if let Entry::Command { label, .. } = e { columns(label) } else { 0 }).max();
    let key_cols =
        entries.iter().filter_map(|e| if let Entry::Command { keys, .. } = e { keys.as_deref() } else { None });
    let key_cols = key_cols.map(columns).max().unwrap_or(0);
    let cols = 3 + label_cols.unwrap_or(0) + if key_cols > 0 { 4 + key_cols } else { 0 } + 2;
    let w = (cols.max(20) as f32 * cw).round();
    let height = |e: &Entry| if *e == Entry::Separator { sep_h } else { row_h };
    let full = entries.iter().map(height).sum::<f32>() + 2.0 * pad;
    let h = full.min((lay.height - below - pad).max(row_h + 2.0 * pad)).round();
    // taller than the window: it scrolls to keep the highlighted entry in view
    let mut top = if full > h { m.top.min(entries.len().saturating_sub(1)) } else { 0 };
    if let Some(i) = m.item.filter(|_| full > h) {
        top = top.min(i);
        while top < i && entries[top..=i].iter().map(height).sum::<f32>() + 2.0 * pad > h {
            top += 1;
        }
    }
    let mut x = sub_at.0;
    if x + w > lay.width {
        // no room on the right: open to the left of the list of menus (or against the window's edge)
        x = hits.panels.first().map_or(lay.width - w, |p| p.x - w - pad / 2.0);
    }
    let x = x.clamp(0.0, (lay.width - w).max(0.0)).round();
    let y = sub_at.1.min(lay.height - h - pad).max(below).round();
    let panel = Rect::new(x, y, w, h);
    popup(scene, panel, lay, c);
    hits.panels.push(panel);
    let inner = Rect::new(panel.x, panel.y + pad, panel.w, panel.h - 2.0 * pad);
    scene.push_clip(inner);
    let mut ry = inner.y;
    for (i, e) in entries.iter().enumerate().skip(top) {
        if ry >= inner.bottom() {
            break;
        }
        match e {
            Entry::Separator => {
                let r = Rect::new(panel.x + pad, (ry + sep_h / 2.0).floor(), panel.w - 2.0 * pad, 1.0);
                scene.rect(r, c.panel_border);
                ry += sep_h;
            }
            Entry::Command { label, keys, checked, .. } => {
                let row = Rect::new(panel.x + pad, ry, panel.w - 2.0 * pad, row_h);
                if m.item == Some(i) {
                    scene.rounded_rect(row, (5.0 * lay.scale).round(), c.panel_selection, None);
                }
                let ty = lay.centered(row.y, row.h);
                if *checked {
                    let (cx, cy, k) = (row.x + 1.2 * cw, (row.y + row.h / 2.0).round(), 3.0 * lay.scale);
                    line(scene, (cx - k, cy), (cx - k * 0.2, cy + k * 0.8), 1.4 * lay.scale, c.accent);
                    line(scene, (cx - k * 0.2, cy + k * 0.8), (cx + k * 1.3, cy - k), 1.4 * lay.scale, c.accent);
                }
                scene.text((row.x + 2.5 * cw).round(), ty, label, TextStyle::new(c.panel_foreground), &[]);
                if let Some(k) = keys {
                    let kx = row.right() - (columns(k) as f32 + 1.0) * cw;
                    scene.text(kx.round(), ty, k, TextStyle::new(c.panel_muted), &[]);
                }
                hits.items.push((row.intersect(&inner), i));
                ry += row_h;
            }
        }
    }
    scene.pop_clip();
    m.top = top;
    m.hits = hits;
    scene.layer(0);
}
