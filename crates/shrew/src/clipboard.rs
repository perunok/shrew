//! The system clipboard (arboard: Wayland data-control, X11, macOS, Windows) behind shrew-core's [`Clipboard`].
//!
//! A dedicated thread owns the arboard handle: it connects lazily (the first yank, not startup), and writes are
//! fire-and-forget so `dd`/`x` never wait for the display server. Reads wait briefly for the reply.

use shrew_core::Clipboard;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

enum Req {
    Set(String),
    Get(Sender<Option<String>>),
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
        }
    }
}

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        let Some(tx) = &self.tx else { return self.last.clone() };
        let (reply, rx) = mpsc::channel();
        if tx.send(Req::Get(reply)).is_err() {
            return self.last.clone();
        }
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Some(t)) => Some(t),
            _ => self.last.clone(),
        }
    }

    fn set(&mut self, text: String) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Req::Set(text.clone()));
        }
        self.last = Some(text);
    }
}
