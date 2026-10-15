//! [`Repo`]: one discovered repository, read in-process with gix. Discovery, HEAD and its upstream, blobs for the
//! gutter, and the cheap file stamps that tell when HEAD, the index or the refs moved.

use crate::cli::Target;
use crate::error::{Error, ErrorKind, Result};
use crate::oid::Oid;
use crate::text::Text;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// What kind of checkout a repository is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RepoKind {
    /// An ordinary repository (or an old-style submodule with its own `.git` folder).
    Main,
    /// A linked worktree (`git worktree add`).
    Worktree,
    /// A submodule checkout whose git dir lives in the superproject's `.git/modules`.
    Submodule,
}

/// Where a repository lives. Its `workdir` identifies it in [`crate::service`] requests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoInfo {
    /// The worktree root (canonical).
    pub workdir: PathBuf,
    /// This worktree's git dir (`.git`, or `.git/worktrees/<name>` for a linked worktree).
    pub git_dir: PathBuf,
    /// The shared git dir holding objects and refs.
    pub common_dir: PathBuf,
    pub kind: RepoKind,
}

/// An operation git is in the middle of.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum RepoState {
    #[default]
    Clean,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
    Bisecting,
    ApplyingPatches,
}

/// A branch's remote-tracking counterpart and how far apart they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// Short name, as `origin/main`.
    pub name: String,
    /// Commits on HEAD not on the upstream (capped at [`AHEAD_BEHIND_CAP`]).
    pub ahead: u32,
    /// Commits on the upstream not on HEAD (capped).
    pub behind: u32,
    /// Configured, but the remote-tracking branch does not exist (deleted upstream, or never fetched).
    pub gone: bool,
}

/// Counting ahead/behind stops here (shown as "10000+").
pub const AHEAD_BEHIND_CAP: u32 = 10_000;

/// Where HEAD points.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Head {
    /// The checked-out branch's short name (also set for an unborn branch); `None` when detached.
    pub branch: Option<String>,
    /// The commit HEAD points to; `None` on an unborn branch (no commits yet).
    pub oid: Option<Oid>,
    pub detached: bool,
    pub upstream: Option<Upstream>,
    pub state: RepoState,
    /// First line of HEAD's commit message.
    pub summary: Option<String>,
}

impl Head {
    pub fn is_unborn(&self) -> bool {
        self.oid.is_none()
    }

    /// What a status bar shows: the branch, or the short commit id when detached.
    pub fn label(&self) -> String {
        match (&self.branch, &self.oid) {
            (Some(b), _) => b.clone(),
            (None, Some(id)) => id.short(),
            (None, None) => "HEAD".into(),
        }
    }
}

/// A file's content as stored by git, with its mode (`0o100644`, `0o100755`, `0o120000` for a symlink).
#[derive(Clone, Debug, PartialEq)]
pub struct Blob {
    pub id: Oid,
    pub mode: u32,
    pub text: Text,
}

/// Mode of a regular file.
pub const MODE_FILE: u32 = 0o100644;
/// Mode of an executable file.
pub const MODE_EXEC: u32 = 0o100755;
/// Mode of a symlink.
pub const MODE_LINK: u32 = 0o120000;
/// Mode of a submodule entry.
pub const MODE_SUBMODULE: u32 = 0o160000;

/// A discovered repository. Cheap to clone; `Send` but not `Sync` (each thread keeps its own clone).
/// It reads the config once when opened: after [`Changes::config`], open it again ([`Repo::open`]).
#[derive(Clone)]
pub struct Repo {
    pub(crate) git: gix::Repository,
    info: Arc<RepoInfo>,
}

impl std::fmt::Debug for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Repo({})", self.info.workdir.display())
    }
}

/// `fs::canonicalize` without Windows' verbatim prefix. Windows returns `\\?\C:\repo`, which the `git` program rejects
/// as `GIT_DIR` ("not a git repository") and which never equals the plain `C:\repo` other code compares it with, so every
/// path shrew-git and its callers compare or hand to git goes through this.
pub fn canonicalize(p: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(p).map(simplify_path)
}

/// Drops a Windows verbatim prefix: `\\?\C:\a` becomes `C:\a` and `\\?\UNC\host\share\a` becomes `\\host\share\a`. Anything
/// else (every Unix path, `\\?\Volume{..}` paths that have no plain form) is returned unchanged.
pub fn simplify_path(p: PathBuf) -> PathBuf {
    let Some(s) = p.to_str() else { return p };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\') {
            return PathBuf::from(rest);
        }
    }
    p
}

fn canonical(p: &Path) -> PathBuf {
    canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

impl Repo {
    /// Finds the repository containing `path` (a file or folder), walking up like git does and honouring
    /// `GIT_DIR`. `Ok(None)` when there is none, the repository is bare, or `path` is inside the `.git` folder.
    pub fn discover(path: &Path) -> Result<Option<Repo>> {
        let start = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        let start = if start.as_os_str().is_empty() { Path::new(".") } else { start };
        let ts = match gix::ThreadSafeRepository::discover_with_environment_overrides(start) {
            Ok(ts) => ts,
            Err(e) => {
                log::debug!("no git repository at {}: {e}", start.display());
                return Ok(None);
            }
        };
        let repo = Repo::from_gix(ts.to_thread_local())?;
        let Some(repo) = repo else { return Ok(None) };
        if repo.rel_path(path).is_none() && canonical(path) != repo.info.workdir {
            return Ok(None);
        }
        Ok(Some(repo))
    }

    /// Opens a repository from its info (as found by [`Repo::discover`]).
    pub fn open(info: &RepoInfo) -> Result<Repo> {
        let mut git = gix::open(&info.git_dir)?;
        git.object_cache_size_if_unset(8 * 1024 * 1024);
        Ok(Repo { git, info: Arc::new(info.clone()) })
    }

    fn from_gix(mut git: gix::Repository) -> Result<Option<Repo>> {
        let Some(workdir) = git.workdir().map(canonical) else { return Ok(None) };
        let kind = match git.kind() {
            gix::repository::Kind::Common => RepoKind::Main,
            gix::repository::Kind::LinkedWorkTree => RepoKind::Worktree,
            gix::repository::Kind::Submodule => RepoKind::Submodule,
        };
        let info =
            RepoInfo { workdir, git_dir: canonical(git.git_dir()), common_dir: canonical(git.common_dir()), kind };
        git.object_cache_size_if_unset(8 * 1024 * 1024);
        Ok(Some(Repo { git, info: Arc::new(info) }))
    }

    pub fn info(&self) -> &RepoInfo {
        &self.info
    }

    pub fn workdir(&self) -> &Path {
        &self.info.workdir
    }

    pub(crate) fn target(&self) -> Target {
        Target { git_dir: self.info.git_dir.clone(), workdir: self.info.workdir.clone() }
    }

    /// `path` (absolute, or relative to the current directory) as a repository path (`src/main.rs`, always `/`),
    /// or `None` when it is outside the worktree or inside `.git`.
    pub fn rel_path(&self, path: &Path) -> Option<String> {
        let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().ok()?.join(path) };
        let abs = simplify_path(abs);
        let rel = match abs.strip_prefix(&self.info.workdir) {
            Ok(r) => r.to_path_buf(),
            Err(_) => {
                // symlinked folders: compare canonical paths (the file itself may not exist yet)
                let (dir, name) = (abs.parent()?, abs.file_name());
                let dir = canonical(dir);
                let full = match name {
                    Some(n) => dir.join(n),
                    None => dir,
                };
                full.strip_prefix(&self.info.workdir).ok()?.to_path_buf()
            }
        };
        let mut parts = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_str()?.to_string()),
                Component::CurDir => {}
                _ => return None,
            }
        }
        if parts.is_empty() || parts[0] == ".git" {
            return None;
        }
        Some(parts.join("/"))
    }

    /// The absolute path of repository path `rel`.
    pub fn abs_path(&self, rel: &str) -> PathBuf {
        let mut p = self.info.workdir.clone();
        for part in rel.split('/').filter(|s| !s.is_empty()) {
            p.push(part);
        }
        p
    }

    /// The state of HEAD, with its upstream counted (a walk of the commits in between).
    pub fn head(&self) -> Result<Head> {
        let git = &self.git;
        let head = git.head()?;
        let mut out = Head { state: self.state(), ..Head::default() };
        let referent = head.referent_name().map(|n| n.to_owned());
        out.detached = head.is_detached();
        out.oid = head.id().map(|id| Oid::from_gix(&id));
        if let Some(name) = &referent {
            out.branch = Some(name.shorten().to_string());
        }
        if let Some(id) = out.oid {
            out.summary =
                git.find_commit(id.to_gix()).ok().and_then(|c| c.message_raw().ok().map(crate::log::summary_of));
        }
        if let Some(name) = referent.filter(|_| !out.detached) {
            out.upstream = self.upstream_of(name.as_ref(), out.oid);
        }
        Ok(out)
    }

    pub(crate) fn upstream_of(&self, branch: &gix::refs::FullNameRef, local: Option<Oid>) -> Option<Upstream> {
        let git = &self.git;
        let tracking = git.branch_remote_tracking_ref_name(branch, gix::remote::Direction::Fetch)?.ok()?;
        let name = tracking.as_ref().shorten().to_string();
        let remote_id = git
            .try_find_reference(tracking.as_ref())
            .ok()
            .flatten()
            .and_then(|mut r| r.peel_to_id().ok().map(|id| Oid::from_gix(&id)));
        let Some(remote_id) = remote_id else {
            return Some(Upstream { name, ahead: 0, behind: 0, gone: true });
        };
        let (ahead, behind) = match local {
            Some(local) => (self.count_between(local, remote_id), self.count_between(remote_id, local)),
            None => (0, 0),
        };
        Some(Upstream { name, ahead, behind, gone: false })
    }

    /// Commits reachable from `from` but not from `hidden`, capped.
    pub(crate) fn count_between(&self, from: Oid, hidden: Oid) -> u32 {
        if from == hidden {
            return 0;
        }
        match self.git.rev_walk([from.to_gix()]).with_hidden([hidden.to_gix()]).all() {
            Ok(walk) => walk.take(AHEAD_BEHIND_CAP as usize).filter(|r| r.is_ok()).count() as u32,
            Err(_) => 0,
        }
    }

    /// The operation in progress, if any.
    pub fn state(&self) -> RepoState {
        use gix::state::InProgress::*;
        match self.git.state() {
            None => RepoState::Clean,
            Some(Merge) => RepoState::Merging,
            Some(Rebase | RebaseInteractive | ApplyMailboxRebase) => RepoState::Rebasing,
            Some(CherryPick | CherryPickSequence) => RepoState::CherryPicking,
            Some(Revert | RevertSequence) => RepoState::Reverting,
            Some(Bisect) => RepoState::Bisecting,
            Some(ApplyMailbox) => RepoState::ApplyingPatches,
        }
    }

    fn blob(&self, id: gix::ObjectId, mode: u32) -> Result<Blob> {
        let blob = self.git.find_blob(id)?;
        Ok(Blob { id: Oid::from_gix(&id), mode, text: Text::new(blob.data.clone()) })
    }

    /// The staged version of `rel` (stage 0), `None` when it is not in the index or is conflicted.
    pub fn index_blob(&self, rel: &str) -> Result<Option<Blob>> {
        let Some((id, mode)) = self.index_entry(rel)? else { return Ok(None) };
        if mode == MODE_SUBMODULE {
            return Ok(None);
        }
        self.blob(id.to_gix(), mode).map(Some)
    }

    /// The id and mode of `rel`'s stage-0 index entry (cheap: no blob read).
    pub fn index_entry(&self, rel: &str) -> Result<Option<(Oid, u32)>> {
        let index = self.git.index_or_empty()?;
        let entry = index.entry_by_path_and_stage(rel.into(), gix::index::entry::Stage::Unconflicted);
        Ok(entry.map(|e| (Oid::from_gix(&e.id), e.mode.bits())))
    }

    /// `rel` as committed in HEAD.
    pub fn head_blob(&self, rel: &str) -> Result<Option<Blob>> {
        match self.git.head_id() {
            Ok(id) => self.blob_at(Oid::from_gix(&id), rel),
            Err(_) => Ok(None), // unborn
        }
    }

    /// The id and mode of `rel` in HEAD's tree (cheap: no blob read).
    pub fn head_entry(&self, rel: &str) -> Result<Option<(Oid, u32)>> {
        match self.git.head_id() {
            Ok(id) => self.tree_entry(Oid::from_gix(&id), rel),
            Err(_) => Ok(None),
        }
    }

    /// The id and mode of `rel` in commit `commit`'s tree.
    pub fn tree_entry(&self, commit: Oid, rel: &str) -> Result<Option<(Oid, u32)>> {
        let tree = self.git.find_commit(commit.to_gix())?.tree()?;
        let entry = tree.lookup_entry_by_path(rel_to_path(rel))?;
        Ok(entry.map(|e| (Oid::from_gix(e.oid()), e.mode().value() as u32)))
    }

    /// `rel` in commit `commit`.
    pub fn blob_at(&self, commit: Oid, rel: &str) -> Result<Option<Blob>> {
        match self.tree_entry(commit, rel)? {
            Some((id, mode)) if mode != MODE_SUBMODULE && mode != 0o040000 => self.blob(id.to_gix(), mode).map(Some),
            _ => Ok(None),
        }
    }

    /// A blob by id.
    pub fn blob_by_id(&self, id: Oid) -> Result<Text> {
        Ok(self.blob(id.to_gix(), MODE_FILE)?.text)
    }

    /// The worktree file `rel` and its mode as git would record it; `None` if it does not exist.
    pub fn worktree_file(&self, rel: &str) -> Result<Option<(Text, u32)>> {
        let path = self.abs_path(rel);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&path)?;
            return Ok(Some((Text::new(target.to_string_lossy().into_owned().into_bytes()), MODE_LINK)));
        }
        if meta.is_dir() {
            return Ok(None);
        }
        Ok(Some((Text::new(std::fs::read(&path)?), file_mode(&meta))))
    }

    /// Whether `rel` has conflict stages in the index.
    pub fn is_conflicted(&self, rel: &str) -> Result<bool> {
        use gix::index::entry::Stage;
        let index = self.git.index_or_empty()?;
        Ok([Stage::Base, Stage::Ours, Stage::Theirs]
            .into_iter()
            .any(|s| index.entry_by_path_and_stage(rel.into(), s).is_some()))
    }

    /// The three sides of a conflicted file: `[base, ours, theirs]` (index stages 1..3), `None` where absent.
    pub fn conflict_versions(&self, rel: &str) -> Result<[Option<Blob>; 3]> {
        use gix::index::entry::Stage;
        let index = self.git.index_or_empty()?;
        let mut out: [Option<Blob>; 3] = [None, None, None];
        for (i, stage) in [Stage::Base, Stage::Ours, Stage::Theirs].into_iter().enumerate() {
            if let Some(e) = index.entry_by_path_and_stage(rel.into(), stage) {
                out[i] = Some(self.blob(e.id, e.mode.bits())?);
            }
        }
        Ok(out)
    }

    /// Resolves a revision (`HEAD~2`, a branch, a tag, an id prefix) to a commit.
    pub fn resolve(&self, rev: &str) -> Result<Oid> {
        let id = self.git.rev_parse_single(rev).map_err(|e| {
            log::debug!("rev-parse {rev}: {e}");
            Error::new(ErrorKind::NotFound, format!("unknown revision {rev}"))
        })?;
        let commit = id.object()?.peel_to_commit().map_err(Error::from)?;
        Ok(Oid::from_gix(&commit.id))
    }

    /// File stamps of HEAD, the index and the refs: compare two with [`Stamp::changes`].
    pub fn stamp(&self) -> Stamp {
        Stamp::read(&self.info)
    }
}

/// Git's mode for a worktree file.
fn file_mode(meta: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return MODE_EXEC;
        }
    }
    let _ = meta;
    MODE_FILE
}

pub(crate) fn rel_to_path(rel: &str) -> PathBuf {
    rel.split('/').collect()
}

/// mtime and size of a file or folder, `None` when missing.
type FileStamp = Option<(SystemTime, u64)>;

fn file_stamp(p: &Path) -> FileStamp {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// What moved between two [`Stamp`]s.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// HEAD moved: commit, checkout, reset, merge or rebase state.
    pub head: bool,
    /// The index changed: staging, or anything that rewrote it.
    pub index: bool,
    /// Branches, tags, remote-tracking refs or stashes changed.
    pub refs: bool,
    /// The repository's config changed (upstreams, remotes): an open [`Repo`] must be opened again to see it.
    pub config: bool,
}

impl Changes {
    pub fn any(&self) -> bool {
        self.head || self.index || self.refs || self.config
    }
}

/// A handful of `stat` calls that tell whether git state changed (no file is read but HEAD, a few bytes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    head: [FileStamp; 6],
    index: FileStamp,
    refs: [FileStamp; 6],
    config: [FileStamp; 2],
}

impl Stamp {
    pub(crate) fn read(info: &RepoInfo) -> Stamp {
        let (g, c) = (&info.git_dir, &info.common_dir);
        // the ref HEAD points to, so a commit on the current branch shows even without reflogs
        let branch_ref = std::fs::read_to_string(g.join("HEAD"))
            .ok()
            .and_then(|s| s.strip_prefix("ref: ").map(|r| c.join(r.trim())));
        Stamp {
            head: [
                file_stamp(&g.join("HEAD")),
                branch_ref.and_then(|p| file_stamp(&p)),
                file_stamp(&g.join("logs/HEAD")),
                file_stamp(&g.join("MERGE_HEAD")),
                file_stamp(&g.join("rebase-merge")).or_else(|| file_stamp(&g.join("rebase-apply"))),
                file_stamp(&g.join("CHERRY_PICK_HEAD")).or_else(|| file_stamp(&g.join("REVERT_HEAD"))),
            ],
            index: file_stamp(&g.join("index")),
            refs: [
                file_stamp(&c.join("packed-refs")),
                file_stamp(&c.join("refs/heads")),
                file_stamp(&c.join("refs/tags")),
                file_stamp(&c.join("refs/stash")),
                file_stamp(&g.join("FETCH_HEAD")),
                file_stamp(&c.join("refs/remotes")),
            ],
            config: [file_stamp(&c.join("config")), file_stamp(&g.join("config.worktree"))],
        }
    }

    /// What changed since `older`.
    pub fn changes(&self, older: &Stamp) -> Changes {
        Changes {
            head: self.head != older.head,
            index: self.index != older.index,
            refs: self.refs != older.refs,
            config: self.config != older.config,
        }
    }
}

#[cfg(test)]
mod path_tests {
    use super::simplify_path;
    use std::path::PathBuf;

    fn s(p: &str) -> String {
        simplify_path(PathBuf::from(p)).display().to_string()
    }

    #[test]
    fn windows_verbatim_prefixes_are_dropped() {
        assert_eq!(s(r"\\?\C:\Users\me\repo\.git"), r"C:\Users\me\repo\.git");
        assert_eq!(s(r"\\?\d:"), "d:");
        assert_eq!(s(r"\\?\UNC\host\share\repo"), r"\\host\share\repo");
    }

    #[test]
    fn everything_else_is_left_alone() {
        for p in ["/home/me/repo", r"C:\Users\me", r"\\host\share\x", r"\\?\Volume{1234}\x", r"\\?\1:\x", "", "."] {
            assert_eq!(s(p), p);
        }
    }
}
