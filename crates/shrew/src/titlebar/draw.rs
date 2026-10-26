//! Drawing the title bar (layer 0, the tab row) and its menus (layer 3, above every overlay). Icons are drawn from
//! rects and circles rather than font glyphs, so they look the same in every font; colors are theme tokens only.

use super::items::{self, Item};
use super::menu::MenuHits;
use super::{on, own_window_buttons, project_name, shown};
use crate::colors::{Colors, mix};
use crate::context_menu::{draw_list, list_size, max_top, row_h, scroll_top};
use crate::draw::{Ui, line, popup, tabs_in};
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
    // ----- git: the branch (not shown outside a repository) -----
    #[cfg(feature = "git")]
    let branch = crate::git::status_ui::title_label(st);
    #[cfg(not(feature = "git"))]
    let branch: Option<String> = None;
    let shown = |i: Item| shown(i) && (i != Item::Branch || branch.is_some());
    let button = bar.h.round();
    let width = |i: Item| match i {
        Item::Project => ((columns(&project) + 3) as f32 * lay.cw).round(),
        #[cfg(feature = "git")]
        Item::Branch => crate::git::status_ui::title_width(branch.as_deref().unwrap_or(""), lay),
        i if i.is_window_button() => (bar.h * 1.4).round(),
        _ => button,
    };
    let placed = items::place(&st.titlebar.layout, inner, shown, width, (lay.pad / 2.0).round());
    let tb = &st.titlebar;
    let menu_open = tb.menu.is_some();
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
        } else if item == Item::Branch {
            #[cfg(feature = "git")]
            crate::git::status_ui::draw_title(scene, st, r, lay, c, ink);
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
        Item::Project | Item::Tabs | Item::Branch => {}
    }
}

/// The open menu bar, above everything else: the list of menus under the menu button, the open one beside it.
pub fn draw_menus(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors) {
    let Some(m) = &mut st.titlebar.menu else { return };
    scene.layer(4);
    let (cw, pad) = (lay.cw, lay.pad);
    let row_h = row_h(lay);
    let mut hits = MenuHits::default();
    let below = st.titlebar.bar.bottom().max(lay.tabs.bottom());
    // the list of menus, under the menu button
    let mut sub_at = (m.anchor_x, below + pad);
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
        let span = u.map(|(b, ch)| (b..b + ch.len_utf8(), fg.underline(c.panel_foreground)));
        scene.text((row.x + cw).round(), y, menu.title, fg, span.as_slice());
        let chev = Rect::new(row.right() - 2.0 * cw, row.y, cw, row.h);
        let (hx, hy) = (chev.x + cw * 0.35, (chev.y + chev.h / 2.0).round());
        let k = 3.0 * lay.scale;
        line(scene, (hx, hy - k), (hx + k, hy), 1.2 * lay.scale, c.panel_muted);
        line(scene, (hx + k, hy), (hx, hy + k), 1.2 * lay.scale, c.panel_muted);
        hits.roots.push((row, i));
    }
    // the open menu's entries, drawn as a context menu's
    let entries = m.entries();
    let (w, full) = list_size(entries, lay);
    let h = full.min((lay.height - below - pad).max(row_h + 2.0 * pad)).round();
    // taller than the window: it scrolls to keep the highlighted entry in view (not after the wheel scrolled it)
    let top = scroll_top(entries, if m.wheeled { None } else { m.item }, m.top, h, lay);
    // how far the wheel can scroll it: the last entries just fit
    let max_top = max_top(entries, h, lay);
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
    draw_list(scene, entries, m.item, panel, top, lay, c, &mut hits.items);
    m.top = top;
    m.max_top = max_top;
    m.hits = hits;
    scene.layer(0);
}
