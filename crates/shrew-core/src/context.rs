//! Editing the app's context menus and the middle button need besides the commands: putting text at the cursor
//! in any mode (a middle-click paste) and copying one given line.

use crate::editor::{Editor, Mode, Register};
use crate::ops;
use crate::selection::Range;
use crate::text;

impl Editor {
    /// Inserts `s` at every cursor as one undo step, whatever the mode (a middle-click paste). A visual selection
    /// is left first; vim's normal mode stays, with the cursor on the last char put in (as after `P`).
    pub fn put_text(&mut self, s: &str) {
        if s.is_empty() || self.about.is_some() || self.picker.is_some() {
            return;
        }
        if self.mode.is_visual() {
            self.set_mode(Mode::Normal);
        }
        self.commit();
        let normal = self.vim() && self.mode == Mode::Normal;
        ops::insert_str(self.buf_mut(), s);
        let b = self.buf_mut();
        if normal {
            let rope = &b.doc.rope;
            b.sel = b.sel.transform(|r| Range::point(text::clamp_normal(rope, text::prev_grapheme(rope, r.head))));
        }
        b.commit();
        let (so, tw) = (self.settings.scrolloff, self.settings.tab_width);
        self.buf_mut().scroll_to_cursor(so, tw);
    }

    /// Copies line `line` whole (as Ctrl+C does with nothing selected) to the unnamed register and the clipboard.
    pub fn copy_line(&mut self, line: usize) {
        let rope = &self.buf().doc.rope;
        let line = line.min(text::last_line(rope));
        let reg = Register { parts: vec![text::line_text(rope, line)], linewise: true, blockwise: false };
        self.write_register(None, reg, true);
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, Editor, KeyPress, Mode};

    fn editor(text: &str) -> Editor {
        let mut e = Editor::new();
        e.install_document(Document::from_str(text));
        e
    }

    #[test]
    fn put_text_keeps_the_mode_and_undoes_in_one_step() {
        let mut e = editor("hello world\n");
        assert_eq!(e.mode, Mode::Normal);
        e.buf_mut().sel = crate::Selection::point(6);
        e.put_text("big ");
        assert_eq!(e.buf().rope().to_string(), "hello big world\n");
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.buf().sel.primary().head, 9, "on the last char put in");
        e.handle_key(KeyPress::ch('u'));
        assert_eq!(e.buf().rope().to_string(), "hello world\n");
        // insert mode: the caret ends after the text
        e.buf_mut().sel = crate::Selection::point(0);
        e.handle_key(KeyPress::ch('i'));
        e.put_text("x");
        assert_eq!((e.mode, e.buf().sel.primary().head), (Mode::Insert, 1));
    }

    #[test]
    fn copy_line_is_linewise() {
        let mut e = editor("one\ntwo\n");
        e.copy_line(1);
        assert_eq!(e.clipboard.get().as_deref(), Some("two\n"));
        e.handle_key(KeyPress::ch('P'));
        assert_eq!(e.buf().rope().to_string(), "two\none\ntwo\n");
    }
}
