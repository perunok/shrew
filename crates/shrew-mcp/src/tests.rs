use crate::bridge::{Bridge, SendError, Sent, Transport};
use crate::files::{self, Endpoint, Launch};
use crate::tools::{Host, Job, with_editor};
use crate::{McpServer, http, protocol, server};
use serde_json::{Value, json};
use shrew_core::{Editor, Feature};
use std::cell::{Cell, RefCell};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

/// Stands in for the app: an editor owned by its own "UI" thread, fed jobs through a channel, like the real event
/// loop.
struct FakeHost {
    tx: mpsc::Sender<Job>,
    activity: Mutex<Vec<String>>,
}

impl FakeHost {
    fn new(setup: impl FnOnce(&mut Editor) + Send + 'static) -> FakeHost {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            let mut ed = Editor::new();
            setup(&mut ed);
            for job in rx {
                job(&mut ed);
            }
        });
        FakeHost { tx, activity: Mutex::new(Vec::new()) }
    }
}

impl Host for FakeHost {
    fn run(&self, job: Job) -> Result<(), String> {
        self.tx.send(job).map_err(|_| "window closed".to_string())
    }
    fn activity(&self, tool: &str) {
        self.activity.lock().unwrap().push(tool.into());
    }
}

/// No window: jobs are dropped unrun.
struct DeadHost;

impl Host for DeadHost {
    fn run(&self, _job: Job) -> Result<(), String> {
        Ok(())
    }
}

fn ui<R: Send + 'static>(host: &dyn Host, f: impl FnOnce(&mut Editor) -> R + Send + 'static) -> R {
    with_editor(host, f).unwrap()
}

/// A project folder with src/main.rs open in the editor.
fn project() -> (tempfile::TempDir, PathBuf, FakeHost) {
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}\npub fn b() {}\n").unwrap();
    std::fs::write(root.join("notes.md"), "# Notes\nhi there\n").unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(root.join("target/junk.rs"), "fn main() {}\n").unwrap();
    let r = root.clone();
    let host = FakeHost::new(move |ed| {
        ed.cwd = r.clone();
        ed.open(&r.join("src/main.rs")).unwrap();
    });
    (d, root, host)
}

fn call(host: &dyn Host, name: &str, args: Value) -> (bool, String, Value) {
    let msg =
        json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": name, "arguments": args } });
    let r = protocol::handle(host, msg).unwrap();
    let result = &r["result"];
    (
        result["isError"].as_bool().unwrap(),
        result["content"][0]["text"].as_str().unwrap().to_string(),
        result.get("structuredContent").cloned().unwrap_or(Value::Null),
    )
}

fn ok(host: &dyn Host, name: &str, args: Value) -> (String, Value) {
    let (e, text, data) = call(host, name, args.clone());
    assert!(!e, "{name} {args}: {text}");
    (text, data)
}

fn fails(host: &dyn Host, name: &str, args: Value) -> String {
    let (e, text, _) = call(host, name, args.clone());
    assert!(e, "{name} {args} should fail: {text}");
    text
}

fn buffer_text(host: &dyn Host, name: &'static str) -> String {
    ui(host, move |ed| {
        ed.buffers.iter().find(|b| b.doc.display_name() == name).map(|b| b.doc.rope.to_string()).unwrap()
    })
}

// ---------------------------------------------------------------------------
// protocol
// ---------------------------------------------------------------------------

#[test]
fn initialize_negotiates_the_version_and_explains_shrew() {
    let r = protocol::handle(
        &DeadHost,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
    )
    .unwrap();
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(r["result"]["serverInfo"]["name"], "shrew");
    assert_eq!(r["result"]["capabilities"]["tools"]["listChanged"], false);
    assert!(r["result"]["instructions"].as_str().unwrap().contains("get_active_buffer"));
    let r = protocol::handle(
        &DeadHost,
        json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
    )
    .unwrap();
    assert_eq!(r["result"]["protocolVersion"], protocol::PROTOCOL_VERSIONS[0]);
}

#[test]
fn notifications_get_no_answer_and_unknown_methods_an_error() {
    assert!(protocol::handle(&DeadHost, json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
    let r = protocol::handle(&DeadHost, json!({"jsonrpc":"2.0","id":"x","method":"resources/list"})).unwrap();
    assert_eq!(r["error"]["code"], protocol::METHOD_NOT_FOUND);
    assert_eq!(r["id"], "x");
    let r = protocol::handle(&DeadHost, json!({"jsonrpc":"2.0","id":3,"method":"ping"})).unwrap();
    assert_eq!(r["result"], json!({}));
    let r = protocol::handle(&DeadHost, json!(42)).unwrap();
    assert_eq!(r["error"]["code"], protocol::INVALID_REQUEST);
    // batches answer each request, skipping notifications
    let r = protocol::handle(
        &DeadHost,
        json!([{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/x"}]),
    )
    .unwrap();
    assert_eq!(r.as_array().unwrap().len(), 1);
}

#[test]
fn tools_are_listed_with_schemas_and_hints() {
    let list = protocol::tool_list();
    let tools = list["tools"].as_array().unwrap();
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "list_buffers",
            "get_active_buffer",
            "read_buffer",
            "open_file",
            "edit_buffer",
            "replace_lines",
            "write_buffer",
            "save_buffer",
            "run_command",
            "search",
            "list_files",
            "goto"
        ]
    );
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
        assert_eq!(t["inputSchema"]["additionalProperties"], false, "{}", t["name"]);
        assert_eq!(t["annotations"]["openWorldHint"], false);
        let read_only = ["list_buffers", "get_active_buffer", "read_buffer", "search", "list_files"]
            .contains(&t["name"].as_str().unwrap());
        assert_eq!(t["annotations"]["readOnlyHint"], read_only, "{}", t["name"]);
    }
    let write = tools.iter().find(|t| t["name"] == "write_buffer").unwrap();
    assert_eq!(write["annotations"]["destructiveHint"], true);
    assert_eq!(write["inputSchema"]["required"], json!(["content"]));
    let edit = tools.iter().find(|t| t["name"] == "edit_buffer").unwrap();
    assert_eq!(edit["annotations"]["destructiveHint"], false);
    assert!(edit["inputSchema"]["properties"]["path"].is_object());
}

#[test]
fn bad_calls_are_errors_not_crashes() {
    let (_d, _root, host) = project();
    let r = protocol::handle(&host, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rm_rf"}}))
        .unwrap();
    assert_eq!(r["error"]["code"], protocol::INVALID_PARAMS);
    let r = protocol::handle(&host, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}})).unwrap();
    assert_eq!(r["error"]["code"], protocol::INVALID_PARAMS);
    for (tool, args) in [
        ("read_buffer", json!({"path": 5})),
        ("read_buffer", json!({"path": "a", "id": 1})),
        ("read_buffer", json!({"start_line": 0})),
        ("read_buffer", json!({"start_line": 3, "end_line": 2})),
        ("read_buffer", json!("nope")),
        ("read_buffer", json!({"id": 99})),
        ("edit_buffer", json!({"old_string": "", "new_string": "x"})),
        ("edit_buffer", json!({"old_string": "main"})),
        ("search", json!({"pattern": "("})),
        ("search", json!({"pattern": "x", "scope": "galaxy"})),
        ("goto", json!({"line": 1, "end_col": 3})),
        ("save_buffer", json!({"all": true, "path": "x"})),
    ] {
        fails(&host, tool, args);
    }
}

#[test]
fn without_a_window_tools_fail_cleanly() {
    let msg = fails(&DeadHost, "list_buffers", json!({}));
    assert!(msg.contains("not available"), "{msg}");
    struct Closed;
    impl Host for Closed {
        fn run(&self, _job: Job) -> Result<(), String> {
            Err("shrew is closing".into())
        }
    }
    assert!(fails(&Closed, "get_active_buffer", json!({})).contains("closing"));
}

// ---------------------------------------------------------------------------
// reading
// ---------------------------------------------------------------------------

#[test]
fn list_buffers_and_the_active_one() {
    let (_d, root, host) = project();
    let (_, data) = ok(&host, "list_buffers", json!({}));
    assert_eq!(data["cwd"], root.to_string_lossy().as_ref());
    let b = &data["buffers"][0];
    assert_eq!(data["buffers"].as_array().unwrap().len(), 1, "the empty scratch buffer was replaced");
    assert_eq!(
        (b["path"].as_str(), b["language"].as_str(), b["lines"].as_u64()),
        (Some("src/main.rs"), Some("rust"), Some(3))
    );
    assert_eq!((b["dirty"].as_bool(), b["active"].as_bool()), (Some(false), Some(true)));

    let (_, data) = ok(&host, "get_active_buffer", json!({ "context": 1 }));
    assert_eq!(data["path"], "src/main.rs");
    assert_eq!(data["cursor"], json!({"line": 1, "col": 1}));
    assert_eq!(data["mode"], "NORMAL");
    assert_eq!(data["selections"], json!([]));
    assert_eq!(data["context"], "     1\tfn main() {\n     2\t    println!(\"hi\");\n");
    assert_eq!(*host.activity.lock().unwrap(), ["list_buffers", "get_active_buffer"]);
}

#[test]
fn the_selection_is_reported_with_its_text() {
    let (_d, _root, host) = project();
    ok(&host, "goto", json!({"line": 2, "col": 5, "end_line": 2, "end_col": 12}));
    let (_, data) = ok(&host, "get_active_buffer", json!({}));
    assert_eq!(data["mode"], "VISUAL");
    assert_eq!(data["selections"][0]["text"], "println");
    assert_eq!(data["selections"][0]["start"], json!({"line": 2, "col": 5}));
    assert_eq!(data["selections"][0]["end"], json!({"line": 2, "col": 12}));

    // Sublime style (no vim): caret selections, same text
    ui(&host, |ed| ed.set_feature(Feature::Vim, false));
    ok(&host, "goto", json!({"line": 1, "col": 4, "end_line": 1, "end_col": 8}));
    let (_, data) = ok(&host, "get_active_buffer", json!({}));
    assert_eq!(data["mode"], "SUBLIME");
    assert_eq!(data["selections"][0]["text"], "main");
    assert_eq!(data["cursor"], json!({"line": 1, "col": 8}));
}

#[test]
fn read_buffer_lists_numbered_lines_from_the_editor_or_disk() {
    let (_d, root, host) = project();
    let (text, data) = ok(&host, "read_buffer", json!({}));
    assert_eq!(text, "     1\tfn main() {\n     2\t    println!(\"hi\");\n     3\t}\n");
    assert_eq!((data["open"].as_bool(), data["lines"].as_u64()), (Some(true), Some(3)));

    // not open: read from disk, without opening it
    let (text, data) = ok(&host, "read_buffer", json!({"path": "src/lib.rs", "start_line": 2}));
    assert_eq!(text, "     2\tpub fn b() {}\n");
    assert_eq!(data["open"], false);
    assert_eq!(ui(&host, |ed| ed.buffers.len()), 1);
    // absolute paths inside the project work too
    ok(&host, "read_buffer", json!({"path": root.join("notes.md").to_string_lossy()}));

    // a partial read says how to go on
    let (text, data) = ok(&host, "read_buffer", json!({"end_line": 1}));
    assert!(text.contains("pass start_line=2"), "{text}");
    assert_eq!(data["truncated"], true);

    // unsaved edits are what the assistant sees
    ui(&host, |ed| ed.insert_text("// new\n"));
    let (text, data) = ok(&host, "read_buffer", json!({"path": "src/main.rs", "end_line": 1}));
    assert!(text.starts_with("     1\t// new\n"), "{text}");
    assert!(text.contains("unsaved changes"), "{text}");
    assert_eq!(data["dirty"], true);

    assert!(fails(&host, "read_buffer", json!({"start_line": 40})).contains("past the end"));
    assert!(fails(&host, "read_buffer", json!({"path": "missing.rs"})).contains("does not exist"));
    for outside in ["../secret.rs", "/etc/passwd", "src/../../x"] {
        assert!(fails(&host, "read_buffer", json!({ "path": outside })).contains("outside"), "{outside}");
    }
    std::fs::write(root.join("blob.bin"), b"\x00\x01\x02").unwrap();
    assert!(fails(&host, "read_buffer", json!({"path": "blob.bin"})).contains("binary"));
}

// ---------------------------------------------------------------------------
// editing
// ---------------------------------------------------------------------------

#[test]
fn edit_buffer_replaces_exactly_once_as_one_undo_step_without_saving() {
    let (_d, root, host) = project();
    let (text, data) = ok(
        &host,
        "edit_buffer",
        json!({"old_string": "println!(\"hi\")", "new_string": "println!(\"hello\");\n    done()"}),
    );
    assert!(text.contains("Replaced 1 occurrence in src/main.rs at line 2"), "{text}");
    assert!(text.contains("     3\t    done();"), "{text}");
    assert_eq!(
        (data["dirty"].as_bool(), data["saved"].as_bool(), data["line"].as_u64()),
        (Some(true), Some(false), Some(2))
    );
    assert_eq!(buffer_text(&host, "main.rs"), "fn main() {\n    println!(\"hello\");\n    done();\n}\n");
    // not saved
    assert!(std::fs::read_to_string(root.join("src/main.rs")).unwrap().contains("\"hi\""));
    // the user sees a status message, and one undo restores everything
    let msg = ui(&host, |ed| ed.message.clone().unwrap().0);
    assert!(msg.contains("main.rs"), "{msg}");
    ui(&host, |ed| ed.undo());
    assert_eq!(buffer_text(&host, "main.rs"), "fn main() {\n    println!(\"hi\");\n}\n");

    assert!(
        fails(&host, "edit_buffer", json!({"old_string": "nothing like this", "new_string": "x"}))
            .contains("not found")
    );
}

#[test]
fn editing_a_closed_file_opens_it_in_the_background() {
    let (_d, root, host) = project();
    let msg = fails(&host, "edit_buffer", json!({"path": "src/lib.rs", "old_string": "pub fn", "new_string": "fn"}));
    assert!(msg.contains("2 times"), "{msg}");
    let (_, data) = ok(
        &host,
        "edit_buffer",
        json!({"path": "src/lib.rs", "old_string": "pub fn", "new_string": "fn", "replace_all": true}),
    );
    assert_eq!(data["path"], "src/lib.rs");
    assert_eq!(buffer_text(&host, "lib.rs"), "fn a() {}\nfn b() {}\n");
    // still looking at main.rs; lib.rs waits as a dirty buffer
    let (_, data) = ok(&host, "list_buffers", json!({}));
    let bufs = data["buffers"].as_array().unwrap();
    assert_eq!(bufs.len(), 2);
    assert_eq!(bufs.iter().find(|b| b["active"] == true).unwrap()["path"], "src/main.rs");
    assert_eq!(bufs.iter().find(|b| b["path"] == "src/lib.rs").unwrap()["dirty"], true);
    assert_eq!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap(), "pub fn a() {}\npub fn b() {}\n");
    // edits to files outside the project are refused
    let outside = root.parent().unwrap().join("x.rs");
    std::fs::write(&outside, "a").unwrap();
    assert!(
        fails(&host, "edit_buffer", json!({"path": outside.to_string_lossy(), "old_string": "a", "new_string": "b"}))
            .contains("outside")
    );
}

#[test]
fn replace_lines_and_write_buffer() {
    let (_d, root, host) = project();
    ok(&host, "replace_lines", json!({"path": "notes.md", "start_line": 2, "end_line": 2, "text": "bye"}));
    assert_eq!(buffer_text(&host, "notes.md"), "# Notes\nbye\n");
    ok(&host, "replace_lines", json!({"path": "notes.md", "start_line": 3, "end_line": 2, "text": "- one\n- two"}));
    assert_eq!(buffer_text(&host, "notes.md"), "# Notes\nbye\n- one\n- two\n");
    assert!(
        fails(&host, "replace_lines", json!({"path": "notes.md", "start_line": 9, "end_line": 9, "text": ""}))
            .contains("start_line")
    );

    // whole text: only the changed part is replaced, so the cursor on line 1 stays put
    ui(&host, |ed| ed.goto_line(1));
    let (text, _) = ok(&host, "write_buffer", json!({"content": "fn main() {\n    run();\n}\n"}));
    assert!(text.contains("Replaced the text in src/main.rs at line 2"), "{text}");
    assert_eq!(buffer_text(&host, "main.rs"), "fn main() {\n    run();\n}\n");
    let (_, data) = ok(&host, "write_buffer", json!({"content": "fn main() {\n    run();\n}\n"}));
    assert_eq!(data["changed"], false);

    // a new file: a new (unsaved) buffer, written by save_buffer
    ok(&host, "write_buffer", json!({"path": "src/new.rs", "content": "pub const X: u8 = 1;\n"}));
    assert!(!root.join("src/new.rs").exists());
    let (_, data) = ok(&host, "save_buffer", json!({"path": "src/new.rs"}));
    assert_eq!(data["saved"][0]["path"], "src/new.rs");
    assert_eq!(std::fs::read_to_string(root.join("src/new.rs")).unwrap(), "pub const X: u8 = 1;\n");
    assert!(
        fails(&host, "write_buffer", json!({"path": "/tmp/shrew-mcp-nope.rs", "content": "x"})).contains("outside")
    );
}

#[test]
fn save_buffer_writes_one_or_all() {
    let (_d, root, host) = project();
    // a buffer without changes is not rewritten (the file may have changed on disk meanwhile)
    let (text, _) = ok(&host, "save_buffer", json!({}));
    assert!(text.contains("no unsaved changes"), "{text}");
    ok(&host, "edit_buffer", json!({"old_string": "hi", "new_string": "yo"}));
    ok(&host, "edit_buffer", json!({"path": "notes.md", "old_string": "hi there", "new_string": "hello"}));
    let (_, data) = ok(&host, "save_buffer", json!({"all": true}));
    assert_eq!(data["saved"].as_array().unwrap().len(), 2);
    assert!(std::fs::read_to_string(root.join("src/main.rs")).unwrap().contains("\"yo\""));
    assert_eq!(std::fs::read_to_string(root.join("notes.md")).unwrap(), "# Notes\nhello\n");
    let (text, _) = ok(&host, "save_buffer", json!({"all": true}));
    assert!(text.contains("Nothing to save"), "{text}");
    // a scratch buffer has no file to go to
    let id = ui(&host, |ed| {
        let i = ed.add_buffer(shrew_core::Document::from_str(""));
        ed.buffers[i].doc.dirty = true;
        ed.buffers[i].id
    });
    assert!(fails(&host, "save_buffer", json!({ "id": id })).contains("no file name"));
    assert!(fails(&host, "save_buffer", json!({"path": "src/lib.rs"})).contains("not open"));
}

#[test]
fn run_command_runs_ex_commands_but_never_quits() {
    let (_d, _root, host) = project();
    let (text, data) = ok(&host, "run_command", json!({"command": ":%s/hi/yo/g"}));
    assert!(text.contains("1 substitution"), "{text}");
    assert_eq!(data["dirty"], true);
    assert_eq!(buffer_text(&host, "main.rs"), "fn main() {\n    println!(\"yo\");\n}\n");

    let (text, _) = ok(&host, "run_command", json!({"command": "set nonumber"}));
    assert!(text.starts_with("line_numbers off"), "{text}");
    let (text, _) = ok(&host, "run_command", json!({"command": "nohlsearch"}));
    assert!(text.starts_with("Done"), "{text}");
    assert!(!ui(&host, |ed| ed.features.on(Feature::LineNumbers)));

    for cmd in ["q!", "qa", "wq", "cd /", "update"] {
        assert!(fails(&host, "run_command", json!({ "command": cmd })).contains("not available"), "{cmd}");
    }
    assert!(fails(&host, "run_command", json!({"command": "frobnicate"})).contains("not an editor command"));
    assert!(fails(&host, "run_command", json!({"command": "e /etc/passwd"})).contains("outside"));
    assert!(ui(&host, |ed| !ed.events.contains(&shrew_core::Event::Quit)));

    // on another buffer: switches to it first
    ok(&host, "open_file", json!({"path": "notes.md"}));
    ok(&host, "run_command", json!({"path": "src/main.rs", "command": "2"}));
    let (_, data) = ok(&host, "get_active_buffer", json!({}));
    assert_eq!((data["path"].as_str(), data["cursor"]["line"].as_u64()), (Some("src/main.rs"), Some(2)));
}

// ---------------------------------------------------------------------------
// search, files, navigation
// ---------------------------------------------------------------------------

#[test]
fn search_covers_the_project_and_open_buffers() {
    let (_d, _root, host) = project();
    let (text, data) = ok(&host, "search", json!({"pattern": "fn \\w+\\("}));
    let paths: Vec<_> =
        data["hits"].as_array().unwrap().iter().map(|h| h["path"].as_str().unwrap().to_string()).collect();
    assert_eq!(paths, ["src/lib.rs", "src/lib.rs", "src/main.rs"], "{text}");
    assert!(text.contains("src/lib.rs:2:5: pub fn b() {}"), "{text}");

    // the editor's unsaved text wins over the disk
    ui(&host, |ed| ed.insert_text("// zebra\n"));
    let (_, data) = ok(&host, "search", json!({"pattern": "ZEBRA", "ignore_case": true}));
    assert_eq!(data["hits"][0]["path"], "src/main.rs");
    assert_eq!(data["hits"][0]["line"], 1);

    let (_, data) = ok(&host, "search", json!({"pattern": "hi", "glob": "*.md"}));
    assert_eq!(data["hits"].as_array().unwrap().len(), 1);
    let (_, data) = ok(&host, "search", json!({"pattern": "hi", "scope": "buffer"}));
    assert_eq!(data["hits"][0]["path"], "src/main.rs");
    let (_, data) = ok(&host, "search", json!({"pattern": "b", "path": "src/lib.rs"}));
    assert_eq!(data["hits"].as_array().unwrap().len(), 2);
    let (text, data) = ok(&host, "search", json!({"pattern": ".", "limit": 2}));
    assert_eq!(data["hits"].as_array().unwrap().len(), 2);
    assert_eq!(data["truncated"], true);
    assert!(text.contains("stopped at 2"), "{text}");
    let (text, _) = ok(&host, "search", json!({"pattern": "nowhere to be found"}));
    assert_eq!(text, "No matches.");
}

#[test]
fn list_files_respects_gitignore_and_filters() {
    let (_d, _root, host) = project();
    let (text, data) = ok(&host, "list_files", json!({}));
    assert_eq!(data["files"], json!(["notes.md", "src/lib.rs", "src/main.rs"]), "{text}");
    let (_, data) = ok(&host, "list_files", json!({"filter": "src/*.rs"}));
    assert_eq!(data["files"], json!(["src/lib.rs", "src/main.rs"]));
    let (_, data) = ok(&host, "list_files", json!({"filter": "NOTES"}));
    assert_eq!(data["files"], json!(["notes.md"]));
    let (text, data) = ok(&host, "list_files", json!({"limit": 1}));
    assert_eq!(data["truncated"], true);
    assert!(text.contains("more files"), "{text}");
}

#[test]
fn open_file_and_goto_show_places() {
    let (_d, root, host) = project();
    let (text, _) = ok(&host, "open_file", json!({"path": "notes.md", "line": 2}));
    assert!(text.contains("Showing notes.md at line 2"), "{text}");
    let (_, data) = ok(&host, "get_active_buffer", json!({}));
    assert_eq!((data["path"].as_str(), data["cursor"]["line"].as_u64()), (Some("notes.md"), Some(2)));

    ok(&host, "goto", json!({"path": "src/main.rs", "line": 2, "col": 5}));
    let (_, data) = ok(&host, "get_active_buffer", json!({}));
    assert_eq!(data["path"], "src/main.rs");
    assert_eq!(data["cursor"], json!({"line": 2, "col": 5}));
    // no column: the first non-blank, like :2
    ok(&host, "goto", json!({"line": 2}));
    let (_, data) = ok(&host, "get_active_buffer", json!({}));
    assert_eq!(data["cursor"], json!({"line": 2, "col": 5}));

    // a missing file inside the project opens as a new buffer, like :e
    ok(&host, "open_file", json!({"path": "src/fresh.rs"}));
    let (_, data) = ok(&host, "get_active_buffer", json!({}));
    assert_eq!((data["path"].as_str(), data["lines"].as_u64()), (Some("src/fresh.rs"), Some(0)));
    assert!(!root.join("src/fresh.rs").exists());
    assert!(fails(&host, "open_file", json!({"path": "/etc/hostname"})).contains("outside"));
}

// ---------------------------------------------------------------------------
// http endpoint
// ---------------------------------------------------------------------------

fn serve(host: Arc<dyn Host>, token: &str) -> (http::RunningServer, Arc<RwLock<String>>) {
    let token = Arc::new(RwLock::new(token.to_string()));
    let handler: http::Handler = Arc::new(move |msg| protocol::handle(host.as_ref(), msg));
    (http::start(0, token.clone(), handler).unwrap(), token)
}

/// A raw HTTP/1.1 request, so headers (Host, Origin) are exactly what the test says.
fn raw(port: u16, request: &str) -> (u16, String) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    let status = out.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = out.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

fn post(port: u16, token: &str, extra: &str, body: &str) -> (u16, String) {
    raw(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

#[test]
fn endpoint_answers_only_authorised_local_requests() {
    let (_d, _root, host) = project();
    let (server, token) = serve(Arc::new(host), "shrew_secret");
    let port = server.port;
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

    let (status, body) = post(port, "shrew_secret", "", ping);
    assert_eq!(status, 200);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["result"], json!({}));
    // a real tool through HTTP
    let (status, body) = post(
        port,
        "shrew_secret",
        "",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_buffers","arguments":{}}}"#,
    );
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["result"]["structuredContent"]["buffers"][0]["path"],
        "src/main.rs"
    );

    assert_eq!(post(port, "wrong", "", ping).0, 401);
    assert_eq!(post(port, "", "", ping).0, 401);
    assert_eq!(post(port, "shrew_secret", "Origin: https://evil.example\r\n", ping).0, 403);
    assert_eq!(post(port, "shrew_secret", "Origin: null\r\n", ping).0, 403);
    let (status, _) = raw(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: evil.example:{port}\r\nAuthorization: Bearer shrew_secret\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    );
    assert_eq!(status, 403);
    let (status, _) = raw(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: localhost:{}\r\nAuthorization: Bearer shrew_secret\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}",
            port + 1
        ),
    );
    assert_eq!(status, 403);
    let (status, _) = raw(
        port,
        &format!(
            "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer shrew_secret\r\nConnection: close\r\n\r\n"
        ),
    );
    assert_eq!(status, 405);
    let (status, _) = raw(
        port,
        &format!("POST /other HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!(status, 404);

    // notifications: accepted, no body; broken JSON: a parse error
    assert_eq!(post(port, "shrew_secret", "", r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).0, 202);
    let (status, body) = post(port, "shrew_secret", "", "{nope");
    assert_eq!(status, 200);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["error"]["code"], protocol::PARSE_ERROR);

    // a new token locks the old one out at once
    *token.write().unwrap() = "shrew_new".into();
    assert_eq!(post(port, "shrew_secret", "", ping).0, 401);
    assert_eq!(post(port, "shrew_new", "", ping).0, 200);
    server.stop();
}

#[test]
fn a_busy_port_moves_to_the_next_one() {
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = blocker.local_addr().unwrap().port();
    let handler: http::Handler = Arc::new(|_| None);
    let server = http::start(busy, Arc::new(RwLock::new("t".into())), handler).unwrap();
    assert_ne!(server.port, busy);
    assert!(server.port > busy && server.port < busy + http::PORT_ATTEMPTS);
}

// ---------------------------------------------------------------------------
// server lifecycle and files
// ---------------------------------------------------------------------------

#[test]
fn server_publishes_its_endpoint_and_can_change_its_token() {
    let (_d, _root, host) = project();
    let cfg = tempfile::tempdir().unwrap();
    let dir = cfg.path().join("mcp");
    files::save_settings(&dir, &files::Settings { port: 0, token: files::new_token() }).unwrap();
    let mut server = McpServer::start(&dir, Arc::new(host)).unwrap();
    let ep = files::read_endpoint(&dir).unwrap();
    assert_eq!(ep, *server.endpoint());
    assert_eq!(ep.url, format!("http://127.0.0.1:{}/mcp", server.port()));
    assert!(files::read_launch(&dir).enabled);
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    assert_eq!(post(server.port(), &ep.token, "", ping).0, 200);

    let new = server.regenerate_token().unwrap();
    assert_ne!(new, ep.token);
    assert_eq!(post(server.port(), &ep.token, "", ping).0, 401);
    assert_eq!(post(server.port(), &new, "", ping).0, 200);
    assert_eq!(files::read_endpoint(&dir).unwrap().token, new);
    assert_eq!(files::load_settings(&dir).token, new);

    let port = server.port();
    server.stop();
    assert!(files::read_endpoint(&dir).is_none());
    assert!(files::read_launch(&dir).enabled, "shrew closing keeps MCP on for the bridge");
    // tiny_http closes its listener on its own thread, right after
    let closed = (0..200).any(|_| {
        std::thread::sleep(Duration::from_millis(10));
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
    });
    assert!(closed, "the port is still open");
    server::set_enabled(&dir, false).unwrap();
    assert!(!files::read_launch(&dir).enabled);
}

#[test]
fn settings_are_private_and_launch_starts_off() {
    let d = tempfile::tempdir().unwrap();
    assert!(files::read_settings(d.path()).is_none());
    let s = files::load_settings(d.path());
    assert!(s.token.starts_with("shrew_") && s.token.len() == 70);
    assert_eq!(s.port, files::DEFAULT_PORT);
    assert_ne!(s.token, files::new_token());
    files::save_settings(d.path(), &s).unwrap();
    assert_eq!(files::load_settings(d.path()), s);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(d.path().join("settings.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(!files::read_launch(d.path()).enabled);
    // a token that is not ours is replaced
    std::fs::write(d.path().join("settings.json"), r#"{"port": 1, "token": "guessable"}"#).unwrap();
    assert!(files::read_settings(d.path()).is_none());
    // endpoint.json is only removed by the shrew that wrote it
    let a = Endpoint { url: "http://127.0.0.1:1/mcp".into(), token: "shrew_a".into() };
    let b = Endpoint { url: "http://127.0.0.1:2/mcp".into(), token: "shrew_b".into() };
    files::write_endpoint(d.path(), &b).unwrap();
    files::remove_endpoint(d.path(), &a);
    assert_eq!(files::read_endpoint(d.path()), Some(b.clone()));
    files::remove_endpoint(d.path(), &b);
    assert!(files::read_endpoint(d.path()).is_none());
}

// ---------------------------------------------------------------------------
// stdio bridge
// ---------------------------------------------------------------------------

/// Stands in for shrew: `running` decides whether posts get through; starting the app writes endpoint.json.
struct FakeTransport {
    dir: PathBuf,
    running: Cell<bool>,
    token: RefCell<String>,
    starts: Cell<u32>,
    versions: RefCell<Vec<Option<String>>>,
}

impl Transport for FakeTransport {
    fn post(&self, ep: &Endpoint, pv: Option<&str>, body: &str) -> Result<Sent, SendError> {
        if !self.running.get() {
            return Err(SendError::Unreachable);
        }
        if ep.token != *self.token.borrow() {
            return Err(SendError::Unauthorized);
        }
        self.versions.borrow_mut().push(pv.map(String::from));
        let msg: Value = serde_json::from_str(body).unwrap();
        Ok(match protocol::handle(&DeadHost, msg) {
            Some(v) => Sent::Answer(v.to_string()),
            None => Sent::Accepted,
        })
    }
    fn start_app(&self) -> std::io::Result<()> {
        self.starts.set(self.starts.get() + 1);
        self.running.set(true);
        files::write_endpoint(
            &self.dir,
            &Endpoint { url: "http://127.0.0.1:1/mcp".into(), token: self.token.borrow().clone() },
        )
    }
    fn sleep(&self, _d: Duration) {}
}

fn bridge(running: bool) -> (tempfile::TempDir, Bridge<FakeTransport>) {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("mcp");
    let t = FakeTransport {
        dir: dir.clone(),
        running: Cell::new(running),
        token: RefCell::new("shrew_a".into()),
        starts: Cell::new(0),
        versions: RefCell::new(Vec::new()),
    };
    if running {
        files::write_endpoint(&dir, &Endpoint { url: "http://127.0.0.1:1/mcp".into(), token: "shrew_a".into() })
            .unwrap();
    }
    (d, Bridge::new(dir, t))
}

const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;

#[test]
fn bridge_relays_answers_and_swallows_notification_acks() {
    let (_d, mut b) = bridge(true);
    let answer: Value = serde_json::from_str(&b.forward(INIT).unwrap()).unwrap();
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert!(b.forward(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    // the negotiated version goes along with later requests
    b.forward(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#).unwrap();
    assert_eq!(b.transport.versions.borrow().last().unwrap().as_deref(), Some("2025-06-18"));
    assert_eq!(b.transport.starts.get(), 0);
}

#[test]
fn bridge_refuses_to_start_shrew_when_access_is_off() {
    let (_d, mut b) = bridge(false);
    files::write_launch(&b.dir, Launch { enabled: false }).unwrap();
    let answer: Value = serde_json::from_str(&b.forward(INIT).unwrap()).unwrap();
    assert_eq!(answer["id"], 1);
    assert!(answer["error"]["message"].as_str().unwrap().contains("turned off"));
    assert_eq!(b.transport.starts.get(), 0);
    // no launch.json at all: off too
    std::fs::remove_file(b.dir.join("launch.json")).unwrap();
    assert!(b.forward(INIT).unwrap().contains("turned off"));
    // notifications that cannot be delivered get no answer
    assert!(b.forward(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
}

#[test]
fn bridge_starts_shrew_when_it_is_closed() {
    let (_d, mut b) = bridge(false);
    files::write_launch(&b.dir, Launch { enabled: true }).unwrap();
    let answer: Value = serde_json::from_str(&b.forward(INIT).unwrap()).unwrap();
    assert_eq!(answer["result"]["serverInfo"]["name"], "shrew");
    assert_eq!(b.transport.starts.get(), 1);
}

#[test]
fn bridge_picks_up_a_new_token() {
    let (_d, mut b) = bridge(true);
    *b.transport.token.borrow_mut() = "shrew_b".into();
    files::write_endpoint(&b.dir, &Endpoint { url: "http://127.0.0.1:1/mcp".into(), token: "shrew_b".into() }).unwrap();
    assert!(b.forward(INIT).unwrap().contains("protocolVersion"));
}

#[test]
fn bridge_works_end_to_end_over_http() {
    let (_d, _root, host) = project();
    let (server, _token) = serve(Arc::new(host), "shrew_e2e");
    let cfg = tempfile::tempdir().unwrap();
    let dir = cfg.path().join("mcp");
    files::write_endpoint(&dir, &Endpoint { url: server.url(), token: "shrew_e2e".into() }).unwrap();
    let mut b = Bridge::new(dir, crate::bridge::Http::default());
    assert!(b.forward(INIT).unwrap().contains("\"shrew\""));
    let r: Value = serde_json::from_str(
        &b.forward(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_buffer","arguments":{"path":"notes.md"}}}"#).unwrap(),
    )
    .unwrap();
    assert_eq!(r["result"]["content"][0]["text"], "     1\t# Notes\n     2\thi there\n");
    server.stop();
    // shrew closed and access off: a clean JSON-RPC error, not a hang
    let r: Value = serde_json::from_str(&b.forward(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#).unwrap()).unwrap();
    assert!(r["error"]["message"].as_str().unwrap().contains("turned off"), "{r}");
}

#[test]
fn ui_jobs_are_cancelled_when_the_window_does_not_answer() {
    // a host whose UI thread is stuck: jobs queue up and are run only later
    struct Stuck(Mutex<Vec<Job>>);
    impl Host for Stuck {
        fn run(&self, job: Job) -> Result<(), String> {
            self.0.lock().unwrap().push(job);
            Ok(())
        }
    }
    let stuck = Stuck(Mutex::new(Vec::new()));
    let ran = Arc::new(Mutex::new(false));
    let r = ran.clone();
    let err =
        crate::tools::with_editor_timeout(&stuck, Duration::from_millis(50), move |_ed| *r.lock().unwrap() = true)
            .unwrap_err();
    assert!(err.contains("busy"), "{err}");
    // the UI thread wakes up later: the cancelled job does nothing
    let mut ed = Editor::new();
    for job in stuck.0.lock().unwrap().drain(..) {
        job(&mut ed);
    }
    assert!(!*ran.lock().unwrap());

    // picked up in time: the answer comes back
    let stuck = Arc::new(Stuck(Mutex::new(Vec::new())));
    let s = stuck.clone();
    let worker = std::thread::spawn(move || with_editor(s.as_ref(), |ed| ed.buffers.len()));
    let job = loop {
        if let Some(j) = stuck.0.lock().unwrap().pop() {
            break j;
        }
        std::thread::yield_now();
    };
    job(&mut ed);
    assert_eq!(worker.join().unwrap(), Ok(1));
}
