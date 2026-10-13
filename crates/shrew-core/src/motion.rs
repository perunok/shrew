//! Cursor motions and text objects on a rope. Positions are char indices.

use crate::text::{self, CharClass, big_class, class};
use ropey::Rope;

/// How far bracket matching scans before giving up (keeps it instant on huge files).
const BRACKET_SCAN_LIMIT: usize = 200_000;

fn cls(c: char, big: bool) -> CharClass {
    if big { big_class(c) } else { class(c) }
}

/// `w` / `W`: start of the next word. An empty line counts as a word, like in vim.
pub fn next_word_start(rope: &Rope, pos: usize, big: bool) -> usize {
    let len = rope.len_chars();
    if pos >= len {
        return len;
    }
    let mut i = pos;
    let k = cls(rope.char(i), big);
    if !matches!(k, CharClass::Space | CharClass::Eol) {
        while i < len && cls(rope.char(i), big) == k {
            i += 1;
        }
    }
    while i < len {
        let c = rope.char(i);
        if c == '\n' {
            if i + 1 < len && rope.char(i + 1) == '\n' {
                return i + 1;
            }
            i += 1;
        } else if class(c) == CharClass::Space {
            i += 1;
        } else {
            return i;
        }
    }
    len
}

/// `b` / `B`: start of the current or previous word.
pub fn prev_word_start(rope: &Rope, pos: usize, big: bool) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut i = pos.min(rope.len_chars()) - 1;
    loop {
        let c = rope.char(i);
        if !matches!(class(c), CharClass::Space | CharClass::Eol) {
            break;
        }
        if c == '\n' && (i == 0 || rope.char(i - 1) == '\n') {
            return i;
        }
        if i == 0 {
            return 0;
        }
        i -= 1;
    }
    let k = cls(rope.char(i), big);
    while i > 0 && cls(rope.char(i - 1), big) == k {
        i -= 1;
    }
    i
}

/// `e` / `E`: end of the current or next word (inclusive position).
pub fn word_end(rope: &Rope, pos: usize, big: bool) -> usize {
    let len = rope.len_chars();
    if len == 0 {
        return 0;
    }
    let mut i = (pos + 1).min(len);
    while i < len && matches!(class(rope.char(i)), CharClass::Space | CharClass::Eol) {
        i += 1;
    }
    if i >= len {
        return len - 1;
    }
    let k = cls(rope.char(i), big);
    while i + 1 < len && cls(rope.char(i + 1), big) == k {
        i += 1;
    }
    i
}

/// `ge` / `gE`: end of the previous word.
pub fn prev_word_end(rope: &Rope, pos: usize, big: bool) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut i = pos;
    let k = cls(rope.char(i.min(rope.len_chars().saturating_sub(1))), big);
    // leave the current word
    while i > 0 && cls(rope.char(i), big) == k && !matches!(k, CharClass::Space | CharClass::Eol) {
        i -= 1;
    }
    while i > 0 && matches!(class(rope.char(i)), CharClass::Space | CharClass::Eol) {
        i -= 1;
    }
    i
}

/// `f`/`t`/`F`/`T` within the current line. `count` occurrences; None when not found.
pub fn find_in_line(rope: &Rope, pos: usize, ch: char, forward: bool, till: bool, count: usize) -> Option<usize> {
    let line = text::line_of(rope, pos);
    let start = text::line_start(rope, line);
    let end = text::line_end(rope, line);
    let mut found = 0;
    if forward {
        let mut i = pos + 1;
        while i < end {
            if rope.char(i) == ch {
                found += 1;
                if found == count {
                    return Some(if till { i - 1 } else { i });
                }
            }
            i += 1;
        }
    } else {
        let mut i = pos;
        while i > start {
            i -= 1;
            if rope.char(i) == ch {
                found += 1;
                if found == count {
                    return Some(if till { i + 1 } else { i });
                }
            }
        }
    }
    None
}

pub fn is_open_bracket(c: char) -> bool {
    matches!(c, '(' | '[' | '{')
}

pub fn is_close_bracket(c: char) -> bool {
    matches!(c, ')' | ']' | '}')
}

pub fn bracket_pair(c: char) -> Option<(char, char, bool)> {
    Some(match c {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' => ('[', ']', false),
        '{' => ('{', '}', true),
        '}' => ('{', '}', false),
        '<' => ('<', '>', true),
        '>' => ('<', '>', false),
        _ => return None,
    })
}

/// The bracket matching the one at `pos` (round, square and curly only).
pub fn matching_bracket(rope: &Rope, pos: usize) -> Option<usize> {
    let c = text::char_at(rope, pos)?;
    let (open, close, forward) = bracket_pair(c).filter(|(o, _, _)| *o != '<')?;
    let len = rope.len_chars();
    let mut depth = 0usize;
    if forward {
        let mut chars = rope.chars_at(pos);
        for (n, ch) in (&mut chars).enumerate().take(BRACKET_SCAN_LIMIT) {
            if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some(pos + n);
                }
            }
        }
    } else {
        let mut chars = rope.chars_at((pos + 1).min(len));
        let mut i = pos + 1;
        for _ in 0..BRACKET_SCAN_LIMIT {
            let Some(ch) = chars.prev() else { break };
            i -= 1;
            if ch == close {
                depth += 1;
            } else if ch == open {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// `%`: jump to the match of the first bracket at or after the cursor on this line.
pub fn match_pair_motion(rope: &Rope, pos: usize) -> Option<usize> {
    let line = text::line_of(rope, pos);
    let end = text::line_end(rope, line);
    let at = (pos..end).find(|&i| matches!(rope.char(i), '(' | ')' | '[' | ']' | '{' | '}'))?;
    matching_bracket(rope, at)
}

/// The innermost pair of `open`/`close` around `pos` (positions of the brackets themselves).
pub fn enclosing_pair(rope: &Rope, pos: usize, open: char, close: char) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    let here = text::char_at(rope, pos);
    // on a bracket: that pair
    if here == Some(open) {
        return matching_bracket(rope, pos).map(|m| (pos, m));
    }
    if here == Some(close) {
        return matching_bracket(rope, pos).map(|m| (m, pos));
    }
    let mut depth = 0usize;
    let mut chars = rope.chars_at(pos.min(len));
    let mut i = pos;
    let mut o = None;
    for _ in 0..BRACKET_SCAN_LIMIT {
        let Some(ch) = chars.prev() else { break };
        i -= 1;
        if ch == close {
            depth += 1;
        } else if ch == open {
            if depth == 0 {
                o = Some(i);
                break;
            }
            depth -= 1;
        }
    }
    let o = o?;
    matching_bracket(rope, o).map(|c| (o, c))
}

/// Text object for a word (`iw`/`aw`, `iW`/`aW`). Returns `[from, to)`.
pub fn word_object(rope: &Rope, pos: usize, big: bool, around: bool) -> (usize, usize) {
    let len = rope.len_chars();
    if len == 0 {
        return (0, 0);
    }
    let pos = pos.min(len - 1);
    let k = cls(rope.char(pos), big);
    let line = text::line_of(rope, pos);
    let ls = text::line_start(rope, line);
    let le = text::line_end(rope, line);
    let mut from = pos;
    while from > ls && cls(rope.char(from - 1), big) == k {
        from -= 1;
    }
    let mut to = pos + 1;
    while to < le && cls(rope.char(to), big) == k {
        to += 1;
    }
    if around {
        if to < le && class(rope.char(to)) == CharClass::Space {
            while to < le && class(rope.char(to)) == CharClass::Space {
                to += 1;
            }
        } else {
            while from > ls && class(rope.char(from - 1)) == CharClass::Space {
                from -= 1;
            }
        }
    }
    (from, to)
}

/// `i(`/`a(` and friends. `[from, to)`.
pub fn pair_object(rope: &Rope, pos: usize, open: char, close: char, around: bool) -> Option<(usize, usize)> {
    let (o, c) = enclosing_pair(rope, pos, open, close)?;
    Some(if around { (o, c + 1) } else { (o + 1, c) })
}

/// `i"`/`a"`: quotes on the current line.
pub fn quote_object(rope: &Rope, pos: usize, q: char, around: bool) -> Option<(usize, usize)> {
    let line = text::line_of(rope, pos);
    let ls = text::line_start(rope, line);
    let le = text::line_end(rope, line);
    let quotes: Vec<usize> = (ls..le).filter(|&i| rope.char(i) == q && (i == ls || rope.char(i - 1) != '\\')).collect();
    // the pair containing pos, else the first pair after it
    let mut pair = None;
    for w in quotes.chunks(2) {
        if let [a, b] = *w {
            if (a <= pos && pos <= b) || (pair.is_none() && a > pos) {
                pair = Some((a, b));
                if a <= pos {
                    break;
                }
            }
        }
    }
    let (a, b) = pair?;
    Some(if around { (a, b + 1) } else { (a + 1, b) })
}

/// `ip`/`ap`: lines of the paragraph around `pos`, as a char range of whole lines.
pub fn paragraph_object(rope: &Rope, pos: usize, around: bool) -> (usize, usize) {
    let last = text::last_line(rope);
    let line = text::line_of(rope, pos);
    let blank = text::is_blank_line(rope, line);
    let mut a = line;
    while a > 0 && text::is_blank_line(rope, a - 1) == blank {
        a -= 1;
    }
    let mut b = line;
    while b < last && text::is_blank_line(rope, b + 1) == blank {
        b += 1;
    }
    if around {
        while b < last && text::is_blank_line(rope, b + 1) != blank {
            b += 1;
        }
    }
    let from = text::line_start(rope, a);
    let to = if b < last { text::line_start(rope, b + 1) } else { rope.len_chars() };
    (from, to)
}

/// `}`: next blank line after a non-blank one.
pub fn paragraph_forward(rope: &Rope, pos: usize) -> usize {
    let last = text::last_line(rope);
    let mut l = text::line_of(rope, pos);
    while l < last && text::is_blank_line(rope, l) {
        l += 1;
    }
    while l < last && !text::is_blank_line(rope, l) {
        l += 1;
    }
    if l >= last && !text::is_blank_line(rope, l) { text::line_end(rope, l) } else { text::line_start(rope, l) }
}

/// `{`: previous blank line.
pub fn paragraph_backward(rope: &Rope, pos: usize) -> usize {
    let mut l = text::line_of(rope, pos);
    while l > 0 && text::is_blank_line(rope, l) {
        l -= 1;
    }
    while l > 0 && !text::is_blank_line(rope, l) {
        l -= 1;
    }
    text::line_start(rope, l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        let r = Rope::from_str("foo.bar  baz\n\nqux");
        assert_eq!(next_word_start(&r, 0, false), 3);
        assert_eq!(next_word_start(&r, 3, false), 4);
        assert_eq!(next_word_start(&r, 4, false), 9);
        assert_eq!(next_word_start(&r, 0, true), 9);
        assert_eq!(next_word_start(&r, 9, false), 13); // empty line
        assert_eq!(next_word_start(&r, 13, false), 14);
        assert_eq!(prev_word_start(&r, 9, false), 4);
        assert_eq!(prev_word_start(&r, 14, false), 13);
        assert_eq!(word_end(&r, 0, false), 2);
        assert_eq!(word_end(&r, 2, false), 3);
        assert_eq!(word_end(&r, 0, true), 6);
    }

    #[test]
    fn brackets() {
        let r = Rope::from_str("f(a, [b], {c})");
        assert_eq!(matching_bracket(&r, 1), Some(13));
        assert_eq!(matching_bracket(&r, 13), Some(1));
        assert_eq!(matching_bracket(&r, 5), Some(7));
        assert_eq!(pair_object(&r, 3, '(', ')', false), Some((2, 13)));
        assert_eq!(pair_object(&r, 11, '{', '}', true), Some((10, 13)));
        assert_eq!(match_pair_motion(&r, 0), Some(13));
    }

    #[test]
    fn objects() {
        let r = Rope::from_str("let x = \"hi there\";");
        assert_eq!(quote_object(&r, 10, '"', false), Some((9, 17)));
        assert_eq!(quote_object(&r, 0, '"', true), Some((8, 18)));
        assert_eq!(word_object(&r, 1, false, false), (0, 3));
        assert_eq!(word_object(&r, 1, false, true), (0, 4));
    }
}
