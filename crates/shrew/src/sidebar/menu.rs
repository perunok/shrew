//! The file tree's right-click menu. On a row (selected first, and the sidebar gets the keys, so the hints are the
//! tree's own keys): new file and folder, the file's git actions when it has changes, rename, duplicate, delete,
//! copying its path, showing it in the file manager or a terminal, finding in the folder, collapsing and reading the
//! tree again. On the empty part below the rows: new file and folder at the root, and the tree-wide entries.

use crate::context_menu::{Action, ContextMenu, MenuItem};
use crate::mouse::Target;
use crate::state::State;
use shrew_core::Command;
use shrew_core::features::Feature;
use shrew_core::tree::Ask;
use std::path::PathBuf;

/// What copy-path copies.
const ABSOLUTE: usize = 0;
const RELATIVE: usize = 1;
const NAME: usize = 2;

/// The menu for row `row` of the tree, or its empty part (`None`). The row is selected and the sidebar focused.
pub fn menu(st: &mut State, row: Option<usize>) -> Option<ContextMenu> {
    let t = st.ed.tree.as_mut()?;
    let row = row.filter(|&i| i < t.rows.len());
    if let Some(i) = row {
        t.selected = i;
        t.follow = true;
    }
    st.ed.sidebar_focus = true;
    #[cfg(feature = "terminal")]
    st.terminals.set_focus(false);
    let ed = &st.ed;
    let t = ed.tree.as_ref()?;
    let r = row.and_then(|i| t.rows.get(i));
    let dir = r.is_none_or(|r| r.dir);
    let key = |s: &str| Some(s.to_string());
    let mut items: Vec<MenuItem> = Vec::new();
    // ----- git: a changed file's stage, unstage, discard and diff come first -----
    #[cfg(feature = "git")]
    if let Some(r) = r {
        let git = crate::git::tree_menu::items(st, &r.path);
        if !git.is_empty() {
            items.extend(git);
            items.push(MenuItem::Separator);
        }
    }
    // -----
    items.push(Action::app("New File…", new_file).hint(key("a")).into());
    items.push(Action::app("New Folder…", new_folder).hint(key("Shift+A")).into());
    items.push(MenuItem::Separator);
    if r.is_some() {
        items.push(Action::app("Rename…", rename).hint(key("F2")).into());
        items.push(Action::app("Duplicate…", duplicate).into());
        let delete = if cfg!(all(unix, not(target_os = "macos"))) { "Move to Trash" } else { "Delete…" };
        items.push(Action::app(delete, delete_row).hint(key("Delete")).into());
        items.push(MenuItem::Separator);
        items.push(Action::app_with("Copy Path", copy_path, ABSOLUTE).into());
        items.push(Action::app_with("Copy Relative Path", copy_path, RELATIVE).into());
        items.push(Action::app_with("Copy Name", copy_path, NAME).into());
        items.push(MenuItem::Separator);
    }
    items.push(Action::app("Reveal in File Manager", reveal).into());
    #[cfg(feature = "terminal")]
    if ed.features.on(Feature::Terminal) {
        items.push(Action::app("Open in Terminal", terminal).into());
    }
    if dir && ed.features.on(Feature::ProjectSearch) && cfg!(feature = "project") {
        // the whole project from the empty part: that is Find in Project, with its chord
        let find = match r {
            Some(_) => Action::app("Find in Folder…", find_in_folder),
            None => Action::command(ed, Command::FindInProject),
        };
        items.push(find.into());
    }
    items.push(MenuItem::Separator);
    items.push(Action::command(ed, Command::CollapseSidebar).label("Collapse All").into());
    let refresh = Action::command(ed, Command::RefreshSidebar).label("Refresh");
    let hint = refresh.hint.clone().or_else(|| key("F5"));
    items.push(refresh.hint(hint).into());
    Some(ContextMenu::new(Target::Sidebar(row), items))
}

/// The row the menu is for: the selected one (a right click selects its row); `None` for the empty part.
fn row_path(st: &State, t: &Target) -> Option<(PathBuf, bool)> {
    if !matches!(t, Target::Sidebar(Some(_))) {
        return None;
    }
    st.ed.tree.as_ref()?.current().map(|r| (r.path.clone(), r.dir))
}

/// The folder an action works in: the row's folder (a file's own folder), or the root.
fn folder(st: &State, t: &Target) -> Option<PathBuf> {
    match row_path(st, t) {
        Some((p, true)) => Some(p),
        Some((p, false)) => p.parent().map(PathBuf::from),
        None => st.ed.tree.as_ref().map(|t| t.root.clone()),
    }
}

fn ask(st: &mut State, t: &Target, a: Ask) {
    st.ed.sidebar_focus = true;
    // the empty part makes new entries at the root
    let root = match t {
        Target::Sidebar(None) => st.ed.tree.as_ref().map(|t| t.root.clone()),
        _ => None,
    };
    st.ed.tree_ask_in(a, root);
}

fn new_file(st: &mut State, t: &Target, _: usize) {
    ask(st, t, Ask::NewFile);
}

fn new_folder(st: &mut State, t: &Target, _: usize) {
    ask(st, t, Ask::NewFolder);
}

fn rename(st: &mut State, t: &Target, _: usize) {
    ask(st, t, Ask::Rename);
}

fn duplicate(st: &mut State, t: &Target, _: usize) {
    ask(st, t, Ask::Duplicate);
}

fn delete_row(st: &mut State, t: &Target, _: usize) {
    ask(st, t, Ask::Delete);
}

fn copy_path(st: &mut State, t: &Target, what: usize) {
    let Some((path, _)) = row_path(st, t) else { return };
    let root = st.ed.tree.as_ref().map(|t| t.root.clone()).unwrap_or_default();
    let s = match what {
        RELATIVE => path.strip_prefix(&root).unwrap_or(&path).display().to_string(),
        NAME => path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        _ => path.display().to_string(),
    };
    st.ed.clipboard.set(s.clone());
    st.ed.info(format!("copied {s}"));
}

fn reveal(st: &mut State, t: &Target, _: usize) {
    let path = row_path(st, t).map(|p| p.0).or_else(|| folder(st, t));
    let Some(path) = path else { return };
    if let Err(e) = crate::open::reveal(&path) {
        st.ed.error(format!("cannot show {}: {e}", path.display()));
    }
}

#[cfg(feature = "terminal")]
fn terminal(st: &mut State, t: &Target, _: usize) {
    let Some(dir) = folder(st, t) else { return };
    st.ed.sidebar_focus = false;
    st.open_terminal_in(None, dir);
}

fn find_in_folder(st: &mut State, t: &Target, _: usize) {
    let Some(dir) = folder(st, t) else { return };
    st.ed.sidebar_focus = false;
    st.ed.open_grep_in(&dir);
}
