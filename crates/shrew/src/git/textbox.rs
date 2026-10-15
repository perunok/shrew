//! A small multi-line text field with its own caret: the commit message box and the git prompt's line. The
//! caret is a byte index kept on a char boundary; lines wrap at a column count for drawing.

use shrew_core::{Key, KeyPress};
use shrew_render::{char_columns, columns};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextBox {
    pub text: String,
    /// Byte index into `text`.
    pub cursor: usize,
}

/// What a key did to the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Moved or changed.
    Done,
    /// Not a key the field takes.
    Ignored,
}

impl TextBox {
    pub fn new(text: &str) -> TextBox {
        TextBox { text: text.to_string(), cursor: text.len() }
    }

    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
    }

    /// The first line (a commit's subject).
    pub fn subject(&self) -> &str {
        self.text.lines().next().unwrap_or("")
    }

    pub fn insert(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map_or(0, |(b, _)| b)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
    }

    fn line_start(&self, i: usize) -> usize {
        self.text[..i].rfind('\n').map_or(0, |b| b + 1)
    }

    fn line_end(&self, i: usize) -> usize {
        self.text[i..].find('\n').map_or(self.text.len(), |b| i + b)
    }

    /// The start of the word before `i` (Ctrl+Left, Ctrl+Backspace).
    fn word_left(&self, i: usize) -> usize {
        let before = &self.text[..i];
        let trimmed = before.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');
        trimmed.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_').len()
    }

    fn word_right(&self, i: usize) -> usize {
        let after = &self.text[i..];
        let skip = after.len() - after.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_').len();
        let rest = &after[skip..];
        i + skip + (rest.len() - rest.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_').len())
    }

    /// Moves to the same column of the line above or below (by chars, unwrapped lines).
    fn vertical(&mut self, down: bool) -> bool {
        let start = self.line_start(self.cursor);
        let col = self.text[start..self.cursor].chars().count();
        let target = if down {
            let end = self.line_end(self.cursor);
            if end >= self.text.len() {
                return false;
            }
            end + 1
        } else {
            if start == 0 {
                return false;
            }
            self.line_start(start - 1)
        };
        let end = self.line_end(target);
        self.cursor = self.text[target..end].char_indices().nth(col).map_or(end, |(b, _)| target + b);
        true
    }

    /// The editing keys. `multiline`: Enter inserts a line break (else it is not taken).
    pub fn key(&mut self, k: KeyPress, multiline: bool) -> Edit {
        let m = k.mods;
        let c = self.cursor;
        match k.key {
            Key::Left if m.ctrl => self.cursor = self.word_left(c),
            Key::Right if m.ctrl => self.cursor = self.word_right(c),
            Key::Left => self.cursor = self.prev(c),
            Key::Right => self.cursor = self.next(c),
            Key::Up if multiline => return if self.vertical(false) { Edit::Done } else { Edit::Ignored },
            Key::Down if multiline => return if self.vertical(true) { Edit::Done } else { Edit::Ignored },
            Key::Home if m.ctrl => self.cursor = 0,
            Key::End if m.ctrl => self.cursor = self.text.len(),
            Key::Home => self.cursor = self.line_start(c),
            Key::End => self.cursor = self.line_end(c),
            Key::Backspace if m.ctrl || m.alt => {
                let from = self.word_left(c);
                self.text.replace_range(from..c, "");
                self.cursor = from;
            }
            Key::Char('w') if m.ctrl && !m.shift => {
                let from = self.word_left(c);
                self.text.replace_range(from..c, "");
                self.cursor = from;
            }
            Key::Char('u') if m.ctrl && !m.shift => {
                let from = self.line_start(c);
                self.text.replace_range(from..c, "");
                self.cursor = from;
            }
            Key::Backspace => {
                let from = self.prev(c);
                self.text.replace_range(from..c, "");
                self.cursor = from;
            }
            Key::Delete => {
                let to = self.next(c);
                self.text.replace_range(c..to, "");
            }
            Key::Enter if multiline && !m.ctrl && !m.alt => self.insert("\n"),
            _ => match k.text() {
                Some(ch) if !ch.is_control() => {
                    let mut b = [0u8; 4];
                    self.insert(ch.encode_utf8(&mut b));
                }
                _ => return Edit::Ignored,
            },
        }
        Edit::Done
    }

    /// The text cut into display rows of at most `cols` columns: byte ranges into `text`, every line at least one
    /// row. Long words break anywhere.
    pub fn rows(&self, cols: usize) -> Vec<(usize, usize)> {
        let cols = cols.max(1);
        let mut out = Vec::new();
        let mut line_start = 0;
        for line in self.text.split('\n') {
            let mut start = 0;
            let mut w = 0;
            for (i, ch) in line.char_indices() {
                let cw = char_columns(ch);
                if w + cw > cols && i > start {
                    out.push((line_start + start, line_start + i));
                    start = i;
                    w = 0;
                }
                w += cw;
            }
            out.push((line_start + start, line_start + line.len()));
            line_start += line.len() + 1;
        }
        out
    }

    /// The byte index at display `row` and column `col` of [`TextBox::rows`] (a click).
    pub fn index_at(&self, cols: usize, row: usize, col: usize) -> usize {
        let rows = self.rows(cols);
        let Some(&(a, z)) = rows.get(row).or(rows.last()) else { return 0 };
        let mut w = 0;
        for (i, ch) in self.text[a..z].char_indices() {
            let cw = char_columns(ch);
            if w + cw > col {
                return a + i;
            }
            w += cw;
        }
        z
    }

    /// The caret's display row and column in [`TextBox::rows`].
    pub fn caret(&self, cols: usize) -> (usize, usize) {
        let rows = self.rows(cols);
        for (r, &(a, z)) in rows.iter().enumerate() {
            let last_of_line = rows.get(r + 1).is_none_or(|&(na, _)| na != z);
            if self.cursor >= a && (self.cursor < z || (self.cursor == z && last_of_line)) {
                return (r, columns(&self.text[a..self.cursor]));
            }
        }
        (rows.len().saturating_sub(1), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(b: &mut TextBox, s: &str) {
        for k in KeyPress::parse_seq(s) {
            b.key(k, true);
        }
    }

    #[test]
    fn typing_and_moving() {
        let mut b = TextBox::default();
        keys(&mut b, "fix: thé bug<CR><CR>body");
        assert_eq!(b.text, "fix: thé bug\n\nbody");
        assert_eq!(b.subject(), "fix: thé bug");
        keys(&mut b, "<Up><Up><End><BS><BS><BS>");
        assert_eq!(b.subject(), "fix: thé ");
        keys(&mut b, "<C-BS>");
        assert_eq!(b.subject(), "fix: ");
        keys(&mut b, "<Home><Del>");
        assert_eq!(b.subject(), "ix: ");
        keys(&mut b, "<C-End>!");
        assert!(b.text.ends_with("body!"));
        keys(&mut b, "<C-u>");
        assert_eq!(b.text, "ix: \n\n");
    }

    #[test]
    fn up_and_down_keep_the_column() {
        let mut b = TextBox::new("abcdef\nxy\nlonger line");
        b.cursor = 4;
        keys(&mut b, "<Down>");
        assert_eq!(b.cursor, 9, "end of the short line");
        keys(&mut b, "<Down>");
        assert_eq!(&b.text[b.cursor..], "nger line", "the column it has now");
        assert_eq!(b.key(KeyPress::parse("<Down>").unwrap(), true), Edit::Ignored, "last line");
    }

    #[test]
    fn rows_wrap_and_place_the_caret() {
        let b = TextBox::new("abcdefgh\n\nxy");
        assert_eq!(b.rows(3), vec![(0, 3), (3, 6), (6, 8), (9, 9), (10, 12)]);
        assert_eq!(b.caret(3), (4, 2));
        let mut b = TextBox::new("abcdef");
        b.cursor = 3;
        assert_eq!(b.caret(3), (1, 0), "at a wrap: the start of the next row");
        b.cursor = 6;
        assert_eq!(b.caret(3), (1, 3));
    }
}
