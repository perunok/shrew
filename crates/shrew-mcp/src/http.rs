//! The MCP endpoint: Streamable HTTP in its stateless JSON-response form, `POST http://127.0.0.1:<port>/mcp`.
//!
//! Every request must carry `Authorization: Bearer <token>`, a `Host` naming the loopback address and port, and no
//! `Origin` header (browsers always send one, so web pages and DNS-rebinding attacks are turned away). Bodies are
//! capped. Each request runs on its own thread, so a long project search does not hold up the others; the editor
//! itself is only touched through [`crate::Host`].

use serde_json::Value;
use std::io::{self, Read};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::protocol::{PARSE_ERROR, error_response};

/// Largest request body (a whole-file `write_buffer` of a big file; JSON escaping adds a bit).
pub const MAX_BODY: usize = 64 * 1024 * 1024;
/// When the preferred port is taken, the next few are tried.
pub const PORT_ATTEMPTS: u16 = 10;

/// Answers one parsed JSON-RPC message (None: accepted, nothing to say).
pub type Handler = Arc<dyn Fn(Value) -> Option<Value> + Send + Sync>;

pub struct RunningServer {
    pub port: u16,
    server: Arc<Server>,
    thread: Option<JoinHandle<()>>,
}

impl RunningServer {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Binds 127.0.0.1 on `port` (or one of the next ones; 0 = any free port) and serves until stopped. `token` can be
/// swapped while running (a new token locks old clients out at once).
pub fn start(port: u16, token: Arc<RwLock<String>>, handler: Handler) -> io::Result<RunningServer> {
    let candidates: Vec<u16> = if port == 0 { vec![0] } else { (port..port.saturating_add(PORT_ATTEMPTS)).collect() };
    let mut last_err = None;
    for p in candidates {
        match Server::http(("127.0.0.1", p)) {
            Ok(server) => {
                let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(p);
                let server = Arc::new(server);
                let s = server.clone();
                let thread = std::thread::Builder::new().name("shrew-mcp-http".into()).spawn(move || {
                    for req in s.incoming_requests() {
                        let token = token.clone();
                        let handler = handler.clone();
                        let _ = std::thread::Builder::new()
                            .name("shrew-mcp-req".into())
                            .spawn(move || serve(req, port, &token, &handler));
                    }
                })?;
                return Ok(RunningServer { port, server, thread: Some(thread) });
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        format!("no free port from {port}: {}", last_err.map(|e| e.to_string()).unwrap_or_default()),
    ))
}

fn header<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

fn text(status: u16, body: &str) -> Response<io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(Header::from_bytes("Content-Type", "text/plain; charset=utf-8").unwrap())
}

/// Constant-time comparison, so the token cannot be guessed byte by byte from response times.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Why a request is turned away (status and message), or None when it may proceed.
fn reject(req: &Request, port: u16, token: &str) -> Option<(u16, &'static str)> {
    if req.url().split('?').next() != Some("/mcp") {
        return Some((404, "not found"));
    }
    if header(req, "Origin").is_some() {
        return Some((403, "requests from web pages are not allowed"));
    }
    let host_ok = header(req, "Host").is_some_and(|h| {
        [format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")]
            .iter()
            .any(|ok| h.eq_ignore_ascii_case(ok))
    });
    if !host_ok {
        return Some((403, "bad Host header"));
    }
    if *req.method() != Method::Post {
        return Some((405, "only POST is supported"));
    }
    let bearer = header(req, "Authorization").and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("");
    if token.is_empty() || !same(bearer, token) {
        return Some((401, "missing or wrong token"));
    }
    if req.body_length().is_some_and(|n| n > MAX_BODY) {
        return Some((413, "request too large"));
    }
    None
}

fn serve(mut req: Request, port: u16, token: &RwLock<String>, handler: &Handler) {
    let current = token.read().map(|t| t.clone()).unwrap_or_default();
    if let Some((status, why)) = reject(&req, port, &current) {
        let mut resp = text(status, why);
        if status == 405 {
            resp = resp.with_header(Header::from_bytes("Allow", "POST").unwrap());
        }
        let _ = req.respond(resp);
        return;
    }
    let mut body = Vec::new();
    if req.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut body).is_err() || body.len() > MAX_BODY {
        let _ = req.respond(text(413, "request too large"));
        return;
    }
    let answer = match serde_json::from_slice::<Value>(&body) {
        Ok(msg) => handler(msg),
        Err(e) => Some(error_response(Value::Null, PARSE_ERROR, &format!("invalid JSON: {e}"))),
    };
    let resp = match answer {
        // notifications and responses: accepted, nothing to say
        None => Response::from_data(Vec::new()).with_status_code(202),
        Some(v) => Response::from_data(serde_json::to_vec(&v).unwrap_or_default())
            .with_status_code(200)
            .with_header(Header::from_bytes("Content-Type", "application/json").unwrap()),
    };
    let _ = req.respond(resp);
}
