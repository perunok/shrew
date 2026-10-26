//! Opening a URL with the desktop's handler (`xdg-open`, `open`, the Windows URL handler): the About panel's links
//! and the terminal's Ctrl+click; and showing a file in the file manager (the sidebar's *Reveal in File Manager*).

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

// ----- sidebar: Reveal in File Manager -----

/// Shows `path` in the desktop's file manager, selected where it can be: Finder (`open -R`), Explorer
/// (`/select,`), or on Linux the freedesktop.org `FileManager1.ShowItems` call (Files, Dolphin, Thunar, Nemo...),
/// else the folder opened with `xdg-open`. Runs off the UI thread; failures go to the log.
pub fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    let path = std::path::absolute(path)?;
    let run = |mut c: Command| {
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        c.status().is_ok_and(|s| s.success())
    };
    std::thread::Builder::new().name("shrew-reveal".into()).spawn(move || {
        let ok = if cfg!(target_os = "macos") {
            let mut c = Command::new("open");
            c.arg("-R").arg(&path);
            run(c)
        } else if cfg!(windows) {
            let mut c = Command::new("explorer");
            c.arg(format!("/select,{}", path.display()));
            // Explorer exits with 1 even when it worked
            let _ = run(c);
            true
        } else {
            let mut c = Command::new("dbus-send");
            c.args(["--session", "--print-reply", "--dest=org.freedesktop.FileManager1", "--type=method_call"]);
            c.args(["/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems"]);
            c.arg(format!("array:string:file://{}", uri_path(&path))).arg("string:");
            run(c) || {
                let dir = if path.is_dir() { path.as_path() } else { path.parent().unwrap_or(&path) };
                let mut c = Command::new("xdg-open");
                c.arg(dir);
                run(c)
            }
        };
        if !ok {
            log::warn!("could not show {} in a file manager", path.display());
        }
    })?;
    Ok(())
}

/// A path for a `file://` URI: unreserved bytes as they are, the rest percent-encoded.
fn uri_path(p: &std::path::Path) -> String {
    let mut out = String::new();
    for &b in p.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    out
}

// -----

#[cfg(test)]
mod tests {
    #[test]
    fn file_uris_are_encoded() {
        assert_eq!(super::uri_path(std::path::Path::new("/a b/ü#.rs")), "/a%20b/%C3%BC%23.rs");
    }

    #[test]
    fn only_links() {
        for bad in ["/usr/bin/true", "javascript:x", "-h", "ftp://x"] {
            assert!(super::url(bad).is_err(), "{bad}");
        }
    }
}
