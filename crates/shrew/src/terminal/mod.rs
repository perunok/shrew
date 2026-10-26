//! The built-in terminal: a panel at the bottom of the window with a tab per terminal, each running your shell (or
//! a `:term` command) on a pseudo-terminal, drawn on the editor's text grid in the theme's colors.
//!
//! - [`session`]: the PTY, the reader thread (alacritty_terminal parses on it) and the screen they share.
//! - [`view`]: drawing the panel; rows are rebuilt only when the screen changed.
//! - [`input`]: keys, mouse reports and pastes as the program in the terminal expects them.
//! - [`normal`]: terminal normal mode (scroll, select, search the scrollback with vim keys).
//! - [`paths`]: `file:line:col` and URLs in the output; Ctrl+click opens them.
//!
//! Costs: nothing until the first terminal opens (no thread, no PTY, an empty `Vec`). Then one reader thread per
//! terminal, which wakes the UI at most once per drawn frame however fast the program writes.
//!
//! Keys: while a terminal has focus every key goes to it, except the escape chords (Ctrl+`, Ctrl+Shift+`,
//! Ctrl+PageUp/PageDown, Ctrl+Shift+P, plus `[terminal.keys]`), Ctrl+Shift+C/V, Shift+PageUp/PageDown, and the
//! prefixes Ctrl+\ (Ctrl+\ Ctrl+N: normal mode) and, with vim keys on, Ctrl+W (Ctrl+W k: back to the editor).

mod input;
mod normal;
pub mod paths;
mod session;
mod view;

pub use view::draw;

use crate::layout::Layout;
use crate::state::State;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use input::{Modes, MouseEvent};
use session::{Session, Size, Spawn};
use shrew_core::{Command, Key, KeyPress, Mods, MouseButton};
use shrew_render::Rect;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use view::Button;

/// The panel always leaves the editor this many rows (unless maximized).
const MIN_EDITOR_ROWS: f32 = 3.0;
const MIN_ROWS: usize = 2;
const MIN_COLS: usize = 2;
/// Clicks closer together than this count as double and triple clicks.
const MULTI_CLICK: Duration = Duration::from_millis(400);
/// A terminal whose program wrote nothing for this long is quiet (headless screenshots wait for it).
const QUIET: Duration = Duration::from_millis(400);

/// Chords that stay with shrew while a terminal has focus; `[terminal.keys]` adds to and overrides them.
const ESCAPE_KEYS: &[(&str, Command)] = &[
    ("<C-`>", Command::ToggleTerminal),
    ("<C-S-`>", Command::NewTerminal),
    ("<C-PageUp>", Command::PrevTerminal),
    ("<C-PageDown>", Command::NextTerminal),
    ("<C-S-p>", Command::CommandPalette),
];

/// One terminal in the panel.
pub struct Tab {
    pub id: u64,
    pub session: Session,
    view: view::View,
    normal: normal::Normal,
    /// The focus last reported to the program (when it asked for focus events).
    reported_focus: Option<bool>,
    /// The tab's label and when it was looked up.
    title: (String, Option<Instant>),
}

impl Tab {
    /// The modes that change how input is encoded; typing also brings the view back to the bottom.
    fn typing(&mut self) -> Modes {
        let mut term = self.session.term.lock();
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
            self.view.stale = true;
        }
        modes(*term.mode())
    }

    fn mode(&self) -> TermMode {
        *self.session.term.lock().mode()
    }
}

fn modes(m: TermMode) -> Modes {
    Modes {
        app_cursor: m.contains(TermMode::APP_CURSOR),
        bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
        sgr_mouse: m.contains(TermMode::SGR_MOUSE),
        utf8_mouse: m.contains(TermMode::UTF8_MOUSE),
    }
}

/// Where the panel's parts are this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    pub panel: Rect,
    /// The tab strip along the top.
    pub strip: Rect,
    /// Below the strip: the screen.
    pub body: Rect,
    /// Top-left of cell (0, 0).
    pub x0: f32,
    pub y0: f32,
    pub cols: usize,
    pub rows: usize,
    pub cw: f32,
    pub ch: f32,
    /// The top edge, dragged to resize the panel.
    pub handle: Rect,
    pub new: Rect,
    pub max: Rect,
    pub close: Rect,
}

/// Height of the tab strip.
pub fn strip_h(lay: &Layout) -> f32 {
    (lay.ch + 2.0 * lay.pad).round()
}

/// The panel height that shows `rows` rows of terminal.
pub fn panel_h(rows: usize, lay: &Layout) -> f32 {
    strip_h(lay) + rows as f32 * lay.ch + 2.0 * lay.pad
}

impl Geom {
    pub fn new(lay: &Layout) -> Geom {
        let p = lay.panel;
        let sh = strip_h(lay).min(p.h);
        let strip = Rect::new(p.x, p.y, p.w, sh);
        let body = Rect::new(p.x, p.y + sh, p.w, (p.h - sh).max(0.0));
        let (padx, pady) = (2.0 * lay.pad, lay.pad);
        let scrollbar = (10.0 * lay.scale).round();
        let cols = (((body.w - padx - scrollbar) / lay.cw).floor().max(0.0) as usize).max(MIN_COLS);
        let rows = (((body.h - 2.0 * pady) / lay.ch).floor().max(0.0) as usize).max(1);
        let btn = (sh - 2.0 * lay.pad).round().max(1.0);
        let by = (strip.y + (sh - btn) / 2.0).round();
        let close = Rect::new((strip.right() - 2.0 * lay.pad - btn).round(), by, btn, btn);
        let max = Rect::new(close.x - btn - lay.pad, by, btn, btn);
        let new = Rect::new(max.x - btn - lay.pad, by, btn, btn);
        let grab = (3.0 * lay.scale).round().max(2.0);
        Geom {
            panel: p,
            strip,
            body,
            x0: (body.x + padx).round(),
            y0: (body.y + pady).round(),
            cols,
            rows,
            cw: lay.cw,
            ch: lay.ch,
            handle: Rect::new(p.x, p.y - grab, p.w, 2.0 * grab),
            new,
            max,
            close,
        }
    }

    pub fn size(&self) -> Size {
        Size { cols: self.cols, rows: self.rows, cell_w: self.cw.round() as u16, cell_h: self.ch.round() as u16 }
    }

    pub fn cell_rect(&self, col: usize, row: usize, w: usize) -> Rect {
        let x = (self.x0 + col as f32 * self.cw).round();
        let r = (self.x0 + (col + w) as f32 * self.cw).round();
        Rect::new(x, self.y0 + row as f32 * self.ch, r - x, self.ch)
    }

    /// The cell under a point (clamped to the grid), and which half of it.
    pub fn cell_at(&self, x: f32, y: f32) -> (usize, usize, Side) {
        let fx = ((x - self.x0) / self.cw).max(0.0);
        let col = (fx.floor() as usize).min(self.cols - 1);
        let row = (((y - self.y0) / self.ch).floor().max(0.0) as usize).min(self.rows - 1);
        let side = if fx - fx.floor() < 0.5 || fx >= self.cols as f32 { Side::Left } else { Side::Right };
        (col, row, side)
    }
}

/// The panel cut out of the editor's area, when it is open.
pub fn carve(lay: Layout, t: &Terminals) -> Layout {
    if !t.visible || t.tabs.is_empty() {
        return lay;
    }
    lay.with_panel(t.panel_height(&lay))
}

/// A link under the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct Hover {
    pub tab: u64,
    pub line: Line,
    pub cols: Range<usize>,
    pub target: Link,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    File(PathBuf, Option<usize>, Option<usize>),
    Url(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Drag {
    #[default]
    None,
    /// The panel's top edge: from this pointer y and height in rows.
    Resize(f32, usize),
    Select,
    /// A button reported to the program.
    Report(u8),
}

/// What a key in the terminal amounts to for the rest of the app.
#[derive(Debug, PartialEq)]
pub enum Act {
    Done,
    Run(Command),
    Copy(String),
    Paste,
}

pub struct Terminals {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub visible: bool,
    /// Keys go to the terminal (else to the editor).
    pub focused: bool,
    pub maximized: bool,
    pub window_focused: bool,
    /// Panel height in rows.
    pub rows: usize,
    pub cfg: shrew_config::TerminalConfig,
    /// The single-instance endpoint, so `srw file` in a terminal opens in this window.
    pub instance: Option<PathBuf>,
    /// `<C-\>` or `<C-w>`, waiting for the key after it.
    pending: Option<KeyPress>,
    keys: Option<Vec<(KeyPress, Command)>>,
    next_id: u64,
    palette: Option<view::Pal>,
    /// The last frame: its layout (a new terminal starts at the size it will be drawn at), the panel's geometry,
    /// the tab rects.
    lay: Option<Layout>,
    pub geom: Option<Geom>,
    tab_hits: Vec<(Rect, usize)>,
    drag: Drag,
    clicks: Option<(Instant, (f32, f32), u8)>,
    /// The cell last reported for motion, and the touchpad's partial wheel lines.
    last_cell: Option<(usize, usize)>,
    wheel: f32,
    pub hover: Option<Hover>,
    hover_cell: Option<(u64, usize, usize)>,
    hover_button: Option<Button>,
}

impl Terminals {
    pub fn new(cfg: shrew_config::TerminalConfig) -> Terminals {
        Terminals {
            tabs: Vec::new(),
            active: 0,
            visible: false,
            focused: false,
            maximized: false,
            window_focused: true,
            rows: cfg.height().max(MIN_ROWS),
            cfg,
            instance: None,
            pending: None,
            keys: None,
            next_id: 1,
            palette: None,
            lay: None,
            geom: None,
            tab_hits: Vec::new(),
            drag: Drag::None,
            clicks: None,
            last_cell: None,
            wheel: 0.0,
            hover: None,
            hover_cell: None,
            hover_button: None,
        }
    }

    pub fn set_config(&mut self, cfg: shrew_config::TerminalConfig) {
        if cfg.height != self.cfg.height {
            self.rows = cfg.height().max(MIN_ROWS);
        }
        self.cfg = cfg;
        self.keys = None;
    }

    /// Keys go to a terminal now.
    pub fn has_focus(&self) -> bool {
        self.focused && self.visible && !self.tabs.is_empty()
    }

    /// Where the active terminal's cursor was drawn (the input method puts its candidates there).
    pub fn cursor_rect(&self) -> Option<Rect> {
        let (g, tab) = (self.geom?, self.tabs.get(self.active)?);
        let c = tab.view.cursor?;
        Some(g.cell_rect(c.col, c.row, if c.wide { 2 } else { 1 }))
    }

    fn tab(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// The escape chords: built in, then `[terminal.keys]` (an empty command removes one).
    fn escape(&mut self, k: KeyPress) -> Option<Command> {
        let cfg = &self.cfg;
        let keys = self.keys.get_or_insert_with(|| {
            let mut v: Vec<(KeyPress, Command)> =
                ESCAPE_KEYS.iter().filter_map(|(k, c)| Some((KeyPress::parse(k)?, *c))).collect();
            for (k, name) in &cfg.keys {
                let Some(kp) = KeyPress::parse(k) else { continue };
                v.retain(|(x, _)| *x != kp);
                if let Some(c) = Command::from_name(name) {
                    v.push((kp, c));
                }
            }
            v
        });
        keys.iter().find(|(x, _)| *x == k).map(|(_, c)| *c)
    }

    /// Moves the focus between the terminal and the editor, telling programs that asked.
    pub fn set_focus(&mut self, focused: bool) {
        self.focused = focused && !self.tabs.is_empty();
        if !self.focused {
            self.pending = None;
        }
        self.report_focus();
    }

    /// Focus in/out events (`ESC [I`, `ESC [O`) for programs that turned them on.
    pub fn report_focus(&mut self) {
        let on = self.window_focused && self.focused && self.visible;
        let active = self.active;
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            let want = on && i == active;
            if tab.reported_focus == Some(want) {
                continue;
            }
            tab.reported_focus = Some(want);
            if tab.mode().contains(TermMode::FOCUS_IN_OUT) {
                tab.session.write(if want { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
            }
        }
    }

    pub fn select(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.active = i;
            self.hover = None;
            if let Some(t) = self.tab() {
                t.view.stale = true;
            }
            self.report_focus();
        }
    }

    /// Closes a terminal (its program gets SIGHUP). The panel hides with the last one.
    pub fn close(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i);
        self.hover = None;
        if self.active >= i && self.active > 0 {
            self.active -= 1;
        }
        if self.tabs.is_empty() {
            self.visible = false;
            self.maximized = false;
            self.set_focus(false);
        } else {
            self.select(self.active.min(self.tabs.len() - 1));
        }
    }

    pub fn close_all(&mut self) {
        while !self.tabs.is_empty() {
            self.close(self.tabs.len() - 1);
        }
    }

    /// The panel's height in a layout without it.
    fn panel_height(&self, lay: &Layout) -> f32 {
        if self.maximized {
            return lay.text.h;
        }
        panel_h(self.rows, lay).min(lay.text.h - MIN_EDITOR_ROWS * lay.ch).max(strip_h(lay) + lay.ch)
    }

    /// The size a new terminal gets: the panel's in the last frame's layout (an 80-column guess before the first
    /// frame, corrected when it is drawn).
    fn size(&self) -> Size {
        let Some(lay) = self.lay else { return Size { cols: 80, rows: self.rows, cell_w: 8, cell_h: 16 } };
        let lay = if lay.panel.h > 0.0 { lay } else { lay.with_panel(self.panel_height(&lay)) };
        Geom::new(&lay).size()
    }

    // ----- keys -----

    /// A key while the terminal has focus.
    pub fn key(&mut self, k: KeyPress, vim: bool) -> Act {
        if let Some(prefix) = self.pending.take() {
            match self.prefixed(prefix, k) {
                Some(act) => return act,
                None => {
                    // not a chord after all: the prefix goes to the program, then this key as usual
                    if let Some(t) = self.tab() {
                        let m = t.typing();
                        if let Some(b) = input::encode_key(prefix, m) {
                            t.session.write(b);
                        }
                    }
                }
            }
        }
        if let Some(cmd) = self.escape(k) {
            return Act::Run(cmd);
        }
        let Some(tab) = self.tabs.get_mut(self.active) else { return Act::Done };
        let m = k.mods;
        let ctrl_shift = m.ctrl && m.shift && !m.alt;
        match k.key {
            Key::Char('c') if ctrl_shift => {
                let text = tab.session.term.lock().selection_to_string();
                return text.filter(|t| !t.is_empty()).map_or(Act::Done, Act::Copy);
            }
            Key::Char('v') if ctrl_shift => return Act::Paste,
            Key::Insert if m.shift && !m.ctrl => return Act::Paste,
            Key::PageUp | Key::PageDown | Key::Home | Key::End
                if m.shift && !m.ctrl && !m.alt && !tab.mode().contains(TermMode::ALT_SCREEN) =>
            {
                let scroll = match k.key {
                    Key::PageUp => Scroll::PageUp,
                    Key::PageDown => Scroll::PageDown,
                    Key::Home => Scroll::Top,
                    _ => Scroll::Bottom,
                };
                tab.session.term.lock().scroll_display(scroll);
                tab.view.stale = true;
                return Act::Done;
            }
            _ => {}
        }
        {
            let mut term = tab.session.term.lock();
            if normal::active(&term) {
                tab.view.stale = true;
                return match normal::key(&mut term, &mut tab.normal, k) {
                    normal::Outcome::Stay => Act::Done,
                    normal::Outcome::Exit => {
                        normal::exit(&mut term, &mut tab.normal);
                        Act::Done
                    }
                    normal::Outcome::Copy(t) => Act::Copy(t),
                };
            }
        }
        let prefix = m.ctrl && !m.alt && !m.shift && (k.key == Key::Char('\\') || (vim && k.key == Key::Char('w')));
        if prefix {
            self.pending = Some(k);
            return Act::Done;
        }
        // a finished `:term` command: Enter, Esc or q closes it
        if tab.session.exited() {
            let plain = !m.ctrl && !m.alt;
            if plain && matches!(k.key, Key::Enter | Key::Esc | Key::Char('q')) {
                return Act::Run(Command::CloseTerminal);
            }
            return Act::Done;
        }
        let modes = tab.typing();
        if let Some(bytes) = input::encode_key(k, modes) {
            tab.session.write(bytes);
        }
        Act::Done
    }

    /// The key after a prefix: `None` when the two are not a chord.
    fn prefixed(&mut self, prefix: KeyPress, k: KeyPress) -> Option<Act> {
        let m = k.mods;
        let plain = !m.ctrl && !m.alt;
        if prefix.key == Key::Char('\\') {
            return (m.ctrl && k.key == Key::Char('n')).then(|| {
                self.normal_mode();
                Act::Done
            });
        }
        // Ctrl+W, vim's window prefix in its terminal
        match k.key {
            Key::Char('k') | Key::Up if plain => Some(Act::Run(Command::FocusEditor)),
            Key::Char('w' | 'p') if !m.alt => Some(Act::Run(Command::FocusEditor)),
            Key::Char('N') if plain => {
                self.normal_mode();
                Some(Act::Done)
            }
            Key::Char(':') if plain => Some(Act::Run(Command::GotoLine)),
            Key::Char('c') if plain => Some(Act::Run(Command::CloseTerminal)),
            Key::Char('.') if plain => {
                // Ctrl+W . sends Ctrl+W itself
                let t = self.tab()?;
                t.typing();
                t.session.write(vec![0x17]);
                Some(Act::Done)
            }
            _ => None,
        }
    }

    pub fn normal_mode(&mut self) {
        if let Some(t) = self.tab() {
            normal::enter(&mut t.session.term.lock());
            t.view.stale = true;
        }
    }

    /// Text from an input method, or a paste: to the program.
    pub fn text(&mut self, s: &str) {
        if let Some(t) = self.tab()
            && !t.session.exited()
            && !view::in_normal(t)
        {
            t.typing();
            t.session.write(s.as_bytes().to_vec());
        }
    }

    pub fn paste(&mut self, s: &str) {
        if let Some(t) = self.tab()
            && !t.session.exited()
        {
            let m = t.typing();
            t.session.write(input::paste(s, m));
        }
    }

    // ----- mouse -----

    /// The wheel over the panel (`lines` > 0: up). The program gets it when it reports the mouse or runs on the
    /// alternate screen (as arrow keys, the way less and man scroll); otherwise it scrolls the history.
    fn scroll(&mut self, lines: f32, x: f32, y: f32, mods: Mods) {
        let Some(g) = self.geom else { return };
        self.wheel += lines;
        let n = self.wheel.trunc() as i32;
        if n == 0 {
            return;
        }
        self.wheel -= n as f32;
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let mode = tab.mode();
        let (col, row, _) = g.cell_at(x, y);
        if mode.intersects(TermMode::MOUSE_MODE) && !mods.shift {
            let ev = if n > 0 { MouseEvent::WheelUp } else { MouseEvent::WheelDown };
            for _ in 0..n.unsigned_abs() {
                if let Some(b) = input::encode_mouse(ev, col, row, mods, modes(mode)) {
                    tab.session.write(b);
                }
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !mods.shift {
            let key = KeyPress::plain(if n > 0 { Key::Up } else { Key::Down });
            for _ in 0..n.unsigned_abs() {
                if let Some(b) = input::encode_key(key, modes(mode)) {
                    tab.session.write(b);
                }
            }
        } else {
            tab.session.term.lock().scroll_display(Scroll::Delta(n));
            tab.view.stale = true;
            self.hover = None;
            self.hover_cell = None;
        }
    }

    /// The grid point under a cell of the screen (as it is scrolled now).
    fn point(&self, col: usize, row: usize) -> Option<Point> {
        let off = self.tabs.get(self.active)?.session.term.lock().grid().display_offset() as i32;
        Some(Point::new(Line(row as i32 - off), Column(col)))
    }

    /// The link under the pointer, found again only when the pointer moves to another cell.
    fn update_hover(&mut self, col: usize, row: usize) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let key = (tab.id, col, row);
        if self.hover_cell == Some(key) {
            return;
        }
        self.hover_cell = Some(key);
        let Some(point) = self.point(col, row) else { return };
        let (text, cols) = view::row_text(tab, point.line);
        let byte = cols.iter().position(|&c| c >= col).unwrap_or(text.len());
        let found = paths::at(&text, byte).filter(|_| cols.get(byte) == Some(&col));
        let target = found.as_ref().and_then(|f| match &f.target {
            paths::Target::Url(u) => Some(Link::Url(u.clone())),
            paths::Target::File { path, line, col } => {
                paths::resolve(path, &tab.session.cwd()).map(|p| Link::File(p, *line, *col))
            }
        });
        self.hover = found.zip(target).map(|(f, target)| {
            let a = cols.get(f.bytes.start).copied().unwrap_or(0);
            let z = cols.get(f.bytes.end).copied().unwrap_or_else(|| cols.last().map_or(0, |c| c + 1));
            Hover { tab: tab.id, line: point.line, cols: a..z, target }
        });
    }
}

// ----- the app's side -----

/// What a press in the panel asks of the app.
enum Press {
    Nothing,
    Run(Command),
    Open(Link),
    Paste,
}

impl State {
    /// A terminal command (Ctrl+`, the palette, `:term [cmd]`).
    pub fn terminal_command(&mut self, cmd: Command, command_line: Option<String>) {
        let t = &mut self.terminals;
        match cmd {
            Command::ToggleTerminal if t.visible && t.focused => {
                t.visible = false;
                t.set_focus(false);
            }
            Command::ToggleTerminal | Command::FocusTerminal => self.show_terminal(),
            Command::NewTerminal => self.open_terminal(command_line),
            Command::CloseTerminal => {
                let i = t.active;
                t.close(i);
            }
            Command::NextTerminal | Command::PrevTerminal if !t.tabs.is_empty() => {
                let n = t.tabs.len();
                let i = if cmd == Command::NextTerminal { (t.active + 1) % n } else { (t.active + n - 1) % n };
                t.select(i);
            }
            Command::MaximizeTerminal => {
                t.maximized = !t.maximized;
                if t.maximized || !t.visible {
                    self.show_terminal();
                }
            }
            Command::FocusEditor => t.set_focus(false),
            Command::TerminalNormalMode => {
                self.show_terminal();
                self.terminals.normal_mode();
            }
            _ => {}
        }
        // one place has the keys: a focused terminal takes them from the sidebar
        if self.terminals.has_focus() {
            self.ed.sidebar_focus = false;
        }
    }

    /// Shows the panel and focuses it, starting a shell if there is none.
    fn show_terminal(&mut self) {
        if self.terminals.tabs.is_empty() {
            return self.open_terminal(None);
        }
        self.terminals.visible = true;
        self.terminals.set_focus(true);
    }

    /// Starts a terminal: the shell, or `command` run by it, in the project folder.
    pub fn open_terminal(&mut self, command: Option<String>) {
        let cwd = self.ed.work_dir();
        self.open_terminal_in(command, cwd);
    }

    /// [`State::open_terminal`] in the folder `cwd` (a tab's Open in Terminal).
    pub fn open_terminal_in(&mut self, command: Option<String>, cwd: std::path::PathBuf) {
        let t = &mut self.terminals;
        let mut env = Vec::new();
        if let Some(addr) = &t.instance {
            env.push(("SHREW_INSTANCE".to_string(), addr.display().to_string()));
        }
        let spawn = Spawn {
            command: command.filter(|c| !c.trim().is_empty()),
            shell: t.cfg.shell.clone(),
            cwd,
            scrollback: t.cfg.scrollback(),
            env,
        };
        let size = t.size();
        let palette = view::Pal::new(&self.colors);
        match Session::spawn(&spawn, size, palette.rgb(), self.wake.clone()) {
            Ok(session) => {
                let id = t.next_id;
                t.next_id += 1;
                let tab = Tab {
                    id,
                    session,
                    view: view::View::default(),
                    normal: normal::Normal::default(),
                    reported_focus: None,
                    title: (String::new(), None),
                };
                t.tabs.push(tab);
                t.visible = true;
                t.select(t.tabs.len() - 1);
                t.set_focus(true);
            }
            Err(e) => self.ed.error(format!("cannot start a terminal: {e}")),
        }
    }

    /// Keys while a terminal has focus. False when the editor should take the key instead (no terminal focused,
    /// or a picker, prompt or the About panel is open over everything).
    pub fn terminal_key(&mut self, k: KeyPress) -> bool {
        if !self.terminals.has_focus()
            || self.ed.picker.is_some()
            || self.ed.prompt.is_some()
            || self.ed.about.is_some()
        {
            return false;
        }
        let vim = self.ed.vim();
        let act = self.terminals.key(k, vim);
        self.terminal_act(act);
        true
    }

    fn terminal_act(&mut self, act: Act) {
        match act {
            Act::Done => {}
            Act::Run(cmd) => {
                self.ed.message = None;
                self.ed.run(cmd);
                self.after_input();
            }
            Act::Copy(text) => {
                self.ed.clipboard.set(text);
                self.ed.info("copied");
            }
            Act::Paste => {
                if let Some(text) = self.ed.clipboard.get() {
                    self.terminals.paste(&text);
                }
            }
        }
    }

    /// Text from an input method: to the focused terminal. False when no terminal has focus.
    pub fn terminal_text(&mut self, s: &str) -> bool {
        if !self.terminals.has_focus() || self.ed.picker.is_some() || self.ed.prompt.is_some() {
            return false;
        }
        self.terminals.text(s);
        true
    }

    /// Picks up what the terminals' programs did: shells that exited close their tab (a `:term` command stays
    /// until dismissed), OSC 52 copies reach the clipboard. True when something changed.
    pub fn poll_terminals(&mut self) -> bool {
        let t = &mut self.terminals;
        let mut changed = false;
        let mut i = 0;
        while i < t.tabs.len() {
            let s = &t.tabs[i].session;
            if let Some(text) = s.shared.copied.lock().unwrap_or_else(|e| e.into_inner()).take() {
                self.ed.clipboard.set(text);
                changed = true;
            }
            if s.exited() && s.command.is_none() {
                t.close(i);
                changed = true;
                continue;
            }
            i += 1;
        }
        changed
    }

    /// A press over the panel. False when it is not over the panel (a left click elsewhere moves the focus to the
    /// editor).
    pub fn terminal_press(&mut self, x: f32, y: f32, button: MouseButton, mods: Mods) -> bool {
        let t = &mut self.terminals;
        let Some(g) = t.geom else { return false };
        if button == MouseButton::Left && g.handle.contains(x, y) {
            t.drag = Drag::Resize(y, t.rows);
            return true;
        }
        if !g.panel.contains(x, y) {
            if button == MouseButton::Left && t.focused {
                t.set_focus(false);
            }
            return false;
        }
        if self.ed.prompt.is_some() {
            self.ed.handle_key(KeyPress::plain(Key::Esc));
        }
        let t = &mut self.terminals;
        let press = t.press(&g, x, y, button, mods);
        if t.has_focus() {
            self.ed.sidebar_focus = false;
        }
        match press {
            Press::Nothing => {}
            Press::Run(cmd) => {
                self.ed.run(cmd);
                self.after_input();
            }
            Press::Open(link) => self.open_link(link),
            Press::Paste => self.terminal_act(Act::Paste),
        }
        true
    }

    fn open_link(&mut self, link: Link) {
        match link {
            Link::File(path, line, col) => {
                self.terminals.set_focus(false);
                self.open_at(&path, line, col);
                self.after_input();
            }
            Link::Url(url) => {
                if let Err(e) = crate::open::url(&url) {
                    self.ed.error(format!("cannot open {url}: {e}"));
                }
            }
        }
    }

    /// The button went up: ends a resize (saving the height), a selection (copying it) or a reported press.
    pub fn terminal_release(&mut self, button: MouseButton, mods: Mods) {
        let t = &mut self.terminals;
        let number = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        };
        // another button's release does not end a drag
        let ends = match t.drag {
            Drag::Report(b) => b == number,
            Drag::None => false,
            _ => button == MouseButton::Left,
        };
        if !ends {
            return;
        }
        match std::mem::take(&mut t.drag) {
            Drag::Resize(..) => {
                let rows = t.rows;
                if let Some(path) = &self.config_path
                    && t.cfg.height != Some(rows)
                {
                    t.cfg.height = Some(rows);
                    if let Err(e) = shrew_config::persist_value(path, "terminal", "height", rows as i64) {
                        self.ed.error(format!("could not save the terminal height to {}: {e}", path.display()));
                    }
                }
            }
            Drag::Select => {
                let copy = t.cfg.copy_on_select();
                if let Some(tab) = t.tab() {
                    let text = tab.session.term.lock().selection_to_string().filter(|s| !s.is_empty());
                    if let Some(text) = text.filter(|_| copy) {
                        self.ed.clipboard.set(text);
                    }
                }
            }
            Drag::Report(b) => {
                let (col, row) = t.last_cell.unwrap_or((0, 0));
                if let Some(tab) = t.tab() {
                    let m = modes(tab.mode());
                    if let Some(bytes) = input::encode_mouse(MouseEvent::Release(b), col, row, mods, m) {
                        tab.session.write(bytes);
                    }
                }
            }
            Drag::None => {}
        }
    }

    /// The pointer moved: drags, the link and button under it. Returns the pointer shape to show (`None`: not over
    /// the panel) and whether the panel needs drawing again.
    pub fn terminal_motion(&mut self, x: f32, y: f32, mods: Mods) -> (Option<winit::window::CursorIcon>, bool) {
        let t = &self.terminals;
        let before = (t.hover.clone(), t.hover_button, t.drag != Drag::None);
        let icon = self.terminal_pointer(x, y, mods);
        let t = &self.terminals;
        let redraw = before.2 || before.0 != t.hover || before.1 != t.hover_button;
        (icon, redraw)
    }

    fn terminal_pointer(&mut self, x: f32, y: f32, mods: Mods) -> Option<winit::window::CursorIcon> {
        use winit::window::CursorIcon;
        let t = &mut self.terminals;
        let g = t.geom?;
        match t.drag {
            Drag::Resize(y0, rows0) => {
                let lay = t.lay?;
                let room = lay.text.h + lay.panel.h - MIN_EDITOR_ROWS * lay.ch - strip_h(&lay) - 2.0 * lay.pad;
                let max = ((room / lay.ch).floor().max(MIN_ROWS as f32)) as usize;
                let rows = (rows0 as f32 + (y0 - y) / lay.ch).round().max(MIN_ROWS as f32) as usize;
                t.rows = rows.min(max);
                t.maximized = false;
                return Some(CursorIcon::NsResize);
            }
            Drag::Select => {
                let (col, row, side) = g.cell_at(x, y);
                // dragging past the top or bottom scrolls the history
                if y < g.y0 || y >= g.y0 + g.rows as f32 * g.ch {
                    let n = if y < g.y0 { 1 } else { -1 };
                    if let Some(tab) = t.tab() {
                        tab.session.term.lock().scroll_display(Scroll::Delta(n));
                    }
                }
                let point = t.point(col, row);
                if let Some(tab) = t.tab()
                    && let Some(p) = point
                {
                    let mut term = tab.session.term.lock();
                    if let Some(s) = &mut term.selection {
                        s.update(p, side);
                    }
                    tab.view.stale = true;
                }
                return Some(CursorIcon::Text);
            }
            Drag::Report(b) => {
                let (col, row, _) = g.cell_at(x, y);
                if t.last_cell != Some((col, row)) {
                    t.last_cell = Some((col, row));
                    if let Some(tab) = t.tab() {
                        let mode = tab.mode();
                        if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
                            && let Some(bytes) =
                                input::encode_mouse(MouseEvent::Motion(Some(b)), col, row, mods, modes(mode))
                        {
                            tab.session.write(bytes);
                        }
                    }
                }
                return Some(CursorIcon::Default);
            }
            Drag::None => {}
        }
        let button = [(g.new, Button::New), (g.max, Button::Max), (g.close, Button::Close)]
            .into_iter()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, b)| b);
        t.hover_button = button;
        if g.handle.contains(x, y) {
            return Some(CursorIcon::NsResize);
        }
        if !g.panel.contains(x, y) {
            t.hover = None;
            t.hover_cell = None;
            return None;
        }
        if !g.body.contains(x, y) {
            t.hover = None;
            return Some(if button.is_some() { CursorIcon::Pointer } else { CursorIcon::Default });
        }
        let (col, row, _) = g.cell_at(x, y);
        let mode = t.tabs.get(t.active).map(Tab::mode).unwrap_or_default();
        if mode.contains(TermMode::MOUSE_MOTION) && t.last_cell != Some((col, row)) {
            t.last_cell = Some((col, row));
            if let Some(tab) = t.tab()
                && let Some(bytes) = input::encode_mouse(MouseEvent::Motion(None), col, row, mods, modes(mode))
            {
                tab.session.write(bytes);
            }
        }
        t.update_hover(col, row);
        Some(if t.hover.is_some() { CursorIcon::Pointer } else { CursorIcon::Text })
    }

    /// The wheel over the panel. False when the pointer is elsewhere.
    pub fn terminal_wheel(&mut self, lines: f32, x: f32, y: f32, mods: Mods) -> bool {
        let t = &mut self.terminals;
        if !t.geom.is_some_and(|g| g.panel.contains(x, y)) {
            return false;
        }
        t.scroll(lines, x, y, mods);
        true
    }

    /// The window gained or lost the focus.
    pub fn terminal_window_focus(&mut self, focused: bool) {
        self.terminals.window_focused = focused;
        if !self.terminals.tabs.is_empty() {
            self.terminals.report_focus();
        }
    }

    /// Headless screenshots: true while a terminal's program is still starting up or writing.
    pub fn terminals_busy(&self) -> bool {
        self.terminals.tabs.iter().any(|t| {
            let s = &t.session.shared;
            let last = s.last_output.load(Ordering::Relaxed).max(s.last_input.load(Ordering::Relaxed));
            let age = s.started.elapsed();
            if s.exited.load(Ordering::Acquire) {
                return false;
            }
            if last == 0 {
                return age < Duration::from_secs(3);
            }
            age.saturating_sub(Duration::from_millis(last)) < QUIET
        })
    }
}

impl Terminals {
    fn press(&mut self, g: &Geom, x: f32, y: f32, button: MouseButton, mods: Mods) -> Press {
        let was_focused = self.focused;
        self.set_focus(true);
        if g.strip.contains(x, y) {
            if button != MouseButton::Left && button != MouseButton::Middle {
                return Press::Nothing;
            }
            for (r, b) in [(g.new, Button::New), (g.max, Button::Max), (g.close, Button::Close)] {
                if r.contains(x, y) && button == MouseButton::Left {
                    return Press::Run(match b {
                        Button::New => Command::NewTerminal,
                        Button::Max => Command::MaximizeTerminal,
                        Button::Close => Command::CloseTerminal,
                    });
                }
            }
            if let Some(&(_, i)) = self.tab_hits.iter().find(|(r, _)| r.contains(x, y)) {
                if button == MouseButton::Middle {
                    self.close(i);
                } else {
                    self.select(i);
                }
            }
            return Press::Nothing;
        }
        let (col, row, side) = g.cell_at(x, y);
        // Ctrl+click opens the link under the pointer
        if button == MouseButton::Left && mods.ctrl {
            self.hover_cell = None;
            self.update_hover(col, row);
            if let Some(h) = self.hover.clone() {
                return Press::Open(h.target);
            }
        }
        let Some(point) = self.point(col, row) else { return Press::Nothing };
        let Some(tab) = self.tabs.get_mut(self.active) else { return Press::Nothing };
        let mode = tab.mode();
        let b = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        };
        if mode.intersects(TermMode::MOUSE_MODE) && !mods.shift && !normal_mode(tab) {
            if let Some(bytes) = input::encode_mouse(MouseEvent::Press(b), col, row, mods, modes(mode)) {
                tab.session.write(bytes);
            }
            self.drag = Drag::Report(b);
            self.last_cell = Some((col, row));
            return Press::Nothing;
        }
        match button {
            MouseButton::Left => {
                let now = Instant::now();
                let n = match self.clicks {
                    Some((t, (px, py), n)) if now - t < MULTI_CLICK && (px - x).hypot(py - y) < g.cw => (n % 3) + 1,
                    _ => 1,
                };
                self.clicks = Some((now, (x, y), n));
                // the first click into an unfocused terminal only focuses it
                if !was_focused && n == 1 {
                    let mut term = tab.session.term.lock();
                    term.selection = None;
                    tab.view.stale = true;
                    return Press::Nothing;
                }
                let ty = match n {
                    2 => SelectionType::Semantic,
                    3 => SelectionType::Lines,
                    _ if mods.alt => SelectionType::Block,
                    _ => SelectionType::Simple,
                };
                let mut term = tab.session.term.lock();
                if normal_mode_term(&term) {
                    term.vi_goto_point(point);
                }
                term.selection = Some(Selection::new(ty, point, side));
                tab.view.stale = true;
                self.drag = Drag::Select;
                Press::Nothing
            }
            MouseButton::Middle => Press::Paste,
            MouseButton::Right => Press::Nothing,
        }
    }
}

fn normal_mode(tab: &Tab) -> bool {
    tab.mode().contains(TermMode::VI)
}

fn normal_mode_term(term: &alacritty_terminal::Term<session::Listener>) -> bool {
    normal::active(term)
}

#[cfg(test)]
mod tests;
