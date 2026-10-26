//! The git part of the sidebar tree's right-click menu: on a changed file, *Open Diff*, *Stage*, *Unstage*,
//! *Discard Changes…* (asks first) or *Mark Resolved*, from the file's state in its repository's status (feature
//! `git_status`, the same state that colours the row).

use super::ask::{Ask, Then};
use super::diffview::{DiffSpec, Rev};
use crate::context_menu::{Action, MenuItem};
use crate::mouse::Target;
use crate::state::State;
use shrew_git::{Op, StatusEntry};
use std::path::{Path, PathBuf};

/// The file's repository, its path there and its state, when it has changes.
fn state_of(st: &State, path: &Path) -> Option<(PathBuf, String, StatusEntry)> {
    let root = &st.ed.tree.as_ref()?.root;
    let deco = super::status_ui::tree_deco(&st.git, &st.ed.features, root)?;
    let repo = st.git.known_dir_repo(root)?;
    let (rel, e) = deco.entry(path)?;
    (!e.ignored).then_some((repo, rel, e))
}

/// The items for the file at `path` (nothing when it has no changes, or it is a folder).
pub fn items(st: &State, path: &Path) -> Vec<MenuItem> {
    if path.is_dir() {
        return Vec::new();
    }
    let Some((_, _, e)) = state_of(st, path) else { return Vec::new() };
    let mut out: Vec<MenuItem> = Vec::new();
    if e.conflict.is_some() {
        out.push(Action::app("Mark Resolved", mark_resolved).into());
        return out;
    }
    if e.unstaged.is_some() || e.staged.is_some() {
        out.push(Action::app("Open Diff", open_diff).into());
    }
    if e.unstaged.is_some() || e.untracked {
        out.push(Action::app("Stage", stage).into());
    }
    if e.staged.is_some() {
        out.push(Action::app("Unstage", unstage).into());
    }
    if e.unstaged.is_some() {
        out.push(Action::app("Discard Changes…", discard).into());
    }
    out
}

/// The selected row's file (a right click selects its row first) and its git state.
fn target(st: &State, _t: &Target) -> Option<(PathBuf, PathBuf, String, StatusEntry)> {
    let path = st.ed.tree.as_ref()?.current()?.path.clone();
    let (repo, rel, e) = state_of(st, &path)?;
    Some((path, repo, rel, e))
}

fn open_diff(st: &mut State, t: &Target, _: usize) {
    let Some((path, repo, _, e)) = target(st, t) else { return };
    // unstaged changes against the index; a file with only staged ones, the index against HEAD
    let (left, right) = if e.unstaged.is_some() { (Rev::Index, Rev::Worktree) } else { (Rev::Head, Rev::Index) };
    let mut spec = DiffSpec::file(path, left, right).in_repo(repo);
    if e.unstaged.is_none() {
        spec.old_path = e.orig_path.clone();
    }
    st.ed.sidebar_focus = false;
    st.open_diff(spec);
}

fn stage(st: &mut State, t: &Target, _: usize) {
    let Some((_, repo, rel, _)) = target(st, t) else { return };
    super::scm::send_op(st, repo, Op::Stage(vec![rel]));
}

fn unstage(st: &mut State, t: &Target, _: usize) {
    let Some((_, repo, rel, e)) = target(st, t) else { return };
    // a rename unstages both its sides
    let mut paths = vec![rel];
    paths.extend(e.orig_path);
    super::scm::send_op(st, repo, Op::Unstage(paths));
}

fn mark_resolved(st: &mut State, t: &Target, _: usize) {
    let Some((_, repo, rel, _)) = target(st, t) else { return };
    super::scm::send_op(st, repo, Op::MarkResolved(vec![rel]));
}

fn discard(st: &mut State, t: &Target, _: usize) {
    let Some((_, repo, rel, _)) = target(st, t) else { return };
    let q = format!("Discard changes to {rel}?");
    st.git.ask = Some(Ask::confirm(q, Then::OpIn(repo, Op::Discard(vec![rel]))));
}
