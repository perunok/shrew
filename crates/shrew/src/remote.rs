//! Requests from `srw` applied to the [`State`]: files (at a line and column), a project folder, stdin text, and
//! `--wait`, which tells the client once every buffer the request opened is closed.

use crate::ipc::Request;
use crate::state::State;
use shrew_core::{Command, Document};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// A `--wait` request still open in the editor.
pub struct Wait {
    targets: Vec<Target>,
    done: Sender<()>,
}

enum Target {
    /// A file: open while a buffer shows it (or it is still loading).
    Path(PathBuf),
    /// The stdin buffer.
    Buffer(u64),
}

impl State {
    /// Opens what a request names; the first file (or the stdin buffer) ends up active. `done` fires once the
    /// buffers are closed.
    pub fn open_request(&mut self, req: Request, done: Option<Sender<()>>) {
        if let Some(dir) = &req.dir {
            self.ed.cwd = self.ed.absolute(dir);
            self.ed.run(Command::OpenFile);
        }
        let mut targets = Vec::new();
        let mut first = None;
        for f in &req.files {
            let path = self.ed.absolute(&f.path);
            self.open_at(&path, f.line, f.col);
            if first.is_none() {
                first = self.ed.buffers.iter().position(|b| b.doc.path.as_deref() == Some(path.as_path()));
            }
            targets.push(Target::Path(path));
        }
        if let Some(text) = req.stdin_text {
            self.ed.install_document(Document::from_str(&text));
            targets.push(Target::Buffer(self.ed.buf().id));
        } else if let Some(i) = first {
            self.ed.switch_to(i);
        }
        self.after_input();
        if let Some(done) = done {
            self.waits.push(Wait { targets, done });
            self.check_waits();
        }
    }

    /// Answers the `--wait` requests whose buffers are all closed.
    pub(crate) fn check_waits(&mut self) {
        if self.waits.is_empty() {
            return;
        }
        let (buffers, loading) = (&self.ed.buffers, &self.loading);
        self.waits.retain(|w| {
            let open = w.targets.iter().any(|t| match t {
                Target::Path(p) => {
                    buffers.iter().any(|b| b.doc.path.as_ref() == Some(p)) || loading.iter().any(|l| &l.path == p)
                }
                Target::Buffer(id) => buffers.iter().any(|b| b.id == *id),
            });
            if !open {
                let _ = w.done.send(());
            }
            open
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::ipc::{FileArg, Request};
    use crate::state::State;
    use shrew_core::KeyPress;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::mpsc;

    fn state() -> State {
        State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true)
    }

    fn keys(st: &mut State, k: &str) {
        for k in KeyPress::parse_seq(k) {
            st.key(k);
        }
    }

    fn src(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src").join(name)
    }

    #[test]
    fn opens_files_at_line_and_column() {
        let mut st = state();
        let req = Request {
            files: vec![
                FileArg { path: src("main.rs"), line: Some(1), col: Some(5) },
                FileArg { path: src("cli.rs"), line: None, col: None },
            ],
            ..Request::default()
        };
        st.open_request(req, None);
        assert_eq!(st.ed.buffers.len(), 2);
        let b = st.ed.buf();
        assert_eq!(b.doc.path.as_deref(), Some(src("main.rs").as_path()), "the first file is active");
        let rope = b.rope();
        let head = b.sel.primary().head;
        assert_eq!(shrew_core::text::line_of(rope, head), 0);
        assert_eq!(head - shrew_core::text::line_start(rope, 0), 4);
    }

    #[test]
    fn stdin_text_becomes_a_buffer() {
        let mut st = state();
        st.open_request(Request { stdin_text: Some("hi\n".into()), ..Request::default() }, None);
        assert_eq!(st.ed.buf().rope().to_string(), "hi\n");
        assert!(st.ed.buf().doc.path.is_none());
    }

    #[test]
    fn a_folder_becomes_the_project() {
        let mut st = state();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        st.open_request(Request { dir: Some(dir.clone()), ..Request::default() }, None);
        assert_eq!(st.ed.cwd, dir);
        if cfg!(feature = "picker") {
            assert!(st.ed.picker.is_some(), "the file picker opens there");
        }
    }

    #[test]
    fn wait_fires_when_its_buffers_close() {
        let mut st = state();
        let (tx, rx) = mpsc::channel();
        let files = vec![FileArg { path: src("main.rs"), line: None, col: None }];
        st.open_request(Request { files, wait: true, stdin_text: Some("x".into()), ..Request::default() }, Some(tx));
        st.open_request(
            Request { files: vec![FileArg { path: src("cli.rs"), ..FileArg::default() }], ..Default::default() },
            None,
        );
        assert_eq!(st.ed.buffers.len(), 3);
        keys(&mut st, ":bd<CR>"); // cli.rs: not part of the wait
        assert!(rx.try_recv().is_err());
        keys(&mut st, ":bd!<CR>"); // the stdin buffer
        assert!(rx.try_recv().is_err(), "main.rs is still open");
        keys(&mut st, ":bd<CR>");
        assert!(rx.try_recv().is_ok(), "all closed: the client is told");
        assert!(st.waits.is_empty());
    }

    #[test]
    fn wait_for_a_new_file_and_for_nothing() {
        let mut st = state();
        let (tx, rx) = mpsc::channel();
        // a new file opens as an empty buffer with that path: still waits for it
        let new = std::env::temp_dir().join(format!("shrew-remote-new-{}.txt", std::process::id()));
        st.open_request(
            Request { files: vec![FileArg { path: new, ..FileArg::default() }], wait: true, ..Default::default() },
            Some(tx),
        );
        assert!(rx.try_recv().is_err());
        // nothing to open at all
        let (tx, rx) = mpsc::channel();
        st.open_request(Request { wait: true, ..Request::default() }, Some(tx));
        assert!(rx.try_recv().is_ok());
    }
}
