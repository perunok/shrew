//! Links in terminal output: file paths with an optional line and column (`src/main.rs:12:5`, `--> a.rs:3:1`,
//! Python's `File "x.py", line 7`, MSVC's `a.c(12,5)`) and URLs. The pointer underlines one when it exists, and
//! Ctrl+click opens it: files in the editor, URLs in the browser.

use regex::Regex;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// What a link points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A file path as written (not yet resolved), with 1-based line and column.
    File {
        path: String,
        line: Option<usize>,
        col: Option<usize>,
    },
    Url(String),
}

/// A link candidate in a line of text: its byte range and target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub bytes: Range<usize>,
    pub target: Target,
}

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\b(?:https?|file)://[^\s<>"'`]+"#).expect("valid regex"));

/// `path:line:col`, `path:line`, `path(line,col)`, `path(line)` or a bare path. A path is a run of path chars that
/// has a `/` or a `.` in it, so plain words are not taken for files.
static PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        (?P<path>
            (?:[A-Za-z]:[\\/])?               # a Windows drive
            [\w~.@+\-]*[\w~@+\-/\\]           # not ending in a dot
            (?:[\w~.@+\-/\\]*[\w~@+\-])?
        )
        (?:
            :(?P<line>\d+)(?::(?P<col>\d+))?  # :12 or :12:5
          | \((?P<pline>\d+)(?:,\s*(?P<pcol>\d+))?\)   # (12) or (12,5)
        )?",
    )
    .expect("valid regex")
});

static PYTHON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"File "(?P<path>[^"]+)", line (?P<line>\d+)"#).expect("valid regex"));

/// Every link candidate in `text`, left to right. Paths are candidates only: [`resolve`] says whether one exists.
pub fn find(text: &str) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    let taken = |out: &[Found], r: &Range<usize>| out.iter().any(|f| f.bytes.start < r.end && r.start < f.bytes.end);
    for m in URL.find_iter(text) {
        // trailing punctuation belongs to the sentence
        let url = m.as_str().trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '!', '?']);
        out.push(Found { bytes: m.start()..m.start() + url.len(), target: Target::Url(url.to_string()) });
    }
    for c in PYTHON.captures_iter(text) {
        let (whole, path) = (c.get(0).expect("match"), &c["path"]);
        if !taken(&out, &whole.range()) {
            let line = c["line"].parse().ok();
            let target = Target::File { path: path.to_string(), line, col: None };
            out.push(Found { bytes: whole.range(), target });
        }
    }
    for c in PATH.captures_iter(text) {
        let whole = c.get(0).expect("match");
        let path = c.name("path").expect("always matches").as_str();
        if taken(&out, &whole.range()) || !(path.contains('/') || path.contains('\\') || path.contains('.')) {
            continue;
        }
        // not part of a URL or an email-like word, and not just dots
        if path.chars().all(|ch| ch == '.' || ch == '/') {
            continue;
        }
        let num = |a: &str, b: &str| c.name(a).or_else(|| c.name(b)).and_then(|m| m.as_str().parse().ok());
        let target = Target::File { path: path.to_string(), line: num("line", "pline"), col: num("col", "pcol") };
        out.push(Found { bytes: whole.range(), target });
    }
    out.sort_by_key(|f| f.bytes.start);
    out
}

/// The link candidate covering byte `at` of `text`.
pub fn at(text: &str, at: usize) -> Option<Found> {
    find(text).into_iter().find(|f| f.bytes.contains(&at))
}

/// The file a path from the terminal means, relative to the shell's directory, if it is one.
pub fn resolve(path: &str, cwd: &Path) -> Option<PathBuf> {
    let p = match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(PathBuf::from)?.join(rest),
        None => PathBuf::from(path),
    };
    let p = if p.is_absolute() { p } else { cwd.join(p) };
    p.is_file().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(text: &str) -> Vec<(String, Option<usize>, Option<usize>, &str)> {
        find(text)
            .into_iter()
            .filter_map(|f| match f.target {
                Target::File { path, line, col } => Some((path, line, col, &text[f.bytes])),
                Target::Url(_) => None,
            })
            .collect()
    }

    #[test]
    fn compiler_and_grep_output() {
        assert_eq!(
            files("  --> crates/shrew/src/app.rs:42:9"),
            vec![("crates/shrew/src/app.rs".into(), Some(42), Some(9), "crates/shrew/src/app.rs:42:9")]
        );
        assert_eq!(files("src/main.rs:7: fn main() {"), vec![("src/main.rs".into(), Some(7), None, "src/main.rs:7")]);
        assert_eq!(files("error in ./build.sh"), vec![("./build.sh".into(), None, None, "./build.sh")]);
        assert_eq!(files("/etc/hosts:3:1:"), vec![("/etc/hosts".into(), Some(3), Some(1), "/etc/hosts:3:1")]);
        assert_eq!(files("see notes.md."), vec![("notes.md".into(), None, None, "notes.md")]);
        assert_eq!(files("~/x/y.txt:2"), vec![("~/x/y.txt".into(), Some(2), None, "~/x/y.txt:2")]);
        assert_eq!(files("main.c(12,5): error"), vec![("main.c".into(), Some(12), Some(5), "main.c(12,5)")]);
        assert_eq!(
            files(r#"  File "tools/run.py", line 18, in <module>"#),
            vec![("tools/run.py".into(), Some(18), None, r#"File "tools/run.py", line 18"#)]
        );
    }

    #[test]
    fn plain_words_and_dots_are_not_paths() {
        assert!(files("hello world").is_empty());
        assert!(files("... and ..").is_empty());
        assert_eq!(files("drwxr-xr-x 2 me me 4096 Cargo.toml").len(), 1);
    }

    #[test]
    fn urls_win_over_paths_inside_them() {
        let f = find("docs at https://github.com/perunok/shrew/blob/master/README.md, ok");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].target, Target::Url("https://github.com/perunok/shrew/blob/master/README.md".into()));
        let text = "x src/a.rs:1 y";
        let hit = at(text, 5).unwrap();
        assert_eq!(&text[hit.bytes], "src/a.rs:1");
        assert!(at(text, 0).is_none());
    }

    #[test]
    fn resolving_needs_a_file() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert_eq!(resolve("src/main.rs", dir), Some(dir.join("src/main.rs")));
        assert_eq!(resolve("src", dir), None, "a directory is not a link");
        assert_eq!(resolve("src/nope.rs", dir), None);
    }
}
