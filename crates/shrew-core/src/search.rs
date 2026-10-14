//! Regex search, line by line, so huge files never get copied into one string.

use crate::text;
use regex::Regex;
use ropey::Rope;
use std::borrow::Cow;

/// Compiles a search pattern. Smart case: all-lowercase patterns ignore case. Vim's `\<` / `\>` become `\b`.
/// An invalid regex falls back to a literal search.
pub fn compile(pattern: &str) -> Option<Regex> {
    if pattern.is_empty() {
        return None;
    }
    let ignore_case = !pattern.chars().any(|c| c.is_uppercase());
    let pat = pattern.replace("\\<", "\\b").replace("\\>", "\\b");
    let flags = if ignore_case { "(?i)" } else { "" };
    Regex::new(&format!("{flags}{pat}")).or_else(|_| Regex::new(&format!("{flags}{}", regex::escape(pattern)))).ok()
}

/// Compiles a vim pattern (`magic`, the default; `\v` very magic and `\V` very nomagic too) into a Rust regex:
/// `\(a\|b\)`, `\{2,3}`, `\+`, `\=`, `\<word\>`, `\a`/`\l`/`\u` classes, `\c`/`\C` case flags. Smart case like
/// [`compile`]: lowercase patterns ignore case (escaped letters such as `\S` do not count as uppercase).
pub fn compile_vim(pattern: &str) -> Option<Regex> {
    let (pat, case) = vim_to_rust(pattern);
    if pattern.is_empty() {
        return None;
    }
    let ignore_case = case.unwrap_or_else(|| !has_uppercase(pattern));
    let flags = if ignore_case { "(?i)" } else { "" };
    Regex::new(&format!("{flags}{pat}")).or_else(|_| Regex::new(&format!("{flags}{}", regex::escape(pattern)))).ok()
}

/// Uppercase letters outside of backslash escapes.
fn has_uppercase(pattern: &str) -> bool {
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c.is_uppercase() {
            return true;
        }
    }
    false
}

/// Translates a vim regex; returns it with the case override from `\c` (Some(true)) or `\C` (Some(false)).
pub fn vim_to_rust(pattern: &str) -> (String, Option<bool>) {
    #[derive(PartialEq, Clone, Copy)]
    enum Magic {
        Very,
        Normal,
        No,
    }
    let mut magic = Magic::Normal;
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut case = None;
    let mut chars = pattern.chars().peekable();
    // a char that is literal in Rust regex syntax
    let lit = |out: &mut String, c: char| {
        if regex_syntax_special(c) {
            out.push('\\');
        }
        out.push(c);
    };
    while let Some(c) = chars.next() {
        if c == '\\' {
            let Some(e) = chars.next() else {
                out.push_str("\\\\");
                break;
            };
            match e {
                'v' => magic = Magic::Very,
                'm' => magic = Magic::Normal,
                'V' | 'M' => magic = Magic::No,
                'c' => case = Some(true),
                'C' => case = Some(false),
                '<' | '>' if magic != Magic::Very => out.push_str("\\b"),
                '(' | ')' | '|' if magic != Magic::Very => out.push(e),
                '+' | '?' if magic != Magic::Very => out.push(e),
                '=' if magic != Magic::Very => out.push('?'),
                '{' if magic != Magic::Very => {
                    out.push_str(&brace_count(&mut chars));
                }
                '.' | '*' | '[' | '~' | '^' | '$' if magic == Magic::No => out.push(if e == '~' { '~' } else { e }),
                's' | 'S' | 'd' | 'D' | 'w' | 'W' | 'n' | 't' | 'r' => {
                    out.push('\\');
                    out.push(e);
                }
                'a' => out.push_str("[A-Za-z]"),
                'A' => out.push_str("[^A-Za-z]"),
                'l' => out.push_str("[a-z]"),
                'L' => out.push_str("[^a-z]"),
                'u' => out.push_str("[A-Z]"),
                'U' => out.push_str("[^A-Z]"),
                'x' => out.push_str("[0-9A-Fa-f]"),
                'X' => out.push_str("[^0-9A-Fa-f]"),
                'h' => out.push_str("[A-Za-z_]"),
                'H' => out.push_str("[^A-Za-z_]"),
                'k' | 'i' | 'f' | 'p' => out.push_str("\\w"),
                'e' => out.push_str("\\x1b"),
                _ => lit(&mut out, e),
            }
            continue;
        }
        match (magic, c) {
            (Magic::No, _) => lit(&mut out, c),
            (_, '[') => {
                // a bracket class: copied, with Rust's nested-class and set-operation chars escaped
                let mut class = String::from("[");
                let mut closed = false;
                if chars.peek() == Some(&'^') {
                    class.push(chars.next().unwrap());
                }
                if chars.peek() == Some(&']') {
                    chars.next();
                    class.push_str("\\]");
                }
                while let Some(d) = chars.next() {
                    match d {
                        ']' => {
                            closed = true;
                            break;
                        }
                        '\\' => match chars.next() {
                            Some(e @ ('n' | 't' | 'd' | 's' | 'w')) => {
                                class.push('\\');
                                class.push(e);
                            }
                            Some(e) => {
                                class.push('\\');
                                class.push(e);
                            }
                            None => class.push_str("\\\\"),
                        },
                        '[' if chars.peek() == Some(&':') => {
                            // [:alpha:] and friends are the same in both
                            class.push('[');
                            for e in chars.by_ref() {
                                class.push(e);
                                if e == ']' {
                                    break;
                                }
                            }
                        }
                        '[' | '&' | '~' => {
                            class.push('\\');
                            class.push(d);
                        }
                        d => class.push(d),
                    }
                }
                if closed {
                    out.push_str(&class);
                    out.push(']');
                } else {
                    // no closing bracket: vim takes the `[` literally
                    out.push_str("\\[");
                    out.push_str(&regex::escape(&class[1..]));
                }
            }
            (Magic::Very, '<' | '>') => out.push_str("\\b"),
            (Magic::Very, '=') => out.push('?'),
            (Magic::Very, '{') => out.push_str(&brace_count(&mut chars)),
            (Magic::Very, '(' | ')' | '|' | '+' | '?') => out.push(c),
            (_, '.' | '*' | '^' | '$') => out.push(c),
            (_, c) => lit(&mut out, c),
        }
    }
    (out, case)
}

/// The rest of a vim `\{n,m}` count (after the brace): `{n,m}`, with `-` (lazy) turned into a trailing `?`.
fn brace_count(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut body = String::new();
    for d in chars.by_ref() {
        if d == '}' {
            break;
        }
        if d != '\\' {
            body.push(d);
        }
    }
    let lazy = body.starts_with('-');
    let body = body.trim_start_matches('-');
    let q = match body {
        "" => "*".to_string(),
        b if !b.contains(',') => format!("{{{b}}}"),
        b if b.starts_with(',') => format!("{{0{b}}}"),
        b => format!("{{{b}}}"),
    };
    if lazy { format!("{q}?") } else { q }
}

fn regex_syntax_special(c: char) -> bool {
    matches!(
        c,
        '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '#' | '&' | '-' | '~'
    )
}

/// A vim `:s` replacement: `&`/`\0` (the match), `\1`..`\9`, `\r`/`\n` (line break), `\t`, and the case
/// modifiers `\u` `\l` (next char) and `\U` `\L` ... `\e`/`\E` (until the end).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement(Vec<Piece>);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    Text(String),
    Group(usize),
    /// `\u` / `\l`: the next char upper (true) or lower.
    One(bool),
    /// `\U` / `\L`: upper (Some(true)) or lower until `\e` (None).
    Span(Option<bool>),
}

impl Replacement {
    pub fn parse(s: &str) -> Replacement {
        let mut out = Vec::new();
        let mut text = String::new();
        let mut chars = s.chars();
        let flush = |text: &mut String, out: &mut Vec<Piece>| {
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(text)));
            }
        };
        while let Some(c) = chars.next() {
            let piece = match c {
                '&' => Piece::Group(0),
                '\\' => match chars.next() {
                    Some(d @ '0'..='9') => Piece::Group(d as usize - '0' as usize),
                    Some('n' | 'r') => {
                        text.push('\n');
                        continue;
                    }
                    Some('t') => {
                        text.push('\t');
                        continue;
                    }
                    Some('u') => Piece::One(true),
                    Some('l') => Piece::One(false),
                    Some('U') => Piece::Span(Some(true)),
                    Some('L') => Piece::Span(Some(false)),
                    Some('e' | 'E') => Piece::Span(None),
                    Some(o) => {
                        text.push(o);
                        continue;
                    }
                    None => continue,
                },
                c => {
                    text.push(c);
                    continue;
                }
            };
            flush(&mut text, &mut out);
            out.push(piece);
        }
        flush(&mut text, &mut out);
        Replacement(out)
    }

    /// The replacement text for one match.
    pub fn expand(&self, caps: &regex::Captures, out: &mut String) {
        let mut one: Option<bool> = None;
        let mut span: Option<bool> = None;
        let push = |s: &str, one: &mut Option<bool>, span: Option<bool>, out: &mut String| {
            for c in s.chars() {
                match one.take().or(span) {
                    Some(true) => out.extend(c.to_uppercase()),
                    Some(false) => out.extend(c.to_lowercase()),
                    None => out.push(c),
                }
            }
        };
        for p in &self.0 {
            match p {
                Piece::Text(t) => push(t, &mut one, span, out),
                Piece::Group(n) => push(caps.get(*n).map_or("", |m| m.as_str()), &mut one, span, out),
                Piece::One(up) => one = Some(*up),
                Piece::Span(s) => span = *s,
            }
        }
    }
}

/// Matches on `line` as char ranges in the rope.
pub fn line_matches(rope: &Rope, re: &Regex, line: usize, out: &mut Vec<(usize, usize)>) {
    let slice = rope.line(line);
    let start = text::line_start(rope, line);
    let s: Cow<str> = slice.into();
    let s = s.strip_suffix('\n').unwrap_or(&s);
    let ascii = s.is_ascii();
    for m in re.find_iter(s) {
        if m.start() == m.end() {
            continue;
        }
        if ascii {
            out.push((start + m.start(), start + m.end()));
        } else {
            out.push((start + slice.byte_to_char(m.start()), start + slice.byte_to_char(m.end())));
        }
    }
}

// ----- whole-file scans -----
//
// Searching line by line costs a rope lookup and a regex call per line, which adds up to a second on a million
// lines. Scans instead run the regex (in multi-line mode, so `^`/`$` still mean line start/end) over blocks of
// whole lines, about 64 KB each: blocks without a match are skipped at memchr speed, and a match that does not
// cross a line break is exactly what the per-line search finds. Anything else falls back to the per-line search.

const BLOCK: usize = 64 * 1024;

thread_local! {
    static MULTILINE: std::cell::RefCell<Option<(String, Regex)>> = const { std::cell::RefCell::new(None) };
}

/// `re` with `^`/`$` matching at line breaks (cached: scans call this on every keystroke).
fn multiline(re: &Regex) -> Regex {
    MULTILINE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((p, r)) = c.as_ref()
            && p == re.as_str()
        {
            return r.clone();
        }
        let r = Regex::new(&format!("(?m){}", re.as_str())).unwrap_or_else(|_| re.clone());
        *c = Some((re.as_str().to_string(), r.clone()));
        r
    })
}

/// A block of whole lines `[first, end)` copied out of the rope.
struct Block {
    first: usize,
    end: usize,
    byte0: usize,
}

/// Blocks of about [`BLOCK`] bytes covering lines `first..=last`, front to back (or back to front).
fn blocks(rope: &Rope, first: usize, last: usize, forward: bool) -> impl Iterator<Item = Block> + '_ {
    let total = rope.len_bytes();
    let n_lines = rope.len_lines();
    let mut lo = first;
    let mut hi = last + 1; // exclusive
    std::iter::from_fn(move || {
        if lo >= hi {
            return None;
        }
        if forward {
            let b0 = rope.line_to_byte(lo);
            let target = b0 + BLOCK;
            let end = if target >= total { hi } else { (rope.byte_to_line(target) + 1).clamp(lo + 1, hi) };
            let blk = Block { first: lo, end, byte0: b0 };
            lo = end;
            Some(blk)
        } else {
            let b1 = if hi >= n_lines { total } else { rope.line_to_byte(hi) };
            let start = if b1 <= BLOCK { lo } else { rope.byte_to_line(b1 - BLOCK).clamp(lo, hi - 1) };
            let blk = Block { first: start, end: hi, byte0: rope.line_to_byte(start) };
            hi = start;
            Some(blk)
        }
    })
}

fn block_text<'a>(rope: &Rope, b: &Block, buf: &'a mut String) -> &'a str {
    let b1 = if b.end >= rope.len_lines() { rope.len_bytes() } else { rope.line_to_byte(b.end) };
    buf.clear();
    for chunk in rope.byte_slice(b.byte0..b1).chunks() {
        buf.push_str(chunk);
    }
    buf
}

/// Lines looked at one by one before switching to blocks (`n` usually finds the next match close by).
const NEAR_LINES: usize = 32;

/// The first match on lines `first..=last`.
fn first_in_lines(rope: &Rope, re: &Regex, first: usize, last: usize) -> Option<(usize, usize)> {
    let mut hits = Vec::new();
    let near = last.min(first + NEAR_LINES);
    for l in first..=near {
        line_matches(rope, re, l, &mut hits);
        if let Some(&m) = hits.first() {
            return Some(m);
        }
    }
    if near == last {
        return None;
    }
    let first = near + 1;
    let pre = multiline(re);
    let mut buf = String::new();
    for b in blocks(rope, first, last, true) {
        if !pre.is_match(block_text(rope, &b, &mut buf)) {
            continue;
        }
        for l in b.first..b.end {
            line_matches(rope, re, l, &mut hits);
            if let Some(&m) = hits.first() {
                return Some(m);
            }
        }
    }
    None
}

/// The last match on lines `first..=last`.
fn last_in_lines(rope: &Rope, re: &Regex, first: usize, last: usize) -> Option<(usize, usize)> {
    let mut hits = Vec::new();
    let near = first.max(last.saturating_sub(NEAR_LINES));
    for l in (near..=last).rev() {
        line_matches(rope, re, l, &mut hits);
        if let Some(&m) = hits.last() {
            return Some(m);
        }
    }
    if near == first {
        return None;
    }
    let last = near - 1;
    let pre = multiline(re);
    let mut buf = String::new();
    for b in blocks(rope, first, last, false) {
        if !pre.is_match(block_text(rope, &b, &mut buf)) {
            continue;
        }
        for l in (b.first..b.end).rev() {
            line_matches(rope, re, l, &mut hits);
            if let Some(&m) = hits.last() {
                return Some(m);
            }
        }
    }
    None
}

/// Calls `f(line, from, to, captures)` for every match on lines `first..=last`, in order (char positions).
/// `captures` is only filled in when `with_captures` is set (it costs more). Empty matches (`^`, `$`) are
/// reported only with `with_empty` (`:s` wants them, searching does not).
pub fn for_each_match(
    rope: &Rope,
    re: &Regex,
    (first, last): (usize, usize),
    with_captures: bool,
    with_empty: bool,
    mut f: impl FnMut(usize, usize, usize, Option<&regex::Captures>),
) {
    let last = last.min(text::last_line(rope));
    if first > last {
        return;
    }
    let pre = multiline(re);
    let mut buf = String::new();
    let count = |s: &str, ascii: bool| if ascii { s.len() } else { s.chars().count() };
    for b in blocks(rope, first, last, true) {
        let text = block_text(rope, &b, &mut buf);
        let ascii = text.is_ascii();
        let mut char0 = rope.byte_to_char(b.byte0);
        let mut line = b.first;
        let mut cursor = 0; // bytes of `text` already counted into `char0` and `line`
        let mut at = 0;
        while at <= text.len() {
            let (m, caps) = if with_captures {
                let Some(c) = pre.captures_at(text, at) else { break };
                (c.get(0).unwrap(), Some(c))
            } else {
                let Some(m) = pre.find_at(text, at) else { break };
                (m, None)
            };
            let gap = &text[cursor..m.start()];
            line += gap.bytes().filter(|&c| c == b'\n').count();
            char0 += count(gap, ascii);
            cursor = m.start();
            if m.start() == m.end() {
                // past the block's last line break is the next block's first line
                let next_block = m.start() == text.len() && text.ends_with('\n');
                if with_empty && !next_block {
                    f(line, char0, char0, caps.as_ref());
                }
                // step over one char
                at = m.end() + text[m.end()..].chars().next().map_or(1, char::len_utf8);
                continue;
            }
            if !m.as_str().contains('\n') {
                f(line, char0, char0 + count(m.as_str(), ascii), caps.as_ref());
                at = m.end();
                continue;
            }
            // the match runs over a line break: redo this line on its own, from the match start
            let ls_byte = text[..m.start()].rfind('\n').map_or(0, |i| i + 1);
            let le = text[m.start()..].find('\n').map_or(text.len(), |i| m.start() + i);
            let s = &text[ls_byte..le];
            let ls_char = char0 - count(&text[ls_byte..m.start()], ascii);
            for c in re.captures_iter(s) {
                let lm = c.get(0).unwrap();
                if (lm.start() == lm.end() && !with_empty) || ls_byte + lm.start() < m.start() {
                    continue;
                }
                let x = ls_char + count(&s[..lm.start()], ascii);
                f(line, x, x + count(lm.as_str(), ascii), Some(&c));
            }
            at = le + 1;
        }
    }
}

/// All matches on lines `[first, last]`.
pub fn matches_in(rope: &Rope, re: &Regex, first: usize, last: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let last = last.min(text::last_line(rope));
    if last.saturating_sub(first) < 64 {
        // a screenful (search highlighting): per line is cheapest
        for l in first..=last {
            line_matches(rope, re, l, &mut out);
        }
        return out;
    }
    for_each_match(rope, re, (first, last), false, false, |_, a, b, _| out.push((a, b)));
    out
}

/// Like [`find`], but looks at no more than about `budget` bytes past the cursor line and does not wrap on a file
/// bigger than that: incremental search on a huge file stays interactive (Enter runs the full [`find`]).
pub fn find_near(rope: &Rope, re: &Regex, pos: usize, forward: bool, budget: usize) -> Option<(usize, usize, bool)> {
    if rope.len_bytes() <= budget {
        return find(rope, re, pos, forward);
    }
    let last = text::last_line(rope);
    let here = text::line_of(rope, pos);
    let mut hits = Vec::new();
    line_matches(rope, re, here, &mut hits);
    if forward {
        if let Some(&(a, b)) = hits.iter().find(|m| m.0 > pos) {
            return Some((a, b, false));
        }
        if here == last {
            return None;
        }
        let end = (rope.line_to_byte(here + 1) + budget).min(rope.len_bytes());
        first_in_lines(rope, re, here + 1, rope.byte_to_line(end).min(last)).map(|(a, b)| (a, b, false))
    } else {
        if let Some(&(a, b)) = hits.iter().rev().find(|m| m.0 < pos) {
            return Some((a, b, false));
        }
        if here == 0 {
            return None;
        }
        let start = rope.line_to_byte(here).saturating_sub(budget);
        last_in_lines(rope, re, rope.byte_to_line(start), here - 1).map(|(a, b)| (a, b, false))
    }
}

/// The next match strictly after (or before) `pos`, wrapping around the file. Returns `(from, to, wrapped)`.
pub fn find(rope: &Rope, re: &Regex, pos: usize, forward: bool) -> Option<(usize, usize, bool)> {
    let last = text::last_line(rope);
    let here = text::line_of(rope, pos);
    let mut hits = Vec::new();
    line_matches(rope, re, here, &mut hits);
    if forward {
        if let Some(&(a, b)) = hits.iter().find(|m| m.0 > pos) {
            return Some((a, b, false));
        }
        if here < last
            && let Some((a, b)) = first_in_lines(rope, re, here + 1, last)
        {
            return Some((a, b, false));
        }
        // from the top; on the starting line only what is not after `pos` is left
        first_in_lines(rope, re, 0, here).map(|(a, b)| (a, b, true))
    } else {
        if let Some(&(a, b)) = hits.iter().rev().find(|m| m.0 < pos) {
            return Some((a, b, false));
        }
        if here > 0
            && let Some((a, b)) = last_in_lines(rope, re, 0, here - 1)
        {
            return Some((a, b, false));
        }
        last_in_lines(rope, re, here, last).map(|(a, b)| (a, b, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_wraps() {
        let r = Rope::from_str("foo bar\nbar Foo\nbaz");
        let re = compile("foo").unwrap();
        assert_eq!(find(&r, &re, 0, true), Some((12, 15, false)));
        assert_eq!(find(&r, &re, 12, true), Some((0, 3, true)));
        assert_eq!(find(&r, &re, 12, false), Some((0, 3, false)));
        assert_eq!(find(&r, &re, 0, false), Some((12, 15, true)));
        let re = compile("Foo").unwrap();
        assert_eq!(matches_in(&r, &re, 0, 2), vec![(12, 15)]);
    }

    /// The block scans must find exactly what the line-by-line search finds.
    #[test]
    fn block_scans_match_line_scans() {
        let mut text = String::new();
        for i in 0..12_000 {
            text.push_str(match i % 7 {
                0 => "foo bar\n",
                1 => "  é foo\tbaz\n",
                2 => "\n",
                3 => "barfoo foo foo\n",
                4 => "x\n",
                5 => "日本 foo 語\n",
                _ => "nothing here\n",
            });
        }
        let r = Rope::from_str(&text);
        let last = text::last_line(&r);
        for pat in ["foo", "^foo", "foo$", "o\\s", "\\s*$", "^", "[^a]o", "\\bfoo\\b", "é", "語$", "zzz"] {
            let re = compile_vim(pat).unwrap();
            let mut want = Vec::new();
            for l in 0..=last {
                line_matches(&r, &re, l, &mut want);
            }
            assert_eq!(matches_in(&r, &re, 0, last), want, "pattern {pat}");
            // forward from a few places: the first match after the position, else wrapped
            for &pos in &[0, 1, 5000, r.len_chars() / 2, r.len_chars() - 1] {
                let exp = want
                    .iter()
                    .find(|m| m.0 > pos)
                    .map(|&(a, b)| (a, b, false))
                    .or_else(|| want.first().map(|&(a, b)| (a, b, true)));
                assert_eq!(find(&r, &re, pos, true), exp, "pattern {pat} from {pos}");
                let exp = want
                    .iter()
                    .rev()
                    .find(|m| m.0 < pos)
                    .map(|&(a, b)| (a, b, false))
                    .or_else(|| want.last().map(|&(a, b)| (a, b, true)));
                assert_eq!(find(&r, &re, pos, false), exp, "pattern {pat} back from {pos}");
            }
        }
    }

    #[test]
    fn vim_regex_translation() {
        let t = |p: &str| vim_to_rust(p).0;
        assert_eq!(t(r"\(a\|b\)\+"), "(a|b)+");
        assert_eq!(t(r"a(b)"), r"a\(b\)");
        assert_eq!(t(r"x\{2,3}"), "x{2,3}");
        assert_eq!(t(r"x\{-}"), "x*?");
        assert_eq!(t(r"\<word\>"), r"\bword\b");
        assert_eq!(t(r"\v(a|b)+"), "(a|b)+");
        assert_eq!(t(r"[a-z]\d"), r"[a-z]\d");
        assert_eq!(vim_to_rust(r"\cFoo").1, Some(true));
        assert!(compile_vim("abc").unwrap().is_match("ABC"));
        assert!(!compile_vim("Abc").unwrap().is_match("ABC"));
        assert!(!compile_vim(r"\Cabc").unwrap().is_match("ABC"));
        // escaped letters do not make it case sensitive
        assert!(compile_vim(r"\Sbc").unwrap().is_match("ABC"));
    }

    #[test]
    fn replacement_templates() {
        let re = Regex::new("(\\w+) (\\w+)").unwrap();
        let caps = re.captures("hello world").unwrap();
        let ex = |r: &str| {
            let mut out = String::new();
            Replacement::parse(r).expand(&caps, &mut out);
            out
        };
        assert_eq!(ex(r"\2 \1"), "world hello");
        assert_eq!(ex("[&]"), "[hello world]");
        assert_eq!(ex(r"\u\1"), "Hello");
        assert_eq!(ex(r"\U\1\E!"), "HELLO!");
        assert_eq!(ex(r"a\rb"), "a\nb");
    }
}
