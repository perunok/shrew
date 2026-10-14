//! Latency checks on a 50 MB, 1M-line buffer: every interactive operation must stay far below a 16 ms frame.
//! Ignored by default (the buffer takes a moment to build in debug); run them optimised:
//!
//! `cargo test --release -p shrew-core --test perf -- --ignored --nocapture`

use shrew_core::document::Document;
use shrew_core::{Command, Editor, Feature, KeyPress, Range, Selection};
use std::time::{Duration, Instant};

const LINES: usize = 1_000_000;

/// ~50 bytes a line: identifiers, punctuation, a common word (`value`) on every line.
fn big_text() -> String {
    let mut s = String::with_capacity(LINES * 52);
    for i in 0..LINES {
        use std::fmt::Write;
        let _ = writeln!(s, "    let value_{i:07} = compute(value, {});  // ok", i % 97);
    }
    s
}

fn editor_with(text: &str) -> Editor {
    let mut e = Editor::new();
    e.install_document(Document::from_str(text));
    e.buf_mut().view.rows = 50;
    e.buf_mut().view.cols = 160;
    e
}

fn keys(e: &mut Editor, s: &str) {
    for k in KeyPress::parse_seq(s) {
        e.handle_key(k);
    }
}

/// Runs `f` `n` times; returns the worst run.
fn worst(n: usize, mut f: impl FnMut()) -> Duration {
    (0..n)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .max()
        .unwrap()
}

struct Report(Vec<(String, Duration, Duration)>);

impl Report {
    fn check(&mut self, name: &str, budget_ms: u64, d: Duration) {
        println!("{name:<48} {:>10.3} ms  (budget {budget_ms} ms)", d.as_secs_f64() * 1e3);
        self.0.push((name.to_string(), d, Duration::from_millis(budget_ms)));
    }
    fn finish(self) {
        let over: Vec<String> =
            self.0.iter().filter(|(_, d, b)| d > b).map(|(n, d, _)| format!("{n}: {d:?}")).collect();
        assert!(over.is_empty(), "over budget: {over:#?}");
    }
}

#[test]
#[ignore]
fn big_file_latency() {
    let text = big_text();
    println!("buffer: {} MB, {} lines", text.len() / 1_000_000, LINES);
    let mut r = Report(Vec::new());

    // loading (off the UI thread in the app, but it bounds time-to-first-frame)
    let dir = std::env::temp_dir().join(format!("shrew-perf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.rs");
    std::fs::write(&path, &text).unwrap();
    let t = Instant::now();
    let doc = Document::load(&path).unwrap();
    r.check("load 50 MB file", 400, t.elapsed());
    let t = Instant::now();
    let mut e = Editor::new();
    e.install_document(doc);
    e.buf_mut().view.rows = 50;
    r.check("install document", 5, t.elapsed());
    let _ = std::fs::remove_dir_all(&dir);

    // motions
    r.check("G (last line)", 2, worst(5, || keys(&mut e, "G")));
    r.check("gg", 2, worst(5, || keys(&mut e, "gg")));
    keys(&mut e, "500000G");
    r.check("j / k in the middle", 1, worst(50, || keys(&mut e, "jk")));
    r.check("w / b", 1, worst(50, || keys(&mut e, "wb")));
    r.check("$ / 0", 1, worst(50, || keys(&mut e, "$0")));
    r.check("%", 1, worst(20, || keys(&mut e, "f(%")));
    r.check("<C-d> / <C-u>", 1, worst(20, || keys(&mut e, "<C-d><C-u>")));
    r.check("scroll(+3) mouse wheel", 1, worst(50, || e.scroll(3, 0)));

    // editing in the middle
    r.check("insert a char (1 cursor)", 2, worst(50, || keys(&mut e, "ix<Esc>")));
    r.check("x", 2, worst(50, || keys(&mut e, "x")));
    r.check("dd in the middle", 2, worst(20, || keys(&mut e, "dd")));
    r.check("u (undo dd)", 2, worst(20, || keys(&mut e, "u")));
    r.check("<C-r> (redo)", 2, worst(20, || keys(&mut e, "<C-r>")));
    r.check("yyp", 2, worst(20, || keys(&mut e, "yyp")));
    r.check("o + typing + Esc", 3, worst(20, || keys(&mut e, "ohello world<Esc>")));
    r.check(". (repeat)", 3, worst(20, || keys(&mut e, ".")));

    // at the end of the file
    keys(&mut e, "G");
    r.check("dd at the end", 2, worst(10, || keys(&mut e, "dd")));
    r.check("undo at the end", 2, worst(10, || keys(&mut e, "u")));

    // search: the incremental preview looks a few MB around the cursor, Enter and n scan the whole file
    keys(&mut e, "gg/");
    let mut typing = Duration::ZERO;
    for k in KeyPress::parse_seq("value_0999999") {
        let t = Instant::now();
        e.handle_key(k);
        typing = typing.max(t.elapsed());
    }
    r.check("typing a search pattern (worst key, incremental)", 16, typing);
    let t = Instant::now();
    keys(&mut e, "<CR>");
    r.check("<CR>: match on the last line (scans 50 MB)", 150, t.elapsed());
    r.check("n (wraps around the file)", 150, worst(3, || keys(&mut e, "n")));
    keys(&mut e, "gg/compute<CR>");
    r.check("n (next line)", 1, worst(50, || keys(&mut e, "n")));
    r.check("* on a word", 2, worst(10, || keys(&mut e, "*")));

    // many cursors
    let mut e2 = editor_with(&text);
    let sel = Selection::new((0..1000).map(|i| Range::point(i * 997 * 50)), 0);
    e2.buf_mut().sel = sel;
    keys(&mut e2, "i");
    r.check("insert a char with 1000 cursors", 8, worst(20, || keys(&mut e2, "x")));
    r.check("backspace with 1000 cursors", 8, worst(20, || keys(&mut e2, "<BS>")));
    r.check("arrow down with 1000 cursors", 8, worst(20, || keys(&mut e2, "<Down>")));
    keys(&mut e2, "<Esc>");

    // Sublime-style all occurrences of a word on every line (1M selections)
    let mut e3 = editor_with(&text);
    e3.set_feature(Feature::Vim, false);
    e3.buf_mut().sel = Selection::point(text.find("compute").unwrap() + 1);
    let t = Instant::now();
    e3.run(Command::SelectAllOccurrences);
    r.check("select all occurrences of a word (1M matches)", 900, t.elapsed());
    println!("  -> {} selections", e3.buf().sel.len());
    let t = Instant::now();
    keys(&mut e3, "z");
    r.check("type over 1M selections", 2500, t.elapsed());
    drop(e3);

    // Ctrl+D a few times on a common word
    let mut e4 = editor_with(&text);
    e4.set_feature(Feature::Vim, false);
    e4.buf_mut().sel = Selection::point(text.find("compute").unwrap() + 1);
    r.check("Ctrl+D (next occurrence)", 2, worst(20, || keys(&mut e4, "<C-d>")));
    drop(e4);

    // :%s over the whole file
    let mut e5 = editor_with(&text);
    let t = Instant::now();
    keys(&mut e5, ":%s/compute/calc/g<CR>");
    r.check(":%s/compute/calc/g (1M substitutions)", 1500, t.elapsed());
    let t = Instant::now();
    keys(&mut e5, "u");
    r.check("undo :%s", 400, t.elapsed());
    drop(e5);

    // rendering helpers, called every frame
    let mut e6 = editor_with(&text);
    keys(&mut e6, "500000Gvjjj");
    r.check("selection_spans (visual)", 1, worst(50, || drop(e6.selection_spans())));
    r.check("scroll_to_cursor", 1, worst(50, || e6.buf_mut().scroll_to_cursor(4, 4)));
    r.check("cursor_shape + pending_keys", 1, worst(50, || drop((e6.cursor_shape(), e6.pending_keys()))));

    // one 50 MB line (minified file): columns must not walk the line per keystroke
    let long: String = "abcdefghij".repeat(5_000_000);
    let mut e7 = editor_with(&long);
    // the first `$` measures the line once; after that the column is cached
    r.check("$ 0 $ on a 50 MB line (first visit)", 25, worst(1, || keys(&mut e7, "$0$")));
    r.check("$ 0 $ on a 50 MB line (again)", 2, worst(5, || keys(&mut e7, "$0$")));
    keys(&mut e7, "$");
    r.check("type at the end of a 50 MB line", 10, worst(10, || keys(&mut e7, "ax<Esc>")));
    r.check("scroll_to_cursor at col 50M", 10, worst(10, || e7.buf_mut().scroll_to_cursor(4, 4)));
    r.check("h / l at col 50M", 10, worst(10, || keys(&mut e7, "hl")));
    r.finish();
}

#[test]
#[ignore]
fn scan_breakdown() {
    let text = big_text();
    let rope = shrew_core::ropey::Rope::from_str(&text);
    for pat in ["value_0999999", "value_0999999\\C", "0999999", "zzz_no_match"] {
        let re = shrew_core::search::compile_vim(pat).unwrap();
        let t = Instant::now();
        let hit = shrew_core::search::find(&rope, &re, 0, true);
        println!("find {pat:<16} {:>8.3} ms {hit:?}", t.elapsed().as_secs_f64() * 1e3);
    }
    let t = Instant::now();
    let mut n = 0;
    for c in rope.chunks() {
        n += c.len();
    }
    println!("walk chunks {:>8.3} ms ({n})", t.elapsed().as_secs_f64() * 1e3);
    let re = regex::Regex::new("(?mi)value_0999999").unwrap();
    let t = Instant::now();
    println!("is_match whole string (?mi) {:?} {:>8.3} ms", re.is_match(&text), t.elapsed().as_secs_f64() * 1e3);
    let re = regex::Regex::new("(?m)value_0999999").unwrap();
    let t = Instant::now();
    println!("is_match whole string (?m) {:?} {:>8.3} ms", re.is_match(&text), t.elapsed().as_secs_f64() * 1e3);
}
