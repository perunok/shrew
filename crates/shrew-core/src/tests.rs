//! Key-sequence tests: type vim/Sublime keys, check text and cursor.

use crate::document::Document;
use crate::{Editor, KeyPress, Mode, Mods};

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
    check("|foo", "ciwbar<Esc>u<C-r>", "|bar");
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

// ----- Sublime mode (vim off) -----

/// An editor with vim off; `|` is the caret, `[`..`]` a selection (anchor at `[`).
fn sub(text: &str) -> Editor {
    let mut e = Editor::new();
    e.set_feature(crate::Feature::Vim, false);
    e.set_feature(crate::Feature::AutoPairs, false);
    let clean: String = text.chars().filter(|c| !matches!(c, '|' | '[' | ']')).collect();
    e.install_document(Document::from_str(&clean));
    let idx = |m: char| {
        let mut n = 0;
        for c in text.chars() {
            if c == m {
                return Some(n);
            }
            if !matches!(c, '|' | '[' | ']') {
                n += 1;
            }
        }
        None
    };
    let sel = match (idx('['), idx(']'), idx('|')) {
        (Some(a), Some(z), _) => crate::Selection::single(crate::Range::new(a, z)),
        (_, _, Some(p)) => crate::Selection::point(p),
        _ => crate::Selection::point(0),
    };
    e.buf_mut().sel = sel;
    e
}

/// Text with every selection shown as `[`..`]` (anchor side first) and every caret as `|`.
fn show_all(e: &Editor) -> String {
    let rope = &e.buf().doc.rope;
    let mut marks: Vec<(usize, char)> = Vec::new();
    for r in e.buf().sel.ranges() {
        if r.is_empty() {
            marks.push((r.head, '|'));
        } else {
            marks.push((r.from(), '['));
            marks.push((r.to(), ']'));
        }
    }
    marks.sort();
    let mut out = String::new();
    let mut k = 0;
    for (i, c) in rope.chars().enumerate() {
        while k < marks.len() && marks[k].0 == i {
            out.push(marks[k].1);
            k += 1;
        }
        out.push(c);
    }
    while k < marks.len() {
        out.push(marks[k].1);
        k += 1;
    }
    out
}

fn sub_check(start: &str, input: &str, expect: &str) {
    let mut e = sub(start);
    keys(&mut e, input);
    assert_eq!(show_all(&e), expect, "keys {input:?} on {start:?}");
}

#[test]
fn sublime_motions_and_selection() {
    sub_check("|foo bar", "<S-Right><S-Right>", "[fo]o bar");
    sub_check("foo |bar", "<S-Left><S-Left>", "fo[o ]bar");
    sub_check("|foo bar baz", "<C-Right>", "foo| bar baz");
    sub_check("|foo bar baz", "<C-Right><C-Right>", "foo bar| baz");
    sub_check("foo bar |baz", "<C-Left>", "foo |bar baz");
    sub_check("|foo bar", "<C-S-Right>", "[foo] bar");
    sub_check("  fo|o", "<Home>", "  |foo");
    sub_check("  fo|o", "<Home><Home>", "|  foo");
    sub_check("  fo|o", "<Home><Home><Home>", "  |foo");
    sub_check("  fo|o", "<S-Home>", "  [fo]o");
    sub_check("|foo\nbar", "<End>", "foo|\nbar");
    sub_check("a|bc\nd", "<C-End>", "abc\nd|");
    sub_check("abc\nd|", "<C-Home>", "|abc\nd");
    sub_check("ab|cdef\nxy\nabcdef", "<Down><Down>", "abcdef\nxy\nab|cdef");
    sub_check("ab|c\nxy", "<S-Down>", "ab[c\nxy]");
    sub_check("[abc]", "<Left>", "|abc");
    sub_check("[abc]", "<Right>", "abc|");
    sub_check("[ab]c", "x", "x|c");
    sub_check("[ab]c", "<BS>", "|c");
    sub_check("|abc", "<C-a>", "[abc]");
    sub_check("ab|c\ndef", "<C-l>", "[abc\n]def");
    sub_check("ab|c\ndef", "<C-l><C-l>", "[abc\ndef]");
}

#[test]
fn sublime_line_commands() {
    sub_check("a\nb|\nc", "<C-S-k>", "a\n|c");
    sub_check("a\nb|x\nc", "<C-S-d>", "a\nbx\nb|x\nc");
    // with a selection, Ctrl+Shift+D duplicates the selected text
    sub_check("a [bx] c", "<C-S-d>", "a bx[bx] c");
    sub_check("a\nb|\nc", "<C-S-Up>", "b|\na\nc");
    sub_check("a\nb|\nc", "<C-S-Down>", "a\nc\nb|");
    sub_check("a|\nb", "<C-]>", "    a|\nb");
    sub_check("    a|\nb", "<C-[>", "a|\nb");
    sub_check("a|\nb", "<C-Enter>x", "a\nx|\nb");
    sub_check("a|\nb", "<C-S-Enter>x", "x|\na\nb");
    sub_check("[a\nb\nc]", "<C-S-l>", "[a]\n[b]\n[c]");
    sub_check("[a\nb\nc]", "<C-S-l>x", "x|\nx|\nx|");
    sub_check("a b|\n  c", "<C-j>", "a b| c");
}

#[test]
fn sublime_comments() {
    let mut e = sub("fo|o\nbar");
    e.buf_mut().doc.path = Some("x.rs".into());
    keys(&mut e, "<C-/>");
    assert_eq!(show_all(&e), "// fo|o\nbar");
    keys(&mut e, "<C-/>");
    assert_eq!(show_all(&e), "fo|o\nbar");
}

#[test]
fn sublime_occurrences() {
    sub_check("|foo x foo y foo", "<C-d>", "[foo] x foo y foo");
    sub_check("|foo x foo y foo", "<C-d><C-d>", "[foo] x [foo] y foo");
    sub_check("foo| x foo y foo", "<C-d><C-d>", "[foo] x [foo] y foo");
    sub_check("|foo x foo y foo", "<C-d><C-d><C-d>z", "z| x z| y z|");
    // Ctrl+K Ctrl+D skips the newest occurrence
    sub_check("|foo x foo y foo", "<C-d><C-d><C-k><C-d>", "[foo] x foo y [foo]");
    // whole words only when started from a caret
    sub_check("|foo foobar foo", "<C-d><C-d>", "[foo] foobar [foo]");
    // from a selection, any substring
    sub_check("[foo] foobar foo", "<C-d>", "[foo] [foo]bar foo");
    sub_check("|foo x foo", "<A-F3>", "[foo] x [foo]");
    // Esc keeps the first cursor
    sub_check("|foo x foo y", "<C-d><C-d><Esc>", "foo| x foo y");
}

#[test]
fn sublime_clipboard() {
    // copy/cut/paste of whole lines with an empty selection
    // a copied line goes in above the cursor line
    sub_check("a\nb|x\nc", "<C-c><Down><C-v>", "a\nbx\nbx\nc|");
    sub_check("a\nb|x\nc", "<C-x>", "a\n|c");
    sub_check("a\nb|x\nc", "<C-x><C-v>", "a\nbx\n|c");
    sub_check("[ab]c", "<C-c><End><C-v>", "abcab|");
    // parts copied from several selections: one per cursor, or one per line into a single cursor
    let mut e = sub("a1\na2\n|");
    e.buf_mut().sel = crate::Selection::new([crate::Range::new(0, 1), crate::Range::new(3, 4)], 0);
    keys(&mut e, "<C-c><C-End><C-v>");
    assert_eq!(e.buf().doc.rope.to_string(), "a1\na2\na\na");
    let mut e = sub("a1\na2");
    e.buf_mut().sel = crate::Selection::new([crate::Range::new(0, 1), crate::Range::new(3, 4)], 0);
    keys(&mut e, "<C-c><End>!<C-v>");
    assert_eq!(e.buf().doc.rope.to_string(), "a1!a\na2!a");
    // lines from outside distribute over as many cursors
    let mut e = sub("x\ny\nz");
    e.clipboard.set("1\n2\n3".into());
    e.buf_mut().sel =
        crate::Selection::new([crate::Range::point(1), crate::Range::point(3), crate::Range::point(5)], 0);
    keys(&mut e, "<C-v>");
    assert_eq!(e.buf().doc.rope.to_string(), "x1\ny2\nz3");
}

#[test]
fn sublime_undo_groups_words() {
    let mut e = sub("|");
    keys(&mut e, "hello world");
    keys(&mut e, "<C-z>");
    assert_eq!(e.buf().doc.rope.to_string(), "hello ");
    keys(&mut e, "<C-z>");
    assert_eq!(e.buf().doc.rope.to_string(), "");
    keys(&mut e, "<C-S-z><C-S-z>");
    assert_eq!(e.buf().doc.rope.to_string(), "hello world");
}

#[test]
fn sublime_multi_cursor_keys() {
    sub_check("a|b\ncd\nef", "<C-A-Down><C-A-Down>x", "ax|b\ncx|d\nex|f");
    sub_check("a|b\ncd\nef", "<C-A-Down><C-A-Down><Esc>", "a|b\ncd\nef");
    sub_check("ab|\ncd\nef", "<C-A-Down><BS>", "a|\nc|\nef");
    sub_check("a|b\nc", "<C-A-Down><End>", "ab|\nc|");
}

#[test]
fn mouse() {
    let mut e = sub("foo bar\nbaz");
    e.mouse_down(5, 1, Mods::default());
    e.mouse_up();
    assert_eq!(show_all(&e), "foo b|ar\nbaz");
    e.mouse_down(1, 1, Mods::default());
    e.mouse_drag(6);
    e.mouse_up();
    assert_eq!(show_all(&e), "f[oo ba]r\nbaz");
    e.mouse_down(5, 2, Mods::default());
    e.mouse_up();
    assert_eq!(show_all(&e), "foo [bar]\nbaz");
    e.mouse_down(5, 3, Mods::default());
    e.mouse_up();
    assert_eq!(show_all(&e), "[foo bar\n]baz");
    e.mouse_down(1, 1, Mods::default());
    e.mouse_up();
    e.mouse_down(9, 1, Mods { ctrl: true, ..Mods::default() });
    e.mouse_up();
    assert_eq!(show_all(&e), "f|oo bar\nb|az");
    e.mouse_down(1, 1, Mods::default());
    e.mouse_up();
    e.mouse_down(6, 1, Mods { shift: true, ..Mods::default() });
    e.mouse_up();
    assert_eq!(show_all(&e), "f[oo ba]r\nbaz");
    // double-click drag extends by words
    e.mouse_down(1, 2, Mods::default());
    e.mouse_drag(9);
    e.mouse_up();
    assert_eq!(show_all(&e), "[foo bar\nbaz]");

    // vim: a click stays in normal mode, a drag selects in visual mode
    let mut e = ed("foo bar\nbaz");
    e.mouse_down(7, 1, Mods::default());
    e.mouse_up();
    assert_eq!((show(&e).as_str(), e.mode), ("foo ba|r\nbaz", Mode::Normal));
    // on the char clicked, not the one before it
    e.mouse_down(5, 1, Mods::default());
    e.mouse_up();
    assert_eq!((show(&e).as_str(), e.mode), ("foo b|ar\nbaz", Mode::Normal));
    e.mouse_down(0, 1, Mods::default());
    e.mouse_drag(3);
    e.mouse_up();
    assert_eq!(e.mode, Mode::Visual);
    keys(&mut e, "d");
    assert_eq!(show(&e), "| bar\nbaz");
}

#[test]
fn graphemes_in_sublime_mode() {
    let fam = "👨\u{200d}👩\u{200d}👧";
    let text = format!("a{fam}b");
    let mut e = sub(&format!("{text}|"));
    keys(&mut e, "<Left><Left>");
    assert_eq!(e.buf().sel.primary().head, 1);
    keys(&mut e, "<Right>");
    assert_eq!(e.buf().sel.primary().head, 6);
    keys(&mut e, "<BS>");
    assert_eq!(e.buf().doc.rope.to_string(), "ab");
    let mut e = sub("|e\u{301}x");
    keys(&mut e, "<Del>");
    assert_eq!(e.buf().doc.rope.to_string(), "x");
    let mut e = sub("|e\u{301}x");
    keys(&mut e, "<S-Right>");
    assert_eq!(show_all(&e), "[e\u{301}]x");
    // a click inside a cluster lands on its start
    let mut e = sub(&text);
    e.mouse_down(3, 1, Mods::default());
    e.mouse_up();
    assert_eq!(e.buf().sel.primary().head, 1);
}

#[test]
fn graphemes_in_vim_mode() {
    let fam = "👨\u{200d}👩\u{200d}👧";
    check(&format!("|{fam}x"), "x", "|x");
    check(&format!("|a{fam}x"), "lx", "a|x");
    check(&format!("|a{fam}x"), "$hx", "a|x");
    check(&format!("|a{fam}"), "$", &format!("a|{fam}"));
    check(&format!("|a{fam}"), "A<BS><Esc>", "|a");
    check(&format!("|{fam}{fam}"), "lD", &format!("|{fam}"));
    check(&format!("|{fam}ab"), "vly", &format!("|{fam}ab"));
    check(&format!("|{fam}ab"), "vlyP", &format!("{fam}|a{fam}ab"));
    check(&format!("|{fam}ab"), "rx", "|xab");
    check(&format!("|{fam}\n{fam}"), "jx", &format!("{fam}\n|"));
}

// ----- vim behaviour the differential test cannot cover -----

#[test]
fn neovim_and_shrew_choices() {
    // Y yanks to the end of the line (neovim's default)
    check("ab|c def", "Yp", "abcc de|f def");
    // `*` matches the word case-sensitively even though searches are smart-case
    check("|foo bar Foo foo", "*", "foo bar Foo |foo");
    check("|foo bar Foo", "/foo<CR>", "foo bar |Foo");
}

#[test]
fn read_only_registers() {
    check("|foo bar", "/bar<CR>0\"/P", "ba|rfoo bar");
    check("|x", ":s/x/y/<CR>\":p", "ys/x/y|/");
}

#[test]
fn vim_multi_cursor_operators() {
    // Ctrl+N then normal-mode operators on every selection
    // Esc leaves visual mode with a cursor at the end of each selection, a second Esc keeps one
    let mut e = ed("|foo x foo y");
    keys(&mut e, "<C-n><C-n><Esc>");
    assert_eq!((show(&e).as_str(), e.buf().sel.len()), ("foo x fo|o y", 2));
    keys(&mut e, "<Esc>");
    assert_eq!(e.buf().sel.len(), 1);
    let mut e = ed("|ab\nab\nab");
    keys(&mut e, "<C-A-Down><C-A-Down>");
    assert_eq!(e.buf().sel.len(), 3);
    keys(&mut e, "x");
    assert_eq!(e.buf().doc.rope.to_string(), "b\nb\nb");
    keys(&mut e, "u");
    assert_eq!(e.buf().doc.rope.to_string(), "ab\nab\nab");
    keys(&mut e, "Ax<Esc>");
    assert_eq!(e.buf().doc.rope.to_string(), "abx\nabx\nabx");
    keys(&mut e, "dd");
    assert_eq!(e.buf().doc.rope.to_string(), "");
}

#[test]
fn undo_groups_macros_and_ex() {
    // a macro run, :g and :normal are one undo step each
    check("|a b c d", "qqxq3@qu", "| b c d");
    check("|a\nb\na", ":g/a/s/a/x/<CR>u", "|a\nb\na");
    check("|a\nb", ":%norm Ax<CR>u", "|a\nb");
}

#[test]
fn insert_ctrl_o_and_ctrl_r() {
    check("|abc", "A<C-o>0X<Esc>", "|Xabc");
    check("|abc def", "yiwA <C-r>\"<Esc>", "abc def ab|c");
    check("|abc", "ix<C-o>$y<Esc>", "xabc|y");
}

#[test]
fn search_operators_and_counts() {
    check("|a b c d", "d/c<CR>", "|c d");
    check("|a b c d", "d/c<CR>u", "|a b c d");
    // `.` after `cgn` changes the next match, wrapping around like `n`
    check("|ab ab ab", "/ab<CR>cgnX<Esc>.", "ab X |X");
    check("|ab ab ab", "/ab<CR>cgnX<Esc>..", "|X X X");
    check("|a\nb\nc\nd", "3:d<CR>", "|d");
}

#[test]
fn marks_follow_edits() {
    check("a\n|b\nc", "maggOx<Esc>'ax", "x\na\n|\nc");
    check("a\nb|c\nd", "maggdd`ax", "|b\nd");
    check("|a\nb\nc", "jmaggyyP'ax", "a\na\n|\nc");
}

#[test]
fn about_panel_is_modal_and_read_only() {
    let mut e = ed("|abc");
    keys(&mut e, ":about<CR>");
    assert_eq!(e.about, Some(0));
    // keys scroll the panel instead of editing
    keys(&mut e, "jjxdd<PageDown>k");
    assert_eq!(e.about, Some(11));
    assert_eq!(show(&e), "|abc");
    e.insert_text("typed");
    assert_eq!(show(&e), "|abc");
    keys(&mut e, "gq");
    assert_eq!(e.about, None);
    // the palette finds it too
    keys(&mut e, "<C-S-p>about shrew<CR>");
    assert!(e.about.is_some() && e.picker.is_none());
    keys(&mut e, "<Esc>");
    assert_eq!(e.about, None);
}

#[test]
fn settings_open_from_a_chord_the_palette_and_ex() {
    use crate::Event;
    let mut e = ed("|abc");
    keys(&mut e, "<C-,>");
    assert_eq!(e.events.pop(), Some(Event::OpenSettings(String::new())));
    keys(&mut e, ":settings editor<CR>");
    assert_eq!(e.events.pop(), Some(Event::OpenSettings("editor".into())));
    keys(&mut e, "<C-S-p>preferences settings<CR>");
    assert_eq!(e.events.pop(), Some(Event::OpenSettings(String::new())));
    assert!(e.picker.is_none());
    // the chord list asks the real bindings: Ctrl+Shift+Q is Ctrl+Q again and is listed once
    let chords = e.chords();
    assert!(chords.contains(&(KeyPress::ctrl(','), crate::Command::Settings)));
    assert!(chords.contains(&(KeyPress::ctrl('s'), crate::Command::Save)));
    let quit = chords.iter().filter(|c| c.1 == crate::Command::Quit).count();
    assert_eq!(quit, 1);
}
