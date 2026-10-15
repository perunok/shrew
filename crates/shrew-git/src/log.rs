//! History reads: the commit log (paged, topological, with graph rows and ref labels), commit details, file
//! history following renames, and the lists of branches, tags, remotes and stashes.

use crate::cli::Cancel;
use crate::diff::{EolMode, diff, stats};
use crate::error::{Error, ErrorKind, Result};
use crate::graph::{GraphBuilder, GraphRow};
use crate::oid::Oid;
use crate::repo::Repo;
use crate::text::Text;
use gix::bstr::{BStr, ByteSlice};
use std::collections::{BinaryHeap, HashMap, HashSet};

/// Who and when.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Signature {
    pub name: String,
    pub email: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
    /// The author's UTC offset in seconds (east positive).
    pub offset: i32,
}

/// A commit as a log row shows it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CommitInfo {
    pub id: Oid,
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    /// First line of the message.
    pub summary: String,
}

/// What kind of ref a label names.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RefKind {
    /// A detached `HEAD`.
    Head,
    Branch,
    RemoteBranch,
    Tag,
    Stash,
}

/// A ref pointing at a commit, for decorating log rows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RefLabel {
    /// Short name (`main`, `origin/main`, `v1.0`, `HEAD`).
    pub name: String,
    pub kind: RefKind,
    /// The branch HEAD is on.
    pub is_head: bool,
}

/// Which commits [`Repo::log`] lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogQuery {
    /// Revisions to start from (branches, tags, ids); empty means HEAD.
    pub revs: Vec<String>,
    /// Start from every branch, remote branch and tag too (Sublime Merge's full graph).
    pub all: bool,
    /// Follow only first parents (`--first-parent`).
    pub first_parent: bool,
    /// Rows to skip (paging); the graph is still laid out through them so lanes line up across pages.
    pub skip: usize,
    pub limit: usize,
    /// Compute [`LogRow::graph`].
    pub graph: bool,
}

impl Default for LogQuery {
    fn default() -> Self {
        LogQuery { revs: Vec::new(), all: false, first_parent: false, skip: 0, limit: 200, graph: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRow {
    pub commit: CommitInfo,
    pub graph: Option<GraphRow>,
    pub refs: Vec<RefLabel>,
}

/// One page of history.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LogPage {
    pub rows: Vec<LogRow>,
    /// More commits follow (ask again with `skip += rows.len()`).
    pub more: bool,
}

/// A commit that changed a file, and the file's path in it (it differs before a rename).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRow {
    pub commit: CommitInfo,
    pub path: String,
}

/// One page of a file's history.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct HistoryPage {
    pub rows: Vec<HistoryRow>,
    pub more: bool,
}

/// A changed file in a commit or between two trees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// The source of a rename or copy.
    pub old_path: Option<String>,
    pub state: crate::status::FileState,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
}

/// Everything about one commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetails {
    pub info: CommitInfo,
    /// The whole message.
    pub message: String,
    /// Changes against the first parent (against nothing for a root commit).
    pub files: Vec<FileChange>,
    pub refs: Vec<RefLabel>,
}

/// A branch, local or remote-tracking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// Short name (`main`, `origin/main`).
    pub name: String,
    pub remote: bool,
    pub tip: Oid,
    pub is_head: bool,
    /// The upstream of a local branch (`origin/main`).
    pub upstream: Option<String>,
    /// Commit time of the tip (sort by recency).
    pub time: i64,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// The commit it points to.
    pub target: Oid,
    pub annotated: bool,
    /// An annotated tag's message.
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: Option<String>,
    pub push_url: Option<String>,
}

/// One `stash@{index}` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stash {
    pub index: usize,
    pub id: Oid,
    pub message: String,
    pub time: i64,
}

/// Branches, tags, remotes and stashes at once.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Refs {
    pub branches: Vec<Branch>,
    pub remote_branches: Vec<Branch>,
    pub tags: Vec<Tag>,
    pub remotes: Vec<Remote>,
    pub stashes: Vec<Stash>,
}

/// The first line of a message, trimmed.
pub(crate) fn summary_of(message: &BStr) -> String {
    let s = message.to_str_lossy();
    s.trim_start().lines().next().unwrap_or("").trim_end().to_string()
}

fn signature(s: gix::actor::SignatureRef<'_>) -> Signature {
    let s = s.trim();
    let t = s.time().unwrap_or_default();
    Signature { name: s.name.to_string(), email: s.email.to_string(), time: t.seconds, offset: t.offset }
}

fn cancelled() -> Error {
    Error::new(ErrorKind::Cancelled, "cancelled")
}

impl Repo {
    fn commit_info(&self, id: gix::ObjectId) -> Result<(CommitInfo, String)> {
        let commit = self.git.find_commit(id)?;
        let c = commit.decode()?;
        let parents = c.parents().map(|p| Oid::from_gix(&p)).collect();
        let info = CommitInfo {
            id: Oid::from_gix(&id),
            parents,
            author: c.author().map(signature).unwrap_or_default(),
            committer: c.committer().map(signature).unwrap_or_default(),
            summary: summary_of(c.message),
        };
        Ok((info, c.message.to_str_lossy().into_owned()))
    }

    /// Ref labels by the commit they point at.
    pub fn decorations(&self) -> Result<HashMap<Oid, Vec<RefLabel>>> {
        let mut out: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
        let head = self.git.head()?;
        let head_branch = head.referent_name().map(|n| n.as_bstr().to_string());
        if head.is_detached()
            && let Some(id) = head.id()
        {
            out.entry(Oid::from_gix(&id)).or_default().push(RefLabel {
                name: "HEAD".into(),
                kind: RefKind::Head,
                is_head: true,
            });
        }
        let refs = self.git.references()?;
        for r in refs.all()? {
            let Ok(mut r) = r else { continue };
            let full = r.name().as_bstr().to_string();
            let kind = if full.starts_with("refs/heads/") {
                RefKind::Branch
            } else if full.starts_with("refs/remotes/") {
                if full.ends_with("/HEAD") {
                    continue;
                }
                RefKind::RemoteBranch
            } else if full.starts_with("refs/tags/") {
                RefKind::Tag
            } else if full == "refs/stash" {
                RefKind::Stash
            } else {
                continue;
            };
            let Ok(id) = r.peel_to_id() else { continue };
            let name = r.name().shorten().to_string();
            let is_head = head_branch.as_deref() == Some(full.as_str());
            out.entry(Oid::from_gix(&id)).or_default().push(RefLabel { name, kind, is_head });
        }
        for labels in out.values_mut() {
            labels.sort_by(|a, b| (!a.is_head, a.kind, &a.name).cmp(&(!b.is_head, b.kind, &b.name)));
        }
        Ok(out)
    }

    fn tips(&self, q: &LogQuery) -> Result<Vec<gix::ObjectId>> {
        let mut tips = Vec::new();
        if q.revs.is_empty()
            && let Ok(id) = self.git.head_id()
        {
            tips.push(id.detach());
        }
        for rev in &q.revs {
            tips.push(self.resolve(rev)?.to_gix());
        }
        if q.all {
            for r in self.git.references()?.all()? {
                let Ok(mut r) = r else { continue };
                let name = r.name().as_bstr().to_string();
                if !(name.starts_with("refs/heads/")
                    || name.starts_with("refs/remotes/")
                    || name.starts_with("refs/tags/"))
                {
                    continue;
                }
                if let Ok(c) = r.peel_to_commit() {
                    tips.push(c.id);
                }
            }
        }
        Ok(tips)
    }

    /// A page of history, newest first. With a commit-graph file (git writes one on `gc`) the order is
    /// topological like `git log --graph`, computed incrementally; without one a full topological sort would
    /// read every commit, so the walk is by commit date instead and stays lazy.
    pub fn log(&self, q: &LogQuery, cancel: &Cancel) -> Result<LogPage> {
        use gix::traverse::commit::{Parents, topo};
        let tips = self.tips(q)?;
        if tips.is_empty() {
            return Ok(LogPage::default());
        }
        let decorations = self.decorations()?;
        let commit_graph = self.git.commit_graph_if_enabled().ok().flatten();
        let walk: Box<dyn Iterator<Item = Result<gix::traverse::commit::Info>>> = match commit_graph {
            Some(cg) => Box::new(
                topo::Builder::from_iters(&self.git.objects, tips, None::<Vec<gix::ObjectId>>)
                    .sorting(topo::Sorting::TopoOrder)
                    .parents(if q.first_parent { Parents::First } else { Parents::All })
                    .with_commit_graph(Some(cg))
                    .build()?
                    .map(|i| i.map_err(Error::from)),
            ),
            None => {
                let order = gix::traverse::commit::simple::CommitTimeOrder::NewestFirst;
                let mut platform = self.git.rev_walk(tips).sorting(gix::revision::walk::Sorting::ByCommitTime(order));
                if q.first_parent {
                    platform = platform.first_parent_only();
                }
                Box::new(platform.all()?.map(|i| i.map(|i| i.detach()).map_err(Error::from)))
            }
        };
        let mut graph = q.graph.then(GraphBuilder::new);
        let mut page = LogPage::default();
        let mut parents: Vec<Oid> = Vec::with_capacity(4);
        for (n, info) in walk.enumerate() {
            if n.is_multiple_of(256) && cancel.is_cancelled() {
                return Err(cancelled());
            }
            let info = info?;
            if n >= q.skip + q.limit {
                page.more = true;
                break;
            }
            parents.clear();
            let n_parents = if q.first_parent { 1 } else { usize::MAX };
            parents.extend(info.parent_ids.iter().take(n_parents).map(|p| Oid::from_gix(p)));
            let id = Oid::from_gix(&info.id);
            let row = graph.as_mut().map(|g| g.push(id, &parents));
            if n < q.skip {
                continue;
            }
            let (commit, _) = self.commit_info(info.id)?;
            let refs = decorations.get(&id).cloned().unwrap_or_default();
            page.rows.push(LogRow { commit, graph: row, refs });
        }
        Ok(page)
    }

    /// The full commit: message and the files it changed (with line counts) against its first parent.
    pub fn commit_details(&self, id: Oid) -> Result<CommitDetails> {
        let (info, message) = self.commit_info(id.to_gix())?;
        let parent = info.parents.first().copied();
        let files = self.diff_commits(parent, id)?;
        let refs = self.decorations()?.remove(&id).unwrap_or_default();
        Ok(CommitDetails { info, message, files, refs })
    }

    /// Files changed between two commits (`from` `None` = the empty tree), renames detected per the repository's
    /// `diff.renames`, with line counts.
    pub fn diff_commits(&self, from: Option<Oid>, to: Oid) -> Result<Vec<FileChange>> {
        let new_tree = self.git.find_commit(to.to_gix())?.tree()?;
        let old_tree = match from {
            Some(f) => Some(self.git.find_commit(f.to_gix())?.tree()?),
            None => None,
        };
        let changes = self.git.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)?;
        let mut out = Vec::new();
        for c in changes {
            use crate::status::FileState as S;
            use gix::object::tree::diff::ChangeDetached as C;
            let (path, old_path, state, old, new) = match c {
                C::Addition { location, entry_mode, id, .. } => {
                    if !entry_mode.is_blob_or_symlink() {
                        continue;
                    }
                    (location.to_string(), None, S::Added, None, Some(id))
                }
                C::Deletion { location, entry_mode, id, .. } => {
                    if !entry_mode.is_blob_or_symlink() {
                        continue;
                    }
                    (location.to_string(), None, S::Deleted, Some(id), None)
                }
                C::Modification { location, previous_entry_mode, previous_id, entry_mode, id, .. } => {
                    if !entry_mode.is_blob_or_symlink() && !previous_entry_mode.is_blob_or_symlink() {
                        continue;
                    }
                    let state = if previous_entry_mode.kind() != entry_mode.kind()
                        && !(previous_entry_mode.is_blob() && entry_mode.is_blob())
                    {
                        S::TypeChanged
                    } else {
                        S::Modified
                    };
                    (location.to_string(), None, state, Some(previous_id), Some(id))
                }
                C::Rewrite { source_location, source_id, location, id, copy, entry_mode, .. } => {
                    if !entry_mode.is_blob_or_symlink() {
                        continue;
                    }
                    let state = if copy { S::Copied } else { S::Renamed };
                    (location.to_string(), Some(source_location.to_string()), state, Some(source_id), Some(id))
                }
            };
            let (added, removed, binary) = self.blob_stats(old, new)?;
            out.push(FileChange { path, old_path, state, added, removed, binary });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Line counts between two blobs (`None` = absent).
    pub(crate) fn blob_stats(
        &self,
        old: Option<gix::ObjectId>,
        new: Option<gix::ObjectId>,
    ) -> Result<(u32, u32, bool)> {
        if old.is_some() && old == new {
            return Ok((0, 0, false));
        }
        let load = |id: Option<gix::ObjectId>| -> Result<Text> {
            match id {
                Some(id) => Ok(Text::new(self.git.find_blob(id)?.data.clone())),
                None => Ok(Text::default()),
            }
        };
        let (a, b) = (load(old)?, load(new)?);
        Ok(text_stats(&a, &b, EolMode::Exact))
    }

    /// Commits that changed `path`, newest first, following renames (like `git log --follow`). Each commit
    /// comes with the path the file had in it.
    pub fn file_history(&self, path: &str, skip: usize, limit: usize, cancel: &Cancel) -> Result<HistoryPage> {
        let Ok(head) = self.git.head_id() else { return Ok(HistoryPage::default()) };
        // newest first by committer time, each queued commit carrying the path to look at
        #[derive(PartialEq, Eq)]
        struct Item(i64, gix::ObjectId, String);
        impl Ord for Item {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                self.0.cmp(&o.0).then_with(|| self.1.cmp(&o.1))
            }
        }
        impl PartialOrd for Item {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        let time_of = |id: gix::ObjectId| -> i64 {
            self.git.find_commit(id).ok().and_then(|c| c.time().ok()).map_or(0, |t| t.seconds)
        };
        let mut queue = BinaryHeap::new();
        let mut seen: HashSet<gix::ObjectId> = HashSet::new();
        queue.push(Item(time_of(head.detach()), head.detach(), path.to_string()));
        seen.insert(head.detach());
        let mut out = Vec::new();
        let mut matched = 0usize;
        let mut steps = 0usize;
        while let Some(Item(_, id, path)) = queue.pop() {
            steps += 1;
            if steps.is_multiple_of(256) && cancel.is_cancelled() {
                return Err(cancelled());
            }
            let commit = self.git.find_commit(id)?;
            let tree = commit.tree()?;
            let entry = |tree: &gix::Tree<'_>, p: &str| -> Option<gix::ObjectId> {
                tree.lookup_entry_by_path(crate::repo::rel_to_path(p)).ok().flatten().map(|e| e.object_id())
            };
            let here = entry(&tree, &path);
            let parent_ids: Vec<gix::ObjectId> = commit.parent_ids().map(|p| p.detach()).collect();
            let mut parent_entries = Vec::with_capacity(parent_ids.len());
            for p in &parent_ids {
                let pt = self.git.find_commit(*p)?.tree()?;
                parent_entries.push(entry(&pt, &path));
            }
            // TREESAME to a parent: the change came through that parent only (git's history simplification)
            if let Some(k) = parent_entries.iter().position(|e| *e == here) {
                let p = parent_ids[k];
                if here.is_some() && seen.insert(p) {
                    queue.push(Item(time_of(p), p, path));
                }
                continue;
            }
            if here.is_none() {
                continue; // the file does not exist on this line of history
            }
            if matched >= skip + limit {
                return Ok(HistoryPage { rows: out, more: true });
            }
            if matched >= skip {
                out.push(HistoryRow { commit: self.commit_info(id)?.0, path: path.clone() });
            }
            matched += 1;
            // where did it come from? same path in a parent, or a rename found by tree diff
            for (k, p) in parent_ids.iter().enumerate() {
                let from =
                    if parent_entries[k].is_some() { Some(path.clone()) } else { self.rename_source(*p, id, &path)? };
                if let Some(from) = from
                    && seen.insert(*p)
                {
                    queue.push(Item(time_of(*p), *p, from));
                }
            }
        }
        Ok(HistoryPage { rows: out, more: false })
    }

    /// The path `path` (added in `commit`) had in `parent`, if the diff sees a rename or copy.
    fn rename_source(&self, parent: gix::ObjectId, commit: gix::ObjectId, path: &str) -> Result<Option<String>> {
        let old = self.git.find_commit(parent)?.tree()?;
        let new = self.git.find_commit(commit)?.tree()?;
        let mut opts = gix::diff::Options::default().with_rewrites(Some(gix::diff::Rewrites::default()));
        opts.track_path();
        for c in self.git.diff_tree_to_tree(Some(&old), Some(&new), Some(opts))? {
            if let gix::object::tree::diff::ChangeDetached::Rewrite { source_location, location, .. } = c
                && location == path
            {
                return Ok(Some(source_location.to_string()));
            }
        }
        Ok(None)
    }

    /// Local and remote branches, tags, remotes and stashes.
    pub fn refs(&self) -> Result<Refs> {
        let mut out = Refs::default();
        let head = self.git.head()?;
        let head_branch = head.referent_name().map(|n| n.as_bstr().to_string());
        let platform = self.git.references()?;
        for r in platform.all()? {
            let Ok(mut r) = r else { continue };
            let full = r.name().as_bstr().to_string();
            let short = r.name().shorten().to_string();
            if let Some(remote) =
                full.strip_prefix("refs/remotes/").map(|_| true).or(full.starts_with("refs/heads/").then_some(false))
            {
                if full.ends_with("/HEAD") && remote {
                    continue;
                }
                let Ok(commit) = r.peel_to_commit() else { continue };
                let (time, summary) = match commit.decode() {
                    Ok(c) => (c.committer().map(|s| signature(s).time).unwrap_or(0), summary_of(c.message)),
                    Err(_) => (0, String::new()),
                };
                let upstream = (!remote)
                    .then(|| self.git.branch_remote_tracking_ref_name(r.name(), gix::remote::Direction::Fetch))
                    .flatten()
                    .and_then(|u| u.ok())
                    .map(|u| u.as_ref().shorten().to_string());
                let b = Branch {
                    name: short,
                    remote,
                    tip: Oid::from_gix(&commit.id),
                    is_head: head_branch.as_deref() == Some(full.as_str()),
                    upstream,
                    time,
                    summary,
                };
                if remote { out.remote_branches.push(b) } else { out.branches.push(b) }
            } else if full.starts_with("refs/tags/") {
                let target_id = r.target().try_id().map(|i| i.to_owned());
                let (annotated, message) = match target_id.and_then(|i| self.git.find_object(i).ok()) {
                    Some(obj) if obj.kind == gix::object::Kind::Tag => {
                        let msg = obj.into_tag().decode().map(|t| t.message.to_str_lossy().trim().to_string()).ok();
                        (true, msg)
                    }
                    _ => (false, None),
                };
                let Ok(commit) = r.peel_to_commit() else { continue };
                out.tags.push(Tag { name: short, target: Oid::from_gix(&commit.id), annotated, message });
            }
        }
        for name in self.git.remote_names() {
            let Ok(remote) = self.git.find_remote(name.as_bstr()) else { continue };
            let url = |d| remote.url(d).map(|u| u.to_bstring().to_string());
            out.remotes.push(Remote {
                name: name.to_string(),
                url: url(gix::remote::Direction::Fetch),
                push_url: url(gix::remote::Direction::Push),
            });
        }
        out.stashes = self.stashes()?;
        out.branches.sort_by(|a, b| a.name.cmp(&b.name));
        out.remote_branches.sort_by(|a, b| a.name.cmp(&b.name));
        out.tags.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// `git stash list`, newest (`stash@{0}`) first.
    pub fn stashes(&self) -> Result<Vec<Stash>> {
        let Some(r) = self.git.try_find_reference("refs/stash")? else { return Ok(Vec::new()) };
        let mut platform = r.log_iter();
        let Some(lines) = platform.rev()? else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for (index, line) in lines.enumerate() {
            let Ok(line) = line else { continue };
            let Some(id) = Oid::from_hex(&line.new_oid.to_string()) else { continue };
            out.push(Stash { index, id, message: line.message.to_string(), time: line.signature.time.seconds });
        }
        Ok(out)
    }
}

/// Lines added and removed between two texts, and whether either is binary (then no counts).
pub(crate) fn text_stats(a: &Text, b: &Text, eol: EolMode) -> (u32, u32, bool) {
    if a.is_binary() || b.is_binary() {
        return (0, 0, true);
    }
    let (added, removed) = stats(&diff(a, b, eol));
    (added, removed, false)
}
