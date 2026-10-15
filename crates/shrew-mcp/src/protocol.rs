//! MCP over JSON-RPC 2.0, the small part shrew needs: `initialize`, `ping`, `tools/list`, `tools/call`. Stateless:
//! every request is answered on its own (the HTTP transport runs in JSON-response mode, no sessions, no SSE).

use crate::tools::{self, Host};
use serde_json::{Value, json};

/// Newest first. The client's requested version is used when shrew knows it, otherwise the newest.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub const INSTRUCTIONS: &str = "shrew is the user's text editor (vim and Sublime style). These tools read and edit \
what is open in it, live: the user sees every change at once. Paths are relative to shrew's working directory \
(the project; list_buffers shows it) or absolute; tools that take a path or id use the buffer on screen when both \
are left out. \"This file\" or \"the selection\" means get_active_buffer. Open buffers may hold unsaved edits: \
read_buffer and search see the editor's text, not just the disk. Prefer edit_buffer (exact old_string -> \
new_string, like a careful find-and-replace) for small changes and read the lines first; replace_lines and \
write_buffer for bigger ones. Every edit is one undo step in shrew (the user can press u / Ctrl+Z) and is NOT \
saved: call save_buffer when the user wants it on disk, or leave it for them to review. Edits to files not open yet \
open them in the background. open_file and goto show a place to the user. run_command runs an ex command line \
(\"%s/foo/bar/g\", \"set nonumber\", \"w\") and returns shrew's status message. search and list_files cover the \
project (respecting .gitignore); results are capped.";

/// Added to the instructions when this build has the git tools.
#[cfg(feature = "git")]
const GIT_INSTRUCTIONS: &str = " git_status, git_diff, git_log, git_blame and git_show_commit read the project's git \
repository (read-only: nothing is staged, committed or fetched). They see saved files, except git_blame, which uses \
the text open in shrew; ask git_status first to learn the branch and what changed, and give git_diff and git_show_commit \
a path to keep answers short.";

pub fn instructions() -> String {
    #[cfg(feature = "git")]
    return format!("{INSTRUCTIONS}{GIT_INSTRUCTIONS}");
    #[cfg(not(feature = "git"))]
    INSTRUCTIONS.to_string()
}

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

pub fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn tool_list() -> Value {
    let tools: Vec<Value> = tools::tools()
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": (t.input)(),
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": t.read_only,
                    "destructiveHint": t.destructive,
                    "idempotentHint": t.read_only,
                    "openWorldHint": false
                }
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// Answers one JSON-RPC message (or a batch). None for notifications, which get no answer.
pub fn handle(host: &dyn Host, msg: Value) -> Option<Value> {
    if let Value::Array(batch) = msg {
        let answers: Vec<Value> = batch.into_iter().filter_map(|m| handle(host, m)).collect();
        return (!answers.is_empty()).then_some(Value::Array(answers));
    }
    let Value::Object(m) = &msg else {
        return Some(error_response(Value::Null, INVALID_REQUEST, "expected a JSON-RPC object"));
    };
    let method = m.get("method").and_then(Value::as_str);
    let Some(id) = m.get("id").cloned() else {
        // a notification (`notifications/initialized`, `notifications/cancelled`, ...) or a stray response
        return None;
    };
    let Some(method) = method else {
        return Some(error_response(id, INVALID_REQUEST, "missing method"));
    };
    let params = m.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked.filter(|v| PROTOCOL_VERSIONS.contains(v)).unwrap_or(PROTOCOL_VERSIONS[0]);
            ok(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "shrew", "title": "shrew", "version": crate::app_version() },
                    "instructions": instructions()
                }),
            )
        }
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, tool_list()),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error_response(id, INVALID_PARAMS, "tools/call needs a tool name"));
            };
            if !tools::tools().iter().any(|t| t.name == name) {
                return Some(error_response(id, INVALID_PARAMS, &format!("Unknown tool \"{name}\"")));
            }
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            match tools::call(host, name, &args) {
                Ok(out) => {
                    let mut result = json!({ "content": [{ "type": "text", "text": out.text }], "isError": false });
                    if let Some(s) = out.structured {
                        result["structuredContent"] = s;
                    }
                    ok(id, result)
                }
                Err(message) => ok(id, json!({ "content": [{ "type": "text", "text": message }], "isError": true })),
            }
        }
        _ => error_response(id, METHOD_NOT_FOUND, &format!("shrew does not support \"{method}\"")),
    })
}
