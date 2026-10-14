use crate::*;
use shrew_core::{Buffer, Change, ChangeSet, Document};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn service() -> Syntaxes {
    let s = Syntaxes::new(Theme::shrew(), Arc::new(|| {}));
    assert!(s.wait_ready(Duration::from_secs(10)));
    s
}

fn settle(hl: &mut Highlighter) {
    let end = Instant::now() + Duration::from_secs(60);
    loop {
        hl.poll();
        if hl.is_settled() {
            return;
        }
        assert!(Instant::now() < end, "highlighter never settled");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn doc(path: &str, text: &str) -> Document {
    let mut d = Document::from_str(text);
    d.path = Some(path.into());
    d
}

/// Color of the text at the first occurrence of `needle` in `line` (`None` = default foreground).
fn color_at(hl: &Highlighter, rope: &ropey::Rope, line: usize, needle: &str) -> Option<Rgb> {
    let text = rope.line(line).to_string();
    let at = text.find(needle).unwrap_or_else(|| panic!("{needle:?} not in {text:?}")) as u32;
    let spans = hl.spans_for_line(line).expect("line highlighted");
    spans.iter().find(|s| s.start <= at && at < s.end).map(|s| s.fg)
}

fn style(theme: &Theme, scopes: &str) -> Option<Rgb> {
    let (c, _) = theme.style_for(scopes);
    (c != theme.foreground).then_some(c)
}

#[test]
fn rust_python_json() {
    let s = service();
    let theme = s.theme();
    let kw = style(&theme, "keyword.control");
    let string = style(&theme, "string.quoted.double");
    let comment = style(&theme, "comment.line");
    let num = style(&theme, "constant.numeric");

    let d = doc("main.rs", "// hi\nfn main() {\n    let s = \"x\";\n    if s.len() > 2 { return; }\n}\n");
    let mut hl = s.highlighter(&d);
    settle(&mut hl);
    assert_eq!(hl.syntax_name(), Some("Rust"));
    assert_eq!(color_at(&hl, &d.rope, 0, "hi"), comment);
    assert_eq!(color_at(&hl, &d.rope, 1, "main"), style(&theme, "entity.name.function"));
    assert_eq!(color_at(&hl, &d.rope, 2, "\"x\""), string);
    assert_eq!(color_at(&hl, &d.rope, 3, "if"), kw);
    assert_eq!(color_at(&hl, &d.rope, 3, "2"), num);
    let italic = hl.spans_for_line(0).unwrap()[0].italic;
    assert!(italic, "comments are italic");

    let d = doc("x.py", "def f(a):\n    # note\n    return 'q' + str(42)\n");
    let mut hl = s.highlighter(&d);
    settle(&mut hl);
    assert_eq!(hl.syntax_name(), Some("Python"));
    assert_eq!(color_at(&hl, &d.rope, 0, "def"), style(&theme, "storage.type.function"));
    assert_eq!(color_at(&hl, &d.rope, 1, "note"), comment);
    assert_eq!(color_at(&hl, &d.rope, 2, "return"), kw);
    assert_eq!(color_at(&hl, &d.rope, 2, "'q'"), string);
    assert_eq!(color_at(&hl, &d.rope, 2, "42"), num);

    let d = doc("a.json", "{\n  \"key\": [1, true, null, \"v\"]\n}\n");
    let mut hl = s.highlighter(&d);
    settle(&mut hl);
    assert_eq!(hl.syntax_name(), Some("JSON"));
    let key = color_at(&hl, &d.rope, 1, "key");
    assert!(key.is_some() && key != string, "keys stand out from values: {key:?}");
    assert_eq!(color_at(&hl, &d.rope, 1, "1"), num);
    assert_eq!(color_at(&hl, &d.rope, 1, "true"), style(&theme, "constant.language"));
    assert_eq!(color_at(&hl, &d.rope, 1, "\"v\""), string);

    // spans are sorted, disjoint, inside the line, never covering the newline
    for line in 0..d.rope.len_lines() {
        let len = d.rope.line(line).to_string().trim_end_matches('\n').len() as u32;
        let spans = hl.spans_for_line(line).unwrap();
        let mut at = 0;
        for sp in spans {
            assert!(sp.start >= at && sp.start < sp.end && sp.end <= len, "{line}: {spans:?}");
            at = sp.end;
        }
    }
}

#[test]
fn plain_text_and_wake() {
    let woken = Arc::new(AtomicUsize::new(0));
    let w = woken.clone();
    let s = Syntaxes::new(
        Theme::shrew(),
        Arc::new(move || {
            w.fetch_add(1, Ordering::Relaxed);
        }),
    );
    assert!(s.wait_ready(Duration::from_secs(10)));
    let d = doc("notes.txt", "just words\n");
    let mut hl = s.highlighter(&d);
    settle(&mut hl);
    assert_eq!(hl.syntax_name(), None);
    assert_eq!(hl.spans_for_line(0), None);
    let before = woken.load(Ordering::Relaxed);
    let d = doc("a.rs", "fn x() {}\n");
    let mut hl = s.highlighter(&d);
    settle(&mut hl);
    assert!(woken.load(Ordering::Relaxed) > before, "wake fires when visible lines are ready");
    // switching grammar on the fly
    hl.set_language("python");
    settle(&mut hl);
    assert_eq!(hl.syntax_name(), Some("Python"));
    hl.set_language("text");
    settle(&mut hl);
    assert_eq!(hl.syntax_name(), None);
    assert_eq!(hl.spans_for_line(0), None);
}

fn rust_source(lines: usize) -> String {
    let chunk = r#"/// Documentation for item NUM.
#[derive(Debug, Clone)]
pub struct ItemNUM<'a> {
    name: &'a str,
    values: Vec<u64>, // trailing comment
}

impl<'a> ItemNUM<'a> {
    pub fn new(name: &'a str) -> Self {
        let values = (0..NUM).map(|x| x * 2 + 0x1f).collect();
        println!("made {} with {:?}", name, "esc\n");
        Self { name, values }
    }
}

"#;
    let per = chunk.lines().count();
    (0..lines.div_ceil(per)).map(|i| chunk.replace("NUM", &i.to_string())).collect()
}

/// Every line of `hl` must match a highlighter that saw the final text from scratch.
fn assert_matches_fresh(s: &Syntaxes, hl: &Highlighter, d: &Document) {
    let mut fresh = s.highlighter(d);
    fresh.set_viewport(0, 0);
    settle(&mut fresh);
    for line in 0..d.rope.len_lines() {
        assert_eq!(
            hl.spans_for_line(line),
            fresh.spans_for_line(line),
            "line {line}: {:?}",
            d.rope.line(line).to_string()
        );
    }
}

#[test]
fn incremental_edits() {
    let s = service();
    let theme = s.theme();
    let comment = style(&theme, "comment.line");
    let mut buf = Buffer::new(1, doc("lib.rs", &rust_source(2_000)));
    let mut hl = s.highlighter(&buf.doc);
    hl.set_viewport(0, 50);
    settle(&mut hl);
    let n = buf.doc.rope.len_lines();
    let snapshot: Vec<Option<Vec<Span>>> = (0..n).map(|l| hl.spans_for_line(l).map(|s| s.to_vec())).collect();

    // open a block comment in the middle: everything after it turns into comment
    let mid = 1_000;
    let at = buf.doc.rope.line_to_char(mid);
    buf.apply(ChangeSet::single(Change::insert(at, "/*\n")), None);
    hl.sync(&mut buf.doc);
    // right after the edit (before the worker answers): earlier lines keep their spans, later ones moved down
    for l in (0..mid).step_by(37) {
        assert_eq!(hl.spans_for_line(l).map(|s| s.to_vec()), snapshot[l], "line {l} before the edit");
    }
    assert_eq!(hl.spans_for_line(mid + 1 + 9).map(|s| s.to_vec()), snapshot[mid + 9], "stale spans shifted");
    settle(&mut hl);
    for l in (0..mid).step_by(37) {
        assert_eq!(hl.spans_for_line(l).map(|s| s.to_vec()), snapshot[l], "line {l} before the edit");
    }
    let rope = buf.doc.rope.clone();
    for l in [mid + 3, mid + 500, n - 2] {
        if rope.line(l).len_chars() > 1 {
            let text = rope.line(l).to_string();
            let word = text.split_whitespace().next().unwrap();
            assert_eq!(color_at(&hl, &rope, l, word), comment, "line {l} is inside the comment");
        }
    }
    assert_matches_fresh(&s, &hl, &buf.doc);

    // close it again: later lines go back to what they were
    let at = buf.doc.rope.line_to_char(mid);
    buf.apply(ChangeSet::single(Change::delete(at, at + 3)), None);
    hl.sync(&mut buf.doc);
    settle(&mut hl);
    for (l, snap) in snapshot.iter().enumerate() {
        assert_eq!(hl.spans_for_line(l).map(|s| s.to_vec()), *snap, "line {l} after undoing the comment");
    }
}

#[test]
fn random_edits_match_fresh() {
    const ROUNDS: usize = 120;
    let s = service();
    let mut buf = Buffer::new(1, doc("lib.rs", &rust_source(600)));
    let mut hl = s.highlighter(&buf.doc);
    hl.set_viewport(0, 40);
    let mut seed: u64 = std::env::var("SHREW_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x9e3779b97f4a7c15);
    let mut rnd = |m: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % m as u64) as usize
    };
    let snippets = ["\"", "/*", "*/", "\n", "fn x() {\n", "}", "// c\n", "'a ", "r#\"", "\"#", "é", "\n\n\n"];
    for round in 0..ROUNDS {
        // several edits per sync, sometimes multi-change sets
        for _ in 0..1 + rnd(3) {
            let len = buf.doc.len();
            let a = rnd(len + 1);
            let cs = match rnd(4) {
                0 => ChangeSet::single(Change::delete(a, (a + rnd(40)).min(len))),
                1 => {
                    let b = rnd(len + 1);
                    ChangeSet::new(vec![
                        Change::insert(a.min(b), snippets[rnd(snippets.len())]),
                        Change::insert(a.max(b), "x"),
                    ])
                }
                _ => ChangeSet::single(Change::insert(a, snippets[rnd(snippets.len())])),
            };
            buf.apply(cs, None);
        }
        hl.sync(&mut buf.doc);
        if round % 3 == 0 {
            hl.set_viewport(rnd(buf.doc.rope.len_lines()), 40);
        }
        if round % 4 == 0 {
            std::thread::sleep(Duration::from_millis(rnd(3) as u64));
            hl.poll();
        }
        if round % 10 == 9 {
            settle(&mut hl);
            assert_matches_fresh(&s, &hl, &buf.doc);
        }
    }
    settle(&mut hl);
    assert_matches_fresh(&s, &hl, &buf.doc);
}

#[test]
fn theme_switch_rehighlights() {
    let s = service();
    let d = doc("a.rs", "fn main() { let x = 1; }\n");
    let mut hl = s.highlighter(&d);
    settle(&mut hl);
    let dark = color_at(&hl, &d.rope, 0, "let");
    s.set_theme(Theme::shrew_light());
    let end = Instant::now() + Duration::from_secs(10);
    while color_at(&hl, &d.rope, 0, "let") == dark {
        assert!(Instant::now() < end);
        hl.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    let light = Theme::shrew_light();
    assert_eq!(color_at(&hl, &d.rope, 0, "let"), style(&light, "storage.type"));
}

#[test]
fn long_lines_are_skipped() {
    let s = service();
    let long = "x".repeat(MAX_LINE_CHARS + 1);
    let d = doc("a.rs", &format!("fn a() {{}}\nlet s = \"{long}\";\nfn b() {{}}\n"));
    let mut hl = s.highlighter(&d);
    settle(&mut hl);
    assert_eq!(hl.spans_for_line(1), Some(&[][..]));
    assert!(!hl.spans_for_line(2).unwrap().is_empty());
}

#[test]
fn windowed_big_file() {
    let s = service();
    let text = rust_source(WINDOWED_LINES + 20_000);
    let d = doc("big.rs", &text);
    let n = d.rope.len_lines();
    let mut hl = s.highlighter(&d);
    let top = n - 1_000;
    hl.set_viewport(top, 50);
    let t = Instant::now();
    settle(&mut hl);
    let took = t.elapsed();
    assert!(hl.spans_for_line(top + 10).is_some());
    assert!(hl.spans_for_line(10).is_none(), "far-away lines are not highlighted");
    assert!(took < Duration::from_secs(2), "viewport near the end of a huge file took {took:?}");
    // scrolling far evicts what is no longer near
    hl.set_viewport(0, 50);
    settle(&mut hl);
    assert!(hl.spans_for_line(10).is_some());
    assert!(hl.spans_for_line(top + 10).is_none());
}

/// `cargo test -p shrew-syntax --release -- --ignored --nocapture perf`
#[test]
#[ignore]
fn perf() {
    let t = Instant::now();
    let s = service();
    println!("grammar load: {:?}", t.elapsed());
    let text = rust_source(20_000);
    let d = doc("big.rs", &text);
    let n = d.rope.len_lines();

    let t = Instant::now();
    let mut hl = s.highlighter(&d);
    hl.set_viewport(0, 60);
    while (0..60).any(|l| hl.spans_for_line(l).is_none()) {
        hl.poll();
        std::thread::yield_now();
    }
    let first = t.elapsed();
    settle(&mut hl);
    let all = t.elapsed();
    println!("{n} lines: first screen {first:?}, whole file {all:?} = {:.0} lines/s", n as f64 / all.as_secs_f64());
    let spans: usize = (0..n).map(|l| hl.spans_for_line(l).map_or(0, |s| s.len())).sum();
    println!(
        "span cache: {spans} spans, ~{} KiB",
        (spans * std::mem::size_of::<Span>() + n * std::mem::size_of::<Option<Box<[Span]>>>()) / 1024
    );

    // warm grammar now: a fresh buffer measures pure throughput
    let t = Instant::now();
    let mut hl2 = s.highlighter(&d);
    hl2.set_viewport(0, 0);
    settle(&mut hl2);
    let warm = t.elapsed();
    println!("warm: {:?} = {:.0} lines/s", warm, n as f64 / warm.as_secs_f64());

    // jump to the end of the file
    let t = Instant::now();
    let mut hl3 = s.highlighter(&d);
    hl3.set_viewport(n - 60, 60);
    while (n - 60..n).any(|l| hl3.spans_for_line(l).is_none()) {
        hl3.poll();
        std::thread::yield_now();
    }
    println!("first screen at the end of the file: {:?}", t.elapsed());

    // keystroke in the middle: time until the worker has re-highlighted
    let mut buf = Buffer::new(9, d);
    let mut hl4 = s.highlighter(&buf.doc);
    hl4.set_viewport(10_000, 60);
    settle(&mut hl4);
    let mut worst = Duration::ZERO;
    let mut ui = Duration::ZERO;
    for k in 0..50 {
        let at = buf.doc.rope.line_to_char(10_010 + k) + 4;
        buf.apply(ChangeSet::single(Change::insert(at, "a")), None);
        let t = Instant::now();
        hl4.sync(&mut buf.doc);
        ui = ui.max(t.elapsed());
        settle(&mut hl4);
        worst = worst.max(t.elapsed());
    }
    println!("keystroke: UI-thread sync <= {ui:?}, re-highlighted (until settled) <= {worst:?}");
}

/// Prints the scope stack of each token (theme tuning aid).
#[test]
#[ignore]
fn dump_scopes() {
    use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
    let ss = SyntaxSet::load_defaults_newlines();
    let samples = [
        (
            "rs",
            "use std::io;\n#[derive(Debug)]\npub struct A<'a> { x: &'a str }\nfn main() { let mut v: Vec<u8> = vec![1]; self.x(); Foo::bar(); }\nimpl Trait for A {}\nconst C: i32 = 0;\nmacro_rules! m {}\n",
        ),
        ("json", "{\"key\": [1, true, \"v\"]}\n"),
        ("py", "import os\n@dec\nclass A(B):\n    def f(self, a=1): return None\n"),
        ("js", "const x = function(a) { return this.y + `t${a}`; };\nclass Q extends R {}\n"),
        ("md", "# Title\n\n*it* **b** [l](http://x) `code`\n- item\n"),
        ("toml", "[a]\nb = 1\n"),
    ];
    for (ext, text) in samples {
        let syn = ss.find_syntax_by_extension(ext).unwrap_or(ss.find_syntax_plain_text());
        println!("== {}", syn.name);
        let mut ps = ParseState::new(syn);
        let mut stack = ScopeStack::new();
        for line in text.split_inclusive('\n') {
            let ops = ps.parse_line(line, &ss).unwrap();
            let mut pos = 0;
            for (i, op) in ops {
                if i > pos {
                    println!(
                        "{:?} -> {}",
                        &line[pos..i],
                        stack.as_slice().iter().map(|s| s.build_string()).collect::<Vec<_>>().join(" ")
                    );
                }
                pos = i;
                stack.apply(&op).unwrap();
            }
        }
    }
}

#[test]
#[ignore]
fn raw_syntect() {
    use syntect::highlighting::{HighlightState, Highlighter as H, RangedHighlightIterator};
    use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
    let ss = SyntaxSet::load_defaults_newlines();
    let text = rust_source(20_000);
    let syn = ss.find_syntax_by_extension("rs").unwrap();
    let theme = Theme::shrew();
    let h = H::new(theme.syntect());
    for round in 0..2 {
        let t = Instant::now();
        let mut ps = ParseState::new(syn);
        let mut parse = Duration::ZERO;
        let mut hs = HighlightState::new(&h, ScopeStack::new());
        let mut n = 0;
        for line in text.split_inclusive('\n') {
            let t0 = Instant::now();
            let ops = ps.parse_line(line, &ss).unwrap();
            parse += t0.elapsed();
            n += RangedHighlightIterator::new(&mut hs, &ops, line, &h).count();
        }
        println!("round {round}: {:?} (parse {parse:?}), {n} tokens", t.elapsed());
    }
}

#[cfg(feature = "sublime-user")]
#[test]
fn user_syntax_dir() {
    let dir = std::env::temp_dir().join(format!("shrew-syntax-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Shrewlang.sublime-syntax"),
        "%YAML 1.2\n---\nname: Shrewlang\nfile_extensions: [shrewl]\nscope: source.shrewl\ncontexts:\n  main:\n    - match: '\\bnibble\\b'\n      scope: keyword.control.shrewl\n",
    )
    .unwrap();
    let opts = Options { sublime_user: false, extra_syntax_dirs: vec![dir.clone()] };
    let t = Instant::now();
    let s = Syntaxes::with_options(Theme::shrew(), Arc::new(|| {}), opts);
    // opened before the user grammars land: picked up again when they do
    let d = doc("x.shrewl", "a nibble b\n");
    let mut hl = s.highlighter(&d);
    assert!(s.wait_ready(Duration::from_secs(20)));
    println!("grammar load with a user dir: {:?}", t.elapsed());
    assert!(s.syntax_names().iter().any(|n| n == "Shrewlang"));
    let end = Instant::now() + Duration::from_secs(10);
    while hl.syntax_name() != Some("Shrewlang") {
        assert!(Instant::now() < end, "user grammar never applied");
        hl.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    settle(&mut hl);
    assert_eq!(color_at(&hl, &d.rope, 0, "nibble"), style(&s.theme(), "keyword.control"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn generations_say_which_lines_changed() {
    let s = service();
    let mut buf = Buffer::new(1, doc("lib.rs", &rust_source(300)));
    let mut hl = s.highlighter(&buf.doc);
    let g0 = hl.generation();
    assert_eq!(hl.changed_since(g0), Some(0..0), "nothing new yet");
    hl.set_viewport(0, 50);
    settle(&mut hl);
    let g1 = hl.generation();
    assert!(g1 > g0, "results arrived");
    if g1 - g0 < 32 {
        assert_eq!(hl.changed_since(g0).expect("recent enough").start, 0);
    }
    assert_eq!(hl.changed_since(g1), Some(0..0));
    // an edit moves lines: anything may have changed
    let at = buf.doc.rope.line_to_char(10);
    buf.apply(ChangeSet::single(Change::insert(at, "x")), None);
    hl.sync(&mut buf.doc);
    assert_eq!(hl.changed_since(g1).map(|r| r.end), Some(usize::MAX));
    // too long ago to tell
    assert!(hl.generation() <= 32 || hl.changed_since(0).is_none());
}
