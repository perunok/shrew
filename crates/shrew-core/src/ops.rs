//! Editing operations shared by the vim and Sublime layers. They act on every cursor of a buffer and treat ranges
//! as caret selections (`[from, to)`); the vim layer converts its inclusive visual ranges before calling in.

use crate::change::{Change, ChangeSet};
use crate::document::Buffer;
use crate::motion;
use crate::search;
use crate::selection::{Range, Selection};
use crate::text::{self, CharClass};
use ropey::Rope;

#[derive(Clone, Debug)]
pub struct Opts {
    pub tab_width: usize,
    /// One level of indentation (spaces or a tab).
    pub indent: String,
    pub auto_pairs: bool,
    pub auto_indent: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { tab_width: 4, indent: "    ".into(), auto_pairs: true, auto_indent: true }
    }
}

/// What one range turns into.
pub enum Edit {
    /// Apply the change; the new range is `(anchor, head)` offsets from the change start in the new text.
    Change(Change, usize, usize),
    /// No text change; move the caret to this position (old coordinates).
    Move(usize),
    /// Leave the range as it is.
    Keep,
}

/// Runs `f` on every range and applies the resulting edits in one step.
pub fn edit_each(buf: &mut Buffer, mut f: impl FnMut(&Rope, Range) -> Edit) {
    let ranges: Vec<Range> = buf.sel.ranges().to_vec();
    let primary = buf.sel.primary_index();
    let mut edits = Vec::with_capacity(ranges.len());
    {
        let rope = buf.rope();
        for r in &ranges {
            edits.push(f(rope, *r));
        }
    }
    let mut changes = Vec::new();
    let mut new_ranges = Vec::with_capacity(ranges.len());
    let mut delta: isize = 0;
    let shift = |p: usize, d: isize| (p as isize + d).max(0) as usize;
    for (r, e) in ranges.iter().zip(edits) {
        match e {
            Edit::Change(c, a, h) => {
                let start = shift(c.from, delta);
                new_ranges.push(Range::new(start + a, start + h));
                delta += c.text.chars().count() as isize - (c.to - c.from) as isize;
                changes.push(c);
            }
            Edit::Move(p) => new_ranges.push(Range::point(shift(p, delta))),
            Edit::Keep => new_ranges.push(Range { anchor: shift(r.anchor, delta), head: shift(r.head, delta), col: r.col }),
        }
    }
    buf.apply(ChangeSet::new(changes), Some(Selection::new(new_ranges, primary)));
}

fn closer_for(c: char) -> Option<char> {
    Some(match c {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' => '"',
        '\'' => '\'',
        '`' => '`',
        _ => return None,
    })
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '`')
}

/// Types `s` at every cursor, replacing selections.
pub fn insert_str(buf: &mut Buffer, s: &str) {
    let n = s.chars().count();
    edit_each(buf, |_, r| Edit::Change(Change::replace(r.from(), r.to(), s), n, n));
}

/// Types one character, with auto-pairing and wrapping of selections.
pub fn insert_char(buf: &mut Buffer, c: char, opts: &Opts) {
    if !opts.auto_pairs {
        let mut tmp = [0u8; 4];
        return insert_str(buf, c.encode_utf8(&mut tmp));
    }
    edit_each(buf, |rope, r| {
        let next = text::char_at(rope, r.head);
        let prev = r.from().checked_sub(1).and_then(|p| text::char_at(rope, p));
        if !r.is_empty() {
            if let Some(close) = closer_for(c) {
                // wrap the selection, keep it selected
                let inner = rope.slice(r.from()..r.to()).to_string();
                let n = inner.chars().count();
                let ch = Change::replace(r.from(), r.to(), format!("{c}{inner}{close}"));
                return if r.forward() { Edit::Change(ch, 1, n + 1) } else { Edit::Change(ch, n + 1, 1) };
            }
            return Edit::Change(Change::replace(r.from(), r.to(), c.to_string()), 1, 1);
        }
        // step over a closer that is already there
        if (motion::is_close_bracket(c) || is_quote(c)) && next == Some(c) {
            return Edit::Move(r.head + 1);
        }
        let next_ok = next.is_none_or(|n| n.is_whitespace() || motion::is_close_bracket(n) || matches!(n, ',' | ';' | '.' | ':'));
        if let Some(close) = closer_for(c) {
            let pair = if is_quote(c) {
                let prev_ok = prev.is_none_or(|p| !text::is_word_char(p) && !matches!(p, '&' | '<' | '\\') && !is_quote(p));
                prev_ok && next_ok
            } else {
                next_ok
            };
            if pair {
                return Edit::Change(Change::insert(r.head, format!("{c}{close}")), 1, 1);
            }
        }
        Edit::Change(Change::insert(r.head, c.to_string()), 1, 1)
    });
}

pub fn backspace(buf: &mut Buffer, opts: &Opts) {
    edit_each(buf, |rope, r| {
        if !r.is_empty() {
            return Edit::Change(Change::delete(r.from(), r.to()), 0, 0);
        }
        let pos = r.head;
        if pos == 0 {
            return Edit::Keep;
        }
        let prev = rope.char(pos - 1);
        if opts.auto_pairs && closer_for(prev).is_some_and(|c| text::char_at(rope, pos) == Some(c)) {
            return Edit::Change(Change::delete(pos - 1, pos + 1), 0, 0);
        }
        // inside leading spaces: back to the previous indent stop
        let line = text::line_of(rope, pos);
        let ls = text::line_start(rope, line);
        if prev == ' ' && pos > ls && rope.slice(ls..pos).chars().all(|c| c == ' ') {
            let col = pos - ls;
            let w = opts.tab_width.max(1);
            let back = if col % w == 0 { w } else { col % w };
            return Edit::Change(Change::delete(pos - back.min(col), pos), 0, 0);
        }
        Edit::Change(Change::delete(pos - 1, pos), 0, 0)
    });
}

pub fn delete_forward(buf: &mut Buffer) {
    edit_each(buf, |rope, r| {
        if !r.is_empty() {
            Edit::Change(Change::delete(r.from(), r.to()), 0, 0)
        } else if r.head < rope.len_chars() {
            Edit::Change(Change::delete(r.head, r.head + 1), 0, 0)
        } else {
            Edit::Keep
        }
    });
}

/// Deletes from each caret back to the start of the word (Ctrl+Backspace, insert-mode Ctrl+W).
pub fn delete_word_back(buf: &mut Buffer) {
    edit_each(buf, |rope, r| {
        if !r.is_empty() {
            return Edit::Change(Change::delete(r.from(), r.to()), 0, 0);
        }
        let ls = text::line_start(rope, text::line_of(rope, r.head));
        let to = if r.head > ls { motion::prev_word_start(rope, r.head, false).max(ls) } else { r.head.saturating_sub(1) };
        Edit::Change(Change::delete(to, r.head), 0, 0)
    });
}

pub fn delete_word_forward(buf: &mut Buffer) {
    edit_each(buf, |rope, r| {
        if !r.is_empty() {
            return Edit::Change(Change::delete(r.from(), r.to()), 0, 0);
        }
        let le = text::line_end(rope, text::line_of(rope, r.head));
        let to = if r.head < le { motion::next_word_start(rope, r.head, false).min(le) } else { (r.head + 1).min(rope.len_chars()) };
        Edit::Change(Change::delete(r.head, to), 0, 0)
    });
}

pub fn delete_to_line_start(buf: &mut Buffer) {
    edit_each(buf, |rope, r| {
        let ls = text::line_start(rope, text::line_of(rope, r.head));
        Edit::Change(Change::delete(ls, r.head.max(ls)), 0, 0)
    });
}

/// Last non-whitespace char in `[from, to)`.
fn last_non_ws(rope: &Rope, from: usize, to: usize) -> Option<char> {
    (from..to).rev().map(|i| rope.char(i)).find(|c| !c.is_whitespace())
}

fn indent_for_newline(rope: &Rope, pos: usize, opts: &Opts) -> (String, bool, bool) {
    if !opts.auto_indent {
        return (String::new(), false, false);
    }
    let line = text::line_of(rope, pos);
    let ls = text::line_start(rope, line);
    let indent: String = rope.slice(ls..pos).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let before = last_non_ws(rope, ls, pos);
    let opens = before.is_some_and(|c| matches!(c, '(' | '[' | '{' | ':'));
    let next = text::char_at(rope, pos);
    let closes = before.and_then(closer_for).is_some_and(|c| next == Some(c) && motion::is_close_bracket(c));
    (indent, opens, closes)
}

/// Enter: new line with the indentation carried over, one level deeper after an opening bracket, and the closing
/// bracket pushed to its own line when the caret was between a pair.
pub fn newline(buf: &mut Buffer, opts: &Opts) {
    edit_each(buf, |rope, r| {
        let (indent, opens, closes) = indent_for_newline(rope, r.from(), opts);
        let inner = if opens { format!("{indent}{}", opts.indent) } else { indent.clone() };
        let mut s = format!("\n{inner}");
        let caret = s.chars().count();
        if closes {
            s.push('\n');
            s.push_str(&indent);
        }
        Edit::Change(Change::replace(r.from(), r.to(), s), caret, caret)
    });
}

/// `o` / `O` / Ctrl+Enter: a new indented line below (or above) each cursor's line.
pub fn open_line(buf: &mut Buffer, below: bool, opts: &Opts) {
    edit_each(buf, |rope, r| {
        let line = text::line_of(rope, r.head);
        let mut indent = if opts.auto_indent { text::indent_of(rope, line) } else { String::new() };
        if below {
            let le = text::line_end(rope, line);
            if opts.auto_indent && last_non_ws(rope, text::line_start(rope, line), le).is_some_and(|c| matches!(c, '{' | '[' | '(' | ':')) {
                indent.push_str(&opts.indent);
            }
            let s = format!("\n{indent}");
            let n = s.chars().count();
            Edit::Change(Change::insert(le, s), n, n)
        } else {
            let ls = text::line_start(rope, line);
            let n = indent.chars().count();
            Edit::Change(Change::insert(ls, format!("{indent}\n")), n, n)
        }
    });
}

/// Distinct lines touched by the selection. A range ending at column 0 of a line does not count that line.
pub fn selected_lines(rope: &Rope, sel: &Selection) -> Vec<usize> {
    let mut lines = Vec::new();
    for r in sel.ranges() {
        let a = text::line_of(rope, r.from());
        let mut b = text::line_of(rope, r.to());
        if b > a && r.to() == text::line_start(rope, b) {
            b -= 1;
        }
        for l in a..=b {
            if lines.last() != Some(&l) {
                lines.push(l);
            }
        }
    }
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// Contiguous blocks of lines `(first, last)`.
fn blocks(lines: &[usize]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for &l in lines {
        match out.last_mut() {
            Some((_, e)) if *e + 1 == l => *e = l,
            _ => out.push((l, l)),
        }
    }
    out
}

pub fn indent_lines(buf: &mut Buffer, lines: &[usize], opts: &Opts) {
    let rope = buf.rope();
    let changes = lines
        .iter()
        .filter(|&&l| text::line_len(rope, l) > 0)
        .map(|&l| Change::insert(text::line_start(rope, l), opts.indent.clone()))
        .collect();
    buf.apply(ChangeSet::new(changes), None);
}

pub fn outdent_lines(buf: &mut Buffer, lines: &[usize], opts: &Opts) {
    let rope = buf.rope();
    let w = opts.tab_width.max(1);
    let changes = lines
        .iter()
        .filter_map(|&l| {
            let ls = text::line_start(rope, l);
            let mut n = 0;
            let mut col = 0;
            for c in rope.slice(ls..text::line_end(rope, l)).chars() {
                match c {
                    ' ' if col < w => {
                        n += 1;
                        col += 1;
                    }
                    '\t' if col == 0 => {
                        n = 1;
                        break;
                    }
                    _ => break,
                }
            }
            (n > 0).then(|| Change::delete(ls, ls + n))
        })
        .collect();
    buf.apply(ChangeSet::new(changes), None);
}

/// Comments or uncomments the selected lines with `token` (`//`, `#`, ...).
pub fn toggle_comment(buf: &mut Buffer, lines: &[usize], token: &str) {
    let rope = buf.rope();
    let content: Vec<usize> = lines.iter().copied().filter(|&l| !text::is_blank_line(rope, l)).collect();
    if content.is_empty() {
        return;
    }
    let commented = |l: usize| {
        let fnb = text::first_non_blank(rope, l);
        let le = text::line_end(rope, l);
        let n = token.chars().count();
        fnb + n <= le && rope.slice(fnb..fnb + n) == token
    };
    let tok_len = token.chars().count();
    let changes = if content.iter().all(|&l| commented(l)) {
        content
            .iter()
            .map(|&l| {
                let fnb = text::first_non_blank(rope, l);
                let extra = usize::from(text::char_at(rope, fnb + tok_len) == Some(' '));
                Change::delete(fnb, fnb + tok_len + extra)
            })
            .collect()
    } else {
        let min_indent = content.iter().map(|&l| text::first_non_blank(rope, l) - text::line_start(rope, l)).min().unwrap_or(0);
        content.iter().map(|&l| Change::insert(text::line_start(rope, l) + min_indent, format!("{token} "))).collect()
    };
    buf.apply(ChangeSet::new(changes), None);
}

/// Moves the selected lines up or down by one, keeping the selection on them.
pub fn move_lines(buf: &mut Buffer, up: bool) {
    let rope = buf.rope();
    let last = text::last_line(rope);
    let blocks = blocks(&selected_lines(rope, &buf.sel));
    if blocks.iter().any(|&(s, e)| if up { s == 0 } else { e >= last }) {
        return;
    }
    let mut changes = Vec::new();
    let mut shifts: Vec<(usize, usize, isize)> = Vec::new(); // (from, to, shift) for positions in a moved block
    for &(s, e) in &blocks {
        let (a, b) = if up { (s - 1, e) } else { (s, e + 1) };
        let lines: Vec<String> = (a..=b).map(|l| text::line_text(rope, l)).collect();
        let mut reordered = lines.clone();
        if up {
            reordered.rotate_left(1);
        } else {
            reordered.rotate_right(1);
        }
        let from = text::line_start(rope, a);
        let to = text::line_end(rope, b);
        changes.push(Change::replace(from, to, reordered.join("\n")));
        let other = if up { lines[0].chars().count() + 1 } else { lines[lines.len() - 1].chars().count() + 1 };
        let block_from = text::line_start(rope, s);
        let block_to = text::line_end(rope, e) + 1;
        shifts.push((block_from, block_to, if up { -(other as isize) } else { other as isize }));
    }
    let shift = |p: usize| {
        shifts.iter().find(|(f, t, _)| p >= *f && p < *t).map_or(p, |(_, _, d)| (p as isize + d) as usize)
    };
    let sel = buf.sel.transform(|r| Range { anchor: shift(r.anchor), head: shift(r.head), col: r.col });
    buf.apply(ChangeSet::new(changes), Some(sel));
}

/// Duplicates the selected lines below themselves; the cursors move onto the copy.
pub fn duplicate_lines(buf: &mut Buffer) {
    let rope = buf.rope();
    let blocks = blocks(&selected_lines(rope, &buf.sel));
    let mut changes = Vec::new();
    let mut shifts = Vec::new();
    for &(s, e) in &blocks {
        let from = text::line_start(rope, s);
        let to = text::line_end(rope, e);
        let block = rope.slice(from..to).to_string();
        let n = block.chars().count() + 1;
        changes.push(Change::insert(to, format!("\n{block}")));
        shifts.push((from, to + 1, n));
    }
    let sel = buf.sel.clone();
    buf.apply(ChangeSet::new(changes), None);
    // map() put carets before the inserted copy; move each onto it
    let mut total = 0;
    let mut out = Vec::new();
    for r in sel.ranges() {
        let before: usize = shifts.iter().filter(|(_, t, _)| *t <= r.from()).map(|(_, _, n)| *n).sum();
        let own = shifts.iter().find(|(f, t, _)| r.from() >= *f && r.from() < *t).map_or(0, |s| s.2);
        total = before + own;
        out.push(Range::new(r.anchor + total, r.head + total));
    }
    let _ = total;
    let len = buf.doc.len();
    buf.sel = Selection::new(out, sel.primary_index()).clamp(len);
}

/// Deletes whole lines; returns their text (with a trailing newline) for the register.
pub fn delete_lines(buf: &mut Buffer, lines: &[usize]) -> String {
    let rope = buf.rope();
    let len = rope.len_chars();
    let mut removed = String::new();
    let changes: Vec<Change> = blocks(lines)
        .into_iter()
        .map(|(s, e)| {
            let mut from = text::line_start(rope, s);
            let next = if e < text::last_line(rope) { text::line_start(rope, e + 1) } else { len };
            removed.push_str(&rope.slice(from..text::line_end(rope, e)).to_string());
            removed.push('\n');
            if next == len && from > 0 && e == text::last_line(rope) {
                from -= 1; // last line: take the break before it
            }
            Change::delete(from, next)
        })
        .collect();
    let primary = buf.sel.primary_index();
    let ranges: Vec<usize> = buf.sel.ranges().iter().map(|r| r.head).collect();
    let cs = ChangeSet::new(changes);
    let sel = Selection::new(ranges.iter().map(|&h| Range::point(cs.map(h, crate::change::Assoc::Before))), primary);
    buf.apply(cs, Some(sel));
    let rope = buf.rope();
    let fixed = buf.sel.transform(|r| Range::point(text::first_non_blank(rope, text::line_of(rope, r.head))));
    buf.sel = fixed;
    removed
}

/// Expands every range to whole lines (repeated: adds the next line).
pub fn select_lines(buf: &mut Buffer) {
    let rope = buf.rope();
    let len = rope.len_chars();
    let sel = buf.sel.transform(|r| {
        let a = text::line_of(rope, r.from());
        let mut b = text::line_of(rope, r.to());
        if !r.is_empty() && r.to() == text::line_start(rope, b) && b > a {
            b = (b).min(text::last_line(rope));
            // already whole lines: grab the next one
            let to = if b < text::last_line(rope) { text::line_start(rope, b + 1) } else { len };
            return Range::new(text::line_start(rope, a), to);
        }
        let to = if b < text::last_line(rope) { text::line_start(rope, b + 1) } else { len };
        Range::new(text::line_start(rope, a), to)
    });
    buf.sel = sel;
}

/// `J`: joins each cursor's line with the next, separated by one space.
pub fn join_lines(buf: &mut Buffer, lines: &[usize]) {
    let rope = buf.rope();
    let last = text::last_line(rope);
    let mut targets: Vec<usize> = lines.to_vec();
    if targets.len() > 1 {
        targets.pop(); // joining lines a..=b means joining a..b with their successor
    }
    let changes = targets
        .into_iter()
        .filter(|&l| l < last)
        .map(|l| {
            let le = text::line_end(rope, l);
            let fnb = text::first_non_blank(rope, l + 1);
            let next_empty = fnb == text::line_end(rope, l + 1);
            let next = text::char_at(rope, fnb);
            let sep = if next_empty || next == Some(')') || le == text::line_start(rope, l) { "" } else { " " };
            // trailing whitespace on this line goes too
            let mut from = le;
            while from > text::line_start(rope, l) && matches!(rope.char(from - 1), ' ' | '\t') {
                from -= 1;
            }
            Change::replace(from, fnb, sep)
        })
        .collect();
    buf.apply(ChangeSet::new(changes), None);
}

/// Adds a cursor on the line above the topmost cursor (or below the bottom one), at the same column.
pub fn add_cursor_vertical(buf: &mut Buffer, below: bool, tab_width: usize) {
    let rope = buf.rope();
    let r = if below { *buf.sel.ranges().last().unwrap() } else { buf.sel.ranges()[0] };
    let line = text::line_of(rope, r.head);
    let target = if below {
        if line >= text::last_line(rope) {
            return;
        }
        line + 1
    } else {
        if line == 0 {
            return;
        }
        line - 1
    };
    let col = r.col.unwrap_or_else(|| text::col_of(rope, r.head, tab_width));
    let pos = text::pos_at_col(rope, target, col, tab_width);
    let mut nr = Range::point(pos);
    nr.col = Some(col);
    buf.sel = buf.sel.push(nr);
}

/// One range per line of every multi-line range.
pub fn split_into_lines(buf: &mut Buffer) {
    let rope = buf.rope();
    let mut out = Vec::new();
    for r in buf.sel.ranges() {
        let a = text::line_of(rope, r.from());
        let b = text::line_of(rope, r.to());
        if a == b {
            out.push(*r);
            continue;
        }
        for l in a..=b {
            let from = if l == a { r.from() } else { text::line_start(rope, l) };
            let to = if l == b { r.to() } else { text::line_end(rope, l) };
            if l == b && to == from && l != a {
                continue;
            }
            out.push(Range::new(from, to));
        }
    }
    let n = out.len();
    buf.sel = Selection::new(out, n - 1);
}

/// Sublime's Ctrl+D. With empty carets: selects the word under each. Otherwise adds the next occurrence of the
/// primary selection's text. `whole_word` matches only whole words (set when the selection came from a word).
pub fn add_next_occurrence(buf: &mut Buffer, whole_word: bool, skip: bool) -> bool {
    let rope = buf.rope();
    if buf.sel.ranges().iter().all(|r| r.is_empty()) {
        let sel = buf.sel.transform(|r| {
            let (a, b) = motion::word_object(rope, r.head, false, false);
            if a < b && text::is_word_char(rope.char(a)) { Range::new(a, b) } else { r }
        });
        buf.sel = sel;
        return true;
    }
    let prim = buf.sel.primary();
    if prim.is_empty() {
        return false;
    }
    let needle = rope.slice(prim.from()..prim.to()).to_string();
    let pat = regex::escape(&needle);
    let pat = if whole_word { format!(r"\b{pat}\b") } else { pat };
    let Ok(re) = regex::Regex::new(&pat) else { return false };
    let after = buf.sel.ranges().iter().map(|r| r.to()).max().unwrap_or(0);
    let mut pos = after.saturating_sub(1);
    for _ in 0..buf.sel.len() + 1 {
        let Some((a, b, _)) = search::find(rope, &re, pos, true) else { return false };
        if buf.sel.ranges().iter().any(|r| r.from() == a && r.to() == b) {
            pos = a;
            continue;
        }
        let r = Range::new(a, b);
        let base = if skip { buf.sel.remove(buf.sel.primary_index()) } else { buf.sel.clone() };
        buf.sel = base.push(r);
        return true;
    }
    false
}

/// Sublime's Alt+F3 / Ctrl+Shift+L on a word: every occurrence of the primary selection's text.
pub fn select_all_occurrences(buf: &mut Buffer, whole_word: bool) {
    if buf.sel.ranges().iter().all(|r| r.is_empty()) {
        add_next_occurrence(buf, true, false);
    }
    let rope = buf.rope();
    let prim = buf.sel.primary();
    if prim.is_empty() {
        return;
    }
    let needle = rope.slice(prim.from()..prim.to()).to_string();
    let pat = regex::escape(&needle);
    let pat = if whole_word { format!(r"\b{pat}\b") } else { pat };
    let Ok(re) = regex::Regex::new(&pat) else { return };
    let all = search::matches_in(rope, &re, 0, text::last_line(rope));
    if all.is_empty() {
        return;
    }
    let primary = all.iter().position(|&(a, _)| a == prim.from()).unwrap_or(0);
    buf.sel = Selection::new(all.into_iter().map(|(a, b)| Range::new(a, b)), primary);
}

/// Word under (or just before) the caret, for `*` and word highlighting.
pub fn word_at(rope: &Rope, pos: usize) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    if len == 0 {
        return None;
    }
    let p = if text::char_at(rope, pos).is_some_and(text::is_word_char) {
        pos
    } else if pos > 0 && text::char_at(rope, pos - 1).is_some_and(text::is_word_char) {
        pos - 1
    } else {
        // vim's * uses the next word on the line
        let le = text::line_end(rope, text::line_of(rope, pos));
        (pos..le).find(|&i| text::class(rope.char(i)) == CharClass::Word)?
    };
    let (a, b) = motion::word_object(rope, p, false, false);
    (a < b).then_some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    fn buf(s: &str, sel: Selection) -> Buffer {
        let mut b = Buffer::new(1, Document::from_str(s));
        b.sel = sel;
        b
    }

    #[test]
    fn multi_cursor_typing() {
        let mut b = buf("ab\ncd\n", Selection::new([Range::point(1), Range::point(4)], 0));
        insert_str(&mut b, "X");
        b.commit();
        assert_eq!(b.rope().to_string(), "aXb\ncXd\n");
        assert_eq!(b.sel.ranges(), &[Range::point(2), Range::point(6)]);
        backspace(&mut b, &Opts::default());
        assert_eq!(b.rope().to_string(), "ab\ncd\n");
        b.undo();
        assert_eq!(b.rope().to_string(), "aXb\ncXd\n");
    }

    #[test]
    fn pairs_and_newline() {
        let o = Opts::default();
        let mut b = buf("fn f", Selection::point(4));
        insert_char(&mut b, '(', &o);
        insert_char(&mut b, ')', &o);
        insert_char(&mut b, ' ', &o);
        insert_char(&mut b, '{', &o);
        assert_eq!(b.rope().to_string(), "fn f() {}");
        newline(&mut b, &o);
        assert_eq!(b.rope().to_string(), "fn f() {\n    \n}");
        assert_eq!(b.sel.primary().head, 13);
        backspace(&mut b, &o);
        assert_eq!(b.rope().to_string(), "fn f() {\n\n}");
    }

    #[test]
    fn lines() {
        let mut b = buf("a\nb\nc", Selection::point(2));
        move_lines(&mut b, true);
        assert_eq!(b.rope().to_string(), "b\na\nc");
        assert_eq!(b.sel.primary().head, 0);
        move_lines(&mut b, false);
        move_lines(&mut b, false);
        assert_eq!(b.rope().to_string(), "a\nc\nb");
        assert_eq!(b.sel.primary().head, 4);
        duplicate_lines(&mut b);
        assert_eq!(b.rope().to_string(), "a\nc\nb\nb");
        assert_eq!(b.sel.primary().head, 6);
        let lines = selected_lines(b.rope(), &b.sel);
        toggle_comment(&mut b, &lines, "//");
        assert_eq!(b.rope().to_string(), "a\nc\nb\n// b");
        let lines = selected_lines(b.rope(), &b.sel);
        toggle_comment(&mut b, &lines, "//");
        assert_eq!(b.rope().to_string(), "a\nc\nb\nb");
        let removed = delete_lines(&mut b, &[3]);
        assert_eq!((b.rope().to_string().as_str(), removed.as_str()), ("a\nc\nb", "b\n"));
    }

    #[test]
    fn occurrences() {
        let mut b = buf("foo bar foo foobar foo", Selection::point(1));
        add_next_occurrence(&mut b, true, false);
        assert_eq!(b.sel.ranges(), &[Range::new(0, 3)]);
        add_next_occurrence(&mut b, true, false);
        add_next_occurrence(&mut b, true, false);
        assert_eq!(b.sel.ranges(), &[Range::new(0, 3), Range::new(8, 11), Range::new(19, 22)]);
        insert_str(&mut b, "x");
        assert_eq!(b.rope().to_string(), "x bar x foobar x");
    }
}
