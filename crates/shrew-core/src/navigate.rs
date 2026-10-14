//! Project navigation on the [`Editor`]: the project root, the jump list (Ctrl+O / Ctrl+I), `gf`, find in project,
//! go to symbol, recent files and projects, and reopening closed files. The sidebar's tree is in [`crate::tree`].

use crate::editor::{Editor, Mode};
use crate::features::Feature;
use crate::grep::{GrepField, GrepState};
use crate::input::{Key, KeyPress};
use crate::jumps::Jump;
use crate::picker::{Cancel, Picker, PickerData, PickerItem, PickerKind};
use crate::project;
use crate::text;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

/// What a [`SymbolProvider`] is asked for.
pub enum SymbolScope {
    /// One buffer's text; `path` (its file) picks the grammar.
    File { path: Option<PathBuf>, text: ropey::Rope },
    /// Every file of the project under `root`.
    Project { root: PathBuf },
}

/// Finds symbols on a background thread and streams them back as picker items (with [`PickerData::Location`]).
/// The flag is set when the picker closes. The app installs one when syntax highlighting is compiled in: the
/// grammars know what a symbol is.
pub type SymbolProvider = Arc<dyn Fn(SymbolScope, Arc<AtomicBool>) -> Receiver<Vec<PickerItem>> + Send + Sync>;

/// Files closed this session that Ctrl+Shift+T can bring back.
const MAX_CLOSED: usize = 50;

impl Editor {
    /// The project folder for listing, searching and the sidebar: the one chosen explicitly (`srw <folder>`, `:cd`,
    /// Open Recent Project) while the active file is inside it, else the root of the active file (see
    /// [`project::find_root`]), else of the working directory. `None` when that would be `/` or the home folder.
    pub fn project_root(&self) -> Option<PathBuf> {
        let file = self.buffers.get(self.active).and_then(|b| b.doc.path.as_deref());
        if let Some(p) = &self.project
            && file.is_none_or(|f| f.starts_with(p))
        {
            return Some(p.clone());
        }
        let start = file.map_or_else(|| self.cwd.clone(), Path::to_path_buf);
        let mut cache = self.root_cache.lock().unwrap_or_else(|e| e.into_inner());
        let key = if start.is_dir() {
            start.clone()
        } else {
            start.parent().map_or_else(|| start.clone(), Path::to_path_buf)
        };
        if let Some((k, root)) = cache.as_ref()
            && *k == key
        {
            return root.clone();
        }
        let root = project::find_root(&start);
        *cache = Some((key, root.clone()));
        root
    }

    /// Where project tools start (a terminal, a new file): the project root, else the active file's folder, else
    /// the working directory.
    pub fn work_dir(&self) -> PathBuf {
        self.project_root()
            .or_else(|| self.buf().doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf))
            .unwrap_or_else(|| self.cwd.clone())
    }

    /// Makes `dir` the project (and the working directory).
    pub fn set_project(&mut self, dir: &Path) {
        let dir = self.absolute(dir);
        self.cwd = dir.clone();
        if self.features.on(Feature::Recent) {
            self.recent.add_project(&dir);
        }
        self.project = Some(dir);
        self.tree = None;
        self.sync_tree();
    }

    /// `srw <folder>`, Open Recent Project: the folder becomes the project, shown in the sidebar (which takes the
    /// focus), or in the file picker when the sidebar is not compiled in.
    pub fn open_project(&mut self, dir: &Path) {
        self.set_project(dir);
        if self.features.available(Feature::Sidebar) {
            self.features.set(Feature::Sidebar, true);
            self.focus_sidebar(true);
        } else {
            self.run(crate::Command::OpenFile);
        }
    }

    /// Notes an opened file in the recent lists.
    pub(crate) fn remember_file(&mut self, path: &Path) {
        if !self.features.on(Feature::Recent) {
            return;
        }
        self.recent.add_file(path);
        if let Some(root) = project::find_root(path) {
            self.recent.add_project(&root);
        }
    }

    pub(crate) fn remember_closed(&mut self) {
        let b = self.buf();
        if let Some(p) = &b.doc.path {
            let line = text::line_of(b.rope(), b.sel.primary().head.min(b.doc.len()));
            let p = p.clone();
            self.closed.retain(|(c, _)| *c != p);
            self.closed.push((p, line));
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
    }

    pub(crate) fn reopen_closed(&mut self) {
        while let Some((path, line)) = self.closed.pop() {
            if self.buffers.iter().any(|b| b.doc.path.as_ref() == Some(&path)) {
                continue;
            }
            match self.open(&path) {
                Ok(()) => {
                    if self.buf().doc.path.as_ref() == Some(&path) {
                        self.goto_line(line + 1);
                    }
                }
                Err(e) => self.error(format!("{}: {e}", path.display())),
            }
            return;
        }
        self.info("no closed files to reopen");
    }

    // ----- jump list -----

    fn here(&self) -> Jump {
        let b = self.buf();
        let rope = b.rope();
        let head = b.sel.primary().head.min(rope.len_chars());
        let line = text::line_of(rope, head);
        Jump { buffer: b.id, path: b.doc.path.clone(), line, col: head - text::line_start(rope, line) }
    }

    /// Records where a jump starts (and sets the `'` mark there, for `''`).
    pub(crate) fn push_jump(&mut self) {
        if self.jumping || self.buffers.is_empty() {
            return;
        }
        let j = self.here();
        let pos = self.buf().sel.primary().head;
        self.marks.insert((j.buffer, '\''), pos);
        self.marks.insert((j.buffer, '`'), pos);
        self.jumps.push(j);
    }

    /// Ctrl+O (`newer` false) and Ctrl+I.
    pub fn jump(&mut self, newer: bool) {
        let here = self.here();
        let target = if newer { self.jumps.forward() } else { self.jumps.back(here) };
        let Some(j) = target else { return };
        self.jumping = true;
        if let Some(i) = self.buffers.iter().position(|b| b.id == j.buffer) {
            self.switch_to(i);
        } else if let Some(p) = &j.path
            && let Err(e) = self.open(p)
        {
            self.error(format!("{}: {e}", p.display()));
        }
        self.jumping = false;
        if self.buf().id != j.buffer && self.buf().doc.path != j.path {
            return;
        }
        let caret = self.mode == Mode::Insert || !self.vim();
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        let line = j.line.min(text::last_line(rope));
        let pos = text::line_start(rope, line) + j.col.min(text::line_len(rope, line));
        let pos = if caret { pos } else { text::clamp_normal(rope, pos) };
        b.sel = crate::Selection::point(pos);
        let (top, rows, cur) = (b.view.top, b.view.rows, b.cursor_line());
        if cur < top || cur >= top + rows.max(1) {
            b.center_on_cursor();
        }
    }

    /// Goes to a 0-based place in a file (`None`: the active buffer), as pickers do.
    pub(crate) fn goto_location(&mut self, path: Option<&Path>, line: usize, col: usize) {
        match path {
            Some(p) if self.buf().doc.path.as_deref() != Some(p) => {
                if let Err(e) = self.open(p) {
                    return self.error(format!("{}: {e}", p.display()));
                }
                if self.buf().doc.path.as_deref() != Some(p) {
                    return; // a big file loading in the background
                }
            }
            _ => self.push_jump(),
        }
        self.goto_line_col(line + 1, Some(col + 1));
    }

    /// `gf`: opens the file named under the cursor. `path:line[:col]` goes to that line; relative names are tried
    /// against the file's folder, the project root and the working directory.
    pub fn goto_file_under_cursor(&mut self) {
        let b = self.buf();
        let rope = b.rope();
        let head = b.sel.primary().head.min(rope.len_chars());
        let line = text::line_of(rope, head);
        let text: Vec<char> = rope.line(line).chars().collect();
        let at = head - text::line_start(rope, line);
        let is_path = |c: char| c.is_alphanumeric() || "_-./~+@%:\\".contains(c);
        if !text.get(at).is_some_and(|&c| is_path(c)) {
            return self.error("no file name under the cursor");
        }
        let mut a = at;
        while a > 0 && is_path(text[a - 1]) {
            a -= 1;
        }
        let mut z = at;
        while z < text.len() && is_path(text[z]) {
            z += 1;
        }
        let word: String = text[a..z].iter().collect();
        let (name, line_no, col) = split_location(word.trim_end_matches(['.', ',', ':', ';']));
        let dir = b.doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf);
        let mut candidates = Vec::new();
        let p = Path::new(name);
        if p.is_absolute() || name.starts_with("~/") {
            candidates.push(self.absolute(p));
        } else {
            candidates.extend(dir.map(|d| d.join(p)));
            candidates.extend(self.project_root().map(|r| r.join(p)));
            candidates.push(self.cwd.join(p));
        }
        let Some(found) = candidates.into_iter().map(|c| self.absolute(&c)).find(|c| c.is_file()) else {
            return self.error(format!("no file {name}"));
        };
        match self.open(&found) {
            Ok(()) if self.buf().doc.path.as_deref() == Some(found.as_path()) => {
                if let Some(n) = line_no {
                    self.goto_line_col(n, col);
                }
            }
            Ok(()) => {}
            Err(e) => self.error(format!("{}: {e}", found.display())),
        }
    }

    // ----- find in project -----

    /// Opens find in project with `pattern` (the selection when empty and it is on one line, else the last one).
    pub fn open_grep(&mut self, pattern: &str) {
        if !self.features.on(Feature::ProjectSearch) {
            return self.error("find in project is off (:set project_search)");
        }
        if !cfg!(feature = "project") {
            return self.error("find in project is not compiled into this build");
        }
        let Some(root) = self.project_root() else {
            return self.error("no project folder: open one with srw <folder> or Ctrl+Alt+P");
        };
        let pattern = if pattern.is_empty() {
            self.selected_text().unwrap_or(self.grep_last.0.clone())
        } else {
            pattern.to_string()
        };
        let mut p = Picker::new(PickerKind::Grep, Vec::new());
        let mut g = GrepState::default();
        (g.root, g.opts) = (root, self.grep_last.1.clone());
        p.grep = Some(g);
        p.query = pattern;
        self.picker = Some(p);
        if self.mode.is_visual() {
            self.set_mode(Mode::Normal);
        }
        self.restart_grep();
    }

    /// The primary selection's text, when it is on one line.
    fn selected_text(&self) -> Option<String> {
        let b = self.buf();
        let r = b.sel.primary();
        let visual = self.mode.is_visual();
        if r.is_empty() && !visual {
            return None;
        }
        let z = if visual { text::grapheme_end(b.rope(), r.to()) } else { r.to() };
        let s: String = b.rope().slice(r.from()..z.min(b.doc.len())).chars().collect();
        (!s.is_empty() && !s.contains('\n')).then_some(s)
    }

    /// Starts the search again for the picker's current pattern and options.
    pub(crate) fn restart_grep(&mut self) {
        let Some(p) = self.picker.as_mut().filter(|p| p.kind == PickerKind::Grep) else { return };
        let Some(g) = p.grep.as_mut() else { return };
        g.cancel.store(true, Ordering::Release);
        g.cancel = Arc::new(AtomicBool::new(false));
        g.stats = Default::default();
        g.error = None;
        self.grep_last = (p.query.clone(), g.opts.clone());
        if p.query.is_empty() {
            return p.restart(None);
        }
        #[cfg(feature = "project")]
        {
            let re = crate::grep::build_regex(&p.query, &g.opts)
                .and_then(|re| project::check_globs(&g.root, &g.opts.globs).map(|_| re));
            match re {
                Ok(re) => {
                    let rx = crate::grep::spawn(
                        &g.root,
                        re,
                        &g.opts.globs,
                        g.stats.clone(),
                        g.cancel.clone(),
                        self.waker.clone(),
                    );
                    p.restart(Some(rx));
                }
                Err(e) => {
                    g.error = Some(e);
                    p.restart(None);
                }
            }
        }
    }

    /// Find in project's own keys: Alt+R regex, Alt+C case, Alt+W whole word, Tab to the globs and back. True when
    /// the key was one of them.
    pub(crate) fn grep_key(&mut self, k: KeyPress) -> bool {
        let Some(g) = self.picker.as_mut().and_then(|p| p.grep.as_mut()) else { return false };
        let m = k.mods;
        match k.key {
            Key::Char('r') if m.alt => g.opts.regex ^= true,
            Key::Char('c') if m.alt => g.opts.case ^= true,
            Key::Char('w') if m.alt => g.opts.word ^= true,
            Key::Tab => {
                g.field = if g.field == GrepField::Pattern { GrepField::Globs } else { GrepField::Pattern };
                return true;
            }
            _ if g.field == GrepField::Globs => match k.key {
                Key::Backspace if g.opts.globs.is_empty() => {
                    g.field = GrepField::Pattern;
                    return true;
                }
                Key::Backspace => {
                    g.opts.globs.pop();
                }
                Key::Char('u' | 'w') if m.ctrl => g.opts.globs.clear(),
                Key::Enter => {
                    g.field = GrepField::Pattern;
                    return true;
                }
                _ => match k.text() {
                    Some(c) => g.opts.globs.push(c),
                    None => return false,
                },
            },
            _ => return false,
        }
        self.restart_grep();
        true
    }

    // ----- symbols -----

    /// Go to symbol: in the active file (Ctrl+R, `@` in Ctrl+P) or the whole project (Ctrl+Shift+R).
    pub fn open_symbols(&mut self, project: bool) {
        if !self.features.on(Feature::Symbols) {
            return self.error("symbols are off (:set symbols)");
        }
        let Some(provider) = self.symbols.clone() else {
            return self.error("symbols need syntax highlighting compiled into this build");
        };
        let scope = if project {
            let Some(root) = self.project_root() else {
                return self.error("no project folder: open one with srw <folder> or Ctrl+Alt+P");
            };
            SymbolScope::Project { root }
        } else {
            let doc = &self.buf().doc;
            SymbolScope::File { path: doc.path.clone(), text: doc.rope.clone() }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = provider(scope, cancel.clone());
        let mut p = Picker::streaming(PickerKind::Symbols { project }, rx);
        p.cancel = Some(Cancel(cancel));
        self.picker = Some(p);
    }

    /// After the picker's query changed: Ctrl+P's `@` switches to the file's symbols and back, find in project
    /// searches again.
    pub(crate) fn picker_query_changed(&mut self) {
        let Some(p) = self.picker.as_mut() else { return };
        match p.kind {
            PickerKind::Files if p.query.starts_with('@') => {
                let files = self.picker.take().expect("checked");
                let query = files.query.clone();
                self.open_symbols(false);
                match self.picker.as_mut() {
                    Some(s) => {
                        s.query = query;
                        s.prefix = 1;
                        s.parked = Some(Box::new(files));
                        s.refilter();
                    }
                    None => self.picker = Some(files),
                }
            }
            PickerKind::Symbols { .. } if p.prefix == 1 && !p.query.starts_with('@') => {
                if let Some(mut files) = p.parked.take() {
                    files.query = std::mem::take(&mut p.query);
                    files.refilter();
                    self.picker = Some(*files);
                }
            }
            PickerKind::Grep => self.restart_grep(),
            _ => {}
        }
    }

    /// What Enter does on a picker row this module added. True when it was one.
    pub(crate) fn pick_location(&mut self, data: &PickerData) -> bool {
        match data {
            PickerData::Location { path, line, col, .. } => self.goto_location(path.as_deref(), *line, *col),
            PickerData::Project(dir) => self.open_project(dir),
            PickerData::Header(_) => {}
            _ => return false,
        }
        true
    }

    // ----- recent -----

    /// File: Open Recent… (also Ctrl+P when there is no project to list).
    pub fn open_recent_files(&mut self) {
        let home = project::home();
        let items: Vec<PickerItem> = self
            .recent
            .files
            .iter()
            .filter(|p| p.is_file())
            .map(|p| PickerItem {
                label: tilde(p, home.as_deref()),
                detail: String::new(),
                data: PickerData::Path(p.clone()),
            })
            .collect();
        self.picker = Some(Picker::new(PickerKind::Files, items));
    }

    /// Project: Open Recent… (Ctrl+Alt+P).
    pub fn open_recent_projects(&mut self) {
        let home = project::home();
        let items: Vec<PickerItem> = self
            .recent
            .projects
            .iter()
            .filter(|p| p.is_dir())
            .map(|p| PickerItem {
                label: p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned()),
                detail: tilde(p, home.as_deref()),
                data: PickerData::Project(p.clone()),
            })
            .collect();
        if items.is_empty() {
            return self.info("no recent projects yet: open a folder with srw <folder>");
        }
        self.picker = Some(Picker::new(PickerKind::Projects, items));
    }
}

/// `path:line[:col]` -> (path, line, col).
fn split_location(s: &str) -> (&str, Option<usize>, Option<usize>) {
    let num = |t: &str| t.parse::<usize>().ok().filter(|n| *n > 0);
    if let Some((rest, c)) = s.rsplit_once(':')
        && let Some(c) = num(c)
    {
        if let Some((name, l)) = rest.rsplit_once(':')
            && let Some(l) = num(l)
        {
            return (name, Some(l), Some(c));
        }
        return (rest, Some(c), None);
    }
    (s, None, None)
}

/// `path` with the home folder written `~`.
fn tilde(p: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyPress;

    fn keys(ed: &mut Editor, s: &str) {
        for k in KeyPress::parse_seq(s) {
            ed.handle_key(k);
        }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shrew-nav-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".shrew")).unwrap();
        d
    }

    fn line(ed: &Editor) -> usize {
        text::line_of(ed.buf().rope(), ed.buf().sel.primary().head)
    }

    #[test]
    fn locations() {
        assert_eq!(split_location("src/a.rs:12:5"), ("src/a.rs", Some(12), Some(5)));
        assert_eq!(split_location("src/a.rs:12"), ("src/a.rs", Some(12), None));
        assert_eq!(split_location("a.rs"), ("a.rs", None, None));
        assert_eq!(tilde(Path::new("/h/u/x"), Some(Path::new("/h/u"))), "~/x");
    }

    #[test]
    fn jump_list_across_moves_and_files() {
        let d = temp("jumps");
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(d.join("a.txt"), &text).unwrap();
        std::fs::write(d.join("b.txt"), "b\n").unwrap();
        let mut ed = Editor::new();
        ed.open(&d.join("a.txt")).unwrap();
        keys(&mut ed, "G");
        assert_eq!(line(&ed), 100);
        keys(&mut ed, "<C-o>");
        assert_eq!(line(&ed), 0, "back to where G started");
        keys(&mut ed, "<C-i>");
        assert_eq!(line(&ed), 100);
        // '' toggles between the last two places
        keys(&mut ed, ":50<CR>");
        assert_eq!(line(&ed), 49);
        keys(&mut ed, "''");
        assert_eq!(line(&ed), 100);
        keys(&mut ed, "''");
        assert_eq!(line(&ed), 49);
        // another file, and back
        ed.open(&d.join("b.txt")).unwrap();
        keys(&mut ed, "<C-o>");
        assert_eq!(ed.buf().doc.path.as_deref(), Some(d.join("a.txt").as_path()));
        assert_eq!(line(&ed), 49);
        keys(&mut ed, "<Tab>");
        assert_eq!(ed.buf().doc.path.as_deref(), Some(d.join("b.txt").as_path()), "Tab is Ctrl+I");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn gf_opens_path_at_line() {
        let d = temp("gf");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/b.rs"), "1\n2\n3\n4\n").unwrap();
        std::fs::write(d.join("notes.md"), "see src/b.rs:3 for it\n").unwrap();
        let mut ed = Editor::new();
        ed.open(&d.join("notes.md")).unwrap();
        keys(&mut ed, "fsgf");
        assert_eq!(ed.buf().doc.path.as_deref(), Some(d.join("src/b.rs").as_path()));
        assert_eq!(line(&ed), 2);
        keys(&mut ed, "<C-o>");
        assert_eq!(ed.buf().doc.path.as_deref(), Some(d.join("notes.md").as_path()));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn project_root_follows_the_active_file() {
        let d = temp("root");
        std::fs::create_dir_all(d.join("other/.git")).unwrap();
        std::fs::write(d.join("a.txt"), "").unwrap();
        std::fs::write(d.join("other/b.txt"), "").unwrap();
        let mut ed = Editor::new();
        ed.open(&d.join("a.txt")).unwrap();
        assert_eq!(ed.project_root(), Some(d.clone()));
        ed.open(&d.join("other/b.txt")).unwrap();
        assert_eq!(ed.project_root(), Some(d.join("other")), "a repo inside is its own project");
        ed.set_project(&d);
        assert_eq!(ed.project_root(), Some(d.clone()), "an explicit project wins for files inside it");
        assert_eq!(ed.recent.projects.first(), Some(&d));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn reopen_closed_files() {
        let d = temp("reopen");
        std::fs::write(d.join("a.txt"), "1\n2\n3\n").unwrap();
        let mut ed = Editor::new();
        ed.open(&d.join("a.txt")).unwrap();
        keys(&mut ed, "2G:bd<CR>");
        assert!(ed.buffers.iter().all(|b| b.doc.path.is_none()));
        keys(&mut ed, "<C-S-t>");
        assert_eq!(ed.buf().doc.path.as_deref(), Some(d.join("a.txt").as_path()));
        assert_eq!(line(&ed), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(feature = "project")]
    #[test]
    fn find_in_project_picker() {
        let d = temp("grep");
        std::fs::write(d.join("a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        std::fs::write(d.join("b.rs"), "// alpha again\n").unwrap();
        let mut ed = Editor::new();
        ed.open(&d.join("b.rs")).unwrap();
        keys(&mut ed, "<C-S-f>alpha");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while ed.picker.as_ref().is_some_and(|p| p.loading) && std::time::Instant::now() < deadline {
            ed.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let p = ed.picker.as_ref().unwrap();
        assert_eq!(p.items.len(), 4, "two headers, two matches");
        assert!(!matches!(p.current().unwrap().data, PickerData::Header(_)));
        // Enter goes to the match
        let target = match &p.current().unwrap().data {
            PickerData::Location { path, line, .. } => (path.clone().unwrap(), *line),
            _ => unreachable!(),
        };
        keys(&mut ed, "<CR>");
        assert!(ed.picker.is_none());
        assert_eq!(ed.buf().doc.path.as_ref(), Some(&target.0));
        assert_eq!(line(&ed), target.1);
        // options and globs
        keys(&mut ed, "<C-S-f><A-r>");
        assert!(ed.picker.as_ref().unwrap().grep.as_ref().unwrap().opts.regex);
        keys(&mut ed, "<Tab>b.rs");
        assert_eq!(ed.picker.as_ref().unwrap().grep.as_ref().unwrap().opts.globs, "b.rs");
        keys(&mut ed, "<Esc>");
        assert!(ed.picker.is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn ctrl_p_line_and_at_prefixes() {
        let mut ed = Editor::new();
        ed.install_document(crate::Document::from_str("a\nb\nc\nd\n"));
        ed.picker = Some(Picker::new(PickerKind::Files, Vec::new()));
        keys(&mut ed, ":3<CR>");
        assert!(ed.picker.is_none());
        assert_eq!(line(&ed), 2);
        // `@` without a symbol provider says why and keeps the file list
        ed.picker = Some(Picker::new(PickerKind::Files, Vec::new()));
        keys(&mut ed, "@");
        assert_eq!(ed.picker.as_ref().map(|p| p.kind), Some(PickerKind::Files));
        // with one, `@` swaps to symbols and Backspace swaps back
        let provider: SymbolProvider = Arc::new(|_, _| {
            let (tx, rx) = std::sync::mpsc::channel();
            let loc = PickerData::Location { path: None, line: 3, col: 0, hl: None };
            tx.send(vec![PickerItem { label: "main".into(), detail: String::new(), data: loc }]).unwrap();
            rx
        });
        ed.symbols = Some(provider);
        keys(&mut ed, "<Esc>");
        ed.picker = Some(Picker::new(PickerKind::Files, Vec::new()));
        keys(&mut ed, "@ma");
        ed.poll();
        let p = ed.picker.as_ref().unwrap();
        assert_eq!((p.kind, p.matches.len()), (PickerKind::Symbols { project: false }, 1));
        keys(&mut ed, "<BS><BS><BS>");
        assert_eq!(ed.picker.as_ref().map(|p| p.kind), Some(PickerKind::Files));
        keys(&mut ed, "@");
        ed.poll();
        keys(&mut ed, "<CR>");
        assert_eq!(line(&ed), 3);
    }
}
