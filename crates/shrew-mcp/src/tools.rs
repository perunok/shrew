//! The tools shrew offers AI assistants, and the [`Host`] trait the app implements to let them reach the editor.
//!
//! Tools run on HTTP worker threads. They touch the [`Editor`] only through short jobs the app runs on its UI
//! thread ([`with_editor`]); anything slow (reading files, searching, formatting listings, computing edits) happens
//! on the worker, on rope snapshots (cloning a rope is O(1)). An edit is computed on a snapshot and applied only if
//! the buffer did not change meanwhile (else it is recomputed), so typing never waits for an assistant.
//!
//! Safety rules:
//! - Files on disk are reachable only inside shrew's working directory (symlinks resolved); buffers the user opened
//!   are reachable wherever they live.
//! - Edits are never saved by themselves, and each is one undo step.
//! - `run_command` cannot quit shrew, change its working directory or install updates.

use crate::project::{self, Filter, Hit};
use crate::text;
use serde_json::{Map, Value, json};
use shrew_core::ropey::Rope;
use shrew_core::{Change, ChangeSet, Document, Editor, Event, Mode, Range, Selection, lang};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

/// A piece of work for the UI thread: it gets the editor, does its (short) thing, and returns.
pub type Job = Box<dyn FnOnce(&mut Editor) + Send>;

/// What the tools need from the running app. Called from HTTP worker threads.
pub trait Host: Send + Sync {
    /// Runs `job` on the UI thread with the editor, soon (e.g. through the event loop's proxy), then redraws.
    /// Returns without waiting for it; Err when the window is gone. Dropping the job unrun is fine: the tool then
    /// reports that shrew's window is not available.
    fn run(&self, job: Job) -> Result<(), String>;
    /// A tool is starting (for a status-bar hint).
    fn activity(&self, _tool: &str) {}
}

/// How long a tool waits for the UI thread to pick up its job.
pub const UI_TIMEOUT: Duration = Duration::from_secs(20);

/// Runs `f` with the editor on the UI thread and waits for its result. A job that times out before it started is
/// cancelled (it will not run later); one that already started is waited for.
pub fn with_editor<R: Send + 'static>(
    host: &dyn Host,
    f: impl FnOnce(&mut Editor) -> R + Send + 'static,
) -> Result<R, String> {
    with_editor_timeout(host, UI_TIMEOUT, f)
}

pub(crate) fn with_editor_timeout<R: Send + 'static>(
    host: &dyn Host,
    timeout: Duration,
    f: impl FnOnce(&mut Editor) -> R + Send + 'static,
) -> Result<R, String> {
    const PENDING: u8 = 0;
    const RUNNING: u8 = 1;
    const CANCELLED: u8 = 2;
    let (tx, rx) = mpsc::sync_channel(1);
    let state = Arc::new(AtomicU8::new(PENDING));
    let s = state.clone();
    host.run(Box::new(move |ed| {
        if s.compare_exchange(PENDING, RUNNING, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            let _ = tx.send(f(ed));
        }
    }))?;
    let gone = || "shrew's window is not available.".to_string();
    match rx.recv_timeout(timeout) {
        Ok(r) => Ok(r),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            if state.compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                Err("shrew is busy and did not answer in time; nothing was changed. Try again.".into())
            } else {
                rx.recv().map_err(|_| gone())
            }
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(gone()),
    }
}

// ---------------------------------------------------------------------------
// tool specs
// ---------------------------------------------------------------------------

pub struct ToolSpec {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub read_only: bool,
    pub destructive: bool,
    pub input: fn() -> Value,
}

/// Lines `read_buffer` lists when no range is given.
pub const DEFAULT_READ_LINES: usize = 2000;
/// Longest listing returned (bytes).
pub const READ_CHARS: usize = 256 * 1024;
/// Longest selection text returned by `get_active_buffer` (chars per selection).
pub const SELECTION_CHARS: usize = 100_000;

const PATH: &str =
    "File path, relative to shrew's working directory or absolute. Leave out path and id to use the buffer on screen.";
const ID: &str = "Buffer id from list_buffers (for buffers without a file).";

fn schema(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// `path` and `id` plus `extra`.
fn target_schema(extra: Value, required: &[&str]) -> Value {
    let mut props = json!({ "path": { "type": "string", "description": PATH }, "id": { "type": "integer", "minimum": 1, "description": ID } });
    props.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    schema(props, required)
}

fn int(desc: &str) -> Value {
    json!({ "type": "integer", "minimum": 1, "description": desc })
}

pub fn tools() -> &'static [ToolSpec] {
    static TOOLS: &[ToolSpec] = &[
        ToolSpec {
            name: "list_buffers",
            title: "List open buffers",
            description: "Buffers open in shrew: id, path, unsaved changes (dirty), language, line count, and which one is on screen. Also shrew's working directory.",
            read_only: true,
            destructive: false,
            input: || schema(json!({}), &[]),
        },
        ToolSpec {
            name: "get_active_buffer",
            title: "The buffer on screen",
            description: "The buffer the user is looking at: path, cursor line/column, the selected text (with positions), the editor mode, and the lines around the cursor. Use it for \"this file\", \"here\" or \"the selection\".",
            read_only: true,
            destructive: false,
            input: || {
                schema(
                    json!({ "context": { "type": "integer", "minimum": 0, "maximum": 200, "description": "Lines to show above and below the cursor (default 5)." } }),
                    &[],
                )
            },
        },
        ToolSpec {
            name: "read_buffer",
            title: "Read a buffer or file",
            description: "Lines of a buffer as shrew holds them (unsaved edits included), numbered like `cat -n`; a file that is not open is read from disk (inside the working directory). Default: the first 2000 lines. Line numbers are for reference, not part of the text.",
            read_only: true,
            destructive: false,
            input: || {
                target_schema(
                    json!({ "start_line": int("First line (1-based, default 1)."), "end_line": int("Last line, inclusive (default: start_line + 1999).") }),
                    &[],
                )
            },
        },
        ToolSpec {
            name: "open_file",
            title: "Open a file in shrew",
            description: "Opens a file in shrew (or switches to it when open) so the user sees it, optionally at a line. A missing file opens as a new, empty buffer.",
            read_only: false,
            destructive: false,
            input: || {
                schema(
                    json!({ "path": { "type": "string", "description": "File path, relative to shrew's working directory or absolute." }, "line": int("Line to show (1-based).") }),
                    &["path"],
                )
            },
        },
        ToolSpec {
            name: "edit_buffer",
            title: "Edit a buffer",
            description: "Replaces old_string with new_string, exactly (whitespace and indentation included). old_string must occur once unless replace_all is true: include enough surrounding lines to make it unique. Applied in shrew as one undo step and NOT saved. Read the lines first.",
            read_only: false,
            destructive: false,
            input: || {
                target_schema(
                    json!({
                        "old_string": { "type": "string", "description": "Text to replace (exact, without line numbers)." },
                        "new_string": { "type": "string", "description": "Replacement text." },
                        "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false)." }
                    }),
                    &["old_string", "new_string"],
                )
            },
        },
        ToolSpec {
            name: "replace_lines",
            title: "Replace lines",
            description: "Replaces lines start_line..end_line (1-based, inclusive) with text. end_line = start_line - 1 inserts before start_line; start_line = line count + 1 appends; empty text deletes. One undo step, NOT saved.",
            read_only: false,
            destructive: false,
            input: || {
                target_schema(
                    json!({
                        "start_line": int("First line to replace."),
                        "end_line": { "type": "integer", "minimum": 0, "description": "Last line to replace, inclusive." },
                        "text": { "type": "string", "description": "The new lines (a final line break is added if missing)." }
                    }),
                    &["start_line", "end_line", "text"],
                )
            },
        },
        ToolSpec {
            name: "write_buffer",
            title: "Replace a buffer's text",
            description: "Replaces the whole text of a buffer (or creates a new file's buffer inside the working directory). Prefer edit_buffer for small changes. One undo step, NOT saved.",
            read_only: false,
            destructive: true,
            input: || {
                target_schema(
                    json!({ "content": { "type": "string", "description": "The complete new text." } }),
                    &["content"],
                )
            },
        },
        ToolSpec {
            name: "save_buffer",
            title: "Save to disk",
            description: "Writes a buffer to its file (or every buffer with unsaved changes when all is true).",
            read_only: false,
            destructive: true,
            input: || {
                target_schema(
                    json!({ "all": { "type": "boolean", "description": "Save every buffer with unsaved changes." } }),
                    &[],
                )
            },
        },
        ToolSpec {
            name: "run_command",
            title: "Run an ex command",
            description: "Runs an ex command line in shrew as if typed after `:` (e.g. \"%s/foo/bar/g\", \"s/a/b/\", \"set nonumber\", \"w\", \"42\", \"bn\") on the buffer on screen, or on path/id after switching to it. Returns shrew's status message. Quitting, :cd and :update are refused.",
            read_only: false,
            destructive: true,
            input: || {
                target_schema(
                    json!({ "command": { "type": "string", "description": "The command line, without the leading ':'." } }),
                    &["command"],
                )
            },
        },
        ToolSpec {
            name: "search",
            title: "Search",
            description: "Regex search (Rust regex syntax; ^ and $ match at line breaks). Default: every file in the project (respecting .gitignore), using shrew's text for open buffers. With path/id or scope \"buffer\": one buffer or file. Returns path:line:col: line, one hit per line, capped.",
            read_only: true,
            destructive: false,
            input: || {
                target_schema(
                    json!({
                        "pattern": { "type": "string", "description": "Regular expression." },
                        "scope": { "type": "string", "enum": ["project", "buffer"], "description": "\"project\" (default) or \"buffer\" (path/id, else the one on screen)." },
                        "ignore_case": { "type": "boolean", "description": "Case-insensitive (default false)." },
                        "glob": { "type": "string", "description": "Only files matching this glob (\"*.rs\", \"src/**/*.ts\") or containing this text in their path." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Most hits (default 100)." }
                    }),
                    &["pattern"],
                )
            },
        },
        ToolSpec {
            name: "list_files",
            title: "List project files",
            description: "Files under shrew's working directory (respecting .gitignore, hidden files skipped), as relative paths. Filter with a glob (\"*.rs\", \"src/**/mod.rs\") or a piece of the path.",
            read_only: true,
            destructive: false,
            input: || {
                schema(
                    json!({
                        "filter": { "type": "string", "description": "Glob or path substring." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 5000, "description": "Most files (default 500)." }
                    }),
                    &[],
                )
            },
        },
        ToolSpec {
            name: "goto",
            title: "Move the cursor",
            description: "Shows a place to the user: switches to the buffer (opening the file if needed) and puts the cursor at line/col, or selects up to end_line/end_col (exclusive). Columns count characters, 1-based.",
            read_only: false,
            destructive: false,
            input: || {
                target_schema(
                    json!({
                        "line": int("Line (1-based)."),
                        "col": int("Column (1-based, default: the first non-blank)."),
                        "end_line": int("Select up to this line."),
                        "end_col": int("Select up to this column (exclusive; default 1).")
                    }),
                    &["line"],
                )
            },
        },
    ];
    TOOLS
}

// ---------------------------------------------------------------------------
// arguments
// ---------------------------------------------------------------------------

/// A tool's answer: text for the model plus, when useful, a structured copy.
pub struct ToolOutput {
    pub text: String,
    pub structured: Option<Value>,
}

impl ToolOutput {
    fn data(v: Value) -> ToolOutput {
        ToolOutput { text: serde_json::to_string_pretty(&v).unwrap_or_default(), structured: Some(v) }
    }
    fn with(text: String, v: Value) -> ToolOutput {
        ToolOutput { text, structured: Some(v) }
    }
}

pub type ToolResult = Result<ToolOutput, String>;

type Args = Map<String, Value>;

fn obj(args: &Value) -> Result<Args, String> {
    match args {
        Value::Object(m) => Ok(m.clone()),
        Value::Null => Ok(Map::new()),
        _ => Err("arguments must be an object".into()),
    }
}

fn opt_str<'a>(a: &'a Args, key: &str) -> Result<Option<&'a str>, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("\"{key}\" must be a string")),
    }
}

fn req_str<'a>(a: &'a Args, key: &str) -> Result<&'a str, String> {
    opt_str(a, key)?.ok_or_else(|| format!("\"{key}\" is required"))
}

fn opt_int(a: &Args, key: &str, min: usize, max: usize) -> Result<Option<usize>, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(|n| n as usize)
            .filter(|n| (min..=max).contains(n))
            .map(Some)
            .ok_or_else(|| format!("\"{key}\" must be an integer from {min} to {max}")),
    }
}

fn req_int(a: &Args, key: &str, min: usize) -> Result<usize, String> {
    opt_int(a, key, min, usize::MAX)?.ok_or_else(|| format!("\"{key}\" is required"))
}

fn opt_bool(a: &Args, key: &str) -> Result<bool, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("\"{key}\" must be true or false")),
    }
}

/// Which buffer a call is about.
#[derive(Clone, Debug)]
enum Target {
    Active,
    Id(u64),
    Path(String),
}

fn target(a: &Args) -> Result<Target, String> {
    let id = opt_int(a, "id", 1, usize::MAX)?;
    match (opt_str(a, "path")?, id) {
        (Some(_), Some(_)) => Err("Pass either \"path\" or \"id\", not both.".into()),
        (Some(p), None) if p.trim().is_empty() => Err("\"path\" is empty".into()),
        (Some(p), None) => Ok(Target::Path(p.to_string())),
        (None, Some(id)) => Ok(Target::Id(id as u64)),
        (None, None) => Ok(Target::Active),
    }
}

// ---------------------------------------------------------------------------
// editor side (these run on the UI thread: keep them short)
// ---------------------------------------------------------------------------

/// A buffer's state at one moment; the rope is a cheap shared copy.
#[derive(Clone)]
struct Snap {
    id: u64,
    path: Option<PathBuf>,
    rope: Rope,
    version: u64,
    dirty: bool,
}

impl Snap {
    fn of(ed: &Editor, i: usize) -> Snap {
        let b = &ed.buffers[i];
        Snap {
            id: b.id,
            path: b.doc.path.clone(),
            rope: b.doc.rope.clone(),
            version: b.doc.version,
            dirty: b.doc.dirty,
        }
    }

    fn name(&self, cwd: &Path) -> String {
        match &self.path {
            Some(p) => project::display(cwd, p),
            None => format!("[scratch #{}]", self.id),
        }
    }
}

enum Found {
    Open(Snap),
    /// Not open: the absolute path to read from disk.
    Closed(PathBuf),
}

fn index_of(ed: &Editor, id: u64) -> Option<usize> {
    ed.buffers.iter().position(|b| b.id == id)
}

fn index_by_path(ed: &Editor, path: &Path) -> Option<usize> {
    ed.buffers.iter().position(|b| b.doc.path.as_deref() == Some(path))
}

fn find(ed: &Editor, t: &Target) -> Result<Found, String> {
    match t {
        Target::Active => Ok(Found::Open(Snap::of(ed, ed.active))),
        Target::Id(id) => index_of(ed, *id)
            .map(|i| Found::Open(Snap::of(ed, i)))
            .ok_or_else(|| format!("No buffer has id {id} (see list_buffers).")),
        Target::Path(p) => {
            let abs = project::absolute(&ed.cwd, p);
            Ok(match index_by_path(ed, &abs) {
                Some(i) => Found::Open(Snap::of(ed, i)),
                None => Found::Closed(abs),
            })
        }
    }
}

fn language(doc: &Document) -> Option<&'static str> {
    doc.path
        .as_deref()
        .and_then(lang::detect)
        .or_else(|| {
            let first: String = doc.rope.line(0).chars().take(200).collect();
            lang::detect_shebang(first.trim_end())
        })
        .map(|l| l.name)
}

/// Puts a document loaded off-thread into a buffer, unless one for its path appeared meanwhile. `show` makes it the
/// buffer on screen; otherwise it waits in the background (the user sees it in the buffer list).
fn adopt(ed: &mut Editor, doc: Document, show: bool) -> usize {
    if let Some(i) = doc.path.as_deref().and_then(|p| index_by_path(ed, p)) {
        if show {
            ed.switch_to(i);
        }
        return i;
    }
    if show {
        ed.install_document(doc);
        ed.active
    } else {
        ed.add_buffer(doc)
    }
}

/// Moves the cursor (or selects) in the active buffer the way the user would, in vim or Sublime mode.
fn place(ed: &mut Editor, line: usize, col: Option<usize>, end: Option<(usize, usize)>) {
    let vim = ed.vim();
    let rope = ed.buf().doc.rope.clone();
    let start = match col {
        Some(c) => text::pos_of(&rope, line, c),
        None => first_non_blank_at(&rope, line),
    };
    match end.map(|(l, c)| text::pos_of(&rope, l, c)) {
        Some(e) if e != start => {
            let (a, h) = (start.min(e), start.max(e));
            if vim {
                ed.set_mode(Mode::Visual);
                // visual ranges are inclusive
                ed.buf_mut().sel = Selection::single(Range::new(a, h - 1));
            } else {
                ed.buf_mut().sel = Selection::single(Range::new(a, h));
            }
        }
        _ => {
            if vim {
                ed.set_mode(Mode::Normal);
                ed.buf_mut().sel = Selection::point(shrew_core::text::clamp_normal(&rope, start));
            } else {
                ed.buf_mut().sel = Selection::point(start);
            }
        }
    }
    ed.buf_mut().center_on_cursor();
}

fn first_non_blank_at(rope: &Rope, line: usize) -> usize {
    let l = line.saturating_sub(1).min(shrew_core::text::last_line(rope));
    shrew_core::text::first_non_blank(rope, l)
}

/// Applies `changes` to buffer `id` as one undo step if it is still at `version`; Ok(Err(snap)) when it changed
/// meanwhile, with the fresh state to recompute on. Returns the state after the edit.
fn apply(
    ed: &mut Editor,
    id: u64,
    version: u64,
    changes: Vec<Change>,
    tool: &str,
) -> Result<Result<Snap, Snap>, String> {
    let Some(i) = index_of(ed, id) else { return Err("The buffer was closed in shrew.".into()) };
    if ed.buffers[i].doc.version != version {
        return Ok(Err(Snap::of(ed, i)));
    }
    if changes.is_empty() {
        return Ok(Ok(Snap::of(ed, i)));
    }
    let normal = ed.vim() && i == ed.active && ed.mode == Mode::Normal;
    let b = &mut ed.buffers[i];
    b.commit();
    b.apply(ChangeSet::new(changes), None);
    b.commit();
    if normal {
        let rope = b.doc.rope.clone();
        b.sel = b.sel.transform(|r| Range::point(shrew_core::text::clamp_normal(&rope, r.head)));
    }
    let name = b.doc.display_name();
    ed.info(format!("{tool}: {name} changed by the assistant (unsaved; u to undo)"));
    Ok(Ok(Snap::of(ed, i)))
}

// ---------------------------------------------------------------------------
// tool bodies (worker side)
// ---------------------------------------------------------------------------

/// The buffer for `t`, opening a file that is not open yet in the background. `create`: a missing file becomes a
/// new, empty buffer (else it is an error).
fn buffer_for(host: &dyn Host, t: Target, create: bool) -> Result<(PathBuf, Snap), String> {
    let (cwd, found) = with_editor(host, move |ed| find(ed, &t).map(|f| (ed.cwd.clone(), f)))??;
    match found {
        Found::Open(s) => Ok((cwd, s)),
        Found::Closed(abs) => {
            let doc = load(&cwd, &abs, create)?;
            let snap = with_editor(host, move |ed| {
                let i = adopt(ed, doc, false);
                Snap::of(ed, i)
            })?;
            Ok((cwd, snap))
        }
    }
}

/// Loads a project file (checked to be inside `cwd`, not binary, not huge). Missing: an empty document for that path
/// when `create`, else an error.
fn load(cwd: &Path, abs: &Path, create: bool) -> Result<Document, String> {
    project::guard(cwd, abs)?;
    if !abs.exists() {
        if create {
            let mut doc = Document::default();
            doc.path = Some(abs.to_path_buf());
            return Ok(doc);
        }
        return Err(format!("{} does not exist.", project::display(cwd, abs)));
    }
    // checks size and binary content, then loads the way shrew does (CRLF kept for saving)
    project::read_text(cwd, abs)?;
    Document::load(abs).map_err(|e| format!("{}: {e}", project::display(cwd, abs)))
}

/// Computes changes on a snapshot and applies them, recomputing if the user edited meanwhile.
fn modify(
    host: &dyn Host,
    t: Target,
    create: bool,
    tool: &'static str,
    compute: impl Fn(&Rope) -> Result<Vec<Change>, String>,
) -> Result<(PathBuf, Snap, Snap, Vec<Change>), String> {
    let (cwd, mut snap) = buffer_for(host, t, create)?;
    for _ in 0..5 {
        let changes = compute(&snap.rope)?;
        let c = changes.clone();
        let (id, version) = (snap.id, snap.version);
        match with_editor(host, move |ed| apply(ed, id, version, c, tool))?? {
            Ok(after) => return Ok((cwd, snap, after, changes)),
            Err(fresh) => snap = fresh,
        }
    }
    Err("The buffer kept changing while the edit was prepared; try again.".into())
}

/// Text answer for an edit: what changed and the edited lines with a little context.
fn edit_report(cwd: &Path, before: &Snap, after: &Snap, changes: &[Change], what: &str) -> ToolOutput {
    let name = after.name(cwd);
    let first = changes.first().map_or(1, |c| text::line_col(&before.rope, c.from).0);
    // where the first change ends in the new text
    let inserted_lines = changes.first().map_or(0, |c| c.text.matches('\n').count());
    let (listing, _) =
        text::numbered(&after.rope, first.saturating_sub(3), (first + inserted_lines + 3).min(first + 40), 16 * 1024);
    let total = text::line_count(&after.rope);
    let text = format!(
        "{what} in {name} at line {first} (now {total} lines). Not saved: save_buffer writes it; the user can undo it in shrew.\n{listing}"
    );
    ToolOutput::with(
        text,
        json!({ "path": after.path.as_ref().map(|_| name.clone()), "id": after.id, "line": first, "lines": total, "dirty": after.dirty, "saved": false }),
    )
}

pub fn call(host: &dyn Host, name: &str, args: &Value) -> ToolResult {
    let a = obj(args)?;
    host.activity(name);
    match name {
        "list_buffers" => list_buffers(host),
        "get_active_buffer" => get_active_buffer(host, opt_int(&a, "context", 0, 200)?.unwrap_or(5)),
        "read_buffer" => read_buffer(host, &a),
        "open_file" => {
            let path = req_str(&a, "path")?.to_string();
            let line = opt_int(&a, "line", 1, usize::MAX)?;
            show(host, Target::Path(path), line, None, None, true)
        }
        "goto" => {
            let line = req_int(&a, "line", 1)?;
            let col = opt_int(&a, "col", 1, usize::MAX)?;
            let end = match (opt_int(&a, "end_line", 1, usize::MAX)?, opt_int(&a, "end_col", 1, usize::MAX)?) {
                (Some(l), c) => Some((l, c.unwrap_or(1))),
                (None, Some(_)) => return Err("end_col needs end_line".into()),
                (None, None) => None,
            };
            show(host, target(&a)?, Some(line), col, end, false)
        }
        "edit_buffer" => {
            let old = req_str(&a, "old_string")?.to_string();
            let new = req_str(&a, "new_string")?.to_string();
            let all = opt_bool(&a, "replace_all")?;
            let (cwd, before, after, changes) = modify(host, target(&a)?, false, "edit_buffer", |rope| {
                text::replace_exact(&rope.to_string(), &old, &new, all)
            })?;
            let n = changes.len();
            Ok(edit_report(
                &cwd,
                &before,
                &after,
                &changes,
                &format!("Replaced {n} occurrence{}", if n == 1 { "" } else { "s" }),
            ))
        }
        "replace_lines" => {
            let start = req_int(&a, "start_line", 1)?;
            let end = opt_int(&a, "end_line", 0, usize::MAX)?.ok_or("\"end_line\" is required")?;
            let new = req_str(&a, "text")?.to_string();
            let (cwd, before, after, changes) = modify(host, target(&a)?, false, "replace_lines", |rope| {
                text::replace_lines(rope, start, end, &new).map(|c| vec![c])
            })?;
            let what = if end < start {
                format!("Inserted before line {start}")
            } else {
                format!("Replaced lines {start}-{end}")
            };
            Ok(edit_report(&cwd, &before, &after, &changes, &what))
        }
        "write_buffer" => {
            let content = req_str(&a, "content")?.replace("\r\n", "\n");
            let (cwd, before, after, changes) = modify(host, target(&a)?, true, "write_buffer", |rope| {
                Ok(text::minimal_change(rope, &content).into_iter().collect())
            })?;
            if changes.is_empty() {
                return Ok(ToolOutput::with(
                    format!("{} already has this text; nothing changed.", after.name(&cwd)),
                    json!({ "changed": false }),
                ));
            }
            Ok(edit_report(&cwd, &before, &after, &changes, "Replaced the text"))
        }
        "save_buffer" => save_buffer(host, &a),
        "run_command" => run_command(host, &a),
        "search" => search(host, &a),
        "list_files" => {
            let filter = opt_str(&a, "filter")?.filter(|f| !f.trim().is_empty()).map(Filter::new);
            let limit = opt_int(&a, "limit", 1, 5000)?.unwrap_or(500);
            let cwd = with_editor(host, |ed| ed.cwd.clone())?;
            let (files, truncated) = project::list_files(&cwd, filter.as_ref(), limit);
            let mut text = files.join("\n");
            if files.is_empty() {
                text = "No files match.".into();
            } else if truncated {
                text.push_str("\n[shrew: more files not shown; narrow the filter or raise limit (max 5000)]");
            }
            Ok(ToolOutput::with(text, json!({ "cwd": cwd.to_string_lossy(), "files": files, "truncated": truncated })))
        }
        other => Err(format!("Unknown tool \"{other}\"")),
    }
}

fn list_buffers(host: &dyn Host) -> ToolResult {
    let v = with_editor(host, |ed| {
        let cwd = ed.cwd.clone();
        let list: Vec<Value> = ed
            .buffers
            .iter()
            .enumerate()
            .map(|(i, b)| {
                json!({
                    "id": b.id,
                    "path": b.doc.path.as_ref().map(|p| project::display(&cwd, p)),
                    "name": b.doc.display_name(),
                    "dirty": b.doc.dirty,
                    "language": language(&b.doc),
                    "lines": text::line_count(&b.doc.rope),
                    "active": i == ed.active
                })
            })
            .collect();
        json!({ "cwd": cwd.to_string_lossy(), "buffers": list })
    })?;
    Ok(ToolOutput::data(v))
}

fn get_active_buffer(host: &dyn Host, context: usize) -> ToolResult {
    let (cwd, snap, spans, heads, mode, vim, lang) = with_editor(host, |ed| {
        let heads: Vec<usize> = ed.buf().sel.ranges().iter().map(|r| r.head).collect();
        let primary = ed.buf().sel.primary_index();
        let mut heads_ordered = vec![heads[primary]];
        heads_ordered.extend(heads.iter().enumerate().filter(|(i, _)| *i != primary).map(|(_, h)| *h));
        (
            ed.cwd.clone(),
            Snap::of(ed, ed.active),
            ed.selection_spans(),
            heads_ordered,
            ed.mode.label(),
            ed.vim(),
            language(&ed.buf().doc),
        )
    })?;
    let rope = &snap.rope;
    let at = |pos: usize| {
        let (l, c) = text::line_col(rope, pos);
        json!({ "line": l, "col": c })
    };
    let selections: Vec<Value> = spans
        .iter()
        .map(|&(a, z)| {
            let n = z - a;
            let shown = rope.slice(a..z.min(a + SELECTION_CHARS)).to_string();
            json!({ "start": at(a), "end": at(z), "text": shown, "truncated": n > SELECTION_CHARS })
        })
        .collect();
    let (line, _) = text::line_col(rope, heads[0]);
    let (around, _) = text::numbered(rope, line.saturating_sub(context), line + context, READ_CHARS);
    Ok(ToolOutput::data(json!({
        "id": snap.id,
        "path": snap.path.as_ref().map(|p| project::display(&cwd, p)),
        "dirty": snap.dirty,
        "language": lang,
        "lines": text::line_count(rope),
        "mode": if vim { mode } else { "SUBLIME" },
        "cursor": at(heads[0]),
        "cursors": heads.iter().skip(1).map(|&h| at(h)).collect::<Vec<_>>(),
        "selections": selections,
        "context": around
    })))
}

fn read_buffer(host: &dyn Host, a: &Args) -> ToolResult {
    let start = opt_int(a, "start_line", 1, usize::MAX)?.unwrap_or(1);
    let end = opt_int(a, "end_line", 1, usize::MAX)?.unwrap_or(start.saturating_add(DEFAULT_READ_LINES - 1));
    if end < start {
        return Err("end_line is before start_line".into());
    }
    let t = target(a)?;
    let (cwd, found) = with_editor(host, move |ed| find(ed, &t).map(|f| (ed.cwd.clone(), f)))??;
    let (rope, name, open, dirty, id) = match found {
        Found::Open(s) => {
            let name = s.name(&cwd);
            (s.rope, name, true, s.dirty, Some(s.id))
        }
        Found::Closed(abs) => {
            (Rope::from_str(&project::read_text(&cwd, &abs)?), project::display(&cwd, &abs), false, false, None)
        }
    };
    let total = text::line_count(&rope);
    let meta = |first: usize, last: usize, truncated: bool| json!({ "path": name, "id": id, "open": open, "dirty": dirty, "lines": total, "start_line": first, "end_line": last, "truncated": truncated });
    if total == 0 {
        return Ok(ToolOutput::with(format!("{name} is empty."), meta(0, 0, false)));
    }
    if start > total {
        return Err(format!("start_line {start} is past the end: {name} has {total} lines."));
    }
    let (mut listing, last) = text::numbered(&rope, start, end, READ_CHARS);
    let want = end.min(total);
    let truncated = last < want;
    if last < total && (truncated || end < total) {
        listing
            .push_str(&format!("[shrew: lines {start}-{last} of {total}; pass start_line={} to read on]\n", last + 1));
    }
    if dirty {
        listing.push_str("[shrew: this buffer has unsaved changes; this is the text in the editor, not on disk]\n");
    }
    Ok(ToolOutput::with(listing, meta(start, last, last < total)))
}

/// `open_file` and `goto`: switch to (or open) the buffer and place the cursor.
fn show(
    host: &dyn Host,
    t: Target,
    line: Option<usize>,
    col: Option<usize>,
    end: Option<(usize, usize)>,
    create: bool,
) -> ToolResult {
    let (cwd, found) = with_editor(host, move |ed| find(ed, &t).map(|f| (ed.cwd.clone(), f)))??;
    let doc = match found {
        Found::Open(s) => Err(s.id),
        Found::Closed(abs) => Ok(load(&cwd, &abs, create)?),
    };
    let snap = with_editor(host, move |ed| {
        match doc {
            Ok(doc) => {
                adopt(ed, doc, true);
            }
            Err(id) => match index_of(ed, id) {
                Some(i) => ed.switch_to(i),
                None => return Err("The buffer was closed in shrew.".to_string()),
            },
        }
        if let Some(line) = line {
            place(ed, line, col, end);
        }
        Ok(Snap::of(ed, ed.active))
    })??;
    let name = snap.name(&cwd);
    let lines = text::line_count(&snap.rope);
    let at = line.map(|l| format!(" at line {}", l.min(lines.max(1)))).unwrap_or_default();
    Ok(ToolOutput::with(
        format!("Showing {name}{at} in shrew ({lines} lines{}).", if snap.dirty { ", unsaved changes" } else { "" }),
        json!({ "path": snap.path.as_ref().map(|_| name.clone()), "id": snap.id, "lines": lines, "dirty": snap.dirty }),
    ))
}

fn save_buffer(host: &dyn Host, a: &Args) -> ToolResult {
    let all = opt_bool(a, "all")?;
    let t = target(a)?;
    if all && !matches!(t, Target::Active) {
        return Err("Pass either \"all\" or a path/id.".into());
    }
    let (saved, failed, clean) = with_editor(host, move |ed| {
        let cwd = ed.cwd.clone();
        let picked: Vec<usize> = if all {
            (0..ed.buffers.len()).filter(|&i| ed.buffers[i].doc.dirty).collect()
        } else {
            match find(ed, &t)? {
                Found::Open(s) => vec![index_of(ed, s.id).unwrap_or(ed.active)],
                Found::Closed(abs) => {
                    return Err(format!("{} is not open in shrew; nothing to save.", project::display(&cwd, &abs)));
                }
            }
        };
        let (mut saved, mut failed, mut clean) = (Vec::new(), Vec::new(), Vec::new());
        for i in picked {
            let b = &mut ed.buffers[i];
            let name = b
                .doc
                .path
                .as_ref()
                .map(|p| project::display(&cwd, p))
                .unwrap_or_else(|| format!("[scratch #{}]", b.id));
            let doc = &mut b.doc;
            if !doc.dirty {
                // never rewrite a clean buffer: the file may have been changed on disk since
                clean.push(name);
                continue;
            }
            if doc.path.is_none() {
                failed.push(
                    json!({ "path": name, "error": "no file name: use run_command \"w <path>\" with this buffer" }),
                );
                continue;
            }
            match doc.save() {
                Ok(bytes) => saved.push(json!({ "path": name, "bytes": bytes })),
                Err(e) => failed.push(json!({ "path": name, "error": e.to_string() })),
            }
        }
        if !saved.is_empty() {
            ed.info(format!("saved {} file{} for the assistant", saved.len(), if saved.len() == 1 { "" } else { "s" }));
        }
        Ok((saved, failed, clean))
    })??;
    if saved.is_empty() && failed.is_empty() {
        let text = match clean.as_slice() {
            [one] => format!("Nothing to save: {one} has no unsaved changes."),
            _ => "Nothing to save: no buffer has unsaved changes.".into(),
        };
        return Ok(ToolOutput::with(text, json!({ "saved": [], "failed": [] })));
    }
    if saved.is_empty() {
        let why: Vec<String> = failed
            .iter()
            .map(|f| format!("{}: {}", f["path"].as_str().unwrap_or(""), f["error"].as_str().unwrap_or("")))
            .collect();
        return Err(format!("Nothing was saved. {}", why.join("; ")));
    }
    Ok(ToolOutput::data(json!({ "saved": saved, "failed": failed })))
}

/// Ex commands an assistant may not run: they close shrew, move its sandbox or install software.
const REFUSED: &[&str] =
    &["q", "quit", "close", "q!", "quit!", "qa", "qall", "qa!", "qall!", "wq", "x", "xit", "wqa", "xa", "cd", "update"];
/// Ex commands whose argument is a path to read or write.
const PATH_COMMANDS: &[&str] = &["e", "edit", "o", "open", "w", "write", "w!"];

fn run_command(host: &dyn Host, a: &Args) -> ToolResult {
    let line = req_str(a, "command")?.trim().trim_start_matches(':').trim().to_string();
    if line.is_empty() {
        return Err("\"command\" is empty".into());
    }
    let (cmd, arg) = match line.find(char::is_whitespace) {
        Some(i) => (&line[..i], line[i..].trim()),
        None => (line.as_str(), ""),
    };
    if REFUSED.contains(&cmd) {
        return Err(format!(
            ":{cmd} is not available to assistants (it would close shrew, change its working directory or install software). Ask the user."
        ));
    }
    let path_arg = (PATH_COMMANDS.contains(&cmd) && !arg.is_empty()).then(|| arg.to_string());
    let t = target(a)?;
    let switch = !matches!(t, Target::Active);
    let (message, is_error, cwd, snap) = with_editor(host, move |ed| {
        if let Some(p) = &path_arg {
            project::guard(&ed.cwd, &project::absolute(&ed.cwd, p))?;
        }
        if switch {
            match find(ed, &t)? {
                Found::Open(s) => {
                    let i = index_of(ed, s.id).unwrap_or(ed.active);
                    ed.switch_to(i);
                }
                Found::Closed(abs) => {
                    return Err(format!(
                        "{} is not open in shrew; open it first with open_file.",
                        project::display(&ed.cwd, &abs)
                    ));
                }
            }
        }
        ed.message = None;
        let events = ed.events.len();
        ed.run_ex(&line);
        // belt and braces: nothing an assistant runs may quit shrew
        let mut tail = ed.events.split_off(events);
        tail.retain(|e| *e != Event::Quit);
        ed.events.append(&mut tail);
        let (message, is_error) = ed.message.clone().unwrap_or_default();
        Ok((message, is_error, ed.cwd.clone(), Snap::of(ed, ed.active)))
    })??;
    if is_error {
        return Err(if message.is_empty() { "The command failed.".into() } else { message });
    }
    let name = snap.name(&cwd);
    let text = if message.is_empty() {
        format!("Done (active buffer: {name}).")
    } else {
        format!("{message}\n(active buffer: {name})")
    };
    Ok(ToolOutput::with(
        text,
        json!({ "message": message, "path": snap.path.as_ref().map(|_| name.clone()), "id": snap.id, "dirty": snap.dirty }),
    ))
}

fn search(host: &dyn Host, a: &Args) -> ToolResult {
    let pattern = req_str(a, "pattern")?;
    let re = regex::RegexBuilder::new(pattern)
        .multi_line(true)
        .case_insensitive(opt_bool(a, "ignore_case")?)
        .size_limit(16 * 1024 * 1024)
        .build()
        .map_err(|e| format!("Invalid regex: {e}"))?;
    let limit = opt_int(a, "limit", 1, 1000)?.unwrap_or(100);
    let filter = opt_str(a, "glob")?.filter(|g| !g.trim().is_empty()).map(Filter::new);
    let t = target(a)?;
    let single = match opt_str(a, "scope")? {
        None | Some("project") => !matches!(t, Target::Active),
        Some("buffer") => true,
        Some(other) => return Err(format!("\"scope\" must be \"project\" or \"buffer\", not \"{other}\"")),
    };
    let mut hits: Vec<Hit> = Vec::new();
    let mut truncated;
    if single {
        let (cwd, found) = with_editor(host, move |ed| find(ed, &t).map(|f| (ed.cwd.clone(), f)))??;
        let (name, body) = match found {
            Found::Open(s) => (s.name(&cwd), s.rope.to_string()),
            Found::Closed(abs) => (project::display(&cwd, &abs), project::read_text(&cwd, &abs)?),
        };
        truncated = project::search_text(&name, &body, &re, limit, &mut hits);
    } else {
        let (cwd, open) = with_editor(host, |ed| {
            let open: Vec<(PathBuf, Rope)> =
                ed.buffers.iter().filter_map(|b| Some((b.doc.path.clone()?, b.doc.rope.clone()))).collect();
            (ed.cwd.clone(), open)
        })?;
        // open buffers inside the project are searched in their editor text, the rest on disk
        let inside: Vec<(PathBuf, Rope)> = open.into_iter().filter(|(p, _)| p.starts_with(&cwd)).collect();
        truncated = false;
        for (p, rope) in &inside {
            let name = project::display(&cwd, p);
            if filter.as_ref().is_some_and(|f| !f.matches(&name)) {
                continue;
            }
            let left = limit.saturating_sub(hits.len());
            truncated |= project::search_text(&name, &rope.to_string(), &re, left, &mut hits);
        }
        let skip: Vec<PathBuf> = inside.into_iter().map(|(p, _)| p).collect();
        let left = limit.saturating_sub(hits.len());
        if left > 0 {
            let (more, t) = project::search_files(&cwd, &re, filter.as_ref(), &skip, left);
            hits.extend(more);
            truncated |= t;
        } else {
            truncated = true;
        }
        hits.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    }
    let mut text: String = hits.iter().map(|h| format!("{}:{}:{}: {}\n", h.path, h.line, h.col, h.text)).collect();
    if hits.is_empty() {
        text = "No matches.".into();
    } else if truncated {
        text.push_str(&format!(
            "[shrew: stopped at {limit} hits; narrow the pattern or glob, or raise limit (max 1000)]\n"
        ));
    }
    let list: Vec<Value> =
        hits.iter().map(|h| json!({ "path": h.path, "line": h.line, "col": h.col, "text": h.text })).collect();
    Ok(ToolOutput::with(text, json!({ "hits": list, "truncated": truncated })))
}
