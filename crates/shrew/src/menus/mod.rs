//! The menus of the text and of the gutter, opened by a right click (or Shift+F10 at the cursor), and the primary
//! selection ([`primary`]): what is selected is offered to other programs, a middle click pastes theirs.
//!
//! A right click in the text first moves the cursor there, as in VS Code and Sublime, unless it lands inside a
//! selection or on one of the cursors (they are kept for the menu to act on); in vim's normal mode the cursor goes
//! on the char clicked. A right click on a line number acts on that line: the cursor goes there unless the line is
//! selected. The terminal panel's menu is in `terminal::menu`, a diff tab's in `git::diffview::menu`.
//!
//! Every entry that is a command shows the chord that runs it ([`Action::command`]). Entries that cannot do
//! anything here are greyed out; parts this build or this file lacks (git, the terminal, the sidebar) are left out.

pub mod primary;
#[cfg(test)]
mod tests;

use crate::context_menu::{Action, ContextMenu, MenuItem, open, shortcut};
use crate::draw::Ui;
use crate::layout::Layout;
use crate::mouse::Target;
use crate::state::State;
use shrew_core::features::Feature;
use shrew_core::{Command, Editor, Key, KeyPress, Mode, Mods, motion, text};

/// What the menus keep between frames.
#[derive(Default)]
pub struct Menus {
    /// The selection last offered as the primary selection (a fingerprint; 0: none).
    pub(crate) primary: u64,
    /// A menu opened by Shift+F10 goes under the cursor drawn in the next frame (the cursor may have moved since
    /// the last one).
    pub(crate) at_cursor: bool,
    /// Where the last frame drew the text's cursor.
    cursor: Option<shrew_render::Rect>,
}

// ----- the text -----

/// The text's menu after a right click at char `pos` (the cursor moves there first, see the module docs).
pub fn text(st: &mut State, pos: usize) -> Option<ContextMenu> {
    take_focus(st);
    if !inside_selection(&st.ed, pos) {
        st.ed.mouse_down(pos, 1, Mods::default());
        st.ed.mouse_up();
    }
    st.after_input();
    Some(text_menu(st, Target::Text(pos)))
}

/// Shift+F10 (`context_menu`): the text's menu (a diff tab's on one) at the cursor, its first entry highlighted.
/// It opens at once, so the keys typed next go to it; the next frame puts it under the cursor as drawn then.
pub fn request(st: &mut State) {
    if st.menu_open() || st.ed.picker.is_some() || st.ed.about.is_some() || st.settings.is_open() {
        return;
    }
    #[cfg(feature = "git")]
    if crate::git::diffview::is_active(st) {
        if let Some((m, at)) = crate::git::diffview::cursor_menu(st) {
            st.menus.at_cursor = open(st, m.select_first(), at);
        }
        return;
    }
    let at = st.menus.cursor.map_or((0.0, 0.0), |c| (c.x, c.bottom()));
    let head = st.ed.buf().sel.primary().head;
    let m = text_menu(st, Target::Text(head)).select_first();
    st.menus.at_cursor = open(st, m, at);
}

/// Each frame, before the menus are drawn: notes where the cursor is, and moves a menu just opened by
/// [`request`] under it.
pub fn frame(st: &mut State, ui: &Ui) {
    st.menus.cursor = ui.cursor_rect;
    if !std::mem::take(&mut st.menus.at_cursor) {
        return;
    }
    #[cfg(feature = "git")]
    let diff = crate::git::diffview::cursor_at(st);
    #[cfg(not(feature = "git"))]
    let diff = None;
    let Some(m) = &mut st.menu else { return };
    let at = match m.target {
        Target::Diff(_) => diff,
        _ => ui.cursor_rect.map(|c| (c.x, c.bottom())),
    };
    if let Some(at) = at {
        m.at = at;
    }
}

/// The text gets the keys back, as with a left click: from the command line, the sidebar, the terminal.
fn take_focus(st: &mut State) {
    if st.ed.prompt.is_some() {
        st.key(KeyPress::plain(Key::Esc));
    }
    st.ed.sidebar_focus = false;
    #[cfg(feature = "terminal")]
    if st.terminals.focused {
        st.terminals.set_focus(false);
    }
}

/// Whether char `pos` is in a selection or on a cursor (a right click there keeps them). Caret selections take
/// both their ends, as in VS Code; vim's inclusive ones the chars they cover.
pub fn inside_selection(ed: &Editor, pos: usize) -> bool {
    let ranges = ed.buf().sel.ranges();
    if ed.mode == Mode::Insert || !ed.vim() {
        return ranges.iter().any(|r| r.from() <= pos && pos <= r.to());
    }
    ranges.iter().any(|r| r.is_empty() && r.head == pos)
        || ed.selection_spans().iter().any(|&(a, z)| a <= pos && pos < z)
}

fn text_menu(st: &State, target: Target) -> ContextMenu {
    let ed = &st.ed;
    let cmd = |c| Action::command(ed, c);
    let f = |x| ed.features.on(x);
    let selected = !ed.selection_spans().is_empty();
    let word = word_or_selection(ed);
    let some = selected || word.is_some();
    // in vim's normal and visual modes, where Ctrl+X / Ctrl+V are vim's own, the vim keys that do the same
    let vim = |a: Action, normal: &str, visual: &str| match a.hint {
        None if ed.vim() && ed.mode != Mode::Insert => {
            let k = if ed.mode.is_visual() { visual } else { normal };
            a.hint(Some(k.to_string()))
        }
        _ => a,
    };
    let mut items: Vec<MenuItem> = vec![
        // with nothing selected they take the whole line, as Ctrl+X / Ctrl+C do
        vim(cmd(Command::Cut), "dd", "d").label(if selected { "Cut" } else { "Cut Line" }).mnemonic('t').into(),
        vim(cmd(Command::Copy), "yy", "y").label(if selected { "Copy" } else { "Copy Line" }).mnemonic('c').into(),
        vim(cmd(Command::Paste), "P", "p").mnemonic('p').into(),
        MenuItem::Separator,
        cmd(Command::SelectAll).mnemonic('a').into(),
        cmd(Command::SelectAllOccurrences).enabled(some).mnemonic('o').into(),
        cmd(Command::AddNextOccurrence).enabled(some).mnemonic('n').into(),
        MenuItem::Separator,
        cmd(Command::ToggleComment).mnemonic('m').into(),
        cmd(Command::Indent).mnemonic('i').into(),
        cmd(Command::Outdent).mnemonic('u').into(),
        MenuItem::Separator,
    ];
    if ed.features.available(Feature::ProjectSearch) {
        let label = match &word {
            Some(w) => format!("Find \u{201c}{}\u{201d} in Project", clip(w, 24)),
            None => "Find in Project…".to_string(),
        };
        let a = Action::app(label, find_in_project).hint(shortcut(ed, Command::FindInProject));
        items.push(a.enabled(f(Feature::ProjectSearch)).mnemonic('f').into());
    }
    if ed.features.available(Feature::Symbols) {
        items.push(cmd(Command::GotoSymbol).enabled(f(Feature::Symbols) && ed.symbols.is_some()).mnemonic('y').into());
    }
    #[cfg(feature = "git")]
    if let Some(git) = git_submenu(st) {
        items.push(MenuItem::Separator);
        items.push(git);
    }
    let path = ed.buf().doc.path.is_some();
    items.push(MenuItem::Separator);
    items.push(Action::app("Copy Path", copy_path).enabled(path).mnemonic('h').into());
    items.push(Action::app("Copy Relative Path", copy_relative_path).enabled(path).mnemonic('r').into());
    if ed.features.available(Feature::Sidebar) {
        let root = path && ed.project_root().is_some();
        items.push(Action::app("Reveal in Sidebar", reveal_in_sidebar).enabled(root).mnemonic('v').into());
    }
    #[cfg(feature = "terminal")]
    if ed.features.available(Feature::Terminal) {
        items
            .push(Action::app("Open in Terminal", open_in_terminal).enabled(f(Feature::Terminal)).mnemonic('e').into());
    }
    ContextMenu::new(target, items)
}

/// The selection when it is on one line, else the word at the cursor (or just before a caret).
pub fn word_or_selection(ed: &Editor) -> Option<String> {
    let b = ed.buf();
    let rope = b.rope();
    let len = rope.len_chars();
    let r = b.sel.primary();
    let visual = ed.mode.is_visual();
    if !r.is_empty() || visual {
        let z = if visual { text::grapheme_end(rope, r.to()) } else { r.to() };
        let s = rope.slice(r.from().min(len)..z.min(len)).to_string();
        return (!s.trim().is_empty() && !s.contains('\n')).then_some(s);
    }
    let word = |p: usize| text::char_at(rope, p).is_some_and(text::is_word_char);
    let caret = ed.mode == Mode::Insert || !ed.vim();
    let p = match r.head {
        h if word(h) => h,
        h if caret && h > 0 && word(h - 1) => h - 1,
        _ => return None,
    };
    let (a, z) = motion::word_object(rope, p, false, false);
    (z > a).then(|| rope.slice(a..z.min(len)).to_string())
}

/// At most `n` chars of `s`, with an ellipsis when cut.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n.saturating_sub(1)).chain(['…']).collect()
}

fn find_in_project(st: &mut State, _: &Target, _: usize) {
    let w = word_or_selection(&st.ed).unwrap_or_default();
    st.ed.open_grep(&w);
}

fn copy_path(st: &mut State, _: &Target, _: usize) {
    let Some(p) = st.ed.buf().doc.path.clone() else { return };
    copy_text(st, p.display().to_string(), "the path");
}

fn copy_relative_path(st: &mut State, _: &Target, _: usize) {
    let Some(p) = st.ed.buf().doc.path.clone() else { return };
    let base = st.ed.project_root().unwrap_or_else(|| st.ed.cwd.clone());
    copy_text(st, shrew_core::editor::relative(&p, &base), "the relative path");
}

fn copy_text(st: &mut State, text: String, what: &str) {
    st.ed.info(format!("copied {what}: {text}"));
    st.ed.clipboard.set(text);
}

fn reveal_in_sidebar(st: &mut State, _: &Target, _: usize) {
    let Some(p) = st.ed.buf().doc.path.clone() else { return };
    st.ed.focus_sidebar(true);
    if let Some(t) = &mut st.ed.tree {
        t.reveal(&p);
    }
}

/// A new terminal in the file's folder (the project's for a scratch buffer).
#[cfg(feature = "terminal")]
fn open_in_terminal(st: &mut State, _: &Target, _: usize) {
    let dir = st.ed.buf().doc.path.as_deref().and_then(std::path::Path::parent).map(std::path::Path::to_path_buf);
    let dir = dir.unwrap_or_else(|| st.ed.work_dir());
    st.open_terminal_in(None, dir);
}

// ----- the gutter -----

/// The gutter's menu after a right click beside line `line`: the cursor goes to it unless it is selected.
pub fn gutter(st: &mut State, line: usize) -> Option<ContextMenu> {
    take_focus(st);
    if !st.ed.git_target_lines().0.contains(&line) {
        let p = text::line_start(st.ed.buf().rope(), line);
        st.ed.mouse_down(p, 1, Mods::default());
        st.ed.mouse_up();
    }
    st.after_input();
    let ed = &st.ed;
    let cmd = |c| Action::command(ed, c);
    let mut items: Vec<MenuItem> = vec![
        Action::app("Select Line", select_line).hint(shortcut(ed, Command::SelectLine)).mnemonic('s').into(),
        Action::app("Copy Line", copy_line).mnemonic('c').into(),
        MenuItem::Separator,
    ];
    #[cfg(feature = "git")]
    if in_repo(st) {
        items.extend(hunk_items(st, line..line + 1));
        items.push(MenuItem::Separator);
    }
    items.push(cmd(Command::ToggleLineNumbers).label("Line Numbers").into());
    let numbers = ed.features.on(Feature::LineNumbers);
    items.push(cmd(Command::ToggleRelativeNumbers).label("Relative Line Numbers").enabled(numbers).into());
    #[cfg(feature = "git")]
    if in_repo(st) {
        items.push(cmd(Command::GitToggleGutter).label("Change Markers").into());
        items.extend(blame_items(st));
    }
    Some(ContextMenu::new(Target::Gutter(line), items))
}

fn select_line(st: &mut State, t: &Target, _: usize) {
    let Target::Gutter(line) = *t else { return };
    let p = text::line_start(st.ed.buf().rope(), line);
    // as a click on the line number does
    st.ed.mouse_down(p, 3, Mods::default());
    st.ed.mouse_up();
}

fn copy_line(st: &mut State, t: &Target, _: usize) {
    let Target::Gutter(line) = *t else { return };
    st.ed.copy_line(line);
    st.ed.info(format!("copied line {}", line + 1));
}

// ----- git -----

/// Whether the active file is in a git repository (as far as is known yet).
#[cfg(feature = "git")]
fn in_repo(st: &State) -> bool {
    let Some(p) = st.ed.buf().doc.path.as_deref() else { return false };
    matches!(st.git.file_place(p), Some(crate::git::Place::In { .. }))
}

/// The text's Git submenu: the change at the cursor (or the selected lines), the diff, history and blame.
#[cfg(feature = "git")]
fn git_submenu(st: &State) -> Option<MenuItem> {
    if !in_repo(st) {
        return None;
    }
    let ed = &st.ed;
    let cmd = |c| Action::command(ed, c);
    let mut items = hunk_items(st, ed.git_target_lines().0);
    let diff = if st.git.head_base { Command::GitDiffFileHead } else { Command::GitDiffFile };
    items.extend([
        MenuItem::Separator,
        cmd(diff).label("Open Diff").into(),
        cmd(Command::GitFileHistory).label("File History").into(),
        MenuItem::Separator,
    ]);
    items.extend(blame_items(st));
    use crate::context_menu::Submenu;
    Some(Submenu { mnemonic: Some('g'), ..Submenu::new("Git", items) }.into())
}

/// Peek, stage, (unstage,) revert: enabled when a change touches `lines`. They act on the cursor line or the
/// selected lines, which the right click has just set.
#[cfg(feature = "git")]
fn hunk_items(st: &State, lines: std::ops::Range<usize>) -> Vec<MenuItem> {
    let ed = &st.ed;
    let cmd = |c| Action::command(ed, c);
    let last = text::last_line(ed.buf().rope());
    let on = ed.hunks().iter().any(|h| {
        let r = h.lines(last);
        r.start < lines.end && lines.start < r.end
    });
    let what = if ed.git_target_lines().1 { "Selected Lines" } else { "Change" };
    // vim's normal-mode keys for them (`gh` then a letter), unless a chord of yours runs them
    let vim = ed.vim() && ed.mode != Mode::Insert;
    let item = |c, label: String, keys: &str| {
        let a = cmd(c).label(label).enabled(on);
        let keys = (a.hint.is_none() && vim).then(|| keys.to_string());
        MenuItem::from(if keys.is_some() { a.hint(keys) } else { a })
    };
    let mut items = vec![
        item(Command::GitPreviewHunk, "Peek Change".into(), "ghp"),
        item(Command::GitStageHunk, format!("Stage {what}"), "ghs"),
    ];
    // markers against HEAD show staged changes too
    if st.git.head_base {
        items.push(item(Command::GitUnstageHunk, format!("Unstage {what}"), "ghu"));
    }
    items.push(item(Command::GitRevertHunk, format!("Revert {what}"), "ghr"));
    items
}

#[cfg(feature = "git")]
fn blame_items(st: &State) -> Vec<MenuItem> {
    let ed = &st.ed;
    let blame = st.git.blame_view.get(ed.buf().id).is_some();
    vec![
        Action::command(ed, Command::GitBlame).label("Blame Column").checked(blame).into(),
        Action::command(ed, Command::GitToggleBlameInline).label("Inline Blame").into(),
    ]
}

// ----- the middle button -----

/// A middle press: in the text it pastes the primary selection at the pointer. True when it did.
pub fn middle_press(st: &mut State, ui: &Ui, lay: &Layout, x: f32, y: f32) -> bool {
    match crate::mouse::target_at(st, ui, lay, x, y) {
        Target::Text(pos) => primary::paste(st, pos),
        _ => false,
    }
}
