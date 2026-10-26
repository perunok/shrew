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
/// A late tick (a slow frame) scrolls for this long at most, so the view never leaps.
const AUTOSCROLL_MAX_DT: Duration = Duration::from_millis(50);
/// Drag autoscroll speed in lines (or columns) per second: slow just past the edge, faster further out.
const AUTOSCROLL_BASE: f32 = 12.0;
const AUTOSCROLL_PER_CELL: f32 = 40.0;
const AUTOSCROLL_MAX: f32 = 1000.0;
/// Lines per mouse-wheel notch.
pub const WHEEL_LINES: f32 = 3.0;
/// Columns per notch scrolling sideways (a sideways wheel, or Shift with the wheel).
const WHEEL_COLS: f32 = 6.0;

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
    /// A text drag goes to the nearest caret position (insert-like modes), not the char under the pointer.
    round: bool,
    /// A box (column) selection's corner where the button went down: (line, display column).
    boxed: Option<(usize, usize)>,
    /// The drag autoscroll's next tick, the last one, and the lines and columns it owes (fractions carried over).
    autoscroll: Option<Instant>,
    scrolled_at: Option<Instant>,
    scroll_owed: (f32, f32),
    /// Where the scrollbar's thumb was grabbed, from its top (it moves with the pointer without jumping).
    grab: f32,
    /// Wheel steps not taken yet (touchpads and fine wheels send fractions of one), and what they scroll.
    wheel_owed: (f32, f32),
    wheel_what: WheelWhat,
    icon: CursorIcon,
}

/// What the wheel scrolled last: steps owed to one thing are dropped when the wheel turns over another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WheelWhat {
    #[default]
    Text,
    Picker,
    Menu,
    Settings,
    About,
    Sidebar,
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    Git,
    Zoom,
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
            round: true,
            boxed: None,
            autoscroll: None,
            scrolled_at: None,
            scroll_owed: (0.0, 0.0),
            grab: 0.0,
            wheel_owed: (0.0, 0.0),
            wheel_what: WheelWhat::Text,
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
            MouseButton::Middle => self.middle_press(st, ui, lay, x, y),
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
        // the middle button ends a box drag
        if button == MouseButton::Middle && self.boxed.is_some() && self.drag == Drag::Text {
            self.boxed = None;
            self.drag = Drag::None;
            self.stop_autoscroll();
            st.after_input();
            return Response::INPUT;
        }
        if button != MouseButton::Left {
            return if menu { Response::INPUT } else { Response::NONE };
        }
        self.boxed = None;
        if self.drag == Drag::Settings {
            st.settings_release();
        }
        if self.drag == Drag::Text {
            st.ed.mouse_up();
            if let Some(lay) = &lay {
                reveal_cursor_column(st, lay);
            }
            st.after_input();
        }
        // a title bar button acts when released over it
        crate::titlebar::release(st, &ui.hits, x, y);
        if self.drag == Drag::Sidebar {
            st.persist_sidebar_width();
        }
        if self.drag == Drag::Scrollbar {
            ui.scrollbar_hot = ui.hits.scrollbar.is_some_and(|(t, _)| t.contains(x, y));
        }
        // ----- git -----
        #[cfg(feature = "git")]
        if self.drag == Drag::Diff {
            crate::git::diffview::release(st);
        }
        // -----
        self.drag = Drag::None;
        self.stop_autoscroll();
        Response::INPUT
    }

    fn stop_autoscroll(&mut self) {
        self.autoscroll = None;
        self.scrolled_at = None;
        self.scroll_owed = (0.0, 0.0);
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
            // the thumb moves with the pointer from where it was grabbed; a click beside it centres it there
            let thumb = crate::draw::scrollbar_thumb(&lay, track, total, ui.anim.scroll.top);
            self.grab = if (thumb.y..thumb.bottom()).contains(&y) { y - thumb.y } else { thumb.h / 2.0 };
            scroll_to_pointer(st, ui, &lay, track, total, y - self.grab);
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
        // Shift+Alt+click / drag: a box of columns from the cursor (a middle drag: from the pointer)
        if self.mods.shift && self.mods.alt && !in_gutter {
            let b = st.ed.buf();
            let head = b.sel.primary().head.min(b.rope().len_chars());
            let corner = (text::line_of(b.rope(), head), b.doc.col_of(head, st.ed.settings.tab_width.max(1)));
            return self.box_press(st, ui, &lay, x, y, Some(corner));
        }
        let ed = &st.ed;
        // a caret goes between the chars nearest the pointer; vim's normal mode, words and lines take the char
        // under it (the right half of a word's last letter is still that word)
        self.round = (ed.mode == Mode::Insert || !ed.vim()) && n == 1 && !in_gutter;
        let (pos, n) = if in_gutter {
            // a click on a line number selects the line
            (pos_at(st, ui, &lay, lay.text_x(), y, true), 3)
        } else {
            (pos_at(st, ui, &lay, x, y, self.round), n)
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
    fn middle_press(&mut self, st: &mut State, ui: &mut Ui, lay: Option<Layout>, x: f32, y: f32) -> Response {
        if st.menu_open() {
            st.menu = None;
            st.titlebar.menu = None;
            return Response::INPUT;
        }
        // ----- jank hunt: a middle drag over the text selects a box of columns, as in Sublime -----
        if let Some(lay) = lay
            && self.drag == Drag::None
            && matches!(target_at(st, ui, &lay, x, y), Target::Text(_))
        {
            return self.box_press(st, ui, &lay, x, y, None);
        }
        // -----
        Response::NONE
    }

    /// Starts a box selection from `corner` (line, display column; none: the pointer's cell) to (`x`, `y`).
    fn box_press(
        &mut self,
        st: &mut State,
        ui: &mut Ui,
        lay: &Layout,
        x: f32,
        y: f32,
        corner: Option<(usize, usize)>,
    ) -> Response {
        if st.ed.prompt.is_some() {
            st.key(KeyPress::plain(Key::Esc));
        }
        let ed = &st.ed;
        self.round = ed.mode == Mode::Insert || !ed.vim();
        let at = cell_at(st, ui, lay, x, y, self.round);
        let corner = corner.unwrap_or(at);
        self.boxed = Some(corner);
        st.ed.sidebar_focus = false;
        st.ed.mouse_box(corner, at);
        st.after_input();
        self.drag = Drag::Text;
        Response::INPUT
    }

    /// A text drag goes on to (`x`, `y`), held inside the text (the autoscroll takes it further).
    fn drag_text(&mut self, st: &mut State, ui: &Ui, lay: &Layout, x: f32, y: f32) {
        let tx = x.clamp(lay.text_x(), text_right(lay) - 1.0);
        let ty = y.clamp(lay.text.y, text_bottom(lay) - 1.0);
        if let Some(corner) = self.boxed {
            st.ed.mouse_box(corner, cell_at(st, ui, lay, tx, ty, self.round));
        } else if let Some(pos) = pos_at(st, ui, lay, tx, ty, self.round) {
            st.ed.mouse_drag(pos);
        }
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
                // past an edge the selection stops at it (the autoscroll takes it further)
                self.drag_text(st, ui, &lay, x, y);
                redraw = true;
                // past an edge of the text: keep scrolling while the button is held
                if autoscroll_speed(st, &lay, x, y) != (0.0, 0.0) {
                    self.autoscroll.get_or_insert(now);
                } else {
                    self.stop_autoscroll();
                }
            }
            Drag::Scrollbar => {
                if let Some((track, total)) = ui.hits.scrollbar {
                    scroll_to_pointer(st, ui, &lay, track, total, y - self.grab);
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
        // ----- jank hunt: the scrollbar lights up under the pointer; the picker row under it -----
        let hot = self.drag == Drag::Scrollbar
            || (self.drag == Drag::None && ui.hits.scrollbar.is_some_and(|(t, _)| t.contains(x, y)));
        redraw |= std::mem::replace(&mut ui.scrollbar_hot, hot) != hot;
        if st.ed.picker.is_some() {
            let row = ui.hits.picker_rows.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1 - ui.picker_top);
            redraw |= std::mem::replace(&mut ui.picker_hover, row) != row;
        }
        // -----
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
        // a drag keeps its shape wherever the pointer goes (over the gutter, the title bar, out of the text)
        let icon = match self.drag {
            Drag::Text => CursorIcon::Text,
            Drag::Scrollbar | Drag::Minimap => CursorIcon::Default,
            _ => icon,
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
        let row = ui.picker_hover.take().is_some();
        let hot = self.drag != Drag::Scrollbar && std::mem::take(&mut ui.scrollbar_hot);
        Response::redraw_if(bar | map | row | hot)
    }

    /// The drag autoscroll's timer: scrolls on while the pointer is past the text's top or bottom edge.
    pub fn tick(&mut self, st: &mut State, ui: &mut Ui, lay: Option<Layout>, now: Instant) -> Response {
        if self.autoscroll.is_none_or(|t| now < t) {
            return Response::NONE;
        }
        let Some(lay) = lay else { return Response::NONE };
        let (x, y) = self.pointer;
        let speed = autoscroll_speed(st, &lay, x, y);
        if self.drag != Drag::Text || speed == (0.0, 0.0) {
            self.stop_autoscroll();
            return Response::NONE;
        }
        // lines and columns per second times the time since the last step; fractions are owed to the next
        let dt = self.scrolled_at.map_or(AUTOSCROLL_TICK, |t| now.saturating_duration_since(t)).min(AUTOSCROLL_MAX_DT);
        let owed = (self.scroll_owed.0 + speed.0 * dt.as_secs_f32(), self.scroll_owed.1 + speed.1 * dt.as_secs_f32());
        let (lines, cols) = (owed.0.trunc(), owed.1.trunc());
        self.scroll_owed = (owed.0 - lines, owed.1 - cols);
        self.scrolled_at = Some(now);
        self.autoscroll = Some(now + AUTOSCROLL_TICK);
        if lines == 0.0 && cols == 0.0 {
            return Response::NONE;
        }
        // no further than the last line at the bottom, the widest line's end at the right
        let v = st.ed.buf().view;
        let last = text::last_line(st.ed.buf().rope());
        let lines = if lines > 0.0 { lines.min((last + 1).saturating_sub(lay.rows + v.top) as f32) } else { lines };
        let cols =
            if cols > 0.0 { cols.min(widest_line(st, &lay).saturating_sub(v.left + lay.cols) as f32) } else { cols };
        st.ed.scroll(lines as isize, cols as isize);
        ui.frac = 0.0;
        let scroll = ui.target_scroll(&st.ed);
        ui.anim.set_scroll(scroll);
        // the selection follows to the text's edge
        self.drag_text(st, ui, &lay, x, y);
        Response::REDRAW
    }

    // ----- the wheel -----

    pub fn wheel(&mut self, st: &mut State, ui: &mut Ui, lay: Option<Layout>, delta: Wheel) -> Response {
        let Some(lay) = lay else { return Response::NONE };
        // ----- jank hunt: the menu bar's open menu scrolls when it is taller than the window -----
        let (px, py) = self.pointer;
        if let Some(m) = &mut st.titlebar.menu
            && m.hits.panels.last().is_some_and(|p| p.contains(px, py))
        {
            let lines = match delta {
                Wheel::Lines(_, y) => -y * WHEEL_LINES,
                Wheel::Pixels(_, y) => -y / crate::context_menu::row_h(&lay),
            };
            let rows = self.steps(WheelWhat::Menu, lines);
            let Some(m) = &mut st.titlebar.menu else { return Response::NONE };
            return Response::redraw_if(m.wheel(rows));
        }
        // -----
        if st.menu_open() {
            return Response::NONE; // nothing scrolls under an open menu
        }
        // notches times `per_notch`, or pixels over `per_pixel`
        let dy = |per_notch: f32, per_pixel: f32| match delta {
            Wheel::Lines(_, y) => -y * per_notch,
            Wheel::Pixels(_, y) => -y / per_pixel,
        };
        // the list scrolls under the highlighted row, which stays where it is
        if st.ed.picker.is_some() {
            let row_h = (lay.ch + lay.pad).round();
            let rows = self.steps(WheelWhat::Picker, dy(WHEEL_LINES, row_h));
            ui.picker_top = ui.picker_top.saturating_add_signed(rows);
            return Response::redraw_if(rows != 0);
        }
        if st.settings.is_open() {
            let px = dy(WHEEL_LINES * lay.ch, 1.0);
            // an open list scrolls by whole rows: a touchpad's pixels add up to them
            let list_row = st.settings.panel.as_ref().filter(|p| p.list.is_some()).map(|p| p.hits.list_row_h.max(1.0));
            if let Some(row_h) = list_row {
                let rows = self.steps(WheelWhat::Settings, px / row_h);
                st.settings_wheel(rows as f32 * row_h);
                return Response::redraw_if(rows != 0);
            }
            st.settings_wheel(px);
            return Response::REDRAW;
        }
        if st.ed.about.is_some() {
            let lines = self.steps(WheelWhat::About, dy(WHEEL_LINES, lay.ch));
            if let Some(top) = &mut st.ed.about {
                *top = top.saturating_add_signed(lines);
            }
            return Response::redraw_if(lines != 0);
        }
        #[cfg(feature = "terminal")]
        if st.terminal_wheel(-dy(WHEEL_LINES, lay.ch), px, py, self.mods) {
            return Response::REDRAW;
        }
        let lines = dy(WHEEL_LINES, lay.ch);
        // lists that scroll by whole rows get whole rows, the fractions kept for the next event
        #[cfg(feature = "git")]
        {
            let (rows, rest) = self.owed(WheelWhat::Git, lines);
            if crate::git::wheel(st, px, py, rows as f32) {
                self.keep(WheelWhat::Git, rest);
                return Response::redraw_if(rows != 0);
            }
        }
        let (rows, rest) = self.owed(WheelWhat::Sidebar, lines);
        if crate::sidebar::wheel(st, &ui.hits.sidebar, px, py, rows as f32) {
            self.keep(WheelWhat::Sidebar, rest);
            return Response::redraw_if(rows != 0);
        }
        // Ctrl+wheel zooms, like Ctrl+= / Ctrl+- (a touchpad: a step per two lines' height)
        if self.mods.ctrl {
            let up = -self.steps(WheelWhat::Zoom, dy(1.0, 2.0 * lay.ch));
            if up != 0 {
                let cmd = if up > 0 { shrew_core::Command::FontBigger } else { shrew_core::Command::FontSmaller };
                for _ in 0..up.unsigned_abs().min(4) {
                    st.ed.run(cmd);
                }
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
        // sideways: the wheel's own axis, or Shift with an upright wheel (as in most editors)
        let sideways = matches!(delta, Wheel::Lines(x, _) if x == 0.0) && self.mods.shift;
        let cols = match delta {
            Wheel::Lines(_, y) if sideways => -y * WHEEL_COLS,
            Wheel::Lines(x, _) => -x * WHEEL_COLS,
            Wheel::Pixels(x, _) => -x / lay.cw,
        };
        let lines = if sideways { 0.0 } else { lines };
        self.take_wheel(WheelWhat::Text);
        let owed = (self.wheel_owed.0 + lines, self.wheel_owed.1 + cols);
        let whole = (owed.0.trunc(), owed.1.trunc());
        match delta {
            Wheel::Lines(..) => {
                // a fine wheel's fractions of a notch add up
                self.wheel_owed = (owed.0 - whole.0, owed.1 - whole.1);
                ui.frac = 0.0;
                st.ed.scroll(whole.0 as isize, 0);
            }
            Wheel::Pixels(..) => {
                // touchpads: follow the fingers pixel by pixel
                self.wheel_owed.1 = owed.1 - whole.1;
                let v = st.ed.buf().view;
                let cur = v.top as f32 + if ui.frac_top == v.top { ui.frac } else { 0.0 };
                let last = text::last_line(st.ed.buf().rope()) as f32;
                let to = (cur + lines).clamp(0.0, last);
                let top = to.floor();
                st.ed.scroll(top as isize - v.top as isize, 0);
                ui.frac = to - top;
                ui.frac_top = st.ed.buf().view.top;
            }
        }
        let moved = scroll_cols(st, &lay, whole.1 as isize);
        if self.drag == Drag::Text {
            // selecting while the wheel scrolls: the selection follows the text under the still pointer (the view
            // goes there at once, so what is drawn is what is selected)
            ui.anim.set_scroll(ui.target_scroll(&st.ed));
            self.drag_text(st, ui, &lay, px, py);
        } else if let Wheel::Pixels(..) = delta {
            let s = ui.target_scroll(&st.ed);
            ui.anim.set_scroll(Scroll { top: s.top, left: ui.anim.scroll.left });
        }
        Response::redraw_if(lines != 0.0 || moved)
    }

    /// Whole steps out of `v` plus what earlier events left over (on the same thing, the same way).
    fn steps(&mut self, what: WheelWhat, v: f32) -> isize {
        let (n, rest) = self.owed(what, v);
        self.keep(what, rest);
        n
    }

    /// [`Mouse::steps`] without taking them: (whole steps, what would stay owed).
    fn owed(&self, what: WheelWhat, v: f32) -> (isize, f32) {
        let before = if self.wheel_what == what { self.wheel_owed.0 } else { 0.0 };
        // turned back: at once
        let before = if v * before < 0.0 { 0.0 } else { before };
        let owed = before + v;
        (owed.trunc() as isize, owed - owed.trunc())
    }

    /// `what` took its whole steps: `rest` stays owed to it.
    fn keep(&mut self, what: WheelWhat, rest: f32) {
        self.take_wheel(what);
        self.wheel_owed.0 = rest;
    }

    /// The wheel turns over `what`: steps owed to something else are dropped.
    fn take_wheel(&mut self, what: WheelWhat) {
        if self.wheel_what != what {
            self.wheel_what = what;
            self.wheel_owed = (0.0, 0.0);
        }
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

/// The char position under the pointer. `round`: the nearest caret position (the end of the text below the last
/// line); else the char under it.
pub fn pos_at(st: &State, ui: &Ui, lay: &Layout, x: f32, y: f32, round: bool) -> Option<usize> {
    let scroll = ui.anim.scroll;
    let rope = &st.ed.buf().doc.rope;
    let last = text::last_line(rope);
    let line = scroll.line_at(lay, y);
    // a caret below the last line goes to the end of the text, as in most editors (vim keeps the column)
    if round && line > last as isize {
        return Some(rope.len_chars());
    }
    let line = line.clamp(0, last as isize) as usize;
    let col = scroll.col_at(lay, x).max(0.0);
    let col = if round { col.round() } else { col.floor() } as usize;
    Some(text::pos_at_col(rope, line, col, st.ed.settings.tab_width.max(1)))
}

/// The (line, display column) under the pointer: the nearest column boundary with `round`, else the cell under it.
/// Past the end of a line the column goes on (a box can be wider than its lines).
fn cell_at(st: &State, ui: &Ui, lay: &Layout, x: f32, y: f32, round: bool) -> (usize, usize) {
    let scroll = ui.anim.scroll;
    let last = text::last_line(st.ed.buf().rope());
    let line = scroll.line_at(lay, y).clamp(0, last as isize) as usize;
    let col = scroll.col_at(lay, x).max(0.0);
    (line, if round { col.round() } else { col.floor() } as usize)
}

/// Scrolls so the scrollbar's thumb has its top at `thumb_y` (the inverse of [`crate::draw::scrollbar_thumb`]), at
/// once: the thumb stays under the pointer.
fn scroll_to_pointer(st: &mut State, ui: &mut Ui, lay: &Layout, track: shrew_render::Rect, total: usize, thumb_y: f32) {
    let thumb = crate::draw::scrollbar_thumb(lay, track, total, 0.0);
    let room = (track.h - thumb.h).max(1.0);
    let f = ((thumb_y - track.y) / room).clamp(0.0, 1.0);
    let target = (f * total.saturating_sub(1) as f32).round() as isize;
    let top = st.ed.buf().view.top as isize;
    st.ed.scroll(target - top, 0);
    ui.frac = 0.0;
    let s = ui.target_scroll(&st.ed);
    ui.anim.set_scroll(Scroll { top: s.top, left: ui.anim.scroll.left });
}

/// Scrolls the text `cols` columns sideways, to the left edge at most and no further right than shows the end of
/// the widest line on screen (unless the view is already past it). True when it moved.
fn scroll_cols(st: &mut State, lay: &Layout, cols: isize) -> bool {
    if cols == 0 {
        return false;
    }
    let left = st.ed.buf().view.left;
    let most = widest_line(st, lay).saturating_sub(lay.cols).max(left);
    let to = left.saturating_add_signed(cols).min(most);
    st.ed.scroll(0, to as isize - left as isize);
    to != left
}

/// The bottom of the text's last whole row (a drag past it scrolls; the cut-off row under it is not a target).
fn text_bottom(lay: &Layout) -> f32 {
    lay.text.y + lay.rows as f32 * lay.ch
}

/// Scrolls sideways (only) to show the cursor: a click on a short line while the view is scrolled right lands at
/// its end, which may be out of sight.
fn reveal_cursor_column(st: &mut State, lay: &Layout) {
    let b = st.ed.buf();
    let head = b.sel.primary().head.min(b.rope().len_chars());
    let col = b.doc.col_of(head, st.ed.settings.tab_width.max(1));
    let left = b.view.left;
    let margin = 4.min(lay.cols / 4);
    let to = if col < left {
        col.saturating_sub(margin)
    } else if col + 1 > left + lay.cols {
        col + 1 + margin - lay.cols
    } else {
        return;
    };
    st.ed.scroll(0, to as isize - left as isize);
}

/// The right edge of the text's whole visible columns (the minimap and the scrollbar are right of it).
fn text_right(lay: &Layout) -> f32 {
    lay.text_x() + lay.cols as f32 * lay.cw
}

/// The widest of the lines on screen, in columns, plus one for a cursor after its end. Very long lines count
/// their chars (no fewer columns than that), so this stays cheap.
fn widest_line(st: &State, lay: &Layout) -> usize {
    let b = st.ed.buf();
    let rope = b.rope();
    let tw = st.ed.settings.tab_width.max(1);
    let last = text::last_line(rope);
    let first = b.view.top.min(last);
    let width = |l: usize| {
        let n = rope.line(l).len_chars();
        if n > 4096 { n } else { text::col_of(rope, text::line_end(rope, l), tw) + 1 }
    };
    (first..(first + lay.rows + 1).min(last + 1)).map(width).max().unwrap_or(0)
}

/// How fast a text drag with the pointer at (`x`, `y`) scrolls, in (lines, columns) per second: nothing inside the
/// text or where there is no more to show that way, slowly just past an edge and faster further out.
fn autoscroll_speed(st: &State, lay: &Layout, x: f32, y: f32) -> (f32, f32) {
    let speed = |past: f32, cell: f32| (AUTOSCROLL_BASE + AUTOSCROLL_PER_CELL * past / cell).min(AUTOSCROLL_MAX);
    let v = st.ed.buf().view;
    let last = text::last_line(st.ed.buf().rope());
    let lines = if y < lay.text.y && v.top > 0 {
        -speed(lay.text.y - y, lay.ch)
    } else if y >= text_bottom(lay) && v.top + lay.rows <= last {
        speed((y - lay.text.bottom()).max(0.0) + 1.0, lay.ch)
    } else {
        0.0
    };
    let right = text_right(lay);
    // columns are narrow: count two per cell of distance
    let cols = if x < lay.text_x() && v.left > 0 {
        -speed(lay.text_x() - x, 2.0 * lay.cw)
    } else if x >= right && v.left + lay.cols < widest_line(st, lay) {
        speed(x - right + 1.0, 2.0 * lay.cw)
    } else {
        0.0
    };
    (lines, cols)
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
// ----- jank hunt -----
#[cfg(test)]
mod jank_tests;
// -----
