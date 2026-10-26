//! The system clipboard (arboard: Wayland data-control, X11, macOS, Windows) behind shrew-core's [`Clipboard`].
//!
//! A dedicated thread owns the arboard handle: it connects lazily (the first yank, not startup), and writes are
//! fire-and-forget so `dd`/`x` never wait for the display server. Reads wait briefly for the reply.
//!
//! On Linux and the BSDs it also serves the primary selection (what is selected; a middle click pastes it), through
//! the same handle. macOS and Windows have none: [`Clipboard::get_primary`] is `None` there.

use shrew_core::Clipboard;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// The systems with a primary selection (where arboard has its Linux backends).
const PRIMARY: bool = cfg!(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))));

enum Req {
    Set(String),
    Get(Sender<Option<String>>),
    SetPrimary(String),
    GetPrimary(Sender<Option<String>>),
}

pub struct SystemClipboard {
    tx: Option<Sender<Req>>,
    /// What we last wrote: the answer when the system clipboard is unreachable.
    last: Option<String>,
}

impl SystemClipboard {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let ok = std::thread::Builder::new().name("shrew-clipboard".into()).spawn(move || serve(rx)).is_ok();
        SystemClipboard { tx: ok.then_some(tx), last: None }
    }

    /// Asks the thread and waits briefly for its answer.
    fn ask(&self, req: impl FnOnce(Sender<Option<String>>) -> Req) -> Option<Option<String>> {
        let tx = self.tx.as_ref()?;
        let (reply, rx) = mpsc::channel();
        tx.send(req(reply)).ok()?;
        rx.recv_timeout(Duration::from_millis(500)).ok()
    }
}

fn serve(rx: Receiver<Req>) {
    let mut cb: Option<arboard::Clipboard> = None;
    let mut failed = false;
    while let Ok(req) = rx.recv() {
        if cb.is_none() && !failed {
            match arboard::Clipboard::new() {
                Ok(c) => cb = Some(c),
                Err(e) => {
                    log::warn!("system clipboard unavailable: {e}");
                    failed = true;
                }
            }
        }
        match req {
            Req::Set(t) => {
                if let Some(c) = &mut cb
                    && let Err(e) = c.set_text(t)
                {
                    log::warn!("clipboard write failed: {e}");
                }
            }
            Req::Get(reply) => {
                let _ = reply.send(cb.as_mut().and_then(|c| c.get_text().ok()));
            }
            Req::SetPrimary(t) => {
                if let Some(c) = &mut cb {
                    primary::set(c, t);
                }
            }
            Req::GetPrimary(reply) => {
                let _ = reply.send(cb.as_mut().and_then(primary::get));
            }
        }
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))))]
mod primary {
    use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

    pub fn set(c: &mut arboard::Clipboard, t: String) {
        // a compositor without primary-selection support says so on every selection: only log it quietly
        if let Err(e) = c.set().clipboard(LinuxClipboardKind::Primary).text(t) {
            log::debug!("primary selection write failed: {e}");
        }
    }

    pub fn get(c: &mut arboard::Clipboard) -> Option<String> {
        c.get().clipboard(LinuxClipboardKind::Primary).text().ok()
    }
}

#[cfg(not(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "emscripten")))))]
mod primary {
    pub fn set(_: &mut arboard::Clipboard, _: String) {}

    pub fn get(_: &mut arboard::Clipboard) -> Option<String> {
        None
    }
}

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        match self.ask(Req::Get) {
            Some(Some(t)) => Some(t),
            _ => self.last.clone(),
        }
    }

    fn set(&mut self, text: String) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Req::Set(text.clone()));
        }
        self.last = Some(text);
    }

    fn get_primary(&mut self) -> Option<String> {
        if !PRIMARY {
            return None;
        }
        self.ask(Req::GetPrimary).flatten().filter(|t| !t.is_empty())
    }

    fn set_primary(&mut self, text: String) {
        if let Some(tx) = self.tx.as_ref().filter(|_| PRIMARY) {
            let _ = tx.send(Req::SetPrimary(text));
        }
    }
}
