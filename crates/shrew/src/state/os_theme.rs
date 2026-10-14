//! The desktop's light/dark preference, for `theme = "system"`. winit reports it on macOS and Windows (the app
//! forwards `Window::theme` and `ThemeChanged`); on Linux winit does not, so this asks the XDG desktop portal
//! (`org.freedesktop.appearance color-scheme`) through `gdbus`, on a background thread, and keeps a `gdbus monitor`
//! running to hear about changes. The monitor sleeps in a blocking read: no polling, no idle CPU. A quick look at
//! GTK/KDE settings files gives the first frame a good guess before the portal answers.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

/// A best guess without asking anyone: `GTK_THEME`, GTK's settings.ini, KDE's color scheme. Reads at most a few
/// small files.
pub fn guess_dark() -> Option<bool> {
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return None;
    }
    if let Ok(t) = std::env::var("GTK_THEME") {
        return Some(t.to_ascii_lowercase().contains("dark"));
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    for gtk in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
        if let Ok(text) = std::fs::read_to_string(config.join(gtk)) {
            for line in text.lines() {
                let Some((k, v)) = line.split_once('=') else { continue };
                let v = v.trim().to_ascii_lowercase();
                match k.trim() {
                    "gtk-application-prefer-dark-theme" => return Some(v == "1" || v == "true"),
                    "gtk-theme-name" if v.contains("dark") => return Some(true),
                    _ => {}
                }
            }
        }
    }
    let kde = std::fs::read_to_string(config.join("kdeglobals")).ok()?;
    let scheme = kde.lines().find_map(|l| l.strip_prefix("ColorScheme="))?;
    Some(scheme.to_ascii_lowercase().contains("dark"))
}

/// Watches the portal's color-scheme setting. Dropping it stops the monitor process.
pub struct Watcher {
    pub rx: Receiver<bool>,
    /// The monitor process, and whether the watcher was dropped (so a monitor started late is stopped at once).
    child: Arc<Mutex<(Option<Child>, bool)>>,
}

fn stop(child: &Mutex<(Option<Child>, bool)>) {
    let mut g = child.lock().unwrap_or_else(|e| e.into_inner());
    g.1 = true;
    if let Some(mut c) = g.0.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        stop(&self.child);
    }
}

const DEST: [&str; 4] =
    ["--dest", "org.freedesktop.portal.Desktop", "--object-path", "/org/freedesktop/portal/desktop"];

/// `1` prefers dark, `2` prefers light, `0` no preference.
fn parse_scheme(line: &str) -> Option<bool> {
    let i = line.find("uint32")?;
    let n: u32 =
        line[i + 6..].trim_start().chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()?;
    match n {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// Starts asking the portal (Linux and BSDs only; `None` elsewhere). `wake` runs after each answer.
pub fn watch(wake: impl Fn() + Send + 'static) -> Option<Watcher> {
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    let child: Arc<Mutex<(Option<Child>, bool)>> = Arc::default();
    let slot = child.clone();
    std::thread::Builder::new()
        .name("shrew-appearance".into())
        .spawn(move || {
            let read = Command::new("gdbus")
                .args(["call", "--session"])
                .args(DEST)
                .args([
                    "--method",
                    "org.freedesktop.portal.Settings.Read",
                    "org.freedesktop.appearance",
                    "color-scheme",
                ])
                .stderr(Stdio::null())
                .output();
            let Ok(out) = read else { return }; // no gdbus: keep the guess
            if let Some(dark) = parse_scheme(&String::from_utf8_lossy(&out.stdout)) {
                if tx.send(dark).is_err() {
                    return;
                }
                wake();
            }
            let spawned = Command::new("gdbus")
                .args(["monitor", "--session"])
                .args(DEST)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            let Ok(mut c) = spawned else { return };
            let Some(stdout) = c.stdout.take() else { return };
            {
                let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
                g.0 = Some(c);
                if g.1 {
                    drop(g);
                    return stop(&slot);
                }
            }
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if !line.contains("SettingChanged") || !line.contains("color-scheme") {
                    continue;
                }
                if let Some(dark) = parse_scheme(&line) {
                    if tx.send(dark).is_err() {
                        break;
                    }
                    wake();
                }
            }
            stop(&slot);
        })
        .ok()?;
    Some(Watcher { rx, child })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_answers() {
        assert_eq!(parse_scheme("(<<uint32 1>>,)"), Some(true));
        assert_eq!(parse_scheme("(<<uint32 2>>,)"), Some(false));
        assert_eq!(parse_scheme("(<<uint32 0>>,)"), None);
        let sig = "/org/freedesktop/portal/desktop: org.freedesktop.portal.Settings.SettingChanged ('org.freedesktop.appearance', 'color-scheme', <uint32 1>)";
        assert_eq!(parse_scheme(sig), Some(true));
    }
}
