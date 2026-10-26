//! The sidebar's file tree: the project folder, read lazily (a folder is listed when it is first expanded), with
//! `.gitignore`d entries marked so the sidebar can dim them, a filter (`/`), and the file operations (new file and
//! folder, rename, duplicate, move to the trash; the app's right-click menu asks for them through
//! [`Editor::tree_ask_in`]). The tree follows the active file: it expands to it and selects it.
//!
//! Nothing watches the disk: folders are read again on `R`, when the window gets the focus back, and after the tree's
//! own file operations. That costs nothing while you type.
//!
//! Compact folders (feature `compact_folders`, like VS Code's): a folder whose only entry is another folder shares
//! its row with it, so `src/main/java/com` is one row instead of four nested ones. The row stands for the deepest
//! folder: opening, new files, rename and delete act on that one.

use crate::editor::{Editor, Mode, PromptKind};
use crate::features::Feature;
use crate::input::{Key, KeyPress};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Never listed: version-control internals.
const HIDDEN: &[&str] = &[".git", ".hg", ".svn", ".jj", ".DS_Store"];

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    path: PathBuf,
    dir: bool,
    ignored: bool,
}

/// One visible line of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub dir: bool,
    /// An expanded folder.
    pub open: bool,
    /// Ignored by `.gitignore` (drawn dimmed).
    pub ignored: bool,
    /// Chars of `name` the filter matched.
    pub hl: Vec<usize>,
}

/// A question the sidebar asks on the command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    NewFile,
    NewFolder,
    Rename,
    Delete,
    /// Copy a file or folder under a new name (the sidebar's menu).
    Duplicate,
}

pub struct Tree {
    pub root: PathBuf,
    listings: HashMap<PathBuf, Vec<Entry>>,
    expanded: HashSet<PathBuf>,
    pub rows: Vec<Row>,
    pub selected: usize,
    /// First row on screen; the sidebar keeps the selection in view while `follow` is set.
    pub top: usize,
    pub follow: bool,
    /// Rows the sidebar shows (set when drawing; PageUp/PageDown move by it).
    pub page: usize,
    pub filter: String,
    /// Keys go to the filter (after `/`, until Enter or Esc).
    pub filtering: bool,
    /// The file the tree last revealed.
    revealed: Option<PathBuf>,
    /// What the open sidebar prompt acts on.
    target: Option<(Ask, PathBuf)>,
    /// Merge chains of single-folder folders into one row.
    pub compact: bool,
    /// For folders not listed yet: their only entry when that is a folder (one cheap `read_dir` each).
    probes: HashMap<PathBuf, Option<(String, PathBuf)>>,
    /// The row under the mouse pointer (drawn lightly highlighted; the app keeps it current).
    pub hover: Option<usize>,
    /// The last click on a row, for double clicks (the app's mouse sets it).
    pub last_click: Option<(std::time::Instant, usize)>,
}

impl Tree {
    pub fn new(root: PathBuf) -> Tree {
        let mut t = Tree {
            root,
            listings: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: 0,
            top: 0,
            follow: true,
            page: 20,
            filter: String::new(),
            filtering: false,
            revealed: None,
            target: None,
            compact: true,
            probes: HashMap::new(),
            hover: None,
            last_click: None,
        };
        t.rebuild();
        t
    }

    pub fn current(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// Reads the expanded folders again (files made or removed outside shrew show up).
    pub fn refresh(&mut self) {
        self.listings.clear();
        self.probes.clear();
        self.expanded.retain(|p| p.is_dir());
        self.rebuild();
    }

    /// Lays the visible rows out again, keeping the selected path selected.
    pub fn rebuild(&mut self) {
        let keep = self.current().map(|r| r.path.clone());
        let mut rows = Vec::new();
        let root = self.root.clone();
        let filter = self.filter.trim().to_string();
        self.push_rows(&root, 0, &filter, &mut rows);
        self.rows = rows;
        match keep.and_then(|p| self.rows.iter().position(|r| r.path == p)) {
            Some(i) => self.selected = i,
            None => self.selected = self.selected.min(self.rows.len().saturating_sub(1)),
        }
    }

    /// Adds the rows of `dir`'s entries (and of its expanded folders). True when something matched the filter.
    fn push_rows(&mut self, dir: &Path, depth: usize, filter: &str, rows: &mut Vec<Row>) -> bool {
        if !self.listings.contains_key(dir) {
            self.listings.insert(dir.to_path_buf(), list_dir(dir));
        }
        let entries = std::mem::take(self.listings.get_mut(dir).expect("just listed"));
        let mut any = false;
        for e in &entries {
            let hl = if filter.is_empty() {
                Some(Vec::new())
            } else {
                crate::picker::fuzzy_match(filter, &e.name).map(|m| m.1)
            };
            let (name, path) = if e.dir && self.compact { self.chain(e) } else { (e.name.clone(), e.path.clone()) };
            let hl = if filter.is_empty() || name == e.name {
                hl
            } else {
                crate::picker::fuzzy_match(filter, &name).map(|m| m.1)
            };
            let open = e.dir && self.expanded.contains(&path);
            let at = rows.len();
            rows.push(Row {
                path: path.clone(),
                name,
                depth,
                dir: e.dir,
                open,
                ignored: e.ignored,
                hl: hl.clone().unwrap_or_default(),
            });
            let inner = open && self.push_rows(&path, depth + 1, filter, rows);
            if filter.is_empty() || hl.is_some() || inner {
                any = true;
            } else {
                rows.truncate(at);
            }
        }
        self.listings.insert(dir.to_path_buf(), entries);
        any
    }

    /// `dir`'s row: its name joined with the folders below it while each holds exactly one entry, a folder.
    fn chain(&mut self, e: &Entry) -> (String, PathBuf) {
        let (mut name, mut path) = (e.name.clone(), e.path.clone());
        // a symlink loop cannot go on for ever
        for _ in 0..64 {
            let Some((n, p)) = self.only_subdir(&path) else { break };
            name.push('/');
            name.push_str(&n);
            path = p;
        }
        (name, path)
    }

    /// The only entry of `dir` when it is a folder.
    fn only_subdir(&mut self, dir: &Path) -> Option<(String, PathBuf)> {
        if let Some(entries) = self.listings.get(dir) {
            return match entries.as_slice() {
                [e] if e.dir => Some((e.name.clone(), e.path.clone())),
                _ => None,
            };
        }
        self.probes.entry(dir.to_path_buf()).or_insert_with(|| probe_only_subdir(dir)).clone()
    }

    pub fn set_open(&mut self, path: &Path, open: bool) {
        if open {
            // a folder reads its entries again when it opens
            self.listings.remove(path);
            self.expanded.insert(path.to_path_buf());
        } else {
            self.expanded.remove(path);
        }
        self.rebuild();
    }

    /// Closes every folder; the selection moves to the top-level entry it was in.
    pub fn collapse_all(&mut self) {
        let top = self.current().and_then(|r| {
            let rel = r.path.strip_prefix(&self.root).ok()?;
            rel.components().next().map(|c| self.root.join(c))
        });
        self.expanded.clear();
        self.rebuild();
        // a compact row stands for its deepest folder: the top-level entry is the start of its path
        if let Some(top) = top
            && let Some(i) = self.rows.iter().position(|r| r.path.starts_with(&top))
        {
            self.selected = i;
        }
        self.top = 0;
        self.follow = true;
    }

    /// Expands the folders down to `path` and selects it. False when it is outside the tree.
    pub fn reveal(&mut self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else { return false };
        let mut dir = self.root.clone();
        let parts: Vec<_> = rel.components().collect();
        for c in parts.iter().take(parts.len().saturating_sub(1)) {
            dir.push(c);
            self.expanded.insert(dir.clone());
        }
        self.rebuild();
        self.select_path(path)
    }

    pub fn select_path(&mut self, path: &Path) -> bool {
        match self.rows.iter().position(|r| r.path == path) {
            Some(i) => {
                self.selected = i;
                self.follow = true;
                true
            }
            None => false,
        }
    }

    pub fn move_by(&mut self, d: isize) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (self.selected as isize + d).clamp(0, self.rows.len() as isize - 1) as usize;
        self.follow = true;
    }

    /// Scrolls by `d` rows without moving the selection (the mouse wheel).
    pub fn scroll(&mut self, d: isize) {
        let max = self.rows.len().saturating_sub(self.page.max(1)) as isize;
        self.top = (self.top as isize + d).clamp(0, max.max(0)) as usize;
        self.follow = false;
    }

    /// Keeps the selection on screen when `follow` is set; clamps the scroll.
    pub fn fit(&mut self, visible: usize) {
        self.page = visible.max(1);
        if self.follow {
            if self.selected < self.top {
                self.top = self.selected;
            } else if self.selected >= self.top + self.page {
                self.top = self.selected + 1 - self.page;
            }
        }
        self.top = self.top.min(self.rows.len().saturating_sub(self.page));
    }

    /// The folder new entries go into: the selected folder, or the selected file's.
    fn target_dir(&self) -> PathBuf {
        match self.current() {
            Some(r) if r.dir => r.path.clone(),
            Some(r) => r.path.parent().map_or_else(|| self.root.clone(), Path::to_path_buf),
            None => self.root.clone(),
        }
    }

    /// Forgets the listing of `dir` (and makes sure the folders down to it are open).
    fn changed(&mut self, dir: &Path) {
        self.listings.remove(dir);
        // a chain splits (or joins) when one of its folders gains or loses an entry
        self.probes.clear();
        if let Ok(rel) = dir.strip_prefix(&self.root) {
            let mut d = self.root.clone();
            for c in rel.components() {
                d.push(c);
                self.expanded.insert(d.clone());
            }
        }
    }
}

/// Reads `dir` only as far as needed to tell whether its one entry is a folder.
fn probe_only_subdir(dir: &Path) -> Option<(String, PathBuf)> {
    let mut only = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if HIDDEN.contains(&name.as_str()) {
            continue;
        }
        if only.is_some() {
            return None;
        }
        let path = e.path();
        if !e.file_type().is_ok_and(|t| t.is_dir() || (t.is_symlink() && path.is_dir())) {
            return None;
        }
        only = Some((name, path));
    }
    only
}

fn list_dir(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Entry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if HIDDEN.contains(&name.as_str()) {
                return None;
            }
            let path = e.path();
            let dir = e.file_type().is_ok_and(|t| t.is_dir() || (t.is_symlink() && path.is_dir()));
            Some(Entry { name, path, dir, ignored: false })
        })
        .collect();
    mark_ignored(dir, &mut out);
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| natural_cmp(&a.name, &b.name)));
    out
}

/// Marks what `.gitignore` (and `.ignore`, `.git/info/exclude`) leave out of `dir`.
#[cfg(feature = "project")]
fn mark_ignored(dir: &Path, entries: &mut [Entry]) {
    if entries.is_empty() {
        return;
    }
    let kept: HashSet<PathBuf> = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .parents(true)
        .build()
        .flatten()
        .map(|e| e.into_path())
        .collect();
    for e in entries {
        e.ignored = !kept.contains(&e.path);
    }
}

#[cfg(not(feature = "project"))]
fn mark_ignored(_: &Path, _: &mut [Entry]) {}

/// Case-insensitive, with digit runs compared as numbers (`file2` before `file10`).
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (n, m) = (num(&mut x), num(&mut y));
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let o = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if o.is_ne() {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o.is_ne() {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

impl Ask {
    /// The command-line prompt for it.
    fn label(self, name: &str) -> String {
        match self {
            Ask::NewFile => "New file: ".into(),
            Ask::NewFolder => "New folder: ".into(),
            Ask::Rename => format!("Rename {name} to: "),
            Ask::Duplicate => format!("Duplicate {name} as: "),
            Ask::Delete if cfg!(all(unix, not(target_os = "macos"))) => {
                format!("Move {name} to the trash? (y/n) ")
            }
            Ask::Delete => format!("Delete {name} for good? (y/n) "),
        }
    }
}

impl Editor {
    pub fn sidebar_on(&self) -> bool {
        self.features.on(Feature::Sidebar)
    }

    /// Makes the tree show the current project and select the active file. Cheap when neither changed.
    pub fn sync_tree(&mut self) {
        if !self.sidebar_on() || self.buffers.is_empty() {
            return;
        }
        let compact = self.features.on(Feature::CompactFolders);
        if let Some(t) = self.tree.as_mut()
            && t.compact != compact
        {
            t.compact = compact;
            t.rebuild();
        }
        let active = self.buf().doc.path.as_deref();
        if let Some(t) = &self.tree
            && (active.is_none() || t.revealed.as_deref() == active)
        {
            return;
        }
        let active = active.map(Path::to_path_buf);
        let Some(root) = self.project_root() else {
            self.tree = None;
            return;
        };
        if self.tree.as_ref().is_none_or(|t| t.root != root) {
            let mut t = Tree::new(root);
            if !compact {
                t.compact = false;
                t.rebuild();
            }
            self.tree = Some(t);
        }
        let t = self.tree.as_mut().expect("just made");
        t.revealed = active.clone();
        if let Some(p) = &active {
            t.reveal(p);
        }
    }

    /// Shows the sidebar and gives it the keys (`on`), or gives them back to the text.
    pub fn focus_sidebar(&mut self, on: bool) {
        if on && !self.sidebar_on() {
            if !self.features.available(Feature::Sidebar) {
                return self.error("the sidebar is not compiled into this build");
            }
            self.toggle_feature(Feature::Sidebar);
        }
        self.sync_tree();
        self.sidebar_focus = on && self.tree.is_some();
        if on && self.tree.is_none() {
            self.error("no project folder: open one with srw <folder> or Ctrl+Alt+P");
        }
    }

    /// A click on row `i`: a folder opens or closes, a file opens (and takes the focus unless `keep_focus`).
    pub fn tree_click(&mut self, i: usize, keep_focus: bool) {
        let Some(t) = self.tree.as_mut() else { return };
        let Some(row) = t.rows.get(i).cloned() else { return };
        t.selected = i;
        if row.dir {
            t.set_open(&row.path, !row.open);
            self.sidebar_focus = true;
        } else {
            self.sidebar_focus = keep_focus;
            self.open_from_tree(&row.path);
        }
    }

    fn open_from_tree(&mut self, path: &Path) {
        if let Err(e) = self.open(path) {
            self.error(format!("{}: {e}", path.display()));
        }
        if let Some(t) = &mut self.tree {
            t.revealed = Some(path.to_path_buf());
        }
    }

    /// Keys while the sidebar has the focus.
    pub(crate) fn tree_key(&mut self, k: KeyPress) {
        self.message = None;
        let Some(t) = self.tree.as_mut() else {
            self.sidebar_focus = false;
            return self.dispatch_key(k);
        };
        let m = k.mods;
        if t.filtering {
            match k.key {
                Key::Esc => {
                    t.filtering = false;
                    t.filter.clear();
                }
                Key::Enter => t.filtering = false,
                Key::Backspace => {
                    if t.filter.pop().is_none() {
                        t.filtering = false;
                    }
                }
                Key::Up => return t.move_by(-1),
                Key::Down => return t.move_by(1),
                _ => match k.text() {
                    Some(c) => t.filter.push(c),
                    None => return,
                },
            }
            t.rebuild();
            // the first file that matches
            if !t.filter.is_empty()
                && t.current().is_none_or(|r| r.dir || r.hl.is_empty())
                && let Some(i) = t.rows.iter().position(|r| !r.dir && !r.hl.is_empty())
            {
                t.selected = i;
            }
            t.follow = true;
            return;
        }
        let page = t.page.max(2) as isize;
        // Shift+F10: the selected row's context menu (the app opens it), the sidebar keeping the keys
        if k.key == Key::F(10) && m.shift && !m.ctrl && !m.alt {
            return self.events.push(crate::Event::App(crate::Command::ContextMenu));
        }
        if m.ctrl || m.alt || m.logo {
            match k.key {
                Key::Char('d') if m.ctrl && !m.shift => t.move_by(page / 2),
                Key::Char('u') if m.ctrl && !m.shift => t.move_by(-page / 2),
                Key::Char('n' | 'j') if m.ctrl && !m.shift => t.move_by(1),
                Key::Char('p' | 'k') if m.ctrl && !m.shift => t.move_by(-1),
                Key::Char('c' | '[') if m.ctrl => self.sidebar_focus = false,
                _ => {
                    // every other chord (Ctrl+P, Ctrl+Shift+F, ...) works as in the text, which then has the keys
                    if let Some(cmd) = self.binding(k) {
                        use crate::Command::{FocusSidebar, RefreshSidebar, ToggleSidebar};
                        if !matches!(cmd, FocusSidebar | ToggleSidebar | RefreshSidebar) {
                            self.sidebar_focus = false;
                        }
                        self.run(cmd);
                    }
                }
            }
            return;
        }
        let row = t.current().cloned();
        match k.key {
            Key::Down | Key::Char('j') => t.move_by(1),
            Key::Up | Key::Char('k') => t.move_by(-1),
            Key::Home | Key::Char('g') => t.move_by(isize::MIN / 2),
            Key::End | Key::Char('G') => t.move_by(isize::MAX / 2),
            Key::PageDown => t.move_by(page),
            Key::PageUp => t.move_by(-page),
            Key::Left | Key::Char('h') => match row {
                Some(r) if r.dir && r.open => t.set_open(&r.path, false),
                Some(r) => {
                    // to the parent folder
                    if let Some(i) = t.rows[..t.selected].iter().rposition(|p| p.depth + 1 == r.depth) {
                        t.selected = i;
                        t.follow = true;
                    }
                }
                None => {}
            },
            Key::Right | Key::Char('l') => match row {
                Some(r) if r.dir && !r.open => t.set_open(&r.path, true),
                Some(r) if r.dir => t.move_by(1),
                Some(r) => self.open_from_tree(&r.path),
                None => {}
            },
            Key::Enter | Key::Char('o') => match row {
                Some(r) if r.dir => t.set_open(&r.path, !r.open),
                Some(r) => {
                    self.sidebar_focus = false;
                    self.open_from_tree(&r.path);
                }
                None => {}
            },
            // a look at the file, the focus stays here
            Key::Char(' ') => match row {
                Some(r) if r.dir => t.set_open(&r.path, !r.open),
                Some(r) => self.open_from_tree(&r.path),
                None => {}
            },
            Key::Char('a') => self.tree_ask(Ask::NewFile),
            Key::Char('A') => self.tree_ask(Ask::NewFolder),
            Key::Char('r') | Key::F(2) => self.tree_ask(Ask::Rename),
            Key::Char('d') | Key::Delete => self.tree_ask(Ask::Delete),
            Key::Char('R') | Key::F(5) => {
                t.refresh();
                self.info("sidebar refreshed");
            }
            Key::Char('/') => {
                t.filtering = true;
                t.filter.clear();
            }
            Key::Char('q') => {
                self.sidebar_focus = false;
                self.toggle_feature(Feature::Sidebar);
            }
            Key::Char(':') => self.open_prompt(PromptKind::Command),
            Key::Esc => {
                if t.filter.is_empty() {
                    self.sidebar_focus = false;
                } else {
                    t.filter.clear();
                    t.rebuild();
                }
            }
            Key::Tab => self.sidebar_focus = false,
            _ => {}
        }
    }

    /// Asks on the command line for what `ask` needs, for the selected row (new entries: in its folder).
    pub fn tree_ask(&mut self, ask: Ask) {
        self.tree_ask_in(ask, None);
    }

    /// [`Editor::tree_ask`]; a new file or folder goes into `dir` when given (the tree's empty part: the root).
    pub fn tree_ask_in(&mut self, ask: Ask, dir: Option<PathBuf>) {
        let Some(t) = self.tree.as_mut() else { return };
        let row = t.current().cloned();
        let target = match ask {
            Ask::NewFile | Ask::NewFolder => dir.unwrap_or_else(|| t.target_dir()),
            Ask::Rename | Ask::Delete | Ask::Duplicate => match row {
                Some(r) => r.path,
                None => return,
            },
        };
        let name = target.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let is_dir = target.is_dir();
        let shown = match ask {
            Ask::NewFile | Ask::NewFolder => String::new(),
            _ => name.clone(),
        };
        let mut label = ask.label(&shown);
        if matches!(ask, Ask::NewFile | Ask::NewFolder) {
            // where it goes
            let rel = target.strip_prefix(&t.root).unwrap_or(&target).display().to_string();
            if !rel.is_empty() {
                label = format!("{label}{rel}/");
            }
        }
        t.target = Some((ask, target));
        self.open_prompt(PromptKind::Ask(ask));
        if let Some(p) = self.prompt.as_mut() {
            p.label = label;
            if matches!(ask, Ask::Rename | Ask::Duplicate) {
                // the name is there to edit, the caret before its extension ("main copy.rs" for a duplicate)
                let dot = name.rfind('.').filter(|&i| i > 0 && !is_dir);
                let text = match (ask, dot) {
                    (Ask::Duplicate, Some(i)) => format!("{} copy{}", &name[..i], &name[i..]),
                    (Ask::Duplicate, None) => format!("{name} copy"),
                    _ => name.clone(),
                };
                p.cursor = match dot {
                    Some(i) => text.chars().count() - name[i..].chars().count(),
                    None => text.chars().count(),
                };
                p.text = text;
            }
        }
    }

    /// The answer to a sidebar prompt (Enter; for Delete, a `y`).
    pub(crate) fn tree_answer(&mut self, ask: Ask, text: &str) {
        let Some(t) = self.tree.as_mut() else { return };
        let Some((a, target)) = t.target.take().filter(|(a, _)| *a == ask) else { return };
        let _ = a;
        let text = text.trim();
        let result: Result<PathBuf, String> = match ask {
            Ask::Delete => {
                let r = remove(&target).map_err(|e| e.to_string());
                if let Some(parent) = target.parent() {
                    t.changed(parent);
                }
                t.rebuild();
                match r {
                    Ok(()) => {
                        let name = target.file_name().unwrap_or_default().to_string_lossy();
                        let how =
                            if cfg!(all(unix, not(target_os = "macos"))) { "moved to the trash" } else { "deleted" };
                        return self.info(format!("{name} {how}"));
                    }
                    Err(e) => return self.error(format!("{}: {e}", target.display())),
                }
            }
            _ if text.is_empty() => return,
            Ask::NewFile | Ask::NewFolder => {
                let path = crate::editor::normalize(&target.join(text));
                if path.exists() {
                    Err(format!("{} already exists", path.display()))
                } else if ask == Ask::NewFolder {
                    std::fs::create_dir_all(&path).map(|_| path).map_err(|e| e.to_string())
                } else {
                    let made = path.parent().map_or(Ok(()), std::fs::create_dir_all);
                    made.and_then(|_| std::fs::File::create_new(&path)).map(|_| path).map_err(|e| e.to_string())
                }
            }
            Ask::Rename => {
                let to = crate::editor::normalize(&target.parent().unwrap_or(&target).join(text));
                if to == target {
                    return;
                }
                if to.exists() {
                    Err(format!("{} already exists", to.display()))
                } else {
                    let made = to.parent().map_or(Ok(()), std::fs::create_dir_all);
                    made.and_then(|_| std::fs::rename(&target, &to)).map(|_| to).map_err(|e| e.to_string())
                }
            }
            Ask::Duplicate => {
                let to = crate::editor::normalize(&target.parent().unwrap_or(&target).join(text));
                if to.exists() {
                    Err(format!("{} already exists", to.display()))
                } else if to.starts_with(&target) {
                    Err("cannot copy a folder into itself".into())
                } else {
                    let made = to.parent().map_or(Ok(()), std::fs::create_dir_all);
                    made.and_then(|_| copy_all(&target, &to)).map(|_| to).map_err(|e| e.to_string())
                }
            }
        };
        let path = match result {
            Ok(p) => p,
            Err(e) => return self.error(e),
        };
        // open buffers follow a rename (of the file, or of a folder above it)
        if ask == Ask::Rename {
            for b in &mut self.buffers {
                if let Some(p) = &b.doc.path
                    && let Ok(rest) = p.strip_prefix(&target)
                {
                    b.doc.path = Some(if rest.as_os_str().is_empty() { path.clone() } else { path.join(rest) });
                }
            }
            if let Some(parent) = target.parent() {
                t.changed(parent);
            }
        }
        if let Some(parent) = path.parent() {
            t.changed(parent);
        }
        t.rebuild();
        t.select_path(&path);
        if ask == Ask::Duplicate {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            self.info(format!("copied to {name}"));
        }
        if ask == Ask::NewFile {
            self.sidebar_focus = false;
            self.open_from_tree(&path);
            if self.vim() {
                self.set_mode(Mode::Normal);
            }
        }
    }
}

/// Copies a file, or a folder with everything in it. A symlinked file is copied as its content; a symlinked folder
/// inside is left out (it could loop).
fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.is_dir() {
        return std::fs::copy(from, to).map(|_| ());
    }
    std::fs::create_dir(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let path = e.path();
        if e.file_type()?.is_symlink() && path.is_dir() {
            continue;
        }
        copy_all(&path, &to.join(e.file_name()))?;
    }
    Ok(())
}

/// Moves `path` to the trash (Linux and the BSDs, the freedesktop.org way), or deletes it (elsewhere).
fn remove(path: &Path) -> std::io::Result<()> {
    if cfg!(all(unix, not(target_os = "macos"))) {
        return trash(path);
    }
    if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
}

/// The freedesktop.org trash: the file goes to `$XDG_DATA_HOME/Trash/files` with a `.trashinfo` beside it, so a
/// file manager can put it back. Only within one file system (a rename); elsewhere it fails and says so.
fn trash(path: &Path) -> std::io::Result<()> {
    use std::io::{Error, Write};
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| crate::project::home().map(|h| h.join(".local/share")))
        .ok_or_else(|| Error::other("no home folder for the trash"))?;
    let trash = data.join("Trash");
    std::fs::create_dir_all(trash.join("files"))?;
    std::fs::create_dir_all(trash.join("info"))?;
    let name = path.file_name().ok_or_else(|| Error::other("nothing to trash"))?.to_string_lossy().into_owned();
    let abs = std::path::absolute(path)?;
    for n in 1..1000 {
        let file = if n == 1 { name.clone() } else { format!("{name}.{n}") };
        let info_path = trash.join("info").join(format!("{file}.trashinfo"));
        // claiming the info file first makes the name ours
        let Ok(mut info) = std::fs::OpenOptions::new().write(true).create_new(true).open(&info_path) else { continue };
        if trash.join("files").join(&file).exists() {
            let _ = std::fs::remove_file(&info_path);
            continue;
        }
        writeln!(info, "[Trash Info]\nPath={}\nDeletionDate={}", percent_encode(&abs), utc_now())?;
        return match std::fs::rename(path, trash.join("files").join(&file)) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&info_path);
                Err(Error::other(format!("cannot move it to the trash ({e}); delete it from a file manager")))
            }
        };
    }
    Err(Error::other("the trash is full of files with this name"))
}

fn percent_encode(p: &Path) -> String {
    let mut out = String::new();
    for &b in p.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    out
}

/// `YYYY-MM-DDThh:mm:ss` in UTC (the spec wants local time; file managers show either fine).
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // days to a civil date (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shrew-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(t: &Tree) -> Vec<String> {
        t.rows.iter().map(|r| format!("{}{}{}", "  ".repeat(r.depth), r.name, if r.dir { "/" } else { "" })).collect()
    }

    #[test]
    fn lazy_tree_with_folders_first() {
        let d = temp("lazy");
        std::fs::create_dir_all(d.join("src/deep")).unwrap();
        std::fs::create_dir_all(d.join(".git")).unwrap();
        for f in ["b.txt", "a10.txt", "a2.txt", "src/main.rs", "src/deep/x.rs"] {
            std::fs::write(d.join(f), "").unwrap();
        }
        let mut t = Tree::new(d.clone());
        assert_eq!(names(&t), ["src/", "a2.txt", "a10.txt", "b.txt"], ".git hidden, natural order");
        assert!(!t.listings.contains_key(&d.join("src")), "nothing read before it is opened");
        t.set_open(&d.join("src"), true);
        assert_eq!(names(&t), ["src/", "  deep/", "  main.rs", "a2.txt", "a10.txt", "b.txt"]);
        assert!(t.reveal(&d.join("src/deep/x.rs")));
        assert_eq!(t.current().unwrap().name, "x.rs");
        t.filter = "a1".into();
        t.rebuild();
        assert_eq!(names(&t), ["a10.txt"]);
        t.filter = "deep".into();
        t.rebuild();
        assert_eq!(names(&t), ["src/", "  deep/"], "matches keep their folders");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn single_folder_chains_share_a_row() {
        let d = temp("compact");
        // a/ and b/ hold files; c/ holds only d/, which holds only e/
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::create_dir_all(d.join("b")).unwrap();
        std::fs::create_dir_all(d.join("c/d/e")).unwrap();
        for f in ["a/x.rs", "b/y.rs", "c/d/e/one.rs", "c/d/e/two.rs"] {
            std::fs::write(d.join(f), "").unwrap();
        }
        let mut t = Tree::new(d.clone());
        assert_eq!(names(&t), ["a/", "b/", "c/d/e/"]);
        assert_eq!(t.rows[2].path, d.join("c/d/e"), "the row stands for the deepest folder");
        t.set_open(&d.join("c/d/e"), true);
        assert_eq!(names(&t), ["a/", "b/", "c/d/e/", "  one.rs", "  two.rs"]);
        assert!(t.reveal(&d.join("c/d/e/two.rs")));
        assert_eq!(t.current().unwrap().name, "two.rs");
        // a second entry in c/d splits the chain there
        std::fs::write(d.join("c/d/note.md"), "").unwrap();
        t.refresh();
        assert_eq!(names(&t), ["a/", "b/", "c/d/", "  e/", "    one.rs", "    two.rs", "  note.md"]);
        // the filter matches the joined name
        t.filter = "c/d".into();
        t.rebuild();
        assert_eq!(names(&t)[0], "c/d/");
        // off: one row per folder
        t.filter.clear();
        t.compact = false;
        t.rebuild();
        assert_eq!(names(&t)[..3], ["a/", "b/", "c/"]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(feature = "project")]
    #[test]
    fn gitignored_entries_are_marked() {
        let d = temp("ignored");
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::create_dir_all(d.join("target")).unwrap();
        std::fs::write(d.join(".gitignore"), "target/\n*.log\n").unwrap();
        std::fs::write(d.join("a.log"), "").unwrap();
        std::fs::write(d.join("a.rs"), "").unwrap();
        let t = Tree::new(d.clone());
        let ignored: Vec<(&str, bool)> = t.rows.iter().map(|r| (r.name.as_str(), r.ignored)).collect();
        assert_eq!(ignored, [("target", true), (".gitignore", false), ("a.log", true), ("a.rs", false)]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn file_operations_from_the_sidebar() {
        let d = temp("ops");
        std::fs::write(d.join(".shrew"), "").unwrap();
        std::fs::write(d.join("old.txt"), "hi").unwrap();
        let mut ed = Editor::new();
        ed.project = Some(d.clone());
        ed.features.set(Feature::Sidebar, true);
        ed.focus_sidebar(true);
        assert!(ed.sidebar_focus);
        let keys = |ed: &mut Editor, s: &str| {
            for k in KeyPress::parse_seq(s) {
                ed.handle_key(k);
            }
        };
        // a new file in a new folder opens in the editor
        keys(&mut ed, "asub/new.rs<CR>");
        assert!(d.join("sub/new.rs").is_file());
        assert_eq!(ed.buf().doc.path.as_deref(), Some(d.join("sub/new.rs").as_path()));
        assert!(!ed.sidebar_focus);
        // rename the old file
        ed.focus_sidebar(true);
        let t = ed.tree.as_mut().unwrap();
        assert!(t.select_path(&d.join("old.txt")));
        keys(&mut ed, "r<End><C-u>renamed.txt<CR>");
        assert!(d.join("renamed.txt").is_file() && !d.join("old.txt").exists(), "{:?}", ed.message);
        assert_eq!(ed.tree.as_ref().unwrap().current().unwrap().name, "renamed.txt");
        // anything but y keeps it
        keys(&mut ed, "dn");
        assert!(d.join("renamed.txt").exists());
        assert!(ed.prompt.is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn trash_info_parts() {
        assert_eq!(percent_encode(Path::new("/a b/ü.txt")), "/a%20b/%C3%BC.txt");
        let now = utc_now();
        assert_eq!(now.len(), 19);
        assert!(now.starts_with("20"));
    }
}
