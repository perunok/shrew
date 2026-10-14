//! Importing a vimrc / init.vim: options shrew has an equivalent for, `let mapleader`, and the `map` family.
//! Everything else (plugins, autocmds, functions, conditionals) is counted and reported, never fatal.
//!
//! Like vim, `<leader>` is resolved when a mapping is defined, so `let mapleader` only affects later mappings.

use crate::{Config, Env, ImportReport, ImportSource, Imported, Os, Warning};
use shrew_core::features::Feature;
use shrew_core::keymap::{self, Keymap, MapCommand, MapMode};
use std::path::{Path, PathBuf};

/// The vimrc vim or neovim would read, first found: `~/.vimrc`, `~/.vim/vimrc`, `~/_vimrc` (Windows),
/// `$XDG_CONFIG_HOME/nvim/init.vim`, `%LOCALAPPDATA%/nvim/init.vim`.
pub fn candidates(env: &Env) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = &env.home {
        v.push(h.join(".vimrc"));
        v.push(h.join(".vim").join("vimrc"));
        if env.os == Os::Windows {
            v.push(h.join("_vimrc"));
            v.push(h.join("vimfiles").join("vimrc"));
        }
    }
    if let Some(c) = &env.config_dir {
        v.push(c.join("nvim").join("init.vim"));
    }
    if env.os != Os::Linux {
        // neovim uses ~/.config on macOS too, and %LOCALAPPDATA% on Windows
        if let Some(h) = &env.home {
            v.push(h.join(".config").join("nvim").join("init.vim"));
        }
        if let Some(l) = std::env::var_os("LOCALAPPDATA").filter(|_| env.os == Os::Windows) {
            v.push(PathBuf::from(l).join("nvim").join("init.vim"));
        }
    }
    v
}

/// Runs the import `src` asks for. `None` when off or nothing was found.
pub fn import(src: &ImportSource, env: &Env) -> Option<Imported> {
    let path = match src {
        ImportSource::Off => return None,
        ImportSource::Path(p) => p.clone(),
        ImportSource::Auto => match candidates(env).into_iter().find(|p| p.is_file()) {
            Some(p) => p,
            None => {
                // only an init.lua: say why nothing was imported
                let lua = env.config_dir.as_ref().map(|c| c.join("nvim").join("init.lua")).filter(|p| p.is_file())?;
                return Some(lua_only(&lua));
            }
        },
    };
    if path.extension().is_some_and(|e| e == "lua") {
        return Some(lua_only(&path));
    }
    let source = path.display().to_string();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let mut imp = parse(&text);
            imp.report.paths.push(path);
            Some(imp)
        }
        Err(e) => {
            let mut imp = Imported { report: ImportReport::new("vimrc"), ..Default::default() };
            imp.warnings.push(Warning::new(source, None, format!("cannot read: {e}")));
            Some(imp)
        }
    }
}

fn lua_only(path: &Path) -> Imported {
    let mut imp = Imported { report: ImportReport::new("vimrc"), ..Default::default() };
    imp.report.paths.push(path.to_path_buf());
    imp.warnings.push(Warning::new(
        path.display().to_string(),
        None,
        "init.lua is not supported (Lua config); write settings to shrew's config.toml or an init.vim",
    ));
    imp
}

#[derive(Default)]
struct Tabs {
    ts: Option<usize>,
    sw: Option<usize>,
    sts: Option<usize>,
}

struct State {
    config: Config,
    keymap: Keymap,
    report: ImportReport,
    warnings: Vec<Warning>,
    tabs: Tabs,
}

/// Parses vimrc text.
pub fn parse(text: &str) -> Imported {
    let mut st = State {
        config: Config::default(),
        keymap: Keymap::default(),
        report: ImportReport::new("vimrc"),
        warnings: Vec::new(),
        tabs: Tabs::default(),
    };
    let lines = logical_lines(text);
    let mut i = 0;
    while i < lines.len() {
        let (no, line) = (lines[i].0, lines[i].1.as_str());
        let cmd = line.trim_start().trim_start_matches([':', ' ', '\t']).trim_end_matches(['\r']);
        if cmd.trim().is_empty() || cmd.starts_with('"') || cmd.starts_with("#") {
            i += 1;
            continue;
        }
        if let Some(end) = block_end(&lines, i) {
            st.report.skip(Some(no), cmd.trim());
            i = end + 1;
            continue;
        }
        st.command(no, cmd);
        i += 1;
    }
    st.finish()
}

/// Joins `\` continuation lines; drops `"\ ` comment continuations. Each line keeps its first line number.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix('\\') {
            if let Some(last) = out.last_mut() {
                last.1.push_str(rest);
                continue;
            }
        }
        if t.starts_with("\"\\ ") {
            continue;
        }
        out.push((n + 1, line.to_string()));
    }
    out
}

fn first_word(cmd: &str) -> &str {
    let end = cmd.find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':' && c != '#').unwrap_or(cmd.len());
    &cmd[..end]
}

/// If line `i` opens a block shrew cannot evaluate (if, function, for, while, try, augroup, heredoc), the index of
/// the line that closes it.
fn block_end(lines: &[(usize, String)], i: usize) -> Option<usize> {
    let cmd = lines[i].1.trim().trim_start_matches(':');
    let word = first_word(cmd);
    // `lua << EOF`, `python3 << EOF`, `let x =<< trim END`
    if let Some(pos) = cmd.find("<<") {
        let marker =
            cmd[pos + 2..].split_whitespace().filter(|w| *w != "trim" && *w != "eval").next_back().unwrap_or("EOF");
        let heredoc = matches!(word, "lua" | "python" | "python3" | "py" | "py3" | "perl" | "ruby" | "let");
        if heredoc && !cmd[pos..].contains('>') {
            return Some(
                lines.iter().skip(i + 1).position(|l| l.1.trim() == marker).map_or(lines.len() - 1, |p| i + 1 + p),
            );
        }
    }
    if split_bar(cmd).len() > 1 && split_bar(cmd).last().is_some_and(|l| first_word(l.trim()).starts_with("end")) {
        return Some(i);
    }
    let ends: &[&str] = match word {
        "if" => &["endif", "endi", "en"],
        "fu" | "fun" | "func" | "function" => &["endfunction", "endf", "endfu", "endfun", "endfunc"],
        "for" => &["endfor", "endfo"],
        "wh" | "while" => &["endwhile", "endw", "endwh"],
        "try" => &["endtry", "endt"],
        "aug" | "augroup" => {
            let arg = cmd[word.len()..].trim();
            if arg.eq_ignore_ascii_case("end") || arg.is_empty() {
                return None;
            }
            let mut j = i + 1;
            while j < lines.len() {
                let c = lines[j].1.trim().trim_start_matches(':');
                if matches!(first_word(c), "aug" | "augroup")
                    && c[first_word(c).len()..].trim().eq_ignore_ascii_case("end")
                {
                    return Some(j);
                }
                j += 1;
            }
            return Some(lines.len() - 1);
        }
        _ => return None,
    };
    let opener = word;
    let opens = |w: &str| match opener {
        "if" => w == "if",
        "for" => w == "for",
        "wh" | "while" => w == "wh" || w == "while",
        "try" => w == "try",
        _ => matches!(w, "fu" | "fun" | "func" | "function"),
    };
    let mut depth = 0;
    for (j, (_, l)) in lines.iter().enumerate().skip(i) {
        let w = first_word(l.trim().trim_start_matches(':'));
        if opens(w) {
            depth += 1;
        } else if ends.contains(&w) {
            depth -= 1;
            if depth == 0 {
                return Some(j);
            }
        }
    }
    Some(lines.len() - 1)
}

/// Splits at `|` (outside quotes) into separate commands.
fn split_bar(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quote, mut prev) = (0, None, ' ');
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q && prev != '\\' => quote = None,
            (None, '|') if prev != '\\' => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        prev = c;
    }
    out.push(&s[start..]);
    out
}

/// A mapping command ends at an unescaped `|`; `\|` is a literal bar.
fn split_map_bar(s: &str) -> (String, Option<&str>) {
    let mut out = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' if chars.peek().is_some_and(|x| x.1 == '|') => {
                chars.next();
                out += "<Bar>";
            }
            '\u{16}' if chars.peek().is_some_and(|x| x.1 == '|') => {
                chars.next();
                out += "<Bar>";
            }
            '|' => return (out, Some(&s[i + 1..])),
            _ => out.push(c),
        }
    }
    (out, None)
}

/// Options that mean nothing to shrew but are fine to see (terminal, files, UI chrome shrew does its own way).
const HARMLESS: &[&str] = &[
    "compatible",
    "encoding",
    "fileencoding",
    "fileencodings",
    "fileformat",
    "fileformats",
    "ttyfast",
    "lazyredraw",
    "showcmd",
    "showmode",
    "ruler",
    "laststatus",
    "backspace",
    "hidden",
    "wildmenu",
    "wildmode",
    "history",
    "undolevels",
    "autoread",
    "ttimeout",
    "ttimeoutlen",
    "belloff",
    "visualbell",
    "errorbells",
    "termguicolors",
    "mouse",
    "ignorecase",
    "smartcase",
    "incsearch",
    "magic",
    "smarttab",
    "shiftround",
    "swapfile",
    "backup",
    "writebackup",
    "undofile",
    "updatetime",
    "signcolumn",
    "modeline",
    "modelines",
    "confirm",
    "title",
    "shortmess",
    "completeopt",
    "smartindent",
    "cindent",
    "t_Co",
    "timeout",
    "nrformats",
    "showmatch",
];

fn canonical(name: &str) -> &str {
    match name {
        "ts" => "tabstop",
        "sw" => "shiftwidth",
        "sts" => "softtabstop",
        "et" => "expandtab",
        "nu" => "number",
        "rnu" => "relativenumber",
        "so" => "scrolloff",
        "hls" => "hlsearch",
        "ic" => "ignorecase",
        "scs" => "smartcase",
        "is" => "incsearch",
        "cul" => "cursorline",
        "ai" => "autoindent",
        "si" => "smartindent",
        "cin" => "cindent",
        "sm" => "showmatch",
        "cb" => "clipboard",
        "tm" => "timeoutlen",
        "bg" => "background",
        "gfn" => "guifont",
        "cp" => "compatible",
        "enc" => "encoding",
        "fenc" => "fileencoding",
        "fencs" => "fileencodings",
        "ff" => "fileformat",
        "ffs" => "fileformats",
        "sc" => "showcmd",
        "smd" => "showmode",
        "ru" => "ruler",
        "ls" => "laststatus",
        "bs" => "backspace",
        "hid" => "hidden",
        "wmnu" => "wildmenu",
        "wim" => "wildmode",
        "hi" => "history",
        "ul" => "undolevels",
        "ar" => "autoread",
        "to" => "ttimeout",
        "ttm" => "ttimeoutlen",
        "bo" => "belloff",
        "vb" => "visualbell",
        "eb" => "errorbells",
        "tgc" => "termguicolors",
        "sta" => "smarttab",
        "sr" => "shiftround",
        "swf" => "swapfile",
        "bk" => "backup",
        "wb" => "writebackup",
        "udf" => "undofile",
        "ut" => "updatetime",
        "scl" => "signcolumn",
        "lz" => "lazyredraw",
        "tf" => "ttyfast",
        "ml" => "modeline",
        "mls" => "modelines",
        "cf" => "confirm",
        "shm" => "shortmess",
        "cot" => "completeopt",
        "nf" => "nrformats",
        n => n,
    }
}

impl State {
    fn skip(&mut self, no: usize, text: &str) {
        self.report.skip(Some(no), text.trim());
    }

    fn command(&mut self, no: usize, cmd: &str) {
        let word = first_word(cmd);
        let rest = cmd[word.len()..].trim_start();
        let silent = word == "silent" || word == "sil";
        if silent {
            // `silent! nnoremap ...`
            let rest = rest.trim_start_matches('!').trim_start();
            if !rest.is_empty() {
                return self.command(no, rest);
            }
        }
        if keymap::parse_map_command(word, &self.keymap).is_some() {
            let (head, tail) = split_map_bar(cmd);
            self.map(no, &head);
            if let Some(t) = tail.filter(|t| !t.trim().is_empty()) {
                self.command(no, t.trim_start());
            }
            return;
        }
        match word {
            "set" | "se" | "setlocal" | "setl" | "setglobal" | "setg" | "let" | "syntax" | "syn" | "filetype"
            | "filet" | "noh" | "nohlsearch" | "scriptencoding" | "scripte" => {}
            _ => return self.skip(no, cmd),
        }
        let parts = split_bar(cmd);
        if parts.len() > 1 {
            for p in parts {
                if !p.trim().is_empty() {
                    self.command(no, p.trim());
                }
            }
            return;
        }
        match word {
            "set" | "se" | "setlocal" | "setl" | "setglobal" | "setg" => self.set(no, strip_comment(rest)),
            "let" => self.let_(no, cmd, rest),
            "syntax" | "syn" => match strip_comment(rest).trim() {
                "on" | "enable" => self.feature(Feature::Syntax, true),
                "off" => self.feature(Feature::Syntax, false),
                _ => self.skip(no, cmd),
            },
            // filetype detection is automatic; :nohlsearch and :scriptencoding are no-ops at startup
            _ => {}
        }
    }

    fn map(&mut self, no: usize, cmd: &str) {
        match keymap::parse_map_command(cmd, &self.keymap) {
            Some(Ok(MapCommand::Ignored)) | None => self.skip(no, cmd),
            Some(Ok(c)) => {
                self.keymap.apply(c);
                self.report.applied += 1;
            }
            Some(Err(_)) => self.skip(no, cmd),
        }
    }

    fn feature(&mut self, f: Feature, on: bool) {
        self.config.set_feature(f, on);
        self.report.applied += 1;
    }

    fn let_(&mut self, no: usize, cmd: &str, rest: &str) {
        let Some((var, expr)) = rest.split_once('=') else { return self.skip(no, cmd) };
        let var = var.trim();
        let leader = match var {
            "mapleader" | "g:mapleader" => true,
            "maplocalleader" | "g:maplocalleader" => false,
            _ => return self.skip(no, cmd),
        };
        let Some(s) = vim_string(expr.trim()) else { return self.skip(no, cmd) };
        let Some(k) = keymap::leader_key(&s) else { return self.skip(no, cmd) };
        if leader {
            self.keymap.leader = k;
            self.config.editor.leader = Some(s);
        } else {
            self.keymap.local_leader = k;
        }
        self.report.applied += 1;
    }

    fn set(&mut self, no: usize, args: &str) {
        for tok in set_tokens(args) {
            if !self.option(&tok) {
                self.skip(no, &format!("set {tok}"));
            }
        }
    }

    /// One `:set` argument. False if shrew has no equivalent.
    fn option(&mut self, tok: &str) -> bool {
        // name=value, name:value, name+=value, ...
        if let Some(i) = tok.find(['=', ':']) {
            let (name, value) = (&tok[..i], &tok[i + 1..]);
            if name.ends_with(['+', '-', '^']) {
                return HARMLESS.contains(&canonical(name.trim_end_matches(['+', '-', '^'])));
            }
            return self.value_option(canonical(name), value);
        }
        if tok.ends_with('?') || tok.ends_with('&') {
            return true; // query or reset to default
        }
        let (name, on) = match tok.strip_prefix("no") {
            Some(n) if is_bool_option(canonical(n)) => (canonical(n), false),
            _ => (canonical(tok), true),
        };
        let f = match name {
            "number" => Feature::LineNumbers,
            "relativenumber" => Feature::RelativeNumbers,
            "hlsearch" => Feature::SearchHighlight,
            "list" => Feature::Whitespace,
            "cursorline" => Feature::CurrentLine,
            "showmatch" if on => Feature::BracketMatch,
            "autoindent" => Feature::AutoIndent,
            "expandtab" => {
                self.config.editor.expand_tab = Some(on);
                self.report.applied += 1;
                return true;
            }
            n => return HARMLESS.contains(&n),
        };
        self.feature(f, on);
        true
    }

    fn value_option(&mut self, name: &str, value: &str) -> bool {
        let num = value.parse::<usize>().ok();
        let e = &mut self.config.editor;
        let tabs = &mut self.tabs;
        let mut clipboard = false;
        let ok = match (name, num) {
            ("tabstop", Some(n @ 1..=16)) => {
                tabs.ts = Some(n);
                true
            }
            ("shiftwidth", Some(n @ 0..=16)) => {
                tabs.sw = Some(n);
                true
            }
            ("softtabstop", Some(n)) => {
                // negative (-1, "use shiftwidth") does not parse and is treated like 0
                tabs.sts = (n <= 16).then_some(n);
                true
            }
            ("scrolloff", Some(n)) => {
                e.scrolloff = Some(n.min(999));
                true
            }
            ("timeoutlen", Some(n)) => {
                e.mapping_timeout_ms = Some(n.min(10_000) as u64);
                true
            }
            ("background", _) if value == "light" || value == "dark" => {
                e.theme = Some(format!("shrew-{value}"));
                true
            }
            ("clipboard", _) => {
                clipboard = value.contains("unnamed");
                true
            }
            ("guifont", _) => match guifont(value) {
                Some((family, size)) => {
                    e.font_family = Some(family);
                    if size.is_some() {
                        e.font_size = size;
                    }
                    true
                }
                None => false,
            },
            ("softtabstop", None) => true,
            (n, _) => return HARMLESS.contains(&n),
        };
        if clipboard {
            self.config.set_feature(Feature::Clipboard, true);
        }
        if ok {
            self.report.applied += 1;
        }
        ok
    }

    fn finish(mut self) -> Imported {
        // shrew has one width for indents and tabs: the indent width when indenting with spaces, else the tab stop
        let Tabs { ts, sw, sts } = self.tabs;
        let indent = sw.filter(|&n| n > 0).or(sts.filter(|&n| n > 0)).or(ts);
        let width = if self.config.editor.expand_tab == Some(false) { ts.or(indent) } else { indent };
        if width.is_some() {
            self.config.editor.tab_width = width;
        }
        for mode in MapMode::ALL {
            for m in self.keymap.mappings(mode) {
                self.config.bindings.push((mode, m.clone()));
            }
        }
        Imported { config: self.config, report: self.report, warnings: self.warnings }
    }
}

fn is_bool_option(name: &str) -> bool {
    matches!(
        name,
        "number" | "relativenumber" | "hlsearch" | "list" | "cursorline" | "showmatch" | "autoindent" | "expandtab"
    ) || HARMLESS.contains(&name)
}

/// Drops a trailing `" comment` from a `:set` / `:syntax` argument.
fn strip_comment(s: &str) -> &str {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'"' && (i == 0 || b[i - 1] == b' ' || b[i - 1] == b'\t') {
            return &s[..i];
        }
    }
    s
}

/// `:set` arguments split at unescaped whitespace, with `\ ` unescaped.
fn set_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => cur.extend(chars.next()),
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A vim string literal at the start of `expr`: `","`, `'\'`, `"\<Space>"` (key notation stays `<Space>`).
fn vim_string(expr: &str) -> Option<String> {
    let mut chars = expr.chars();
    match chars.next()? {
        '\'' => {
            let mut out = String::new();
            loop {
                match chars.next()? {
                    '\'' if chars.clone().next() == Some('\'') => {
                        chars.next();
                        out.push('\'');
                    }
                    '\'' => return Some(out),
                    c => out.push(c),
                }
            }
        }
        '"' => {
            let mut out = String::new();
            loop {
                match chars.next()? {
                    '"' => return Some(out),
                    '\\' => match chars.next()? {
                        't' => out.push('\t'),
                        'e' => out += "<Esc>",
                        '<' => out.push('<'),
                        c => out.push(c),
                    },
                    c => out.push(c),
                }
            }
        }
        _ => None,
    }
}

/// `Fira Code:h12`, `Fira_Code:h12:cANSI` (Windows), `DejaVu Sans Mono 12` (GTK); the first of a comma list.
fn guifont(v: &str) -> Option<(String, Option<f32>)> {
    let first = v.split(',').next()?.trim();
    if first.is_empty() || first == "*" {
        return None;
    }
    if first.contains(':') {
        let mut parts = first.split(':');
        let family = parts.next()?.replace('_', " ");
        let size = parts.find_map(|p| p.strip_prefix('h')).and_then(|s| s.parse().ok());
        return Some((family, size));
    }
    match first.rsplit_once(' ') {
        Some((family, size)) if size.parse::<f32>().is_ok() => Some((family.to_string(), size.parse().ok())),
        _ => Some((first.to_string(), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shrew_core::KeyPress;
    use shrew_core::keymap::Rhs;

    fn get<'a>(c: &'a Config, mode: MapMode, lhs: &str) -> Option<&'a Rhs> {
        let lhs = KeyPress::parse_seq(lhs);
        c.bindings.iter().rev().find(|(m, b)| *m == mode && b.lhs == lhs).map(|(_, b)| &b.rhs)
    }

    #[test]
    fn realistic_vimrc() {
        let imp = parse(include_str!("../tests/fixtures/vimrc"));
        let c = &imp.config;
        assert_eq!(c.editor.tab_width, Some(2));
        assert_eq!(c.editor.expand_tab, Some(true));
        assert_eq!(c.editor.scrolloff, Some(8));
        assert_eq!(c.editor.leader.as_deref(), Some("<Space>"));
        assert_eq!(c.editor.mapping_timeout_ms, Some(500));
        assert_eq!(c.editor.theme.as_deref(), Some("shrew-dark"));
        assert_eq!(c.editor.font_family.as_deref(), Some("JetBrains Mono"));
        assert_eq!(c.editor.font_size, Some(13.0));
        assert_eq!(c.feature(Feature::LineNumbers), Some(true));
        assert_eq!(c.feature(Feature::RelativeNumbers), Some(true));
        assert_eq!(c.feature(Feature::SearchHighlight), Some(false));
        assert_eq!(c.feature(Feature::Whitespace), Some(true));
        assert_eq!(c.feature(Feature::Syntax), Some(true));
        assert_eq!(c.feature(Feature::Clipboard), Some(true));
        assert_eq!(c.feature(Feature::CurrentLine), Some(true));

        // the leader was "\" until `let mapleader`, then space
        let keys = |s: &str| Rhs::Keys { keys: KeyPress::parse_seq(s), remap: false };
        assert_eq!(get(c, MapMode::Normal, "\\q"), Some(&keys(":q<CR>")));
        assert_eq!(get(c, MapMode::Normal, " w"), Some(&keys(":w<CR>")));
        assert_eq!(get(c, MapMode::Insert, "jk"), Some(&keys("<Esc>")));
        assert_eq!(get(c, MapMode::Normal, "Y"), Some(&Rhs::Keys { keys: KeyPress::parse_seq("y$"), remap: true }));
        assert_eq!(get(c, MapMode::Visual, "<"), Some(&keys("<gv")));
        assert_eq!(get(c, MapMode::Normal, "<C-h>"), Some(&keys("<C-w>h")));
        assert_eq!(get(c, MapMode::Normal, " s"), Some(&Rhs::Ex("w".into())));
        // `map` covers normal, visual and operator-pending
        for m in [MapMode::Normal, MapMode::Visual, MapMode::OperatorPending] {
            assert_eq!(get(c, m, "H"), Some(&Rhs::Keys { keys: KeyPress::parse_seq("^"), remap: true }), "{m:?}");
        }
        // continuation line + bar-separated second mapping
        assert_eq!(get(c, MapMode::Normal, " n"), Some(&keys(":nohlsearch<CR>")));
        assert_eq!(get(c, MapMode::Normal, " b"), Some(&keys(":ls<CR>")));
        // `\|` is a literal bar inside the mapping
        assert_eq!(get(c, MapMode::Normal, " p"), Some(&keys("a|<Esc>")));
        // unmapped again later
        assert_eq!(get(c, MapMode::Normal, "Q"), None);
        // <expr>, <Plug> and :call mappings are skipped
        assert_eq!(get(c, MapMode::Normal, "j"), None);
        assert_eq!(get(c, MapMode::Normal, " c"), None);
        assert!(c.bindings.iter().all(|(_, b)| b.lhs != KeyPress::parse_seq("<F2>")));

        let r = &imp.report;
        let skipped: Vec<&str> = r.samples.iter().map(|s| s.1.as_str()).collect();
        assert_eq!(skipped[0], "call plug#begin('~/.vim/plugged')", "{skipped:?}");
        assert!(r.unsupported >= 10, "{r:?}");
        assert!(r.applied >= 25, "{r:?}");
        assert!(r.samples.iter().all(|s| s.0.is_some()));
    }

    #[test]
    fn neovim_init_vim() {
        let imp = parse(include_str!("../tests/fixtures/init.vim"));
        let c = &imp.config;
        // ts=8 sw=4 et: indents of 4
        assert_eq!(c.editor.tab_width, Some(4));
        assert_eq!(c.editor.leader.as_deref(), Some(","));
        assert_eq!(c.feature(Feature::LineNumbers), Some(false));
        assert_eq!(c.feature(Feature::AutoIndent), Some(false));
        assert_eq!(c.editor.theme.as_deref(), Some("shrew-light"));
        assert!(get(c, MapMode::Normal, ",f").is_some());
        // the whole lua heredoc is one skipped block, and nothing inside it was read as vim
        assert!(imp.report.samples.iter().any(|s| s.1.starts_with("lua << EOF")));
        assert!(get(c, MapMode::Normal, "X").is_none());
    }

    #[test]
    fn tab_settings() {
        let t = |s: &str| parse(s).config.editor.tab_width;
        assert_eq!(t("set ts=4"), Some(4));
        assert_eq!(t("set tabstop=8 shiftwidth=2"), Some(2));
        assert_eq!(t("set ts=8 sts=3"), Some(3));
        assert_eq!(t("set ts=8 sw=4 noexpandtab"), Some(8));
        assert_eq!(t("set sw=0 ts=6"), Some(6));
        assert_eq!(t("set ts=99"), None);
    }

    #[test]
    fn pieces() {
        assert_eq!(vim_string(r#""\<Space>""#).as_deref(), Some("<Space>"));
        assert_eq!(vim_string(r#""\\" " comment"#).as_deref(), Some("\\"));
        assert_eq!(vim_string("'\\'").as_deref(), Some("\\"));
        assert_eq!(vim_string("','").as_deref(), Some(","));
        assert_eq!(set_tokens(r"gfn=Fira\ Code:h12 nu"), vec!["gfn=Fira Code:h12".to_string(), "nu".into()]);
        assert_eq!(guifont("Fira_Code:h11:cANSI"), Some(("Fira Code".into(), Some(11.0))));
        assert_eq!(guifont("DejaVu Sans Mono 12,Monospace 11"), Some(("DejaVu Sans Mono".into(), Some(12.0))));
        assert_eq!(strip_comment("ts=4 \" tabs"), "ts=4 ");
        assert_eq!(split_bar(r#"set nu | let x = "a|b""#), vec!["set nu ", r#" let x = "a|b""#]);
    }

    #[test]
    fn lua_and_missing_files() {
        let dir = std::env::temp_dir().join(format!("shrew-vimrc-{}", std::process::id()));
        let nvim = dir.join("cfg").join("nvim");
        std::fs::create_dir_all(&nvim).unwrap();
        std::fs::write(nvim.join("init.lua"), "vim.o.number = true\n").unwrap();
        let env = Env { home: Some(dir.join("home")), config_dir: Some(dir.join("cfg")), appdata: None, os: Os::Linux };
        let imp = import(&ImportSource::Auto, &env).unwrap();
        assert!(imp.warnings[0].message.contains("init.lua"));
        assert!(imp.config.bindings.is_empty());
        std::fs::write(nvim.join("init.vim"), "set ts=3\n").unwrap();
        let imp = import(&ImportSource::Auto, &env).unwrap();
        assert_eq!(imp.config.editor.tab_width, Some(3));
        assert!(import(&ImportSource::Off, &env).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
