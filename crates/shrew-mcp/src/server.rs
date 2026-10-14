//! [`McpServer`]: what the app starts when MCP is turned on and stops when it is turned off or shrew exits. It owns
//! the HTTP endpoint and keeps `endpoint.json` / `launch.json` in step with it.

use crate::files::{self, Endpoint, Launch};
use crate::{Host, http, protocol};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

pub struct McpServer {
    http: Option<http::RunningServer>,
    token: Arc<RwLock<String>>,
    dir: PathBuf,
    endpoint: Endpoint,
}

impl McpServer {
    /// Starts the endpoint on the saved port (or one of the next ones when taken) with the saved token (created and
    /// saved on first use), then writes `endpoint.json` and marks MCP as on in `launch.json`. `dir` is
    /// [`files::mcp_dir`]. Binding and writing a few small files: fast enough for startup, but the app may call it
    /// after the first frame.
    pub fn start(dir: &Path, host: Arc<dyn Host>) -> io::Result<McpServer> {
        let settings = match files::read_settings(dir) {
            Some(s) => s,
            None => {
                let s = files::Settings::default();
                files::save_settings(dir, &s)?;
                s
            }
        };
        let token = Arc::new(RwLock::new(settings.token.clone()));
        let handler: http::Handler = Arc::new(move |msg| protocol::handle(host.as_ref(), msg));
        let server = http::start(settings.port, token.clone(), handler)?;
        let endpoint = Endpoint { url: server.url(), token: settings.token };
        files::write_endpoint(dir, &endpoint)?;
        files::write_launch(dir, Launch { enabled: true })?;
        Ok(McpServer { http: Some(server), token, dir: dir.to_path_buf(), endpoint })
    }

    /// `http://127.0.0.1:<port>/mcp`.
    pub fn url(&self) -> &str {
        &self.endpoint.url
    }

    pub fn port(&self) -> u16 {
        self.http.as_ref().map_or(0, |h| h.port)
    }

    /// The Bearer token clients must send (the bridge reads it from `endpoint.json` by itself).
    pub fn token(&self) -> &str {
        &self.endpoint.token
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// A new token, effective at once: clients configured with the old one are locked out (the stdio bridge picks the
    /// new one up from `endpoint.json` by itself).
    pub fn regenerate_token(&mut self) -> io::Result<String> {
        let token = regenerate_token(&self.dir)?;
        *self.token.write().unwrap_or_else(|e| e.into_inner()) = token.clone();
        self.endpoint.token = token.clone();
        files::write_endpoint(&self.dir, &self.endpoint)?;
        Ok(token)
    }

    /// Stops serving and removes `endpoint.json` (launch.json stays: the bridge may start shrew again later).
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(h) = self.http.take() {
            h.stop();
            files::remove_endpoint(&self.dir, &self.endpoint);
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Records whether MCP is on, for the bridge (which starts shrew only when it is). The app calls this whenever the
/// `mcp` feature is toggled, and at startup to stay in step with a hand-edited config.
pub fn set_enabled(dir: &Path, enabled: bool) -> io::Result<()> {
    if files::read_launch(dir).enabled == enabled && dir.join("launch.json").exists() {
        return Ok(());
    }
    files::write_launch(dir, Launch { enabled })
}

/// Saves a new token (for when the server is not running; a running [`McpServer`] has its own method).
pub fn regenerate_token(dir: &Path) -> io::Result<String> {
    let mut settings = files::load_settings(dir);
    settings.token = files::new_token();
    files::save_settings(dir, &settings)?;
    Ok(settings.token)
}
