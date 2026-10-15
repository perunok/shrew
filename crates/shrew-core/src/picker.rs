//! Fuzzy pickers: files, commands (palette), buffers, features, and lists the app fills (themes, accents, fonts).
//! File lists stream in from a background thread so the picker opens instantly even in huge trees.

use crate::commands::Command;
use crate::features::Feature;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Files,
    Commands,
    Buffers,
    Features,
    Choice(Choice),
    // ----- project navigation -----
    /// Find in project: the query is the pattern, rows come grouped by file (see [`crate::grep`]).
    Grep,
    /// Symbols of the active file, or of the whole project.
    Symbols {
        project: bool,
    },
    /// Recent projects (Ctrl+Alt+P).
    Projects,
}

/// Settings picked from a list the app provides. The app previews the highlighted row as it moves (and restores
/// the old value on Esc); Enter sends [`Event::Choose`](crate::Event::Choose).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Choice {
    Theme,
    Accent,
    Font,
    // ----- git: diff view (no preview; the app fills and answers these) -----
    /// A commit to diff against its parent.
    GitCommit,
    /// A branch, tag or commit to compare (picked twice: the old side, then the new one).
    GitRef,
    // -----
    // ----- git: panel (the app's git module fills and answers these; no preview) -----
    GitBranches,
    GitStashes,
    GitChanges,
}

impl Choice {
    /// What the list holds, for the picker's header.
    pub fn noun(self) -> &'static str {
        match self {
            Choice::Theme => "themes",
            Choice::Accent => "accents",
            Choice::Font => "fonts",
            Choice::GitCommit => "commits",
            Choice::GitRef => "refs",
            Choice::GitBranches => "branches",
            Choice::GitStashes => "stashes",
            Choice::GitChanges => "changed files",
        }
    }

    /// A list of the git panel's (branches, stashes, changed files), not a setting.
    pub fn is_git(self) -> bool {
        matches!(self, Choice::GitBranches | Choice::GitStashes | Choice::GitChanges)
    }
}

#[derive(Clone, Debug)]
pub enum PickerData {
    Path(PathBuf),
    Command(Command),
    Buffer(usize),
    Feature(Feature),
    /// A [`Choice`] value (theme id, accent id, font family).
    Value(String),
    // ----- project navigation -----
    /// A place in a file (`None`: the active buffer), 0-based; `hl` is a byte range of the label to highlight.
    Location {
        path: Option<PathBuf>,
        line: usize,
        col: usize,
        hl: Option<(u32, u32)>,
    },
    /// A file's header row in grouped results: drawn, never selected.
    Header(PathBuf),
    /// A project folder to open.
    Project(PathBuf),
}

#[derive(Clone, Debug)]
pub struct PickerItem {
    pub label: String,
    pub detail: String,
    pub data: PickerData,
}

pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    pub items: Vec<PickerItem>,
    /// Indices into `items`, best first.
    pub matches: Vec<u32>,
    pub selected: usize,
    /// Still receiving items in the background.
    pub loading: bool,
    rx: Option<Receiver<Vec<PickerItem>>>,
    // ----- project navigation -----
    /// Leading chars of the query that pick the mode (Ctrl+P's `@`), not matched.
    pub prefix: usize,
    /// The Ctrl+P file list, kept while `@` shows symbols, so deleting the `@` brings it back.
    pub parked: Option<Box<Picker>>,
    /// Find in project's options and counters.
    pub grep: Option<crate::grep::GrepState>,
    /// Stops the background work feeding this picker when it closes.
    pub cancel: Option<Cancel>,
}

/// Sets its flag when dropped: background work checks it and stops.
#[derive(Debug, Default)]
pub struct Cancel(pub Arc<std::sync::atomic::AtomicBool>);

impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

impl Picker {
    pub fn new(kind: PickerKind, items: Vec<PickerItem>) -> Self {
        let mut p = Picker {
            kind,
            query: String::new(),
            items,
            matches: Vec::new(),
            selected: 0,
            loading: false,
            rx: None,
            prefix: 0,
            parked: None,
            grep: None,
            cancel: None,
        };
        p.refilter();
        p
    }

    /// A picker whose items arrive from a background thread on `rx`.
    pub fn streaming(kind: PickerKind, rx: Receiver<Vec<PickerItem>>) -> Self {
        let mut p = Picker::new(kind, Vec::new());
        p.rx = Some(rx);
        p.loading = true;
        p
    }

    /// Replaces the item source (a new find-in-project search): the old items go.
    pub fn restart(&mut self, rx: Option<Receiver<Vec<PickerItem>>>) {
        self.items.clear();
        self.matches.clear();
        self.selected = 0;
        self.loading = rx.is_some();
        self.rx = rx;
    }

    /// The part of the query that is matched: without the mode prefix, and for files without a `:line` suffix.
    pub fn pattern(&self) -> &str {
        let q = self.query.get(self.prefix.min(self.query.len())..).unwrap_or("");
        if self.kind == PickerKind::Files {
            return split_line_suffix(q).0;
        }
        q
    }

    /// Ctrl+P's `:42` (go to line) and `name:42` (open at line): the line typed.
    pub fn line_suffix(&self) -> Option<usize> {
        if self.kind != PickerKind::Files {
            return None;
        }
        split_line_suffix(&self.query).1
    }

    /// What to show instead of results (Ctrl+P's `:42`).
    pub fn hint(&self) -> Option<String> {
        let line = self.line_suffix()?;
        self.query.starts_with(':').then(|| format!("Enter: go to line {line}"))
    }

    fn selectable_item(&self, i: u32) -> bool {
        !matches!(self.items[i as usize].data, PickerData::Header(_))
    }

    /// Puts find in project's file groups (a header and its rows) in path order.
    fn sort_groups(&mut self) {
        let mut groups: Vec<Vec<PickerItem>> = Vec::new();
        for item in self.items.drain(..) {
            match (&item.data, groups.last_mut()) {
                (PickerData::Header(_), _) | (_, None) => groups.push(vec![item]),
                (_, Some(g)) => g.push(item),
            }
        }
        groups.sort_by(|a, b| a[0].label.cmp(&b[0].label));
        self.items = groups.into_iter().flatten().collect();
    }

    fn selectable(&self, at: usize) -> bool {
        self.matches.get(at).is_some_and(|&i| !matches!(self.items[i as usize].data, PickerData::Header(_)))
    }

    /// Lists the files under `root` (respecting .gitignore) on a background thread.
    #[cfg(feature = "picker")]
    pub fn files(root: &std::path::Path, waker: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let root = root.to_path_buf();
        std::thread::Builder::new()
            .name("shrew-files".into())
            .spawn(move || {
                let mut batch = Vec::with_capacity(512);
                let walker = ignore::WalkBuilder::new(&root).hidden(true).git_ignore(true).parents(true).build();
                let mut sent = 0usize;
                for entry in walker.flatten() {
                    if !entry.file_type().is_some_and(|t| t.is_file()) {
                        continue;
                    }
                    let path = entry.into_path();
                    let rel = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
                    batch.push(PickerItem { label: rel, detail: String::new(), data: PickerData::Path(path) });
                    if batch.len() >= 512 {
                        if tx.send(std::mem::take(&mut batch)).is_err() {
                            return;
                        }
                        sent += 1;
                        if let Some(w) = &waker
                            && (sent < 4 || sent.is_multiple_of(8))
                        {
                            w();
                        }
                    }
                    if sent > 2_000 {
                        break; // a million files: stop listing
                    }
                }
                let _ = tx.send(batch);
                if let Some(w) = &waker {
                    w();
                }
            })
            .ok();
        let mut p = Picker::new(PickerKind::Files, Vec::new());
        p.rx = Some(rx);
        p.loading = true;
        p
    }

    #[cfg(not(feature = "picker"))]
    #[allow(dead_code)]
    fn _unused(_: Option<Arc<dyn Fn() + Send + Sync>>) {}

    /// Takes in items that arrived from the background. True when the list changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(batch) => {
                    self.items.extend(batch);
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    self.loading = false;
                    changed = true;
                    break;
                }
            }
        }
        if changed && self.kind == PickerKind::Grep {
            // results come in arrival order; once all are in, and nothing was picked yet, files go in path order
            let untouched =
                self.matches.iter().position(|&i| self.selectable_item(i)).is_none_or(|f| f == self.selected);
            if !self.loading && untouched {
                self.sort_groups();
                self.selected = 0;
            }
            self.matches = (0..self.items.len() as u32).collect();
            if !self.selectable(self.selected) {
                self.move_by(1);
            }
            return true;
        }
        if changed {
            // a row picked by moving stays picked; untouched, the best match stays on top
            let keep = self.current().filter(|_| self.selected > 0).map(|i| i.label.clone());
            self.refilter();
            if let Some(k) = keep
                && let Some(i) = self.matches.iter().position(|&m| self.items[m as usize].label == k)
            {
                self.selected = i;
            }
        }
        changed
    }

    pub fn refilter(&mut self) {
        let q = self.pattern().trim();
        if self.kind == PickerKind::Grep {
            self.matches = (0..self.items.len() as u32).collect();
            self.selected = 0;
            if !self.selectable(0) {
                self.move_by(1);
            }
            return;
        }
        if self.kind == PickerKind::Files && self.query.starts_with(':') {
            self.matches.clear();
        } else if q.is_empty() {
            self.matches = (0..self.items.len() as u32).collect();
        } else {
            let mut scored: Vec<(i32, u32)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| fuzzy_score(q, &it.label).map(|s| (s, i as u32)))
                .collect();
            scored.sort_by(|a, b| {
                b.0.cmp(&a.0).then(self.items[a.1 as usize].label.len().cmp(&self.items[b.1 as usize].label.len()))
            });
            self.matches = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.selected = 0;
    }

    pub fn current(&self) -> Option<&PickerItem> {
        self.matches.get(self.selected).map(|&i| &self.items[i as usize])
    }

    pub fn move_by(&mut self, d: isize) {
        if self.matches.is_empty() {
            return;
        }
        let n = self.matches.len() as isize;
        self.selected = (self.selected as isize + d).rem_euclid(n) as usize;
        // header rows are skipped, in the direction of travel
        let step = if d < 0 { -1 } else { 1 };
        for _ in 0..n {
            if self.selectable(self.selected) {
                break;
            }
            self.selected = (self.selected as isize + step).rem_euclid(n) as usize;
        }
    }
}

/// `name:42` -> (`name`, Some(42)); anything else unchanged.
fn split_line_suffix(q: &str) -> (&str, Option<usize>) {
    match q.rsplit_once(':') {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => (head, n.parse().ok()),
        Some((head, "")) => (head, None),
        _ => (q, None),
    }
}

/// Subsequence fuzzy match with bonuses for word starts, consecutive runs and the file name. None if `query` is
/// not a subsequence of `cand`. Smart case.
pub fn fuzzy_score(query: &str, cand: &str) -> Option<i32> {
    fuzzy_match(query, cand).map(|(s, _)| s)
}

/// Like [`fuzzy_score`], also returning the matched char positions (for highlighting).
pub fn fuzzy_match(query: &str, cand: &str) -> Option<(i32, Vec<usize>)> {
    let case_sensitive = query.chars().any(|c| c.is_uppercase());
    let eq = |a: char, b: char| {
        if case_sensitive { a == b } else { a.eq_ignore_ascii_case(&b) || a.to_lowercase().eq(b.to_lowercase()) }
    };
    let chars: Vec<char> = cand.chars().collect();
    let name_start = cand.rfind('/').map_or(0, |b| cand[..=b].chars().count());
    let mut positions = Vec::with_capacity(query.len());
    let mut ci = 0;
    for q in query.chars().filter(|c| !c.is_whitespace()) {
        // prefer a word-start match ahead, else the first match
        let first = (ci..chars.len()).find(|&i| eq(chars[i], q))?;
        let boundary = (first..chars.len()).take(64).find(|&i| eq(chars[i], q) && is_boundary(&chars, i));
        let pick = match boundary {
            Some(b) if positions.last().is_none_or(|&l: &usize| l + 1 != first) => b,
            _ => first,
        };
        positions.push(pick);
        ci = pick + 1;
    }
    let mut score = 0i32;
    let mut prev: Option<usize> = None;
    for &p in &positions {
        score += 16;
        if is_boundary(&chars, p) {
            score += 24;
        }
        if p >= name_start {
            score += 12;
        }
        if let Some(pp) = prev {
            if pp + 1 == p {
                score += 20;
            } else {
                score -= (p - pp).min(20) as i32;
            }
        } else {
            score -= p.min(30) as i32 / 2;
        }
        prev = Some(p);
    }
    score -= (chars.len() as i32) / 16;
    Some((score, positions))
}

fn is_boundary(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let (p, c) = (chars[i - 1], chars[i]);
    matches!(p, '/' | '_' | '-' | '.' | ' ' | ':') || (p.is_lowercase() && c.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy() {
        assert!(fuzzy_score("edrs", "crates/shrew-core/src/editor.rs").is_some());
        assert!(fuzzy_score("xyz", "editor.rs").is_none());
        let a = fuzzy_score("ed", "src/editor.rs").unwrap();
        let b = fuzzy_score("ed", "src/shared.rs").unwrap();
        assert!(a > b);
    }

    fn item(label: &str, data: PickerData) -> PickerItem {
        PickerItem { label: label.into(), detail: String::new(), data }
    }

    #[test]
    fn line_suffixes_in_the_file_picker() {
        let mut p = Picker::new(PickerKind::Files, vec![item("src/editor.rs", PickerData::Path("e".into()))]);
        p.query = "edit:42".into();
        p.refilter();
        assert_eq!((p.pattern(), p.line_suffix(), p.matches.len()), ("edit", Some(42), 1));
        p.query = ":7".into();
        p.refilter();
        assert!(p.matches.is_empty());
        assert_eq!(p.hint().as_deref(), Some("Enter: go to line 7"));
        p.query = "@main".into();
        p.prefix = 1;
        assert_eq!(p.pattern(), "main");
    }

    #[test]
    fn headers_are_never_selected() {
        let loc = || PickerData::Location { path: None, line: 0, col: 0, hl: None };
        let items = vec![
            item("a.rs", PickerData::Header("a.rs".into())),
            item("x", loc()),
            item("b.rs", PickerData::Header("b.rs".into())),
            item("y", loc()),
        ];
        let mut p = Picker::new(PickerKind::Grep, items);
        assert_eq!(p.selected, 1);
        p.move_by(1);
        assert_eq!(p.selected, 3);
        p.move_by(1);
        assert_eq!(p.selected, 1, "wraps past the header");
        p.move_by(-1);
        assert_eq!(p.selected, 3);
    }
}
