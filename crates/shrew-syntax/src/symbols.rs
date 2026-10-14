//! Symbols for go to symbol, the way Sublime Text's Symbol List finds them: the text the grammar scopes as
//! `entity.name.function`, `.class`, `.struct`, `.type`, `.section` (Markdown headings) and friends. Parsing runs
//! on the caller's (background) thread with the shared default grammars.

use crate::detect::{LangHint, resolve};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

/// Lines longer than this are skipped (minified code has no symbols worth listing).
const MAX_LINE_BYTES: usize = 4_000;

/// A symbol: its name, what it is, and where (0-based line, char column).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: &'static str,
    pub line: usize,
    pub col: usize,
}

/// Scope prefixes that make a symbol, and the kind each one is.
const KINDS: &[(&str, &str)] = &[
    ("entity.name.function", "function"),
    ("entity.name.method", "method"),
    ("entity.name.class", "class"),
    ("entity.name.struct", "struct"),
    ("entity.name.enum", "enum"),
    ("entity.name.union", "union"),
    ("entity.name.trait", "trait"),
    ("entity.name.interface", "interface"),
    ("entity.name.impl", "impl"),
    ("entity.name.type", "type"),
    ("entity.name.namespace", "namespace"),
    ("entity.name.module", "module"),
    ("entity.name.macro", "macro"),
    ("entity.name.section", "heading"),
];

fn kinds() -> &'static [(Scope, &'static str)] {
    static K: OnceLock<Vec<(Scope, &'static str)>> = OnceLock::new();
    K.get_or_init(|| KINDS.iter().filter_map(|(s, k)| Scope::new(s).ok().map(|s| (s, *k))).collect())
}

/// syntect's built-in grammars, loaded once per process and shared by the highlighter and the symbol finder.
pub fn default_set() -> Arc<SyntaxSet> {
    static SET: OnceLock<Arc<SyntaxSet>> = OnceLock::new();
    SET.get_or_init(|| Arc::new(SyntaxSet::load_defaults_newlines())).clone()
}

/// Whether a file's name alone picks a grammar (the project index skips the others without reading them).
pub fn has_grammar(ss: &SyntaxSet, path: &Path) -> bool {
    resolve(ss, &LangHint::new(Some(path), "")).is_some()
}

/// The symbols of `text` (a file at `path`, which picks the grammar with the first line), in order. Stops early
/// when `stop()` turns true (checked every few hundred lines).
pub fn extract(ss: &SyntaxSet, path: Option<&Path>, text: &str, stop: &dyn Fn() -> bool) -> Vec<Symbol> {
    let first = text.lines().next().unwrap_or("");
    let Some(idx) = resolve(ss, &LangHint::new(path, first)) else { return Vec::new() };
    let mut state = ParseState::new(&ss.syntaxes()[idx]);
    let mut stack = ScopeStack::new();
    let mut out = Vec::new();
    for (n, line) in text.split_inclusive('\n').enumerate() {
        if n % 256 == 255 && stop() {
            break;
        }
        if line.len() > MAX_LINE_BYTES {
            continue;
        }
        let Ok(ops) = state.parse_line(line, ss) else { break };
        // the run of text under a symbol scope: (start, end, kind)
        let mut cur: Option<(usize, usize, &'static str)> = None;
        let mut emit = |cur: &mut Option<(usize, usize, &'static str)>| {
            if let Some((a, z, kind)) = cur.take() {
                let name = line[a..z].trim();
                if !name.is_empty() {
                    let col = line[..a].chars().count() + (line[a..z].len() - line[a..z].trim_start().len());
                    out.push(Symbol { name: name.to_string(), kind, line: n, col });
                }
            }
        };
        let mut pos = 0;
        let mut segment = |a: usize, z: usize, stack: &ScopeStack, cur: &mut Option<(usize, usize, &'static str)>| {
            if a >= z {
                return;
            }
            match kind_of(stack) {
                Some(k) => match cur {
                    Some((_, end, ck)) if *end == a && *ck == k => *end = z,
                    _ => {
                        emit(cur);
                        *cur = Some((a, z, k));
                    }
                },
                None => emit(cur),
            }
        };
        for (at, op) in &ops {
            segment(pos, *at, &stack, &mut cur);
            if stack.apply(op).is_err() {
                break;
            }
            pos = *at;
        }
        segment(pos, line.trim_end_matches(['\n', '\r']).len().max(pos), &stack, &mut cur);
        emit(&mut cur);
    }
    out
}

fn kind_of(stack: &ScopeStack) -> Option<&'static str> {
    let kinds = kinds();
    stack.as_slice().iter().rev().find_map(|s| kinds.iter().find(|(k, _)| k.is_prefix_of(*s)).map(|(_, n)| *n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(path: &str, text: &str) -> Vec<(String, &'static str, usize)> {
        let ss = default_set();
        extract(&ss, Some(Path::new(path)), text, &|| false).into_iter().map(|s| (s.name, s.kind, s.line)).collect()
    }

    #[test]
    fn rust_symbols() {
        let src = "struct Point { x: i32 }\n\nimpl Point {\n    fn new() -> Self { todo!() }\n}\n\nenum Dir { Up }\ntrait Shape {}\nfn main() {\n    let v = Point::new();\n}\n";
        let got = names("a.rs", src);
        let find = |n: &str| got.iter().find(|g| g.0 == n).cloned();
        assert_eq!(find("Point").map(|g| (g.1, g.2)), Some(("struct", 0)), "{got:?}");
        assert_eq!(find("new").map(|g| (g.1, g.2)), Some(("function", 3)), "{got:?}");
        assert_eq!(find("main").map(|g| (g.1, g.2)), Some(("function", 8)), "{got:?}");
        assert!(find("Dir").is_some() && find("Shape").is_some(), "{got:?}");
        assert!(find("v").is_none(), "variables are not symbols: {got:?}");
    }

    #[test]
    fn python_markdown_and_plain_text() {
        let py = names("x.py", "class Foo:\n    def bar(self):\n        pass\n\ndef baz():\n    pass\n");
        let py: Vec<&str> = py.iter().map(|g| g.0.as_str()).collect();
        assert_eq!(py, ["Foo", "bar", "baz"]);
        let md = names("README.md", "# Title\n\ntext\n\n## Usage\n");
        assert!(md.iter().any(|g| g.0.contains("Title")), "{md:?}");
        assert!(md.iter().any(|g| g.0.contains("Usage")), "{md:?}");
        assert!(names("notes.txt", "fn main() {}\n").is_empty());
    }

    #[test]
    fn columns_point_at_the_name() {
        let ss = default_set();
        let s = extract(&ss, Some(Path::new("a.rs")), "    pub fn café_x() {}\n", &|| false);
        assert_eq!((s[0].name.as_str(), s[0].col), ("café_x", 11));
        assert!(has_grammar(&ss, Path::new("x.go")));
        assert!(!has_grammar(&ss, Path::new("x.unknownext")));
    }
}
