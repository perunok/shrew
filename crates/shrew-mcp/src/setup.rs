//! How to connect assistants to shrew: the text the app shows (e.g. in a scratch buffer on `:mcp-setup`) and the
//! snippets inside it. Clients launch `shrew --mcp-stdio`, which needs no token; the direct HTTP variant does.

use crate::files::Endpoint;
use std::path::{Path, PathBuf};

/// The executable clients should launch: the AppImage itself when running from one (its mount point is temporary),
/// else this executable.
pub fn launcher_exe() -> std::io::Result<PathBuf> {
    std::env::var_os("APPIMAGE").map(PathBuf::from).map_or_else(std::env::current_exe, Ok)
}

/// A path quoted for a shell when it needs it (POSIX single quotes; double quotes on Windows).
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%=,".contains(c)) {
        return s.to_string();
    }
    if cfg!(windows) { format!("\"{}\"", s.replace('"', "\\\"")) } else { format!("'{}'", s.replace('\'', "'\\''")) }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientSetup {
    /// `claude mcp add shrew -- <exe> --mcp-stdio`
    pub claude_code: String,
    /// The `mcpServers` entry for Claude Desktop, Cursor and most other clients.
    pub json: String,
    /// `claude mcp add --transport http ...` with the Bearer token, when the endpoint runs.
    pub claude_code_http: Option<String>,
}

pub fn client_setup(exe: &Path, endpoint: Option<&Endpoint>) -> ClientSetup {
    let exe_s = exe.to_string_lossy();
    let claude_code = format!("claude mcp add shrew -- {} {}", shell_quote(&exe_s), crate::STDIO_FLAG);
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": { "shrew": { "command": exe_s, "args": [crate::STDIO_FLAG] } }
    }))
    .expect("plain JSON");
    let claude_code_http = endpoint.map(|e| {
        format!(
            "claude mcp add --transport http shrew {} --header {}",
            e.url,
            shell_quote(&format!("Authorization: Bearer {}", e.token))
        )
    });
    ClientSetup { claude_code, json, claude_code_http }
}

/// Where Claude Desktop keeps its config on this platform.
fn claude_desktop_config() -> &'static str {
    if cfg!(target_os = "macos") {
        "~/Library/Application Support/Claude/claude_desktop_config.json"
    } else if cfg!(windows) {
        "%APPDATA%\\Claude\\claude_desktop_config.json"
    } else {
        "~/.config/Claude/claude_desktop_config.json"
    }
}

/// A plain-text guide (Markdown) with every snippet filled in. `endpoint`: the running server's, to also show the
/// direct HTTP variant (it contains the token).
pub fn setup_text(exe: &Path, endpoint: Option<&Endpoint>) -> String {
    let s = client_setup(exe, endpoint);
    let mut out = format!(
        "# Connect AI assistants to shrew (MCP)\n\n\
Assistants can then list, read, search and edit the buffers open in shrew. Edits are never saved by themselves and \
each one is a single undo step. Turn access off any time with :set nomcp.\n\n\
## Claude Code\n\nRun once in a terminal:\n\n    {}\n\n\
## Claude Desktop, Cursor and other clients\n\n\
Add this to the client's MCP config (Claude Desktop: {}; Cursor: ~/.cursor/mcp.json), then restart it:\n\n{}\n\n\
The client starts `shrew {}` itself; it talks to the running shrew and opens shrew when it is closed (only while \
access is on).\n",
        s.claude_code,
        claude_desktop_config(),
        s.json.lines().map(|l| format!("    {l}\n")).collect::<String>().trim_end(),
        crate::STDIO_FLAG,
    );
    if let (Some(http), Some(e)) = (&s.claude_code_http, endpoint) {
        out.push_str(&format!(
            "\n## Direct HTTP (advanced)\n\nEndpoint: {}\n\n    {http}\n\n\
The token is a password for your editor: keep it private. A new token (which disconnects clients that use it) is one \
command away in shrew.\n",
            e.url
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_quote_paths_with_spaces() {
        let s = client_setup(Path::new("/home/me/my apps/shrew"), None);
        if !cfg!(windows) {
            assert_eq!(s.claude_code, "claude mcp add shrew -- '/home/me/my apps/shrew' --mcp-stdio");
        }
        let v: serde_json::Value = serde_json::from_str(&s.json).unwrap();
        assert_eq!(v["mcpServers"]["shrew"]["args"][0], "--mcp-stdio");
        assert_eq!(v["mcpServers"]["shrew"]["command"], "/home/me/my apps/shrew");
        assert!(s.claude_code_http.is_none());
        assert_eq!(shell_quote("/usr/bin/shrew"), "/usr/bin/shrew");
    }

    #[test]
    fn the_guide_shows_http_only_with_an_endpoint() {
        let exe = Path::new("/usr/bin/shrew");
        assert!(!setup_text(exe, None).contains("Bearer"));
        let e = Endpoint { url: "http://127.0.0.1:7356/mcp".into(), token: "shrew_abc".into() };
        let t = setup_text(exe, Some(&e));
        assert!(t.contains("claude mcp add shrew -- /usr/bin/shrew --mcp-stdio"));
        assert!(t.contains("--header 'Authorization: Bearer shrew_abc'") || cfg!(windows));
    }
}
