//! Which grammar a buffer uses: shrew-core's `lang` table first (it knows `.h` is C, not Objective-C), then
//! syntect's own extension and file-name lists (which also cover user grammars), then the first line.

use shrew_core::lang;
use std::path::{Path, PathBuf};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// What the worker needs to pick a grammar for a buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LangHint {
    pub path: Option<PathBuf>,
    /// The buffer's first line (shebangs, `<?xml`, mode lines).
    pub first_line: String,
    /// Explicit choice (`:set syntax=python`): a shrew lang name, a grammar name or an extension.
    pub name: Option<String>,
}

impl LangHint {
    pub fn new(path: Option<&Path>, first_line: &str) -> Self {
        let first_line: String = first_line.chars().take(256).collect();
        LangHint { path: path.map(Path::to_path_buf), first_line, name: None }
    }

    pub fn named(name: &str) -> Self {
        LangHint { name: Some(name.to_string()), ..LangHint::default() }
    }
}

/// shrew lang name -> grammar name in syntect's default set. `false` marks a stand-in from a related language,
/// used only when no grammar (user ones included) claims the file's extension.
const MAP: &[(&str, &str, bool)] = &[
    ("rust", "Rust", true),
    ("c", "C", true),
    ("cpp", "C++", true),
    ("go", "Go", true),
    ("python", "Python", true),
    ("javascript", "JavaScript", true),
    ("java", "Java", true),
    ("csharp", "C#", true),
    ("php", "PHP", true),
    ("ruby", "Ruby", true),
    ("lua", "Lua", true),
    ("bash", "Bourne Again Shell (bash)", true),
    ("fish", "Bourne Again Shell (bash)", false),
    ("json", "JSON", true),
    ("yaml", "YAML", true),
    ("markdown", "Markdown", true),
    ("html", "HTML", true),
    ("xml", "XML", true),
    ("css", "CSS", true),
    ("sql", "SQL", true),
    ("make", "Makefile", true),
    ("haskell", "Haskell", true),
    ("ocaml", "OCaml", true),
    ("erlang", "Erlang", true),
    ("clojure", "Clojure", true),
    ("scala", "Scala", true),
    ("r", "R", true),
    ("diff", "Diff", true),
    // stand-ins: close relatives highlight comments, strings, numbers and most keywords correctly
    ("typescript", "JavaScript", false),
    ("tsx", "JavaScript", false),
    ("svelte", "HTML", false),
    ("vue", "HTML", false),
    ("kotlin", "Scala", false),
    ("dart", "Java", false),
    ("glsl", "C", false),
    ("wgsl", "Rust", false),
    ("proto", "C++", false),
    ("ini", "Java Properties", false),
];

fn mapped(lang: &str, exact: bool) -> Option<&'static str> {
    MAP.iter().find(|(l, _, e)| *l == lang && *e == exact).map(|(_, s, _)| *s)
}

fn index_of(ss: &SyntaxSet, s: &SyntaxReference) -> usize {
    ss.syntaxes().iter().position(|x| std::ptr::eq(x, s)).expect("syntax belongs to the set")
}

fn by_name<'a>(ss: &'a SyntaxSet, name: &str) -> Option<&'a SyntaxReference> {
    ss.find_syntax_by_name(name).or_else(|| ss.syntaxes().iter().rev().find(|s| s.name.eq_ignore_ascii_case(name)))
}

/// The grammar for a hint as an index into `ss.syntaxes()`, or `None` for plain text.
pub fn resolve(ss: &SyntaxSet, hint: &LangHint) -> Option<usize> {
    let found = find(ss, hint)?;
    (found.name != "Plain Text").then(|| index_of(ss, found))
}

fn find<'a>(ss: &'a SyntaxSet, hint: &LangHint) -> Option<&'a SyntaxReference> {
    if let Some(name) = hint.name.as_deref() {
        let lower = name.to_ascii_lowercase();
        if matches!(lower.as_str(), "text" | "plain" | "plain text" | "none") {
            return None;
        }
        return mapped(&lower, true)
            .or_else(|| mapped(&lower, false))
            .and_then(|n| by_name(ss, n))
            .or_else(|| by_name(ss, name))
            .or_else(|| ss.find_syntax_by_token(name));
    }
    let path = hint.path.as_deref();
    let file_name = path.and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("");
    let ext = path.and_then(|p| p.extension()).and_then(|e| e.to_str()).unwrap_or("");
    let shrew_lang = path.and_then(lang::detect).or_else(|| lang::detect_shebang(&hint.first_line));
    if shrew_lang.is_some_and(|l| l.name == "text") {
        return None;
    }
    // exact file names (`Makefile`, `Gemfile`), then shrew's mapping, then syntect's extensions
    let named = (!file_name.is_empty()).then(|| ss.find_syntax_by_extension(file_name)).flatten();
    named
        .or_else(|| shrew_lang.and_then(|l| mapped(l.name, true)).and_then(|n| by_name(ss, n)))
        .or_else(|| (!ext.is_empty()).then(|| ss.find_syntax_by_extension(ext)).flatten())
        .or_else(|| shrew_lang.and_then(|l| mapped(l.name, false)).and_then(|n| by_name(ss, n)))
        .or_else(|| ss.find_syntax_by_first_line(&hint.first_line))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(ss: &SyntaxSet, path: &str, first: &str) -> String {
        let hint = LangHint::new(Some(Path::new(path)).filter(|p| !p.as_os_str().is_empty()), first);
        resolve(ss, &hint).map(|i| ss.syntaxes()[i].name.clone()).unwrap_or_else(|| "-".into())
    }

    #[test]
    fn detection() {
        let ss = SyntaxSet::load_defaults_newlines();
        assert_eq!(name(&ss, "src/main.rs", ""), "Rust");
        assert_eq!(name(&ss, "a/b.h", ""), "C");
        assert_eq!(name(&ss, "x.hpp", ""), "C++");
        assert_eq!(name(&ss, "Makefile", ""), "Makefile");
        assert_eq!(name(&ss, "Gemfile", ""), "Ruby");
        assert_eq!(name(&ss, "app.ts", ""), "JavaScript");
        assert_eq!(name(&ss, "conf.jsonc", ""), "JSON");
        assert_eq!(name(&ss, ".bashrc", ""), "Bourne Again Shell (bash)");
        assert_eq!(name(&ss, "script", "#!/usr/bin/env python3"), "Python");
        assert_eq!(name(&ss, "", "#!/bin/bash"), "Bourne Again Shell (bash)");
        assert_eq!(name(&ss, "x.pl", ""), "Perl"); // unknown to shrew, known to syntect
        assert_eq!(name(&ss, "notes.txt", ""), "-");
        assert_eq!(name(&ss, "unknown.zzz", ""), "-");
        let named = |n: &str| resolve(&ss, &LangHint::named(n)).map(|i| ss.syntaxes()[i].name.clone());
        assert_eq!(named("python").as_deref(), Some("Python"));
        assert_eq!(named("Rust").as_deref(), Some("Rust"));
        assert_eq!(named("rs").as_deref(), Some("Rust"));
        assert_eq!(named("plain"), None);
    }

    #[test]
    fn map_targets_exist() {
        let ss = SyntaxSet::load_defaults_newlines();
        for (lang, syn, _) in MAP {
            assert!(lang::by_name(lang).is_some(), "shrew has no lang {lang}");
            assert!(ss.find_syntax_by_name(syn).is_some(), "syntect has no grammar {syn}");
        }
    }
}
