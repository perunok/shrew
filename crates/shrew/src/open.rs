//! Opening a URL with the desktop's handler (`xdg-open`, `open`, the Windows URL handler): the About panel's links
//! and the terminal's Ctrl+click.

use std::process::{Command, Stdio};

/// Schemes passed on to the desktop: never a bare word, which the opener could take for an option or a program.
const SCHEMES: [&str; 4] = ["https://", "http://", "file://", "mailto:"];

/// Opens `url` with the desktop's handler (a browser, for the web), detached.
pub fn url(url: &str) -> std::io::Result<()> {
    if !SCHEMES.iter().any(|s| url.starts_with(s)) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a link"));
    }
    let mut cmd = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        // not `cmd /C start`, which would take the `&` of a query string as a command separator
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        Command::new("xdg-open")
    };
    let mut child = cmd.arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    // reap it without blocking the UI
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_links() {
        for bad in ["/usr/bin/true", "javascript:x", "-h", "ftp://x"] {
            assert!(super::url(bad).is_err(), "{bad}");
        }
    }
}
