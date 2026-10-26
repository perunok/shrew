//! The status bar's mouse. Its items light up under the pointer and do something when clicked: the vim mode badge
//! opens a small menu (Vim Mode, Features, Settings), the cursor position opens Go to Line (`:`), the language opens
//! *Set Language…*, the branch the branch picker, the update pill updates, the gear opens Settings. A right click
//! anywhere on the bar opens a menu with the same things. Drawing stays in [`crate::draw`]; it records where each
//! item went in a [`Hits`] (a fixed array: nothing is allocated per frame) and asks [`hovered`] how to draw it.

use crate::colors::{Colors, mix};
use crate::context_menu::{Action, ContextMenu, MenuItem};
use crate::draw::Ui;
use crate::mouse::Target;
use crate::state::State;
use shrew_core::Command;
use shrew_render::{Color, Rect};

/// The clickable items, left to right as drawn on the right side (the mode badge is on the left).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Mode,
    Update,
    Branch,
    Language,
    Position,
    Gear,
}

const ITEMS: usize = 6;

/// Where each item was drawn in the last frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Hits {
    rects: [Option<Rect>; ITEMS],
}

impl Hits {
    pub fn set(&mut self, item: Item, r: Rect) {
        self.rects[item as usize] = Some(r);
    }

    pub fn get(&self, item: Item) -> Option<Rect> {
        self.rects[item as usize]
    }

    /// The item at (`x`, `y`).
    pub fn at(&self, x: f32, y: f32) -> Option<Item> {
        use Item::*;
        [Mode, Update, Branch, Language, Position, Gear]
            .into_iter()
            .find(|&i| self.get(i).is_some_and(|r| r.contains(x, y)))
    }
}

/// The fill of a hovered row (tree rows, Source Control rows): a faint tint of the panel's text, which reads in
/// light and dark themes alike and never looks like the selection.
pub fn hover_fill(c: &Colors) -> Color {
    mix(c.panel_foreground, c.panel_background, 0.08)
}

/// The fill of a hovered status bar item: the panel's selection (nothing in the bar is ever selected).
pub fn item_fill(c: &Colors) -> Color {
    c.panel_selection
}

/// A filled item (the mode badge, the update pill) under the pointer: a little lighter or darker.
pub fn lit(fill: Color, ink: Color) -> Color {
    mix(ink, fill, 0.14)
}

/// True when `item` is under the pointer.
pub fn hovered(ui: &Ui, item: Item) -> bool {
    ui.status_hover == Some(item)
}

/// The rect a text item gets: its text plus half a column each side, the bar's height less its padding.
pub fn item_rect(x: f32, cols: usize, lay: &crate::layout::Layout) -> Rect {
    let bar = lay.status;
    Rect::new(
        (x - lay.cw / 2.0).round(),
        bar.y + lay.pad,
        ((cols as f32 + 1.0) * lay.cw).round(),
        bar.h - 2.0 * lay.pad,
    )
}

/// Nothing under a modal panel or an open menu lights up.
fn blocked(st: &State) -> bool {
    st.ed.picker.is_some() || st.ed.about.is_some() || st.settings.is_open() || st.menu_open()
}

/// The pointer moved: the item under it lights up. (redraw, over a clickable item)
pub fn hover(st: &State, ui: &mut Ui, x: f32, y: f32) -> (bool, bool) {
    let over = if blocked(st) { None } else { ui.hits.status.at(x, y) };
    let changed = over != ui.status_hover;
    ui.status_hover = over;
    (changed, over.is_some())
}

/// The pointer left the window. True when something was lit.
pub fn left(ui: &mut Ui) -> bool {
    ui.status_hover.take().is_some()
}

/// A left click on an item that is not drawn by someone else (the gear, the update pill and the branch take their
/// own clicks first). True when taken.
pub fn press(st: &mut State, ui: &mut Ui, x: f32, y: f32) -> bool {
    if blocked(st) {
        return false;
    }
    let item = ui.hits.status.at(x, y);
    // what opens covers the bar: the item stops being lit (except the badge, which shows where its menu came from)
    if item.is_some_and(|i| i != Item::Mode) {
        ui.status_hover = None;
    }
    match item {
        Some(Item::Mode) => {
            let r = ui.hits.status.get(Item::Mode).unwrap_or_default();
            // above the badge, its left edge with the badge's
            crate::context_menu::open(st, mode_menu(st), (r.x, r.y));
        }
        Some(Item::Position) => st.ed.run(Command::GotoLine),
        Some(Item::Language) => st.ed.run(Command::SetLanguage),
        _ => return false,
    }
    st.after_input();
    true
}

/// The vim mode badge's menu.
fn mode_menu(st: &State) -> ContextMenu {
    let ed = &st.ed;
    let items: Vec<MenuItem> = vec![
        Action::command(ed, Command::ToggleVim).label("Vim Mode").into(),
        MenuItem::Separator,
        Action::command(ed, Command::ShowFeatures).label("Features…").into(),
        Action::command(ed, Command::Settings).label("Settings…").into(),
    ];
    ContextMenu::new(Target::StatusBar, items)
}

/// A right click on the bar: what its items do, in one place.
pub fn menu(st: &State) -> Option<ContextMenu> {
    let ed = &st.ed;
    let mut items: Vec<MenuItem> =
        vec![Action::command(ed, Command::GotoLine).into(), Action::command(ed, Command::SetLanguage).into()];
    // ----- git -----
    #[cfg(feature = "git")]
    if ed.features.on(shrew_core::Feature::GitPanel) {
        items.push(Action::command(ed, Command::GitBranches).label("Switch Branch…").into());
    }
    items.push(MenuItem::Separator);
    items.push(Action::command(ed, Command::ToggleVim).label("Vim Mode").into());
    items.push(Action::command(ed, Command::ShowFeatures).label("Features…").into());
    items.push(Action::command(ed, Command::Settings).label("Settings…").into());
    #[cfg(feature = "update")]
    items.push(Action::command(ed, Command::CheckForUpdates).into());
    Some(ContextMenu::new(Target::StatusBar, items))
}

#[cfg(test)]
mod tests;
