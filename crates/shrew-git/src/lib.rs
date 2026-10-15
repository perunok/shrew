//! shrew's git backend: everything git, no UI. Reads run in-process with [gix] (pure Rust, no network code),
//! line and word diffs with imara-diff, and writes and network go through the `git` program, so hooks,
//! credential helpers, signing and ssh behave exactly as on the command line. Nothing here touches the UI
//! thread: the app talks to a [`GitService`], or calls [`Repo`] methods from its own threads.
//!
//! # Using the service (the UI's entry point)
//!
//! ```no_run
//! use shrew_git::*;
//! use std::sync::Arc;
//! # let post_redraw_event = || {};
//! let git = GitService::new(Arc::new(move || post_redraw_event())); // no thread starts yet
//! git.send(Request::Discover { path: "/project/src/main.rs".into() });
//! // later, on the wake callback:
//! for ev in git.poll() {
//!     match ev.reply {
//!         Reply::Discovered { repo: Some(info), .. } => {
//!             git.send(Request::Status { repo: info.workdir.clone(), options: StatusOptions::default() });
//!         }
//!         Reply::Status(Ok(status)) => { /* sidebar marks, branch in the status bar */ }
//!         Reply::Gutter(Ok(g)) => { /* keep if g.version is the buffer's current one */ }
//!         Reply::Changed(c) if c.index || c.head => { /* re-send Gutter for open buffers of ev.repo */ }
//!         _ => {}
//!     }
//! }
//! ```
//!
//! * Requests name a repository by [`RepoInfo::workdir`] and files by repository path (`src/main.rs`, `/`
//!   separators; [`Repo::rel_path`] converts). Every request returns a [`Ticket`]; its [`Event`]s carry it.
//! * Send [`Request::Gutter`] with a rope snapshot after edits (clone is O(1)); pending ones coalesce per
//!   buffer, so sending on every keystroke is fine. A 10k-line file diffs in about 0.6 ms on the worker.
//! * Call [`GitService::set_focused`] from window focus events and [`GitService::refresh`] after saving a file.
//!   A repository is watched once its status was requested: while focused the service stats a few files
//!   every 3 s and reports [`Reply::Changed`] plus a new status; unfocused it does nothing.
//! * Writes are [`Op`]s on one ordered lane; network ops stream [`Reply::Progress`] and stop with
//!   [`GitService::cancel`]. Every op is followed by a fresh [`Reply::Status`].
//!
//! # Gutter, hunks and staging lines
//!
//! [`diff::diff`] gives [`diff::Hunk`]s: `old` and `new` 0-based line ranges, kind from [`diff::Hunk::kind`]
//! (a deletion has an empty `new` range anchored *before* new line `new.start`). Lines are git's: a trailing
//! newline adds no empty last line, so hunk line `i` is rope line `i`. Against the index or worktree use
//! [`diff::EolMode::Normalize`] (an LF index and a CRLF checkout differ only in real changes).
//!
//! To stage from the gutter, build a [`patch::LineSel`] over the gutter's hunks — whole hunks with
//! [`patch::LineSel::hunks`], or editor lines with [`patch::LineSel::new_lines`] — and send
//! `Op::StageLines { path, text: Some(buffer text) or None for the file on disk, sel,
//! base: Base::Blob(gutter.base.id) }`. With an index-based gutter the selection's line numbers are exactly
//! the diff the op recomputes; `base` makes it fail with [`ErrorKind::Stale`] instead of staging the wrong
//! lines if the index moved. Unstaging works on the HEAD -> index diff ([`Version::Head`] to
//! [`Version::Index`] in a [`Request::FileDiff`]), discarding on index -> worktree; reverting a hunk in an
//! open buffer is just replacing `hunk.new` lines with `base.text.hunk_bytes(&hunk.old)`, undoable.
//!
//! # Diff views
//!
//! [`Request::FileDiff`] returns both texts, the hunks and per-hunk [`diff::WordDiff`]s (byte spans per line
//! for intra-line highlights). [`diff::groups`] adds context around hunks for a unified view and
//! [`diff::unified`] renders one. [`Request::TreeDiff`] lists changed files with line counts between any two
//! [`Version`]s (commit, HEAD, index, worktree).
//!
//! # History
//!
//! [`Request::Log`] pages ([`LogQuery::skip`]/`limit`) return rows with commit info, ref labels and a
//! [`graph::GraphRow`] (see [`graph`] for how to draw lanes; colours are indices into a palette). Pages
//! line up: the graph is laid out from the top each time. [`Request::FileHistory`] follows renames,
//! [`Request::CommitDetails`] gives the message and changed files, [`Request::Blame`] streams
//! [`BlameHunk`]s (unsaved buffers too: their new lines are [`BlameCommit::is_uncommitted`]).
//!
//! # Performance (release build, measured by `tests/perf.rs`)
//!
//! | what | time |
//! |---|---|
//! | gutter diff, 10k lines, one edit (snapshot + diff) | 0.6 ms |
//! | gutter diff, 10k lines, 100 hunks | 1.7 ms |
//! | status, 10k-file repository, 20 changes | 6 ms (`git status`: 7.6 ms) |
//! | log first page (200 rows), 20k commits | 0.45 ms (1.3 ms cold) |
//! | log page at row 10,000 | 7 ms |
//! | blame, 5000 lines over 200 commits | first lines after 2 ms, done in 106 ms |
//!
//! # Limits
//!
//! SHA-1 repositories only. Paths that are not UTF-8 are skipped. Ignored folders are listed as one
//! entry. A [`Repo`] reads the config once; after [`Changes::config`] open it again (the service does).

mod blame;
mod changes;
mod cli;
pub mod diff;
mod error;
pub mod graph;
mod log;
mod oid;
mod ops;
pub mod patch;
mod repo;
pub mod service;
mod status;
mod text;

pub use blame::{BlameCommit, BlameHunk};
pub use changes::{FileDiff, Version};
pub use cli::{Cancel, git_available};
pub use error::{Error, ErrorKind, Result};
pub use log::*;
pub use oid::Oid;
pub use ops::*;
pub use repo::*;
pub use service::{
    Event, FileTrack, GitService, Gutter, GutterBase, GutterBaseText, Op, OpOutput, Reply, Request, ServiceOptions,
    Ticket, Wake,
};
pub use status::*;
pub use text::{LineEnding, Text};

pub use ropey;
