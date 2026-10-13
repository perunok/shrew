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

/// Matches on `line` as char ranges in the rope.
pub fn line_matches(rope: &Rope, re: &Regex, line: usize, out: &mut Vec<(usize, usize)>) {
    let slice = rope.line(line);
    let start = text::line_start(rope, line);
    let s: Cow<str> = slice.into();
    let s = s.strip_suffix('\n').unwrap_or(&s);
    for m in re.find_iter(s) {
        if m.start() == m.end() {
            continue;
        }
        out.push((start + slice.byte_to_char(m.start()), start + slice.byte_to_char(m.end())));
    }
}

/// All matches on lines `[first, last]`.
pub fn matches_in(rope: &Rope, re: &Regex, first: usize, last: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let last = last.min(text::last_line(rope));
    for l in first..=last {
        line_matches(rope, re, l, &mut out);
    }
    out
}

/// The next match strictly after (or before) `pos`, wrapping around the file. Returns `(from, to, wrapped)`.
pub fn find(rope: &Rope, re: &Regex, pos: usize, forward: bool) -> Option<(usize, usize, bool)> {
    let lines = rope.len_lines();
    let here = text::line_of(rope, pos);
    let mut buf = Vec::new();
    for step in 0..=lines {
        let line = if forward { (here + step) % lines } else { (here + lines - step % lines) % lines };
        let wrapped = if forward { here + step >= lines } else { step > here };
        buf.clear();
        line_matches(rope, re, line, &mut buf);
        let hit = if step == 0 {
            if forward { buf.iter().find(|m| m.0 > pos) } else { buf.iter().rev().find(|m| m.0 < pos) }
        } else if step == lines {
            // back on the starting line after a full wrap
            if forward { buf.iter().find(|m| m.0 <= pos) } else { buf.iter().rev().find(|m| m.0 >= pos) }
        } else if forward {
            buf.first()
        } else {
            buf.last()
        };
        if let Some(&(a, b)) = hit {
            return Some((a, b, wrapped || step == lines));
        }
    }
    None
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
}
