//! The application menu: File, Edit, Selection, View, Go, Git, Terminal, Help, built from the named commands when it
//! opens (a command this build lacks is left out, an empty menu too), each entry with the chord that runs it right
//! now. Opened by the title bar's menu button, F10 or Alt+F/E/S/V/G/I/T/H (each menu's underlined letter); driven
//! by arrows, Enter, Esc and typing the start of an entry, or by the mouse. The entries are [`MenuItem`]s drawn as a context menu's list
//! ([`crate::context_menu`]), so every menu looks the same.

use crate::context_menu::{MenuItem, Run, items_from_spec};
use shrew_core::{Command, Editor, Key, KeyPress};
use shrew_render::Rect;
use std::time::{Duration, Instant};

/// Typed letters within this long of each other make one prefix ("sa" for Save All).
const TYPE_AHEAD: Duration = Duration::from_millis(900);

/// (title, mnemonic, entries): a command name, `name|Label` to name it differently here, or `-` for a separator.
static MENUS: &[(&str, char, &[&str])] = &[
    (
        "File",
        'f',
        &[
            "new|New File",
            "files|Open File…",
            "recent|Open Recent…",
            "recent_projects|Open Recent Project…",
            "reopen_closed|Reopen Closed File",
            "-",
            "save|Save",
            "save_all|Save All",
            "reload|Reload From Disk",
            "-",
            "settings|Settings…",
            "config|Open Config File",
            "-",
            "close|Close Buffer",
            "quit|Quit",
        ],
    ),
    (
        "Edit",
        'e',
        &[
            "undo",
            "redo",
            "-",
            "cut",
            "copy",
            "paste",
            "-",
            "find",
            "find_next",
            "find_prev",
            "find_in_project|Find in Project…",
            "nohlsearch",
            "-",
            "toggle_comment",
            "indent",
            "outdent",
            "-",
            "duplicate_line",
            "delete_line",
            "move_line_up",
            "move_line_down",
            "join_lines",
            "insert_line_below",
            "insert_line_above",
        ],
    ),
    (
        "Selection",
        's',
        &[
            "select_all|Select All",
            "select_line|Expand to Line",
            "select_brackets|Expand to Brackets",
            "split_lines|Split Into Lines",
            "-",
            "add_cursor_above",
            "add_cursor_below",
            "add_next_occurrence",
            "skip_occurrence",
            "select_all_occurrences",
            "single_cursor",
        ],
    ),
    (
        "View",
        'v',
        &[
            "palette|Command Palette…",
            "-",
            "toggle_sidebar|Sidebar",
            "focus_sidebar|Focus Sidebar",
            "toggle_minimap|Minimap",
            "toggle_terminal|Terminal",
            "-",
            "toggle_line_numbers|Line Numbers",
            "toggle_relative_numbers|Relative Line Numbers",
            "toggle_whitespace|Whitespace",
            "toggle_syntax|Syntax Highlighting",
            "toggle_vim|Vim Mode",
            "-",
            "font_bigger|Zoom In",
            "font_smaller|Zoom Out",
            "font_reset|Reset Zoom",
            "-",
            "select_theme|Theme…",
            "select_accent|Accent…",
            "select_font|Font…",
            "toggle_ligatures|Ligatures",
            "-",
            "features|Features…",
            "customize_titlebar|Customize Title Bar…",
            "system_titlebar|Use System Title Bar",
        ],
    ),
    (
        "Go",
        'g',
        &[
            "files|Go to File…",
            "buffers|Switch Buffer…",
            "next_buffer|Next Buffer",
            "prev_buffer|Previous Buffer",
            "-",
            "goto_line|Go to Line…",
            "goto_bracket|Go to Matching Bracket",
            "symbols|Go to Symbol…",
            "project_symbols|Go to Symbol in Project…",
            "goto_file|Go to File Under Cursor",
            "-",
            "jump_back|Back",
            "jump_forward|Forward",
        ],
    ),
    // ----- git -----
    (
        "Git",
        'i',
        &[
            "git_panel|Source Control",
            "git_changed_files|Changed Files…",
            "-",
            "git_commit|Commit",
            "git_amend|Amend Last Commit",
            "git_stage_all|Stage All Changes",
            "git_unstage_all|Unstage All",
            "-",
            "git_branches|Checkout Branch…",
            "git_new_branch|New Branch…",
            "git_stash|Stash Changes…",
            "git_stashes|Stashes…",
            "-",
            "git_fetch|Fetch",
            "git_pull|Pull",
            "git_push|Push",
            "-",
            "git_diff_file|Diff File",
            "git_diff_all|Diff All Changes",
            "git_diff_staged|Diff Staged Changes",
            "git_diff_commit|Diff Commit…",
            "git_diff_compare|Compare Branches or Commits…",
            "-",
            "git_log|Log",
            "git_file_history|File History",
            "git_blame|Blame",
            "-",
            "git_next_hunk|Next Change",
            "git_preview_hunk|Preview Change",
            "git_stage_hunk|Stage Change",
            "git_revert_hunk|Revert Change",
            "-",
            "git_next_conflict|Next Conflict",
            "git_mark_resolved|Mark Resolved",
            "git_abort|Abort Merge or Rebase",
            "-",
            "git_toggle_gutter|Change Markers",
            "git_toggle_blame_inline|Inline Blame",
        ],
    ),
    (
        "Terminal",
        't',
        &[
            "new_terminal|New Terminal",
            "toggle_terminal|Toggle Terminal",
            "focus_terminal|Focus Terminal",
            "-",
            "next_terminal|Next Terminal",
            "prev_terminal|Previous Terminal",
            "maximize_terminal|Maximize Panel",
            "terminal_normal_mode|Normal Mode",
            "-",
            "close_terminal|Close Terminal",
        ],
    ),
    (
        "Help",
        'h',
        &[
            "palette|Command Palette…",
            "features|Features…",
            "-",
            "mcp|AI Assistants (MCP)…",
            "check_updates|Check for Updates…",
            "update|Install Update and Restart",
            "-",
            "about|About shrew",
        ],
    ),
];

/// Menus Alt+<letter> opens: every menu's underlined letter (Git is Alt+I: Alt+G is Go).
pub const ALT_MNEMONICS: &[char] = &['f', 'e', 's', 'v', 'g', 'i', 't', 'h'];

#[derive(Clone, Debug)]
pub struct Menu {
    pub title: &'static str,
    pub mnemonic: char,
    pub entries: Vec<MenuItem>,
}

/// Clickable rows of the open menu (filled when it is drawn).
#[derive(Clone, Debug, Default)]
pub struct MenuHits {
    pub roots: Vec<(Rect, usize)>,
    pub items: Vec<(Rect, usize)>,
    pub panels: Vec<Rect>,
}

impl MenuHits {
    pub fn inside(&self, x: f32, y: f32) -> bool {
        self.panels.iter().any(|p| p.contains(x, y))
    }
}

/// The open menu bar: its list of menus with one of them open.
#[derive(Debug)]
pub struct MenuState {
    pub menus: Vec<Menu>,
    /// The highlighted (open) menu.
    pub open: usize,
    /// The highlighted entry in it; `None` while the focus is on the list of menus.
    pub item: Option<usize>,
    /// Left edge of the menu bar's dropdown (under the menu button).
    pub anchor_x: f32,
    pub hits: MenuHits,
    /// The first entry shown, when the open menu is taller than the window.
    pub top: usize,
    /// The largest `top` (the last entries just fit), set when it is drawn.
    pub max_top: usize,
    /// The wheel scrolled it: the highlighted entry may go out of view (keys bring it back).
    pub wheeled: bool,
    typed: String,
    typed_at: Option<Instant>,
}

/// What a key or click on an open menu did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Close,
    Run(Command),
}

impl MenuState {
    /// The menu bar, `menu` open; `focus_items` moves the focus into it (Alt+F) instead of onto the list of menus.
    pub fn bar(ed: &Editor, menu: usize, focus_items: bool, anchor_x: f32) -> MenuState {
        let menus = build(ed);
        let open = menu.min(menus.len().saturating_sub(1));
        let mut m = MenuState {
            menus,
            open,
            item: None,
            anchor_x,
            hits: MenuHits::default(),
            top: 0,
            max_top: 0,
            wheeled: false,
            typed: String::new(),
            typed_at: None,
        };
        if focus_items {
            m.item = m.first_item(open);
        }
        m
    }

    /// The menu bar opened by mnemonic (`f` for File).
    pub fn index_of(&self, mnemonic: char) -> Option<usize> {
        self.menus.iter().position(|m| m.mnemonic == mnemonic)
    }

    pub fn entries(&self) -> &[MenuItem] {
        self.menus.get(self.open).map_or(&[], |m| &m.entries)
    }

    fn selectable(&self, menu: usize) -> impl Iterator<Item = usize> + '_ {
        let entries = self.menus.get(menu).map_or(&[][..], |m| &m.entries);
        entries.iter().enumerate().filter(|(_, e)| e.enabled()).map(|(i, _)| i)
    }

    fn first_item(&self, menu: usize) -> Option<usize> {
        self.selectable(menu).next()
    }

    /// The next selectable entry after (or before) the highlighted one, wrapping around.
    fn step_item(&self, forward: bool) -> Option<usize> {
        let all: Vec<usize> = self.selectable(self.open).collect();
        if all.is_empty() {
            return None;
        }
        let at = self.item.and_then(|i| all.iter().position(|&x| x == i));
        let n = all.len();
        Some(match (at, forward) {
            (None, true) => all[0],
            (None, false) => all[n - 1],
            (Some(p), true) => all[(p + 1) % n],
            (Some(p), false) => all[(p + n - 1) % n],
        })
    }

    /// Opens `menu`; `focus_items` puts the focus on its first entry.
    pub fn switch_menu(&mut self, menu: usize, focus_items: bool) {
        self.open = menu;
        self.top = 0;
        self.wheeled = false;
        self.item = if focus_items { self.first_item(menu) } else { None };
    }

    pub fn key(&mut self, k: KeyPress, now: Instant) -> Outcome {
        self.wheeled = false;
        let n = self.menus.len().max(1);
        let m = k.mods;
        if !matches!(k.key, Key::Char(_)) || m.ctrl || m.alt {
            self.typed.clear();
        }
        match k.key {
            Key::Esc => return Outcome::Close,
            Key::F(10) => return Outcome::Close,
            Key::Char(c) if m.alt && !m.ctrl => {
                if let Some(i) = self.index_of(c.to_ascii_lowercase()) {
                    self.switch_menu(i, true);
                }
            }
            Key::Up | Key::Down => {
                if self.item.is_none() {
                    // on the list of menus: up and down pick a menu
                    self.open = if k.key == Key::Down { (self.open + 1) % n } else { (self.open + n - 1) % n };
                } else {
                    self.item = self.step_item(k.key == Key::Down);
                }
            }
            Key::Home | Key::PageUp if self.item.is_some() => self.item = self.first_item(self.open),
            Key::End | Key::PageDown if self.item.is_some() => self.item = self.selectable(self.open).last(),
            Key::Right => {
                if self.item.is_none() {
                    self.item = self.first_item(self.open);
                } else {
                    self.switch_menu((self.open + 1) % n, true);
                }
            }
            Key::Left => {
                if self.item.is_some() {
                    self.item = None;
                } else {
                    self.open = (self.open + n - 1) % n;
                }
            }
            Key::Enter | Key::Char(' ') => {
                if let Some(cmd) = self.item.and_then(|i| self.command_at(i)) {
                    return Outcome::Run(cmd);
                }
                if self.item.is_none() {
                    self.item = self.first_item(self.open);
                }
            }
            Key::Char(c) if !m.ctrl && !c.is_control() => return self.type_ahead(c, now),
            _ => {}
        }
        Outcome::Stay
    }

    /// Typing selects the next entry starting with what was typed; on the list of menus, a menu's letter opens it.
    fn type_ahead(&mut self, c: char, now: Instant) -> Outcome {
        if self.item.is_none() {
            if let Some(i) = self.index_of(c.to_ascii_lowercase()) {
                self.switch_menu(i, true);
            }
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
            self.item = found;
        }
        Outcome::Stay
    }

    /// The first entry from the highlighted one on (`here`: including it) whose label starts with `prefix`.
    fn find_prefix(&self, prefix: &str, here: bool) -> Option<usize> {
        let all: Vec<usize> = self.selectable(self.open).collect();
        let start = self.item.and_then(|i| all.iter().position(|&x| x == i)).map_or(0, |p| p + usize::from(!here));
        (0..all.len())
            .map(|k| all[(start + k) % all.len()])
            .find(|&i| self.entries()[i].label().to_lowercase().starts_with(prefix))
    }

    pub fn command_at(&self, i: usize) -> Option<Command> {
        match self.entries().get(i)? {
            MenuItem::Action(a) => match a.run {
                Run::Command(cmd) => Some(cmd),
                _ => None,
            },
            _ => None,
        }
    }

    /// The pointer moved over the open menu: hovering a menu opens it, hovering an entry highlights it. True when
    /// that changed something.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let before = (self.open, self.item);
        if let Some(&(_, i)) = self.hits.roots.iter().find(|(r, _)| r.contains(x, y)) {
            if i != self.open {
                self.switch_menu(i, false);
            } else {
                self.item = None;
            }
        } else if let Some(&(_, i)) = self.hits.items.iter().find(|(r, _)| r.contains(x, y)) {
            self.item = Some(i);
        }
        before != (self.open, self.item)
    }

    /// The wheel over the open menu, `rows` entries down (up when negative). True when it scrolled.
    pub fn wheel(&mut self, rows: isize) -> bool {
        let top = self.top.saturating_add_signed(rows).min(self.max_top);
        self.wheeled = true;
        std::mem::replace(&mut self.top, top) != top
    }

    /// A click: an entry runs, a menu opens, anywhere else closes the menu.
    pub fn click(&mut self, x: f32, y: f32) -> Outcome {
        if let Some(&(_, i)) = self.hits.items.iter().find(|(r, _)| r.contains(x, y)) {
            return self.command_at(i).map_or(Outcome::Stay, Outcome::Run);
        }
        if let Some(&(_, i)) = self.hits.roots.iter().find(|(r, _)| r.contains(x, y)) {
            self.switch_menu(i, false);
            return Outcome::Stay;
        }
        if self.hits.inside(x, y) { Outcome::Stay } else { Outcome::Close }
    }
}

/// Every menu with what this build and mode have.
pub fn build(ed: &Editor) -> Vec<Menu> {
    MENUS
        .iter()
        .map(|&(title, mnemonic, names)| Menu { title, mnemonic, entries: items_from_spec(ed, names) })
        .filter(|m| !m.entries.is_empty())
        // ----- git: no Git menu in a build without git -----
        .filter(|m| cfg!(feature = "git") || m.title != "Git")
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_menu::chord_label;

    fn ed() -> Editor {
        Editor::new()
    }

    fn titles(m: &MenuState) -> Vec<&str> {
        m.menus.iter().map(|m| m.title).collect()
    }

    fn label(m: &MenuState) -> String {
        m.item.map_or(String::new(), |i| m.entries()[i].label().to_string())
    }

    fn key(m: &mut MenuState, s: &str) -> Outcome {
        let mut out = Outcome::Stay;
        for k in KeyPress::parse_seq(s) {
            out = m.key(k, Instant::now());
        }
        out
    }

    #[test]
    fn menus_hold_only_commands_that_exist() {
        let m = MenuState::bar(&ed(), 0, false, 0.0);
        // nothing in this build opens a terminal yet: no Terminal menu at all
        let has = |name: &str| Command::from_name(name).is_some();
        assert_eq!(titles(&m).contains(&"Terminal"), has("toggle_terminal") || has("new_terminal"));
        for t in ["File", "Edit", "Selection", "View", "Go", "Help"] {
            assert!(titles(&m).contains(&t), "{t}");
        }
        for menu in &m.menus {
            let e = &menu.entries;
            assert!(!e.first().unwrap().is_separator(), "{}", menu.title);
            assert!(!e.last().unwrap().is_separator(), "{}", menu.title);
            assert!(e.windows(2).all(|w| !(w[0].is_separator() && w[1].is_separator())));
        }
        let file = &m.menus[0].entries;
        let save = file.iter().find_map(|e| match e {
            MenuItem::Action(a) if matches!(a.run, Run::Command(Command::Save)) => {
                Some((a.label.clone(), a.hint.clone()))
            }
            _ => None,
        });
        assert_eq!(save, Some(("Save".to_string(), Some(chord_label(KeyPress::parse("<C-s>").unwrap())))));
    }

    #[test]
    fn keyboard_navigation() {
        let mut m = MenuState::bar(&ed(), 0, false, 0.0);
        assert_eq!((m.open, m.item), (0, None), "F10: focus on the list of menus");
        key(&mut m, "<Down><Down>");
        assert_eq!(m.menus[m.open].title, "Selection");
        key(&mut m, "<Right>");
        assert_eq!(label(&m), "Select All");
        key(&mut m, "<Up>");
        assert_eq!(label(&m), "Single Cursor", "wraps to the last entry");
        key(&mut m, "<Down>");
        assert_eq!(label(&m), "Select All");
        // separators are skipped
        key(&mut m, "<Down><Down><Down><Down>");
        assert_eq!(label(&m), "Add Above");
        key(&mut m, "<Right>");
        assert_eq!((m.menus[m.open].title, label(&m).as_str()), ("View", "Command Palette…"));
        key(&mut m, "<Left>");
        assert_eq!((m.menus[m.open].title, m.item), ("View", None), "back to the list of menus");
        key(&mut m, "<Left>");
        assert_eq!(m.menus[m.open].title, "Selection");
        key(&mut m, "<M-h>");
        assert_eq!(m.menus[m.open].title, "Help");
        assert!(m.item.is_some());
        assert_eq!(key(&mut m, "<End><CR>"), Outcome::Run(Command::About));
        assert_eq!(key(&mut m, "<Esc>"), Outcome::Close);
    }

    #[test]
    fn typing_jumps_to_entries() {
        let mut m = MenuState::bar(&ed(), 0, false, 0.0);
        // on the list of menus a letter opens that menu
        key(&mut m, "e");
        assert_eq!((m.menus[m.open].title, label(&m).as_str()), ("Edit", "Undo"));
        key(&mut m, "c");
        assert_eq!(label(&m), "Cut");
        key(&mut m, "c");
        assert_eq!(label(&m), "Copy", "the same letter again: the next match");
        key(&mut m, "c");
        assert_eq!(label(&m), "Clear Search Highlight");
        let mut m = MenuState::bar(&ed(), 0, true, 0.0);
        key(&mut m, "sav");
        assert_eq!(label(&m), "Save", "letters typed together make a prefix");
        key(&mut m, "<Down>r");
        assert_eq!(label(&m), "Reload From Disk", "a key in between starts a new prefix");
        let mut m = MenuState::bar(&ed(), 0, true, 0.0);
        key(&mut m, "ss");
        assert_eq!(label(&m), "Save All");
        assert_eq!(key(&mut m, "<CR>"), Outcome::Run(Command::SaveAll));
    }

    #[test]
    fn mouse_hover_and_click() {
        let mut m = MenuState::bar(&ed(), 0, false, 0.0);
        m.hits.roots = vec![(Rect::new(0.0, 0.0, 100.0, 20.0), 0), (Rect::new(0.0, 20.0, 100.0, 20.0), 1)];
        m.hits.panels = vec![Rect::new(0.0, 0.0, 100.0, 40.0), Rect::new(100.0, 0.0, 200.0, 400.0)];
        assert!(m.hover(10.0, 25.0));
        assert_eq!((m.open, m.item), (1, None), "hovering a menu opens it");
        assert!(!m.hover(10.0, 26.0), "no change, no redraw");
        m.hits.items = vec![(Rect::new(100.0, 0.0, 200.0, 20.0), 0)];
        assert!(m.hover(150.0, 10.0));
        assert_eq!(m.item, Some(0));
        assert_eq!(m.click(150.0, 10.0), Outcome::Run(Command::Undo));
        assert_eq!(m.click(150.0, 300.0), Outcome::Stay, "inside a panel, between rows");
        assert_eq!(m.click(500.0, 300.0), Outcome::Close);
    }
}
