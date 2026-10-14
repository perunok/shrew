//! The project on disk: shrew's working directory. Files there can be listed, searched and read even when they are
//! not open; nothing outside it is touched (paths are checked after resolving symlinks). Walks respect .gitignore
//! and skip hidden files, like the file picker. Runs on HTTP worker threads only.

use regex::Regex;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Files bigger than this are skipped by search and refused by reads (bytes).
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// `path` (relative to `cwd`, or absolute) made absolute and normalised, without touching the disk.
pub fn absolute(cwd: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    let p = if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| p.to_path_buf())
    } else if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// Resolves symlinks of the longest existing ancestor (the rest may not exist yet).
fn canonical(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    let mut p = path;
    loop {
        if let Ok(c) = std::fs::canonicalize(p) {
            return Some(rest.iter().rev().fold(c, |acc: PathBuf, part| acc.join(part)));
        }
        rest.push(p.file_name()?.to_os_string());
        p = p.parent()?;
    }
}

/// Checks that `abs` lies inside the project `cwd` (symlinks resolved).
pub fn guard(cwd: &Path, abs: &Path) -> Result<(), String> {
    let root = std::fs::canonicalize(cwd).map_err(|e| format!("shrew's working directory is not readable: {e}"))?;
    match canonical(abs) {
        Some(c) if c.starts_with(&root) => Ok(()),
        _ => Err(format!(
            "{} is outside shrew's working directory ({}). Only files inside it, or buffers already open in shrew, are reachable.",
            abs.display(),
            cwd.display()
        )),
    }
}

/// `path` relative to `cwd` with `/` separators when inside it, else as given.
pub fn display(cwd: &Path, path: &Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().replace('\\', "/"),
        _ => path.to_string_lossy().into_owned(),
    }
}

/// Reads a project file for listing or editing: inside `cwd`, a regular file, not too big, not binary. Invalid UTF-8
/// is replaced, CRLF becomes LF (like shrew's own loader).
pub fn read_text(cwd: &Path, abs: &Path) -> Result<String, String> {
    guard(cwd, abs)?;
    let shown = display(cwd, abs);
    let meta = std::fs::metadata(abs).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{shown} does not exist."),
        _ => format!("{shown}: {e}"),
    })?;
    if !meta.is_file() {
        return Err(format!("{shown} is not a file."));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("{shown} is larger than 32 MB."));
    }
    let bytes = std::fs::read(abs).map_err(|e| format!("{shown}: {e}"))?;
    if is_binary(&bytes) {
        return Err(format!("{shown} looks like a binary file."));
    }
    let text = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    Ok(if text.contains("\r\n") { text.replace("\r\n", "\n") } else { text })
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8192)].contains(&0)
}

/// A file-name filter: a glob when it has `*`, `?` or `[`, else a case-insensitive substring of the relative path.
/// Globs without `/` match the file name at any depth; with `/` the whole relative path. `**` crosses folders.
pub struct Filter {
    pattern: String,
    glob: bool,
}

impl Filter {
    pub fn new(pattern: &str) -> Filter {
        let glob = pattern.contains(['*', '?', '[']);
        Filter {
            pattern: if glob { pattern.trim_start_matches("./").to_string() } else { pattern.to_lowercase() },
            glob,
        }
    }

    pub fn matches(&self, rel: &str) -> bool {
        if !self.glob {
            return rel.to_lowercase().contains(&self.pattern);
        }
        let subject = if self.pattern.contains('/') { rel } else { rel.rsplit('/').next().unwrap_or(rel) };
        glob_match(self.pattern.as_bytes(), subject.as_bytes())
    }
}

/// `*` (not `/`), `**` (anything), `?` (one char, not `/`), `[abc]` / `[a-z]` / `[!x]`.
fn glob_match(p: &[u8], s: &[u8]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(b'*') if p.get(1) == Some(&b'*') => {
            let mut rest = &p[2..];
            if rest.first() == Some(&b'/') {
                // `**/x` also matches `x` at the top
                if glob_match(&rest[1..], s) {
                    return true;
                }
                rest = &p[2..];
            }
            (0..=s.len()).any(|i| glob_match(rest, &s[i..]))
        }
        Some(b'*') => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != b'/').any(|i| glob_match(&p[1..], &s[i..])),
        Some(b'?') => s.first().is_some_and(|&c| c != b'/') && glob_match(&p[1..], &s[1..]),
        Some(b'[') => {
            let Some(close) = p.iter().skip(1).position(|&c| c == b']').map(|i| i + 1) else {
                return s.first() == Some(&b'[') && glob_match(&p[1..], &s[1..]);
            };
            let Some(&c) = s.first() else { return false };
            let set = &p[1..close];
            let (neg, set) = match set.first() {
                Some(b'!' | b'^') => (true, &set[1..]),
                _ => (false, set),
            };
            let mut hit = false;
            let mut i = 0;
            while i < set.len() {
                if i + 2 < set.len() && set[i + 1] == b'-' {
                    hit |= (set[i]..=set[i + 2]).contains(&c);
                    i += 3;
                } else {
                    hit |= set[i] == c;
                    i += 1;
                }
            }
            hit != neg && glob_match(&p[close + 1..], &s[1..])
        }
        Some(&c) => s.first() == Some(&c) && glob_match(&p[1..], &s[1..]),
    }
}

fn walker(cwd: &Path) -> ignore::WalkBuilder {
    let mut w = ignore::WalkBuilder::new(cwd);
    // .gitignore applies even outside a git repository (a fresh project folder)
    w.require_git(false).follow_links(false);
    w
}

/// Project files (relative paths, sorted per folder), up to `limit`. Returns the files and whether there were more.
pub fn list_files(cwd: &Path, filter: Option<&Filter>, limit: usize) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut w = walker(cwd);
    w.sort_by_file_name(|a, b| a.cmp(b));
    for entry in w.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = display(cwd, entry.path());
        if filter.is_some_and(|f| !f.matches(&rel)) {
            continue;
        }
        if out.len() == limit {
            return (out, true);
        }
        out.push(rel);
    }
    (out, false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Display path (relative to the project when inside it).
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// 1-based, in chars.
    pub col: usize,
    /// The line, cut around the match when long.
    pub text: String,
}

/// Longest preview of a matching line (chars).
const PREVIEW_CHARS: usize = 240;

/// Every match of `re` in `text` (one hit per match start; `re` should be built with multi-line on so `^`/`$` work
/// per line). Stops after `limit` hits. Returns whether it stopped early.
pub fn search_text(path: &str, text: &str, re: &Regex, limit: usize, out: &mut Vec<Hit>) -> bool {
    let mut line = 1;
    let mut line_start = 0;
    let mut scanned = 0;
    let mut last_line = 0;
    for m in re.find_iter(text) {
        if m.start() == m.end() && m.start() == text.len() && !text.is_empty() {
            continue;
        }
        line += text.as_bytes()[scanned..m.start()].iter().filter(|&&b| b == b'\n').count();
        scanned = m.start();
        if let Some(i) = text[..m.start()].rfind('\n') {
            line_start = line_start.max(i + 1);
        }
        // one hit per line is enough to point at it
        if line == last_line {
            continue;
        }
        last_line = line;
        if out.len() >= limit {
            return true;
        }
        let line_end = text[m.start()..].find('\n').map_or(text.len(), |i| m.start() + i);
        let line_text = &text[line_start..line_end];
        let col = text[line_start..m.start()].chars().count();
        out.push(Hit {
            path: path.to_string(),
            line,
            col: col + 1,
            text: crate::text::preview(line_text, col, PREVIEW_CHARS),
        });
    }
    false
}

/// Searches the project's files in parallel; `skip` holds files whose text is searched elsewhere (open buffers).
/// Hits are sorted by path and line. Returns the hits and whether the limit cut them short.
pub fn search_files(
    cwd: &Path,
    re: &Regex,
    filter: Option<&Filter>,
    skip: &[PathBuf],
    limit: usize,
) -> (Vec<Hit>, bool) {
    let hits = Mutex::new(Vec::new());
    let found = AtomicUsize::new(0);
    let truncated = std::sync::atomic::AtomicBool::new(false);
    walker(cwd).max_filesize(Some(MAX_FILE_BYTES)).build_parallel().run(|| {
        Box::new(|entry| {
            use ignore::WalkState;
            if found.load(Ordering::Relaxed) >= limit {
                truncated.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) || skip.iter().any(|s| s == entry.path()) {
                return WalkState::Continue;
            }
            let rel = display(cwd, entry.path());
            if filter.is_some_and(|f| !f.matches(&rel)) {
                return WalkState::Continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else { return WalkState::Continue };
            if is_binary(&bytes) {
                return WalkState::Continue;
            }
            let Ok(text) = std::str::from_utf8(&bytes) else { return WalkState::Continue };
            let mut mine = Vec::new();
            let left = limit.saturating_sub(found.load(Ordering::Relaxed));
            if search_text(&rel, text, re, left, &mut mine) {
                truncated.store(true, Ordering::Relaxed);
            }
            if !mine.is_empty() {
                found.fetch_add(mine.len(), Ordering::Relaxed);
                hits.lock().unwrap().extend(mine);
            }
            WalkState::Continue
        })
    });
    let mut hits = hits.into_inner().unwrap();
    hits.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    let truncated = truncated.into_inner() || hits.len() > limit;
    hits.truncate(limit);
    (hits, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        let f = |p: &str, s: &str| Filter::new(p).matches(s);
        assert!(f("*.rs", "src/main.rs"));
        assert!(!f("*.rs", "src/main.rsx"));
        assert!(f("src/*.rs", "src/main.rs"));
        assert!(!f("src/*.rs", "src/a/main.rs"));
        assert!(f("src/**/*.rs", "src/a/b/main.rs"));
        assert!(f("src/**/*.rs", "src/main.rs"));
        assert!(f("**/*.toml", "Cargo.toml"));
        assert!(f("ma?n.[rt]s", "main.rs"));
        assert!(f("[!x]*.md", "readme.md"));
        assert!(!f("[!r]*.md", "readme.md"));
        // no glob characters: a case-insensitive substring
        assert!(f("MAIN", "src/main.rs"));
        assert!(!f("lib", "src/main.rs"));
    }

    #[test]
    fn paths_are_kept_inside_the_project() {
        let d = tempfile::tempdir().unwrap();
        let cwd = d.path().join("p");
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        assert!(guard(&cwd, &absolute(&cwd, "src/new.rs")).is_ok());
        assert!(guard(&cwd, &absolute(&cwd, "src/deep/new/file.rs")).is_ok());
        assert!(guard(&cwd, &absolute(&cwd, "../outside.rs")).is_err());
        assert!(guard(&cwd, Path::new("/etc/passwd")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path(), cwd.join("escape")).unwrap();
            assert!(guard(&cwd, &absolute(&cwd, "escape/x")).is_err());
        }
        assert_eq!(absolute(&cwd, "./src/../a.rs"), cwd.join("a.rs"));
        assert_eq!(display(&cwd, &cwd.join("src/a.rs")), "src/a.rs");
    }

    #[test]
    fn text_search_reports_lines_and_columns() {
        let re = regex::RegexBuilder::new("b+").multi_line(true).build().unwrap();
        let mut out = Vec::new();
        assert!(!search_text("f", "abb\nxx\n  bab\n", &re, 10, &mut out));
        assert_eq!(out.iter().map(|h| (h.line, h.col)).collect::<Vec<_>>(), [(1, 2), (3, 3)]);
        assert_eq!(out[1].text, "  bab");
        let mut out = Vec::new();
        assert!(search_text("f", "b\nb\nb\n", &re, 2, &mut out));
        assert_eq!(out.len(), 2);
        let re = regex::RegexBuilder::new("^x").multi_line(true).build().unwrap();
        let mut out = Vec::new();
        search_text("f", "ax\nxa\n", &re, 10, &mut out);
        assert_eq!(out[0].line, 2);
    }
}
