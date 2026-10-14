//! `shrew --mcp-stdio`: what MCP clients (Claude Code, Claude Desktop, Cursor, ...) launch. It reads JSON-RPC
//! messages from stdin, one per line, posts each to the running shrew's endpoint (found in `endpoint.json`) and
//! writes the answers to stdout. When shrew is not running it starts it (in the client's working directory, so the
//! project is the one the assistant works in), unless MCP access is turned off.

use crate::files::{self, Endpoint};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for a freshly started shrew to answer.
pub const START_TIMEOUT: Duration = Duration::from_secs(30);

pub const TURNED_OFF: &str = "shrew's AI assistant access is turned off. Turn it on in shrew with :set mcp (or `mcp = true` under [features] in the config).";

/// What happened to one forwarded message.
#[derive(Debug, PartialEq)]
pub enum Sent {
    /// The answer to print.
    Answer(String),
    /// Accepted without an answer (a notification).
    Accepted,
}

#[derive(Debug, PartialEq)]
pub enum SendError {
    /// Nothing is listening (shrew is closed, or MCP access was just turned off).
    Unreachable,
    /// The token in endpoint.json is stale (it was regenerated).
    Unauthorized,
    Failed(String),
}

/// How the bridge reaches shrew; a trait so tests can stand in for the network and for starting the app.
pub trait Transport {
    fn post(&self, endpoint: &Endpoint, protocol_version: Option<&str>, body: &str) -> Result<Sent, SendError>;
    fn start_app(&self) -> io::Result<()>;
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d)
    }
}

pub struct Bridge<T: Transport> {
    pub dir: PathBuf,
    pub transport: T,
    protocol_version: Option<String>,
}

impl<T: Transport> Bridge<T> {
    pub fn new(dir: PathBuf, transport: T) -> Self {
        Bridge { dir, transport, protocol_version: None }
    }

    /// Forwards one line; returns what to print (None for notifications).
    pub fn forward(&mut self, line: &str) -> Option<String> {
        let parsed: Option<Value> = serde_json::from_str(line).ok();
        let id = parsed.as_ref().and_then(|v| v.get("id")).cloned();
        let is_initialize = parsed.as_ref().and_then(|v| v.get("method")).and_then(Value::as_str) == Some("initialize");

        let answer = match self.send(line) {
            Ok(Sent::Answer(a)) => a,
            Ok(Sent::Accepted) => return None,
            Err(message) => return id.map(|id| crate::protocol::error_response(id, -32000, &message).to_string()),
        };
        if is_initialize && let Ok(v) = serde_json::from_str::<Value>(&answer) {
            self.protocol_version = v.pointer("/result/protocolVersion").and_then(Value::as_str).map(String::from);
        }
        Some(answer)
    }

    fn try_once(&self, body: &str) -> Result<Sent, SendError> {
        match files::read_endpoint(&self.dir) {
            Some(ep) => self.transport.post(&ep, self.protocol_version.as_deref(), body),
            None => Err(SendError::Unreachable),
        }
    }

    fn send(&mut self, body: &str) -> Result<Sent, String> {
        match self.try_once(body) {
            Ok(s) => Ok(s),
            // the token was regenerated: endpoint.json already has the new one
            Err(SendError::Unauthorized) => self.try_once(body).map_err(|e| describe(&e)),
            Err(SendError::Unreachable) => {
                self.ensure_running()?;
                self.try_once(body).map_err(|e| describe(&e))
            }
            Err(e) => Err(describe(&e)),
        }
    }

    /// Starts shrew (when allowed) and waits until its endpoint answers a ping.
    fn ensure_running(&self) -> Result<(), String> {
        if !files::read_launch(&self.dir).enabled {
            return Err(TURNED_OFF.into());
        }
        self.transport.start_app().map_err(|e| format!("Could not start shrew: {e}"))?;
        let ping = json!({ "jsonrpc": "2.0", "id": "shrew-bridge-ping", "method": "ping" }).to_string();
        let started = Instant::now();
        while started.elapsed() < START_TIMEOUT {
            self.transport.sleep(Duration::from_millis(100));
            if let Some(ep) = files::read_endpoint(&self.dir)
                && matches!(self.transport.post(&ep, None, &ping), Ok(Sent::Answer(_)))
            {
                return Ok(());
            }
            if !files::read_launch(&self.dir).enabled {
                return Err(TURNED_OFF.into());
            }
        }
        Err(format!("shrew was started but did not answer within {} seconds.", START_TIMEOUT.as_secs()))
    }
}

fn describe(e: &SendError) -> String {
    match e {
        SendError::Unreachable => {
            "shrew is not reachable. Is it running with AI assistant access (:set mcp) turned on?".into()
        }
        SendError::Unauthorized => {
            "shrew refused the connection (its access token changed). Restart the assistant's MCP connection.".into()
        }
        SendError::Failed(m) => format!("Talking to shrew failed: {m}"),
    }
}

/// The real transport: plain HTTP to 127.0.0.1, and starting the shrew executable.
pub struct Http {
    agent: ureq::Agent,
}

impl Default for Http {
    fn default() -> Self {
        // No pooled connections: when shrew stops its endpoint, a kept-alive socket would accept the next request
        // and never answer; a fresh connection fails fast with "connection refused" instead.
        let agent = ureq::Agent::config_builder()
            .max_idle_connections(0)
            .max_idle_connections_per_host(0)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(2)))
            .timeout_global(Some(Duration::from_secs(30 * 60)))
            .build()
            .into();
        Http { agent }
    }
}

impl Transport for Http {
    fn post(&self, endpoint: &Endpoint, protocol_version: Option<&str>, body: &str) -> Result<Sent, SendError> {
        let mut req = self
            .agent
            .post(&endpoint.url)
            .header("Authorization", &format!("Bearer {}", endpoint.token))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        if let Some(v) = protocol_version {
            req = req.header("MCP-Protocol-Version", v);
        }
        let mut resp = match req.send(body) {
            Ok(r) => r,
            Err(ureq::Error::Io(e))
                if matches!(e.kind(), io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset) =>
            {
                return Err(SendError::Unreachable);
            }
            Err(ureq::Error::ConnectionFailed) => return Err(SendError::Unreachable),
            // Windows retries a SYN to a closed loopback port for ~2 s instead of refusing at once: a connect that
            // times out on 127.0.0.1 means nothing is listening
            Err(ureq::Error::Timeout(ureq::Timeout::Connect)) => return Err(SendError::Unreachable),
            Err(e) => return Err(SendError::Failed(e.to_string())),
        };
        match resp.status().as_u16() {
            202 => Ok(Sent::Accepted),
            200 => resp
                .body_mut()
                .with_config()
                .limit(256 * 1024 * 1024)
                .read_to_string()
                .map(Sent::Answer)
                .map_err(|e| SendError::Failed(e.to_string())),
            401 => Err(SendError::Unauthorized),
            s => Err(SendError::Failed(format!("HTTP {s}"))),
        }
    }

    fn start_app(&self) -> io::Result<()> {
        // An AppImage runs from a temporary mount that disappears with this process: start the AppImage itself.
        let exe = std::env::var_os("APPIMAGE").map(PathBuf::from).map_or_else(std::env::current_exe, Ok)?;
        let mut cmd = Command::new(exe);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        detach(&mut cmd);
        cmd.spawn().map(|_| ())
    }
}

/// The app must outlive the bridge (the client may kill the bridge's process group when it is done).
fn detach(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

/// Runs the bridge until stdin closes, with `dir` = [`files::mcp_dir`]. Returns the process exit code.
pub fn run(dir: &Path) -> i32 {
    let mut bridge = Bridge::new(dir.to_path_buf(), Http::default());
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(answer) = bridge.forward(&line) {
            // one message per line: the endpoint's JSON has no raw newlines
            if writeln!(stdout, "{}", answer.trim_end()).and_then(|_| stdout.flush()).is_err() {
                return 1;
            }
        }
    }
    0
}
