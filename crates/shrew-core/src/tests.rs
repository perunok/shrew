//! Key-sequence tests: type vim/Sublime keys, check text and cursor.

use crate::{Editor, KeyPress, Mode};
use crate::document::Document;

/// An editor with `text` loaded and the cursor at `|` (removed from the text).
fn ed(text: &str) -> Editor {
    let pos = text.find('|').map(|b| text[..b].chars().count()).unwrap_or(0);
    let mut e = Editor::new();
    e.install_document(Document::from_str(&text.replace('|', "")));
    e.buf_mut().sel = crate::Selection::point(pos);
    e
}

fn keys(e: &mut Editor, s: &str) {
    for k in KeyPress::parse_seq(s) {
        e.handle_key(k);
    }
}

/// Text with `|` at the primary cursor.
fn show(e: &Editor) -> String {
    let mut s = e.buf().doc.rope.to_string();
    let h = e.buf().sel.primary().head;
    let b = s.char_indices().nth(h).map_or(s.len(), |(b, _)| b);
    s.insert(b, '|');
    s
}

fn check(start: &str, input: &str, expect: &str) {
    let mut e = ed(start);
    keys(&mut e, input);
    assert_eq!(show(&e), expect, "keys {input:?} on {start:?}");
}

#[test]
fn motions() {
    check("|foo bar baz", "w", "foo |bar baz");
    check("|foo bar baz", "2w", "foo bar |baz");
    check("foo bar |baz", "b", "foo |bar baz");
    check("|foo bar", "e", "fo|o bar");
    check("|foo bar", "$", "foo ba|r");
    check("  foo|", "^", "  |foo");
    check("a|bc\ndef\nghi", "j", "abc\nd|ef\nghi");
    check("abc\nd|ef\ngh", "G", "abc\ndef\n|gh");
    check("abc\nd|ef\ngh", "gg", "|abc\ndef\ngh");
    check("|a(b)c", "f)", "a(b|)c");
    check("|a(b)c", "t)", "a(|b)c");
    check("|a(b)c", "%", "a(b|)c");
    check("abcdef\n|ab", "k$j", "abcdef\na|b");
}

#[test]
fn operators() {
    check("|foo bar baz", "dw", "|bar baz");
    check("foo |bar baz", "d2w", "foo| ");
    check("foo |bar", "cwqux<Esc>", "foo qu|x");
    check("|a\nb\nc", "dd", "|b\nc");
    check("a\nb\n|c", "dd", "a\n|b");
    check("|a\nb\nc", "2dd", "|c");
    check("|a\nb\nc", "dj", "|c");
    check("x(|abc)y", "di(", "x(|)y");
    check("x(|abc)y", "da(", "x|y");
    check("say \"h|i\"", "ci\"yo<Esc>", "say \"y|o\"");
    check("|foo", "x", "|oo");
    check("fo|o", "D", "f|o");
    check("|abc", "yyp", "abc\n|abc");
    check("|abc", "ylp", "a|abc");
    check("|ab", "~", "A|b");
    check("|abc def", "gUiw", "|ABC def");
    check("|a\nb", ">>", "    |a\nb");
    check("|x 41 y", "<C-a>", "x 4|2 y");
    check("|a\nb", "J", "a| b");
}

#[test]
fn insert_and_repeat() {
    check("|", "ihello<Esc>", "hell|o");
    check("|ab", "aX<Esc>", "a|Xb");
    check("|ab", "AX<Esc>", "ab|X");
    check("|ab", "oX<Esc>", "ab\n|X");
    check("|ab", "OX<Esc>", "|X\nab");
    check("|a a a", "cwb<Esc>w.w.", "b b |b");
    check("|abc", "x..", "|");
    check("|a\nb\nc\nd", "dd.", "|c\nd");
}

#[test]
fn undo_redo() {
    check("|foo", "ciwbar<Esc>u", "|foo");
    check("|foo", "ciwbar<Esc>u<C-r>", "ba|r");
    check("|a b", "xxuu", "|a b");
}

#[test]
fn visual() {
    check("|abc def", "vlld", "| def");
    check("|abc def", "veyP", "ab|cabc def");
    check("a\n|b\nc", "Vd", "a\n|c");
    check("|a\nb\nc", "Vjd", "|c");
    check("x(|abc)y", "vi(d", "x(|)y");
    check("|ab\ncd\nef", "<C-v>jjIX<Esc>", "|Xab\nXcd\nXef");
}

#[test]
fn search_and_ex() {
    check("|foo bar foo", "/foo<CR>", "foo bar |foo");
    check("|foo bar foo", "*", "foo bar |foo");
    check("foo bar |foo", "N", "foo bar |foo");
    let mut e = ed("|a b a");
    keys(&mut e, ":%s/a/x/g<CR>");
    assert_eq!(e.buf().doc.rope.to_string(), "x b x");
    keys(&mut e, ":1<CR>");
    assert_eq!(e.mode, Mode::Normal);
}

#[test]
fn multi_cursor() {
    // Ctrl+N selects the word, then its next occurrences; c changes all of them
    check("|foo x foo y foo", "<C-n><C-n><C-n>cbar<Esc>", "bar x bar y ba|r");
    let mut e = ed("|ab\ncd\nef");
    keys(&mut e, "<C-A-Down><C-A-Down>iX<Esc>");
    assert_eq!(e.buf().doc.rope.to_string(), "Xab\nXcd\nXef");
    assert_eq!(e.buf().sel.len(), 3);
    keys(&mut e, "<Esc>");
    assert_eq!(e.buf().sel.len(), 1);
}

#[test]
fn macros() {
    check("|1\n2\n3", "qaA!<Esc>jq2@a", "1!\n2!\n3|!");
}

#[test]
fn sublime_mode() {
    let mut e = ed("|hello");
    e.set_feature(crate::Feature::Vim, false);
    assert_eq!(e.mode, Mode::Insert);
    keys(&mut e, "<S-End>X<Esc>");
    assert_eq!(e.buf().doc.rope.to_string(), "X");
    keys(&mut e, "(<C-z>");
    assert_eq!(e.buf().doc.rope.to_string(), "X");
}
