//! `git status` in-process (gix, parallel): HEAD vs index ("staged"), index vs worktree ("unstaged"), untracked,
//! ignored on demand, conflicts and staged renames. Read-only: the index is never rewritten.

use crate::cli::Cancel;
use crate::error::{Error, ErrorKind, Result};
use crate::repo::{Head, Repo};
use gix::status::index_worktree::Item as WtItem;
use gix::status::plumbing::index_as_worktree::{Change as WtChange, Conflict, EntryStatus};

/// How a file differs on one side.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FileState {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    /// File became symlink, or the other way round.
    TypeChanged,
}

impl FileState {
    /// The one-letter code `git status --short` uses.
    pub fn letter(self) -> char {
        match self {
            FileState::Added => 'A',
            FileState::Modified => 'M',
            FileState::Deleted => 'D',
            FileState::Renamed => 'R',
            FileState::Copied => 'C',
            FileState::TypeChanged => 'T',
        }
    }
}

/// How a path conflicts during a merge, rebase or cherry-pick.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ConflictKind {
    BothModified,
    BothAdded,
    BothDeleted,
    AddedByUs,
    AddedByThem,
    DeletedByUs,
    DeletedByThem,
}

impl ConflictKind {
    /// The two letters of `git status --short` (`UU`, `AA`, ...).
    pub fn letters(self) -> &'static str {
        match self {
            ConflictKind::BothModified => "UU",
            ConflictKind::BothAdded => "AA",
            ConflictKind::BothDeleted => "DD",
            ConflictKind::AddedByUs => "AU",
            ConflictKind::AddedByThem => "UA",
            ConflictKind::DeletedByUs => "DU",
            ConflictKind::DeletedByThem => "UD",
        }
    }
}

/// One path that is not clean.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StatusEntry {
    /// Repository path, `/`-separated. An untracked or ignored folder shown as a whole ends in `/`.
    pub path: String,
    /// The old path of a staged rename or copy.
    pub orig_path: Option<String>,
    /// HEAD -> index.
    pub staged: Option<FileState>,
    /// Index -> worktree.
    pub unstaged: Option<FileState>,
    pub conflict: Option<ConflictKind>,
    pub untracked: bool,
    pub ignored: bool,
    /// The path is a submodule (its changes are its new commits or dirty worktree).
    pub submodule: bool,
}

impl StatusEntry {
    /// The two-letter code of `git status --short` (`M `, ` M`, `??`, `!!`, `UU`, ...).
    pub fn short_code(&self) -> String {
        if let Some(c) = self.conflict {
            return c.letters().into();
        }
        if self.untracked {
            return "??".into();
        }
        if self.ignored {
            return "!!".into();
        }
        let l = |s: Option<FileState>| s.map_or(' ', FileState::letter);
        format!("{}{}", l(self.staged), l(self.unstaged))
    }

    /// The most notable state for a single mark (file tree, tab): conflict, then worktree, then index.
    pub fn mark(&self) -> Mark {
        if self.conflict.is_some() {
            Mark::Conflict
        } else if self.untracked {
            Mark::Untracked
        } else if self.ignored {
            Mark::Ignored
        } else {
            match self.unstaged.or(self.staged) {
                Some(FileState::Added) => Mark::Added,
                Some(FileState::Deleted) => Mark::Deleted,
                Some(FileState::Renamed | FileState::Copied) => Mark::Renamed,
                _ => Mark::Modified,
            }
        }
    }
}

/// A single-colour summary of a path's state, matching the theme's git tokens.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mark {
    Ignored,
    Untracked,
    Added,
    Renamed,
    Modified,
    Deleted,
    Conflict,
}

/// Which untracked files to list.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Untracked {
    None,
    /// As `git status` by default: an untracked folder is one entry (`dir/`).
    #[default]
    Normal,
    /// Every untracked file.
    All,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StatusOptions {
    pub untracked: Untracked,
    /// Also list ignored files. An ignored folder is always one entry (`build/`), never its files.
    pub ignored: bool,
}

/// A whole repository's status, entries sorted by path.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Status {
    pub head: Head,
    pub entries: Vec<StatusEntry>,
}

impl Status {
    /// The entry for `path`; a file inside an untracked or ignored folder gets that folder's entry.
    pub fn get(&self, path: &str) -> Option<&StatusEntry> {
        if let Ok(i) = self.entries.binary_search_by(|e| e.path.as_str().cmp(path)) {
            return Some(&self.entries[i]);
        }
        // inside an untracked or ignored folder listed as a whole
        let find = |p: &str| self.entries.binary_search_by(|e| e.path.as_str().cmp(p)).ok();
        path.match_indices('/').find_map(|(i, _)| find(&path[..=i])).map(|i| &self.entries[i])
    }

    pub fn staged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.staged.is_some())
    }

    /// Changed tracked files not staged (conflicts excluded).
    pub fn unstaged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.unstaged.is_some())
    }

    pub fn untracked(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.untracked)
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.conflict.is_some())
    }

    /// Nothing staged, changed, untracked or conflicted (ignored files do not count).
    pub fn is_clean(&self) -> bool {
        self.entries.iter().all(|e| e.ignored)
    }
}

fn slot(map: &mut std::collections::BTreeMap<String, StatusEntry>, path: String) -> &mut StatusEntry {
    map.entry(path.clone()).or_insert_with(|| StatusEntry { path, ..StatusEntry::default() })
}

fn conflict_kind(c: Conflict) -> ConflictKind {
    match c {
        Conflict::BothDeleted => ConflictKind::BothDeleted,
        Conflict::AddedByUs => ConflictKind::AddedByUs,
        Conflict::DeletedByThem => ConflictKind::DeletedByThem,
        Conflict::AddedByThem => ConflictKind::AddedByThem,
        Conflict::DeletedByUs => ConflictKind::DeletedByUs,
        Conflict::BothAdded => ConflictKind::BothAdded,
        Conflict::BothModified => ConflictKind::BothModified,
    }
}

impl Repo {
    /// The status of the whole worktree (whatever the process's current directory). Stops early with
    /// [`ErrorKind::Cancelled`] when `cancel` fires.
    pub fn status(&self, opts: &StatusOptions, cancel: &Cancel) -> Result<Status> {
        use gix::status::UntrackedFiles;
        use std::collections::BTreeMap;

        let head = self.head()?;
        let untracked = match opts.untracked {
            Untracked::None => UntrackedFiles::None,
            Untracked::Normal => UntrackedFiles::Collapsed,
            Untracked::All => UntrackedFiles::Files,
        };
        let mut platform = self
            .git
            .status(gix::progress::Discard)?
            .should_interrupt_owned(cancel.flag())
            .index_worktree_rewrites(None)
            .index_worktree_submodules(gix::status::Submodule::AsConfigured { check_dirty: true })
            .untracked_files(untracked)
            .dirwalk_options(|o| o.empty_patterns_match_prefix(false));
        if opts.ignored {
            let mode = gix::dir::walk::EmissionMode::CollapseDirectory;
            platform = platform.dirwalk_options(|o| o.emit_ignored(Some(mode)));
        }
        let iter = platform.into_iter(Vec::<gix::bstr::BString>::new())?;

        let mut map: BTreeMap<String, StatusEntry> = BTreeMap::new();
        for item in iter {
            if cancel.is_cancelled() {
                return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
            }
            match item? {
                gix::status::Item::IndexWorktree(WtItem::Modification { entry: ie, rela_path, status, .. }) => {
                    let submodule = ie.mode.is_submodule();
                    let state = match status {
                        EntryStatus::Conflict { summary, .. } => {
                            slot(&mut map, rela_path.to_string()).conflict = Some(conflict_kind(summary));
                            continue;
                        }
                        EntryStatus::NeedsUpdate(_) => continue,
                        EntryStatus::IntentToAdd => FileState::Added,
                        EntryStatus::Change(WtChange::Removed) => FileState::Deleted,
                        EntryStatus::Change(WtChange::Type { .. }) => FileState::TypeChanged,
                        EntryStatus::Change(WtChange::Modification { .. }) => FileState::Modified,
                        EntryStatus::Change(WtChange::SubmoduleModification(s)) => {
                            if s.is_dirty() == Some(false) {
                                continue;
                            }
                            FileState::Modified
                        }
                    };
                    let e = slot(&mut map, rela_path.to_string());
                    e.unstaged = Some(state);
                    e.submodule |= submodule;
                }
                gix::status::Item::IndexWorktree(WtItem::DirectoryContents { entry: de, .. }) => {
                    use gix::dir::entry::Status as S;
                    let (untracked, ignored) = match de.status {
                        S::Untracked => (true, false),
                        S::Ignored(_) => (false, true),
                        S::Tracked | S::Pruned => continue,
                    };
                    let mut path = de.rela_path.to_string();
                    if de.disk_kind == Some(gix::dir::entry::Kind::Directory) {
                        path.push('/');
                    }
                    let e = slot(&mut map, path);
                    e.untracked = untracked;
                    e.ignored = ignored;
                    e.submodule |= de.disk_kind == Some(gix::dir::entry::Kind::Repository);
                }
                gix::status::Item::IndexWorktree(WtItem::Rewrite { .. }) => {} // worktree renames are off
                gix::status::Item::TreeIndex(change) => {
                    use gix::diff::index::ChangeRef as C;
                    match change {
                        C::Addition { location, entry_mode, .. } => {
                            let e = slot(&mut map, location.to_string());
                            e.staged = Some(FileState::Added);
                            e.submodule |= entry_mode.is_submodule();
                        }
                        C::Deletion { location, entry_mode, .. } => {
                            let e = slot(&mut map, location.to_string());
                            e.staged = Some(FileState::Deleted);
                            e.submodule |= entry_mode.is_submodule();
                        }
                        C::Modification { location, previous_entry_mode, entry_mode, .. } => {
                            use gix::object::tree::EntryKind;
                            let kind = |m: gix::index::entry::Mode| m.to_tree_entry_mode().map(|t| t.kind());
                            let blob = |k| matches!(k, Some(EntryKind::Blob | EntryKind::BlobExecutable));
                            let (was, is) = (kind(previous_entry_mode), kind(entry_mode));
                            let state = if was != is && !(blob(was) && blob(is)) {
                                FileState::TypeChanged
                            } else {
                                FileState::Modified
                            };
                            let e = slot(&mut map, location.to_string());
                            e.staged = Some(state);
                            e.submodule |= entry_mode.is_submodule();
                        }
                        C::Rewrite { source_location, location, copy, .. } => {
                            let e = slot(&mut map, location.to_string());
                            e.staged = Some(if copy { FileState::Copied } else { FileState::Renamed });
                            e.orig_path = Some(source_location.to_string());
                        }
                    }
                }
            }
        }
        let entries = map
            .into_values()
            .map(|mut e| {
                if e.conflict.is_some() {
                    e.staged = None;
                    e.unstaged = None;
                }
                e
            })
            .collect();
        Ok(Status { head, entries })
    }
}
