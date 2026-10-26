//! Restarting into an update without losing anything ("hot exit", as Sublime calls it).
//!
//! Before shrew re-executes itself it writes every open buffer to `<cache dir>/shrew/session-restart.json`: its
//! path, the cursors and selections, the scroll position, the active tab and the project, and the text itself of
//! every buffer with unsaved changes (and of untitled buffers). The new process gets `--restore-session <file>`,
//! reopens everything, puts unsaved text back as unsaved changes on top of the file on disk (one undo step reverts
//! to what is on disk) and deletes the file. Nothing is saved to the files themselves.
//!
//! The file is written atomically (temp file, fsync, rename) before anything else happens; when that fails, shrew
//! does not restart. If the new version never gets to read it, the next shrew that starts picks it up.
//! Views that are not files (diffs, logs, blame) and empty untitled buffers are not kept; terminals cannot be (their
//! shells end with the process); undo history starts afresh.

use crate::state::State;
use serde::{Deserialize, Serialize};
use shrew_core::{Change, ChangeSet, Document, Editor, Range, Selection};
use std::io::Write;
use std::path::{Path, PathBuf};

const FORMAT: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub format: u32,
    /// The version that wrote it (the one updated from).
    pub from: String,
    /// The release page of the version restarted into, for the "What's new" link.
    pub page: Option<String>,
    pub cwd: Option<PathBuf>,
    pub project: Option<PathBuf>,
    pub active: usize,
    pub buffers: Vec<Buf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Buf {
    pub path: Option<PathBuf>,
    /// The text, when it is not what the file on disk says (unsaved changes, untitled buffers).
    pub text: Option<String>,
    pub dirty: bool,
    /// (anchor, head) char positions.
    pub sel: Vec<(usize, usize)>,
    pub primary: usize,
    pub top: usize,
    pub left: usize,
}

/// `<cache dir>/shrew/session-restart.json`.
pub fn path() -> Option<PathBuf> {
    shrew_update::cache_dir().map(|d| d.join("session-restart.json"))
}

/// Everything open in `ed` worth bringing back.
pub fn capture(ed: &Editor, from: &str, page: Option<String>) -> Session {
    let mut buffers = Vec::new();
    let mut active = 0;
    for (i, b) in ed.buffers.iter().enumerate() {
        let doc = &b.doc;
        if doc.title.is_some() || (doc.path.is_none() && doc.is_empty()) {
            continue; // a diff or log view, an empty scratch buffer
        }
        if i == ed.active {
            active = buffers.len();
        }
        let keep_text = doc.dirty || doc.path.is_none();
        buffers.push(Buf {
            path: doc.path.clone(),
            text: keep_text.then(|| doc.rope.to_string()),
            dirty: doc.dirty,
            sel: b.sel.ranges().iter().map(|r| (r.anchor, r.head)).collect(),
            primary: b.sel.primary_index(),
            top: b.view.top,
            left: b.view.left,
        });
    }
    Session {
        format: FORMAT,
        from: from.to_string(),
        page,
        cwd: Some(ed.cwd.clone()),
        project: ed.project.clone(),
        active,
        buffers,
    }
}

/// Writes `s` to `path` durably: a temp file, fsync, rename, fsync of the folder.
pub fn save(path: &Path, s: &Session) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let json = serde_json::to_vec(s).map_err(std::io::Error::other)?;
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    };
    write().inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Reads the session at `path` and removes the file (claimed first by a rename, so two shrews starting together
/// never both restore it). `None` when there is none or it cannot be read (then it is left for a look by hand).
pub fn take(path: &Path) -> Option<Session> {
    let claimed = path.with_extension(format!("claimed{}", std::process::id()));
    std::fs::rename(path, &claimed).ok()?;
    let text = std::fs::read_to_string(&claimed).ok()?;
    match serde_json::from_str::<Session>(&text) {
        Ok(s) => {
            let _ = std::fs::remove_file(&claimed);
            Some(s)
        }
        Err(e) => {
            // keep the text: it may hold unsaved changes
            let keep = path.with_extension("unreadable.json");
            let _ = std::fs::rename(&claimed, &keep);
            log::error!("the restart session {} is unreadable ({e}); kept as {}", path.display(), keep.display());
            None
        }
    }
}

/// Opens the session's buffers in `st` (after whatever is open already). Returns how many came back.
pub fn restore(st: &mut State, s: &Session) -> usize {
    let ed = &mut st.ed;
    if let Some(p) = s.project.as_deref().filter(|p| p.is_dir()) {
        ed.set_project(p);
    } else if let Some(c) = s.cwd.as_deref().filter(|p| p.is_dir()) {
        ed.cwd = c.to_path_buf();
    }
    let mut active = None;
    let mut restored = 0;
    for (i, b) in s.buffers.iter().enumerate() {
        let mut doc = match &b.path {
            Some(p) => match Document::load(p) {
                Ok(d) => d,
                Err(e) => {
                    // unreadable now: keep the text if there is any, under its name
                    log::warn!("restore {}: {e}", p.display());
                    let Some(text) = &b.text else { continue };
                    let mut d = Document::from_str(text);
                    d.path = Some(p.clone());
                    d.dirty = true;
                    d
                }
            },
            None => Document::from_str(b.text.as_deref().unwrap_or_default()),
        };
        if b.path.is_none() {
            doc.dirty = b.dirty;
        }
        let existing = b.path.as_ref().and_then(|p| ed.buffers.iter().position(|x| x.doc.path.as_ref() == Some(p)));
        match existing {
            Some(at) => ed.switch_to(at),
            None => ed.install_document(doc),
        }
        let buf = ed.buf_mut();
        if let (Some(text), Some(_)) = (&b.text, &b.path) {
            let now = buf.doc.rope.to_string();
            if *text != now {
                // the unsaved text over the file: one undo step goes back to what is on disk
                let len = buf.doc.len();
                buf.apply(ChangeSet::single(Change::replace(0, len, text.clone())), None);
            }
            buf.doc.dirty = b.dirty || *text != now;
        }
        let len = buf.doc.len();
        let ranges: Vec<Range> = b.sel.iter().map(|&(a, h)| Range::new(a.min(len), h.min(len))).collect();
        if !ranges.is_empty() {
            buf.sel = Selection::new(ranges, b.primary.min(b.sel.len() - 1)).clamp(len);
        }
        let last = shrew_core::text::last_line(&buf.doc.rope);
        buf.view.top = b.top.min(last);
        buf.view.left = b.left;
        if i == s.active {
            active = Some(ed.active);
        }
        restored += 1;
    }
    if let Some(a) = active {
        ed.switch_to(a);
    }
    st.process_events();
    restored
}

#[cfg(test)]
mod tests {
    use super::*;
    use shrew_core::KeyPress;
    use std::sync::Arc;

    fn state() -> State {
        State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true)
    }

    fn keys(st: &mut State, k: &str) {
        for k in KeyPress::parse_seq(k) {
            st.key(k);
        }
    }

    #[test]
    fn unsaved_changes_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("shrew-session-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, "alpha\nbeta\ngamma\n").unwrap();
        std::fs::write(&b, "one\r\ntwo\r\n").unwrap();

        let mut st = state();
        st.open_at(&a, Some(2), Some(3));
        st.open_at(&b, None, None);
        keys(&mut st, "Aedited<Esc>"); // b.txt: unsaved
        st.ed.install_document(Document::from_str("scratch notes"));
        st.ed.buf_mut().doc.dirty = true;
        st.ed.switch_to(0);
        st.ed.buf_mut().view.top = 1;
        let before: Vec<(Option<PathBuf>, String, bool)> =
            st.ed.buffers.iter().map(|b| (b.doc.path.clone(), b.doc.rope.to_string(), b.doc.dirty)).collect();
        let ends = |s: &Selection| s.ranges().iter().map(|r| (r.anchor, r.head)).collect::<Vec<_>>();
        let sel_a = ends(&st.ed.buffers[0].sel);
        assert_ne!(sel_a, [(0, 0)], "the cursor was moved");

        let session = capture(&st.ed, "0.2.0", Some("https://github.com/perunok/shrew/releases/tag/v0.3.0".into()));
        assert_eq!(session.buffers.len(), 3);
        assert_eq!(session.buffers[0].text, None, "a clean file is read again from disk");
        assert_eq!(session.buffers[1].text.as_deref(), Some("oneedited\ntwo\n"));
        assert!(session.buffers[1].dirty);
        let file = dir.join("session-restart.json");
        save(&file, &session).unwrap();

        // the new process
        let mut st2 = state();
        let s = take(&file).unwrap();
        assert!(!file.exists(), "taken once");
        assert_eq!(take(&file), None);
        assert_eq!(restore(&mut st2, &s), 3);
        let after: Vec<(Option<PathBuf>, String, bool)> =
            st2.ed.buffers.iter().map(|b| (b.doc.path.clone(), b.doc.rope.to_string(), b.doc.dirty)).collect();
        assert_eq!(after, before, "same buffers, same text, same unsaved state");
        assert_eq!(st2.ed.active, 0);
        assert_eq!(ends(&st2.ed.buf().sel), sel_a, "the cursor is where it was");
        assert_eq!(st2.ed.buf().view.top, 1);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "one\r\ntwo\r\n", "nothing was written to the file");

        // undo goes back to the file on disk; saving keeps its CRLF line ends
        st2.ed.switch_to(1);
        keys(&mut st2, "u");
        assert_eq!(st2.ed.buf().doc.rope.to_string(), "one\ntwo\n");
        keys(&mut st2, "<C-r>:w<CR>");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "oneedited\r\ntwo\r\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn views_and_empty_scratch_buffers_are_skipped() {
        let mut st = state();
        let mut view = Document::from_str("diff text");
        view.title = Some("diff".into());
        st.ed.install_document(view);
        let s = capture(&st.ed, "0.2.0", None);
        assert!(s.buffers.is_empty(), "{:?}", s.buffers);
        let mut st2 = state();
        assert_eq!(restore(&mut st2, &s), 0);
        assert_eq!(st2.ed.buffers.len(), 1, "the usual empty scratch buffer");
    }

    #[test]
    fn a_deleted_file_with_unsaved_text_comes_back_unsaved() {
        let dir = std::env::temp_dir().join(format!("shrew-session-gone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gone = dir.join("gone.txt");
        let s = Session {
            format: FORMAT,
            buffers: vec![Buf { path: Some(gone.clone()), text: Some("kept".into()), dirty: true, ..Buf::default() }],
            ..Session::default()
        };
        let mut st = state();
        assert_eq!(restore(&mut st, &s), 1);
        assert_eq!(st.ed.buf().doc.rope.to_string(), "kept");
        assert!(st.ed.buf().doc.dirty);
        assert_eq!(st.ed.buf().doc.path.as_deref(), Some(gone.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unreadable_session_is_kept_aside() {
        let dir = std::env::temp_dir().join(format!("shrew-session-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("session-restart.json");
        std::fs::write(&file, "{\"buffers\": [truncated").unwrap();
        assert_eq!(take(&file), None);
        assert!(dir.join("session-restart.unreadable.json").exists(), "never deleted: it may hold unsaved text");
        std::fs::remove_dir_all(&dir).ok();
    }
}
