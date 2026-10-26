//! The mouse without a window: the pointer, drags, click counting, drag autoscroll, presses and releases of every
//! button, motion, the wheel and the pointer's shape. The windowed app translates winit's events into calls here and
//! carries out what comes back; headless runs (`--keys` with mouse tokens, see [`parse_input`]) and tests go
//! through the same code, so what the mouse does can be screenshotted and tested.
//!
//! Every call returns a [`Response`]: draw again, the editor changed (the app restarts the cursor blink and syncs
//! the input method), a new pointer shape. Window work (moving it by the title bar, resizing it from an edge,
//! maximizing) is queued in `st.titlebar.actions` as `titlebar::WindowAction`s, which the app carries out.
//!
//! [`target_at`] says what is under a point ([`Target`]: the text, a tab, a sidebar row, the gutter...). A right
//! press opens that target's context menu (see [`crate::context_menu::for_target`]); while a menu is open, the
//! mouse goes to it first.

use crate::draw::Ui;
use crate::layout::{Layout, Scroll};
use crate::state::State;
use shrew_core::{Key, KeyPress, Mode, Mods, MouseButton, text};
use std::time::{Duration, Instant};
use winit::window::CursorIcon;

/// Clicks closer together than this (and than [`DOUBLE_CLICK_PX`]) count as double/triple clicks.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);
pub const DOUBLE_CLICK_PX: f32 = 5.0;
/// The pointer moves this far (logical pixels) with a button held before it counts as a drag.
pub const DRAG_SLOP: f32 = 3.0;
const AUTOSCROLL_TICK: Duration = Duration::from_millis(16);
/// Lines per mouse-wheel notch.
pub const WHEEL_LINES: f32 = 3.0;

/// What is under a point: what a right click opens a menu for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(all(feature = "terminal", feature = "git")), allow(dead_code))]
pub enum Target {
    /// The text, at this char position (the caret position nearest the pointer).
    Text(usize),
    /// The gutter (line numbers, change markers) beside this line (0-based).
    Gutter(usize),
    /// A buffer tab.
    Tab(usize),
    /// The empty part of the tab row (its own row, with the system title bar).
    TabStrip,
    /// An empty part of the custom title bar (the room the tabs leave too).
    TitleBar,
    /// The sidebar's file tree: a row (an index into the tree's rows), or its empty part.
    Sidebar(Option<usize>),
    /// The terminal panel.
    Terminal,
    /// A diff tab: the row under the pointer.
    Diff(Option<usize>),
    Panel(Panel),
    StatusBar,
    Minimap,
    Scrollbar,
    Nothing,
}

/// Lists that are not the text: git's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub enum Panel {
    /// The Source Control panel: a row (a section header or a file), or its empty part.
    Git(Option<usize>),
    /// A log tab: a commit row, or elsewhere on the tab.
    Log(Option<usize>),
}

/// The wheel, as the window reports it: (sideways, vertical), positive y away from the user (scrolls up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wheel {
    /// Notches of a mouse wheel.
    Lines(f32, f32),
    /// Pixels of a touchpad.
    Pixels(f32, f32),
}

/// What the window must do after a mouse event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[must_use]
pub struct Response {
    /// Draw again (a hover state, a drag, a scroll).
    pub redraw: bool,
    /// The editor changed: draw, restart the cursor blink, sync the input method and the window.
    pub input: bool,
    /// The pointer's new shape, when it changed.
    pub icon: Option<CursorIcon>,
}

impl Response {
    pub const NONE: Response = Response { redraw: false, input: false, icon: None };
    pub const REDRAW: Response = Response { redraw: true, input: false, icon: None };
    pub const INPUT: Response = Response { redraw: true, input: true, icon: None };

    fn redraw_if(on: bool) -> Response {
        if on { Response::REDRAW } else { Response::NONE }
    }
}

/// What a held left button is dragging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    None,
    Text,
    Scrollbar,
    /// The sidebar's right edge (resizing it).
    Sidebar,
    Minimap,
    /// A slider in the Settings panel.
    Settings,
    // ----- git -----
    /// Rows or the scrollbar of a diff tab.
    #[cfg(feature = "git")]
    Diff,
    // -----
}

pub struct Mouse {
    /// Where the pointer is (physical pixels).
    pub pointer: (f32, f32),
    /// The modifier keys held (the app keeps them current).
    pub mods: Mods,
    drag: Drag,
    clicks: Option<(Instant, (f32, f32), u8)>,
    autoscroll: Option<Instant>,
    icon: CursorIcon,
}

impl Default for Mouse {
    fn default() -> Mouse {
        Mouse::new()
    }
}

impl Mouse {
    pub fn new() -> Mouse {
        Mouse {
            pointer: (0.0, 0.0),
            mods: Mods::default(),
            drag: Drag::None,
            clicks: None,
            autoscroll: None,
            icon: CursorIcon::Default,
        }
    }

    /// The pointer shape last chosen.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn icon(&self) -> CursorIcon {
        self.icon
    }

    /// The window set this shape itself.
    pub fn set_icon(&mut self, icon: CursorIcon) {
        self.icon = icon;
    }

    /// When the drag autoscroll wants the next [`Mouse::tick`].
    pub fn deadline(&self) -> Option<Instant> {
        self.autoscroll
    }

    /// The new icon, when it differs from the one shown.
    fn shape(&mut self, icon: CursorIcon) -> Option<CursorIcon> {
        (icon != self.icon).then(|| {
            self.icon = icon;
            icon
        })
    }

    // ----- buttons -----

    /// A button went down at (`x`, `y`).
    #[allow(clippy::too_many_arguments)]
    pub fn press(
        &mut self,
        st: &mut State,
        ui: &mut Ui,
        lay: Option<Layout>,
        x: f32,
        y: f32,
        button: MouseButton,
        now: Instant,
    ) -> Response {
        self.pointer = (x, y);
        if st.ed.mapping_pending() {
            st.ed.flush_pending_mappings();
        }
        // the terminal panel takes presses over it; a click elsewhere gives the editor the focus back and goes on
        #[cfg(feature = "terminal")]
        {
            let modal = st.ed.picker.is_some() || st.ed.about.is_some() || st.settings.is_open() || st.menu_open();
            if !modal && st.terminal_press(x, y, button, self.mods) {
                return Response::INPUT;
            }
        }
        match button {
            MouseButton::Left => self.left_press(st, ui, lay, x, y, now),
            MouseButton::Right => self.right_press(st, ui, lay, x, y),
            MouseButton::Middle => self.middle_press(st),
        }
    }

    pub fn release(
        &mut self,
        st: &mut State,
        ui: &mut Ui,
        lay: Option<Layout>,
        x: f32,
        y: f32,
        button: MouseButton,
    ) -> Response {
        self.pointer = (x, y);
        if st.ed.mapping_pending() {
            st.ed.flush_pending_mappings();
        }
        #[cfg(feature = "terminal")]
        st.terminal_release(button, self.mods);
        // a release on the open menu's item runs it (after a click on it, or a drag from the press that opened it)
        let slop = DRAG_SLOP * lay.map_or(1.0, |l| l.scale);
        let mut menu = false;
        if let Some(m) = &mut st.menu {
            let out = m.release(x, y, button, slop);
            menu = !matches!(out, crate::context_menu::Outcome::Stay);
            crate::context_menu::apply(st, out);
        }
        if button != MouseButton::Left {
            return if menu { Response::INPUT } else { Response::NONE };
        }
        if self.drag == Drag::Settings {
            st.settings_release();
        }
        if self.drag == Drag::Text {
            st.ed.mouse_up();
            st.after_input();
        }
        // a title bar button acts when released over it
        crate::titlebar::release(st, &ui.hits, x, y);
        if self.drag == Drag::Sidebar {
            st.persist_sidebar_width();
        }
        // ----- git -----
        #[cfg(feature = "git")]
        if self.drag == Drag::Diff {
            crate::git::diffview::release(st);
        }
        // -----
        self.drag = Drag::None;
        self.autoscroll = None;
        Response::INPUT
    }

    fn left_press(
        &mut self,
        st: &mut State,
        ui: &mut Ui,
        lay: Option<Layout>,
        x: f32,
        y: f32,
        now: Instant,
    ) -> Response {
        let Some(lay) = lay else { return Response::NONE };
        // an open context menu takes the click: on it an item runs on release, anywhere else closes it, and the
        // click goes no further
        if let Some(m) = &mut st.menu {
            let out = m.press(x, y, MouseButton::Left);
            crate::context_menu::apply(st, out);
            return Response::INPUT;
        }
        // the title bar, its menus and the window's resize borders come first
        if crate::titlebar::press(st, &ui.hits, &lay, x, y, now) {
            return Response::INPUT;
        }
        let hits = &ui.hits;
        // the About panel is modal too: a click on a link opens it, a click outside closes the panel
        if st.ed.about.is_some() {
            if let Some((_, url)) = hits.about_links.iter().find(|(r, _)| r.contains(x, y)) {
                if let Err(e) = crate::open::url(url) {
                    st.ed.error(format!("cannot open {url}: {e}"));
                }
            } else if !hits.about.is_some_and(|r| r.contains(x, y)) {
                st.key(KeyPress::plain(Key::Esc));
            }
            return Response::INPUT;
        }
        // the picker is modal: a click picks a row or closes it
        if st.ed.picker.is_some() {
            let row = hits.picker_rows.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1);
            let inside = hits.picker.is_some_and(|r| r.contains(x, y));
            if let Some(row) = row {
                if let Some(p) = &mut st.ed.picker {
                    p.selected = row;
                }
                st.key(KeyPress::plain(Key::Enter));
            } else if !inside {
                st.key(KeyPress::plain(Key::Esc));
            }
            return Response::INPUT;
        }
        // the Settings panel is modal: a click outside closes it
        if st.settings.is_open() {
            if st.settings_click(x, y) {
                self.drag = Drag::Settings;
            }
            st.after_input();
            return Response::INPUT;
        }
        // ----- git ----- its toast, the panel and its tabs, a log tab, blame and conflict actions
        #[cfg(feature = "git")]
        if crate::git::press(st, x, y) {
            st.after_input();
            return Response::INPUT;
        }
        let hits = &ui.hits;
        if hits.gear.is_some_and(|r| r.contains(x, y)) {
            st.ed.run(shrew_core::Command::Settings);
            st.after_input();
            return Response::INPUT;
        }
        if hits.update.is_some_and(|r| r.contains(x, y)) {
            st.activate_update();
            return Response::INPUT;
        }
        if hits.sidebar.edge.is_some_and(|r| r.contains(x, y)) {
            self.drag = Drag::Sidebar;
            return Response::INPUT;
        }
        if crate::sidebar::press(st, &ui.hits.sidebar, x, y) {
            return Response::INPUT;
        }
        if let Some(&(_, i)) = ui.hits.tabs.iter().find(|(r, _)| r.contains(x, y)) {
            st.ed.switch_to(i);
            st.after_input();
            return Response::INPUT;
        }
        // ----- git: a diff tab in the editor's place; the hunk peek's buttons, gutter markers, the branch in
        // the status bar -----
        #[cfg(feature = "git")]
        {
            if crate::git::diffview::press(st, x, y, self.mods, now) {
                self.drag = Drag::Diff;
                st.after_input();
                return Response::INPUT;
            }
            if crate::git::press_text(st, &ui.hits.git, &lay, ui.anim.scroll, x, y) {
                return Response::INPUT;
            }
        }
        if crate::minimap::press(st, ui, &lay, x, y) {
            self.drag = Drag::Minimap;
            return Response::INPUT;
        }
        if let Some((track, total)) = ui.hits.scrollbar
            && track.contains(x, y)
        {
            self.drag = Drag::Scrollbar;
            scroll_to_pointer(st, ui, &lay, track, total, y);
            return Response::INPUT;
        }
        if !lay.text.contains(x, y) {
            return Response::NONE;
        }
        // a click in the text leaves the command line, as in vim
        if st.ed.prompt.is_some() {
            st.key(KeyPress::plain(Key::Esc));
        }
        // click counting: double = word, triple = line
        let n = match self.clicks {
            Some((t, (px, py), n))
                if now.saturating_duration_since(t) < DOUBLE_CLICK
                    && (px - x).hypot(py - y) < DOUBLE_CLICK_PX * lay.scale =>
            {
                (n % 3) + 1
            }
            _ => 1,
        };
        self.clicks = Some((now, (x, y), n));
        let in_gutter = x < lay.text_x();
        let ed = &st.ed;
        let caret = ed.mode == Mode::Insert || !ed.vim();
        let (pos, n) = if in_gutter {
            // a click on a line number selects the line
            (pos_at(st, ui, &lay, lay.text_x(), y, true), 3)
        } else {
            (pos_at(st, ui, &lay, x, y, caret), n)
        };
        let Some(pos) = pos else { return Response::NONE };
        st.ed.sidebar_focus = false;
        st.ed.mouse_down(pos, n, self.mods);
        st.after_input();
        self.drag = Drag::Text;
        Response::INPUT
    }

    /// A right press: an open menu takes it (a press on it starts a release-to-select, the same spot again closes
    /// it, anywhere else closes it and opens the menu there); else the menu of what is under the pointer opens.
    fn right_press(&mut self, st: &mut State, ui: &mut Ui, lay: Option<Layout>, x: f32, y: f32) -> Response {
        let Some(lay) = lay else { return Response::NONE };
        let slop = DRAG_SLOP * lay.scale;
        if let Some(m) = &mut st.menu {
            if m.inside(x, y) {
                m.hover(x, y);
                m.hold(MouseButton::Right, (x, y), false);
                return Response::REDRAW;
            }
            let again = (m.at.0 - x).hypot(m.at.1 - y) <= 2.0 * slop;
            st.menu = None;
            if again {
                return Response::INPUT;
            }
        }
        if st.titlebar.menu.take().is_some() {
            return Response::INPUT;
        }
        if !self.open_menu(st, ui, &lay, x, y) {
            return Response::NONE;
        }
        // holding the button: releasing it on an item runs the item
        if let Some(m) = &mut st.menu {
            m.hold(MouseButton::Right, (x, y), true);
        }
        Response::INPUT
    }

    /// Opens the menu of what is at (`x`, `y`). True when one opened (or the press was taken).
    fn open_menu(&mut self, st: &mut State, ui: &Ui, lay: &Layout, x: f32, y: f32) -> bool {
        // ----- git ----- the Source Control panel selects the row first
        #[cfg(feature = "git")]
        if crate::git::right_press(st, x, y) {
            return true;
        }
        let target = target_at(st, ui, lay, x, y);
        crate::context_menu::for_target(st, target, (x, y))
    }

    /// A middle press closes an open menu (and goes no further).
    fn middle_press(&mut self, st: &mut State) -> Response {
        if st.menu_open() {
            st.menu = None;
            st.titlebar.menu = None;
            return Response::INPUT;
        }
        Response::NONE
    }

    // ----- motion -----

    pub fn moved(
        &mut self,
        st: &mut State,
        ui: &mut Ui,
        lay: Option<Layout>,
        x: f32,
        y: f32,
        now: Instant,
    ) -> Response {
        self.pointer = (x, y);
        let Some(lay) = lay else { return Response::NONE };
        let mut redraw = false;
        match self.drag {
            Drag::Text => {
                if let Some(pos) = pos_at(st, ui, &lay, x, y, true) {
                    st.ed.mouse_drag(pos);
                    redraw = true;
                }
                // past the top or bottom edge: keep scrolling while the button is held
                let outside = y < lay.text.y || y >= lay.text.bottom();
                self.autoscroll = if outside { Some(self.autoscroll.unwrap_or(now)) } else { None };
            }
            Drag::Scrollbar => {
                if let Some((track, total)) = ui.hits.scrollbar {
                    scroll_to_pointer(st, ui, &lay, track, total, y);
                    redraw = true;
                }
            }
            Drag::Sidebar => {
                crate::sidebar::drag_to(st, x, lay.scale);
                redraw = true;
            }
            Drag::Minimap => redraw |= crate::minimap::drag(st, ui, &lay, y),
            Drag::Settings => {
                st.settings_drag(x);
                st.after_input();
                redraw = true;
            }
            // ----- git -----
            #[cfg(feature = "git")]
            Drag::Diff => redraw |= crate::git::diffview::drag(st, x, y),
            // -----
            Drag::None => {}
        }
        let (bar, bar_icon) = crate::titlebar::pointer_moved(st, &ui.hits, &lay, x, y);
        let on_menu = st.menu.as_ref().is_some_and(|m| m.inside(x, y));
        let menu = st.menu.as_mut().is_some_and(|m| m.hover(x, y));
        let map = crate::minimap::hover(ui, &lay, Some((x, y)));
        #[cfg(feature = "git")]
        let (git_redraw, on_git) = crate::git::hover(st, x, y);
        #[cfg(not(feature = "git"))]
        let (git_redraw, on_git) = (false, false);
        redraw |= bar || menu || map || git_redraw;
        let on_edge = st.ed.picker.is_none() && ui.hits.sidebar.edge.is_some_and(|r| r.contains(x, y));
        let on_link = st.ed.about.is_some() && ui.hits.about_links.iter().any(|(r, _)| r.contains(x, y));
        // ----- git -----
        #[cfg(feature = "git")]
        let (on_link, diff) = (
            on_link || (st.ed.picker.is_none() && crate::git::diffview::hover(st, x, y)),
            crate::git::diffview::is_active(st),
        );
        #[cfg(not(feature = "git"))]
        let diff = false;
        // -----
        let icon = if on_menu {
            CursorIcon::Default
        } else if let Some(icon) = bar_icon {
            icon
        } else if on_link || on_git {
            CursorIcon::Pointer
        } else if on_edge || self.drag == Drag::Sidebar {
            CursorIcon::ColResize
        } else if st.ed.picker.is_none()
            && st.ed.about.is_none()
            && !st.settings.is_open()
            && !diff
            && lay.text_rect().contains(x, y)
            && !ui.hits.scrollbar.is_some_and(|(t, _)| t.contains(x, y))
        {
            CursorIcon::Text
        } else {
            CursorIcon::Default
        };
        #[cfg(feature = "terminal")]
        let icon = match self.drag {
            Drag::None if st.ed.picker.is_none() && st.ed.about.is_none() && !on_menu => {
                let (over, r) = st.terminal_motion(x, y, self.mods);
                redraw |= r;
                over.unwrap_or(icon)
            }
            _ => icon,
        };
        Response { redraw, input: false, icon: self.shape(icon) }
    }

    /// The pointer left the window: nothing is hovered.
    pub fn left(&mut self, st: &mut State, ui: &mut Ui, lay: Option<Layout>) -> Response {
        // both run: each clears its own hover state
        let bar = crate::titlebar::pointer_left(st);
        let map = lay.is_some_and(|lay| crate::minimap::hover(ui, &lay, None));
        Response::redraw_if(bar | map)
    }

    /// The drag autoscroll's timer: scrolls on while the pointer is past the text's top or bottom edge.
    pub fn tick(&mut self, st: &mut State, ui: &mut Ui, lay: Option<Layout>, now: Instant) -> Response {
        if self.autoscroll.is_none_or(|t| now < t) {
            return Response::NONE;
        }
        let Some(lay) = lay else { return Response::NONE };
        let (x, y) = self.pointer;
        let dist = if y < lay.text.y { y - lay.text.y } else { y - lay.text.bottom() + 1.0 };
        if self.drag != Drag::Text || dist.abs() < 0.5 {
            self.autoscroll = None;
            return Response::NONE;
        }
        // faster the further away the pointer is
        let lines = (dist / lay.ch).clamp(-10.0, 10.0);
        let lines = if lines < 0.0 { lines.floor() } else { lines.ceil() } as isize;
        st.ed.scroll(lines, 0);
        ui.frac = 0.0;
        let ty = y.clamp(lay.text.y, lay.text.bottom() - 1.0);
        let scroll = ui.target_scroll(&st.ed);
        ui.anim.set_scroll(scroll);
        if let Some(pos) = pos_at(st, ui, &lay, x, ty, true) {
            st.ed.mouse_drag(pos);
        }
        self.autoscroll = Some(now + AUTOSCROLL_TICK);
        Response::REDRAW
    }

    // ----- the wheel -----

    pub fn wheel(&mut self, st: &mut State, ui: &mut Ui, lay: Option<Layout>, delta: Wheel) -> Response {
        let Some(lay) = lay else { return Response::NONE };
        if st.menu_open() {
            return Response::NONE; // nothing scrolls under an open menu
        }
        // notches times `per_notch`, or pixels over `per_pixel`
        let dy = |per_notch: f32, per_pixel: f32| match delta {
            Wheel::Lines(_, y) => -y * per_notch,
            Wheel::Pixels(_, y) => -y / per_pixel,
        };
        if let Some(p) = &mut st.ed.picker {
            let dy = dy(1.0, lay.ch);
            if dy.abs() >= 0.5 {
                p.move_by(dy.round() as isize);
                return Response::REDRAW;
            }
            return Response::NONE;
        }
        if st.settings.is_open() {
            st.settings_wheel(dy(WHEEL_LINES * lay.ch, 1.0));
            return Response::REDRAW;
        }
        if let Some(top) = &mut st.ed.about {
            let dy = dy(WHEEL_LINES, lay.ch);
            if dy.abs() >= 0.5 {
                *top = top.saturating_add_signed(dy.round() as isize);
                return Response::REDRAW;
            }
            return Response::NONE;
        }
        let (px, py) = self.pointer;
        #[cfg(feature = "terminal")]
        if st.terminal_wheel(-dy(WHEEL_LINES, lay.ch), px, py, self.mods) {
            return Response::REDRAW;
        }
        let lines = dy(WHEEL_LINES, lay.ch);
        #[cfg(feature = "git")]
        if crate::git::wheel(st, px, py, lines) {
            return Response::REDRAW;
        }
        if crate::sidebar::wheel(st, &ui.hits.sidebar, px, py, lines) {
            return Response::REDRAW;
        }
        // Ctrl+wheel zooms, like Ctrl+= / Ctrl+-
        if self.mods.ctrl {
            let up = -dy(1.0, lay.ch);
            if up.abs() >= 0.5 {
                let cmd = if up > 0.0 { shrew_core::Command::FontBigger } else { shrew_core::Command::FontSmaller };
                st.ed.run(cmd);
                st.after_input();
                return Response::REDRAW;
            }
            return Response::NONE;
        }
        // ----- git: a diff tab scrolls its rows -----
        #[cfg(feature = "git")]
        {
            let cols = match delta {
                Wheel::Lines(x, _) => -x * WHEEL_LINES,
                Wheel::Pixels(x, _) => -x / lay.cw,
            };
            if crate::git::diffview::wheel(st, px, py, lines, cols, self.mods.shift) {
                return Response::REDRAW;
            }
        }
        // -----
        match delta {
            Wheel::Lines(x, y) => {
                ui.frac = 0.0;
                st.ed.scroll((-y * WHEEL_LINES).round() as isize, (-x * WHEEL_LINES).round() as isize);
            }
            Wheel::Pixels(dx, dy) => {
                // touchpads: follow the fingers pixel by pixel
                let v = st.ed.buf().view;
                let cur = v.top as f32 + if ui.frac_top == v.top { ui.frac } else { 0.0 };
                let last = text::last_line(st.ed.buf().rope()) as f32;
                let to = (cur - dy / lay.ch).clamp(0.0, last);
                let top = to.floor();
                st.ed.scroll(top as isize - v.top as isize, 0);
                ui.frac = to - top;
                ui.frac_top = st.ed.buf().view.top;
                let cols = (-dx / lay.cw).round() as isize;
                if cols != 0 {
                    st.ed.scroll(0, cols);
                }
                let s = ui.target_scroll(&st.ed);
                ui.anim.set_scroll(Scroll { top: s.top, left: ui.anim.scroll.left });
            }
        }
        Response::REDRAW
    }

    // ----- headless input -----

    /// Feeds one parsed mouse [`Event`] (logical pixels, times `scale`), as the window would deliver it.
    pub fn feed(
        &mut self,
        st: &mut State,
        ui: &mut Ui,
        lay: Option<Layout>,
        ev: &Event,
        scale: f32,
        now: Instant,
    ) -> Response {
        self.mods = ev.mods;
        let (x, y) = (ev.x * scale, ev.y * scale);
        // a drag past the text's edge scrolls once per event (time is made up here)
        if self.deadline().is_some_and(|t| now >= t) {
            let _ = self.tick(st, ui, lay, now);
        }
        match ev.kind {
            Kind::Move => self.moved(st, ui, lay, x, y, now),
            Kind::Press(b) => self.press(st, ui, lay, x, y, b, now),
            Kind::Release(b) => self.release(st, ui, lay, x, y, b),
            Kind::Wheel(dx, dy) => self.wheel(st, ui, lay, Wheel::Lines(-dx, -dy)),
        }
    }
}

/// The char position under the pointer. `round`: the nearest caret position; else the char under it.
pub fn pos_at(st: &State, ui: &Ui, lay: &Layout, x: f32, y: f32, round: bool) -> Option<usize> {
    let scroll = ui.anim.scroll;
    let rope = &st.ed.buf().doc.rope;
    let last = text::last_line(rope);
    let line = scroll.line_at(lay, y).clamp(0, last as isize) as usize;
    let col = scroll.col_at(lay, x).max(0.0);
    let col = if round { col.round() } else { col.floor() } as usize;
    Some(text::pos_at_col(rope, line, col, st.ed.settings.tab_width.max(1)))
}

fn scroll_to_pointer(st: &mut State, ui: &mut Ui, lay: &Layout, track: shrew_render::Rect, total: usize, y: f32) {
    let f = ((y - track.y) / track.h.max(1.0)).clamp(0.0, 1.0);
    // the thumb centres on the pointer
    let rows = lay.rows as f32;
    let target = (f * total as f32 - rows / 2.0).clamp(0.0, total.saturating_sub(1) as f32) as isize;
    let top = st.ed.buf().view.top as isize;
    st.ed.scroll(target - top, 0);
    ui.frac = 0.0;
}

/// What is under (`x`, `y`) in the last frame. Under a modal panel (picker, About, Settings): nothing.
pub fn target_at(st: &State, ui: &Ui, lay: &Layout, x: f32, y: f32) -> Target {
    if st.ed.picker.is_some() || st.ed.about.is_some() || st.settings.is_open() {
        return Target::Nothing;
    }
    let tab = |x: f32, y: f32| ui.hits.tabs.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1);
    if let Some(hit) = crate::titlebar::hit(st, &ui.hits, x, y) {
        return match (hit, tab(x, y)) {
            (None, _) => Target::TitleBar,
            (Some(crate::titlebar::items::Item::Tabs), Some(i)) => Target::Tab(i),
            _ => Target::Nothing,
        };
    }
    #[cfg(feature = "terminal")]
    if st.terminals.geom.is_some_and(|g| g.panel.contains(x, y)) {
        return Target::Terminal;
    }
    #[cfg(feature = "git")]
    {
        let p = &st.git.panel.hits;
        if crate::git::panel::draw::shown(st) && p.area.is_some_and(|a| a.contains(x, y)) {
            return Target::Panel(Panel::Git(p.rows.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1)));
        }
    }
    let side = &ui.hits.sidebar;
    if side.area.is_some_and(|a| a.contains(x, y)) {
        return Target::Sidebar(side.rows.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1));
    }
    if let Some(i) = tab(x, y) {
        return Target::Tab(i);
    }
    if !lay.top_bar && lay.tabs.contains(x, y) {
        return Target::TabStrip;
    }
    // ----- git: a diff or log tab in the editor's place -----
    #[cfg(feature = "git")]
    {
        if crate::git::diffview::is_active(st) && lay.text.contains(x, y) {
            return Target::Diff(crate::git::diffview::row_under(st, x, y));
        }
        if let Some(row) = crate::git::log::row_under(st, x, y) {
            return Target::Panel(Panel::Log(row));
        }
    }
    if lay.minimap.contains(x, y) {
        return Target::Minimap;
    }
    if ui.hits.scrollbar.is_some_and(|(t, _)| t.contains(x, y)) {
        return Target::Scrollbar;
    }
    if lay.text.contains(x, y) {
        if x < lay.text_x() {
            let last = text::last_line(st.ed.buf().rope());
            return Target::Gutter(ui.anim.scroll.line_at(lay, y).clamp(0, last as isize) as usize);
        }
        return pos_at(st, ui, lay, x, y, true).map_or(Target::Nothing, Target::Text);
    }
    if lay.status.contains(x, y) {
        return Target::StatusBar;
    }
    Target::Nothing
}

// ----- mouse tokens in `--keys` -----

/// One step of `--keys`: a key, or a mouse event.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Key(KeyPress),
    Mouse(Event),
}

/// A mouse event at (`x`, `y`), logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    pub kind: Kind,
    pub x: f32,
    pub y: f32,
    pub mods: Mods,
    /// Time since the previous event (a token's first event comes a second after the one before, so separate
    /// clicks never make a double click; the clicks of `<dclick>` come 40 ms apart).
    pub after: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Move,
    Press(MouseButton),
    Release(MouseButton),
    /// Wheel notches: (right, down).
    Wheel(f32, f32),
}

/// Parses `--keys`: vim key notation with mouse tokens in it, in logical pixels of the image:
/// `<click:X,Y>` `<dclick:X,Y>` `<tclick:X,Y>` (left, double, triple), `<rclick:X,Y>` `<mclick:X,Y>` (right,
/// middle), `<move:X,Y>`, `<down:X,Y>` `<up:X,Y>` (left button; `rdown`/`rup`, `mdown`/`mup` for the others),
/// `<drag:X1,Y1,X2,Y2>` `<rdrag:X1,Y1,X2,Y2>` (press, move in steps, release), `<wheel:X,Y,DY[,DX]>` (notches,
/// positive down/right). Modifiers go in front as in vim: `<C-click:X,Y>`, `<S-click:..>`, `<A-..>` (or `M-`),
/// `<D-..>` (Super).
pub fn parse_input(s: &str) -> Result<Vec<Input>, String> {
    let mut out = Vec::new();
    let mut keys_from = 0;
    let mut i = 0;
    let flush = |out: &mut Vec<Input>, keys: &str| out.extend(KeyPress::parse_seq(keys).into_iter().map(Input::Key));
    while i < s.len() {
        let rest = &s[i..];
        if rest.starts_with('<')
            && let Some(end) = rest.find('>')
            && let Some(events) = mouse_token(&rest[1..end])?
        {
            flush(&mut out, &s[keys_from..i]);
            out.extend(events.into_iter().map(Input::Mouse));
            i += end + 1;
            keys_from = i;
            continue;
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    flush(&mut out, &s[keys_from..]);
    Ok(out)
}

/// The events of one `<...>` token; `None` when it is not a mouse token (a key like `<C-p>`).
fn mouse_token(tok: &str) -> Result<Option<Vec<Event>>, String> {
    let Some((head, args)) = tok.split_once(':') else { return Ok(None) };
    let mut mods = Mods::default();
    let mut name = head;
    while let Some((m, rest)) = name.split_once('-') {
        match m {
            "C" | "c" => mods.ctrl = true,
            "S" | "s" => mods.shift = true,
            "A" | "a" | "M" | "m" => mods.alt = true,
            "D" | "d" => mods.logo = true,
            _ => return Ok(None),
        }
        name = rest;
    }
    use MouseButton::{Left, Middle, Right};
    let (button, clicks) = match name {
        "click" => (Left, 1),
        "dclick" => (Left, 2),
        "tclick" => (Left, 3),
        "rclick" => (Right, 1),
        "mclick" => (Middle, 1),
        "move" | "down" | "up" | "rdown" | "rup" | "mdown" | "mup" | "drag" | "rdrag" | "wheel" => (Left, 0),
        _ => return Ok(None),
    };
    let nums: Vec<f32> = args
        .split(',')
        .map(|n| n.trim().parse::<f32>().map_err(|_| format!("<{tok}>: {n:?} is not a number")))
        .collect::<Result<_, _>>()?;
    let want = match name {
        "drag" | "rdrag" => 4..=4,
        "wheel" => 3..=4,
        _ => 2..=2,
    };
    if !want.contains(&nums.len()) {
        return Err(format!("<{tok}>: expected {} numbers", if name == "wheel" { "3 or 4" } else { "2 or 4" }));
    }
    let (x, y) = (nums[0], nums[1]);
    let first = Duration::from_secs(1);
    let soon = Duration::from_millis(40);
    let ev = |kind, x, y, after| Event { kind, x, y, mods, after };
    let mut out = vec![ev(Kind::Move, x, y, first)];
    let mut at = |kind| out.push(ev(kind, x, y, soon));
    match name {
        "move" => {}
        "down" => at(Kind::Press(Left)),
        "up" => at(Kind::Release(Left)),
        "rdown" => at(Kind::Press(Right)),
        "rup" => at(Kind::Release(Right)),
        "mdown" => at(Kind::Press(Middle)),
        "mup" => at(Kind::Release(Middle)),
        "wheel" => at(Kind::Wheel(nums.get(3).copied().unwrap_or(0.0), nums[2])),
        "drag" | "rdrag" => {
            let b = if name == "drag" { Left } else { Right };
            let (x2, y2) = (nums[2], nums[3]);
            at(Kind::Press(b));
            const STEPS: usize = 8;
            for k in 1..=STEPS {
                let t = k as f32 / STEPS as f32;
                out.push(ev(Kind::Move, x + (x2 - x) * t, y + (y2 - y) * t, soon));
            }
            out.push(ev(Kind::Release(b), x2, y2, soon));
        }
        _ => {
            for _ in 0..clicks {
                at(Kind::Press(button));
                at(Kind::Release(button));
            }
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests;
