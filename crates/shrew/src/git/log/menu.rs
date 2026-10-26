//! A log tab's right-click menu. On a commit row (selected first): show or hide the commit, its diff, copy its id,
//! a branch there; then the tab's search, author and path filters and reload. Each entry presses the tab's own key,
//! which is its hint and its mnemonic, so the menu and the keys cannot drift apart.

use super::{active_id, key};
use crate::context_menu::{Action, ContextMenu, MenuItem};
use crate::mouse::{Panel, Target};
use crate::state::State;
use shrew_core::{Key, KeyPress};

/// The menu after a right click on row `row` of the active log tab (none: elsewhere on the tab).
pub fn menu(st: &mut State, row: Option<usize>) -> Option<ContextMenu> {
    let id = active_id(st)?;
    let v = st.git.views.get_mut(id)?;
    if let Some(i) = row.filter(|&i| i < v.shown.len()) {
        v.selected = i;
        v.in_details = false;
    }
    let commit = v.current().map(|r| r.commit.id);
    let shown = commit.is_some_and(|c| v.details.as_ref().is_some_and(|d| d.id == c));
    let on_row = row.is_some() && commit.is_some();
    let press = |label: &str, k: char| {
        let hint = if k == '\n' { "Enter".to_string() } else { k.to_string() };
        let a = Action::app_with(label, press_key, k as usize).hint(Some(hint));
        if k.is_ascii_alphabetic() { a.mnemonic(k) } else { a }
    };
    let mut items: Vec<MenuItem> = Vec::new();
    if on_row {
        items.extend([
            press(if shown { "Hide Commit" } else { "Show Commit" }, '\n').into(),
            press("Open Diff", 'd').into(),
            press("Copy Commit ID", 'y').into(),
            press("New Branch Here…", 'b').into(),
            MenuItem::Separator,
        ]);
    }
    items.extend([
        press("Search…", '/').into(),
        press("Filter by Author…", 'a').into(),
        press("History of a Path…", 'p').into(),
        MenuItem::Separator,
        press("Reload", 'r').into(),
    ]);
    Some(ContextMenu::new(Target::Panel(Panel::Log(row)), items))
}

/// Presses the log tab's key `k` (a char code; `'\n'` is Enter).
fn press_key(st: &mut State, _: &Target, k: usize) {
    let k = match char::from_u32(k as u32) {
        Some('\n') => Key::Enter,
        Some(c) => Key::Char(c),
        None => return,
    };
    key(st, KeyPress::plain(k));
}
