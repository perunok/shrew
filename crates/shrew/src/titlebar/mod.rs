//! The custom title bar (feature `custom_titlebar`, on by default): shrew draws the window's top row itself, with
//! the buffer tabs in the middle and configurable items on either side (`[titlebar]`, see [`items`]): the menu
//! button, sidebar and terminal toggles, the project, go to file, settings, about, and the window buttons. Drag an
//! empty part to move the window, double-click it to maximize, right-click it for a small menu; the window resizes
//! from its edges. The menus ([`menu`]) open from the menu button, F10 or Alt+F/E/V/G/H.
//!
//! Turning the feature off (`:set nocustom_titlebar`, palette *View: Toggle System Title Bar*, or the bar's
//! right-click menu) brings the system title bar back at runtime and the tabs move back to their own row. On macOS
//! the traffic lights stay native, over a transparent title bar.
//!
//! Cost when unused: nothing is built until a menu opens; the bar redraws only when the hovered item changes.

mod draw;
pub mod items;
pub mod menu;
mod window;

pub use draw::{draw, draw_menus};
pub use window::{WindowAction, apply, attributes, hit, pointer_left, pointer_moved, press, release};

use crate::context_menu::ContextMenu;
use crate::layout::Layout;
use crate::mouse::Target;
use crate::state::State;
use items::{BarLayout, Item};
use menu::{MenuState, Outcome};
use shrew_core::features::Feature;
use shrew_core::{Command, Editor, Key, KeyPress, MapMode, Mode};
use shrew_render::Rect;
use std::time::Instant;

/// What the bar's right-click menu offers.
const CONTEXT_MENU: &[&str] = &["customize_titlebar|Customize Title Bar…", "-", "system_titlebar|Use System Title Bar"];

#[derive(Default)]
pub struct TitleBar {
    pub layout: BarLayout,
    /// The open menu bar (its right-click menu is a [`ContextMenu`], in `State::menu`).
    pub menu: Option<MenuState>,
    /// The item under the pointer, and the one being pressed (drawn highlighted).
    pub hover: Option<Item>,
    pub pressed: Option<Item>,
    /// Where each item was drawn in the last frame, and the bar itself.
    pub placed: Vec<(Item, Rect)>,
    pub bar: Rect,
    pub maximized: bool,
    /// Window work the app carries out (it owns the window), queued by keys and menus.
    pub actions: Vec<WindowAction>,
    /// Whether the window has the system's decorations, as last applied (`None` before the window exists).
    pub decorated: Option<bool>,
    /// A press on an empty part of the bar: the window moves once the pointer does.
    drag_from: Option<(f32, f32)>,
    last_click: Option<(Instant, (f32, f32))>,
}

impl TitleBar {
    /// The layout from `[titlebar]`; unknown item names are logged.
    pub fn configure(&mut self, cfg: &shrew_config::TitleBarConfig) {
        let (layout, unknown) = BarLayout::sanitize(cfg);
        for n in unknown {
            log::warn!("[titlebar]: unknown item {n:?}");
        }
        self.layout = layout;
    }
}

/// The custom title bar is on (its feature).
pub fn on(ed: &Editor) -> bool {
    ed.features.on(Feature::CustomTitlebar)
}

/// shrew draws minimize, maximize and close itself (everywhere but macOS, which keeps its traffic lights).
pub fn own_window_buttons() -> bool {
    !cfg!(target_os = "macos")
}

/// The bar's height: room for the tabs and comfortable buttons.
pub fn bar_height(lay: &Layout) -> f32 {
    (lay.ch + 3.0 * lay.pad).max((32.0 * lay.scale).round()).round()
}

/// The frame's layout with the title bar on top (the tab row always there, and taller); unchanged when it is off.
pub fn layout(lay: Layout, ed: &Editor) -> Layout {
    if on(ed) {
        let h = bar_height(&lay);
        lay.with_top_bar(h)
    } else {
        lay
    }
}

/// The project: the project root's name (see `Editor::project_root`), else the working directory's (none for the
/// home directory or `/`).
pub fn project_name(ed: &Editor) -> Option<String> {
    if let Some(root) = ed.project_root() {
        return root.file_name().map(|n| n.to_string_lossy().into_owned());
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    if home.is_some_and(|h| ed.cwd == std::path::Path::new(&h)) {
        return None;
    }
    ed.cwd.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// "<file> — <project> — shrew", for the OS (task bar, window switcher), whichever title bar shows.
pub fn window_title(ed: &Editor) -> String {
    let b = ed.buf();
    let mut t = format!("{}{}", b.doc.display_name(), if b.doc.dirty { " ●" } else { "" });
    if let Some(p) = project_name(ed) {
        t += " — ";
        t += &p;
    }
    t + " — shrew"
}

/// A key while a menu is open, or one that opens a menu (F10, Alt+F/E/V/G/H): true when the title bar took it.
pub fn key(st: &mut State, k: KeyPress) -> bool {
    if let Some(m) = &mut st.titlebar.menu {
        match m.key(k, Instant::now()) {
            Outcome::Stay => {}
            Outcome::Close => st.titlebar.menu = None,
            Outcome::Run(cmd) => run(st, cmd),
        }
        return true;
    }
    let ed = &st.ed;
    if !on(ed) || ed.picker.is_some() || ed.about.is_some() || ed.mapping_pending() || mapped(ed, k) {
        return false;
    }
    let m = k.mods;
    let which = match k.key {
        Key::F(10) if !m.ctrl && !m.alt && !m.shift => None,
        Key::Char(c) if m.alt && !m.ctrl && !m.logo && menu::ALT_MNEMONICS.contains(&c) => Some(c),
        _ => return false,
    };
    open_menu(st, which);
    true
}

/// A mapping of yours on `k` wins over the menu keys.
fn mapped(ed: &Editor, k: KeyPress) -> bool {
    let mode = if ed.mode == Mode::Insert || !ed.vim() {
        MapMode::Insert
    } else if ed.mode.is_visual() {
        MapMode::Visual
    } else {
        MapMode::Normal
    };
    ed.keymap.get(mode, &[k]).is_some()
}

/// Opens the menu bar under the menu button: `mnemonic`'s menu with the focus in it, or (None) the list of menus.
pub fn open_menu(st: &mut State, mnemonic: Option<char>) {
    let tb = &st.titlebar;
    let x = tb.placed.iter().find(|p| p.0 == Item::Menu).map_or(tb.bar.x, |p| p.1.x);
    let mut m = MenuState::bar(&st.ed, 0, false, x);
    if let Some(i) = mnemonic.and_then(|c| m.index_of(c)) {
        m.switch_menu(i, true);
    }
    st.menu = None;
    st.titlebar.menu = Some(m);
}

/// The menu a right click on an empty part of the bar opens (none with the system title bar).
pub fn context_menu(st: &State) -> Option<ContextMenu> {
    on(&st.ed).then(|| ContextMenu::from_spec(&st.ed, Target::TitleBar, CONTEXT_MENU))
}

/// Runs a menu entry (or a title bar button's command), the menu closing first.
fn run(st: &mut State, cmd: Command) {
    st.titlebar.menu = None;
    st.ed.message = None;
    st.ed.run(cmd);
    st.after_input();
}

/// [`shrew_core::Event::App`]: the commands only the app can carry out.
pub fn app_command(st: &mut State, cmd: Command) {
    match cmd {
        Command::ShowMenu => {
            if st.titlebar.menu.is_some() {
                st.titlebar.menu = None;
            } else if on(&st.ed) {
                open_menu(st, None);
            } else {
                st.ed.error("the menus are part of the custom title bar (:set custom_titlebar)");
            }
        }
        Command::MinimizeWindow => st.titlebar.actions.push(WindowAction::Minimize),
        Command::ToggleMaximize => st.titlebar.actions.push(WindowAction::ToggleMaximize),
        Command::CustomizeTitleBar => customize(st),
        _ => {}
    }
}

/// The Settings panel at Layout & window, when it is on; else config.toml at `[titlebar]`.
fn customize(st: &mut State) {
    if st.ed.features.on(Feature::Settings) {
        return st.open_settings("layout");
    }
    edit_config(st);
}

/// Opens config.toml with the cursor on `[titlebar]`.
fn edit_config(st: &mut State) {
    st.ed.run(Command::OpenConfig);
    st.process_events();
    let b = st.ed.buf();
    if st.config_path.is_none() || b.doc.path != st.config_path {
        return; // opening it failed, and said why
    }
    match b.rope().lines().position(|l| l.to_string().trim() == "[titlebar]") {
        Some(n) => {
            st.ed.goto_line(n + 1);
            st.ed.info("[titlebar]: the items on the left and right, in order; saving applies them");
        }
        None => st.ed.info("add a [titlebar] table with left, right and hidden lists (docs/CONFIG.md)"),
    }
}

/// What a click on an item does.
fn activate(st: &mut State, item: Item) {
    let cmd = match item {
        Item::Menu => {
            if st.titlebar.menu.take().is_none() {
                open_menu(st, None);
            }
            return;
        }
        Item::Tabs => return,
        Item::Close => return st.titlebar.actions.push(WindowAction::Close),
        // ----- git -----
        Item::Branch => {
            #[cfg(feature = "git")]
            crate::git::status_ui::branch_clicked(st);
            return;
        }
        Item::Project => Command::from_name("recent_projects").unwrap_or(Command::OpenFile),
        _ => match item.info().command.and_then(Command::from_name) {
            Some(c) => c,
            None => return,
        },
    };
    run(st, cmd);
}

/// Whether an item is drawn: its command exists in this build, a window button only where shrew draws them.
pub fn shown(item: Item) -> bool {
    if item.is_window_button() {
        return own_window_buttons();
    }
    if item == Item::Branch {
        return cfg!(feature = "git");
    }
    match (item, item.info().command) {
        (_, Some(name)) => Command::from_name(name).is_some(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn state() -> State {
        State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true)
    }

    fn keys(st: &mut State, s: &str) {
        for k in KeyPress::parse_seq(s) {
            st.key(k);
        }
    }

    #[test]
    fn menu_keys_open_navigate_and_run() {
        let mut st = state();
        keys(&mut st, "<F10>");
        assert!(st.titlebar.menu.is_some());
        keys(&mut st, "<Esc>");
        assert!(st.titlebar.menu.is_none());
        assert_eq!(st.ed.mode, Mode::Normal, "Esc closed the menu, nothing else");
        // Alt+V opens View, typing picks an entry, Enter runs it
        assert!(st.ed.features.on(Feature::LineNumbers));
        keys(&mut st, "<M-v>l<CR>");
        assert!(st.titlebar.menu.is_none());
        assert!(!st.ed.features.on(Feature::LineNumbers), "View > Line Numbers toggled them");
        // keys went to the menu, not the buffer
        assert_eq!(st.ed.buf().rope().len_chars(), 0);
        // the palette's "View: Open Menu" too
        keys(&mut st, ":menu<CR>");
        assert!(st.titlebar.menu.is_some());
    }

    #[test]
    fn your_mappings_and_a_system_title_bar_keep_the_keys() {
        let mut st = state();
        keys(&mut st, ":nnoremap <lt>M-f> ihi<lt>Esc><CR><M-f>");
        assert!(st.titlebar.menu.is_none());
        assert_eq!(st.ed.buf().rope().to_string(), "hi");
        keys(&mut st, ":set nocustom_titlebar<CR><F10>");
        assert!(st.titlebar.menu.is_none());
    }

    #[test]
    fn window_commands_are_queued_for_the_app() {
        let mut st = state();
        keys(&mut st, ":minimize<CR>:maximize<CR>");
        assert_eq!(st.titlebar.actions, vec![WindowAction::Minimize, WindowAction::ToggleMaximize]);
        // the right-click menu's second entry switches to the system title bar
        let m = context_menu(&st).unwrap();
        crate::context_menu::open(&mut st, m, (10.0, 10.0));
        keys(&mut st, "<Down><Down><CR>");
        assert!(!on(&st.ed));
    }

    #[test]
    fn customizing_opens_the_config_at_titlebar_and_saving_applies_it() {
        let dir = std::env::temp_dir().join(format!("shrew-titlebar-app-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut st = state();
        st.config_path = Some(path.clone());
        // with the Settings panel on, it opens there, at Layout & window
        keys(&mut st, ":customize_titlebar<CR>");
        assert!(st.settings.is_open());
        keys(&mut st, "<Esc>:set nosettings<CR>:customize_titlebar<CR>");
        let b = st.ed.buf();
        assert_eq!(b.doc.path.as_deref(), Some(path.as_path()));
        let line = shrew_core::text::line_of(b.rope(), b.sel.primary().head);
        assert_eq!(b.rope().line(line).to_string().trim(), "[titlebar]");
        // uncomment a layout below it and save: the bar follows
        keys(&mut st, "o<C-u>left = [\"tabs\", \"menu\"]<Esc>:w<CR>");
        // (the items it leaves out go where the default puts them)
        let left = vec![Item::Tabs, Item::Menu, Item::Sidebar, Item::Project, Item::Branch];
        assert_eq!(st.titlebar.layout.left, left, "{:?}", st.ed.message);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn title_names_file_project_and_app() {
        // a folder with no project markers is its own project
        let dir = std::env::temp_dir().join(format!("shrew-title-{}", std::process::id())).join("burrow");
        std::fs::create_dir_all(&dir).unwrap();
        let mut ed = Editor::new();
        ed.cwd = dir.clone();
        assert_eq!(window_title(&ed), format!("{} — burrow — shrew", ed.buf().doc.display_name()));
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
        ed.cwd = std::path::PathBuf::from("/");
        assert_eq!(window_title(&ed), format!("{} — shrew", ed.buf().doc.display_name()));
    }
}
