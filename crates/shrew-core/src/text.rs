//! Rope helpers: lines, display columns, character classes.
//!
//! All positions in shrew are *char* indices into the rope. Lines are ropey lines (split on `\n` only; CRLF files
//! are normalised on load and restored on save).

use ropey::Rope;
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharClass {
    Space,
    Eol,
    Word,
    Punct,
}

#[inline]
pub fn class(c: char) -> CharClass {
    if c == '\n' {
        CharClass::Eol
    } else if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// Class used by the "big word" motions (W, B, E): everything that is not whitespace is one class.
#[inline]
pub fn big_class(c: char) -> CharClass {
    match class(c) {
        CharClass::Punct => CharClass::Word,
        k => k,
    }
}

#[inline]
pub fn is_word_char(c: char) -> bool {
    class(c) == CharClass::Word
}

/// Display width of `c` when it starts at display column `col`.
#[inline]
pub fn char_width(c: char, col: usize, tab_width: usize) -> usize {
    if c == '\t' {
        tab_width - col % tab_width
    } else if (c as u32) < 0x20 {
        1
    } else {
        c.width().unwrap_or(1)
    }
}

/// Number of chars on `line`, not counting its line break.
pub fn line_len(rope: &Rope, line: usize) -> usize {
    let s = rope.line(line);
    let n = s.len_chars();
    if n > 0 && s.char(n - 1) == '\n' { n - 1 } else { n }
}

#[inline]
pub fn line_start(rope: &Rope, line: usize) -> usize {
    rope.line_to_char(line)
}

/// Position of the line break (or the end of the text on the last line).
#[inline]
pub fn line_end(rope: &Rope, line: usize) -> usize {
    line_start(rope, line) + line_len(rope, line)
}

#[inline]
pub fn last_line(rope: &Rope) -> usize {
    rope.len_lines().saturating_sub(1)
}

#[inline]
pub fn line_of(rope: &Rope, pos: usize) -> usize {
    rope.char_to_line(pos.min(rope.len_chars()))
}

#[inline]
pub fn char_at(rope: &Rope, pos: usize) -> Option<char> {
    (pos < rope.len_chars()).then(|| rope.char(pos))
}

/// Display column of `pos` within its line.
pub fn col_of(rope: &Rope, pos: usize, tab_width: usize) -> usize {
    let line = line_of(rope, pos);
    let start = line_start(rope, line);
    let mut col = 0;
    for c in rope.slice(start..pos).chars() {
        col += char_width(c, col, tab_width);
    }
    col
}

/// The position on `line` whose display column contains `col` (clamped to the line end).
pub fn pos_at_col(rope: &Rope, line: usize, col: usize, tab_width: usize) -> usize {
    let start = line_start(rope, line);
    let len = line_len(rope, line);
    let mut c = 0;
    for (i, ch) in rope.slice(start..start + len).chars().enumerate() {
        let w = char_width(ch, c, tab_width);
        if c + w > col {
            return start + i;
        }
        c += w;
    }
    start + len
}

pub fn first_non_blank(rope: &Rope, line: usize) -> usize {
    let start = line_start(rope, line);
    let len = line_len(rope, line);
    rope.slice(start..start + len)
        .chars()
        .position(|c| c != ' ' && c != '\t')
        .map_or(start + len, |i| start + i)
}

/// Leading whitespace of `line`.
pub fn indent_of(rope: &Rope, line: usize) -> String {
    let start = line_start(rope, line);
    let len = line_len(rope, line);
    rope.slice(start..start + len).chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

pub fn is_blank_line(rope: &Rope, line: usize) -> bool {
    first_non_blank(rope, line) == line_end(rope, line)
}

/// In normal mode the block cursor sits *on* a char, never on the line break (unless the line is empty).
pub fn clamp_normal(rope: &Rope, pos: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    let line = line_of(rope, pos);
    let start = line_start(rope, line);
    let end = line_end(rope, line);
    if end > start { pos.min(end - 1) } else { start }
}

/// Byte offset → char index helpers live on ropey already; this is the slice text of a line without its break.
pub fn line_text(rope: &Rope, line: usize) -> String {
    let start = line_start(rope, line);
    rope.slice(start..start + line_len(rope, line)).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_columns() {
        let r = Rope::from_str("ab\n\tc\n");
        assert_eq!(line_len(&r, 0), 2);
        assert_eq!(line_len(&r, 1), 2);
        assert_eq!(line_len(&r, 2), 0);
        assert_eq!(col_of(&r, 5, 4), 5); // after "\tc"
        assert_eq!(pos_at_col(&r, 1, 2, 4), 3); // inside the tab
        assert_eq!(pos_at_col(&r, 1, 4, 4), 4);
        assert_eq!(clamp_normal(&r, 2), 1);
        assert_eq!(first_non_blank(&r, 1), 4);
    }
}
