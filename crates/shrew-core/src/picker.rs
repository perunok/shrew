//! Fuzzy pickers: files, commands (palette), buffers, features. File lists stream in from a background thread so
//! the picker opens instantly even in huge trees.

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
}

#[derive(Clone, Debug)]
pub enum PickerData {
    Path(PathBuf),
    Command(Command),
    Buffer(usize),
    Feature(Feature),
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
}

impl Picker {
    pub fn new(kind: PickerKind, items: Vec<PickerItem>) -> Self {
        let mut p = Picker { kind, query: String::new(), items, matches: Vec::new(), selected: 0, loading: false, rx: None };
        p.refilter();
        p
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
                        if let Some(w) = &waker {
                            if sent < 4 || sent % 8 == 0 {
                                w();
                            }
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
        if changed {
            let keep = self.current().map(|i| i.label.clone());
            self.refilter();
            if let Some(k) = keep {
                if let Some(i) = self.matches.iter().position(|&m| self.items[m as usize].label == k) {
                    self.selected = i;
                }
            }
        }
        changed
    }

    pub fn refilter(&mut self) {
        let q = self.query.trim();
        if q.is_empty() {
            self.matches = (0..self.items.len() as u32).collect();
        } else {
            let mut scored: Vec<(i32, u32)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| fuzzy_score(q, &it.label).map(|s| (s, i as u32)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(self.items[a.1 as usize].label.len().cmp(&self.items[b.1 as usize].label.len())));
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
    let eq = |a: char, b: char| if case_sensitive { a == b } else { a.eq_ignore_ascii_case(&b) || a.to_lowercase().eq(b.to_lowercase()) };
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
}
