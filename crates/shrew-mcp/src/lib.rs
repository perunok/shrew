//! shrew's MCP server (Model Context Protocol), so AI assistants (Claude Code, Claude Desktop, Cursor, ...) can read
//! and edit what is open in shrew. Off by default.
//!
//! - [`tools`]: the buffer tools and the [`Host`] trait the app implements (run a job on the UI thread).
//! - [`protocol`]: JSON-RPC handling (`initialize`, `tools/list`, `tools/call`, `ping`).
//! - [`http`]: the endpoint inside the app, `POST http://127.0.0.1:<port>/mcp`, token-protected, loopback only.
//! - [`server`]: [`McpServer`], the endpoint plus its files, which is all the app starts and stops.
//! - [`bridge`]: `shrew --mcp-stdio`, the command assistants launch; relays stdin/stdout to the endpoint and starts
//!   shrew when needed.
//! - [`files`]: `settings.json`, `endpoint.json` and `launch.json` in `~/.config/shrew/mcp/`.
//! - [`setup`]: how to connect each client (`claude mcp add ...`, JSON config).
//!
//! Same design as goo's and Slinger's MCP servers: a Streamable HTTP endpoint in the app plus a stdio bridge.

pub mod bridge;
pub mod files;
pub mod http;
mod project;
pub mod protocol;
pub mod server;
pub mod setup;
mod text;
pub mod tools;

pub use server::McpServer;
pub use tools::{Host, Job, with_editor};

/// shrew's version as reported to clients (`serverInfo.version`); the app may set it at startup.
static APP_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_app_version(version: &str) {
    let _ = APP_VERSION.set(version.to_string());
}

pub fn app_version() -> &'static str {
    APP_VERSION.get().map(String::as_str).unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// The argument clients use to start the bridge: `shrew --mcp-stdio`.
pub const STDIO_FLAG: &str = "--mcp-stdio";

#[cfg(test)]
mod tests;
