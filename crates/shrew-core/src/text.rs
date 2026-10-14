//! Rope helpers: lines, display columns, character classes, grapheme clusters.
//!
//! All positions in shrew are *char* indices into the rope. Lines are ropey lines (split on `\n` only; CRLF files
//! are normalised on load and restored on save). Cursors never stop inside a grapheme cluster (an emoji ZWJ
//! sequence, a letter with combining accents): see [`next_grapheme`] and [`prev_grapheme`].

use ropey::{Rope, RopeSlice};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};
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
    if c.is_ascii() {
        return match c {
            '\n' => CharClass::Eol,
            ' ' | '\t' | '\r' | '\x0b' | '\x0c' => CharClass::Space,
            c if c.is_ascii_alphanumeric() || c == '_' => CharClass::Word,
            _ => CharClass::Punct,
        };
    }
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || is_combining_mark(c) {
        // accents belong to the letter they sit on
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// The common combining-mark blocks (accents written as separate code points).
#[inline]
fn is_combining_mark(c: char) -> bool {
    matches!(c as u32, 0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
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
    } else if (c as u32) < 0x7f {
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

/// Display width of `s` starting at display column `col`. Walks whole chunks: ASCII without tabs is just a length,
/// so even a multi-megabyte line costs milliseconds.
fn advance_cols(s: RopeSlice, col: usize, tab_width: usize) -> usize {
    advance_cols_plain(s, col, tab_width).0
}

/// [`advance_cols`], plus whether every char was one column wide (then columns are just char counts).
pub(crate) fn advance_cols_plain(s: RopeSlice, mut col: usize, tab_width: usize) -> (usize, bool) {
    if s.len_bytes() == s.len_chars() {
        // all ASCII (ropey knows both lengths): only tabs are not one column wide
        let mut plain = true;
        for chunk in s.chunks() {
            let bytes = chunk.as_bytes();
            if !bytes.contains(&b'\t') {
                col += bytes.len();
                continue;
            }
            plain = false;
            for &b in bytes {
                col += if b == b'\t' { tab_width - col % tab_width } else { 1 };
            }
        }
        return (col, plain);
    }
    for chunk in s.chunks() {
        if chunk.is_ascii() {
            let bytes = chunk.as_bytes();
            if !bytes.contains(&b'\t') {
                col += bytes.len();
                continue;
            }
            for &b in bytes {
                col += if b == b'\t' { tab_width - col % tab_width } else { 1 };
            }
        } else {
            for c in chunk.chars() {
                col += char_width(c, col, tab_width);
            }
        }
    }
    (col, false)
}

/// Display column of `pos` within its line.
pub fn col_of(rope: &Rope, pos: usize, tab_width: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    let start = line_start(rope, line_of(rope, pos));
    advance_cols(rope.slice(start..pos), 0, tab_width)
}

/// The position on `line` whose display column contains `col` (clamped to the line end).
pub fn pos_at_col(rope: &Rope, line: usize, col: usize, tab_width: usize) -> usize {
    let start = line_start(rope, line);
    let len = line_len(rope, line);
    let mut c = 0;
    let mut i = start;
    for chunk in rope.slice(start..start + len).chunks() {
        let bytes = chunk.as_bytes();
        if chunk.is_ascii() && !bytes.contains(&b'\t') {
            if c + bytes.len() > col {
                return i + (col - c);
            }
            c += bytes.len();
            i += bytes.len();
            continue;
        }
        for ch in chunk.chars() {
            let w = char_width(ch, c, tab_width);
            if c + w > col {
                return i;
            }
            c += w;
            i += 1;
        }
    }
    start + len
}

pub fn first_non_blank(rope: &Rope, line: usize) -> usize {
    let start = line_start(rope, line);
    let len = line_len(rope, line);
    rope.slice(start..start + len).chars().position(|c| c != ' ' && c != '\t').map_or(start + len, |i| start + i)
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

/// In normal mode the block cursor sits *on* a char (the start of its grapheme), never on the line break (unless
/// the line is empty).
pub fn clamp_normal(rope: &Rope, pos: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    let line = line_of(rope, pos);
    let start = line_start(rope, line);
    let end = line_end(rope, line);
    if end > start { grapheme_start(rope, pos.min(end - 1)) } else { start }
}

/// Byte offset → char index helpers live on ropey already; this is the slice text of a line without its break.
pub fn line_text(rope: &Rope, line: usize) -> String {
    let start = line_start(rope, line);
    rope.slice(start..start + line_len(rope, line)).to_string()
}

// ----- grapheme clusters -----

/// Plain ASCII on both sides of a boundary (and not a CR LF pair): the boundary is trivially there.
#[inline]
fn ascii_boundary(rope: &Rope, pos: usize) -> bool {
    if pos == 0 || pos >= rope.len_chars() {
        return true;
    }
    let (a, b) = (rope.char(pos - 1), rope.char(pos));
    a.is_ascii() && b.is_ascii() && !(a == '\r' && b == '\n')
}

fn byte_to_char(rope: &Rope, byte: usize) -> usize {
    rope.byte_to_char(byte)
}

/// Start of the grapheme after the one at `pos` (or the text length).
pub fn next_grapheme(rope: &Rope, pos: usize) -> usize {
    let len = rope.len_chars();
    if pos >= len {
        return len;
    }
    if ascii_boundary(rope, pos + 1) {
        return pos + 1;
    }
    let byte = rope.char_to_byte(pos);
    let (mut chunk, mut chunk_start, _, _) = rope.chunk_at_byte(byte);
    let mut gc = GraphemeCursor::new(byte, rope.len_bytes(), true);
    loop {
        match gc.next_boundary(chunk, chunk_start) {
            Ok(None) => return len,
            Ok(Some(b)) => return byte_to_char(rope, b),
            Err(GraphemeIncomplete::NextChunk) => {
                chunk_start += chunk.len();
                chunk = rope.chunk_at_byte(chunk_start).0;
            }
            Err(GraphemeIncomplete::PreContext(n)) => {
                let (ctx, ctx_start, _, _) = rope.chunk_at_byte(n - 1);
                gc.provide_context(ctx, ctx_start);
            }
            Err(_) => return pos + 1,
        }
    }
}

/// Start of the grapheme before `pos` (or 0).
pub fn prev_grapheme(rope: &Rope, pos: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    if pos == 0 {
        return 0;
    }
    if ascii_boundary(rope, pos - 1) && rope.char(pos - 1).is_ascii() {
        return pos - 1;
    }
    let byte = rope.char_to_byte(pos);
    let (mut chunk, mut chunk_start, _, _) = rope.chunk_at_byte(byte);
    let mut gc = GraphemeCursor::new(byte, rope.len_bytes(), true);
    loop {
        match gc.prev_boundary(chunk, chunk_start) {
            Ok(None) => return 0,
            Ok(Some(b)) => return byte_to_char(rope, b),
            Err(GraphemeIncomplete::PrevChunk) => {
                let (c, s, _, _) = rope.chunk_at_byte(chunk_start - 1);
                chunk = c;
                chunk_start = s;
            }
            Err(GraphemeIncomplete::PreContext(n)) => {
                let (ctx, ctx_start, _, _) = rope.chunk_at_byte(n - 1);
                gc.provide_context(ctx, ctx_start);
            }
            Err(_) => return pos - 1,
        }
    }
}

/// Whether a cursor may stop at `pos`.
pub fn is_grapheme_boundary(rope: &Rope, pos: usize) -> bool {
    if pos > rope.len_chars() {
        return false;
    }
    if ascii_boundary(rope, pos) {
        return true;
    }
    let byte = rope.char_to_byte(pos);
    let (mut chunk, mut chunk_start, _, _) = rope.chunk_at_byte(byte);
    let mut gc = GraphemeCursor::new(byte, rope.len_bytes(), true);
    loop {
        match gc.is_boundary(chunk, chunk_start) {
            Ok(b) => return b,
            Err(GraphemeIncomplete::PreContext(n)) => {
                let (ctx, ctx_start, _, _) = rope.chunk_at_byte(n - 1);
                gc.provide_context(ctx, ctx_start);
            }
            Err(GraphemeIncomplete::NextChunk) => {
                chunk_start += chunk.len();
                if chunk_start >= rope.len_bytes() {
                    return true;
                }
                chunk = rope.chunk_at_byte(chunk_start).0;
            }
            Err(_) => return true,
        }
    }
}

/// `pos` moved back to the start of the grapheme containing it.
pub fn grapheme_start(rope: &Rope, pos: usize) -> usize {
    if is_grapheme_boundary(rope, pos) { pos } else { prev_grapheme(rope, pos) }
}

/// End of the grapheme starting at `pos`: the exclusive end of an inclusive selection or motion ending there.
#[inline]
pub fn grapheme_end(rope: &Rope, pos: usize) -> usize {
    next_grapheme(rope, pos)
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
        let r = Rope::from_str("a日b\tc");
        assert_eq!(col_of(&r, 2, 4), 3);
        assert_eq!(col_of(&r, 4, 4), 8);
        assert_eq!(pos_at_col(&r, 0, 2, 4), 1);
        assert_eq!(pos_at_col(&r, 0, 3, 4), 2);
        assert_eq!(pos_at_col(&r, 0, 99, 4), 5);
    }

    #[test]
    fn long_lines_span_chunks() {
        let s = "x".repeat(100_000);
        let r = Rope::from_str(&format!("{s}\t{s}"));
        assert_eq!(col_of(&r, 100_001, 4), 100_004);
        assert_eq!(pos_at_col(&r, 0, 100_004, 4), 100_001);
        assert_eq!(pos_at_col(&r, 0, 150_000, 4), 149_997);
    }

    #[test]
    fn graphemes() {
        // family emoji (ZWJ sequence), e + combining acute, flag (regional indicators)
        let s = "a👨\u{200d}👩\u{200d}👧e\u{301}🇫🇷b";
        let r = Rope::from_str(s);
        let stops: Vec<usize> =
            std::iter::successors(Some(0), |&p| (p < r.len_chars()).then(|| next_grapheme(&r, p))).collect();
        assert_eq!(stops, vec![0, 1, 6, 8, 10, 11]);
        let back: Vec<usize> = std::iter::successors(Some(11), |&p| (p > 0).then(|| prev_grapheme(&r, p))).collect();
        assert_eq!(back, vec![11, 10, 8, 6, 1, 0]);
        assert!(!is_grapheme_boundary(&r, 3));
        assert_eq!(grapheme_start(&r, 3), 1);
        assert_eq!(grapheme_start(&r, 7), 6);
        assert_eq!(clamp_normal(&r, 99), 10);
        assert_eq!(class('\u{301}'), CharClass::Word);
    }
}
