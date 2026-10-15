//! Writes, through the `git` program: staging (whole files, hunks, lines), commits, branches, tags, stashes,
//! conflicts, and the network (fetch, pull, push) with streamed progress and cancellation.
//!
//! Partial staging builds the index text it wants (see [`crate::patch`]) and applies the patch from the current
//! index text to it with `git apply --cached --recount`, so git refuses it ([`ErrorKind::Stale`]) when the index
//! changed since it was read. Partial discards write the file directly: `git apply` on the worktree converts line
//! endings both ways under `core.autocrlf`, which would garble a byte-exact patch.

use crate::cli::{Cancel, Git, Output};
use crate::diff::{EolMode, diff};
use crate::error::{Error, ErrorKind, Result};
use crate::oid::Oid;
use crate::patch::{LineSel, Side, apply_selection, file_patch, revert_selection};
use crate::repo::{Blob, MODE_FILE, Repo, RepoState};
use crate::text::Text;

/// `git commit` settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitOptions {
    pub message: String,
    /// Replace the last commit (`--amend`).
    pub amend: bool,
    /// Add a `Signed-off-by` trailer.
    pub signoff: bool,
    /// Allow a commit with no changes (off by default, like git).
    pub allow_empty: bool,
    /// Skip the pre-commit and commit-msg hooks.
    pub no_verify: bool,
}

/// `git stash push` settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StashOptions {
    pub message: Option<String>,
    pub include_untracked: bool,
    /// Leave staged changes in the index too.
    pub keep_index: bool,
    /// Stash only these paths (all when empty).
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FetchOptions {
    /// The remote (the branch's or the default when `None`).
    pub remote: Option<String>,
    /// Every remote (`--all`).
    pub all: bool,
    pub prune: bool,
}

/// How `pull` integrates the upstream.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PullMode {
    /// Only fast-forward; fails when the branches diverged.
    #[default]
    FastForwardOnly,
    Rebase,
    Merge,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushOptions {
    /// Where to push; the branch's push remote, the only remote, or `origin` when `None`.
    pub remote: Option<String>,
    /// The local branch to push (HEAD's when `None`).
    pub branch: Option<String>,
    /// `--force-with-lease`: overwrite the remote only if it is where we last saw it.
    pub force_with_lease: bool,
    /// Also push tags (`--follow-tags`).
    pub tags: bool,
}

/// Which side of a conflict to take.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConflictSide {
    Ours,
    Theirs,
}

/// The staged version a line selection was computed against. When the index no longer holds it, the operation
/// fails with [`ErrorKind::Stale`] instead of applying the line numbers to a different text.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Base {
    /// Do not check (the selection was computed just now from [`Repo::index_blob`]).
    #[default]
    Unchecked,
    /// The file was not in the index.
    Absent,
    /// The index held this blob.
    Blob(Oid),
}

/// Progress lines from git's stderr (`Receiving objects:  45% (450/1000)`), updated in place by `\r`.
pub type Progress<'a> = &'a mut dyn FnMut(&str);

fn reject_option(name: &str) -> Result<()> {
    if name.starts_with('-') || name.is_empty() {
        return Err(Error::new(ErrorKind::NotFound, format!("invalid name: {name:?}")));
    }
    Ok(())
}

impl Repo {
    pub(crate) fn cmd(&self) -> Git {
        Git::new(&self.target())
    }

    fn run(&self, args: &[&str]) -> Result<Output> {
        self.cmd().args(args).run()
    }

    fn run_paths(&self, args: &[&str], paths: &[String]) -> Result<Output> {
        self.cmd().args(args).pathspecs(paths).run()
    }

    /// Stages whole files: modifications, deletions and untracked files (`git add -A`).
    pub fn stage(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        self.run_paths(&["add", "-A"], paths).map(drop)
    }

    /// Stages every change in the worktree.
    pub fn stage_all(&self) -> Result<()> {
        self.run(&["add", "-A"]).map(drop)
    }

    /// Unstages whole files (works before the first commit too).
    pub fn unstage(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        if self.git.head_id().is_err() {
            return self.run_paths(&["rm", "--cached", "-r", "-q", "--ignore-unmatch"], paths).map(drop);
        }
        self.run_paths(&["reset", "-q", "HEAD"], paths).map(drop)
    }

    pub fn unstage_all(&self) -> Result<()> {
        if self.git.head_id().is_err() {
            return self.run(&["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--", "."]).map(drop);
        }
        self.run(&["reset", "-q"]).map(drop)
    }

    /// Throws away unstaged changes to tracked files (back to the index version). Untracked files are left;
    /// see [`Repo::clean`].
    pub fn discard(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        self.run_paths(&["checkout", "-q"], paths).map(drop)
    }

    /// Deletes untracked files and folders.
    pub fn clean(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        self.run_paths(&["clean", "-f", "-d", "-q"], paths).map(drop)
    }

    fn check_base(&self, path: &str, base: Base) -> Result<()> {
        let now = self.index_entry(path)?.map(|(id, _)| id);
        let ok = match base {
            Base::Unchecked => true,
            Base::Absent => now.is_none(),
            Base::Blob(id) => now == Some(id),
        };
        if ok {
            Ok(())
        } else {
            Err(Error::new(ErrorKind::Stale, format!("{path} changed in the index; refresh and retry")))
        }
    }

    /// The worktree text to stage from: the given buffer text, else the file on disk.
    fn new_side(&self, path: &str, new: Option<&Text>, index: Option<&Blob>) -> Result<(Text, u32)> {
        match new {
            Some(t) => {
                let mode = match index {
                    Some(b) => b.mode,
                    None => self.worktree_file(path)?.map_or(MODE_FILE, |(_, m)| m),
                };
                Ok((t.clone(), mode))
            }
            None => self
                .worktree_file(path)?
                .ok_or_else(|| Error::new(ErrorKind::NotFound, format!("{path} does not exist"))),
        }
    }

    /// Stages the selected lines of the diff index -> `new` (an unsaved buffer's text, or the file on disk when
    /// `None`). `sel` is in that diff's line numbers, e.g. from [`crate::diff::diff`] of [`Repo::index_blob`] and the
    /// same text under [`EolMode::Normalize`], or [`LineSel::new_lines`] over the gutter's hunks; `base` is the
    /// index version that diff was computed from.
    pub fn stage_lines(&self, path: &str, new: Option<&Text>, sel: &LineSel, base: Base) -> Result<()> {
        if sel.is_empty() {
            return Ok(());
        }
        self.check_base(path, base)?;
        let index = self.index_blob(path)?;
        let (wt, mode) = self.new_side(path, new, index.as_ref())?;
        let base = index.as_ref().map(|b| b.text.clone()).unwrap_or_default();
        let hunks = diff(&base, &wt, EolMode::Normalize);
        let target = Text::new(apply_selection(&base, &wt, &hunks, sel, EolMode::Normalize));
        let mode = index.as_ref().map_or(mode, |b| b.mode);
        self.apply_cached(path, index.as_ref(), Some(Side { text: &target, mode }))
    }

    /// Stages the changes on new lines `lines` (editor line numbers in `new`, or the file on disk).
    pub fn stage_new_lines(&self, path: &str, new: Option<&Text>, lines: std::ops::Range<u32>) -> Result<()> {
        let index = self.index_blob(path)?;
        let (wt, _) = self.new_side(path, new, index.as_ref())?;
        let base = index.map(|b| b.text).unwrap_or_default();
        let sel = LineSel::new_lines(&diff(&base, &wt, EolMode::Normalize), lines);
        self.stage_lines(path, Some(&wt), &sel, Base::Unchecked)
    }

    /// Unstages the selected lines of the staged diff HEAD -> index (`sel` in that diff's line numbers, `index`
    /// the staged version it was computed from).
    pub fn unstage_lines(&self, path: &str, sel: &LineSel, index: Base) -> Result<()> {
        if sel.is_empty() {
            return Ok(());
        }
        self.check_base(path, index)?;
        let Some(index) = self.index_blob(path)? else {
            return Err(Error::new(ErrorKind::Stale, format!("{path} is not staged")));
        };
        let head = self.head_blob(path)?;
        let base = head.as_ref().map(|b| b.text.clone()).unwrap_or_default();
        let hunks = diff(&base, &index.text, EolMode::Exact);
        if head.is_none() && sel.covers(&hunks) {
            // every line of a new file: it is not staged at all any more
            return self.run_paths(&["rm", "--cached", "-q", "-f"], &[path.to_string()]).map(drop);
        }
        let target = Text::new(revert_selection(&base, &index.text, &hunks, sel, EolMode::Exact));
        self.apply_cached(path, Some(&index), Some(Side { text: &target, mode: index.mode }))
    }

    /// Throws away the selected lines of the unstaged diff index -> worktree file, writing the file.
    /// Returns the new file content (for reloading an open buffer).
    pub fn discard_lines(&self, path: &str, sel: &LineSel, base: Base) -> Result<Text> {
        self.check_base(path, base)?;
        let (wt, _) = self
            .worktree_file(path)?
            .ok_or_else(|| Error::new(ErrorKind::NotFound, format!("{path} does not exist")))?;
        if sel.is_empty() {
            return Ok(wt);
        }
        let base = self.index_blob(path)?.map(|b| b.text).unwrap_or_default();
        let hunks = diff(&base, &wt, EolMode::Normalize);
        let target = revert_selection(&base, &wt, &hunks, sel, EolMode::Normalize);
        std::fs::write(self.abs_path(path), &target)?;
        Ok(Text::new(target))
    }

    /// Makes `new` the staged version of `path` (`None` removes it from the index) through `git apply --cached`.
    fn apply_cached(&self, path: &str, current: Option<&Blob>, new: Option<Side>) -> Result<()> {
        let old = current.map(|b| Side { text: &b.text, mode: b.mode });
        let Some(patch) = file_patch(path, old, new, 3) else { return Ok(()) };
        self.cmd()
            .args(["apply", "--cached", "--recount", "--whitespace=nowarn", "-"])
            .stdin(patch)
            .run()
            .map(drop)
            .map_err(|e| match e.kind {
                ErrorKind::Git(_) => Error::new(ErrorKind::Stale, e.message),
                _ => e,
            })
    }

    /// Commits the index; returns the new commit. A failing hook's output is the error message.
    pub fn commit(&self, opts: &CommitOptions) -> Result<Oid> {
        let mut args = vec!["commit", "-F", "-"];
        if opts.amend {
            args.push("--amend");
        }
        if opts.signoff {
            args.push("--signoff");
        }
        if opts.allow_empty {
            args.push("--allow-empty");
        }
        if opts.no_verify {
            args.push("--no-verify");
        }
        self.cmd().args(&args).stdin(opts.message.clone().into_bytes()).run()?;
        self.resolve("HEAD")
    }

    /// Switches to a local branch (or creates a tracking branch for a remote one of that name, as `git switch`).
    pub fn switch(&self, branch: &str) -> Result<()> {
        reject_option(branch)?;
        self.run(&["switch", "-q", branch]).map(drop)
    }

    /// Checks out a commit without a branch.
    pub fn switch_detach(&self, rev: &str) -> Result<()> {
        reject_option(rev)?;
        self.run(&["switch", "-q", "--detach", rev]).map(drop)
    }

    /// Creates `name` at `start` (HEAD when `None`), and switches to it if `switch`.
    pub fn create_branch(&self, name: &str, start: Option<&str>, switch: bool) -> Result<()> {
        reject_option(name)?;
        let mut args = if switch { vec!["switch", "-q", "-c", name] } else { vec!["branch", name] };
        if let Some(s) = start {
            reject_option(s)?;
            args.push(s);
        }
        self.run(&args).map(drop)
    }

    /// Deletes a local branch; without `force` git refuses when it is not merged.
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        reject_option(name)?;
        self.run(&["branch", if force { "-D" } else { "-d" }, name]).map(drop)
    }

    pub fn rename_branch(&self, old: &str, new: &str) -> Result<()> {
        reject_option(old)?;
        reject_option(new)?;
        self.run(&["branch", "-m", old, new]).map(drop)
    }

    /// Creates a tag at `rev` (HEAD when `None`); annotated when there is a message.
    pub fn create_tag(&self, name: &str, rev: Option<&str>, message: Option<&str>) -> Result<()> {
        reject_option(name)?;
        let mut cmd = self.cmd().arg("tag");
        if let Some(m) = message {
            cmd = cmd.args(["-a", "-F", "-"]).stdin(m.as_bytes().to_vec());
        }
        cmd = cmd.arg(name);
        if let Some(r) = rev {
            reject_option(r)?;
            cmd = cmd.arg(r);
        }
        cmd.run().map(drop)
    }

    pub fn delete_tag(&self, name: &str) -> Result<()> {
        reject_option(name)?;
        self.run(&["tag", "-d", name]).map(drop)
    }

    pub fn stash_push(&self, opts: &StashOptions) -> Result<()> {
        let mut cmd = self.cmd().args(["stash", "push", "-q"]);
        if let Some(m) = &opts.message {
            cmd = cmd.args(["-m", m]);
        }
        if opts.include_untracked {
            cmd = cmd.arg("--include-untracked");
        }
        if opts.keep_index {
            cmd = cmd.arg("--keep-index");
        }
        if !opts.paths.is_empty() {
            cmd = cmd.pathspecs(&opts.paths);
        }
        cmd.run().map(drop)
    }

    /// Applies `stash@{index}` and drops it (kept when it conflicts, as git does).
    pub fn stash_pop(&self, index: usize) -> Result<()> {
        self.run(&["stash", "pop", "-q", &format!("stash@{{{index}}}")]).map(drop)
    }

    pub fn stash_apply(&self, index: usize) -> Result<()> {
        self.run(&["stash", "apply", "-q", &format!("stash@{{{index}}}")]).map(drop)
    }

    pub fn stash_drop(&self, index: usize) -> Result<()> {
        self.run(&["stash", "drop", "-q", &format!("stash@{{{index}}}")]).map(drop)
    }

    /// Resolves a conflict by taking one side's version (deleting the file when that side deleted it), and marks
    /// it resolved when `mark_resolved`.
    pub fn take_side(&self, path: &str, side: ConflictSide, mark_resolved: bool) -> Result<()> {
        let versions = self.conflict_versions(path)?;
        let present = match side {
            ConflictSide::Ours => versions[1].is_some(),
            ConflictSide::Theirs => versions[2].is_some(),
        };
        let p = [path.to_string()];
        if !present {
            return self.run_paths(&["rm", "-q", "--ignore-unmatch"], &p).map(drop);
        }
        let flag = if side == ConflictSide::Ours { "--ours" } else { "--theirs" };
        self.run_paths(&["checkout", flag], &p)?;
        if mark_resolved { self.mark_resolved(&p) } else { Ok(()) }
    }

    /// Marks conflicted files resolved with their worktree content (a deleted file stays deleted).
    pub fn mark_resolved(&self, paths: &[String]) -> Result<()> {
        self.stage(paths)
    }

    /// Aborts the merge, rebase, cherry-pick or revert in progress.
    pub fn abort_operation(&self) -> Result<()> {
        let cmd = match self.state() {
            RepoState::Merging => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::ApplyingPatches => "am",
            RepoState::Bisecting => return self.run(&["bisect", "reset"]).map(drop),
            RepoState::Clean => return Ok(()),
        };
        self.run(&[cmd, "--abort"]).map(drop)
    }

    /// Continues the merge, rebase, cherry-pick or revert in progress once conflicts are resolved.
    pub fn continue_operation(&self) -> Result<()> {
        let cmd = match self.state() {
            RepoState::Merging => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::ApplyingPatches => "am",
            RepoState::Bisecting | RepoState::Clean => return Ok(()),
        };
        self.run(&[cmd, "--continue"]).map(drop)
    }

    /// `git fetch`, streaming progress lines; returns git's summary.
    pub fn fetch(&self, opts: &FetchOptions, cancel: &Cancel, progress: Progress) -> Result<String> {
        let mut cmd = self.cmd().args(["fetch", "--progress"]);
        if opts.prune {
            cmd = cmd.arg("--prune");
        }
        if opts.all {
            cmd = cmd.arg("--all");
        } else if let Some(r) = &opts.remote {
            reject_option(r)?;
            cmd = cmd.arg(r);
        }
        Ok(cmd.stream(cancel, progress, None)?.text())
    }

    /// `git pull` (fast-forward only unless asked otherwise), streaming progress.
    pub fn pull(&self, mode: PullMode, cancel: &Cancel, progress: Progress) -> Result<String> {
        let flag = match mode {
            PullMode::FastForwardOnly => "--ff-only",
            PullMode::Rebase => "--rebase",
            PullMode::Merge => "--no-rebase",
        };
        Ok(self.cmd().args(["pull", "--progress", flag]).stream(cancel, progress, None)?.text())
    }

    /// `git push`, setting the upstream when the branch has none yet.
    pub fn push(&self, opts: &PushOptions, cancel: &Cancel, progress: Progress) -> Result<String> {
        let branch = match &opts.branch {
            Some(b) => b.clone(),
            None => self
                .head()?
                .branch
                .ok_or_else(|| Error::new(ErrorKind::NotFound, "HEAD is detached: no branch to push"))?,
        };
        reject_option(&branch)?;
        let full: gix::refs::FullName = format!("refs/heads/{branch}")
            .try_into()
            .map_err(|_| Error::new(ErrorKind::NotFound, format!("invalid branch name {branch}")))?;
        // an upstream is `branch.<name>.merge` (push direction would answer even without one)
        let has_upstream = self.git.branch_remote_ref_name(full.as_ref(), gix::remote::Direction::Fetch).is_some();
        let remote = match &opts.remote {
            Some(r) => r.clone(),
            None => self.push_remote(&full)?,
        };
        reject_option(&remote)?;
        let mut cmd = self.cmd().args(["push", "--progress", "--porcelain"]);
        if opts.force_with_lease {
            cmd = cmd.arg("--force-with-lease");
        }
        if opts.tags {
            cmd = cmd.arg("--follow-tags");
        }
        if !has_upstream {
            cmd = cmd.arg("--set-upstream");
        }
        Ok(cmd.arg(&remote).arg(&branch).stream(cancel, progress, None)?.text())
    }

    /// The remote `branch` pushes to: its configured one, the only remote, or `origin`.
    fn push_remote(&self, branch: &gix::refs::FullName) -> Result<String> {
        if let Some(name) = self.git.branch_remote_name(branch.as_ref().shorten(), gix::remote::Direction::Push) {
            return Ok(name.as_bstr().to_string());
        }
        let names = self.git.remote_names();
        if names.len() == 1 {
            return Ok(names.iter().next().unwrap().to_string());
        }
        if names.iter().any(|n| n.as_slice() == b"origin") {
            return Ok("origin".into());
        }
        Err(Error::new(ErrorKind::NotFound, "no remote to push to"))
    }
}
