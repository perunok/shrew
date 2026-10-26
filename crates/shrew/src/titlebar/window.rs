//! The title bar's window behaviour: the window attributes, moving (drag an empty part), maximizing (double-click),
//! resizing from the edges while undecorated, the window buttons, and switching decorations at runtime. Moving and
//! resizing are queued as [`WindowAction`]s and handed to the window manager by the app (`drag_window`,
//! `drag_resize_window`: an xdg move/resize on Wayland, `_NET_WM_MOVERESIZE` on X11), so snapping and tiling work as
//! with a system title bar.

use super::items::Item;
use super::menu::Outcome;
use super::{TitleBar, activate, on, run};
use crate::draw::Hits;
use crate::layout::Layout;
use crate::state::State;
use std::time::{Duration, Instant};
use winit::window::{CursorIcon, ResizeDirection, Window, WindowAttributes};

/// Width of the resize borders inside an undecorated window, and of the corners, in logical pixels.
const BORDER: f32 = 5.0;
const CORNER: f32 = 12.0;
/// The pointer moves this far (logical pixels) with the button held before an empty part of the bar drags the window.
const DRAG_SLOP: f32 = crate::mouse::DRAG_SLOP;
const DOUBLE_CLICK: Duration = crate::mouse::DOUBLE_CLICK;

/// Window work only the app can do (it owns the window).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    Minimize,
    ToggleMaximize,
    /// The close button: like the system's (refused once while there are unsaved changes).
    Close,
    /// Move the window with the pointer (a drag on an empty part of the bar), until the button is released.
    Drag,
    /// Resize the window from this edge or corner with the pointer.
    Resize(ResizeDirection),
}

/// The window as it is created: undecorated with the custom title bar; on macOS decorated, with a transparent
/// title bar over the content so the traffic lights sit in shrew's bar.
pub fn attributes(attrs: WindowAttributes, custom: bool) -> WindowAttributes {
    #[cfg(target_os = "macos")]
    let attrs = {
        use winit::platform::macos::WindowAttributesExtMacOS;
        attrs.with_titlebar_transparent(custom).with_title_hidden(custom).with_fullsize_content_view(custom)
    };
    #[cfg(not(target_os = "macos"))]
    let attrs = attrs.with_decorations(!custom);
    attrs
}

/// Carries out queued window actions and switches decorations when the feature changed. True when the window was
/// asked to close.
pub fn apply(st: &mut State, w: &Window) -> bool {
    let custom = on(&st.ed);
    // macOS keeps its decorations; the transparent title bar is only set when the window is created
    if !cfg!(target_os = "macos") && st.titlebar.decorated != Some(!custom) {
        if st.titlebar.decorated.is_some() {
            w.set_decorations(!custom);
        }
        st.titlebar.decorated = Some(!custom);
    }
    let mut close = false;
    for a in std::mem::take(&mut st.titlebar.actions) {
        match a {
            WindowAction::Minimize => w.set_minimized(true),
            WindowAction::ToggleMaximize => w.set_maximized(!w.is_maximized()),
            WindowAction::Close => close = true,
            WindowAction::Drag => {
                if let Err(e) = w.drag_window() {
                    log::warn!("cannot move the window: {e}");
                }
            }
            WindowAction::Resize(dir) => {
                if let Err(e) = w.drag_resize_window(dir) {
                    log::warn!("cannot resize the window: {e}");
                }
            }
        }
    }
    st.titlebar.maximized = w.is_maximized();
    close
}

/// The edge or corner of a `w` x `h` window under (`x`, `y`), within `border` pixels (`corner` along the edges).
pub fn resize_direction(x: f32, y: f32, w: f32, h: f32, border: f32, corner: f32) -> Option<ResizeDirection> {
    if x < 0.0 || y < 0.0 || x >= w || y >= h {
        return None;
    }
    let (left, right, top, bottom) = (x < border, x >= w - border, y < border, y >= h - border);
    // near a corner along either edge counts as the corner, which is easier to hit
    let (near_l, near_r, near_t, near_b) = (x < corner, x >= w - corner, y < corner, y >= h - corner);
    use ResizeDirection::*;
    Some(match () {
        _ if (top && near_l) || (left && near_t) => NorthWest,
        _ if (top && near_r) || (right && near_t) => NorthEast,
        _ if (bottom && near_l) || (left && near_b) => SouthWest,
        _ if (bottom && near_r) || (right && near_b) => SouthEast,
        _ if top => North,
        _ if bottom => South,
        _ if left => West,
        _ if right => East,
        _ => return None,
    })
}

/// Resizing from the edges: only shrew's own frame (undecorated), and not while maximized.
fn edge(tb: &TitleBar, lay: &Layout, custom: bool, x: f32, y: f32) -> Option<ResizeDirection> {
    if !custom || cfg!(target_os = "macos") || tb.maximized {
        return None;
    }
    resize_direction(x, y, lay.width, lay.height, BORDER * lay.scale, CORNER * lay.scale)
}

/// The item under the pointer, if it is on the bar; `Some(None)` for an empty part (or the room the tabs leave).
fn target(tb: &TitleBar, hits: &Hits, x: f32, y: f32) -> Option<Option<Item>> {
    if !tb.bar.contains(x, y) {
        return None;
    }
    if hits.tabs.iter().any(|(r, _)| r.contains(x, y)) {
        return Some(Some(Item::Tabs));
    }
    Some(tb.placed.iter().find(|(i, r)| *i != Item::Tabs && r.contains(x, y)).map(|p| p.0))
}

/// The pointer moved: hover states, the menu, a pending window drag (queued as [`WindowAction::Drag`]). Returns
/// (redraw, a cursor icon to show).
pub fn pointer_moved(st: &mut State, hits: &Hits, lay: &Layout, x: f32, y: f32) -> (bool, Option<CursorIcon>) {
    let custom = on(&st.ed);
    let tb = &mut st.titlebar;
    if let Some((px, py)) = tb.drag_from
        && (px - x).hypot(py - y) > DRAG_SLOP * lay.scale
    {
        tb.drag_from = None;
        tb.actions.push(WindowAction::Drag);
    }
    let mut redraw = false;
    let mut over_menu = false;
    if let Some(m) = &mut tb.menu {
        redraw |= m.hover(x, y);
        over_menu = m.hits.inside(x, y);
    }
    let hover = target(tb, hits, x, y).flatten().filter(|&i| i != Item::Tabs);
    if hover != tb.hover {
        tb.hover = hover;
        redraw = true;
    }
    let icon = edge(tb, lay, custom, x, y).map(CursorIcon::from);
    let icon = icon.or_else(|| (over_menu || tb.bar.contains(x, y)).then_some(CursorIcon::Default));
    (redraw, icon)
}

/// The pointer left the window: nothing is hovered.
pub fn pointer_left(st: &mut State) -> bool {
    let tb = &mut st.titlebar;
    tb.drag_from = None;
    tb.hover.take().is_some()
}

/// A left press. True when the title bar (or its menu, or a resize border) took it; a press on a tab falls through.
pub fn press(st: &mut State, hits: &Hits, lay: &Layout, x: f32, y: f32, now: Instant) -> bool {
    let custom = on(&st.ed);
    // an open menu takes the click: an entry runs, anywhere else (the menu button too) closes it, and the click
    // goes no further
    if let Some(m) = &mut st.titlebar.menu {
        match m.click(x, y) {
            Outcome::Run(cmd) => run(st, cmd),
            Outcome::Close => st.titlebar.menu = None,
            Outcome::Stay => {}
        }
        return true;
    }
    if let Some(dir) = edge(&st.titlebar, lay, custom, x, y) {
        st.titlebar.actions.push(WindowAction::Resize(dir));
        return true;
    }
    if !custom {
        return false;
    }
    let tb = &mut st.titlebar;
    match target(tb, hits, x, y) {
        None | Some(Some(Item::Tabs)) => false,
        Some(Some(item)) => {
            if item == Item::Menu {
                // menus open on press
                activate(st, item);
            } else {
                tb.pressed = Some(item);
            }
            true
        }
        Some(None) => {
            let double = tb.last_click.is_some_and(|(t, (px, py))| {
                now.duration_since(t) < DOUBLE_CLICK && (px - x).hypot(py - y) < DRAG_SLOP * 2.0 * lay.scale
            });
            if double {
                tb.last_click = None;
                tb.drag_from = None;
                tb.actions.push(WindowAction::ToggleMaximize);
            } else {
                tb.last_click = Some((now, (x, y)));
                tb.drag_from = Some((x, y));
            }
            true
        }
    }
}

/// A left release: a pressed button acts when the pointer is still on it. True when that changed the bar.
pub fn release(st: &mut State, hits: &Hits, x: f32, y: f32) -> bool {
    let tb = &mut st.titlebar;
    tb.drag_from = None;
    let Some(item) = tb.pressed.take() else { return false };
    if target(tb, hits, x, y) == Some(Some(item)) {
        activate(st, item);
    }
    true
}

/// What is under a point of the bar, for the mouse: `Some(None)` is an empty part (the room the tabs leave too).
pub fn hit(st: &State, hits: &Hits, x: f32, y: f32) -> Option<Option<Item>> {
    if !on(&st.ed) {
        return None;
    }
    target(&st.titlebar, hits, x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ResizeDirection::*;

    #[test]
    fn resize_borders_and_corners() {
        let d = |x, y| resize_direction(x, y, 800.0, 600.0, 5.0, 12.0);
        assert_eq!(d(400.0, 300.0), None);
        assert_eq!(d(400.0, 0.0), Some(North));
        assert_eq!(d(400.0, 599.0), Some(South));
        assert_eq!(d(0.0, 300.0), Some(West));
        assert_eq!(d(799.5, 300.0), Some(East));
        assert_eq!(d(400.0, 5.0), None, "the border is 5 pixels");
        // corners, also a little along each edge
        assert_eq!(d(1.0, 1.0), Some(NorthWest));
        assert_eq!(d(10.0, 2.0), Some(NorthWest));
        assert_eq!(d(2.0, 10.0), Some(NorthWest));
        assert_eq!(d(799.0, 0.0), Some(NorthEast));
        assert_eq!(d(0.0, 599.0), Some(SouthWest));
        assert_eq!(d(790.0, 598.0), Some(SouthEast));
        assert_eq!(d(-1.0, 300.0), None);
        assert_eq!(d(800.0, 300.0), None);
    }

    fn state() -> State {
        State::new(shrew_config::Loaded::default(), std::sync::Arc::new(|| {}), true)
    }

    /// A bar placed as a frame would: the menu button, a tab, and the close button.
    fn placed(st: &mut State) -> Hits {
        let tb = &mut st.titlebar;
        tb.bar = shrew_render::Rect::new(0.0, 0.0, 800.0, 30.0);
        tb.placed = vec![
            (Item::Menu, shrew_render::Rect::new(4.0, 0.0, 30.0, 30.0)),
            (Item::Tabs, shrew_render::Rect::new(40.0, 0.0, 600.0, 30.0)),
            (Item::Close, shrew_render::Rect::new(760.0, 0.0, 40.0, 30.0)),
        ];
        Hits { tabs: vec![(shrew_render::Rect::new(40.0, 4.0, 100.0, 26.0), 0)], ..Hits::default() }
    }

    fn lay() -> Layout {
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
        Layout::new(&m, (800, 600), 1.0, 10, true, true)
    }

    #[test]
    fn hit_testing_the_bar() {
        let mut st = state();
        let hits = placed(&mut st);
        let tb = &st.titlebar;
        assert_eq!(target(tb, &hits, 10.0, 10.0), Some(Some(Item::Menu)));
        assert_eq!(target(tb, &hits, 50.0, 10.0), Some(Some(Item::Tabs)), "on a tab");
        assert_eq!(target(tb, &hits, 300.0, 10.0), Some(None), "the room the tabs leave is empty bar");
        assert_eq!(target(tb, &hits, 36.0, 10.0), Some(None), "between items");
        assert_eq!(target(tb, &hits, 780.0, 29.0), Some(Some(Item::Close)));
        assert_eq!(target(tb, &hits, 300.0, 40.0), None, "below the bar");
    }

    #[test]
    fn buttons_act_on_release_over_them() {
        let mut st = state();
        let hits = placed(&mut st);
        let (lay, now) = (lay(), Instant::now());
        assert!(press(&mut st, &hits, &lay, 780.0, 15.0, now));
        assert_eq!(st.titlebar.pressed, Some(Item::Close));
        // released elsewhere: nothing
        assert!(release(&mut st, &hits, 300.0, 15.0));
        assert!(st.titlebar.actions.is_empty());
        press(&mut st, &hits, &lay, 780.0, 15.0, now);
        release(&mut st, &hits, 781.0, 16.0);
        assert_eq!(st.titlebar.actions, vec![WindowAction::Close]);
        // a press on a tab is the app's
        assert!(!press(&mut st, &hits, &lay, 50.0, 15.0, now));
        // the menu button opens the menu on press; a click outside closes it and goes no further
        press(&mut st, &hits, &lay, 10.0, 15.0, now);
        assert!(st.titlebar.menu.is_some());
        assert!(press(&mut st, &hits, &lay, 400.0, 300.0, now));
        assert!(st.titlebar.menu.is_none());
    }

    #[test]
    fn double_click_on_an_empty_part_maximizes() {
        let mut st = state();
        let hits = placed(&mut st);
        let (lay, t) = (lay(), Instant::now());
        assert!(press(&mut st, &hits, &lay, 300.0, 15.0, t));
        release(&mut st, &hits, 300.0, 15.0);
        assert!(st.titlebar.actions.is_empty());
        press(&mut st, &hits, &lay, 301.0, 15.0, t + Duration::from_millis(200));
        assert_eq!(st.titlebar.actions, vec![WindowAction::ToggleMaximize]);
        // too slow: two single clicks
        st.titlebar.actions.clear();
        press(&mut st, &hits, &lay, 300.0, 15.0, t + Duration::from_secs(2));
        press(&mut st, &hits, &lay, 300.0, 15.0, t + Duration::from_secs(3));
        assert!(st.titlebar.actions.is_empty());
    }

    #[test]
    fn resizing_needs_an_undecorated_window_that_is_not_maximized() {
        let mut st = state();
        let lay = lay();
        let tb = &mut st.titlebar;
        assert_eq!(edge(tb, &lay, true, 400.0, 599.0), if cfg!(target_os = "macos") { None } else { Some(South) });
        assert_eq!(edge(tb, &lay, false, 400.0, 599.0), None);
        tb.maximized = true;
        assert_eq!(edge(tb, &lay, true, 400.0, 599.0), None);
    }
}
