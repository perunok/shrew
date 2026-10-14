//! Cursor motions and text objects on a rope. Positions are char indices.

use crate::text::{self, CharClass, big_class, class};
use ropey::Rope;

/// How far bracket matching scans before giving up (keeps it instant on huge files).
const BRACKET_SCAN_LIMIT: usize = 200_000;

fn cls(c: char, big: bool) -> CharClass {
    if big { big_class(c) } else { class(c) }
}

#[inline]
fn blank(k: CharClass) -> bool {
    matches!(k, CharClass::Space | CharClass::Eol)
}

/// An empty line: a `\n` at the start of a line. Vim's word motions stop on those.
#[inline]
fn empty_line_at(rope: &Rope, i: usize) -> bool {
    rope.char(i) == '\n' && (i == 0 || rope.char(i - 1) == '\n')
}

/// `w` / `W`: start of the next word. An empty line counts as a word, like in vim.
pub fn next_word_start(rope: &Rope, pos: usize, big: bool) -> usize {
    let len = rope.len_chars();
    if pos >= len {
        return len;
    }
    let mut chars = rope.chars_at(pos).peekable();
    let mut i = pos;
    let k = cls(rope.char(i), big);
    if !blank(k) {
        while chars.peek().is_some_and(|&c| cls(c, big) == k) {
            chars.next();
            i += 1;
        }
    }
    while let Some(c) = chars.next() {
        if c == '\n' {
            if chars.peek() == Some(&'\n') {
                return i + 1;
            }
        } else if class(c) != CharClass::Space {
            return i;
        }
        i += 1;
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
        if !blank(class(c)) {
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
    let mut chars = rope.chars_at(i);
    while chars.prev().is_some_and(|c| cls(c, big) == k) {
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
    while i < len && blank(class(rope.char(i))) {
        i += 1;
    }
    if i >= len {
        return len - 1;
    }
    let k = cls(rope.char(i), big);
    let mut chars = rope.chars_at(i + 1);
    while chars.next().is_some_and(|c| cls(c, big) == k) {
        i += 1;
    }
    i
}

/// `ge` / `gE`: end of the previous word. An empty line counts as a word end.
pub fn prev_word_end(rope: &Rope, pos: usize, big: bool) -> usize {
    let len = rope.len_chars();
    if len == 0 || pos == 0 {
        return 0;
    }
    let mut i = pos.min(len - 1);
    let k = cls(rope.char(i), big);
    if blank(k) {
        i -= 1;
    } else {
        // leave the current word
        while cls(rope.char(i), big) == k {
            if i == 0 {
                return 0;
            }
            i -= 1;
        }
    }
    loop {
        if !blank(class(rope.char(i))) || empty_line_at(rope, i) || i == 0 {
            return i;
        }
        i -= 1;
    }
}

/// `f`/`t`/`F`/`T` within the current line. `count` occurrences; None when not found.
pub fn find_in_line(rope: &Rope, pos: usize, ch: char, forward: bool, till: bool, count: usize) -> Option<usize> {
    let line = text::line_of(rope, pos);
    let start = text::line_start(rope, line);
    let end = text::line_end(rope, line);
    let mut found = 0;
    if forward {
        if pos + 1 >= end {
            return None;
        }
        for (n, c) in rope.slice(pos + 1..end).chars().enumerate() {
            if c == ch {
                found += 1;
                if found == count {
                    let i = pos + 1 + n;
                    return Some(if till { i - 1 } else { i });
                }
            }
        }
    } else {
        let mut chars = rope.chars_at(pos);
        let mut i = pos;
        while i > start {
            i -= 1;
            if chars.prev() == Some(ch) {
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

/// The bracket matching `open` at `pos` (scanning forward) or `close` at `pos` (scanning backward).
fn match_at(rope: &Rope, pos: usize, open: char, close: char, forward: bool) -> Option<usize> {
    let mut depth = 0usize;
    if forward {
        for (n, ch) in rope.chars_at(pos).enumerate().take(BRACKET_SCAN_LIMIT) {
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
        let mut chars = rope.chars_at((pos + 1).min(rope.len_chars()));
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

/// The bracket matching the one at `pos` (round, square and curly only).
pub fn matching_bracket(rope: &Rope, pos: usize) -> Option<usize> {
    let c = text::char_at(rope, pos)?;
    let (open, close, forward) = bracket_pair(c).filter(|(o, _, _)| *o != '<')?;
    match_at(rope, pos, open, close, forward)
}

/// `%`: jump to the match of the first bracket at or after the cursor on this line.
pub fn match_pair_motion(rope: &Rope, pos: usize) -> Option<usize> {
    let line = text::line_of(rope, pos);
    let end = text::line_end(rope, line);
    let at =
        pos + rope.slice(pos.min(end)..end).chars().position(|c| matches!(c, '(' | ')' | '[' | ']' | '{' | '}'))?;
    matching_bracket(rope, at)
}

/// The nearest unmatched `open` strictly before `pos`.
fn open_before(rope: &Rope, pos: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut chars = rope.chars_at(pos.min(rope.len_chars()));
    let mut i = pos;
    for _ in 0..BRACKET_SCAN_LIMIT {
        let ch = chars.prev()?;
        i -= 1;
        if ch == close {
            depth += 1;
        } else if ch == open {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
        }
    }
    None
}

/// The innermost pair of `open`/`close` around `pos` (positions of the brackets themselves).
pub fn enclosing_pair(rope: &Rope, pos: usize, open: char, close: char) -> Option<(usize, usize)> {
    enclosing_pair_n(rope, pos, open, close, 1)
}

/// The `count`-th pair of `open`/`close` around `pos`, counting outwards (`2i(`).
pub fn enclosing_pair_n(rope: &Rope, pos: usize, open: char, close: char, count: usize) -> Option<(usize, usize)> {
    let here = text::char_at(rope, pos);
    // on a bracket: that pair
    let mut pair = if here == Some(open) {
        (pos, match_at(rope, pos, open, close, true)?)
    } else if here == Some(close) {
        (match_at(rope, pos, open, close, false)?, pos)
    } else {
        let o = open_before(rope, pos, open, close)?;
        (o, match_at(rope, o, open, close, true)?)
    };
    for _ in 1..count {
        let o = open_before(rope, pair.0, open, close)?;
        pair = (o, match_at(rope, o, open, close, true)?);
    }
    Some(pair)
}

/// End of the run of chars of the same class starting at `i` (stops at `le`).
fn run_end(rope: &Rope, i: usize, le: usize, big: bool) -> usize {
    let k = cls(rope.char(i), big);
    i + 1 + rope.slice(i + 1..le).chars().take_while(|&c| cls(c, big) == k).count()
}

/// Start of the run of chars of the same class ending at `i` (stops at `ls`).
fn run_start(rope: &Rope, i: usize, ls: usize, big: bool) -> usize {
    let k = cls(rope.char(i), big);
    let mut chars = rope.chars_at(i);
    let mut from = i;
    while from > ls && chars.prev().is_some_and(|c| cls(c, big) == k) {
        from -= 1;
    }
    from
}

/// Text object for a word (`iw`/`aw`, `iW`/`aW`). Returns `[from, to)`.
pub fn word_object(rope: &Rope, pos: usize, big: bool, around: bool) -> (usize, usize) {
    word_object_n(rope, pos, big, around, 1)
}

/// `[count]iw` / `[count]aw`: whitespace runs count as words for `iw`; `aw` takes trailing white space (or leading
/// white space when there is none after the last word). Stays on the cursor line.
pub fn word_object_n(rope: &Rope, pos: usize, big: bool, around: bool, count: usize) -> (usize, usize) {
    let len = rope.len_chars();
    if len == 0 {
        return (0, 0);
    }
    let pos = pos.min(len - 1);
    let line = text::line_of(rope, pos);
    let ls = text::line_start(rope, line);
    let le = text::line_end(rope, line);
    if pos >= le {
        return (pos, pos);
    }
    let space = |i: usize| i < le && class(rope.char(i)) == CharClass::Space;
    let mut from = run_start(rope, pos, ls, big);
    let mut to = run_end(rope, pos, le, big);
    if !around {
        for _ in 1..count {
            if to >= le {
                break;
            }
            to = run_end(rope, to, le, big);
        }
        return (from, to);
    }
    let on_space = space(pos);
    if on_space && to < le {
        to = run_end(rope, to, le, big); // white space plus the word after it
    }
    for _ in 1..count {
        if space(to) {
            to = run_end(rope, to, le, big);
        }
        if to < le {
            to = run_end(rope, to, le, big);
        }
    }
    if !on_space {
        if space(to) {
            to = run_end(rope, to, le, big);
        } else {
            while from > ls && class(rope.char(from - 1)) == CharClass::Space {
                from -= 1;
            }
            // but not the indent at the start of the line
            if from == ls {
                from = run_start(rope, pos, ls, big);
            }
        }
    }
    (from, to)
}

/// `i(`/`a(` and friends. `[from, to)`.
pub fn pair_object(rope: &Rope, pos: usize, open: char, close: char, around: bool) -> Option<(usize, usize)> {
    pair_object_n(rope, pos, open, close, around, 1)
}

/// `[count]i(` / `[count]a(`. Like vim, the inner block of `{` at a line end and `}` after only indent is the
/// lines between them (ending at the start of the `}` line, which makes operators on it linewise).
pub fn pair_object_n(
    rope: &Rope,
    pos: usize,
    open: char,
    close: char,
    around: bool,
    count: usize,
) -> Option<(usize, usize)> {
    let (o, c) = match enclosing_pair_n(rope, pos, open, close, count) {
        Some(p) => p,
        // not inside a pair: the next one on the line (vim 9)
        None if count == 1 => {
            let le = text::line_end(rope, text::line_of(rope, pos));
            let o = pos + rope.slice(pos..le).chars().position(|ch| ch == open)?;
            (o, match_at(rope, o, open, close, true)?)
        }
        None => return None,
    };
    if around {
        return Some((o, c + 1));
    }
    let mut a = o + 1;
    if text::char_at(rope, a) == Some('\n') {
        a += 1;
    }
    let mut z = c;
    let cl = text::line_start(rope, text::line_of(rope, c));
    if cl > o && rope.slice(cl..c).chars().all(|ch| ch == ' ' || ch == '\t') {
        z = cl;
    }
    Some(if a > z { (o + 1, o + 1) } else { (a, z) })
}

/// `i"`/`a"`: quotes on the current line, paired the way vim does it. `a"` takes trailing white space, or leading
/// white space when there is none after the closing quote.
pub fn quote_object(rope: &Rope, pos: usize, q: char, around: bool) -> Option<(usize, usize)> {
    let line = text::line_of(rope, pos);
    let ls = text::line_start(rope, line);
    let le = text::line_end(rope, line);
    let chars: Vec<char> = rope.slice(ls..le).chars().collect();
    let col = pos.checked_sub(ls)?;
    let is_q = |i: usize| chars[i] == q && (i == 0 || chars[i - 1] != '\\');
    let next_q = |from: usize| (from..chars.len()).find(|&i| is_q(i));
    let (a, b) = if col < chars.len() && is_q(col) {
        // on a quote: opening or closing depends on how many come before it
        let before = (0..col).filter(|&i| is_q(i)).count();
        if before % 2 == 0 { (col, next_q(col + 1)?) } else { ((0..col).rev().find(|&i| is_q(i))?, col) }
    } else {
        match (0..col.min(chars.len())).rev().find(|&i| is_q(i)) {
            Some(a) => (a, next_q(col)?),
            None => {
                let a = next_q(col)?;
                (a, next_q(a + 1)?)
            }
        }
    };
    if !around {
        return Some((ls + a + 1, ls + b));
    }
    let ws = |c: char| c == ' ' || c == '\t';
    let mut from = a;
    let mut to = b + 1;
    if to < chars.len() && ws(chars[to]) {
        while to < chars.len() && ws(chars[to]) {
            to += 1;
        }
    } else {
        while from > 0 && ws(chars[from - 1]) {
            from -= 1;
        }
    }
    Some((ls + from, ls + to))
}

/// `ip`/`ap`: lines of the paragraph around `pos`, as a char range of whole lines.
pub fn paragraph_object(rope: &Rope, pos: usize, around: bool) -> (usize, usize) {
    paragraph_object_n(rope, pos, around, 1)
}

/// `[count]ip` / `[count]ap`. A run of blank lines is a paragraph too. `ap` adds the blank lines after (or, at the
/// end of the file, before) the paragraph.
pub fn paragraph_object_n(rope: &Rope, pos: usize, around: bool, count: usize) -> (usize, usize) {
    let last = text::last_line(rope);
    let line = text::line_of(rope, pos);
    let blank = text::is_blank_line(rope, line);
    let mut a = line;
    while a > 0 && text::is_blank_line(rope, a - 1) == blank {
        a -= 1;
    }
    // one run of lines with the same blankness, starting at `l`
    let run = |l: usize| {
        let k = text::is_blank_line(rope, l);
        let mut b = l;
        while b < last && text::is_blank_line(rope, b + 1) == k {
            b += 1;
        }
        b
    };
    let mut b = run(line);
    let runs = if around { count * 2 } else { count };
    let mut last_blank = blank;
    for _ in 1..runs {
        if b >= last {
            break;
        }
        b = run(b + 1);
        last_blank = !last_blank;
    }
    if around && !blank && !last_blank {
        // no blank lines after the paragraph: take the ones before it
        while a > 0 && text::is_blank_line(rope, a - 1) {
            a -= 1;
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
    fn word_end_backward() {
        let r = Rope::from_str("foo bar\n\nbaz");
        assert_eq!(prev_word_end(&r, 6, false), 2);
        assert_eq!(prev_word_end(&r, 7, false), 6); // on the line break
        assert_eq!(prev_word_end(&r, 10, false), 8); // empty line
        assert_eq!(prev_word_end(&r, 8, false), 6);
        assert_eq!(prev_word_end(&r, 1, false), 0);
        assert_eq!(prev_word_end(&Rope::new(), 0, false), 0);
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
        let r = Rope::from_str("f(a(b))");
        assert_eq!(pair_object_n(&r, 4, '(', ')', false, 2), Some((2, 6)));
        assert_eq!(pair_object_n(&r, 4, '(', ')', false, 3), None);
        let r = Rope::from_str("<a <b>>");
        assert_eq!(pair_object(&r, 1, '<', '>', false), Some((1, 6)));
        // block on its own lines: from the line after `{` to the start of the `}` line
        let r = Rope::from_str("x {\n  a\n}");
        assert_eq!(pair_object(&r, 5, '{', '}', false), Some((4, 8)));
    }

    #[test]
    fn objects() {
        let r = Rope::from_str("let x = \"hi there\";");
        assert_eq!(quote_object(&r, 10, '"', false), Some((9, 17)));
        assert_eq!(quote_object(&r, 0, '"', true), Some((7, 18)));
        assert_eq!(word_object(&r, 1, false, false), (0, 3));
        assert_eq!(word_object(&r, 1, false, true), (0, 4));
        let r = Rope::from_str("foo   bar baz");
        assert_eq!(word_object(&r, 4, false, false), (3, 6));
        assert_eq!(word_object(&r, 4, false, true), (3, 9));
        assert_eq!(word_object_n(&r, 0, false, true, 2), (0, 10));
        assert_eq!(word_object_n(&r, 0, false, false, 3), (0, 9));
        let r = Rope::from_str("a \"b\" c \"d\"");
        assert_eq!(quote_object(&r, 6, '"', false), Some((5, 8)));
    }

    #[test]
    fn paragraphs() {
        let r = Rope::from_str("a\nb\n\nc\n\nd");
        assert_eq!(paragraph_object(&r, 0, false), (0, 4));
        assert_eq!(paragraph_object(&r, 0, true), (0, 5));
        assert_eq!(paragraph_object_n(&r, 0, false, 2), (0, 5));
        // the last paragraph takes the blank lines before it
        assert_eq!(paragraph_object(&r, 8, true), (7, 9));
    }
}
