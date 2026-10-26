//! The Settings panel: Ctrl+, (Cmd+, on macOS), palette *Preferences: Settings*, `:settings [section]` or the gear
//! in the status bar. A large centered overlay, sections on the left and their rows on the right, driven by the
//! keyboard (j/k move, Tab or h/l switch columns, Enter/Space change, -/+ adjust, r reset, / search, Esc close) or
//! the mouse (click, wheel, switches, steppers and sliders).
//!
//! The rows are descriptors ([`rows`]); every change applies at once and is written to config.toml by the same code
//! `:set`, the theme/font pickers and `:features` use, so the file stays the one source of truth (comments kept).
//! Nothing here exists until the panel is first opened: [`Settings`] is an empty `Option` and the row table is built
//! on open.

mod rows;
#[cfg(test)]
mod tests;
mod view;

pub use view::draw;

use crate::state::State;
use rows::{Kind, Row, Section};
use shrew_core::features::Feature;
use shrew_core::{Event, Key, KeyPress};
use std::path::Path;
use std::rc::Rc;

/// The panel when it is open, and where it was when it closed.
#[derive(Default)]
pub struct Settings {
    pub panel: Option<Panel>,
    /// The section shown last, so the panel reopens there.
    last: usize,
}

impl Settings {
    pub fn is_open(&self) -> bool {
        self.panel.is_some()
    }

    pub fn close(&mut self) {
        if let Some(p) = self.panel.take() {
            self.last = p.section;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Sections,
    Rows,
    Search,
}

/// A list of choices opened from an options row, inside the panel.
pub struct List {
    /// The row it belongs to (index into [`Panel::rows`]).
    pub row: usize,
    /// (value, label)
    pub options: Vec<(String, String)>,
    pub selected: usize,
    pub top: usize,
}

pub struct Panel {
    pub rows: Rc<[Row]>,
    /// Index into [`Section::ALL`].
    pub section: usize,
    pub focus: Focus,
    /// The selected row: an index into [`Panel::shown`].
    pub row: usize,
    pub query: String,
    pub list: Option<List>,
    /// An action waiting for its confirming second Enter (index into `rows`).
    pub armed: Option<usize>,
    /// Scroll of the rows column in pixels. `follow`: keep the selected row in view (keys move it; the wheel lets go).
    pub scroll: f32,
    pub follow: bool,
    /// The slider being dragged (index into `rows`).
    pub drag: Option<usize>,
    /// Where things were drawn last frame, for the mouse.
    pub hits: view::Hits,
}

impl Panel {
    fn new(rows: Vec<Row>, section: usize) -> Panel {
        Panel {
            rows: rows.into(),
            section,
            focus: Focus::Sections,
            row: 0,
            query: String::new(),
            list: None,
            armed: None,
            scroll: 0.0,
            follow: true,
            drag: None,
            hits: view::Hits::default(),
        }
    }

    pub fn section(&self) -> Section {
        Section::ALL[self.section]
    }

    /// Whether the right column shows search results.
    pub fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// What the right column shows, as indices into `rows`: the section's rows, or every match of the search.
    pub fn shown(&self) -> Vec<usize> {
        let s = self.section();
        let q = self.query.trim();
        (0..self.rows.len())
            .filter(|&i| if q.is_empty() { self.rows[i].section == s } else { self.rows[i].matches(q) })
            .collect()
    }

    /// The selected row (index into `rows`).
    pub fn current(&self) -> Option<usize> {
        self.shown().get(self.row).copied()
    }

    fn set_section(&mut self, i: usize) {
        let i = i.min(Section::ALL.len() - 1);
        if i != self.section {
            self.section = i;
            self.row = 0;
            self.scroll = 0.0;
        }
        self.query.clear();
        self.follow = true;
    }

    fn move_row(&mut self, by: isize) {
        let n = self.shown().len();
        self.row = (self.row as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
        self.follow = true;
    }
}

/// Persists through `f` (one of shrew-config's persist functions) when there is a config file.
pub(crate) fn save_with(st: &mut State, f: impl FnOnce(&Path) -> std::io::Result<()>) {
    if let Some(path) = st.config_path.clone()
        && let Err(e) = f(&path)
    {
        st.ed.error(format!("could not save to {}: {e}", path.display()));
    }
}

/// Feature toggles go through the editor, like `:set`: it emits the event that persists them and starts or stops
/// what depends on them.
fn set_feature(st: &mut State, f: Feature, on: bool) {
    st.ed.set_feature(f, on);
}

/// Opens config.toml with the cursor on the first line starting with `at` (anywhere when empty).
fn open_config_at(st: &mut State, at: &str) {
    st.settings.close();
    st.ed.run(shrew_core::Command::OpenConfig);
    st.process_events();
    let Some(path) = st.config_path.clone() else { return };
    let b = st.ed.buf();
    if at.is_empty() || b.doc.path.as_deref() != Some(path.as_path()) {
        return;
    }
    if let Some(n) = b.rope().lines().position(|l| l.chars().take(at.len()).eq(at.chars())) {
        st.ed.goto_line_col(n + 1, None);
    }
}

fn copy(st: &mut State, what: &str, text: String) {
    if text.is_empty() {
        return st.ed.error(format!("{what}: not available in this build"));
    }
    st.ed.clipboard.set(text);
    st.ed.info(format!("{what} copied to the clipboard"));
}

/// `claude mcp add shrew -- <srw> --mcp-stdio`, or (`json`) the `mcpServers` entry, for this executable.
fn mcp_snippet(json: bool) -> String {
    #[cfg(feature = "mcp")]
    {
        let exe = shrew_mcp::setup::launcher_exe().unwrap_or_else(|_| std::path::PathBuf::from("srw"));
        let s = shrew_mcp::setup::client_setup(&exe, None);
        if json { s.json } else { s.claude_code }
    }
    #[cfg(not(feature = "mcp"))]
    {
        let _ = json;
        String::new()
    }
}

fn regenerate_token(st: &mut State) {
    #[cfg(feature = "mcp")]
    {
        let r = match (&mut st.mcp, shrew_mcp::files::mcp_dir()) {
            (Some(s), _) => s.regenerate_token(),
            (None, Some(dir)) => shrew_mcp::server::regenerate_token(&dir),
            (None, None) => return st.ed.error("no config directory on this system"),
        };
        match r {
            Ok(_) => st.ed.info("new MCP token made: clients that send the old one are locked out"),
            Err(e) => st.ed.error(format!("MCP token: {e}")),
        }
    }
    #[cfg(not(feature = "mcp"))]
    st.ed.error("MCP is not compiled into this build");
}

/// A chord as people write it: `Ctrl+Shift+P`, `Alt+F3` (`Cmd` on macOS).
pub fn pretty_key(k: KeyPress) -> String {
    let m = k.mods;
    let mut s = String::new();
    if m.ctrl {
        s += if cfg!(target_os = "macos") { "Cmd+" } else { "Ctrl+" };
    }
    if m.alt {
        s += "Alt+";
    }
    if m.shift {
        s += "Shift+";
    }
    if m.logo {
        s += "Super+";
    }
    match k.key {
        Key::Char(' ') => s += "Space",
        Key::Char(c) => s.extend(c.to_uppercase()),
        Key::F(n) => s += &format!("F{n}"),
        other => s += &format!("{other:?}"),
    }
    s
}

/// `v` moved onto the step grid of `[lo, hi]`.
fn snap(v: f64, (lo, hi): (f64, f64), step: f64) -> f64 {
    let n = ((v - lo) / step).round();
    ((lo + n * step).clamp(lo, hi) * 1e6).round() / 1e6
}

impl State {
    /// Opens the panel (on `section` when it names one), or switches an open one to `section`.
    pub fn open_settings(&mut self, section: &str) {
        if !self.ed.features.on(Feature::Settings) {
            return self.ed.error("the settings panel is off (:set settings)");
        }
        let want = Section::find(section);
        if want.is_none() && !section.trim().is_empty() {
            let names: Vec<&str> = Section::ALL.iter().map(|s| s.label()).collect();
            self.ed.error(format!("no settings section {:?} ({})", section.trim(), names.join(", ")));
        }
        let at = want.map_or(self.settings.last, |s| Section::ALL.iter().position(|&x| x == s).unwrap_or(0));
        match &mut self.settings.panel {
            Some(p) => p.set_section(at),
            None => {
                let rows = rows::build(self);
                self.settings.panel = Some(Panel::new(rows, at));
            }
        }
    }

    fn panel(&mut self) -> &mut Panel {
        self.settings.panel.as_mut().expect("the settings panel is open")
    }

    /// Builds the rows again (an action may have changed the lists: imports, mappings), keeping the place.
    pub(crate) fn rebuild_rows(&mut self) {
        if self.settings.panel.is_none() {
            return;
        }
        let rows = rows::build(self);
        let p = self.panel();
        p.rows = rows.into();
        p.row = p.row.min(p.shown().len().saturating_sub(1));
    }

    /// A key while the panel is open (and no picker is over it).
    pub fn settings_key(&mut self, k: KeyPress) {
        let Some(p) = self.settings.panel.as_mut() else { return };
        self.ed.message = None;
        let m = k.mods;
        let armed = p.armed.take();
        if k.key == Key::Char(',') && m.ctrl {
            return self.settings.close();
        }
        if p.list.is_some() {
            return self.list_key(k);
        }
        if p.focus == Focus::Search {
            return self.search_key(k);
        }
        let quit =
            matches!(k.key, Key::Esc) || (m.ctrl && matches!(k.key, Key::Char('c' | '['))) || k.text() == Some('q');
        if quit {
            if p.searching() {
                p.query.clear();
                p.row = 0;
                p.follow = true;
            } else {
                self.settings.close();
            }
            return;
        }
        match k.text() {
            Some('/') => {
                p.focus = Focus::Search;
                return;
            }
            Some('R') => return self.reset_section(),
            _ => {}
        }
        match p.focus {
            Focus::Sections => self.sections_key(k),
            _ => self.rows_key(k, armed),
        }
    }

    fn sections_key(&mut self, k: KeyPress) {
        let p = self.panel();
        let (m, last) = (k.mods, Section::ALL.len() - 1);
        match (k.key, k.text()) {
            (Key::Down, _) | (_, Some('j')) => p.set_section(p.section + 1),
            (Key::Up, _) | (_, Some('k')) => p.set_section(p.section.saturating_sub(1)),
            (Key::Char('n'), _) if m.ctrl => p.set_section(p.section + 1),
            (Key::Char('p'), _) if m.ctrl => p.set_section(p.section.saturating_sub(1)),
            (Key::Home, _) | (_, Some('g')) => p.set_section(0),
            (Key::End, _) | (_, Some('G')) => p.set_section(last),
            (Key::Tab | Key::Right | Key::Enter, _) | (_, Some('l' | ' ')) => {
                p.focus = Focus::Rows;
                p.follow = true;
            }
            _ => {}
        }
    }

    fn rows_key(&mut self, k: KeyPress, armed: Option<usize>) {
        let p = self.panel();
        let m = k.mods;
        let cur = p.current();
        let number = cur.is_some_and(|i| matches!(p.rows[i].kind, Kind::Number { .. }));
        match (k.key, k.text()) {
            (Key::Down, _) | (_, Some('j')) => p.move_row(1),
            (Key::Up, _) | (_, Some('k')) => p.move_row(-1),
            (Key::Char('n'), _) if m.ctrl => p.move_row(1),
            (Key::Char('p'), _) if m.ctrl => p.move_row(-1),
            (Key::Char('d'), _) if m.ctrl => p.move_row(8),
            (Key::Char('u'), _) if m.ctrl => p.move_row(-8),
            (Key::PageDown, _) => p.move_row(8),
            (Key::PageUp, _) => p.move_row(-8),
            (Key::Home, _) | (_, Some('g')) => p.move_row(isize::MIN / 2),
            (Key::End, _) | (_, Some('G')) => p.move_row(isize::MAX / 2),
            (Key::Left | Key::Right, _) if number => {
                self.adjust(cur.expect("number row"), if k.key == Key::Left { -1.0 } else { 1.0 })
            }
            (_, Some('-')) => self.adjust_current(-1.0),
            (_, Some('+' | '=')) => self.adjust_current(1.0),
            (_, Some('<')) => self.adjust_current(-10.0),
            (_, Some('>')) => self.adjust_current(10.0),
            (Key::Tab | Key::Left, _) | (_, Some('h')) => {
                p.focus = Focus::Sections;
                if p.searching() {
                    // leaving the results for the sections ends the search
                    if let Some(i) = cur {
                        let s = p.rows[i].section;
                        p.section = Section::ALL.iter().position(|&x| x == s).unwrap_or(p.section);
                    }
                    p.query.clear();
                    p.row = 0;
                    p.scroll = 0.0;
                }
            }
            (Key::Enter, _) | (_, Some(' ')) => {
                if let Some(i) = cur {
                    self.activate(i, armed);
                }
            }
            (_, Some('r')) => {
                if let Some(i) = cur {
                    self.reset_row(i);
                }
            }
            _ => {}
        }
    }

    fn search_key(&mut self, k: KeyPress) {
        let p = self.panel();
        let m = k.mods;
        match k.key {
            Key::Esc => {
                p.query.clear();
                p.focus = Focus::Sections;
                p.row = 0;
            }
            Key::Char('c' | '[') if m.ctrl => {
                p.query.clear();
                p.focus = Focus::Sections;
            }
            Key::Enter | Key::Down | Key::Tab => {
                p.focus = if p.searching() && p.shown().is_empty() { Focus::Search } else { Focus::Rows };
                p.row = 0;
                p.follow = true;
            }
            Key::Backspace if m.ctrl => p.query.clear(),
            Key::Char('u' | 'w') if m.ctrl => p.query.clear(),
            Key::Backspace => {
                if p.query.pop().is_none() {
                    p.focus = Focus::Sections;
                }
            }
            _ => {
                if let Some(c) = k.text() {
                    p.query.push(c);
                }
            }
        }
        p.row = 0;
        p.scroll = 0.0;
    }

    fn list_key(&mut self, k: KeyPress) {
        let p = self.panel();
        let Some(l) = &mut p.list else { return };
        let m = k.mods;
        let n = l.options.len();
        let to = |l: &mut List, i: isize| l.selected = i.clamp(0, n.saturating_sub(1) as isize) as usize;
        match (k.key, k.text()) {
            (Key::Down, _) | (_, Some('j')) => to(l, l.selected as isize + 1),
            (Key::Up, _) | (_, Some('k')) => to(l, l.selected as isize - 1),
            (Key::Char('n'), _) if m.ctrl => to(l, l.selected as isize + 1),
            (Key::Char('p'), _) if m.ctrl => to(l, l.selected as isize - 1),
            (Key::PageDown, _) => to(l, l.selected as isize + 8),
            (Key::PageUp, _) => to(l, l.selected as isize - 8),
            (Key::Home, _) | (_, Some('g')) => to(l, 0),
            (Key::End, _) | (_, Some('G')) => to(l, n as isize),
            (Key::Enter, _) | (_, Some(' ' | 'l')) => {
                let pick = l.options.get(l.selected).map(|o| o.0.clone());
                self.choose_option(pick);
            }
            (Key::Esc, _) | (_, Some('q' | 'h')) => p.list = None,
            (Key::Char('c' | '['), _) if m.ctrl => p.list = None,
            _ => {}
        }
    }

    /// Closes the open list, setting its row to `value` when there is one.
    fn choose_option(&mut self, value: Option<String>) {
        let p = self.panel();
        let Some(l) = p.list.take() else { return };
        let rows = p.rows.clone();
        if let (Some(v), Kind::Options { set, get, .. }) = (value, &rows[l.row].kind)
            && get(self) != v
        {
            set(self, v);
            self.process_events();
        }
    }

    /// Enter, Space or a click on a row's control.
    fn activate(&mut self, i: usize, armed: Option<usize>) {
        let rows = self.panel().rows.clone();
        match &rows[i].kind {
            Kind::Toggle { get, set, .. } => match get(self) {
                Some(on) => set(self, !on),
                None => self.ed.error(format!("{} is not compiled into this build", rows[i].label)),
            },
            Kind::Number { .. } => self.ed.info("←/→ or -/+ change it, < > in bigger steps, r resets it"),
            Kind::Pick { choice, .. } => self.ed.events.push(Event::OpenChoice(*choice)),
            Kind::Options { options, get, .. } => {
                let options = options(self);
                let now = get(self);
                let selected = options.iter().position(|o| o.0 == now).unwrap_or(0);
                self.panel().list = Some(List { row: i, options, selected, top: 0 });
            }
            Kind::Action { run, confirm, .. } => {
                if let Some(text) = confirm
                    && armed != Some(i)
                {
                    self.panel().armed = Some(i);
                    return self.ed.info(*text);
                }
                run(self);
                self.process_events();
                return self.rebuild_rows();
            }
            Kind::Info { .. } | Kind::Heading => {}
        }
        self.process_events();
    }

    fn adjust_current(&mut self, steps: f64) {
        if let Some(i) = self.panel().current() {
            self.adjust(i, steps);
        }
    }

    /// Moves a number row by `steps` steps.
    fn adjust(&mut self, i: usize, steps: f64) {
        let rows = self.panel().rows.clone();
        if let Kind::Number { get, step, range, .. } = &rows[i].kind {
            let v = snap(get(self) + steps * step, *range, *step);
            self.set_number(&rows[i], v);
        }
    }

    fn set_number(&mut self, row: &Row, v: f64) {
        if let Kind::Number { get, set, range, step, .. } = &row.kind {
            let v = snap(v, *range, *step);
            if v != get(self) {
                set(self, v);
                self.process_events();
            }
        }
    }

    fn reset_row(&mut self, i: usize) {
        let rows = self.panel().rows.clone();
        let row = &rows[i];
        if row.reset(self) {
            self.ed.info(format!("{}: back to the default", row.label.trim()));
        } else if row.editable(self) {
            self.ed.info(format!("{} is at its default", row.label.trim()));
        }
    }

    /// `R`: every row of the section (or of the search results) back to its default.
    fn reset_section(&mut self) {
        let p = self.panel();
        let (rows, shown, name) = (p.rows.clone(), p.shown(), p.section().label());
        let n = shown.iter().filter(|&&i| rows[i].reset(self)).count();
        let what = if self.panel().searching() { "the results" } else { name };
        self.ed.info(match n {
            0 => format!("{what}: everything is at its default"),
            n => format!("{what}: {n} setting(s) back to the default"),
        });
    }

    // ----- mouse -----

    /// A left click. True when it grabbed a slider (the app then sends [`State::settings_drag`] until release).
    pub fn settings_click(&mut self, x: f32, y: f32) -> bool {
        let Some(p) = self.settings.panel.as_mut() else { return false };
        let h = &p.hits;
        p.armed = None;
        if p.list.is_some() {
            let hit = h.options.iter().find(|(r, _)| r.contains(x, y)).map(|o| o.1);
            match hit {
                Some(o) => {
                    let v = p.list.as_ref().and_then(|l| l.options.get(o)).map(|o| o.0.clone());
                    self.choose_option(v);
                }
                None => p.list = None,
            }
            return false;
        }
        if !h.panel.contains(x, y) {
            self.settings.close();
            return false;
        }
        if h.search.contains(x, y) {
            p.focus = Focus::Search;
            return false;
        }
        if let Some(&(_, s)) = h.sections.iter().find(|(r, _)| r.contains(x, y)) {
            p.set_section(s);
            p.focus = Focus::Sections;
            return false;
        }
        let Some(hit) = h.rows.iter().find(|r| r.rect.contains(x, y)).cloned() else { return false };
        p.row = hit.index;
        p.focus = Focus::Rows;
        p.follow = false;
        let Some(i) = p.current() else { return false };
        if !hit.control.contains(x, y) {
            return false;
        }
        if matches!(p.rows[i].kind, Kind::Number { .. }) {
            if hit.minus.contains(x, y) {
                self.adjust(i, -1.0);
            } else if hit.plus.contains(x, y) {
                self.adjust(i, 1.0);
            } else if hit.slider.contains(x, y) {
                self.panel().drag = Some(i);
                self.settings_drag(x);
                return true;
            }
            return false;
        }
        self.activate(i, None);
        false
    }

    /// The pointer moved while a slider is held.
    pub fn settings_drag(&mut self, x: f32) {
        let Some(p) = self.settings.panel.as_mut() else { return };
        let Some(i) = p.drag else { return };
        let Some(hit) = p.hits.rows.iter().find(|h| p.shown().get(h.index) == Some(&i)) else { return };
        let track = hit.slider;
        let rows = p.rows.clone();
        if let Kind::Number { range: (lo, hi), .. } = rows[i].kind {
            let f = ((x - track.x) / track.w.max(1.0)).clamp(0.0, 1.0) as f64;
            self.set_number(&rows[i], lo + f * (hi - lo));
        }
    }

    pub fn settings_release(&mut self) {
        if let Some(p) = &mut self.settings.panel {
            p.drag = None;
        }
    }

    /// The wheel over the panel: scrolls the rows (or an open list) by `dy` pixels.
    pub fn settings_wheel(&mut self, dy: f32) {
        let Some(p) = self.settings.panel.as_mut() else { return };
        if let Some(l) = &mut p.list {
            let rows = (dy / p.hits.list_row_h.max(1.0)).round() as isize;
            let n = l.options.len();
            l.top = (l.top as isize + rows).clamp(0, n.saturating_sub(p.hits.list_rows) as isize) as usize;
            return;
        }
        p.scroll = (p.scroll + dy).clamp(0.0, p.hits.max_scroll);
        p.follow = false;
    }
}
