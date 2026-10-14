//! Text helpers for the tools, on ropes and strings: 1-based lines and columns as assistants see them, `cat -n`
//! style listings, and the edits (`edit_buffer`, `replace_lines`, `write_buffer`) computed as shrew-core changes in
//! char offsets. Pure functions: they run on the HTTP worker thread, never on the UI thread.

use shrew_core::Change;
use shrew_core::ropey::Rope;

/// Longest line shown by listings (chars); longer ones are cut with a note.
pub const MAX_LINE_CHARS: usize = 2000;

/// Lines as people count them: an empty text has none, and a final line break does not start another.
pub fn line_count(rope: &Rope) -> usize {
    let n = rope.len_chars();
    if n == 0 {
        0
    } else if rope.char(n - 1) == '\n' {
        rope.len_lines() - 1
    } else {
        rope.len_lines()
    }
}

/// 1-based (line, column) of a char position; columns count chars.
pub fn line_col(rope: &Rope, pos: usize) -> (usize, usize) {
    let pos = pos.min(rope.len_chars());
    let line = rope.char_to_line(pos);
    (line + 1, pos - rope.line_to_char(line) + 1)
}

/// The char position of a 1-based (line, column), clamped to the text (a column past the end of the line is the
/// line end, a line past the end is the end of the text).
pub fn pos_of(rope: &Rope, line: usize, col: usize) -> usize {
    let line0 = line.saturating_sub(1);
    if line0 >= rope.len_lines() {
        return rope.len_chars();
    }
    let start = rope.line_to_char(line0);
    let slice = rope.line(line0);
    let mut len = slice.len_chars();
    if len > 0 && slice.char(len - 1) == '\n' {
        len -= 1;
    }
    start + col.saturating_sub(1).min(len)
}

/// Text of 0-based line `l` without its line break.
fn line_str(rope: &Rope, l: usize) -> String {
    let mut s: String = rope.line(l).into();
    if s.ends_with('\n') {
        s.pop();
    }
    s
}

/// Lines `first..=last` (1-based, clamped) as `cat -n` shows them: the number right-aligned in 6 columns, a tab, the
/// line. Stops early once `max_chars` are written. Returns the text and the last line actually listed (0: none).
pub fn numbered(rope: &Rope, first: usize, last: usize, max_chars: usize) -> (String, usize) {
    let mut out = String::new();
    let last = last.min(line_count(rope));
    let mut shown = 0;
    for l in first.max(1)..=last {
        let mut s = line_str(rope, l - 1);
        let n = s.chars().count();
        if n > MAX_LINE_CHARS {
            s = s.chars().take(MAX_LINE_CHARS).collect();
            s.push_str(&format!(" [... {} more chars]", n - MAX_LINE_CHARS));
        }
        if !out.is_empty() && out.len() + s.len() + 8 > max_chars {
            break;
        }
        out.push_str(&format!("{l:>6}\t{s}\n"));
        shown = l;
    }
    (out, shown)
}

/// Turns sorted byte offsets of `text` into char offsets in one pass.
fn byte_to_char_offsets(text: &str, bytes: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut chars = 0;
    let mut at = 0;
    for &b in bytes {
        chars += text[at..b].chars().count();
        at = b;
        out.push(chars);
    }
    out
}

/// `old` → `new` in `text`, exactly. `old` must occur once unless `all`. CRLF in the arguments is treated as LF
/// (buffers hold LF; shrew writes CRLF back for CRLF files).
pub fn replace_exact(text: &str, old: &str, new: &str, all: bool) -> Result<Vec<Change>, String> {
    let old = old.replace("\r\n", "\n");
    let new = new.replace("\r\n", "\n");
    if old.is_empty() {
        return Err("old_string is empty; use replace_lines or write_buffer to insert text.".into());
    }
    if old == new {
        return Err("old_string and new_string are the same.".into());
    }
    let starts: Vec<usize> = text.match_indices(old.as_str()).map(|(i, _)| i).collect();
    match (starts.len(), all) {
        (0, _) => {
            return Err(
                "old_string was not found in the buffer (it must match exactly, including whitespace and indentation)."
                    .into(),
            );
        }
        (1, _) | (_, true) => {}
        (n, false) => {
            return Err(format!(
                "old_string appears {n} times; include more surrounding lines to make it unique, or set replace_all."
            ));
        }
    }
    let from = byte_to_char_offsets(text, &starts);
    let len = old.chars().count();
    Ok(from.into_iter().map(|f| Change::replace(f, f + len, new.clone())).collect())
}

/// Replaces lines `start..=end` (1-based, inclusive) with `text`. `end = start - 1` inserts before `start`;
/// `start = line_count + 1` appends. Empty `text` deletes the lines. A missing final line break is added unless
/// the replaced lines were the end of a text without one.
pub fn replace_lines(rope: &Rope, start: usize, end: usize, text: &str) -> Result<Change, String> {
    let total = line_count(rope);
    if start == 0 || start > total + 1 {
        return Err(format!("start_line must be 1 to {} (the buffer has {total} lines).", total + 1));
    }
    if end + 1 < start || end > total {
        return Err(format!("end_line must be {} to {total} (start_line - 1 inserts without replacing).", start - 1));
    }
    let len = rope.len_chars();
    let from = rope.line_to_char(start - 1).min(len);
    // the end of line `end`, including its line break
    let to = if end < start { from } else { rope.line_to_char(end).min(len) };
    let mut new = text.replace("\r\n", "\n");
    let ends_without_break = to == len && len > 0 && rope.char(len - 1) != '\n';
    if from == len && ends_without_break && !new.is_empty() {
        // appending to a text whose last line has no break: start a new line
        new.insert(0, '\n');
    }
    if !new.is_empty()
        && !new.ends_with('\n')
        && !(ends_without_break && to > from)
        && !(from == len && ends_without_break)
    {
        new.push('\n');
    }
    Ok(Change::replace(from, to, new))
}

/// The smallest single change turning `old` into `new` (common prefix and suffix kept, so cursors and syntax state
/// outside the changed part stay put). None when they are equal.
pub fn minimal_change(old: &Rope, new: &str) -> Option<Change> {
    let new_chars: Vec<char> = new.chars().collect();
    let old_len = old.len_chars();
    let mut prefix = 0;
    for (a, b) in old.chars().zip(new_chars.iter()) {
        if a != *b {
            break;
        }
        prefix += 1;
    }
    if prefix == old_len && prefix == new_chars.len() {
        return None;
    }
    let max_suffix = (old_len - prefix).min(new_chars.len() - prefix);
    let mut suffix = 0;
    let mut it = old.chars_at(old_len);
    while suffix < max_suffix {
        match it.prev() {
            Some(c) if c == new_chars[new_chars.len() - 1 - suffix] => suffix += 1,
            _ => break,
        }
    }
    let text: String = new_chars[prefix..new_chars.len() - suffix].iter().collect();
    Some(Change::replace(prefix, old_len - suffix, text))
}

/// Cuts `s` to about `max` chars around char `at` (for match previews).
pub fn preview(s: &str, at: usize, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let start = at.saturating_sub(max / 4).min(n - max);
    let body: String = s.chars().skip(start).take(max).collect();
    format!("{}{body}{}", if start > 0 { "…" } else { "" }, if start + max < n { "…" } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(rope: &str, c: Change) -> String {
        let mut r = Rope::from_str(rope);
        shrew_core::ChangeSet::single(c).apply(&mut r);
        r.to_string()
    }

    #[test]
    fn counts_lines_like_people() {
        assert_eq!(line_count(&Rope::from_str("")), 0);
        assert_eq!(line_count(&Rope::from_str("a")), 1);
        assert_eq!(line_count(&Rope::from_str("a\n")), 1);
        assert_eq!(line_count(&Rope::from_str("a\nb")), 2);
        assert_eq!(line_count(&Rope::from_str("a\n\n")), 2);
    }

    #[test]
    fn lines_and_columns_round_trip() {
        let r = Rope::from_str("héllo\nwörld\n");
        assert_eq!(line_col(&r, 0), (1, 1));
        assert_eq!(line_col(&r, 7), (2, 2));
        assert_eq!(pos_of(&r, 2, 2), 7);
        assert_eq!(pos_of(&r, 1, 99), 5);
        assert_eq!(pos_of(&r, 9, 1), r.len_chars());
    }

    #[test]
    fn numbered_listing() {
        let r = Rope::from_str("a\nb\nc\n");
        assert_eq!(numbered(&r, 2, 9, 10_000), ("     2\tb\n     3\tc\n".to_string(), 3));
        let long = format!("{}\n", "x".repeat(MAX_LINE_CHARS + 5));
        let (s, _) = numbered(&Rope::from_str(&long), 1, 1, usize::MAX);
        assert!(s.ends_with("[... 5 more chars]\n"));
        let (_, last) = numbered(&r, 1, 3, 12);
        assert_eq!(last, 1);
    }

    #[test]
    fn exact_replacement_checks_uniqueness() {
        let t = "fn a() {}\nfn b() {}\n";
        let c = replace_exact(t, "fn b", "fn c", false).unwrap();
        assert_eq!(apply(t, c[0].clone()), "fn a() {}\nfn c() {}\n");
        assert!(replace_exact(t, "fn", "x", false).unwrap_err().contains("2 times"));
        assert_eq!(replace_exact(t, "fn", "x", true).unwrap().len(), 2);
        assert!(replace_exact(t, "nope", "x", false).unwrap_err().contains("not found"));
        assert!(replace_exact(t, "", "x", false).is_err());
        // char offsets, not bytes
        let c = replace_exact("ä€x", "x", "y", false).unwrap();
        assert_eq!(c[0], Change::replace(2, 3, "y"));
        // CRLF from the client matches LF in the buffer
        assert_eq!(replace_exact("a\nb", "a\r\nb", "c", false).unwrap()[0], Change::replace(0, 3, "c"));
    }

    #[test]
    fn line_replacement() {
        let t = "1\n2\n3\n";
        let r = Rope::from_str(t);
        assert_eq!(apply(t, replace_lines(&r, 2, 2, "two").unwrap()), "1\ntwo\n3\n");
        assert_eq!(apply(t, replace_lines(&r, 2, 3, "").unwrap()), "1\n");
        assert_eq!(apply(t, replace_lines(&r, 2, 1, "1.5\n").unwrap()), "1\n1.5\n2\n3\n");
        assert_eq!(apply(t, replace_lines(&r, 4, 3, "4").unwrap()), "1\n2\n3\n4\n");
        assert!(replace_lines(&r, 5, 5, "x").is_err());
        assert!(replace_lines(&r, 2, 4, "x").is_err());
        // no final line break: kept that way, and appending starts a new line
        let t = "1\n2";
        let r = Rope::from_str(t);
        assert_eq!(apply(t, replace_lines(&r, 2, 2, "two").unwrap()), "1\ntwo");
        assert_eq!(apply(t, replace_lines(&r, 3, 2, "3").unwrap()), "1\n2\n3");
        assert_eq!(apply("", replace_lines(&Rope::from_str(""), 1, 0, "x").unwrap()), "x\n");
    }

    #[test]
    fn minimal_change_keeps_prefix_and_suffix() {
        let r = Rope::from_str("hello brave world");
        assert_eq!(minimal_change(&r, "hello new world"), Some(Change::replace(6, 11, "new")));
        assert_eq!(minimal_change(&r, "hello brave world"), None);
        assert_eq!(minimal_change(&Rope::from_str("aaa"), "aa"), Some(Change::replace(2, 3, "")));
        assert_eq!(minimal_change(&Rope::from_str(""), "x"), Some(Change::replace(0, 0, "x")));
    }

    #[test]
    fn previews_are_cut_around_the_match() {
        assert_eq!(preview("short", 0, 10), "short");
        let p = preview(&"ab".repeat(100), 100, 20);
        assert!(p.starts_with('…') && p.ends_with('…') && p.chars().count() == 22);
    }
}
