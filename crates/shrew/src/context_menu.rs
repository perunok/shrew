//! Context menus: one popup that every part of the UI opens the same way, and the list drawing the title bar's
//! dropdown menus share, so all of shrew's menus look alike.
//!
//! A [`ContextMenu`] is a list of [`MenuItem`]s (actions, separators, submenus) for a [`Target`] (what was
//! right-clicked: a tab, the text, a sidebar row...). One menu is open at a time, in `State::menu`; opening it
//! closes the title bar's menus and the other way round. It is drawn at the pointer and kept on screen (it opens to
//! the left or upwards near the window's edges), above everything else.
//!
//! - Keys (modal while it is open): Up/Down or j/k move, Home/End, Right/Enter open a submenu, Left/Esc close one,
//!   Enter or Space runs, Esc closes. A letter runs the item it is the mnemonic of, else jumps to the next item
//!   starting with what was typed.
//! - Mouse: hovering highlights (and opens submenus); a click runs; a click outside closes; a second right click on
//!   the same spot closes it, a right click elsewhere opens the menu of what is there. Pressing the right button,
//!   dragging onto an item and releasing runs it, as with native menus.
//!
//! Adding a menu for a target: build it in [`for_target`] (one match arm calling a function of yours), e.g.
//!
//! ```ignore
//! Target::Tab(i) => crate::tabs::menu(st, i),   // -> Option<ContextMenu>
//! ```
//!
//! with items from [`Action::command`] (label, shortcut hint and check mark from the command and your key
//! bindings), [`Action::ex`] (an ex line: a command with an argument) or [`Action::app`] (a function of yours, given
//! the menu's target).

use crate::colors::{Colors, mix};
use crate::draw::{line, popup};
use crate::layout::Layout;
use crate::mouse::Target;
use crate::state::State;
use shrew_core::features::Feature;
use shrew_core::{Command, Editor, Key, KeyPress, MapMode, Mode, MouseButton, Rhs};
use shrew_render::{Rect, Scene, TextStyle, columns};
use std::time::{Duration, Instant};

/// Typed letters within this long of each other make one prefix ("sa" for Save All).
const TYPE_AHEAD: Duration = Duration::from_millis(900);
/// The scene layer of the top level; each submenu draws one higher, over its parent.
const LAYER: usize = 5;

/// An app-level action: called with the state, the menu's target and the item's argument.
pub type AppFn = fn(&mut State, &Target, usize);

/// What an item does.
#[derive(Clone, Debug)]
// the parts of this API no menu uses yet are there for the menus to come
#[cfg_attr(not(test), allow(dead_code))]
pub enum Run {
    Command(Command),
    /// An ex command line (`"e src/main.rs"`, `"Gcheckout main"`): a command with an argument.
    Ex(String),
    /// A function of the app, and the argument it is called with.
    App(AppFn, usize),
}

/// A menu entry that runs something.
#[derive(Clone, Debug)]
pub struct Action {
    pub label: String,
    /// The chord that runs it, shown at the right.
    pub hint: Option<String>,
    pub enabled: bool,
    /// A toggle that is on (drawn with a check mark).
    pub checked: bool,
    /// The letter that runs it while the menu is open (underlined in the label).
    pub mnemonic: Option<char>,
    pub run: Run,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Action {
    pub fn new(label: impl Into<String>, run: Run) -> Action {
        Action { label: label.into(), hint: None, enabled: true, checked: false, mnemonic: None, run }
    }

    /// A command: its title, the chord that runs it now (your mappings first) and, for a toggle, whether it is on.
    pub fn command(ed: &Editor, cmd: Command) -> Action {
        Action {
            hint: shortcut(ed, cmd),
            checked: checked(ed, cmd),
            ..Action::new(short_title(cmd.title()), Run::Command(cmd))
        }
    }

    /// An ex command line, run as if typed after `:`.
    pub fn ex(label: impl Into<String>, line: impl Into<String>) -> Action {
        Action::new(label, Run::Ex(line.into()))
    }

    /// A function of the app (`|st, target, _| ...`), called with the menu's target.
    pub fn app(label: impl Into<String>, f: AppFn) -> Action {
        Action::new(label, Run::App(f, 0))
    }

    /// [`Action::app`] with an argument (which file, which tab...).
    pub fn app_with(label: impl Into<String>, f: AppFn, arg: usize) -> Action {
        Action::new(label, Run::App(f, arg))
    }

    pub fn label(mut self, label: impl Into<String>) -> Action {
        self.label = label.into();
        self
    }

    pub fn hint(mut self, hint: Option<String>) -> Action {
        self.hint = hint;
        self
    }

    pub fn enabled(mut self, on: bool) -> Action {
        self.enabled = on;
        self
    }

    pub fn checked(mut self, on: bool) -> Action {
        self.checked = on;
        self
    }

    pub fn mnemonic(mut self, c: char) -> Action {
        self.mnemonic = Some(c.to_ascii_lowercase());
        self
    }
}

/// A nested list, opened to the side of its row.
#[derive(Clone, Debug)]
pub struct Submenu {
    pub label: String,
    pub enabled: bool,
    pub mnemonic: Option<char>,
    pub items: Vec<MenuItem>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Submenu {
    pub fn new(label: impl Into<String>, items: Vec<MenuItem>) -> Submenu {
        Submenu { label: label.into(), enabled: true, mnemonic: None, items }
    }
}

#[derive(Clone, Debug)]
pub enum MenuItem {
    Action(Action),
    Separator,
    Submenu(Submenu),
}

impl From<Action> for MenuItem {
    fn from(a: Action) -> MenuItem {
        MenuItem::Action(a)
    }
}

impl From<Submenu> for MenuItem {
    fn from(s: Submenu) -> MenuItem {
        MenuItem::Submenu(s)
    }
}

impl MenuItem {
    pub fn is_separator(&self) -> bool {
        matches!(self, MenuItem::Separator)
    }

    pub fn label(&self) -> &str {
        match self {
            MenuItem::Action(a) => &a.label,
            MenuItem::Submenu(s) => &s.label,
            MenuItem::Separator => "",
        }
    }

    /// It can be highlighted and chosen (a separator cannot).
    pub fn enabled(&self) -> bool {
        match self {
            MenuItem::Action(a) => a.enabled,
            MenuItem::Submenu(s) => s.enabled && !s.items.is_empty(),
            MenuItem::Separator => false,
        }
    }

    fn mnemonic(&self) -> Option<char> {
        match self {
            MenuItem::Action(a) => a.mnemonic,
            MenuItem::Submenu(s) => s.mnemonic,
            MenuItem::Separator => None,
        }
    }

    fn hint(&self) -> Option<&str> {
        match self {
            MenuItem::Action(a) => a.hint.as_deref(),
            _ => None,
        }
    }
}

/// No separator first, last or twice in a row, at every level.
pub fn tidy(items: &mut Vec<MenuItem>) {
    let mut prev_sep = true;
    items.retain(|e| {
        let keep = !(e.is_separator() && prev_sep);
        prev_sep = e.is_separator();
        keep
    });
    if items.last().is_some_and(MenuItem::is_separator) {
        items.pop();
    }
    for e in items {
        if let MenuItem::Submenu(s) = e {
            tidy(&mut s.items);
        }
    }
}

/// Items from command names: `name`, `name|Label` to name it differently here, or `-` for a separator. A command
/// this build lacks is left out.
pub fn items_from_spec(ed: &Editor, spec: &[&str]) -> Vec<MenuItem> {
    let mut out = Vec::new();
    for s in spec {
        if *s == "-" {
            out.push(MenuItem::Separator);
            continue;
        }
        let (name, label) = s.split_once('|').unwrap_or((s, ""));
        let Some(cmd) = Command::from_name(name) else { continue };
        let a = Action::command(ed, cmd);
        out.push(MenuItem::Action(if label.is_empty() { a } else { a.label(label) }));
    }
    tidy(&mut out);
    out
}

/// What a key, click or release on the open menu did.
#[derive(Clone, Debug)]
pub enum Outcome {
    Stay,
    Close,
    Run(Run),
}

/// Where one level (the menu, or an open submenu) was drawn, and its rows.
#[derive(Clone, Debug, Default)]
pub struct Level {
    pub panel: Rect,
    /// Row rects and the index of the item each shows.
    pub rows: Vec<(Rect, usize)>,
}

/// A mouse button held since it opened the menu or pressed on it: releasing it on an item runs the item.
#[derive(Clone, Copy, Debug)]
struct Hold {
    button: MouseButton,
    from: (f32, f32),
    /// The press opened the menu: a release on the spot leaves it open.
    opened: bool,
}

#[derive(Debug)]
pub struct ContextMenu {
    pub items: Vec<MenuItem>,
    /// What was clicked; [`Run::App`] functions get it.
    pub target: Target,
    /// Where it opened (physical pixels).
    pub at: (f32, f32),
    /// The open submenus, from the top: the index of each one's item in its parent.
    pub path: Vec<usize>,
    /// The highlighted item of the deepest open level.
    pub selected: Option<usize>,
    /// Each open level as drawn in the last frame.
    pub hits: Vec<Level>,
    /// The first item shown per level, when one is taller than the window.
    tops: Vec<usize>,
    hold: Option<Hold>,
    typed: String,
    typed_at: Option<Instant>,
}

impl ContextMenu {
    pub fn new(target: Target, mut items: Vec<MenuItem>) -> ContextMenu {
        tidy(&mut items);
        ContextMenu {
            items,
            target,
            at: (0.0, 0.0),
            path: Vec::new(),
            selected: None,
            hits: Vec::new(),
            tops: Vec::new(),
            hold: None,
            typed: String::new(),
            typed_at: None,
        }
    }

    /// From command names (see [`items_from_spec`]).
    pub fn from_spec(ed: &Editor, target: Target, spec: &[&str]) -> ContextMenu {
        ContextMenu::new(target, items_from_spec(ed, spec))
    }

    /// Highlights the first item (a menu opened from the keyboard; one opened by the mouse starts with none).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn select_first(mut self) -> ContextMenu {
        self.selected = self.step(true);
        self
    }

    /// The items of open level `depth` (0: the menu itself).
    pub fn level(&self, depth: usize) -> &[MenuItem] {
        level_of(&self.items, &self.path, depth)
    }

    fn deepest(&self) -> &[MenuItem] {
        self.level(self.path.len())
    }

    /// The highlighted item of level `depth`: the open submenu's row above the deepest level.
    pub fn highlighted(&self, depth: usize) -> Option<usize> {
        if depth < self.path.len() { Some(self.path[depth]) } else { self.selected }
    }

    /// The next enabled item after (or before) the highlighted one, wrapping around.
    fn step(&self, forward: bool) -> Option<usize> {
        let items = self.deepest();
        let n = items.len();
        if n == 0 {
            return None;
        }
        let start = self.selected.unwrap_or(if forward { n - 1 } else { 0 });
        (1..=n).map(|k| if forward { (start + k) % n } else { (start + n - k % n) % n }).find(|&i| items[i].enabled())
    }

    fn edge(&self, last: bool) -> Option<usize> {
        let items = self.deepest();
        if last { items.iter().rposition(MenuItem::enabled) } else { items.iter().position(MenuItem::enabled) }
    }

    /// Enter on an item: an action runs, a submenu opens.
    fn activate(&mut self, i: usize) -> Outcome {
        match self.deepest().get(i) {
            Some(MenuItem::Action(a)) if a.enabled => Outcome::Run(a.run.clone()),
            Some(e @ MenuItem::Submenu(_)) if e.enabled() => {
                self.path.push(i);
                self.selected = self.edge(false);
                Outcome::Stay
            }
            _ => Outcome::Stay,
        }
    }

    pub fn key(&mut self, k: KeyPress, now: Instant) -> Outcome {
        let m = k.mods;
        if !matches!(k.key, Key::Char(_)) || m.ctrl || m.alt {
            self.typed.clear();
        }
        match k.key {
            Key::Esc | Key::Left if !self.path.is_empty() => self.selected = self.path.pop(),
            Key::Esc | Key::F(10) => return Outcome::Close,
            Key::Up | Key::Down => self.selected = self.step(k.key == Key::Down),
            Key::Home | Key::PageUp => self.selected = self.edge(false),
            Key::End | Key::PageDown => self.selected = self.edge(true),
            Key::Right => {
                if let Some(i) = self.selected.filter(|&i| matches!(self.deepest()[i], MenuItem::Submenu(_))) {
                    return self.activate(i);
                }
            }
            Key::Enter | Key::Char(' ') => match self.selected {
                Some(i) => return self.activate(i),
                None => self.selected = self.edge(false),
            },
            Key::Char(c) if !m.ctrl && !c.is_control() => return self.letter(c, now),
            _ => {}
        }
        Outcome::Stay
    }

    /// A letter: the item it is the mnemonic of (the next one when several share it), else j/k move, else type-ahead.
    fn letter(&mut self, c: char, now: Instant) -> Outcome {
        let lc = c.to_ascii_lowercase();
        let items = self.deepest();
        let marked: Vec<usize> =
            (0..items.len()).filter(|&i| items[i].enabled() && items[i].mnemonic() == Some(lc)).collect();
        match marked.as_slice() {
            [i] => return self.activate(*i),
            [first, ..] => {
                let next = marked.iter().find(|&&i| self.selected.is_some_and(|s| i > s)).unwrap_or(first);
                self.selected = Some(*next);
                return Outcome::Stay;
            }
            [] => {}
        }
        if c == 'j' || c == 'k' {
            self.typed.clear();
            self.selected = self.step(c == 'j');
            return Outcome::Stay;
        }
        if self.typed_at.is_none_or(|t| now.duration_since(t) > TYPE_AHEAD) {
            self.typed.clear();
        }
        self.typed_at = Some(now);
        self.typed.extend(c.to_lowercase());
        let found = self.find_prefix(&self.typed, true).or_else(|| {
            // a new letter that continues nothing starts over: repeated letters cycle through the matches
            self.typed = c.to_lowercase().collect();
            self.find_prefix(&self.typed, false)
        });
        if found.is_some() {
            self.selected = found;
        }
        Outcome::Stay
    }

    /// The first enabled item from the highlighted one on (`here`: including it) whose label starts with `prefix`.
    fn find_prefix(&self, prefix: &str, here: bool) -> Option<usize> {
        let items = self.deepest();
        let n = items.len();
        let start = self.selected.map_or(0, |s| s + usize::from(!here));
        (0..n)
            .map(|k| (start + k) % n)
            .find(|&i| items[i].enabled() && items[i].label().to_lowercase().starts_with(prefix))
    }

    pub fn inside(&self, x: f32, y: f32) -> bool {
        self.hits.iter().any(|l| l.panel.contains(x, y))
    }

    /// The (level, item) under the pointer, the deepest level first (it is drawn on top).
    fn item_at(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        let (d, l) = self.hits.iter().enumerate().rev().find(|(_, l)| l.panel.contains(x, y))?;
        l.rows.iter().find(|(r, _)| r.contains(x, y)).map(|&(_, i)| (d, i))
    }

    /// The pointer moved: the item under it is highlighted, a submenu under it opens. True when that changed
    /// something (the menu needs drawing again).
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let Some((d, i)) = self.item_at(x, y) else { return false };
        let before = (self.path.len(), self.path.last().copied(), self.selected);
        if self.path.get(d) == Some(&i) {
            // the row of the open submenu: it stays open, nothing in it highlighted
            self.path.truncate(d + 1);
            self.selected = None;
        } else {
            self.path.truncate(d);
            let e = &self.deepest()[i];
            match e {
                MenuItem::Submenu(_) if e.enabled() => {
                    self.path.push(i);
                    self.selected = None;
                }
                e => self.selected = e.enabled().then_some(i),
            }
        }
        self.hits.truncate(self.path.len() + 1);
        before != (self.path.len(), self.path.last().copied(), self.selected)
    }

    /// A button went down at `from` while the menu is open (`opened`: that press opened it).
    pub fn hold(&mut self, button: MouseButton, from: (f32, f32), opened: bool) {
        self.hold = Some(Hold { button, from, opened });
    }

    /// A press: on the menu it highlights (the item runs on release); anywhere else closes the menu.
    pub fn press(&mut self, x: f32, y: f32, button: MouseButton) -> Outcome {
        if !self.inside(x, y) {
            return Outcome::Close;
        }
        self.hover(x, y);
        self.hold(button, (x, y), false);
        Outcome::Stay
    }

    /// A release: on an item, after a press on the menu or a drag from the press that opened it, the item runs. A
    /// drag that ends outside the menu closes it. `slop`: how far (pixels) the pointer must have moved.
    pub fn release(&mut self, x: f32, y: f32, button: MouseButton, slop: f32) -> Outcome {
        let Some(h) = self.hold.filter(|h| h.button == button) else { return Outcome::Stay };
        self.hold = None;
        let moved = (h.from.0 - x).hypot(h.from.1 - y) > slop;
        if h.opened && !moved {
            return Outcome::Stay;
        }
        match self.item_at(x, y) {
            Some((d, i)) => match level_of(&self.items, &self.path, d).get(i) {
                Some(MenuItem::Action(a)) if a.enabled => Outcome::Run(a.run.clone()),
                _ => Outcome::Stay,
            },
            None if h.opened && !self.inside(x, y) => Outcome::Close,
            None => Outcome::Stay,
        }
    }
}

fn level_of<'a>(items: &'a [MenuItem], path: &[usize], depth: usize) -> &'a [MenuItem] {
    let mut items = items;
    for &i in path.iter().take(depth) {
        match items.get(i) {
            Some(MenuItem::Submenu(s)) => items = &s.items,
            _ => return &[],
        }
    }
    items
}

// ----- the state's one menu -----

/// Opens `menu` at `at`, closing whatever menu was open. A menu with nothing in it does not open. True when open.
pub fn open(st: &mut State, mut menu: ContextMenu, at: (f32, f32)) -> bool {
    st.titlebar.menu = None;
    if menu.items.is_empty() {
        st.menu = None;
        return false;
    }
    menu.at = at;
    st.menu = Some(menu);
    true
}

/// The menu for what was right-clicked, opened at `at`. True when one opened. The title bar and the Source Control
/// panel open theirs before this (see `Mouse::right_press`).
pub fn for_target(st: &mut State, target: Target, at: (f32, f32)) -> bool {
    let menu = match target {
        Target::TitleBar => crate::titlebar::context_menu(st),
        // one arm per target with a menu, e.g. `Target::Tab(i) => crate::tabs::menu(st, i)`
        _ => None,
    };
    menu.is_some_and(|m| open(st, m, at))
}

/// A key while the menu is open: true (the menu takes every key) unless none is open.
pub fn key(st: &mut State, k: KeyPress) -> bool {
    let Some(m) = &mut st.menu else { return false };
    let out = m.key(k, Instant::now());
    apply(st, out);
    true
}

/// Carries out what the menu said: closing, or running an item (the menu closes first).
pub fn apply(st: &mut State, out: Outcome) {
    match out {
        Outcome::Stay => {}
        Outcome::Close => st.menu = None,
        Outcome::Run(run) => {
            let target = st.menu.take().map_or(Target::Nothing, |m| m.target);
            self::run(st, &target, run);
        }
    }
}

/// Runs an item's action as if the menu had been there (for tests and keyboard openers too).
pub fn run(st: &mut State, target: &Target, run: Run) {
    st.ed.message = None;
    match run {
        Run::Command(cmd) => st.ed.run(cmd),
        Run::Ex(line) => st.ed.run_ex(&line),
        Run::App(f, arg) => f(st, target, arg),
    }
    st.after_input();
}

// ----- drawing -----

/// Height of an item row and of a separator.
pub fn row_h(lay: &Layout) -> f32 {
    (lay.ch + 1.5 * lay.pad).round()
}

pub fn sep_h(lay: &Layout) -> f32 {
    (2.0 * lay.pad + 1.0).round()
}

fn item_h(e: &MenuItem, lay: &Layout) -> f32 {
    if e.is_separator() { sep_h(lay) } else { row_h(lay) }
}

/// The width a list needs and its full height (both with the panel's padding).
pub fn list_size(items: &[MenuItem], lay: &Layout) -> (f32, f32) {
    let label_cols = items.iter().map(|e| columns(e.label())).max().unwrap_or(0);
    let hint_cols = items.iter().filter_map(MenuItem::hint).map(columns).max().unwrap_or(0);
    let sub = items.iter().any(|e| matches!(e, MenuItem::Submenu(_)));
    let cols = 3 + label_cols + if hint_cols > 0 { 4 + hint_cols } else { 0 } + if sub { 3 } else { 0 } + 2;
    let h = items.iter().map(|e| item_h(e, lay)).sum::<f32>() + 2.0 * lay.pad;
    ((cols.max(20) as f32 * lay.cw).round(), h)
}

/// The first item to show so the highlighted one stays in view in a panel `h` high (0 when everything fits).
pub fn scroll_top(items: &[MenuItem], highlight: Option<usize>, top: usize, h: f32, lay: &Layout) -> usize {
    let pad2 = 2.0 * lay.pad;
    if items.iter().map(|e| item_h(e, lay)).sum::<f32>() + pad2 <= h {
        return 0;
    }
    let mut top = top.min(items.len().saturating_sub(1));
    if let Some(i) = highlight.filter(|&i| i < items.len()) {
        top = top.min(i);
        while top < i && items[top..=i].iter().map(|e| item_h(e, lay)).sum::<f32>() + pad2 > h {
            top += 1;
        }
    }
    top
}

/// A `w` x `h` menu with its corner at the pointer, inside `bounds`: it opens to the left when there is no room on
/// the right, upwards when there is none below.
pub fn place(at: (f32, f32), w: f32, h: f32, bounds: Rect) -> Rect {
    let x = if at.0 + w > bounds.right() { at.0 - w } else { at.0 };
    let y = if at.1 + h > bounds.bottom() { at.1 - h } else { at.1 };
    fit(x, y, w, h, bounds)
}

/// A submenu beside `row` of the `parent` panel: on its right, or its left when there is no room; its first row
/// level with `row`, moved up to fit.
pub fn place_sub(parent: Rect, row: Rect, w: f32, h: f32, bounds: Rect, pad: f32) -> Rect {
    let x = if parent.right() + w > bounds.right() { parent.x - w } else { parent.right() };
    fit(x, row.y - pad, w, h, bounds)
}

fn fit(x: f32, y: f32, w: f32, h: f32, bounds: Rect) -> Rect {
    let x = x.min(bounds.right() - w).max(bounds.x);
    let y = y.min(bounds.bottom() - h).max(bounds.y);
    Rect::new(x.round(), y.round(), w, h.min(bounds.h))
}

/// The open context menu and its open submenus, over everything else.
pub fn draw(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors) {
    let Some(m) = &mut st.menu else { return };
    let levels = m.path.len() + 1;
    let mut hits = std::mem::take(&mut m.hits);
    hits.resize_with(levels, Level::default);
    m.tops.resize(levels, 0);
    let margin = (lay.pad / 2.0).round();
    let bounds = Rect::new(margin, margin, (lay.width - 2.0 * margin).max(1.0), (lay.height - 2.0 * margin).max(1.0));
    let min_h = row_h(lay) + 2.0 * lay.pad;
    let mut parent: Option<(Rect, Rect)> = None;
    for (d, lvl) in hits.iter_mut().enumerate() {
        let items = level_of(&m.items, &m.path, d);
        let (w, full) = list_size(items, lay);
        let h = full.min(bounds.h.max(min_h)).round();
        let panel = match parent {
            None => place(m.at, w, h, bounds),
            Some((p, row)) => place_sub(p, row, w, h, bounds, lay.pad),
        };
        let hl = m.highlighted(d);
        let top = scroll_top(items, hl, m.tops[d], h, lay);
        m.tops[d] = top;
        scene.layer(LAYER + d);
        popup(scene, panel, lay, c);
        lvl.panel = panel;
        lvl.rows.clear();
        draw_list(scene, items, hl, panel, top, lay, c, &mut lvl.rows);
        // the next level opens beside its row (or the panel's top when that row is scrolled away)
        let row = m.path.get(d).and_then(|&i| lvl.rows.iter().find(|r| r.1 == i)).map_or(panel, |r| r.0);
        parent = Some((panel, row));
    }
    m.hits = hits;
    scene.layer(0);
}

/// The rows of a menu list in `panel`, from item `top` on, `highlight` highlighted; fills `rows` with where each
/// item went. The title bar's dropdowns draw with this too.
#[allow(clippy::too_many_arguments)]
pub fn draw_list(
    scene: &mut Scene,
    items: &[MenuItem],
    highlight: Option<usize>,
    panel: Rect,
    top: usize,
    lay: &Layout,
    c: &Colors,
    rows: &mut Vec<(Rect, usize)>,
) {
    let (cw, pad, s) = (lay.cw, lay.pad, lay.scale);
    let (row_h, sep_h) = (row_h(lay), sep_h(lay));
    let inner = Rect::new(panel.x, panel.y + pad, panel.w, panel.h - 2.0 * pad);
    scene.push_clip(inner);
    let mut ry = inner.y;
    for (i, e) in items.iter().enumerate().skip(top) {
        if ry >= inner.bottom() {
            break;
        }
        if e.is_separator() {
            let r = Rect::new(panel.x + pad, (ry + sep_h / 2.0).floor(), panel.w - 2.0 * pad, 1.0);
            scene.rect(r, c.panel_border);
            ry += sep_h;
            continue;
        }
        let row = Rect::new(panel.x + pad, ry, panel.w - 2.0 * pad, row_h);
        let enabled = e.enabled();
        if highlight == Some(i) && enabled {
            scene.rounded_rect(row, (5.0 * s).round(), c.panel_selection, None);
        }
        let ty = lay.centered(row.y, row.h);
        if let MenuItem::Action(a) = e
            && a.checked
        {
            let (cx, cy, k) = (row.x + 1.2 * cw, (row.y + row.h / 2.0).round(), 3.0 * s);
            let ink = if enabled { c.accent } else { c.panel_muted };
            line(scene, (cx - k, cy), (cx - k * 0.2, cy + k * 0.8), 1.4 * s, ink);
            line(scene, (cx - k * 0.2, cy + k * 0.8), (cx + k * 1.3, cy - k), 1.4 * s, ink);
        }
        let fg = if enabled { c.panel_foreground } else { mix(c.panel_muted, c.popup, 0.75) };
        let label = e.label();
        // the mnemonic letter underlined
        let u = e.mnemonic().and_then(|m| label.char_indices().find(|(_, ch)| ch.to_ascii_lowercase() == m));
        let span = u.map(|(b, ch)| (b..b + ch.len_utf8(), TextStyle::new(fg).underline(fg)));
        scene.text((row.x + 2.5 * cw).round(), ty, label, TextStyle::new(fg), span.as_slice());
        if let Some(k) = e.hint() {
            let kx = row.right() - (columns(k) as f32 + 1.0) * cw;
            let ink = if enabled { c.panel_muted } else { mix(c.panel_muted, c.popup, 0.6) };
            scene.text(kx.round(), ty, k, TextStyle::new(ink), &[]);
        }
        if matches!(e, MenuItem::Submenu(_)) {
            let (hx, hy, k) = (row.right() - 1.5 * cw, (row.y + row.h / 2.0).round(), 3.0 * s);
            line(scene, (hx, hy - k), (hx + k, hy), 1.2 * s, c.panel_muted);
            line(scene, (hx + k, hy), (hx, hy + k), 1.2 * s, c.panel_muted);
        }
        rows.push((row.intersect(&inner), i));
        ry += row_h;
    }
    scene.pop_clip();
}

// ----- labels, check marks and shortcut hints -----

/// "Edit: Toggle Comment" -> "Toggle Comment".
pub fn short_title(title: &str) -> String {
    title.split_once(": ").map_or(title, |(_, t)| t).to_string()
}

/// A toggle command's state, for its check mark.
pub fn checked(ed: &Editor, cmd: Command) -> bool {
    let f = |f: Feature| ed.features.on(f);
    match cmd.name() {
        "toggle_line_numbers" => f(Feature::LineNumbers),
        "toggle_relative_numbers" => f(Feature::RelativeNumbers),
        "toggle_whitespace" => f(Feature::Whitespace),
        "toggle_syntax" => f(Feature::Syntax),
        "toggle_vim" => f(Feature::Vim),
        "system_titlebar" => !f(Feature::CustomTitlebar),
        "toggle_sidebar" => f(Feature::Sidebar),
        "toggle_minimap" => f(Feature::Minimap),
        // ----- git -----
        "git_toggle_gutter" => f(Feature::GitGutter),
        "git_toggle_blame_inline" => f(Feature::GitBlameInline),
        _ => false,
    }
}

/// The chord that runs `cmd` now: a mapping of yours (current mode) first, else a built-in global chord.
pub fn shortcut(ed: &Editor, cmd: Command) -> Option<String> {
    if cmd == Command::ShowMenu {
        return Some("F10".into());
    }
    let mode = if ed.mode == Mode::Insert || !ed.vim() {
        MapMode::Insert
    } else if ed.mode.is_visual() {
        MapMode::Visual
    } else {
        MapMode::Normal
    };
    let mapped = ed.keymap.mappings(mode).find(|m| m.rhs == Rhs::Command(cmd) && m.lhs.len() == 1);
    if let Some(m) = mapped {
        return Some(chord_label(m.lhs[0]));
    }
    // the same list the Settings panel shows, so the two never disagree
    ed.chords().into_iter().find(|&(_, c)| c == cmd).map(|(k, _)| chord_label(k))
}

/// "Ctrl+Shift+P", "Alt+F3" (Cmd for Ctrl on macOS).
pub fn chord_label(k: KeyPress) -> String {
    let mut s = String::new();
    let m = k.mods;
    if m.ctrl {
        s += if cfg!(target_os = "macos") { "Cmd+" } else { "Ctrl+" };
    }
    if m.alt {
        s += if cfg!(target_os = "macos") { "Option+" } else { "Alt+" };
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
        Key::PageUp => s += "PageUp",
        Key::PageDown => s += "PageDown",
        Key::Esc => s += "Esc",
        other => s += &format!("{other:?}"),
    }
    s
}

#[cfg(test)]
mod tests;
