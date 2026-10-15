//! Diffs between any two versions of the tree: a commit, HEAD, the index or the worktree. Per file (texts and
//! hunks, for a diff view) or for the whole tree (the changed files with line counts).

use crate::cli::Cancel;
use crate::diff::{EolMode, Hunk, WordDiff, diff, word_diff};
use crate::error::Result;
use crate::log::{FileChange, text_stats};
use crate::oid::Oid;
use crate::repo::{MODE_LINK, Repo};
use crate::status::{FileState, StatusOptions, Untracked};
use crate::text::Text;
use std::collections::BTreeSet;

/// A version of the tree.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Version {
    Commit(Oid),
    Head,
    Index,
    Worktree,
}

impl Version {
    /// Order along the usual flow commit -> HEAD -> index -> worktree (commits count as HEAD).
    fn rank(self) -> u8 {
        match self {
            Version::Commit(_) | Version::Head => 0,
            Version::Index => 1,
            Version::Worktree => 2,
        }
    }
}

/// One file in two versions.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDiff {
    /// `None` when the file does not exist in that version.
    pub old: Option<Text>,
    pub new: Option<Text>,
    pub old_mode: Option<u32>,
    pub new_mode: Option<u32>,
    /// Empty for binary files.
    pub hunks: Vec<Hunk>,
    /// The changed words of each hunk (same order; empty for added, deleted or very different lines).
    pub words: Vec<WordDiff>,
    pub binary: bool,
    /// How the hunks were computed (Normalize when one side is the worktree or index).
    pub eol: EolMode,
}

impl FileDiff {
    pub fn old_text(&self) -> Text {
        self.old.clone().unwrap_or_default()
    }

    pub fn new_text(&self) -> Text {
        self.new.clone().unwrap_or_default()
    }
}

impl Repo {
    /// `path` in `version`, with its mode; `None` when absent.
    pub fn read_version(&self, path: &str, version: Version) -> Result<Option<(Text, u32)>> {
        Ok(match version {
            Version::Commit(id) => self.blob_at(id, path)?.map(|b| (b.text, b.mode)),
            Version::Head => self.head_blob(path)?.map(|b| (b.text, b.mode)),
            Version::Index => self.index_blob(path)?.map(|b| (b.text, b.mode)),
            Version::Worktree => self.worktree_file(path)?,
        })
    }

    /// `path` between two versions. `old_path` names the file on the old side after a rename.
    pub fn file_diff(&self, path: &str, old_path: Option<&str>, old: Version, new: Version) -> Result<FileDiff> {
        let a = self.read_version(old_path.unwrap_or(path), old)?;
        let b = self.read_version(path, new)?;
        let eol =
            if old == Version::Worktree || new == Version::Worktree { EolMode::Normalize } else { EolMode::Exact };
        let (ot, nt) =
            (a.as_ref().map(|x| x.0.clone()).unwrap_or_default(), b.as_ref().map(|x| x.0.clone()).unwrap_or_default());
        let binary = ot.is_binary() || nt.is_binary();
        let hunks = if binary { Vec::new() } else { diff(&ot, &nt, eol) };
        let words = hunks.iter().map(|h| word_diff(&ot, &nt, h)).collect();
        Ok(FileDiff {
            old_mode: a.as_ref().map(|x| x.1),
            new_mode: b.as_ref().map(|x| x.1),
            old: a.map(|x| x.0),
            new: b.map(|x| x.0),
            hunks,
            words,
            binary,
            eol,
        })
    }

    /// Every file that differs between two versions, with line counts, sorted by path. Untracked files count as
    /// added in the worktree. Renames are detected between commits and for staged changes.
    pub fn tree_diff(&self, old: Version, new: Version, cancel: &Cancel) -> Result<Vec<FileChange>> {
        if old.rank() > new.rank() {
            let mut out = self.tree_diff(new, old, cancel)?;
            for c in &mut out {
                std::mem::swap(&mut c.added, &mut c.removed);
                c.state = match c.state {
                    FileState::Added => FileState::Deleted,
                    FileState::Deleted => FileState::Added,
                    s => s,
                };
                if let Some(op) = c.old_path.take() {
                    c.old_path = Some(std::mem::replace(&mut c.path, op));
                }
            }
            out.sort_by(|a, b| a.path.cmp(&b.path));
            return Ok(out);
        }
        let commit_of = |v: Version| -> Result<Option<Oid>> {
            Ok(match v {
                Version::Commit(id) => Some(id),
                Version::Head => self.git.head_id().ok().map(|id| Oid::from_gix(&id)),
                _ => None,
            })
        };
        match (old, new) {
            (Version::Commit(_) | Version::Head, Version::Commit(_) | Version::Head) => match commit_of(new)? {
                Some(to) => self.diff_commits(commit_of(old)?, to),
                None => Ok(Vec::new()),
            },
            (Version::Head, Version::Index) | (Version::Index, Version::Worktree) => {
                let staged = old == Version::Head;
                let untracked = if staged { Untracked::None } else { Untracked::All };
                let status = self.status(&StatusOptions { untracked, ignored: false }, cancel)?;
                let mut out = Vec::new();
                for e in &status.entries {
                    let state = if staged {
                        e.staged
                    } else if e.untracked {
                        Some(FileState::Added)
                    } else {
                        e.unstaged
                    };
                    let Some(state) = state else { continue };
                    if e.submodule || e.path.ends_with('/') {
                        continue;
                    }
                    let from = e.orig_path.as_deref().unwrap_or(&e.path);
                    let a = self.read_version(from, old)?.map(|x| x.0).unwrap_or_default();
                    let b = self.read_version(&e.path, new)?.map(|x| x.0).unwrap_or_default();
                    let eol = if new == Version::Worktree { EolMode::Normalize } else { EolMode::Exact };
                    let (added, removed, binary) = text_stats(&a, &b, eol);
                    out.push(FileChange {
                        path: e.path.clone(),
                        old_path: e.orig_path.clone(),
                        state,
                        added,
                        removed,
                        binary,
                    });
                }
                Ok(out)
            }
            _ => {
                // a commit against the index or worktree: the union of what changed on the way, compared directly
                let base = commit_of(old)?;
                let mut paths: BTreeSet<String> = BTreeSet::new();
                if let (Some(b), Some(h)) = (base, commit_of(Version::Head)?)
                    && b != h
                {
                    for c in self.diff_commits(Some(b), h)? {
                        paths.extend(c.old_path);
                        paths.insert(c.path);
                    }
                }
                let untracked = if new == Version::Worktree { Untracked::All } else { Untracked::None };
                let status = self.status(&StatusOptions { untracked, ignored: false }, cancel)?;
                for e in status.entries {
                    let counts =
                        e.staged.is_some() || (new == Version::Worktree && (e.unstaged.is_some() || e.untracked));
                    if counts && !e.submodule && !e.path.ends_with('/') {
                        paths.extend(e.orig_path);
                        paths.insert(e.path);
                    }
                }
                let mut out = Vec::new();
                for p in paths {
                    let a = match base {
                        Some(b) => self.read_version(&p, Version::Commit(b))?,
                        None => None,
                    };
                    let b = self.read_version(&p, new)?;
                    let state = match (&a, &b) {
                        (None, None) => continue,
                        (None, Some(_)) => FileState::Added,
                        (Some(_), None) => FileState::Deleted,
                        (Some(x), Some(y)) if x == y => continue,
                        (Some((_, ma)), Some((_, mb))) if (*ma == MODE_LINK) != (*mb == MODE_LINK) => {
                            FileState::TypeChanged
                        }
                        _ => FileState::Modified,
                    };
                    let (ta, tb) = (a.map(|x| x.0).unwrap_or_default(), b.map(|x| x.0).unwrap_or_default());
                    let eol = if new == Version::Worktree { EolMode::Normalize } else { EolMode::Exact };
                    let (added, removed, binary) = text_stats(&ta, &tb, eol);
                    out.push(FileChange { path: p, old_path: None, state, added, removed, binary });
                }
                Ok(out)
            }
        }
    }
}
