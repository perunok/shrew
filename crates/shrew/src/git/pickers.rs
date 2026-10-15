//! Git lists in the picker (the editor's fuzzy list with the app's rows, [`Choice::GitBranches`] and friends):
//!
//! - **Branches** (`git_branches`, `b` in the panel): local branches newest first, then remote branches and tags.
//!   Enter checks one out (a remote one as a new tracking branch, a tag detached); typing a new name offers
//!   "+ Create branch", which Ctrl+Enter also does from any row; Delete deletes the selected local branch (asks).
//! - **Stashes** (`git_stashes`): Enter pops, Ctrl+Enter applies, Delete drops (asks).
//! - **Changed files** (`git_changed_files`): Enter opens the change.

use super::ask::{Ask, Then};
use super::{Purpose, ago, send_op};
use crate::state::State;
use shrew_core::picker::{Choice, PickerData, PickerItem, PickerKind};
use shrew_core::{Key, KeyPress};
use shrew_git::{Op, Request};
use std::path::PathBuf;

const LOCAL: &str = "b";
const REMOTE: &str = "r";
const TAG: &str = "t";
const CREATE: &str = "+";

fn value(kind: &str, name: &str) -> String {
    format!("{kind}\t{name}")
}

fn parse(v: &str) -> (&str, &str) {
    v.split_once('\t').unwrap_or(("", v))
}

/// Opens a list at once (typing goes to it while it loads): refs come from the service (fast), changed files
/// from the status.
pub fn open(st: &mut State, repo: PathBuf, choice: Choice) {
    let known = match choice {
        Choice::GitChanges => st.git.scm.status.is_some(),
        _ => st.git.scm.refs.is_some(),
    };
    if known {
        fill(st, choice);
    } else {
        st.ed.open_choice(choice, Vec::new(), 0);
        if let Some(p) = &mut st.ed.picker {
            p.loading = true;
        }
        hint(st, choice);
    }
    match choice {
        Choice::GitChanges if !known => {
            super::request_status(st);
            st.git.scm.changes_pending = true;
        }
        Choice::GitChanges => {}
        _ => {
            st.git.send(Request::Refs { repo }, Purpose::Refs(Some(choice)));
        }
    }
}

/// The rows of a list from what is known now. A list already open keeps its query and selection.
pub fn fill(st: &mut State, choice: Choice) {
    let items = match choice {
        Choice::GitBranches => branch_items(st),
        Choice::GitStashes => stash_items(st),
        Choice::GitChanges => change_items(st),
        _ => return,
    };
    if choice == Choice::GitStashes && items.is_empty() {
        if is_open(st, choice) {
            st.ed.picker = None;
        }
        return st.ed.info("no stashes");
    }
    if let Some(p) = st.ed.picker.as_mut().filter(|p| p.kind == PickerKind::Choice(choice)) {
        let keep = p.current().filter(|_| p.selected > 0).map(|i| i.label.clone());
        let first = p.items.is_empty();
        p.items = items;
        p.loading = false;
        p.refilter();
        // just loaded with nothing typed: the same row as a list opened with its items
        if first && p.query.is_empty() && choice == Choice::GitBranches {
            p.selected = p.items.iter().position(|i| !i.detail.starts_with("current")).unwrap_or(0);
        }
        if let Some(k) = keep
            && let Some(i) = p.matches.iter().position(|&m| p.items[m as usize].label == k)
        {
            p.selected = i;
        }
        update_create_row(st);
    } else {
        // branches: the most recent one that is not checked out, so Enter goes back to it
        let selected = match choice {
            Choice::GitBranches => items.iter().position(|i| !i.detail.starts_with("current")).unwrap_or(0),
            _ => 0,
        };
        st.ed.open_choice(choice, items, selected);
    }
    hint(st, choice);
}

fn is_open(st: &State, choice: Choice) -> bool {
    st.ed.picker.as_ref().is_some_and(|p| p.kind == PickerKind::Choice(choice))
}

/// The keys of a list, on the message line (picker keys clear it, so it is put back after each).
fn hint(st: &mut State, choice: Choice) {
    let h = match choice {
        Choice::GitBranches => "Enter check out · Ctrl+Enter new branch from the query · Delete delete",
        Choice::GitStashes => "Enter pop · Ctrl+Enter apply · Delete drop",
        Choice::GitChanges => "Enter open the change",
        _ => return,
    };
    st.ed.info(h);
}

fn branch_items(st: &State) -> Vec<PickerItem> {
    let Some(refs) = &st.git.scm.refs else { return Vec::new() };
    let item = |label: String, detail: String, v: String| PickerItem { label, detail, data: PickerData::Value(v) };
    let mut local: Vec<_> = refs.branches.iter().collect();
    local.sort_by_key(|b| std::cmp::Reverse(b.time));
    let mut out = Vec::new();
    for b in local {
        let mut d = if b.is_head { "current · ".to_string() } else { String::new() };
        d += &ago(b.time);
        if let Some(u) = &b.upstream {
            d += &format!(" · {u}");
        }
        d += &format!(" · {}", b.summary);
        out.push(item(b.name.clone(), d, value(LOCAL, &b.name)));
    }
    let mut remote: Vec<_> = refs.remote_branches.iter().filter(|b| !b.name.ends_with("/HEAD")).collect();
    remote.sort_by_key(|b| std::cmp::Reverse(b.time));
    for b in remote {
        out.push(item(b.name.clone(), format!("remote · {} · {}", ago(b.time), b.summary), value(REMOTE, &b.name)));
    }
    let mut tags: Vec<_> = refs.tags.iter().collect();
    tags.sort_by(|a, b| b.name.cmp(&a.name));
    for t in tags {
        let msg = t.message.as_deref().and_then(|m| m.lines().next()).unwrap_or("");
        out.push(item(t.name.clone(), format!("tag {msg}").trim_end().to_string(), value(TAG, &t.name)));
    }
    out
}

fn stash_items(st: &State) -> Vec<PickerItem> {
    let Some(refs) = &st.git.scm.refs else { return Vec::new() };
    refs.stashes
        .iter()
        .map(|s| PickerItem {
            label: format!("stash@{{{}}}  {}", s.index, s.message),
            detail: ago(s.time),
            data: PickerData::Value(s.index.to_string()),
        })
        .collect()
}

fn change_items(st: &State) -> Vec<PickerItem> {
    let Some(s) = &st.git.scm.status else { return Vec::new() };
    s.entries
        .iter()
        .filter(|e| !e.ignored)
        .map(|e| {
            let what = if e.conflict.is_some() {
                "conflict"
            } else if e.untracked {
                "untracked"
            } else if e.staged.is_some() && e.unstaged.is_some() {
                "partly staged"
            } else if e.staged.is_some() {
                "staged"
            } else {
                "changed"
            };
            PickerItem {
                label: e.path.clone(),
                detail: format!("{}  {what}", e.short_code()),
                data: PickerData::Value(e.path.clone()),
            }
        })
        .collect()
}

/// A status came in: an open changed-files list follows it, a waiting one opens.
pub fn status_changed(st: &mut State) {
    if std::mem::take(&mut st.git.scm.changes_pending) || is_open(st, Choice::GitChanges) {
        fill(st, Choice::GitChanges);
    }
}

/// The branches list offers to create a branch named by the query when no branch has that name.
fn update_create_row(st: &mut State) {
    let Some(p) = st.ed.picker.as_mut().filter(|p| p.kind == PickerKind::Choice(Choice::GitBranches)) else {
        return;
    };
    let query = p.query.trim().to_string();
    let want = (!query.is_empty() && !query.contains(char::is_whitespace))
        .then(|| query.clone())
        .filter(|q| !p.items.iter().any(|i| parse(value_of(i)) == (LOCAL, q.as_str())));
    let have = p.items.iter().position(|i| parse(value_of(i)).0 == CREATE);
    let same = match (have, &want) {
        (Some(i), Some(q)) => parse(value_of(&p.items[i])).1 == q,
        (None, None) => true,
        _ => false,
    };
    if same {
        return;
    }
    if let Some(i) = have {
        p.items.remove(i);
    }
    if let Some(q) = want {
        p.items.push(PickerItem {
            label: format!("+ Create branch {q}"),
            detail: "from HEAD (Ctrl+Enter)".into(),
            data: PickerData::Value(value(CREATE, &q)),
        });
    }
    p.refilter();
}

fn value_of(i: &PickerItem) -> &str {
    match &i.data {
        PickerData::Value(v) => v,
        _ => "",
    }
}

/// Keys while a git list is open: its own (Ctrl+Enter, Delete), then the picker's.
pub fn key(st: &mut State, choice: Choice, k: KeyPress) -> bool {
    let current = st.ed.picker.as_ref().and_then(|p| p.current()).map(|i| value_of(i).to_string());
    let query = st.ed.picker.as_ref().map(|p| p.query.trim().to_string()).unwrap_or_default();
    let ctrl_enter = k.key == Key::Enter && k.mods.ctrl;
    let delete = k.key == Key::Delete || (k.key == Key::Char('d') && k.mods.ctrl && k.mods.shift);
    match choice {
        Choice::GitBranches if ctrl_enter => {
            let name = if query.is_empty() { None } else { Some(query) };
            st.ed.picker = None;
            match name {
                Some(n) => run(st, Op::CreateBranch { name: n, start: None, switch: true }),
                None => st.git.ask = Some(Ask::text("New branch:", "", Then::NewBranch { start: None })),
            }
            return true;
        }
        Choice::GitBranches if delete => {
            if let Some(v) = current {
                let (kind, name) = parse(&v);
                if kind == LOCAL {
                    st.git.ask = Some(Ask::confirm(
                        format!("Delete branch {name}?"),
                        Then::DeleteBranch { name: name.to_string(), force: false },
                    ));
                } else {
                    st.ed.info("only local branches are deleted here");
                }
            }
            return true;
        }
        Choice::GitStashes if ctrl_enter || delete => {
            if let Some(i) = current.and_then(|v| v.parse::<usize>().ok()) {
                if delete {
                    st.git.ask = Some(Ask::confirm(format!("Drop stash@{{{i}}}?"), Then::DropStash(i)));
                } else {
                    st.ed.picker = None;
                    run(st, Op::StashApply(i));
                }
            }
            return true;
        }
        _ => {}
    }
    st.ed.handle_key(k);
    if is_open(st, choice) {
        update_create_row(st);
        if st.ed.message.is_none() {
            hint(st, choice);
        }
    }
    st.process_events();
    true
}

fn run(st: &mut State, op: Op) {
    if let Some(repo) = st.git.workdir().map(PathBuf::from) {
        send_op(st, repo, op);
    }
}

/// Enter on a row.
pub fn choose(st: &mut State, choice: Choice, v: String) {
    match choice {
        Choice::GitBranches => {
            let (kind, name) = parse(&v);
            let name = name.to_string();
            let op = match kind {
                LOCAL => {
                    if st.git.scm.status.as_ref().is_some_and(|s| s.head.branch.as_deref() == Some(&name)) {
                        return st.ed.info(format!("already on {name}"));
                    }
                    Op::Switch(name)
                }
                // a remote branch: a local one of the same name tracking it
                REMOTE => {
                    let local = name.split_once('/').map_or(name.as_str(), |(_, b)| b).to_string();
                    let exists = st.git.scm.refs.as_ref().is_some_and(|r| r.branches.iter().any(|b| b.name == local));
                    if exists {
                        Op::Switch(local)
                    } else {
                        Op::CreateBranch { name: local, start: Some(name), switch: true }
                    }
                }
                TAG => Op::SwitchDetach(name),
                CREATE => Op::CreateBranch { name, start: None, switch: true },
                _ => return,
            };
            run(st, op);
        }
        Choice::GitStashes => {
            if let Ok(i) = v.parse::<usize>() {
                run(st, Op::StashPop(i));
            }
        }
        Choice::GitChanges => {
            let file = st.git.scm.status.as_ref().and_then(|s| s.get(&v)).map(|e| super::panel::File {
                section: if e.untracked {
                    super::panel::Section::Untracked
                } else if e.unstaged.is_some() {
                    super::panel::Section::Changes
                } else {
                    super::panel::Section::Staged
                },
                path: e.path.clone(),
                orig_path: e.orig_path.clone(),
                letter: ' ',
                mark: e.mark(),
            });
            if let Some(f) = file {
                super::perform(st, super::panel::Action::OpenChange(f, false));
            }
        }
        _ => {}
    }
}
