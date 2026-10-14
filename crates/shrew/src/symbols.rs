//! The symbol provider shrew-core's go to symbol asks (Ctrl+R, `@` in Ctrl+P, Ctrl+Shift+R): grammar scopes via
//! [`shrew_syntax::symbols`], parsed on background threads. The project index keeps each file's symbols with its
//! modification time, so opening it again only parses what changed.

use crate::state::Waker;
use shrew_core::navigate::{SymbolProvider, SymbolScope};
use shrew_core::picker::{PickerData, PickerItem};
use shrew_syntax::symbols::{self, Symbol};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Project files bigger than this are not indexed (bytes).
const MAX_FILE_BYTES: u64 = 1 << 20;

/// path -> (modified, symbols)
type Index = Mutex<HashMap<PathBuf, (SystemTime, Arc<Vec<Symbol>>)>>;

pub fn provider(wake: Waker) -> SymbolProvider {
    let index: Arc<Index> = Arc::default();
    Arc::new(move |scope, cancel| {
        let (tx, rx) = mpsc::channel();
        let (index, wake) = (index.clone(), wake.clone());
        // a failed spawn drops `tx`: the picker just stops loading
        let _ = std::thread::Builder::new().name("shrew-symbols".into()).spawn(move || {
            match scope {
                SymbolScope::File { path, text } => file(path, &text.to_string(), &cancel, &tx),
                SymbolScope::Project { root } => project(&root, &index, &cancel, &tx, &wake),
            }
            drop(tx);
            wake();
        });
        rx
    })
}

fn file(path: Option<PathBuf>, text: &str, cancel: &AtomicBool, tx: &Sender<Vec<PickerItem>>) {
    let ss = symbols::default_set();
    let found = symbols::extract(&ss, path.as_deref(), text, &|| cancel.load(Ordering::Acquire));
    let items = found
        .into_iter()
        .map(|s| PickerItem {
            detail: format!("{}  {}", s.kind, s.line + 1),
            label: s.name,
            data: PickerData::Location { path: None, line: s.line, col: s.col, hl: None },
        })
        .collect();
    let _ = tx.send(items);
}

fn project(root: &Path, index: &Index, cancel: &AtomicBool, tx: &Sender<Vec<PickerItem>>, wake: &Waker) {
    let ss = symbols::default_set();
    let tx = Mutex::new(tx.clone());
    let stop = || cancel.load(Ordering::Acquire);
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
    let each = |path: &Path| {
        if !symbols::has_grammar(&ss, path) {
            return true;
        }
        let Ok(meta) = std::fs::metadata(path) else { return true };
        if meta.len() > MAX_FILE_BYTES {
            return true;
        }
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let cached = index.lock().unwrap_or_else(|e| e.into_inner()).get(path).filter(|c| c.0 == modified).cloned();
        let found = match cached {
            Some((_, s)) => s,
            None => {
                let Ok(text) = std::fs::read_to_string(path) else { return true };
                let s = Arc::new(symbols::extract(&ss, Some(path), &text, &stop));
                if stop() {
                    return false; // cut short: not worth caching
                }
                index.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), (modified, s.clone()));
                s
            }
        };
        if found.is_empty() {
            return true;
        }
        let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
        let items = found
            .iter()
            .map(|s| PickerItem {
                label: s.name.clone(),
                detail: format!("{}  {rel}:{}", s.kind, s.line + 1),
                data: PickerData::Location { path: Some(path.to_path_buf()), line: s.line, col: s.col, hl: None },
            })
            .collect();
        let sent = tx.lock().unwrap_or_else(|e| e.into_inner()).send(items).is_ok();
        wake();
        sent && !stop()
    };
    #[cfg(feature = "project")]
    let _ = shrew_core::project::walk_files(root, "", threads, &stop, &each);
    #[cfg(not(feature = "project"))]
    let _ = (root, threads, each);
}

#[cfg(all(test, feature = "project"))]
mod tests {
    use super::*;

    #[test]
    fn indexes_a_project_and_reuses_it() {
        let d = std::env::temp_dir().join(format!("shrew-symbols-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/a.rs"), "fn alpha() {}\nstruct Beta;\n").unwrap();
        std::fs::write(d.join("notes.txt"), "fn not_a_symbol() {}\n").unwrap();
        let p = provider(Arc::new(|| {}));
        let run = |p: &SymbolProvider| {
            let rx = p(SymbolScope::Project { root: d.clone() }, Arc::new(AtomicBool::new(false)));
            let mut names: Vec<(String, String)> = rx.iter().flatten().map(|i| (i.label, i.detail)).collect();
            names.sort();
            names
        };
        // paths show with the platform's separator (`src\a.rs` on Windows)
        let a = std::path::Path::new("src").join("a.rs").display().to_string();
        let want =
            vec![("Beta".to_string(), format!("struct  {a}:2")), ("alpha".to_string(), format!("function  {a}:1"))];
        assert_eq!(run(&p), want);
        assert_eq!(run(&p), want, "the second run comes from the index");
        let rx = p(
            SymbolScope::File { path: Some(d.join("x.py")), text: "def f():\n    pass\n".into() },
            Arc::new(AtomicBool::new(false)),
        );
        let items: Vec<PickerItem> = rx.iter().flatten().collect();
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0].data, PickerData::Location { path: None, line: 0, col: 4, .. }));
        std::fs::remove_dir_all(&d).ok();
    }
}
