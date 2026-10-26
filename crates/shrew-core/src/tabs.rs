//! The buffer tabs' commands: closing several buffers at once (each unsaved one asks first, on the command line:
//! save, discard or cancel), moving a buffer along the tab row, copying a buffer's path and revealing its file in
//! the sidebar. They take buffer ids rather than indices, so a question answered later still means the same buffer;
//! the app's tab row (`crates/shrew/src/tabs.rs`) calls them with the right-clicked tab, the commands with the
//! active one.

use crate::document::{Buffer, Document};
use crate::editor::{Editor, Event, PromptKind};
use crate::{Command, Key, KeyPress, Mode};
use std::path::PathBuf;

impl Editor {
    /// Where the buffer with this id is in the tab row.
    pub fn buffer_index(&self, id: u64) -> Option<usize> {
        self.buffers.iter().position(|b| b.id == id)
    }

    /// Closes the buffers `ids`, in order. A buffer with unsaved changes becomes the active one and asks first
    /// ("save / discard / cancel"); cancelling keeps it and every buffer after it.
    pub fn close_buffers(&mut self, ids: Vec<u64>) {
        if self.prompt.as_ref().is_some_and(|p| p.kind == PromptKind::CloseUnsaved) {
            self.prompt = None;
        }
        self.closing = ids;
        self.close_pending();
    }

    /// Closes what is queued up to the first unsaved buffer, which asks.
    fn close_pending(&mut self) {
        while let Some(&id) = self.closing.first() {
            let Some(i) = self.buffer_index(id) else {
                self.closing.remove(0);
                continue;
            };
            if self.buffers[i].doc.dirty {
                self.switch_to(i);
                let name = self.buf().doc.display_name();
                self.open_prompt(PromptKind::CloseUnsaved);
                if let Some(p) = self.prompt.as_mut() {
                    p.label = format!("Save changes to {name}? (s)ave (d)iscard (c)ancel ");
                }
                return;
            }
            self.closing.remove(0);
            self.remove_buffer(i);
        }
    }

    /// The answer to "save changes?": `s`/`y` saves and closes, `d`/`n` closes without saving, `c`, Esc or `q`
    /// cancels (the rest stays open too). Other keys leave the question up.
    pub(crate) fn unsaved_answer(&mut self, k: KeyPress) {
        let cancel = matches!(k.key, Key::Esc | Key::Char('c' | 'C' | 'q' | 'Q'))
            || (k.mods.ctrl && matches!(k.key, Key::Char('c' | '[')));
        let save = matches!(k.key, Key::Char('s' | 'S' | 'y' | 'Y')) && !k.mods.ctrl;
        let discard = matches!(k.key, Key::Char('d' | 'D' | 'n' | 'N')) && !k.mods.ctrl;
        if !(cancel || save || discard) {
            return; // the question stays
        }
        self.prompt = None;
        let id = self.closing.first().copied();
        let Some(i) = id.and_then(|id| self.buffer_index(id)) else {
            self.closing.clear();
            return;
        };
        if cancel {
            self.closing.clear();
            return;
        }
        if save {
            self.switch_to(i);
            self.run(Command::Save);
            if self.buf().doc.dirty {
                // it could not be written (no file name, a read-only folder): it stays, with the reason shown
                self.closing.clear();
                return;
            }
        }
        self.closing.remove(0);
        self.remove_buffer(i);
        self.close_pending();
    }

    /// Closes buffer `i` whatever its state (remembered for Reopen Closed). The last buffer becomes an empty scratch
    /// buffer; closing the active one activates its right neighbour (the left one at the end).
    pub fn remove_buffer(&mut self, i: usize) {
        if i >= self.buffers.len() {
            return;
        }
        let active_id = self.buf().id;
        // `remember_closed` reads the active buffer
        self.active = i;
        self.remember_closed();
        if self.buffers.len() == 1 {
            let id = self.buffers[0].id;
            self.buffers[0] = Buffer::new(id, Document::default());
            self.active = 0;
            self.events.push(Event::BufferOpened(id));
            return;
        }
        self.buffers.remove(i);
        match self.buffer_index(active_id) {
            Some(j) => self.active = j,
            None => {
                self.active = i.min(self.buffers.len() - 1);
                self.pending.clear();
                self.set_mode(if self.vim() { Mode::Normal } else { Mode::Insert });
            }
        }
    }

    /// Moves buffer `from` to position `to` in the tab row; the active buffer stays active.
    pub fn move_buffer(&mut self, from: usize, to: usize) {
        let n = self.buffers.len();
        if from >= n || from == to.min(n - 1) {
            return;
        }
        let active = self.buf().id;
        let b = self.buffers.remove(from);
        self.buffers.insert(to.min(n - 1), b);
        self.active = self.buffer_index(active).unwrap_or(0);
    }

    /// A tab command for buffer `id` (the active one from the palette or a key, the right-clicked one from a tab's
    /// menu).
    pub fn tab_command(&mut self, cmd: Command, id: u64) {
        let Some(i) = self.buffer_index(id) else { return };
        let ids = |r: &mut dyn Iterator<Item = &Buffer>| r.map(|b| b.id).collect::<Vec<u64>>();
        match cmd {
            Command::CloseBuffer => self.close_buffers(vec![id]),
            Command::CloseOthers => {
                self.switch_to(i);
                let others = ids(&mut self.buffers.iter().filter(|b| b.id != id));
                self.close_buffers(others);
            }
            Command::CloseRight => {
                let right = ids(&mut self.buffers.iter().skip(i + 1));
                if self.active > i {
                    self.switch_to(i);
                }
                self.close_buffers(right);
            }
            Command::CloseSaved => {
                let saved = ids(&mut self.buffers.iter().filter(|b| !b.doc.dirty));
                self.close_buffers(saved);
            }
            Command::CloseAll => {
                let all = ids(&mut self.buffers.iter());
                self.close_buffers(all);
            }
            Command::MoveTabLeft if i > 0 => self.move_buffer(i, i - 1),
            Command::MoveTabRight => self.move_buffer(i, i + 1),
            Command::CopyPath | Command::CopyRelativePath => self.copy_path(i, cmd == Command::CopyRelativePath),
            Command::RevealInSidebar => self.reveal_in_sidebar(i),
            _ => {}
        }
    }

    /// The file of buffer `i`, or an error saying it has none.
    pub fn file_of(&mut self, i: usize) -> Option<PathBuf> {
        let b = self.buffers.get(i)?;
        let p = b.doc.path.clone();
        if p.is_none() {
            let name = b.doc.display_name();
            self.error(format!("{name} is not a file"));
        }
        p
    }

    /// Copies buffer `i`'s path to the clipboard: absolute, or relative to the project (or working directory).
    fn copy_path(&mut self, i: usize, relative: bool) {
        let Some(p) = self.file_of(i) else { return };
        let s = if relative {
            // that file's project, which need not be the active buffer's
            let base = match &self.project {
                Some(root) if p.starts_with(root) => root.clone(),
                _ => crate::project::find_root(&p).unwrap_or_else(|| self.cwd.clone()),
            };
            crate::editor::relative(&p, &base)
        } else {
            p.display().to_string()
        };
        self.clipboard.set(s.clone());
        self.info(format!("copied {s}"));
    }

    /// Shows the sidebar with buffer `i`'s file selected in it (the buffer becomes the active one).
    fn reveal_in_sidebar(&mut self, i: usize) {
        let Some(p) = self.file_of(i) else { return };
        self.switch_to(i);
        self.focus_sidebar(true);
        if let Some(t) = self.tree.as_mut()
            && !t.reveal(&p)
        {
            self.error(format!("{} is outside the project", p.display()));
        }
    }

    /// `:tabm[ove] N` (to position N, 0 = first; no N = last), `:tabm +N` / `:tabm -N` (along by N).
    pub(crate) fn tab_move_ex(&mut self, arg: &str) {
        let (i, n) = (self.active, self.buffers.len());
        // `+` alone is `+1`
        let by = |s: &str| if s.is_empty() { Some(1) } else { s.parse::<usize>().ok() };
        let arg = arg.trim();
        let to = if arg.is_empty() || arg == "$" {
            Some(n - 1)
        } else if let Some(d) = arg.strip_prefix('+') {
            by(d).map(|d| i + d)
        } else if let Some(d) = arg.strip_prefix('-') {
            by(d).map(|d| i.saturating_sub(d))
        } else {
            arg.parse::<usize>().ok()
        };
        match to {
            Some(to) => self.move_buffer(i, to.min(n - 1)),
            None => self.error(format!(":tabmove {arg}: expected N, +N or -N")),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Command, Document, Editor, KeyPress};

    fn keys(ed: &mut Editor, s: &str) {
        for k in KeyPress::parse_seq(s) {
            ed.handle_key(k);
        }
    }

    /// Scratch buffers named by their text ("a", "b", ...); `dirty` ones have unsaved changes.
    fn tabs(names: &[&str], dirty: &[&str]) -> Editor {
        let mut ed = Editor::new();
        for n in names {
            let mut d = Document::from_str(n);
            d.title = Some((*n).to_string());
            d.dirty = dirty.contains(n);
            ed.install_document(d);
        }
        ed
    }

    fn names(ed: &Editor) -> Vec<String> {
        ed.buffers.iter().map(|b| b.doc.display_name()).collect()
    }

    fn active(ed: &Editor) -> String {
        ed.buf().doc.display_name()
    }

    #[test]
    fn close_others_right_saved_and_all() {
        let mut ed = tabs(&["a", "b", "c", "d"], &[]);
        let b = ed.buffers[1].id;
        ed.tab_command(Command::CloseRight, b);
        assert_eq!(names(&ed), ["a", "b"]);
        assert_eq!(active(&ed), "b", "the active tab was closed: the clicked one takes over");
        let mut ed = tabs(&["a", "b", "c", "d"], &[]);
        let c = ed.buffers[2].id;
        ed.tab_command(Command::CloseOthers, c);
        assert_eq!(names(&ed), ["c"]);
        let mut ed = tabs(&["a", "b", "c"], &["b"]);
        ed.run(Command::CloseSaved);
        assert_eq!(names(&ed), ["b"], "unsaved ones stay, without a question");
        assert!(ed.prompt.is_none());
        let mut ed = tabs(&["a", "b"], &[]);
        ed.run(Command::CloseAll);
        assert_eq!(ed.buffers.len(), 1);
        assert!(ed.buf().doc.path.is_none() && ed.buf().doc.title.is_none(), "an empty scratch buffer is left");
    }

    #[test]
    fn closing_an_unsaved_buffer_asks_save_discard_or_cancel() {
        let mut ed = tabs(&["a", "b", "c"], &["b"]);
        ed.run(Command::CloseAll);
        // a closed, then b asks
        assert_eq!(names(&ed), ["b", "c"]);
        assert_eq!(active(&ed), "b");
        let label = &ed.prompt.as_ref().unwrap().label;
        assert!(label.contains("Save changes to b?") && label.contains("(d)iscard"), "{label}");
        // other keys leave the question up
        keys(&mut ed, "x<CR>");
        assert!(ed.prompt.is_some());
        assert_eq!(names(&ed), ["b", "c"]);
        // cancel keeps it and the rest
        keys(&mut ed, "c");
        assert!(ed.prompt.is_none());
        assert_eq!(names(&ed), ["b", "c"]);
        // discard closes it and goes on
        ed.run(Command::CloseAll);
        keys(&mut ed, "d");
        assert_eq!(ed.buffers.len(), 1);
        assert!(ed.prompt.is_none());
        assert!(!ed.buf().doc.dirty);
        // Esc cancels too
        let mut ed = tabs(&["a", "b"], &["a", "b"]);
        let a = ed.buffers[1].id;
        ed.close_buffers(vec![a]);
        keys(&mut ed, "<Esc>");
        assert_eq!(names(&ed), ["a", "b"]);
    }

    #[test]
    fn saving_from_the_question_writes_the_file_and_closes_it() {
        let dir = std::env::temp_dir().join(format!("shrew-tabs-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.txt");
        std::fs::write(&path, "old\n").unwrap();
        let mut ed = Editor::new();
        ed.open(&path).unwrap();
        keys(&mut ed, "ccnew<Esc>");
        assert!(ed.buf().doc.dirty);
        // Ctrl+W outside vim asks too; s saves
        keys(&mut ed, ":set novim<CR><C-w>");
        assert!(ed.prompt.is_some());
        keys(&mut ed, "s");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
        assert!(ed.buf().doc.path.is_none(), "closed");
        // a scratch buffer cannot be saved without a name: it stays, and says why
        let mut ed = tabs(&["a"], &["a"]);
        ed.run(Command::CloseBuffer);
        keys(&mut ed, "s");
        assert_eq!(names(&ed), ["a"]);
        assert!(ed.message.as_ref().is_some_and(|m| m.1), "{:?}", ed.message);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn moving_tabs_keeps_the_active_one() {
        let mut ed = tabs(&["a", "b", "c"], &[]);
        assert_eq!(active(&ed), "c");
        ed.run(Command::MoveTabLeft);
        assert_eq!(names(&ed), ["a", "c", "b"]);
        assert_eq!(active(&ed), "c");
        keys(&mut ed, "<C-S-PageUp>");
        assert_eq!(names(&ed), ["c", "a", "b"]);
        ed.run(Command::MoveTabLeft);
        assert_eq!(names(&ed), ["c", "a", "b"], "the first stays first");
        keys(&mut ed, ":tabm<CR>");
        assert_eq!(names(&ed), ["a", "b", "c"]);
        keys(&mut ed, ":tabm 0<CR>");
        assert_eq!(names(&ed), ["c", "a", "b"]);
        keys(&mut ed, ":tabm +1<CR>");
        assert_eq!(names(&ed), ["a", "c", "b"]);
        // moving another tab: the active one follows its buffer
        ed.move_buffer(0, 2);
        assert_eq!(names(&ed), ["c", "b", "a"]);
        assert_eq!(active(&ed), "c");
        keys(&mut ed, ":tabo<CR>");
        assert_eq!(names(&ed), ["c"]);
    }

    #[test]
    fn copy_paths() {
        let mut ed = Editor::new();
        ed.cwd = std::env::temp_dir();
        ed.project = Some(ed.cwd.clone());
        let path = ed.cwd.join("shrew-tabs-copy").join("f.rs");
        let mut d = Document::from_str("x");
        d.path = Some(path.clone());
        ed.install_document(d);
        ed.run(Command::CopyPath);
        assert_eq!(ed.clipboard.get().as_deref(), Some(path.display().to_string().as_str()));
        ed.run(Command::CopyRelativePath);
        let rel = std::path::Path::new("shrew-tabs-copy").join("f.rs");
        assert_eq!(ed.clipboard.get().as_deref(), Some(rel.display().to_string().as_str()));
        // a scratch buffer has none
        ed.run(Command::NewFile);
        ed.run(Command::CopyPath);
        assert!(ed.message.as_ref().is_some_and(|m| m.1));
    }
}
