//! Find in project: a pure-Rust grep over the project's files (parallel, `.gitignore`-aware walk), streaming one
//! batch per file into the find-in-project picker: a header row with the file's path, then a row per matching line.
//!
//! Literal by default with smart case (an upper-case letter makes it case-sensitive), regex on request. Every new
//! query cancels the search before it, and results stop at [`MAX_MATCHES`], so typing never waits on a big tree.

use crate::picker::{PickerData, PickerItem};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Results stop here; the picker says there are more.
pub const MAX_MATCHES: usize = 5_000;
/// Matching lines shown per file.
pub const MAX_PER_FILE: usize = 500;
/// Files bigger than this are skipped (bytes).
pub const MAX_FILE_BYTES: u64 = 8 << 20;
/// Preview text kept around a match (chars).
const PREVIEW: usize = 160;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrepOptions {
    /// The pattern is a regex (else literal text).
    pub regex: bool,
    /// Always case-sensitive (else smart case).
    pub case: bool,
    /// Whole words only.
    pub word: bool,
    /// Include/exclude globs, comma or space separated: `*.rs, !tests/`.
    pub globs: String,
}

/// Counters the picker shows while results stream in.
#[derive(Debug, Default)]
pub struct GrepStats {
    pub files: AtomicUsize,
    pub matches: AtomicUsize,
    /// Results were cut at [`MAX_MATCHES`] (or [`MAX_PER_FILE`] in some file).
    pub capped: AtomicBool,
}

/// Which line of the find-in-project header the keys edit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GrepField {
    #[default]
    Pattern,
    Globs,
}

/// The find-in-project picker's own state (the pattern is the picker's query).
#[derive(Debug, Default)]
pub struct GrepState {
    pub root: PathBuf,
    pub opts: GrepOptions,
    pub field: GrepField,
    pub stats: Arc<GrepStats>,
    /// The pattern does not compile (regex mode) or a glob is bad.
    pub error: Option<String>,
    /// Set to stop the running search.
    pub(crate) cancel: Arc<AtomicBool>,
}

impl Drop for GrepState {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// The regex a query searches with: escaped unless `opts.regex`, smart case unless `opts.case`.
pub fn build_regex(pattern: &str, opts: &GrepOptions) -> Result<Regex, String> {
    let body = if opts.regex { pattern.to_string() } else { regex::escape(pattern) };
    let body = if opts.word { format!(r"\b(?:{body})\b") } else { body };
    let insensitive = !opts.case && !pattern.chars().any(char::is_uppercase);
    regex::RegexBuilder::new(&body)
        .case_insensitive(insensitive)
        .multi_line(true)
        .size_limit(1 << 22)
        .build()
        .map_err(|e| e.to_string().lines().last().unwrap_or("bad pattern").trim().to_string())
}

/// The preview of a matching line: trimmed, cut around the match, with the match's byte range in it.
pub fn preview(line: &str, a: usize, z: usize) -> (String, (u32, u32)) {
    let line = line.trim_end_matches(['\n', '\r']);
    let lead = line.len() - line.trim_start().len();
    let (mut start, mut prefix) = (lead.min(a), "");
    // a match far to the right: keep a little context before it
    if line[start..a].chars().count() > PREVIEW / 3 {
        let mut s = a;
        for _ in 0..PREVIEW / 6 {
            match line[..s].char_indices().next_back() {
                Some((i, _)) => s = i,
                None => break,
            }
        }
        start = s;
        prefix = "…";
    }
    let mut end = line.len();
    if let Some((i, _)) = line[start..].char_indices().nth(PREVIEW) {
        end = (start + i).max(z.min(line.len()));
    }
    let text: String = line[start..end].chars().map(|c| if c == '\t' { ' ' } else { c }).collect();
    let shift = prefix.len() as i64 - start as i64;
    let hl = ((a as i64 + shift).max(0) as u32, ((z.min(end)) as i64 + shift).max(0) as u32);
    (format!("{prefix}{text}"), hl)
}

/// Searches one file's text: header plus matching lines, or nothing when it has no match.
pub fn search_text(path: &Path, rel: &str, text: &str, re: &Regex, stats: &GrepStats) -> Vec<PickerItem> {
    let mut out = Vec::new();
    if !re.is_match(text) {
        return out;
    }
    let mut n = 0;
    for (line, l) in text.split_inclusive('\n').enumerate() {
        let Some(m) = re.find(l) else { continue };
        if m.start() == m.end() && l.trim().is_empty() {
            continue;
        }
        if n == MAX_PER_FILE || stats.matches.load(Ordering::Relaxed) >= MAX_MATCHES {
            stats.capped.store(true, Ordering::Relaxed);
            break;
        }
        n += 1;
        stats.matches.fetch_add(1, Ordering::Relaxed);
        let (label, hl) = preview(l, m.start(), m.end());
        let col = l[..m.start()].chars().count();
        out.push(PickerItem {
            label,
            detail: String::new(),
            data: PickerData::Location { path: Some(path.to_path_buf()), line, col, hl: Some(hl) },
        });
    }
    if n > 0 {
        stats.files.fetch_add(1, Ordering::Relaxed);
        let header =
            PickerItem { label: rel.to_string(), detail: format!("{n}"), data: PickerData::Header(path.to_path_buf()) };
        out.insert(0, header);
    }
    out
}

/// Starts a search on a background thread; results come one file at a time.
#[cfg(feature = "project")]
pub fn spawn(
    root: &Path,
    re: Regex,
    globs: &str,
    stats: Arc<GrepStats>,
    cancel: Arc<AtomicBool>,
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
) -> std::sync::mpsc::Receiver<Vec<PickerItem>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let root = root.to_path_buf();
    let globs = globs.to_string();
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8);
    // if the thread cannot start, `tx` goes with the closure and the picker just stops loading
    let _ = std::thread::Builder::new().name("shrew-grep".into()).spawn(move || {
        let tx = std::sync::Mutex::new(tx);
        let stop = || cancel.load(Ordering::Acquire) || stats.matches.load(Ordering::Relaxed) >= MAX_MATCHES;
        let found = |path: &Path| {
            let Ok(meta) = std::fs::metadata(path) else { return true };
            if meta.len() > MAX_FILE_BYTES || meta.len() == 0 {
                return true;
            }
            let Ok(bytes) = std::fs::read(path) else { return true };
            // binary files have a NUL early on
            if bytes[..bytes.len().min(8192)].contains(&0) {
                return true;
            }
            let text = String::from_utf8_lossy(&bytes);
            let rel = path.strip_prefix(&root).unwrap_or(path).display().to_string();
            let items = search_text(path, &rel, &text, &re, &stats);
            if items.is_empty() {
                return true;
            }
            let sent = tx.lock().unwrap_or_else(|e| e.into_inner()).send(items).is_ok();
            if let Some(w) = &waker {
                w();
            }
            sent && !cancel.load(Ordering::Acquire)
        };
        // bad globs were reported before the search started (`project::check_globs`)
        let _ = crate::project::walk_files(&root, &globs, threads, &stop, &found);
        if stats.matches.load(Ordering::Relaxed) >= MAX_MATCHES {
            stats.capped.store(true, Ordering::Relaxed);
        }
        drop(tx);
        if let Some(w) = &waker {
            w();
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> GrepOptions {
        GrepOptions::default()
    }

    #[test]
    fn literal_smart_case_by_default() {
        let re = build_regex("a.b", &opts()).unwrap();
        assert!(re.is_match("xA.Bx"));
        assert!(!re.is_match("axb"), "literal: the dot is a dot");
        let re = build_regex("Foo", &opts()).unwrap();
        assert!(!re.is_match("foo"), "an upper-case letter makes it case-sensitive");
        let re = build_regex("a.b", &GrepOptions { regex: true, ..opts() }).unwrap();
        assert!(re.is_match("axb"));
        let re = build_regex("foo", &GrepOptions { word: true, ..opts() }).unwrap();
        assert!(re.is_match("a foo b") && !re.is_match("food"));
        let re = build_regex("foo", &GrepOptions { case: true, ..opts() }).unwrap();
        assert!(!re.is_match("FOO"));
        assert!(build_regex("(", &GrepOptions { regex: true, ..opts() }).is_err());
    }

    #[test]
    fn previews_keep_the_match() {
        let (p, (a, z)) = preview("    let x = foo(1);\n", 12, 15);
        assert_eq!(p, "let x = foo(1);");
        assert_eq!(&p[a as usize..z as usize], "foo");
        let long = format!("{}needle{}", "x".repeat(300), "y".repeat(300));
        let (p, (a, z)) = preview(&long, 300, 306);
        assert!(p.starts_with('…') && p.chars().count() < 200, "{}", p.len());
        assert_eq!(&p[a as usize..z as usize], "needle");
    }

    #[test]
    fn one_header_per_file() {
        let stats = GrepStats::default();
        let re = build_regex("fn", &opts()).unwrap();
        let items = search_text(Path::new("/p/a.rs"), "a.rs", "fn a() {}\n\nfn b() {}\nlet x;\n", &re, &stats);
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0].data, PickerData::Header(_)));
        assert_eq!(items[0].label, "a.rs");
        assert!(matches!(items[2].data, PickerData::Location { line: 2, col: 0, .. }));
        assert_eq!(stats.matches.load(Ordering::Relaxed), 2);
        assert!(search_text(Path::new("/p/b.rs"), "b.rs", "nothing", &re, &stats).is_empty());
    }

    #[cfg(feature = "project")]
    #[test]
    fn searches_a_tree() {
        let d = std::env::temp_dir().join(format!("shrew-grep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/a.rs"), "fn needle() {}\n").unwrap();
        std::fs::write(d.join("b.txt"), "a Needle\nnone\n").unwrap();
        std::fs::write(d.join("bin.dat"), b"needle\0\0").unwrap();
        let run = |pat: &str, globs: &str| {
            let stats = Arc::new(GrepStats::default());
            let re = build_regex(pat, &opts()).unwrap();
            let rx = spawn(&d, re, globs, stats.clone(), Arc::new(AtomicBool::new(false)), None);
            let mut files: Vec<String> = rx
                .iter()
                .flatten()
                .filter_map(|i| matches!(i.data, PickerData::Header(_)).then_some(i.label))
                .collect();
            files.sort();
            (files, stats.matches.load(Ordering::Relaxed))
        };
        assert_eq!(run("needle", ""), (vec!["b.txt".to_string(), "src/a.rs".to_string()], 2), "binary skipped");
        assert_eq!(run("needle", "*.rs"), (vec!["src/a.rs".to_string()], 1));
        assert_eq!(run("Needle", ""), (vec!["b.txt".to_string()], 1));
        std::fs::remove_dir_all(&d).ok();
    }
}
