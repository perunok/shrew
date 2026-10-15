//! The Source Control panel (feature `git_panel`): the Git tab of the sidebar. A header with the branch, its
//! upstream and a sync button, the commit message box, then the changes in sections: Merge Changes (conflicts),
//! Staged Changes, Changes and Untracked. This module is the model: rows built from a [`Status`], the keys, and
//! the [`Action`]s they ask for, which [`crate::git`] carries out. [`draw`] paints it.
//!
//! Keys in the list: `j`/`k` move, Enter/`l` opens the change (Space keeps the focus here), `o` the file, `s`/`+`
//! stages, `u`/`-` unstages, `x` discards (asks first), `S`/`U` stage or unstage everything, Tab moves between
//! sections and the message box, `c` writes the message, Ctrl+Enter commits (Ctrl+Shift+Enter amends), `b`
//! branches, `f`/`p`/`P` fetch, pull, push, `L` the log, `r` refreshes, Esc gives the keys back, `q` closes.

pub mod draw;
#[cfg(test)]
mod tests;

use super::textbox::{Edit, TextBox};
use shrew_core::{Command, Key, KeyPress};
use shrew_git::{Mark, Op, Status};
use shrew_render::Rect;

/// The groups of the list, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Merge,
    Staged,
    Changes,
    Untracked,
}

impl Section {
    pub const ALL: [Section; 4] = [Section::Merge, Section::Staged, Section::Changes, Section::Untracked];

    pub fn title(self) -> &'static str {
        match self {
            Section::Merge => "MERGE CHANGES",
            Section::Staged => "STAGED CHANGES",
            Section::Changes => "CHANGES",
            Section::Untracked => "UNTRACKED",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One changed file in one section (a partly staged file is in two).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub section: Section,
    /// Repository path.
    pub path: String,
    /// The old path of a staged rename.
    pub orig_path: Option<String>,
    /// The `git status --short` letter shown at the row's end.
    pub letter: char,
    pub mark: Mark,
}

impl File {
    pub fn name(&self) -> &str {
        self.path.trim_end_matches('/').rsplit('/').next().unwrap_or(&self.path)
    }

    /// The folder, without the name (empty at the top).
    pub fn dir(&self) -> &str {
        let p = self.path.trim_end_matches('/');
        p.rfind('/').map_or("", |i| &p[..i])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Header(Section, usize),
    File(File),
}

/// Where the panel's keys go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    List,
    Message,
}

/// What a key or click asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Op(Op),
    /// A destructive op, after a y/n question.
    Ask(String, Op),
    /// Show the change (the diff view, else the file); `keep`: the panel keeps the keys.
    OpenChange(File, bool),
    OpenFile(String, bool),
    Commit {
        amend: bool,
    },
    Run(Command),
    /// A key the panel does not take: the editor's chords (Ctrl+P, `:`) run as in the text.
    Pass(KeyPress),
    Paste,
    /// Give the keys back to the text.
    Unfocus,
    /// Close the Git tab.
    Hide,
}

/// Something clickable in a row or the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// The sidebar's tabs.
    FilesTab,
    GitTab,
    /// Pull, push, publish or fetch, as the header says.
    Sync,
    Commit,
    Amend,
    Abort,
    Continue,
    Cancel,
    /// A row's stage (+), unstage (-), discard and open-file buttons.
    Stage(usize),
    Unstage(usize),
    Discard(usize),
}

/// Where things were drawn in the last frame.
#[derive(Default, Debug)]
pub struct Hits {
    pub area: Option<Rect>,
    pub rows: Vec<(Rect, usize)>,
    pub buttons: Vec<(Rect, Button)>,
    pub message: Option<Rect>,
    /// Where the message's text starts, its cell size, the columns it wraps at and the first row shown.
    pub grid: Option<Grid>,
}

#[derive(Clone, Copy, Debug)]
pub struct Grid {
    pub x: f32,
    pub y: f32,
    pub cw: f32,
    pub ch: f32,
    pub cols: usize,
    pub first: usize,
}

#[derive(Default)]
pub struct Panel {
    /// The Git tab is the one the sidebar shows.
    pub active: bool,
    /// Shown although the sidebar is off (Ctrl+Shift+G); closing it hides the sidebar again.
    pub alone: bool,
    pub focused: bool,
    pub focus: Focus,
    /// Every changed file by section, collapsed or not.
    pub files: [Vec<File>; 4],
    pub rows: Vec<Row>,
    pub selected: usize,
    /// First row shown, and how many fit (set when drawn).
    pub top: usize,
    pub page: usize,
    collapsed: [bool; 4],
    pub message: TextBox,
    /// The row under the pointer (its buttons show).
    pub hover: Option<usize>,
    pub hits: Hits,
    /// Keep the selected row in view at the next draw.
    pub follow: bool,
    /// The last click on a row, for double clicks.
    pub last_click: Option<(std::time::Instant, usize)>,
}

impl Panel {
    /// Rebuilds the rows from a status, keeping the selected file (or its place) selected.
    pub fn set_status(&mut self, status: &Status) {
        let keep = self.current().cloned();
        let mut groups: [Vec<File>; 4] = Default::default();
        for e in &status.entries {
            if e.ignored {
                continue;
            }
            let file = |section, letter: char| File {
                section,
                path: e.path.clone(),
                orig_path: if section == Section::Staged { e.orig_path.clone() } else { None },
                letter,
                mark: e.mark(),
            };
            // git's letters, as the sidebar shows them: U unmerged, ? untracked
            if e.conflict.is_some() {
                groups[0].push(file(Section::Merge, 'U'));
                continue;
            }
            if e.untracked {
                groups[3].push(File { mark: Mark::Untracked, ..file(Section::Untracked, '?') });
                continue;
            }
            if let Some(s) = e.staged {
                groups[1].push(File { mark: mark_of(s), ..file(Section::Staged, s.letter()) });
            }
            if let Some(s) = e.unstaged {
                groups[2].push(File { mark: mark_of(s), ..file(Section::Changes, s.letter()) });
            }
        }
        self.files = groups;
        self.rebuild(keep);
    }

    /// The rows from the files and the folded sections.
    fn rebuild(&mut self, keep: Option<Row>) {
        self.rows.clear();
        for (i, files) in self.files.iter().enumerate() {
            if files.is_empty() {
                continue;
            }
            self.rows.push(Row::Header(Section::ALL[i], files.len()));
            if !self.collapsed[i] {
                self.rows.extend(files.iter().cloned().map(Row::File));
            }
        }
        self.reselect(keep);
    }

    /// Selects `keep` again after the rows changed: the same row, else the row now in its place.
    fn reselect(&mut self, keep: Option<Row>) {
        let found = keep.as_ref().and_then(|k| match k {
            Row::Header(s, _) => self.rows.iter().position(|r| matches!(r, Row::Header(x, _) if x == s)),
            Row::File(f) => {
                self.rows.iter().position(|r| matches!(r, Row::File(g) if g.path == f.path && g.section == f.section))
            }
        });
        if let Some(i) = found {
            self.selected = i;
        } else {
            // the file moved to another section (staged): stay at the same index, on a file if there is one
            self.selected = self.selected.min(self.rows.len().saturating_sub(1));
            if matches!(self.rows.get(self.selected), Some(Row::Header(..)))
                && matches!(self.rows.get(self.selected + 1), Some(Row::File(_)))
                && matches!(keep, Some(Row::File(_)))
            {
                self.selected += 1;
            }
        }
        self.hover = None;
        self.follow = true;
    }

    pub fn current(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn current_file(&self) -> Option<&File> {
        match self.current()? {
            Row::File(f) => Some(f),
            Row::Header(..) => None,
        }
    }

    /// Changed files in all (for the tab's badge).
    pub fn total(&self) -> usize {
        self.files.iter().map(Vec::len).sum()
    }

    pub fn is_collapsed(&self, s: Section) -> bool {
        self.collapsed[s.index()]
    }

    /// Folds or unfolds a section.
    pub fn toggle_section(&mut self, s: Section) {
        self.collapsed[s.index()] = !self.collapsed[s.index()];
        self.rebuild(Some(Row::Header(s, 0)));
    }

    pub fn move_by(&mut self, d: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as isize - 1;
        self.selected = (self.selected as isize + d).clamp(0, last) as usize;
        self.follow = true;
    }

    /// Keeps the selection in view in `visible` rows.
    pub fn fit(&mut self, visible: usize) {
        self.page = visible.max(1);
        let max_top = self.rows.len().saturating_sub(self.page);
        if self.follow {
            if self.selected < self.top {
                self.top = self.selected;
            } else if self.selected >= self.top + self.page {
                self.top = self.selected + 1 - self.page;
            }
            self.follow = false;
        }
        self.top = self.top.min(max_top);
    }

    pub fn scroll(&mut self, lines: isize) {
        let max_top = self.rows.len().saturating_sub(self.page.max(1));
        self.top = (self.top as isize + lines).clamp(0, max_top as isize) as usize;
    }

    /// The next (or previous) section header from the selection; `None` past the last (first) one.
    fn section_step(&self, forward: bool) -> Option<usize> {
        let headers = self.rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Header(..))).map(|(i, _)| i);
        if forward {
            headers.into_iter().find(|&i| i > self.selected)
        } else {
            // the header of the section the selection is in counts as "this one"
            let all: Vec<usize> = headers.collect();
            let here = all.iter().rposition(|&i| i <= self.selected);
            here.and_then(|p| p.checked_sub(1)).map(|p| all[p])
        }
    }

    /// The stage action for a row (a header: its whole section).
    pub fn stage_action(&self, i: usize) -> Option<Action> {
        Some(match self.rows.get(i)? {
            Row::File(f) => match f.section {
                Section::Merge => Action::Op(Op::MarkResolved(vec![f.path.clone()])),
                Section::Staged => return None,
                Section::Changes | Section::Untracked => Action::Op(Op::Stage(vec![f.path.clone()])),
            },
            Row::Header(s, _) => match s {
                Section::Merge => Action::Op(Op::MarkResolved(self.paths(*s))),
                Section::Staged => return None,
                Section::Changes | Section::Untracked => Action::Op(Op::Stage(self.paths(*s))),
            },
        })
    }

    pub fn unstage_action(&self, i: usize) -> Option<Action> {
        Some(match self.rows.get(i)? {
            Row::File(f) if f.section == Section::Staged => {
                // a rename unstages both its sides
                let mut paths = vec![f.path.clone()];
                paths.extend(f.orig_path.clone());
                Action::Op(Op::Unstage(paths))
            }
            Row::Header(Section::Staged, _) => Action::Op(Op::UnstageAll),
            _ => return None,
        })
    }

    pub fn discard_action(&self, i: usize) -> Option<Action> {
        let n = |k: usize, one: &str, many: &str| if k == 1 { one.to_string() } else { format!("{k} {many}") };
        Some(match self.rows.get(i)? {
            Row::File(f) => match f.section {
                Section::Changes => {
                    Action::Ask(format!("Discard changes to {}?", f.path), Op::Discard(vec![f.path.clone()]))
                }
                Section::Untracked => {
                    Action::Ask(format!("Delete untracked {} for good?", f.path), Op::Clean(vec![f.path.clone()]))
                }
                _ => return None,
            },
            Row::Header(Section::Changes, k) => Action::Ask(
                format!("Discard all changes in {}?", n(*k, "1 file", "files")),
                Op::Discard(self.paths(Section::Changes)),
            ),
            Row::Header(Section::Untracked, k) => Action::Ask(
                format!("Delete {} for good?", n(*k, "the untracked file", "untracked files")),
                Op::Clean(self.paths(Section::Untracked)),
            ),
            _ => return None,
        })
    }

    /// Every path in a section, folded or not.
    fn paths(&self, s: Section) -> Vec<String> {
        self.files[s.index()].iter().map(|f| f.path.clone()).collect()
    }

    /// A key while the panel has the focus.
    pub fn key(&mut self, k: KeyPress) -> Option<Action> {
        let m = k.mods;
        if k.key == Key::Enter && m.ctrl {
            return Some(Action::Commit { amend: m.shift });
        }
        if self.focus == Focus::Message {
            return self.message_key(k);
        }
        if m.ctrl || m.alt || m.logo {
            let page = self.page.max(2) as isize;
            match k.key {
                Key::Char('d') if m.ctrl && !m.shift => self.move_by(page / 2),
                Key::Char('u') if m.ctrl && !m.shift => self.move_by(-page / 2),
                Key::Char('n' | 'j') if m.ctrl && !m.shift => self.move_by(1),
                Key::Char('p' | 'k') if m.ctrl && !m.shift => self.move_by(-1),
                Key::Char('c' | '[') if m.ctrl => return Some(Action::Unfocus),
                _ => return Some(Action::Pass(k)),
            }
            return None;
        }
        let sel = self.selected;
        match k.key {
            Key::Down | Key::Char('j') => self.move_by(1),
            Key::Up | Key::Char('k') => self.move_by(-1),
            Key::Home | Key::Char('g') => self.move_by(isize::MIN / 2),
            Key::End | Key::Char('G') => self.move_by(isize::MAX / 2),
            Key::PageDown => self.move_by(self.page.max(2) as isize),
            Key::PageUp => self.move_by(-(self.page.max(2) as isize)),
            Key::Enter | Key::Right | Key::Char('l') | Key::Char(' ') => match self.current()?.clone() {
                Row::Header(s, _) => {
                    if k.key != Key::Right && k.key != Key::Char('l') || self.is_collapsed(s) {
                        self.toggle_section(s);
                    }
                }
                Row::File(f) => return Some(Action::OpenChange(f, k.key == Key::Char(' '))),
            },
            Key::Left | Key::Char('h') => match self.current()?.clone() {
                Row::Header(s, _) if !self.is_collapsed(s) => self.toggle_section(s),
                Row::Header(..) => {}
                Row::File(_) => {
                    if let Some(i) = self.rows[..sel].iter().rposition(|r| matches!(r, Row::Header(..))) {
                        self.selected = i;
                        self.follow = true;
                    }
                }
            },
            Key::Char('o') => return self.current_file().map(|f| Action::OpenFile(f.path.clone(), false)),
            Key::Char('s' | '+') => return self.stage_action(sel),
            Key::Char('u' | '-') => return self.unstage_action(sel),
            Key::Char('x') | Key::Delete => return self.discard_action(sel),
            Key::Char('S') => return Some(Action::Op(Op::StageAll)),
            Key::Char('U') => return Some(Action::Op(Op::UnstageAll)),
            Key::Char('c' | 'i') => self.focus = Focus::Message,
            Key::Tab => match self.section_step(!m.shift) {
                Some(i) => {
                    self.selected = i;
                    self.follow = true;
                }
                None => self.focus = Focus::Message,
            },
            Key::Char('r') | Key::F(5) => return Some(Action::Run(Command::GitRefresh)),
            Key::Char('b') => return Some(Action::Run(Command::GitBranches)),
            Key::Char('f') => return Some(Action::Run(Command::GitFetch)),
            Key::Char('p') => return Some(Action::Run(Command::GitPull)),
            Key::Char('P') => return Some(Action::Run(Command::GitPush)),
            Key::Char('L') => return Some(Action::Run(Command::GitLog)),
            Key::Char('z') => return Some(Action::Run(Command::GitStash)),
            Key::Char(':') => return Some(Action::Pass(k)),
            Key::Esc => return Some(Action::Unfocus),
            Key::Char('q') => return Some(Action::Hide),
            _ => {}
        }
        None
    }

    fn message_key(&mut self, k: KeyPress) -> Option<Action> {
        let m = k.mods;
        match k.key {
            Key::Esc => self.focus = Focus::List,
            Key::Char('c' | '[') if m.ctrl => self.focus = Focus::List,
            Key::Tab => {
                self.focus = Focus::List;
                if m.shift {
                    self.selected = self.rows.len().saturating_sub(1);
                } else {
                    self.selected = 0;
                }
                self.follow = true;
            }
            Key::Char('v') if m.ctrl && !m.shift => return Some(Action::Paste),
            Key::Down if !self.message.text[self.message.cursor..].contains('\n') => {
                // past the last line: on to the list
                self.focus = Focus::List;
                self.follow = true;
            }
            _ => {
                if self.message.key(k, true) == Edit::Ignored && (m.ctrl || m.alt || m.logo) {
                    return Some(Action::Pass(k));
                }
            }
        }
        None
    }
}

fn mark_of(s: shrew_git::FileState) -> Mark {
    use shrew_git::FileState::*;
    match s {
        Added => Mark::Added,
        Deleted => Mark::Deleted,
        Renamed | Copied => Mark::Renamed,
        Modified | TypeChanged => Mark::Modified,
    }
}

/// How long a commit subject may be before the hint warns (git's 50/72 convention).
pub const SUBJECT_SOFT: usize = 50;
pub const SUBJECT_HARD: usize = 72;
