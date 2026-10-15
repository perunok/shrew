//! Git tools for assistants: status, diffs, history, blame and single commits. Read-only by design: nothing here
//! stages, commits, checks out or touches the network, and there is no setting that turns that on (the editor's
//! own Source Control panel is where the user does those, and an assistant can ask for it with `run_command`'s
//! ex commands if the user wants).
//!
//! Reads go through `shrew-git` on the HTTP worker thread, never the UI thread; the editor is asked only for its
//! working directory and, for a path, the open buffer's text. The repository is the one containing shrew's working
//! directory (or, for a path, the repository of a buffer that is open in shrew), so an assistant cannot reach
//! repositories the user has not opened. Unsaved edits are not in git's world: `git_blame` uses the buffer's text
//! (its new lines show as "not committed yet"), the diff tools say when a buffer has edits they did not see.

use super::{Args, Host, ToolOutput, ToolResult, ToolSpec, index_by_path, opt_bool, opt_int, opt_str, schema};
use crate::project;
use serde_json::{Value, json};
use shrew_core::ropey::Rope;
use shrew_git::diff::{EolMode, diff, unified};
use shrew_git::{
    Cancel, CommitInfo, FileChange, Head, LogQuery, Oid, RefKind, RefLabel, Repo, RepoState, Signature, StatusOptions,
    Text, Untracked, Version,
};
use std::path::PathBuf;

/// Names of the tools in [`TOOLS`].
pub const NAMES: &[&str] = &["git_status", "git_diff", "git_log", "git_blame", "git_show_commit"];

/// Most bytes of diff text one call returns.
const MAX_DIFF_BYTES: usize = 200 * 1024;
/// Most changed files one listing returns.
const MAX_FILES: usize = 500;
/// Most blame ranges one call returns.
const MAX_BLAME_RANGES: usize = 1000;

const PATH: &str = "File or folder, relative to shrew's working directory or absolute; it must be in the repository of that folder \
     (or of a buffer open in shrew).";

fn count(desc: &str, min: usize, max: usize) -> Value {
    json!({ "type": "integer", "minimum": min, "maximum": max, "description": desc })
}

pub const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "git_status",
        title: "Git status",
        description: "Branch, upstream (ahead/behind), any merge or rebase in progress, and every changed file: staged, \
                      unstaged, untracked and conflicted, with the two-letter codes of `git status --short` (first letter = staged, \
                      second = unstaged). Reads the repository of shrew's working directory; unsaved edits in shrew are not included.",
        read_only: true,
        destructive: false,
        input: || {
            schema(
                json!({
                    "path": { "type": "string", "description": "Only entries under this file or folder." },
                    "untracked": { "type": "boolean", "description": "List untracked files (default true)." },
                    "ignored": { "type": "boolean", "description": "List ignored files and folders (default false)." },
                    "limit": count("Most entries (default 200).", 1, 2000)
                }),
                &[],
            )
        },
    },
    ToolSpec {
        name: "git_diff",
        title: "Git diff",
        description: "A unified diff. mode \"unstaged\" (default): index -> working tree, like `git diff`; \"staged\": HEAD -> \
                      index, like `git diff --staged`; \"head\": HEAD -> working tree; \"commit\": rev against base (default: the \
                      first parent of rev, so the commit's own changes). Optionally limited to one file or folder. Untracked files \
                      show as added. Output is capped (200 KB); narrow it with path. Saved files only: unsaved edits in shrew are not seen.",
        read_only: true,
        destructive: false,
        input: || {
            schema(
                json!({
                    "mode": { "type": "string", "enum": ["unstaged", "staged", "head", "commit"], "description": "What to compare (default \"unstaged\")." },
                    "rev": { "type": "string", "description": "For mode \"commit\": a commit, branch, tag or `HEAD~2`." },
                    "base": { "type": "string", "description": "For mode \"commit\": compare against this revision instead of rev's first parent." },
                    "path": { "type": "string", "description": PATH },
                    "context": count("Context lines around changes (default 3).", 0, 50)
                }),
                &[],
            )
        },
    },
    ToolSpec {
        name: "git_log",
        title: "Git log",
        description: "Commit history, newest first, a page at a time: short id, date, author, summary and the branches and tags on \
                      each commit. With path: the commits that changed that file, following renames. Ask for the next page with \
                      skip when the answer says more commits follow.",
        read_only: true,
        destructive: false,
        input: || {
            schema(
                json!({
                    "rev": { "type": "string", "description": "Start from this branch, tag or commit (default HEAD)." },
                    "path": { "type": "string", "description": "A file: only the commits that changed it." },
                    "all": { "type": "boolean", "description": "Start from every branch and tag (ignored with path or rev)." },
                    "first_parent": { "type": "boolean", "description": "Follow only first parents (hides what merges brought in)." },
                    "skip": count("Commits to skip (paging, default 0).", 0, 10_000_000),
                    "limit": count("Commits per page (default 30).", 1, 200)
                }),
                &[],
            )
        },
    },
    ToolSpec {
        name: "git_blame",
        title: "Git blame",
        description: "Who last changed each line of a file: line ranges with the commit, author, date and summary. Uses the text \
                      open in shrew when the file has unsaved edits (new lines show as not committed yet). Limit it to a range \
                      of lines to keep the answer short.",
        read_only: true,
        destructive: false,
        input: || {
            schema(
                json!({
                    "path": { "type": "string", "description": "The file (default: the buffer on screen)." },
                    "start_line": { "type": "integer", "minimum": 1, "description": "First line (1-based, default 1)." },
                    "end_line": { "type": "integer", "minimum": 1, "description": "Last line, inclusive (default: the end)." },
                    "rev": { "type": "string", "description": "Blame the file as of this commit instead of the working tree." }
                }),
                &[],
            )
        },
    },
    ToolSpec {
        name: "git_show_commit",
        title: "Show a commit",
        description: "One commit: id, author, date, branches and tags, the whole message and the files it changed with line counts \
                      (against its first parent). With diff true also the unified diff, capped at 200 KB; limit it with path.",
        read_only: true,
        destructive: false,
        input: || {
            schema(
                json!({
                    "rev": { "type": "string", "description": "A commit, branch, tag or `HEAD~2` (default HEAD)." },
                    "diff": { "type": "boolean", "description": "Include the diff (default false: only the file list)." },
                    "path": { "type": "string", "description": PATH },
                    "context": count("Context lines in the diff (default 3).", 0, 50)
                }),
                &[],
            )
        },
    },
];

pub fn call(host: &dyn Host, name: &str, a: &Args) -> ToolResult {
    match name {
        "git_status" => status(host, a),
        "git_diff" => diff_tool(host, a),
        "git_log" => log(host, a),
        "git_blame" => blame(host, a),
        "git_show_commit" => show_commit(host, a),
        other => Err(format!("Unknown tool \"{other}\"")),
    }
}

// ---------------------------------------------------------------------------
// finding the repository
// ---------------------------------------------------------------------------

/// The repository a call is about, and the path argument inside it.
struct Ctx {
    repo: Repo,
    /// The `path` argument as a repository path; `None` for the whole repository.
    rel: Option<String>,
    /// The open buffer for `path`: its text and whether it has unsaved edits.
    buffer: Option<(Rope, bool)>,
}

fn open(host: &dyn Host, path: Option<&str>) -> Result<Ctx, String> {
    let path = path.map(str::to_string);
    let (cwd, abs, buffer) = super::with_editor(host, move |ed| {
        let cwd = ed.cwd.clone();
        let abs = path.as_deref().map(|p| project::absolute(&cwd, p));
        let buffer = abs.as_deref().and_then(|a| index_by_path(ed, a)).map(|i| {
            let d = &ed.buffers[i].doc;
            (d.rope.clone(), d.dirty)
        });
        (cwd, abs, buffer)
    })?;
    let from_cwd = Repo::discover(&cwd).map_err(|e| e.to_string())?;
    let shown = |p: &PathBuf| project::display(&cwd, p);
    let repo = match (&abs, from_cwd) {
        (Some(a), Some(r)) if r.rel_path(a).is_some() || canon(a) == r.workdir() => r,
        (Some(a), _) if buffer.is_some() => Repo::discover(a)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{} is not in a git repository.", shown(a)))?,
        (Some(a), Some(_)) => {
            return Err(format!(
                "{} is outside the git repository of shrew's working directory ({}).",
                shown(a),
                cwd.display()
            ));
        }
        (_, Some(r)) => r,
        (_, None) => {
            return Err(format!("shrew's working directory ({}) is not inside a git repository.", cwd.display()));
        }
    };
    let rel = abs.as_ref().and_then(|a| repo.rel_path(a));
    Ok(Ctx { repo, rel, buffer })
}

fn canon(p: &std::path::Path) -> PathBuf {
    shrew_git::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

impl Ctx {
    /// Does repository path `p` fall under the `path` argument?
    fn covers(&self, p: &str) -> bool {
        match &self.rel {
            None => true,
            Some(r) => p == r || p.strip_prefix(r.as_str()).is_some_and(|rest| rest.starts_with('/')),
        }
    }

    /// A note for diffs and status when the buffer for `path` has edits git cannot see.
    fn unsaved_note(&self) -> Option<&'static str> {
        self.buffer.as_ref().is_some_and(|(_, dirty)| *dirty).then_some(
            "[shrew: this file has unsaved edits in shrew that are not part of this answer; save_buffer first.]",
        )
    }
}

fn e(err: shrew_git::Error) -> String {
    err.to_string()
}

/// A revision the user named, resolved to a commit.
fn resolve(repo: &Repo, rev: &str) -> Result<Oid, String> {
    repo.resolve(rev).map_err(e)
}

// ---------------------------------------------------------------------------
// formatting
// ---------------------------------------------------------------------------

/// `(year, month, day)` of a day count since 1970-01-01 (proleptic Gregorian).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `2026-10-15T14:03:00+02:00`: the author's local time, as git records it.
pub(super) fn iso(time: i64, offset: i32) -> String {
    let local = time + i64::from(offset);
    let (y, m, d) = civil(local.div_euclid(86_400));
    let s = local.rem_euclid(86_400);
    let sign = if offset < 0 { '-' } else { '+' };
    let off = offset.unsigned_abs();
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        s / 3600,
        s % 3600 / 60,
        s % 60,
        off / 3600,
        off % 3600 / 60
    )
}

fn day(sig: &Signature) -> String {
    iso(sig.time, sig.offset)[..10].to_string()
}

fn refs_text(refs: &[RefLabel]) -> String {
    if refs.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = refs
        .iter()
        .map(|r| match r.kind {
            RefKind::Tag => format!("tag: {}", r.name),
            _ if r.is_head => format!("HEAD -> {}", r.name),
            _ => r.name.clone(),
        })
        .collect();
    format!(" ({})", parts.join(", "))
}

fn refs_json(refs: &[RefLabel]) -> Value {
    refs.iter()
        .map(|r| {
            let kind = match r.kind {
                RefKind::Head => "head",
                RefKind::Branch => "branch",
                RefKind::RemoteBranch => "remote",
                RefKind::Tag => "tag",
                RefKind::Stash => "stash",
            };
            json!({ "name": r.name, "kind": kind, "head": r.is_head })
        })
        .collect()
}

fn commit_json(c: &CommitInfo) -> Value {
    json!({
        "id": c.id.to_string(),
        "short": c.id.short(),
        "parents": c.parents.iter().map(|p| p.short()).collect::<Vec<_>>(),
        "author": { "name": c.author.name, "email": c.author.email },
        "date": iso(c.author.time, c.author.offset),
        "summary": c.summary,
    })
}

fn state_text(s: RepoState) -> Option<&'static str> {
    Some(match s {
        RepoState::Clean => return None,
        RepoState::Merging => "merging",
        RepoState::Rebasing => "rebasing",
        RepoState::CherryPicking => "cherry-picking",
        RepoState::Reverting => "reverting",
        RepoState::Bisecting => "bisecting",
        RepoState::ApplyingPatches => "applying patches",
    })
}

fn head_line(h: &Head) -> String {
    let mut s = match (&h.branch, &h.oid) {
        (Some(b), None) => format!("## {b} (no commits yet)"),
        (Some(b), Some(_)) => format!("## {b}"),
        (None, Some(id)) => format!("## HEAD (detached at {})", id.short()),
        (None, None) => "## HEAD".to_string(),
    };
    if let Some(u) = &h.upstream {
        s.push_str(&format!("...{}", u.name));
        if u.gone {
            s.push_str(" [gone]");
        } else if u.ahead > 0 || u.behind > 0 {
            let mut parts = Vec::new();
            if u.ahead > 0 {
                parts.push(format!("ahead {}", u.ahead));
            }
            if u.behind > 0 {
                parts.push(format!("behind {}", u.behind));
            }
            s.push_str(&format!(" [{}]", parts.join(", ")));
        }
    }
    if let Some(st) = state_text(h.state) {
        s.push_str(&format!(" ({st})"));
    }
    s
}

fn changes_json(files: &[FileChange]) -> Value {
    files
        .iter()
        .map(|c| {
            json!({
                "path": c.path, "old_path": c.old_path, "status": c.state.letter().to_string(),
                "added": c.added, "removed": c.removed, "binary": c.binary
            })
        })
        .collect()
}

fn change_line(c: &FileChange) -> String {
    let name = match &c.old_path {
        Some(o) => format!("{o} -> {}", c.path),
        None => c.path.clone(),
    };
    if c.binary {
        format!("{}  {name}  (binary)", c.state.letter())
    } else {
        format!("{}  {name}  +{} -{}", c.state.letter(), c.added, c.removed)
    }
}

// ---------------------------------------------------------------------------
// git_status
// ---------------------------------------------------------------------------

fn status(host: &dyn Host, a: &Args) -> ToolResult {
    let cx = open(host, opt_str(a, "path")?)?;
    let untracked =
        if matches!(a.get("untracked"), Some(Value::Bool(false))) { Untracked::None } else { Untracked::Normal };
    let limit = opt_int(a, "limit", 1, 2000)?.unwrap_or(200);
    let ignored = opt_bool(a, "ignored")?;
    let st = cx.repo.status(&StatusOptions { untracked, ignored }, &Cancel::new()).map_err(e)?;
    let entries: Vec<_> = st.entries.iter().filter(|x| cx.covers(&x.path)).collect();
    let mut text = head_line(&st.head);
    text.push('\n');
    for x in entries.iter().take(limit) {
        match &x.orig_path {
            Some(o) => text.push_str(&format!("{} {o} -> {}\n", x.short_code(), x.path)),
            None => text.push_str(&format!("{} {}\n", x.short_code(), x.path)),
        }
    }
    if entries.is_empty() {
        text.push_str("Nothing to commit, working tree clean.\n");
    }
    let truncated = entries.len() > limit;
    if truncated {
        text.push_str(&format!(
            "[shrew: {} more entries not shown; raise limit (max 2000) or give a path]\n",
            entries.len() - limit
        ));
    }
    let list: Vec<Value> = entries
        .iter()
        .take(limit)
        .map(|x| {
            json!({
                "path": x.path, "old_path": x.orig_path, "code": x.short_code(),
                "staged": x.staged.map(|s| s.letter().to_string()),
                "unstaged": x.unstaged.map(|s| s.letter().to_string()),
                "untracked": x.untracked, "ignored": x.ignored, "conflict": x.conflict.is_some()
            })
        })
        .collect();
    let h = &st.head;
    let v = json!({
        "repository": cx.repo.workdir().to_string_lossy(),
        "branch": h.branch, "detached": h.detached, "head": h.oid.map(|o| o.to_string()),
        "upstream": h.upstream.as_ref().map(|u| json!({ "name": u.name, "ahead": u.ahead, "behind": u.behind, "gone": u.gone })),
        "state": state_text(h.state), "clean": entries.is_empty(),
        "entries": list, "truncated": truncated
    });
    Ok(ToolOutput::with(text, v))
}

// ---------------------------------------------------------------------------
// diffs
// ---------------------------------------------------------------------------

/// Appends the unified diff of `changes` (old side `old`, `None` = nothing) to `out` until `MAX_DIFF_BYTES`.
/// Returns how many files were left out.
fn render(
    repo: &Repo,
    changes: &[&FileChange],
    old: Option<Version>,
    new: Version,
    context: u32,
    out: &mut String,
) -> Result<usize, String> {
    let eol = if new == Version::Worktree { EolMode::Normalize } else { EolMode::Exact };
    for (i, c) in changes.iter().enumerate() {
        if out.len() >= MAX_DIFF_BYTES {
            return Ok(changes.len() - i);
        }
        let from = c.old_path.as_deref().unwrap_or(&c.path);
        let a = match old {
            Some(v) => repo.read_version(from, v).map_err(e)?,
            None => None,
        };
        let b = repo.read_version(&c.path, new).map_err(e)?;
        let (ta, tb) =
            (a.as_ref().map(|x| x.0.clone()).unwrap_or_default(), b.as_ref().map(|x| x.0.clone()).unwrap_or_default());
        out.push_str(&format!("diff --git a/{from} b/{}\n", c.path));
        if c.old_path.is_some() {
            out.push_str(&format!("rename from {from}\nrename to {}\n", c.path));
        }
        if ta.is_binary() || tb.is_binary() {
            out.push_str(&format!("Binary files a/{from} and b/{} differ\n", c.path));
            continue;
        }
        let hunks = diff(&ta, &tb, eol);
        if hunks.is_empty() {
            continue;
        }
        let (la, lb) = (
            if a.is_some() { format!("a/{from}") } else { "/dev/null".into() },
            if b.is_some() { format!("b/{}", c.path) } else { "/dev/null".into() },
        );
        out.push_str(&format!("--- {la}\n+++ {lb}\n"));
        out.push_str(&String::from_utf8_lossy(&unified(&ta, &tb, &hunks, context)));
    }
    Ok(0)
}

fn context_arg(a: &Args) -> Result<u32, String> {
    Ok(opt_int(a, "context", 0, 50)?.unwrap_or(3) as u32)
}

fn diff_tool(host: &dyn Host, a: &Args) -> ToolResult {
    let cx = open(host, opt_str(a, "path")?)?;
    let context = context_arg(a)?;
    let mode = opt_str(a, "mode")?.unwrap_or("unstaged");
    let cancel = Cancel::new();
    let (old, new, label): (Option<Version>, Version, String) = match mode {
        "unstaged" => (Some(Version::Index), Version::Worktree, "index -> working tree".into()),
        "staged" => (Some(Version::Head), Version::Index, "HEAD -> index".into()),
        "head" => (Some(Version::Head), Version::Worktree, "HEAD -> working tree".into()),
        "commit" => {
            let rev = opt_str(a, "rev")?.ok_or("mode \"commit\" needs \"rev\"")?;
            let id = resolve(&cx.repo, rev)?;
            let old = match opt_str(a, "base")? {
                Some(b) => Some(Version::Commit(resolve(&cx.repo, b)?)),
                None => cx.repo.commit_details(id).map_err(e)?.info.parents.first().map(|p| Version::Commit(*p)),
            };
            (old, Version::Commit(id), format!("{rev} ({})", id.short()))
        }
        other => return Err(format!("\"mode\" must be unstaged, staged, head or commit, not \"{other}\"")),
    };
    let all = match (old, new) {
        (Some(o), n) => cx.repo.tree_diff(o, n, &cancel).map_err(e)?,
        (None, Version::Commit(id)) => cx.repo.diff_commits(None, id).map_err(e)?,
        (None, _) => Vec::new(),
    };
    let files: Vec<&FileChange> = all.iter().filter(|c| cx.covers(&c.path)).collect();
    if files.is_empty() {
        return Ok(ToolOutput::with(
            format!("No differences ({label})."),
            json!({ "mode": mode, "files": [], "truncated": false }),
        ));
    }
    let mut text = format!("# {label}: {} file{}\n", files.len(), if files.len() == 1 { "" } else { "s" });
    for c in files.iter().take(MAX_FILES) {
        text.push_str(&format!("# {}\n", change_line(c)));
    }
    let shown: Vec<&FileChange> = files.iter().take(MAX_FILES).copied().collect();
    let left_out = render(&cx.repo, &shown, old, new, context, &mut text)? + files.len().saturating_sub(MAX_FILES);
    if left_out > 0 {
        text.push_str(&format!("[shrew: output cut, {left_out} file(s) not shown; give a path to see them]\n"));
    }
    if let Some(n) = cx.unsaved_note() {
        text.push_str(n);
        text.push('\n');
    }
    let owned: Vec<FileChange> = files.iter().map(|c| (*c).clone()).collect();
    Ok(ToolOutput::with(text, json!({ "mode": mode, "files": changes_json(&owned), "truncated": left_out > 0 })))
}

// ---------------------------------------------------------------------------
// git_log
// ---------------------------------------------------------------------------

/// A commit with its labels and, for a file's history, the file's path in it.
type LogEntry = (CommitInfo, Vec<RefLabel>, Option<String>);

fn log(host: &dyn Host, a: &Args) -> ToolResult {
    let cx = open(host, opt_str(a, "path")?)?;
    let skip = opt_int(a, "skip", 0, 10_000_000)?.unwrap_or(0);
    let limit = opt_int(a, "limit", 1, 200)?.unwrap_or(30);
    let rev = opt_str(a, "rev")?;
    let cancel = Cancel::new();
    let decorations = cx.repo.decorations().unwrap_or_default();
    let (rows, more): (Vec<LogEntry>, bool) = match (&cx.rel, rev) {
        (Some(path), None) => {
            let page = cx.repo.file_history(path, skip, limit, &cancel).map_err(e)?;
            let rows = page
                .rows
                .into_iter()
                .map(|r| {
                    let refs = decorations.get(&r.commit.id).cloned().unwrap_or_default();
                    (r.commit, refs, Some(r.path))
                })
                .collect();
            (rows, page.more)
        }
        (Some(_), Some(_)) => return Err("Pass either \"path\" or \"rev\" (a file's history starts at HEAD).".into()),
        (None, _) => {
            let q = LogQuery {
                revs: rev.map(|r| vec![r.to_string()]).unwrap_or_default(),
                all: opt_bool(a, "all")?,
                first_parent: opt_bool(a, "first_parent")?,
                skip,
                limit,
                graph: false,
            };
            let page = cx.repo.log(&q, &cancel).map_err(e)?;
            (page.rows.into_iter().map(|r| (r.commit, r.refs, None)).collect(), page.more)
        }
    };
    let mut text = String::new();
    for (c, refs, path) in &rows {
        text.push_str(&format!(
            "{} {} {}: {}{}",
            c.id.short(),
            day(&c.author),
            c.author.name,
            c.summary,
            refs_text(refs)
        ));
        if let Some(p) = path.as_ref().filter(|p| Some(*p) != cx.rel.as_ref()) {
            text.push_str(&format!(" [as {p}]"));
        }
        text.push('\n');
    }
    if rows.is_empty() {
        text = "No commits.".into();
    } else if more {
        text.push_str(&format!("[shrew: more commits follow; call again with skip={}]\n", skip + rows.len()));
    }
    let list: Vec<Value> = rows
        .iter()
        .map(|(c, refs, path)| {
            let mut v = commit_json(c);
            v["refs"] = refs_json(refs);
            if let Some(p) = path {
                v["path"] = json!(p);
            }
            v
        })
        .collect();
    Ok(ToolOutput::with(text, json!({ "commits": list, "more": more, "next_skip": skip + rows.len() })))
}

// ---------------------------------------------------------------------------
// git_blame
// ---------------------------------------------------------------------------

fn blame(host: &dyn Host, a: &Args) -> ToolResult {
    let path = match opt_str(a, "path")? {
        Some(p) => p.to_string(),
        None => {
            let active = super::with_editor(host, |ed| ed.buf().doc.path.clone())?;
            active.ok_or("The buffer on screen has no file; pass \"path\".")?.to_string_lossy().into_owned()
        }
    };
    let cx = open(host, Some(&path))?;
    let rel = cx.rel.clone().ok_or_else(|| format!("{path} is not a file in the repository."))?;
    let rev = opt_str(a, "rev")?.map(|r| resolve(&cx.repo, r)).transpose()?;
    // the buffer's own text, so lines edited but not saved show as "not committed yet"
    let contents = match (&cx.buffer, rev) {
        (Some((rope, true)), None) => Some(Text::from_rope(rope)),
        _ => None,
    };
    let mut hunks = Vec::new();
    cx.repo.blame(&rel, rev, contents.as_ref(), &Cancel::new(), &mut |h| hunks.push(h)).map_err(e)?;
    hunks.sort_by_key(|h| h.start);
    let total = hunks.last().map_or(0, |h| (h.start + h.len) as usize);
    let first = opt_int(a, "start_line", 1, usize::MAX)?.unwrap_or(1);
    let last = opt_int(a, "end_line", 1, usize::MAX)?.unwrap_or(total.max(first)).min(total.max(1));
    if first > last {
        return Err(format!("start_line {first} is after end_line {last}."));
    }
    let (lo, hi) = (first as u32 - 1, last as u32);
    let mut text = String::new();
    let mut list = Vec::new();
    let mut cut = false;
    for h in hunks.iter().filter(|h| h.start < hi && h.start + h.len > lo) {
        if list.len() >= MAX_BLAME_RANGES {
            cut = true;
            break;
        }
        let (s, t) = (h.start.max(lo) + 1, (h.start + h.len).min(hi));
        let c = &h.commit;
        let (who, when) = if c.is_uncommitted() {
            ("not committed yet".to_string(), String::new())
        } else {
            (c.author.name.clone(), day(&c.author))
        };
        let id = if c.is_uncommitted() { "0000000".to_string() } else { c.id.short() };
        let range = if s == t { format!("{s}") } else { format!("{s}-{t}") };
        text.push_str(&format!("{range:>11}  {id}  {when:<10}  {who}: {}\n", c.summary));
        list.push(json!({
            "start_line": s, "end_line": t, "commit": c.id.to_string(), "short": id,
            "author": c.author.name, "email": c.author.email,
            "date": (!c.is_uncommitted()).then(|| iso(c.author.time, c.author.offset)),
            "summary": c.summary, "uncommitted": c.is_uncommitted()
        }));
    }
    if list.is_empty() {
        text = "No blame for that range (is the file empty, or past its last line?).".into();
    } else if cut {
        text.push_str("[shrew: more ranges not shown; narrow start_line and end_line]\n");
    }
    Ok(ToolOutput::with(text, json!({ "path": rel, "lines": total, "ranges": list, "truncated": cut })))
}

// ---------------------------------------------------------------------------
// git_show_commit
// ---------------------------------------------------------------------------

fn show_commit(host: &dyn Host, a: &Args) -> ToolResult {
    let cx = open(host, opt_str(a, "path")?)?;
    let rev = opt_str(a, "rev")?.unwrap_or("HEAD");
    let context = context_arg(a)?;
    let id = resolve(&cx.repo, rev)?;
    let d = cx.repo.commit_details(id).map_err(e)?;
    let i = &d.info;
    let mut text = format!(
        "commit {}{}\nAuthor: {} <{}>\nDate:   {}\n",
        i.id,
        refs_text(&d.refs),
        i.author.name,
        i.author.email,
        iso(i.author.time, i.author.offset)
    );
    if i.parents.len() > 1 {
        text.push_str(&format!("Merge:  {}\n", i.parents.iter().map(Oid::short).collect::<Vec<_>>().join(" ")));
    }
    text.push('\n');
    for line in d.message.trim_end().lines() {
        text.push_str(&format!("    {line}\n"));
    }
    let files: Vec<&FileChange> = d.files.iter().filter(|c| cx.covers(&c.path)).collect();
    text.push('\n');
    if i.parents.len() > 1 {
        text.push_str("(changes against the first parent)\n");
    }
    for c in files.iter().take(MAX_FILES) {
        text.push_str(&format!("{}\n", change_line(c)));
    }
    let mut left_out = files.len().saturating_sub(MAX_FILES);
    if opt_bool(a, "diff")? {
        text.push('\n');
        let old = i.parents.first().map(|p| Version::Commit(*p));
        let shown: Vec<&FileChange> = files.iter().take(MAX_FILES).copied().collect();
        left_out += render(&cx.repo, &shown, old, Version::Commit(id), context, &mut text)?;
    }
    if left_out > 0 {
        text.push_str(&format!("[shrew: output cut, {left_out} file(s) not shown; give a path to see them]\n"));
    }
    let owned: Vec<FileChange> = files.iter().map(|c| (*c).clone()).collect();
    let mut v = commit_json(i);
    v["message"] = json!(d.message);
    v["refs"] = refs_json(&d.refs);
    v["files"] = changes_json(&owned);
    v["truncated"] = json!(left_out > 0);
    Ok(ToolOutput::with(text, v))
}
