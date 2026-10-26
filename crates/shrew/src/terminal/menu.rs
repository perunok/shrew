//! The terminal panel's context menu (a right click on its screen or tab strip) and the middle-click paste of the
//! primary selection.
//!
//! A program that reads the mouse (vim, htop, tmux with `mouse on`) gets the right button as usual; Shift+right
//! click opens the menu anyway, as in other terminals. A right click on a tab of the strip selects that terminal
//! first.

use super::{Geom, Terminals, input, normal_mode};
use crate::context_menu::{Action, ContextMenu, MenuItem, chord_label, shortcut};
use crate::mouse::Target;
use crate::state::State;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use shrew_core::features::Feature;
use shrew_core::{Command, Key, KeyPress, Mods};

impl Terminals {
    /// A right press over the panel: true when it opens the menu (the terminal under it is focused, a tab under it
    /// selected), false when the program in the terminal takes the press.
    pub(super) fn right_press(&mut self, g: &Geom, x: f32, y: f32, mods: Mods) -> bool {
        if g.strip.contains(x, y) {
            if let Some(&(_, i)) = self.tab_hits.iter().find(|(r, _)| r.contains(x, y)) {
                self.select(i);
            }
        } else if let Some(tab) = self.tabs.get(self.active)
            && tab.mode().intersects(TermMode::MOUSE_MODE)
            && !mods.shift
            && !normal_mode(tab)
        {
            return false;
        }
        self.set_focus(true);
        true
    }
}

fn chord(s: &str) -> Option<String> {
    KeyPress::parse(s).map(chord_label)
}

/// The panel's menu: copy and paste, select all, clear, and the terminal commands.
pub fn menu(st: &mut State) -> Option<ContextMenu> {
    let ed = &st.ed;
    let t = &st.terminals;
    let tab = t.tabs.get(t.active)?;
    let selection = tab.session.term.lock().selection_to_string().is_some_and(|s| !s.is_empty());
    let live = !tab.session.exited();
    let cmd = |c| Action::command(ed, c);
    let items: Vec<MenuItem> = vec![
        Action::app("Copy", copy).hint(chord("<C-S-c>")).enabled(selection).mnemonic('c').into(),
        Action::app("Paste", paste).hint(chord("<C-S-v>")).enabled(live).mnemonic('p').into(),
        Action::app("Select All", select_all).mnemonic('a').into(),
        Action::app("Clear", clear).mnemonic('l').into(),
        MenuItem::Separator,
        cmd(Command::NewTerminal).label("New Terminal").mnemonic('n').into(),
        cmd(Command::TerminalNormalMode)
            .label("Normal Mode (Scroll, Search)")
            .hint(Some("Ctrl+\\ Ctrl+N".into()))
            .into(),
        cmd(Command::MaximizeTerminal).label("Maximized").checked(t.maximized).mnemonic('x').into(),
        MenuItem::Separator,
        cmd(Command::CloseTerminal).label("Kill Terminal").mnemonic('k').into(),
        Action::app("Close Panel", hide).hint(shortcut(ed, Command::ToggleTerminal)).into(),
    ];
    Some(ContextMenu::new(Target::Terminal, items))
}

fn copy(st: &mut State, _: &Target, _: usize) {
    let t = &st.terminals;
    let Some(tab) = t.tabs.get(t.active) else { return };
    if let Some(text) = tab.session.term.lock().selection_to_string().filter(|s| !s.is_empty()) {
        st.ed.clipboard.set(text);
        st.ed.info("copied");
    }
}

fn paste(st: &mut State, _: &Target, _: usize) {
    if let Some(text) = st.ed.clipboard.get() {
        st.terminals.paste(&text);
    }
}

/// Selects the whole history and screen.
fn select_all(st: &mut State, _: &Target, _: usize) {
    let t = &mut st.terminals;
    let Some(tab) = t.tabs.get_mut(t.active) else { return };
    let mut term = tab.session.term.lock();
    let (top, bottom, right) = (term.topmost_line(), term.bottommost_line(), term.last_column());
    let mut sel = Selection::new(SelectionType::Simple, Point::new(top, Column(0)), Side::Left);
    sel.update(Point::new(bottom, right), Side::Right);
    term.selection = Some(sel);
    tab.view.stale = true;
}

/// Drops the scrollback and has the shell redraw its prompt on a clean screen (Ctrl+L); a full-screen program
/// keeps its screen.
fn clear(st: &mut State, _: &Target, _: usize) {
    let t = &mut st.terminals;
    let Some(tab) = t.tabs.get_mut(t.active) else { return };
    let alt = {
        let mut term = tab.session.term.lock();
        term.scroll_display(Scroll::Bottom);
        term.grid_mut().clear_history();
        term.selection = None;
        term.mode().contains(TermMode::ALT_SCREEN)
    };
    tab.view.stale = true;
    t.hover = None;
    if !alt
        && !tab.session.exited()
        && let Some(b) =
            input::encode_key(KeyPress::new(Key::Char('l'), Mods { ctrl: true, ..Mods::default() }), tab.typing())
    {
        tab.session.write(b);
    }
}

fn hide(st: &mut State, _: &Target, _: usize) {
    st.terminals.visible = false;
    st.terminals.set_focus(false);
}

impl State {
    /// A middle click on a terminal: the primary selection goes to the program (the clipboard on systems without
    /// one, macOS and Windows).
    pub(super) fn terminal_paste_primary(&mut self) {
        let text = if cfg!(any(target_os = "macos", target_os = "windows")) {
            self.ed.clipboard.get()
        } else {
            self.ed.features.on(Feature::Clipboard).then(|| self.ed.clipboard.get_primary()).flatten()
        };
        if let Some(text) = text {
            self.terminals.paste(&text);
        }
    }
}
