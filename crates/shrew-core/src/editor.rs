//! The editor: buffers, mode, registers, prompt and picker, and the key dispatch that ties the vim and Sublime
//! layers together. Everything here is toolkit-free; the app feeds it keys and mouse positions and draws its state.

use crate::commands::Command;
use crate::document::{Buffer, Document};
use crate::features::{self, Feature, Features};
use crate::input::{Key, KeyPress, Mods};
use crate::lang;
use crate::ops::{self, Opts};
use crate::picker::{Choice, Picker, PickerData, PickerItem, PickerKind};
use crate::search;
use crate::selection::{Range, Selection};
use crate::text;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    VisualBlock,
}

impl Mode {
    pub fn is_visual(self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine | Mode::VisualBlock)
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::VisualBlock => "V-BLOCK",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

/// The system clipboard, provided by the app (a feature: without it yanks stay inside shrew).
pub trait Clipboard: Send {
    fn get(&mut self) -> Option<String>;
    fn set(&mut self, text: String);
}

#[derive(Default)]
struct MemoryClipboard(Option<String>);

impl Clipboard for MemoryClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.clone()
    }
    fn set(&mut self, text: String) {
        self.0 = Some(text);
    }
}

/// Register contents: one part per cursor. Linewise parts are whole lines without the final newline.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Register {
    pub parts: Vec<String>,
    pub linewise: bool,
    /// Yanked from a visual block: one part per line, put back as a block.
    pub blockwise: bool,
}

impl Register {
    pub fn joined(&self) -> String {
        let mut s = self.parts.join("\n");
        if self.linewise {
            s.push('\n');
        }
        s
    }
    fn from_text(t: String) -> Register {
        match t.strip_suffix('\n') {
            Some(body) => Register { parts: vec![body.to_string()], linewise: true, blockwise: false },
            None => Register { parts: vec![t], linewise: false, blockwise: false },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Command,
    Search {
        forward: bool,
    },
    /// A question from the sidebar (new file name, rename, delete).
    Ask(crate::tree::Ask),
}

pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    /// Caret as a char index into `text`.
    pub cursor: usize,
    saved_sel: Selection,
    saved_top: usize,
    history_pos: Option<usize>,
    /// Vim count for the search (`3/foo`).
    pub(crate) count: Option<usize>,
    /// Operator waiting for the search target (`d/foo<CR>`), with its register.
    pub(crate) op: Option<(crate::vim::Op, Option<char>)>,
    /// Shown before the text instead of `:` or `/` (sidebar questions).
    pub label: String,
}

/// Things only the app can do.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Quit,
    FontSize(i32),
    FontReset,
    CheckUpdates,
    InstallUpdate,
    OpenConfig,
    McpSetup,
    /// A feature was toggled; the app persists it and starts/stops what depends on it.
    FeatureToggled(Feature),
    /// A buffer was opened or its file type changed; syntax state for it should be (re)created.
    BufferOpened(u64),
    /// A file over [`Editor::async_load_bytes`] to open: the app reads it on a background thread.
    Load(PathBuf),
    /// Open the picker for a [`Choice`] (the app fills it: see [`Editor::open_choice`]).
    OpenChoice(Choice),
    /// A theme, accent or font was picked (Enter in its picker, or `:theme <id>`): apply and save it.
    Choose(Choice, String),
    /// `:theme import <path>`: convert a Sublime/TextMate color scheme into a custom theme.
    ThemeImport(String),
    /// `:set ligatures` (`Some(true)`), `:set noligatures`, or a toggle (`None`).
    Ligatures(Option<bool>),
    /// A command only the app can carry out (the window, the title bar's menus).
    App(Command),
    /// A terminal command (Ctrl+`, the palette, `:term [cmd]` as `NewTerminal` with its command line): the app owns
    /// the terminals.
    Terminal(Command, Option<String>),
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub tab_width: usize,
    pub expand_tab: bool,
    pub scrolloff: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { tab_width: 4, expand_tab: true, scrolloff: 4 }
    }
}

#[derive(Default)]
struct MouseState {
    clicks: u8,
    /// The unit (char, word, line) range the drag started on.
    origin: Option<(usize, usize)>,
    dragging: bool,
}

pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub active: usize,
    next_id: u64,
    pub mode: Mode,
    pub features: Features,
    pub settings: Settings,
    registers: HashMap<char, Register>,
    pub clipboard: Box<dyn Clipboard>,
    pub prompt: Option<Prompt>,
    pub picker: Option<Picker>,
    /// The About panel is open, scrolled down this many lines (the app clamps it to the text it draws).
    pub about: Option<usize>,
    /// Status-line message and whether it is an error.
    pub message: Option<(String, bool)>,
    pub search: Option<Regex>,
    pub search_text: String,
    pub search_visible: bool,
    pub(crate) last_search_forward: bool,
    pub events: Vec<Event>,
    pub cwd: PathBuf,
    /// Lets background work (file listing, syntax loading) wake the UI.
    pub waker: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Files bigger than this are not read by [`Editor::open`]: it emits [`Event::Load`] instead. None: always read.
    pub async_load_bytes: Option<u64>,
    // vim state
    pub(crate) pending: Vec<KeyPress>,
    pub(crate) last_find: Option<(char, bool, bool)>,
    pub(crate) dot: Vec<KeyPress>,
    pub(crate) dot_recording: Option<Vec<KeyPress>>,
    pub(crate) replaying: bool,
    pub(crate) macro_rec: Option<(char, Vec<KeyPress>)>,
    pub(crate) macros: HashMap<char, Vec<KeyPress>>,
    pub(crate) last_macro: Option<char>,
    pub(crate) marks: HashMap<(u64, char), usize>,
    pub(crate) block: Option<(usize, usize)>,
    pub(crate) last_visual: Option<(Mode, Selection)>,
    occurrence_word: bool,
    /// Insert started from visual block `I`/`A`: Esc goes back to a single cursor, like vim.
    pub(crate) block_insert: bool,
    cmd_history: Vec<String>,
    search_history: Vec<String>,
    mouse: MouseState,
    /// User mappings (`:nnoremap`, config `[keys]`, imported vimrc/Sublime bindings) and the keys held for them.
    pub keymap: crate::keymap::Keymap,
    // ----- project navigation (see `navigate` and `tree`) -----
    /// The project folder chosen explicitly (`srw <folder>`, `:cd`, Open Recent Project); see
    /// [`Editor::project_root`].
    pub project: Option<PathBuf>,
    /// The sidebar's file tree, made when the sidebar first shows.
    pub tree: Option<crate::tree::Tree>,
    /// Keys go to the sidebar.
    pub sidebar_focus: bool,
    /// Recent files and projects (the app loads and saves them).
    pub recent: crate::project::Recent,
    pub(crate) jumps: crate::jumps::JumpList,
    /// Moving along the jump list (switching buffers then records no new jump).
    pub(crate) jumping: bool,
    /// Files closed this session (path, line) for Ctrl+Shift+T.
    pub(crate) closed: Vec<(PathBuf, usize)>,
    /// Finds symbols for go to symbol; the app installs one when syntax is compiled in.
    pub symbols: Option<crate::navigate::SymbolProvider>,
    /// The last find-in-project pattern and options.
    pub(crate) grep_last: (String, crate::grep::GrepOptions),
    /// The last project root found (folder asked about, root).
    pub(crate) root_cache: std::sync::Mutex<Option<(PathBuf, Option<PathBuf>)>>,
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        let features = Features::default();
        let mut e = Editor {
            buffers: Vec::new(),
            active: 0,
            next_id: 1,
            mode: Mode::Normal,
            features,
            settings: Settings::default(),
            registers: HashMap::new(),
            clipboard: Box::new(MemoryClipboard::default()),
            prompt: None,
            picker: None,
            about: None,
            message: None,
            search: None,
            search_text: String::new(),
            search_visible: true,
            last_search_forward: true,
            events: Vec::new(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            waker: None,
            async_load_bytes: None,
            pending: Vec::new(),
            last_find: None,
            dot: Vec::new(),
            dot_recording: None,
            replaying: false,
            macro_rec: None,
            macros: HashMap::new(),
            last_macro: None,
            marks: HashMap::new(),
            block: None,
            last_visual: None,
            occurrence_word: false,
            block_insert: false,
            cmd_history: Vec::new(),
            search_history: Vec::new(),
            mouse: MouseState::default(),
            keymap: crate::keymap::Keymap::default(),
            project: None,
            tree: None,
            sidebar_focus: false,
            recent: Default::default(),
            jumps: Default::default(),
            jumping: false,
            closed: Vec::new(),
            symbols: None,
            grep_last: Default::default(),
            root_cache: Default::default(),
        };
        e.add_buffer(Document::default());
        e
    }

    // ----- buffers -----

    #[inline]
    pub fn buf(&self) -> &Buffer {
        &self.buffers[self.active]
    }
    #[inline]
    pub fn buf_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active]
    }

    pub fn add_buffer(&mut self, doc: Document) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.buffers.push(Buffer::new(id, doc));
        self.events.push(Event::BufferOpened(id));
        self.buffers.len() - 1
    }

    /// Opens `path` (or switches to it when already open). Missing files open as new, empty buffers. Big files
    /// (see [`Editor::async_load_bytes`]) are handed to the app as [`Event::Load`].
    pub fn open(&mut self, path: &Path) -> std::io::Result<()> {
        let path = self.absolute(path);
        if let Some(i) = self.buffers.iter().position(|b| b.doc.path.as_deref() == Some(&path)) {
            self.switch_to(i);
            return Ok(());
        }
        if let Some(max) = self.async_load_bytes
            && std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() > max)
        {
            self.events.push(Event::Load(path));
            return Ok(());
        }
        let doc = Document::load(&path)?;
        self.install_document(doc);
        Ok(())
    }

    /// Puts a loaded document in a buffer: replaces an untouched scratch buffer, else adds one.
    pub fn install_document(&mut self, doc: Document) {
        if let Some(p) = doc.path.clone() {
            self.remember_file(&p);
        }
        self.push_jump();
        let reuse = self.buffers.len() == 1
            && self.buffers[0].doc.path.is_none()
            && !self.buffers[0].doc.dirty
            && self.buffers[0].doc.is_empty();
        if reuse {
            let id = self.buffers[0].id;
            self.buffers[0] = Buffer::new(id, doc);
            self.active = 0;
            self.events.push(Event::BufferOpened(id));
        } else {
            self.active = self.add_buffer(doc);
        }
        self.set_mode(if self.vim() { Mode::Normal } else { Mode::Insert });
        let rows = self.buffers.first().map(|b| b.view).unwrap_or_default();
        let v = &mut self.buf_mut().view;
        v.rows = rows.rows;
        v.cols = rows.cols;
    }

    pub fn absolute(&self, path: &Path) -> PathBuf {
        let p = if let Some(rest) = path.to_str().and_then(|s| s.strip_prefix("~/")) {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)).unwrap_or_else(|| path.to_path_buf())
        } else {
            path.to_path_buf()
        };
        let p = if p.is_absolute() { p } else { self.cwd.join(p) };
        normalize(&p)
    }

    pub fn switch_to(&mut self, i: usize) {
        if i < self.buffers.len() && i != self.active {
            self.push_jump();
            self.commit();
            let view = self.buf().view;
            self.active = i;
            let v = &mut self.buf_mut().view;
            v.rows = view.rows;
            v.cols = view.cols;
            self.pending.clear();
            self.set_mode(if self.vim() { Mode::Normal } else { Mode::Insert });
        }
    }

    pub fn close_buffer(&mut self, force: bool) {
        if self.buf().doc.dirty && !force {
            self.error("unsaved changes (add ! to discard, or save first)");
            return;
        }
        self.remember_closed();
        if self.buffers.len() == 1 {
            let id = self.buf().id;
            self.buffers[0] = Buffer::new(id, Document::default());
            self.events.push(Event::BufferOpened(id));
        } else {
            self.buffers.remove(self.active);
            self.active = self.active.min(self.buffers.len() - 1);
        }
    }

    pub fn lang(&self) -> Option<&'static lang::Lang> {
        let doc = &self.buf().doc;
        doc.path.as_deref().and_then(lang::detect).or_else(|| {
            let first = doc.rope.line(0);
            let first: std::borrow::Cow<str> = first.into();
            lang::detect_shebang(&first)
        })
    }

    // ----- state helpers -----

    #[inline]
    pub fn vim(&self) -> bool {
        self.features.on(Feature::Vim)
    }

    pub fn opts(&self) -> Opts {
        Opts {
            tab_width: self.settings.tab_width,
            indent: if self.settings.expand_tab { " ".repeat(self.settings.tab_width) } else { "\t".into() },
            auto_pairs: self.features.on(Feature::AutoPairs),
            auto_indent: self.features.on(Feature::AutoIndent),
        }
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), false));
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), true));
    }

    pub(crate) fn commit(&mut self) {
        self.buf_mut().commit();
    }

    /// Switches mode, converting the selection between caret (insert) and inclusive (visual/normal) forms.
    pub fn set_mode(&mut self, mode: Mode) {
        let old = self.mode;
        if old == mode {
            return;
        }
        if old.is_visual() {
            self.last_visual = Some((old, self.buf().sel.clone()));
        }
        let rope_len = self.buf().doc.len();
        match mode {
            Mode::Normal => {
                let insert_like = old == Mode::Insert;
                let b = self.buf_mut();
                let rope = &b.doc.rope;
                b.sel = b.sel.transform(|r| {
                    let mut p = r.head;
                    if insert_like && p > text::line_start(rope, text::line_of(rope, p)) {
                        p = text::prev_grapheme(rope, p);
                    }
                    Range::point(text::clamp_normal(rope, p))
                });
                self.block = None;
            }
            Mode::Insert => {
                if old.is_visual() {
                    let b = self.buf_mut();
                    b.sel = b.sel.collapse();
                }
                self.block = None;
            }
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock => {
                if old == Mode::Insert {
                    // caret ranges [from, to) become inclusive ranges
                    let b = self.buf_mut();
                    b.sel = b.sel.transform(|r| {
                        if r.is_empty() {
                            r
                        } else if r.forward() {
                            Range::new(r.anchor, r.head - 1)
                        } else {
                            Range::new(r.anchor - 1, r.head)
                        }
                    });
                }
                if mode == Mode::VisualBlock {
                    let p = self.buf().sel.primary();
                    self.block = Some((p.anchor, p.head));
                    self.rebuild_block();
                } else {
                    self.block = None;
                }
            }
        }
        let _ = rope_len;
        self.mode = mode;
        if mode == Mode::Insert || old == Mode::Insert {
            self.commit_unless_recording(mode);
        }
    }

    fn commit_unless_recording(&mut self, mode: Mode) {
        // the whole insert session is one undo step: commit when it starts and when it ends
        if mode != Mode::Insert {
            self.commit();
        }
    }

    /// Visual-block selection: one inclusive range per line of the rectangle.
    pub(crate) fn rebuild_block(&mut self) {
        let Some((anchor, head)) = self.block else { return };
        let tw = self.settings.tab_width;
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        let (la, lh) = (text::line_of(rope, anchor), text::line_of(rope, head));
        let (ca, ch) = (text::col_of(rope, anchor, tw), text::col_of(rope, head, tw));
        let (left, right) = (ca.min(ch), ca.max(ch));
        // the head on a line break: `$` was used, every line goes to its end
        let right = if head == text::line_end(rope, lh) && text::line_len(rope, lh) > 0 { usize::MAX } else { right };
        let mut ranges = Vec::new();
        let mut primary = 0;
        let lines: Vec<usize> = if la <= lh { (la..=lh).collect() } else { (lh..=la).rev().collect() };
        for l in lines {
            let a = text::pos_at_col(rope, l, left, tw);
            if text::col_of(rope, a, tw) < left || a == text::line_end(rope, l) {
                continue; // the line does not reach into the block
            }
            let h = text::clamp_normal(rope, text::pos_at_col(rope, l, right, tw)).max(a);
            if l == lh {
                primary = ranges.len();
            }
            ranges.push(if ca <= ch { Range::new(a, h) } else { Range::new(h, a) });
        }
        if ranges.is_empty() {
            ranges.push(Range::point(text::clamp_normal(rope, head)));
        }
        b.sel = Selection::new(ranges, primary);
    }

    /// `[from, to)` spans to highlight as selected.
    pub fn selection_spans(&self) -> Vec<(usize, usize)> {
        let b = self.buf();
        let rope = &b.doc.rope;
        let len = rope.len_chars();
        b.sel
            .ranges()
            .iter()
            .filter_map(|r| match self.mode {
                Mode::Visual | Mode::VisualBlock => Some((r.from(), text::grapheme_end(rope, r.to()).min(len))),
                Mode::VisualLine => {
                    let a = text::line_of(rope, r.from());
                    let z = text::line_of(rope, r.to());
                    let end = if z < text::last_line(rope) { text::line_start(rope, z + 1) } else { len };
                    Some((text::line_start(rope, a), end))
                }
                _ => (!r.is_empty()).then(|| (r.from(), r.to())),
            })
            .collect()
    }

    pub fn cursor_shape(&self) -> CursorShape {
        match self.mode {
            Mode::Insert => CursorShape::Bar,
            Mode::Normal if !self.pending.is_empty() => CursorShape::Underline,
            _ => CursorShape::Block,
        }
    }

    /// Keys typed so far of an unfinished vim command (shown in the status line).
    pub fn pending_keys(&self) -> String {
        self.pending.iter().filter(|k| !crate::vim::is_insert_once(k)).map(|k| k.to_string()).collect()
    }

    pub fn recording_macro(&self) -> Option<char> {
        self.macro_rec.as_ref().map(|m| m.0)
    }

    // ----- registers -----

    pub(crate) fn write_register(&mut self, name: Option<char>, reg: Register, yank: bool) {
        let unnamed = name.is_none();
        let name = name.unwrap_or('"');
        if name == '_' {
            return;
        }
        // "A appends to "a
        let reg = if name.is_ascii_uppercase() {
            let lower = name.to_ascii_lowercase();
            let reg = match self.registers.get(&lower) {
                Some(old) if old.linewise || reg.linewise => Register {
                    parts: vec![format!("{}\n{}", old.parts.join("\n"), reg.parts.join("\n"))],
                    linewise: true,
                    blockwise: false,
                },
                Some(old) => Register {
                    parts: vec![old.parts.join("\n") + &reg.parts.join("\n")],
                    linewise: false,
                    blockwise: false,
                },
                None => reg,
            };
            self.registers.insert(lower, reg.clone());
            reg
        } else {
            reg
        };
        if yank && unnamed {
            self.registers.insert('0', reg.clone());
        } else if !yank && unnamed {
            // deletes: whole lines or multi-line text shift through "1.."9, smaller ones go to "-
            if reg.linewise || reg.parts.iter().any(|p| p.contains('\n')) {
                for i in (1..9u8).rev() {
                    if let Some(r) = self.registers.remove(&char::from(b'0' + i)) {
                        self.registers.insert(char::from(b'1' + i), r);
                    }
                }
                self.registers.insert('1', reg.clone());
            } else {
                self.registers.insert('-', reg.clone());
            }
        }
        if name != '"' && name != '+' && name != '*' && !name.is_ascii_uppercase() {
            self.registers.insert(name, reg.clone());
        }
        let to_clipboard = name == '+' || name == '*' || (name == '"' && self.features.on(Feature::Clipboard));
        if to_clipboard {
            self.clipboard.set(reg.joined());
        }
        self.registers.insert('"', reg);
    }

    pub(crate) fn read_register(&mut self, name: Option<char>) -> Option<Register> {
        let name = name.unwrap_or('"');
        // read-only registers: last search, last command line, file name
        let text = |s: String| (!s.is_empty()).then(|| Register { parts: vec![s], linewise: false, blockwise: false });
        match name {
            '/' => return text(self.search_text.clone()),
            ':' => return text(self.cmd_history.last().cloned().unwrap_or_default()),
            '%' => return text(self.buf().doc.path.as_ref().map(|p| relative(p, &self.cwd)).unwrap_or_default()),
            _ => {}
        }
        let from_clipboard = name == '+' || name == '*' || (name == '"' && self.features.on(Feature::Clipboard));
        if from_clipboard && let Some(t) = self.clipboard.get() {
            // our own yank keeps its shape (linewise, blockwise, one part per cursor)
            if let Some(r) = self.registers.get(&'"').filter(|r| r.joined() == t) {
                return Some(r.clone());
            }
            return Some(Register::from_text(t));
        }
        self.registers.get(&name).cloned()
    }

    // ----- input -----

    pub fn handle_key(&mut self, k: KeyPress) {
        if self.sidebar_focus && self.picker.is_none() && self.prompt.is_none() && self.about.is_none() {
            return self.tree_key(k);
        }
        if let Some((_, keys)) = &mut self.macro_rec {
            keys.push(k);
        }
        // user mappings see typed keys first; dot-repeat replays keys that were already mapped
        if !self.replaying && self.map_key(k) {
            self.after_key();
            return;
        }
        self.dispatch_key(k);
    }

    /// Resolves keys held as the start of a longer mapping (`j` of `jk`) as if no more keys came. The app calls it
    /// [`Keymap::timeout_ms`](crate::keymap::Keymap) after the last key while [`Editor::mapping_pending`] is true.
    pub fn flush_pending_mappings(&mut self) {
        if self.flush_mappings() {
            self.after_key();
        }
    }

    /// One key past the user keymap: built-in bindings, then the prompt, picker, vim or insert layer.
    pub(crate) fn dispatch_key(&mut self, k: KeyPress) {
        if !self.replaying {
            self.message = None;
        }
        if self.picker.is_some() {
            self.picker_key(k);
            return;
        }
        if self.about.is_some() {
            self.about_key(k);
            return;
        }
        if self.prompt.is_some() {
            self.prompt_key(k);
            self.after_key();
            return;
        }
        if self.pending.is_empty() {
            if let Some(cmd) = self.binding(k) {
                self.run(cmd);
                self.after_key();
                return;
            }
        }
        if self.mode == Mode::Insert || !self.vim() {
            if let Some(rec) = &mut self.dot_recording {
                rec.push(k);
            }
            self.insert_key(k);
        } else {
            self.vim_key(k);
        }
        self.after_key();
    }

    /// Typed text that did not come as a key press (IME commits, paste from the app).
    pub fn insert_text(&mut self, s: &str) {
        if self.about.is_some() {
            return;
        }
        if self.prompt.is_some() || self.picker.is_some() || self.sidebar_focus {
            for c in s.chars() {
                self.handle_key(KeyPress::ch(c));
            }
            return;
        }
        if self.mode != Mode::Insert {
            self.set_mode(Mode::Insert);
        }
        ops::insert_str(self.buf_mut(), s);
        self.after_key();
    }

    fn after_key(&mut self) {
        let so = self.settings.scrolloff;
        let tw = self.settings.tab_width;
        if self.buffers.is_empty() {
            return;
        }
        self.map_marks();
        let b = self.buf_mut();
        // only rebuild the selection when something is out of range (cheap with a million cursors)
        let len = b.doc.len();
        if b.sel.ranges().iter().any(|r| r.anchor > len || r.head > len) {
            b.sel = b.sel.clamp(len);
        }
        b.scroll_to_cursor(so, tw);
    }

    /// Moves marks along with the edits made since the last key, in every buffer (vim keeps a mark on its text).
    pub(crate) fn map_marks(&mut self) {
        for b in &mut self.buffers {
            let log = b.doc.take_mark_log();
            if log.is_empty() || !self.marks.keys().any(|(id, _)| *id == b.id) {
                continue;
            }
            for ((id, _), pos) in self.marks.iter_mut() {
                if *id == b.id {
                    for cs in &log {
                        *pos = cs.map(*pos, crate::change::Assoc::Before);
                    }
                }
            }
        }
    }

    /// Key bindings that work in every mode (Sublime-style chords), checked before the vim layer. Public so menus can
    /// show the chord that runs a command.
    pub fn binding(&self, k: KeyPress) -> Option<Command> {
        use Command::*;
        let m = k.mods;
        let insert = self.mode == Mode::Insert || !self.vim();
        let visual = self.mode.is_visual();
        let c = match k.key {
            Key::Char(c) => c,
            Key::F(3) if m.alt => return Some(SelectAllOccurrences),
            Key::F(3) => return Some(if m.shift { FindPrev } else { FindNext }),
            Key::Up | Key::Down if (m.ctrl && m.shift) || (m.alt && !m.ctrl && !m.shift) => {
                return Some(if k.key == Key::Up { MoveLineUp } else { MoveLineDown });
            }
            Key::Up | Key::Down if (m.ctrl && m.alt) || (m.alt && m.shift) => {
                return Some(if k.key == Key::Up { AddCursorAbove } else { AddCursorBelow });
            }
            Key::Enter if m.ctrl => return Some(if m.shift { InsertLineAbove } else { InsertLineBelow }),
            Key::Tab if m.ctrl => return Some(if m.shift { PrevBuffer } else { NextBuffer }),
            Key::PageDown if m.ctrl => return Some(NextBuffer),
            Key::PageUp if m.ctrl => return Some(PrevBuffer),
            // back / forward along the jump list, as in browsers
            Key::Left if m.alt && !m.ctrl && !m.shift => return Some(JumpBack),
            Key::Right if m.alt && !m.ctrl && !m.shift => return Some(JumpForward),
            _ => return None,
        };
        // Sublime's Alt+- / Alt+Shift+-
        if m.alt && !m.ctrl && c == '-' {
            return Some(if m.shift { JumpForward } else { JumpBack });
        }
        if m.ctrl && m.alt && c == 'p' {
            return Some(OpenRecentProject);
        }
        if !m.ctrl || m.alt {
            return None;
        }
        Some(match (c, m.shift) {
            ('s', false) => Save,
            ('s', true) => SaveAll,
            ('q', _) => Quit,
            ('p', true) => CommandPalette,
            ('p', false) => OpenFile,
            // vim's insert-mode Ctrl+O runs one normal command; Ctrl+P opens files there
            ('o', false) if insert && !self.vim() => OpenFile,
            ('n', false) if insert => NewFile,
            ('w', false) if !self.vim() => CloseBuffer,
            ('z', false) => Undo,
            ('z', true) => Redo,
            ('y', false) if insert => Redo,
            ('/', _) => ToggleComment,
            ('k', true) => DeleteLine,
            ('d', true) => DuplicateLine,
            ('l', true) => SplitSelectionIntoLines,
            ('l', false) => SelectLine,
            ('m', false) => GotoMatchingBracket,
            ('m', true) => SelectInsideBrackets,
            ('=' | '+', _) => FontBigger,
            ('-', _) => FontSmaller,
            ('0', false) => FontReset,
            ('g', false) => GotoLine,
            ('f', false) if insert => Find,
            ('d', false) if insert => AddNextOccurrence,
            ('n', false) if !insert => AddNextOccurrence,
            ('x', true) if !insert => SkipOccurrence,
            ('a', false) if insert => SelectAll,
            ('c', false) if insert || visual => Copy,
            ('x', false) if insert => Cut,
            ('v', false) if insert => Paste,
            (']', false) => Indent,
            ('[', false) if !self.vim() => Outdent,
            ('j', false) if !self.vim() => JoinLines,
            ('b', true) => SwitchBuffer,
            ('u', true) => CheckForUpdates,
            // project navigation (in vim normal mode Ctrl+B and Ctrl+R stay vim's)
            ('b', false) if insert => ToggleSidebar,
            ('e', true) => FocusSidebar,
            ('f', true) => FindInProject,
            ('r', false) if !self.vim() => GotoSymbol,
            ('o', true) => GotoSymbol,
            ('r', true) => GotoSymbolInProject,
            ('t', true) => ReopenClosed,
            ('`', false) => ToggleTerminal,
            ('`', true) => NewTerminal,
            _ => return None,
        })
    }

    // ----- commands -----

    pub fn run(&mut self, cmd: Command) {
        use Command::*;
        let opts = self.opts();
        let tw = self.settings.tab_width;
        match cmd {
            Save => self.save(None),
            SaveAll => {
                let n = self
                    .buffers
                    .iter_mut()
                    .filter(|b| b.doc.dirty && b.doc.path.is_some())
                    .map(|b| b.doc.save())
                    .filter(Result::is_ok)
                    .count();
                self.info(format!("saved {n} file(s)"));
            }
            Quit => self.quit(false),
            ForceQuit => self.quit(true),
            NewFile => {
                self.active = self.add_buffer(Document::default());
                self.set_mode(if self.vim() { Mode::Normal } else { Mode::Insert });
            }
            CloseBuffer => self.close_buffer(false),
            NextBuffer => self.switch_to((self.active + 1) % self.buffers.len()),
            PrevBuffer => self.switch_to((self.active + self.buffers.len() - 1) % self.buffers.len()),
            Reload => match self.buf_mut().reload() {
                Ok(()) => self.info("reloaded"),
                Err(e) => self.error(e.to_string()),
            },
            Undo => self.undo(),
            Redo => self.redo(),
            Copy => self.sublime_copy(false),
            Cut => self.sublime_copy(true),
            Paste => self.sublime_paste(),
            SelectAll => {
                let len = self.buf().doc.len();
                self.buf_mut().sel = Selection::single(Range::new(0, len));
                if self.vim() && self.mode != Mode::Insert {
                    self.buf_mut().sel = Selection::single(Range::new(0, len.saturating_sub(1)));
                    self.mode = Mode::Visual;
                }
            }
            SelectLine => self.with_carets(|b, _| ops::select_lines(b), true),
            DeleteLine => {
                let lines = self.target_lines();
                let removed = ops::delete_lines(self.buf_mut(), &lines);
                let _ = removed;
                self.commit();
                self.exit_visual();
            }
            DuplicateLine => self.with_carets(|b, _| ops::duplicate_lines(b), false),
            MoveLineUp => self.with_carets(|b, _| ops::move_lines(b, true), false),
            MoveLineDown => self.with_carets(|b, _| ops::move_lines(b, false), false),
            ToggleComment => {
                let token = self.lang().and_then(|l| l.line_comment).unwrap_or("#");
                let lines = self.target_lines();
                ops::toggle_comment(self.buf_mut(), &lines, token);
                self.commit();
            }
            Indent => {
                let lines = self.target_lines();
                ops::indent_lines(self.buf_mut(), &lines, &opts);
                self.commit();
            }
            Outdent => {
                let lines = self.target_lines();
                ops::outdent_lines(self.buf_mut(), &lines, &opts);
                self.commit();
            }
            JoinLines => {
                let lines = self.target_lines();
                ops::join_lines(self.buf_mut(), &lines, true, true);
                self.commit();
                self.exit_visual();
            }
            InsertLineBelow | InsertLineAbove => {
                if self.mode != Mode::Insert {
                    self.set_mode(Mode::Insert);
                }
                ops::open_line(self.buf_mut(), cmd == InsertLineBelow, &opts);
            }
            AddCursorAbove | AddCursorBelow => {
                if self.mode.is_visual() {
                    self.set_mode(Mode::Normal);
                }
                ops::add_cursor_vertical(self.buf_mut(), cmd == AddCursorBelow, tw);
            }
            AddNextOccurrence | SkipOccurrence => {
                let first = self.buf().sel.ranges().iter().all(|r| r.is_empty()) && !self.mode.is_visual();
                if first {
                    self.occurrence_word = true;
                }
                let word = self.occurrence_word;
                self.with_carets(
                    |b, _| {
                        ops::add_next_occurrence(b, word, cmd == SkipOccurrence);
                    },
                    true,
                );
            }
            SelectAllOccurrences => {
                let word = self.buf().sel.ranges().iter().all(|r| r.is_empty()) || self.occurrence_word;
                self.with_carets(|b, _| ops::select_all_occurrences(b, word), true);
            }
            SplitSelectionIntoLines => self.with_carets(|b, _| ops::split_into_lines(b), true),
            CollapseCursors => {
                let b = self.buf_mut();
                b.sel = b.sel.only_primary().collapse();
            }
            GotoMatchingBracket => {
                let b = self.buf_mut();
                let rope = &b.doc.rope;
                let sel = b.sel.transform(|r| {
                    let p = crate::motion::matching_bracket(rope, r.head)
                        .or_else(|| r.head.checked_sub(1).and_then(|q| crate::motion::matching_bracket(rope, q)))
                        .or_else(|| crate::motion::enclosing_pair(rope, r.head, '{', '}').map(|(o, _)| o));
                    p.map_or(r, Range::point)
                });
                b.sel = sel;
            }
            SelectInsideBrackets => self.with_carets(
                |b, _| {
                    let rope = &b.doc.rope;
                    let sel = b.sel.transform(|r| {
                        ['(', '[', '{']
                            .iter()
                            .filter_map(|&o| {
                                let c = crate::motion::bracket_pair(o).unwrap().1;
                                crate::motion::pair_object(rope, r.from(), o, c, false)
                            })
                            .filter(|(a, z)| *a <= r.from() && *z >= r.to() && (*a, *z) != (r.from(), r.to()))
                            .max_by_key(|(a, _)| *a)
                            .map_or(r, |(a, z)| Range::new(a, z))
                    });
                    b.sel = sel;
                },
                true,
            ),
            GotoLine => self.open_prompt(PromptKind::Command),
            Find => self.open_prompt(PromptKind::Search { forward: true }),
            FindNext | FindPrev => self.search_next(cmd == FindPrev, 1),
            ClearSearch => self.search_visible = false,
            CommandPalette => self.open_palette(),
            OpenFile => self.open_file_picker(),
            SwitchBuffer => self.open_buffer_picker(),
            ShowFeatures => self.open_feature_picker(),
            ToggleVim => self.toggle_feature(Feature::Vim),
            ToggleLineNumbers => self.toggle_feature(Feature::LineNumbers),
            ToggleRelativeNumbers => self.toggle_feature(Feature::RelativeNumbers),
            ToggleWhitespace => self.toggle_feature(Feature::Whitespace),
            ToggleSyntax => self.toggle_feature(Feature::Syntax),
            FontBigger => self.events.push(Event::FontSize(1)),
            FontSmaller => self.events.push(Event::FontSize(-1)),
            FontReset => self.events.push(Event::FontReset),
            SelectTheme => self.events.push(Event::OpenChoice(Choice::Theme)),
            SelectAccent => self.events.push(Event::OpenChoice(Choice::Accent)),
            SelectFont => self.events.push(Event::OpenChoice(Choice::Font)),
            ToggleLigatures => self.events.push(Event::Ligatures(None)),
            CheckForUpdates => self.events.push(Event::CheckUpdates),
            InstallUpdate => self.events.push(Event::InstallUpdate),
            OpenConfig => self.events.push(Event::OpenConfig),
            McpSetup => self.events.push(Event::McpSetup),
            About => {
                self.picker = None;
                self.prompt = None;
                self.about = Some(0);
            }
            // ----- phase 3: titlebar -----
            ToggleSystemTitleBar => self.toggle_feature(Feature::CustomTitlebar),
            ShowMenu | CustomizeTitleBar | MinimizeWindow | ToggleMaximize => self.events.push(Event::App(cmd)),
            // ----- project navigation -----
            ToggleSidebar => {
                self.sidebar_focus = false;
                self.toggle_feature(Feature::Sidebar);
                self.sync_tree();
            }
            FocusSidebar => {
                let on = !self.sidebar_focus;
                self.focus_sidebar(on);
            }
            RefreshSidebar => {
                if let Some(t) = &mut self.tree {
                    t.refresh();
                }
            }
            FindInProject => self.open_grep(""),
            GotoSymbol => self.open_symbols(false),
            GotoSymbolInProject => self.open_symbols(true),
            OpenRecent => self.open_recent_files(),
            OpenRecentProject => self.open_recent_projects(),
            ReopenClosed => self.reopen_closed(),
            JumpBack => self.jump(false),
            JumpForward => self.jump(true),
            GotoFileUnderCursor => self.goto_file_under_cursor(),
            ToggleMinimap => self.toggle_feature(Feature::Minimap),
            ToggleTerminal | NewTerminal | CloseTerminal | NextTerminal | PrevTerminal | MaximizeTerminal
            | FocusTerminal | FocusEditor | TerminalNormalMode => self.terminal(cmd, None),
        }
    }

    /// Hands a terminal command to the app, unless the terminal feature is off.
    pub fn terminal(&mut self, cmd: Command, command_line: Option<String>) {
        if !self.features.available(Feature::Terminal) {
            return self.error("the terminal is not compiled into this build");
        }
        if !self.features.on(Feature::Terminal) {
            return self.error("the terminal is off (:set terminal)");
        }
        self.events.push(Event::Terminal(cmd, command_line));
    }

    pub fn toggle_feature(&mut self, f: Feature) {
        if !self.features.available(f) {
            self.error(format!("{} is not compiled into this build", features::info(f).key));
            return;
        }
        self.features.toggle(f);
        let on = self.features.on(f);
        if f == Feature::Vim {
            self.set_mode(if on { Mode::Normal } else { Mode::Insert });
        }
        self.info(format!("{} {}", features::info(f).key, if on { "on" } else { "off" }));
        self.events.push(Event::FeatureToggled(f));
    }

    /// Runs `f` with caret-style ranges even in visual mode, then converts back.
    fn with_carets(&mut self, f: impl FnOnce(&mut Buffer, &Opts), selects: bool) {
        let opts = self.opts();
        let vim_sel = self.vim() && self.mode != Mode::Insert;
        if vim_sel {
            let spans = self.selection_spans();
            let visual = self.mode.is_visual();
            let b = self.buf_mut();
            let prim = b.sel.primary_index();
            let back: Vec<bool> = b.sel.ranges().iter().map(|r| !r.forward()).collect();
            if spans.len() == b.sel.len() && visual {
                b.sel = Selection::new(
                    spans.iter().zip(back).map(|(&(a, z), bk)| if bk { Range::new(z, a) } else { Range::new(a, z) }),
                    prim,
                );
            }
        }
        f(self.buf_mut(), &opts);
        self.commit();
        if vim_sel {
            let any = self.buf().sel.ranges().iter().any(|r| !r.is_empty());
            if any && selects {
                if self.mode != Mode::Visual {
                    self.mode = Mode::Insert; // so set_mode converts from caret form
                    self.set_mode(Mode::Visual);
                } else {
                    let b = self.buf_mut();
                    b.sel = b.sel.transform(|r| {
                        if r.is_empty() {
                            r
                        } else if r.forward() {
                            Range::new(r.anchor, r.head - 1)
                        } else {
                            Range::new(r.anchor - 1, r.head)
                        }
                    });
                }
            } else {
                self.mode = Mode::Insert;
                self.set_mode(Mode::Normal);
            }
        }
    }

    /// Lines the current command applies to.
    fn target_lines(&self) -> Vec<usize> {
        let b = self.buf();
        if self.mode.is_visual() {
            let rope = &b.doc.rope;
            let mut lines = Vec::new();
            for (a, z) in self.selection_spans() {
                let la = text::line_of(rope, a);
                let mut lz = text::line_of(rope, z);
                if lz > la && z == text::line_start(rope, lz) {
                    lz -= 1;
                }
                lines.extend(la..=lz);
            }
            lines.sort_unstable();
            lines.dedup();
            lines
        } else {
            ops::selected_lines(&b.doc.rope, &b.sel)
        }
    }

    fn exit_visual(&mut self) {
        if self.mode.is_visual() {
            self.set_mode(Mode::Normal);
        }
    }

    pub fn undo(&mut self) {
        if self.mode == Mode::Insert && self.vim() {
            self.set_mode(Mode::Normal);
        }
        match self.buf_mut().history_step(false) {
            Some(jump) => self.fix_cursor_after_history(jump),
            None => self.info("already at oldest change"),
        }
    }

    pub fn redo(&mut self) {
        match self.buf_mut().history_step(true) {
            Some(jump) => self.fix_cursor_after_history(jump),
            None => self.info("already at newest change"),
        }
    }

    /// Vim puts the cursor on the first changed line: at the column the change started from when that is the same
    /// line (or the line just above, for `o`), else on the first non-blank. Multi-cursor groups and Sublime mode
    /// get their selection back as it was.
    fn fix_cursor_after_history(&mut self, jump: crate::document::HistoryJump) {
        if self.mode.is_visual() {
            self.set_mode(Mode::Normal);
        }
        if self.mode != Mode::Normal {
            return;
        }
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        if jump.single && b.sel.len() == 1 {
            let mut line = text::line_of(rope, jump.top);
            if jump.start.0 + 1 == line && line > 0 {
                line -= 1;
            }
            let line = line.min(text::last_line(rope));
            let pos = if jump.start.0 == line {
                (text::line_start(rope, line) + jump.start.1).min(text::line_end(rope, line))
            } else {
                text::first_non_blank(rope, line)
            };
            b.sel = Selection::point(text::clamp_normal(rope, pos));
        } else {
            b.sel = b.sel.transform(|r| Range::point(text::clamp_normal(rope, r.from())));
        }
    }

    fn sublime_copy(&mut self, cut: bool) {
        let spans = self.selection_spans();
        let b = self.buf();
        let rope = &b.doc.rope;
        let reg = if spans.is_empty() {
            // nothing selected: whole lines
            let lines = ops::selected_lines(rope, &b.sel);
            Register {
                parts: lines.iter().map(|&l| text::line_text(rope, l)).collect(),
                linewise: true,
                blockwise: false,
            }
        } else {
            Register {
                parts: spans.iter().map(|&(a, z)| rope.slice(a..z).to_string()).collect(),
                linewise: self.mode == Mode::VisualLine,
                blockwise: self.mode == Mode::VisualBlock,
            }
        };
        self.write_register(None, reg, !cut);
        if cut {
            if spans.is_empty() {
                let lines = ops::selected_lines(&self.buf().doc.rope, &self.buf().sel);
                ops::delete_lines(self.buf_mut(), &lines);
            } else {
                self.with_carets(|b, _| ops::delete_forward(b), false);
            }
            self.commit();
        }
        self.exit_visual();
    }

    fn sublime_paste(&mut self) {
        let Some(mut reg) = self.read_register(None) else { return };
        let n = self.buf().sel.len();
        if n > 1 && reg.parts.len() == 1 && !reg.linewise {
            // as many lines as cursors: one line each (Sublime)
            let lines: Vec<String> = reg.parts[0].split('\n').map(str::to_string).collect();
            if lines.len() == n {
                reg.parts = lines;
            }
        }
        let per_cursor = reg.parts.len() == n && n > 1;
        let all_empty = self.buf().sel.ranges().iter().all(|r| r.is_empty());
        if self.mode.is_visual() {
            self.with_carets(|b, _| ops::delete_forward(b), false);
            self.set_mode(Mode::Insert);
        }
        let mut i = 0;
        let joined = reg.parts.join("\n");
        ops::edit_each(self.buf_mut(), |rope, r| {
            let t = if per_cursor { reg.parts[i].clone() } else { joined.clone() };
            i += 1;
            if reg.linewise && all_empty {
                let ls = text::line_start(rope, text::line_of(rope, r.head));
                let n = t.chars().count() + 1;
                ops::Edit::Change(crate::change::Change::insert(ls, format!("{t}\n")), n + r.head - ls, n + r.head - ls)
            } else {
                let n = t.chars().count();
                ops::Edit::Change(crate::change::Change::replace(r.from(), r.to(), t), n, n)
            }
        });
        self.commit();
    }

    fn save(&mut self, path: Option<&str>) {
        let result = match path {
            Some(p) => {
                let p = self.absolute(Path::new(p));
                let r = self.buf_mut().doc.save_as(&p);
                let id = self.buf().id;
                self.events.push(Event::BufferOpened(id));
                r
            }
            None => self.buf_mut().doc.save(),
        };
        match result {
            Ok(n) => {
                let name = self.buf().doc.display_name();
                let lines = self.buf().doc.rope.len_lines();
                self.info(format!("\"{name}\" {lines}L, {n}B written"));
            }
            Err(e) => self.error(format!("save failed: {e}")),
        }
    }

    fn quit(&mut self, force: bool) {
        if !force && let Some(b) = self.buffers.iter().find(|b| b.doc.dirty) {
            let name = b.doc.display_name();
            self.error(format!("\"{name}\" has unsaved changes (:q! to discard, :wq to save)"));
            return;
        }
        self.events.push(Event::Quit);
    }

    // ----- insert mode (also the whole of non-vim mode) -----

    pub(crate) fn insert_key(&mut self, k: KeyPress) {
        let opts = self.opts();
        let m = k.mods;
        let tw = self.settings.tab_width;
        if !self.vim() && self.sublime_chord(k) {
            return;
        }
        if self.vim() && self.insert_register(k) {
            return;
        }
        if let Some(c) = k.text() {
            if !self.vim() {
                // Sublime undoes typing a word at a time: a word starting after white space opens a new step
                let b = self.buf();
                let h = b.sel.primary().head;
                let after_space = h > 0 && b.sel.primary().is_empty() && b.rope().char(h - 1).is_whitespace();
                if after_space && !c.is_whitespace() {
                    self.commit();
                }
            }
            ops::insert_char(self.buf_mut(), c, &opts);
            return;
        }
        let shift = m.shift;
        match k.key {
            Key::Esc => self.insert_escape(),
            Key::Char('[') if m.ctrl && self.vim() => self.insert_escape(),
            Key::Char('c') if m.ctrl && self.vim() && self.buf().sel.ranges().iter().all(|r| r.is_empty()) => {
                self.insert_escape()
            }
            Key::Enter => ops::newline(self.buf_mut(), &opts),
            Key::Tab if shift => {
                let lines = ops::selected_lines(self.buf().rope(), &self.buf().sel);
                ops::outdent_lines(self.buf_mut(), &lines, &opts);
            }
            Key::Tab => {
                let multi_line = self.buf().sel.ranges().iter().any(|r| {
                    let rope = self.buf().rope();
                    !r.is_empty() && text::line_of(rope, r.from()) != text::line_of(rope, r.to())
                });
                if multi_line {
                    let lines = ops::selected_lines(self.buf().rope(), &self.buf().sel);
                    ops::indent_lines(self.buf_mut(), &lines, &opts);
                } else if self.settings.expand_tab {
                    ops::edit_each(self.buf_mut(), |rope, r| {
                        let col = text::col_of(rope, r.from(), tw);
                        let n = tw - col % tw;
                        ops::Edit::Change(crate::change::Change::replace(r.from(), r.to(), " ".repeat(n)), n, n)
                    });
                } else {
                    ops::insert_str(self.buf_mut(), "\t");
                }
            }
            Key::Backspace if m.ctrl || m.alt => ops::delete_word_back(self.buf_mut()),
            Key::Backspace => ops::backspace(self.buf_mut(), &opts),
            Key::Char('h') if m.ctrl => ops::backspace(self.buf_mut(), &opts),
            Key::Char('w') if m.ctrl && self.vim() => {
                // vim stops once at the place the insert started
                let id = self.buf().id;
                let start = self.marks.get(&(id, '[')).copied();
                ops::edit_each(self.buf_mut(), |rope, r| {
                    let ls = text::line_start(rope, text::line_of(rope, r.head));
                    let mut to = if r.head > ls {
                        crate::motion::prev_word_start(rope, r.head, false).max(ls)
                    } else {
                        r.head.saturating_sub(1)
                    };
                    if let Some(s) = start.filter(|&s| s < r.head && s > to) {
                        to = s;
                    }
                    ops::Edit::Change(crate::change::Change::delete(to, r.head), 0, 0)
                });
                self.marks.remove(&(id, '['));
            }
            Key::Char('o') if m.ctrl && self.vim() => self.insert_ctrl_o(),
            Key::Char('t') if m.ctrl && self.vim() => {
                let lines = ops::selected_lines(self.buf().rope(), &self.buf().sel);
                ops::indent_lines(self.buf_mut(), &lines, &opts);
            }
            Key::Char('w') if m.ctrl => ops::delete_word_back(self.buf_mut()),
            Key::Char('u') if m.ctrl && self.vim() => {
                // vim: the text typed since the insert started, else back to the indent, else the whole line start
                let id = self.buf().id;
                let start = self.marks.get(&(id, '[')).copied();
                ops::edit_each(self.buf_mut(), |rope, r| {
                    let line = text::line_of(rope, r.head);
                    let ls = text::line_start(rope, line);
                    let fnb = text::first_non_blank(rope, line);
                    let to = match start {
                        Some(s) if s < r.head && s >= ls => s,
                        _ if fnb < r.head => fnb,
                        _ => ls,
                    };
                    ops::Edit::Change(crate::change::Change::delete(to, r.head), 0, 0)
                });
                // the next Ctrl+U goes further
                self.marks.remove(&(id, '['));
            }
            Key::Delete if m.ctrl => ops::delete_word_forward(self.buf_mut()),
            Key::Delete => ops::delete_forward(self.buf_mut()),
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End | Key::PageUp | Key::PageDown => {
                self.caret_move(k.key, m.ctrl, shift);
                // a pause in typing: moving the caret starts a new undo step
                self.commit();
            }
            _ => {}
        }
    }

    /// Vim's insert-mode Ctrl+R {register}: types the register's contents. Returns true when it took the key.
    fn insert_register(&mut self, k: KeyPress) -> bool {
        let ctrl_r = KeyPress::ctrl('r');
        if self.pending.first() != Some(&ctrl_r) {
            if k == ctrl_r {
                self.pending.push(k);
                return true;
            }
            return false;
        }
        self.pending.clear();
        let Some(name) = k.text() else { return true };
        if let Some(reg) = self.read_register(Some(name)) {
            let mut s = reg.parts.join("\n");
            if reg.linewise {
                s.push('\n');
            }
            ops::insert_str(self.buf_mut(), &s);
        }
        true
    }

    /// Sublime's Ctrl+K chords: Ctrl+K Ctrl+D skips an occurrence, Ctrl+K Ctrl+K deletes to the line end,
    /// Ctrl+K Ctrl+U / Ctrl+K Ctrl+L upper/lower-case the selection. Returns true when it took the key.
    fn sublime_chord(&mut self, k: KeyPress) -> bool {
        let ctrl_k = KeyPress::ctrl('k');
        if self.pending.first() != Some(&ctrl_k) {
            if k == ctrl_k {
                self.pending.push(k);
                return true;
            }
            return false;
        }
        self.pending.clear();
        let Key::Char(c) = k.key else { return false };
        if !k.mods.ctrl {
            return false;
        }
        match c {
            'd' => self.run(Command::SkipOccurrence),
            'k' => {
                ops::edit_each(self.buf_mut(), |rope, r| {
                    let le = text::line_end(rope, text::line_of(rope, r.head));
                    let to = if r.head == le { (le + 1).min(rope.len_chars()) } else { le };
                    if r.is_empty() {
                        ops::Edit::Change(crate::change::Change::delete(r.head, to), 0, 0)
                    } else {
                        ops::Edit::Keep
                    }
                });
                self.commit();
            }
            'u' | 'l' => {
                let up = c == 'u';
                ops::edit_each(self.buf_mut(), |rope, r| {
                    let (a, z) = if r.is_empty() {
                        crate::motion::word_object(rope, r.head, false, false)
                    } else {
                        (r.from(), r.to())
                    };
                    let s = rope.slice(a..z).to_string();
                    let t = if up { s.to_uppercase() } else { s.to_lowercase() };
                    let n = t.chars().count();
                    if r.is_empty() {
                        ops::Edit::Change(crate::change::Change::replace(a, z, t), r.head - a, r.head - a)
                    } else if r.forward() {
                        ops::Edit::Change(crate::change::Change::replace(a, z, t), 0, n)
                    } else {
                        ops::Edit::Change(crate::change::Change::replace(a, z, t), n, 0)
                    }
                });
                self.commit();
            }
            _ => return false,
        }
        true
    }

    fn insert_escape(&mut self) {
        let rec = self.dot_recording.take();
        if self.vim() {
            // `3ix<Esc>`: the typed text goes in count times
            let keys = if self.replaying { Some(self.dot.clone()) } else { rec.clone() };
            if let Some(keys) = keys {
                self.repeat_insert(&keys);
            }
            let opts = self.opts();
            let id = self.buf().id;
            let start = self.marks.get(&(id, '[')).copied();
            ops::trim_auto_indent(self.buf_mut(), &opts, start);
        }
        if let Some(rec) = rec {
            self.dot = rec;
        }
        if self.vim() {
            self.set_mode(Mode::Normal);
            if std::mem::take(&mut self.block_insert) {
                // back to the top-left corner of the block
                let id = self.buf().id;
                let corner = self.marks.get(&(id, '[')).copied();
                let b = self.buf_mut();
                let rope = &b.doc.rope;
                b.sel = match corner {
                    Some(c) => Selection::point(text::clamp_normal(rope, c)),
                    None => b.sel.only_primary(),
                };
            }
        } else {
            let b = self.buf_mut();
            // Sublime keeps the first cursor; a lone selection collapses to its caret
            b.sel = if b.sel.len() > 1 { Selection::single(b.sel.ranges()[0]).collapse() } else { b.sel.collapse() };
            self.commit();
        }
        self.occurrence_word = false;
    }

    /// Arrow-key style caret movement (Sublime semantics), optionally extending the selection.
    fn caret_move(&mut self, key: Key, ctrl: bool, extend: bool) {
        let tw = self.settings.tab_width;
        let rows = self.buf().view.rows.max(2);
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        let len = rope.len_chars();
        let sel = b.sel.transform(|r| {
            let mut col = None;
            let head = match key {
                Key::Left if !extend && !r.is_empty() => r.from(),
                Key::Right if !extend && !r.is_empty() => r.to(),
                Key::Left if ctrl => crate::motion::prev_word_start(rope, r.head, false),
                Key::Right if ctrl => {
                    let p = crate::motion::word_end(rope, r.head, false);
                    (p + 1).min(len)
                }
                Key::Left => text::prev_grapheme(rope, r.head),
                Key::Right => text::next_grapheme(rope, r.head),
                Key::Home if ctrl => 0,
                Key::End if ctrl => len,
                Key::Home => {
                    let line = text::line_of(rope, r.head);
                    let fnb = text::first_non_blank(rope, line);
                    if r.head == fnb { text::line_start(rope, line) } else { fnb }
                }
                Key::End => text::line_end(rope, text::line_of(rope, r.head)),
                Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                    let n = if matches!(key, Key::PageUp | Key::PageDown) { rows - 1 } else { 1 };
                    let up = matches!(key, Key::Up | Key::PageUp);
                    let (p, c) = vertical(rope, r, if up { -(n as isize) } else { n as isize }, tw);
                    col = Some(c);
                    p
                }
                _ => r.head,
            };
            let anchor = if extend { r.anchor } else { head };
            Range { anchor, head, col }
        });
        b.sel = sel;
        if extend && self.vim() && self.mode == Mode::Normal {
            // shift+arrows in normal mode select, like a mouse drag
            self.mode = Mode::Insert;
            self.set_mode(Mode::Visual);
        }
    }

    // ----- mouse -----

    /// Mouse press at char position `pos`. `clicks` is 1, 2 (word) or 3 (line). Ctrl/Alt adds a cursor.
    pub fn mouse_down(&mut self, pos: usize, clicks: u8, mods: Mods) {
        self.pending.clear();
        if self.picker.is_some() || self.about.is_some() {
            return;
        }
        self.commit();
        let b = self.buf();
        let rope = &b.doc.rope;
        let pos = text::grapheme_start(rope, pos.min(rope.len_chars()));
        let unit = match clicks {
            2 => {
                let (a, z) = crate::motion::word_object(rope, pos, false, false);
                (a, z.max(a))
            }
            n if n >= 3 => {
                let l = text::line_of(rope, pos);
                let end = if l < text::last_line(rope) { text::line_start(rope, l + 1) } else { rope.len_chars() };
                (text::line_start(rope, l), end)
            }
            _ => (pos, pos),
        };
        self.mouse = MouseState { clicks, origin: Some(unit), dragging: true };
        let was_visual = self.mode.is_visual();
        if self.mode.is_visual() || self.mode == Mode::Normal {
            self.mode = Mode::Insert; // treat ranges as carets while the mouse is down
        }
        let r = if mods.shift {
            let p = self.buf().sel.primary();
            let anchor = if was_visual { p.anchor.min(p.head) } else { p.anchor };
            Range::new(anchor, if unit.0 < anchor { unit.0 } else { unit.1 })
        } else {
            Range::new(unit.0, unit.1)
        };
        let b = self.buf_mut();
        b.sel = if mods.ctrl || mods.alt { b.sel.push(r) } else { Selection::single(r) };
        self.occurrence_word = clicks == 2;
    }

    pub fn mouse_drag(&mut self, pos: usize) {
        if !self.mouse.dragging {
            return;
        }
        let Some((ua, uz)) = self.mouse.origin else { return };
        let clicks = self.mouse.clicks;
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        let pos = text::grapheme_start(rope, pos.min(rope.len_chars()));
        let (a, z) = match clicks {
            2 => crate::motion::word_object(rope, pos.min(rope.len_chars().saturating_sub(1)), false, false),
            n if n >= 3 => {
                let l = text::line_of(rope, pos);
                let end = if l < text::last_line(rope) { text::line_start(rope, l + 1) } else { rope.len_chars() };
                (text::line_start(rope, l), end)
            }
            _ => (pos, pos),
        };
        let r = if a < ua { Range::new(uz, a) } else { Range::new(ua, z.max(uz)) };
        let prim = b.sel.primary_index();
        let mut ranges: Vec<Range> = b.sel.ranges().to_vec();
        ranges[prim] = r;
        b.sel = Selection::new(ranges, prim);
    }

    pub fn mouse_up(&mut self) {
        if !self.mouse.dragging {
            return;
        }
        self.mouse.dragging = false;
        if self.vim() && self.mode == Mode::Insert && self.dot_recording.is_none() {
            let selected = self.buf().sel.ranges().iter().any(|r| !r.is_empty());
            // a click in normal mode stays in normal mode; a drag selects in visual mode
            self.set_mode(if selected { Mode::Visual } else { Mode::Normal });
            if !selected {
                let b = self.buf_mut();
                let rope = &b.doc.rope;
                b.sel = b.sel.transform(|r| Range::point(text::clamp_normal(rope, r.head)));
            }
        }
    }

    pub fn mouse_dragging(&self) -> bool {
        self.mouse.dragging
    }

    /// Scrolls the view without moving the cursor (mouse wheel).
    pub fn scroll(&mut self, lines: isize, cols: isize) {
        let b = self.buf_mut();
        let last = text::last_line(&b.doc.rope);
        b.view.top = (b.view.top as isize + lines).clamp(0, last as isize) as usize;
        b.view.left = (b.view.left as isize + cols).max(0) as usize;
    }

    // ----- search -----

    pub(crate) fn set_search(&mut self, pattern: &str) {
        self.search_text = pattern.to_string();
        self.search = self.compile_search(pattern);
        self.search_visible = true;
    }

    /// Search patterns are vim regexes in vim mode and plain Rust regexes otherwise (both smart-case).
    pub(crate) fn compile_search(&self, pattern: &str) -> Option<Regex> {
        if self.vim() { search::compile_vim(pattern) } else { search::compile(pattern) }
    }

    /// The end of `d/pat<CR>`: runs the operator from the cursor to the `count`-th match.
    fn search_operate(&mut self, op: crate::vim::Op, reg: Option<char>, forward: bool, count: usize) {
        let Some(re) = self.search.clone() else { return };
        let rope = &self.buf().doc.rope;
        let mut t = self.buf().sel.primary().head;
        for _ in 0..count {
            match search::find(rope, &re, t, forward) {
                Some((a, _, _)) => t = a,
                None => {
                    self.dot_recording = None;
                    let pat = self.search_text.clone();
                    return self.error(format!("pattern not found: {pat}"));
                }
            }
        }
        self.operate_to(op, reg, t);
        if self.mode != Mode::Insert
            && let Some(rec) = self.dot_recording.take()
        {
            self.dot = rec;
        }
    }

    pub(crate) fn search_next(&mut self, reverse: bool, count: usize) {
        let Some(re) = self.search.clone() else {
            self.error("no previous search");
            return;
        };
        self.search_visible = true;
        let forward = !reverse;
        let visual = self.mode.is_visual();
        let mut wrapped = false;
        let mut found = false;
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        let sel = b.sel.transform(|r| {
            let mut p = r.head;
            for _ in 0..count.max(1) {
                match search::find(rope, &re, p, forward) {
                    Some((a, _, w)) => {
                        p = a;
                        wrapped |= w;
                        found = true;
                    }
                    None => break,
                }
            }
            if visual { Range::new(r.anchor, p) } else { Range::point(p) }
        });
        b.sel = sel;
        if !found {
            let t = self.search_text.clone();
            self.error(format!("pattern not found: {t}"));
        } else if wrapped {
            self.info(if forward {
                "search hit BOTTOM, continuing at TOP"
            } else {
                "search hit TOP, continuing at BOTTOM"
            });
        }
    }

    // ----- prompt -----

    pub fn open_prompt(&mut self, kind: PromptKind) {
        let b = self.buf();
        self.prompt = Some(Prompt {
            kind,
            text: String::new(),
            cursor: 0,
            saved_sel: b.sel.clone(),
            saved_top: b.view.top,
            history_pos: None,
            count: None,
            op: None,
            label: String::new(),
        });
    }

    fn prompt_key(&mut self, k: KeyPress) {
        self.prompt_key_inner(k);
        if self.prompt.is_none() {
            // a command line typed after insert-mode Ctrl+O goes back to inserting
            self.finish_insert_once();
        }
    }

    fn prompt_key_inner(&mut self, k: KeyPress) {
        let Some(p) = self.prompt.as_mut() else { return };
        // the sidebar's delete question takes one key
        if p.kind == PromptKind::Ask(crate::tree::Ask::Delete) {
            self.prompt = None;
            if matches!(k.key, Key::Char('y' | 'Y')) {
                self.tree_answer(crate::tree::Ask::Delete, "");
            }
            return;
        }
        if p.op.is_some() {
            // `d/foo<CR>` repeats with `.` as a whole
            if let Some(rec) = &mut self.dot_recording {
                rec.push(k);
            }
        }
        let m = k.mods;
        let byte = |s: &str, c: usize| s.char_indices().nth(c).map_or(s.len(), |(b, _)| b);
        match k.key {
            Key::Esc => {
                self.dot_recording = None;
                let p = self.prompt.take().unwrap();
                if let PromptKind::Search { .. } = p.kind {
                    let b = self.buf_mut();
                    b.sel = p.saved_sel;
                    b.view.top = p.saved_top;
                }
                return;
            }
            Key::Char('c' | '[') if m.ctrl => {
                self.prompt = None;
                self.dot_recording = None;
                return;
            }
            Key::Enter => {
                let p = self.prompt.take().unwrap();
                match p.kind {
                    PromptKind::Command => {
                        if !p.text.trim().is_empty() {
                            self.cmd_history.retain(|h| h != &p.text);
                            self.cmd_history.push(p.text.clone());
                        }
                        match p.count {
                            // `3:` is `:.,.+2`
                            Some(n) if n > 1 => {
                                self.run_ex(&format!(".,.+{}{}", n - 1, p.text.trim_start_matches(':')))
                            }
                            _ => self.run_ex(&p.text),
                        }
                    }
                    PromptKind::Ask(ask) => self.tree_answer(ask, &p.text),
                    PromptKind::Search { forward } => {
                        if p.op.is_none() {
                            self.push_jump();
                        }
                        let pat = if p.text.is_empty() {
                            self.search_history.last().cloned().unwrap_or_default()
                        } else {
                            p.text.clone()
                        };
                        if !pat.is_empty() {
                            self.search_history.retain(|h| h != &pat);
                            self.search_history.push(pat.clone());
                            self.set_search(&pat);
                            let b = self.buf_mut();
                            b.sel = p.saved_sel;
                            let count = p.count.unwrap_or(1);
                            match p.op {
                                Some((op, reg)) => self.search_operate(op, reg, forward, count),
                                None => self.search_next(!forward, count),
                            }
                            self.last_search_forward = forward;
                        }
                    }
                }
                return;
            }
            Key::Backspace if p.text.is_empty() => {
                self.prompt_key(KeyPress::plain(Key::Esc));
                return;
            }
            Key::Backspace if m.ctrl => {
                let b = byte(&p.text, p.cursor);
                let start = p.text[..b].trim_end().rfind(|c: char| !text::is_word_char(c)).map_or(0, |i| i + 1);
                let n = p.text[start..b].chars().count();
                p.text.replace_range(start..b, "");
                p.cursor -= n;
            }
            Key::Char('w') if m.ctrl => {
                return self.prompt_key(KeyPress::new(Key::Backspace, Mods { ctrl: true, ..Mods::default() }));
            }
            Key::Char('u') if m.ctrl => {
                let b = byte(&p.text, p.cursor);
                p.text.replace_range(..b, "");
                p.cursor = 0;
            }
            Key::Backspace => {
                if p.cursor > 0 {
                    let b = byte(&p.text, p.cursor - 1);
                    p.text.remove(b);
                    p.cursor -= 1;
                }
            }
            Key::Delete => {
                if p.cursor < p.text.chars().count() {
                    let b = byte(&p.text, p.cursor);
                    p.text.remove(b);
                }
            }
            Key::Left => p.cursor = p.cursor.saturating_sub(1),
            Key::Right => p.cursor = (p.cursor + 1).min(p.text.chars().count()),
            Key::Home => p.cursor = 0,
            Key::End => p.cursor = p.text.chars().count(),
            Key::Up | Key::Down => {
                let hist = if p.kind == PromptKind::Command { &self.cmd_history } else { &self.search_history };
                if hist.is_empty() {
                    return;
                }
                let pos = match (p.history_pos, k.key == Key::Up) {
                    (None, true) => Some(hist.len() - 1),
                    (None, false) => None,
                    (Some(i), true) => Some(i.saturating_sub(1)),
                    (Some(i), false) => (i + 1 < hist.len()).then_some(i + 1),
                };
                p.history_pos = pos;
                p.text = pos.map(|i| hist[i].clone()).unwrap_or_default();
                p.cursor = p.text.chars().count();
            }
            Key::Tab if p.kind == PromptKind::Command => {
                let completed = complete_command(&p.text, &self.cwd);
                if let Some(t) = completed {
                    p.text = t;
                    p.cursor = p.text.chars().count();
                }
            }
            _ => {
                if let Some(c) = k.text() {
                    let b = byte(&p.text, p.cursor);
                    p.text.insert(b, c);
                    p.cursor += 1;
                } else {
                    return;
                }
            }
        }
        // incremental search
        if let Some(p) = &self.prompt
            && let PromptKind::Search { forward } = p.kind
        {
            let pat = p.text.clone();
            let saved = p.saved_sel.clone();
            let top = p.saved_top;
            self.search = self.compile_search(&pat);
            self.search_text = pat;
            self.search_visible = true;
            let b = self.buf_mut();
            b.sel = saved.clone();
            b.view.top = top;
            if let Some(re) = &self.search {
                let rope = &self.buffers[self.active].doc.rope;
                // the preview looks a few MB around the cursor; Enter searches the whole file
                if let Some((a, _, _)) = search::find_near(rope, re, saved.primary().head, forward, 4 << 20) {
                    let visual = self.mode.is_visual();
                    let b = self.buf_mut();
                    b.sel = if visual {
                        Selection::single(Range::new(saved.primary().anchor, a))
                    } else {
                        Selection::point(a)
                    };
                }
            }
        }
    }

    /// Runs an ex command line (`:w`, `:%s/a/b/g`, `:42`, `:'<,'>d`, `:g/pat/d`, `:set nowrap`, ...).
    pub fn run_ex(&mut self, line: &str) {
        // only leading blanks go: `:normal` takes the rest of the line literally
        let line = line.trim_start().trim_start_matches(':').trim_start();
        if line.trim().is_empty() {
            return;
        }
        // `:nnoremap jk <Esc>` and friends, typed at runtime
        if self.run_map_command(line) {
            return;
        }
        let (range, rest) = match self.ex_range(line) {
            Ok(r) => r,
            Err(e) => return self.error(e),
        };
        let rest = rest.trim_start();
        if rest.trim().is_empty() {
            // a bare address jumps to it
            if let Some((_, b)) = range {
                self.push_jump();
                self.goto_line(b + 1);
            }
            return;
        }
        if self.ex_ranged(range, rest) {
            return;
        }
        let line = rest;
        let (cmd, arg) = match line.find(char::is_whitespace) {
            Some(i) => (&line[..i], line[i..].trim()),
            None => (line, ""),
        };
        let arg_opt = (!arg.is_empty()).then_some(arg);
        match cmd {
            "w" | "write" => self.save(arg_opt),
            "w!" => self.save(arg_opt),
            "wa" | "wall" => self.run(Command::SaveAll),
            "q" | "quit" | "close" => {
                if self.buffers.len() > 1 && !self.buf().doc.dirty {
                    self.close_buffer(false)
                } else {
                    self.quit(false)
                }
            }
            "q!" | "quit!" => self.quit(true),
            "qa" | "qall" => self.quit(false),
            "qa!" | "qall!" => self.quit(true),
            "wq" | "x" | "xit" => {
                if self.buf().doc.dirty || cmd == "wq" {
                    self.save(arg_opt);
                }
                if self.message.as_ref().is_some_and(|m| m.1) {
                    return;
                }
                if self.buffers.len() > 1 { self.close_buffer(false) } else { self.quit(false) }
            }
            "wqa" | "xa" => {
                self.run(Command::SaveAll);
                self.quit(false);
            }
            "e" | "edit" | "o" | "open" => match arg_opt {
                Some(p) => {
                    if let Err(e) = self.open(Path::new(p)) {
                        self.error(format!("{p}: {e}"));
                    }
                }
                None => self.run(Command::Reload),
            },
            "e!" => {
                self.buf_mut().doc.dirty = false;
                self.run(Command::Reload);
            }
            "enew" | "new" => self.run(Command::NewFile),
            "bn" | "bnext" => self.run(Command::NextBuffer),
            "bp" | "bprev" | "bprevious" => self.run(Command::PrevBuffer),
            "bd" | "bdelete" => self.close_buffer(false),
            "bd!" => self.close_buffer(true),
            "b" | "buffer" => {
                if let Ok(n) = arg.parse::<usize>() {
                    self.switch_to(n.saturating_sub(1));
                } else if let Some(i) = self.buffers.iter().position(|b| b.doc.display_name().contains(arg)) {
                    self.switch_to(i);
                } else {
                    self.open_buffer_picker();
                }
            }
            "ls" | "buffers" => self.open_buffer_picker(),
            "noh" | "nohlsearch" => self.search_visible = false,
            "set" | "se" => self.set_option(arg),
            "features" => self.open_feature_picker(),
            "cd" => {
                let p = self.absolute(Path::new(if arg.is_empty() { "~/" } else { arg }));
                if p.is_dir() {
                    self.set_project(&p);
                    let d = self.cwd.display().to_string();
                    self.info(d);
                } else {
                    self.error("not a directory");
                }
            }
            "pwd" => {
                let d = self.cwd.display().to_string();
                self.info(d);
            }
            "update" => self.run(Command::InstallUpdate),
            "version" => self.info(format!("shrew {}", env!("CARGO_PKG_VERSION"))),
            "theme" | "colorscheme" | "colo" => match arg.split_once(char::is_whitespace) {
                Some(("import", path)) => self.events.push(Event::ThemeImport(path.trim().to_string())),
                _ if arg.is_empty() => self.events.push(Event::OpenChoice(Choice::Theme)),
                _ => self.events.push(Event::Choose(Choice::Theme, arg.to_string())),
            },
            "accent" if arg.is_empty() => self.events.push(Event::OpenChoice(Choice::Accent)),
            "accent" => self.events.push(Event::Choose(Choice::Accent, arg.to_string())),
            "font" if arg.is_empty() => self.events.push(Event::OpenChoice(Choice::Font)),
            "font" => self.events.push(Event::Choose(Choice::Font, arg.to_string())),
            "grep" | "rg" | "vimgrep" | "Rg" => self.open_grep(arg),
            "Ex" | "Explore" | "Lex" | "NERDTree" => self.focus_sidebar(true),
            "term" | "terminal" => self.terminal(Command::NewTerminal, arg_opt.map(String::from)),
            "syntax" => match arg {
                "on" | "enable" => self.set_option("syntax"),
                "off" => self.set_option("nosyntax"),
                _ => self.run(Command::ToggleSyntax),
            },
            other => match Command::from_name(other) {
                Some(c) => self.run(c),
                None => self.error(format!("not an editor command: {other}")),
            },
        }
    }

    fn set_option(&mut self, arg: &str) {
        for opt in arg.split_whitespace() {
            if let Some((k, v)) = opt.split_once('=') {
                let n: usize = match v.parse() {
                    Ok(n) if n > 0 && n <= 16 => n,
                    _ => {
                        self.error(format!("invalid value: {opt}"));
                        continue;
                    }
                };
                match k {
                    "ts" | "tabstop" | "sw" | "shiftwidth" | "tw" | "tab_width" => self.settings.tab_width = n,
                    "so" | "scrolloff" => self.settings.scrolloff = n,
                    _ => self.error(format!("unknown option: {k}")),
                }
                continue;
            }
            match opt {
                "et" | "expandtab" => self.settings.expand_tab = true,
                "noet" | "noexpandtab" => self.settings.expand_tab = false,
                "nu" | "number" => self.set_feature(Feature::LineNumbers, true),
                "nonu" | "nonumber" => self.set_feature(Feature::LineNumbers, false),
                "rnu" | "relativenumber" => self.set_feature(Feature::RelativeNumbers, true),
                "nornu" | "norelativenumber" => self.set_feature(Feature::RelativeNumbers, false),
                "hls" | "hlsearch" => self.set_feature(Feature::SearchHighlight, true),
                "nohls" | "nohlsearch" => self.set_feature(Feature::SearchHighlight, false),
                "list" => self.set_feature(Feature::Whitespace, true),
                "nolist" => self.set_feature(Feature::Whitespace, false),
                "lig" | "ligatures" => self.events.push(Event::Ligatures(Some(true))),
                "nolig" | "noligatures" => self.events.push(Event::Ligatures(Some(false))),
                "lig!" | "ligatures!" | "invligatures" => self.events.push(Event::Ligatures(None)),
                _ => {
                    let (key, on) = match opt.strip_suffix('!') {
                        Some(k) => (k, None),
                        None => match opt.strip_prefix("no").filter(|k| features::by_key(k).is_some()) {
                            Some(k) => (k, Some(false)),
                            None => (opt, Some(true)),
                        },
                    };
                    match features::by_key(key) {
                        Some(info) => match on {
                            Some(on) => self.set_feature(info.feature, on),
                            None => self.toggle_feature(info.feature),
                        },
                        None => self.error(format!("unknown option: {opt}")),
                    }
                }
            }
        }
    }

    pub fn set_feature(&mut self, f: Feature, on: bool) {
        if self.features.on(f) != on {
            self.toggle_feature(f);
        }
    }

    /// Lines of the visual selection (or the last one, for `'<,'>` typed later).
    fn visual_lines(&self) -> Option<(usize, usize)> {
        let rope = &self.buf().doc.rope;
        let (from, to) = if self.mode.is_visual() {
            let spans = self.selection_spans();
            (spans.first()?.0, spans.last()?.1.saturating_sub(1))
        } else {
            let (_, sel) = self.last_visual.as_ref()?;
            let rs = sel.ranges();
            (rs.first()?.from(), rs.last()?.to())
        };
        let a = text::line_of(rope, from);
        Some((a, text::line_of(rope, to.max(from))))
    }

    /// One ex address (`12`, `.`, `$`, `'a`, `'<`, `/pat/`, `?pat?`, with `+N`/`-N` offsets) and the rest of the line.
    fn ex_address<'a>(&self, s: &'a str, cur: usize) -> Result<(Option<usize>, &'a str), String> {
        let rope = &self.buf().doc.rope;
        let last = text::last_line(rope);
        let mut rest = s;
        let mut base = None;
        match rest.chars().next() {
            Some(c) if c.is_ascii_digit() => {
                let n = rest.chars().take_while(char::is_ascii_digit).count();
                base = Some(rest[..n].parse::<usize>().unwrap_or(usize::MAX).saturating_sub(1));
                rest = &rest[n..];
            }
            Some('.') => {
                base = Some(cur);
                rest = &rest[1..];
            }
            Some('$') => {
                base = Some(last);
                rest = &rest[1..];
            }
            Some('\'') => {
                let m = rest[1..].chars().next().ok_or("missing mark")?;
                base = Some(match m {
                    '<' => self.visual_lines().ok_or("no visual selection")?.0,
                    '>' => self.visual_lines().ok_or("no visual selection")?.1,
                    m => {
                        let p = *self.marks.get(&(self.buf().id, m)).ok_or_else(|| format!("mark not set: {m}"))?;
                        text::line_of(rope, p)
                    }
                });
                rest = &rest[1 + m.len_utf8()..];
            }
            Some(d @ ('/' | '?')) => {
                let body = &rest[1..];
                let end = split_unescaped(body, d).first().map_or(body.len(), |p| p.len());
                let pat = &body[..end];
                rest = body.get(end + 1..).unwrap_or("");
                let pat = if pat.is_empty() { self.search_text.clone() } else { pat.to_string() };
                let re = search::compile_vim(&pat).ok_or("invalid pattern")?;
                // searches start on the line after (before) the cursor line
                let from = if d == '/' { text::line_end(rope, cur) } else { text::line_start(rope, cur) };
                let (a, _, _) =
                    search::find(rope, &re, from, d == '/').ok_or_else(|| format!("pattern not found: {pat}"))?;
                base = Some(text::line_of(rope, a));
            }
            _ => {}
        }
        // offsets: `+3`, `-`, `.+1`
        while let Some(sign @ ('+' | '-')) = rest.chars().next() {
            let n = rest[1..].chars().take_while(char::is_ascii_digit).count();
            let k = if n == 0 { 1 } else { rest[1..=n].parse::<usize>().unwrap_or(0) };
            let b = base.unwrap_or(cur);
            base = Some(if sign == '+' { b.saturating_add(k) } else { b.saturating_sub(k) });
            rest = &rest[1 + n..];
        }
        Ok((base.map(|b| b.min(last)), rest))
    }

    /// The range at the start of an ex command (`%`, `a,b`, `a;b`, `'<,'>`), as 0-based lines.
    fn ex_range<'a>(&self, s: &'a str) -> Result<LineRange<'a>, String> {
        if let Some(rest) = s.strip_prefix('%') {
            return Ok((Some((0, text::last_line(&self.buf().doc.rope))), rest));
        }
        let cur = self.buf().cursor_line();
        let (a, rest) = self.ex_address(s, cur)?;
        let Some(rest2) = rest.strip_prefix([',', ';']) else {
            return Ok((a.map(|a| (a, a)), rest));
        };
        let a = a.unwrap_or(cur);
        // `a;b`: b is relative to a
        let (b, rest) = self.ex_address(rest2, if rest.starts_with(';') { a } else { cur })?;
        let b = b.unwrap_or(cur);
        Ok((Some((a.min(b), a.max(b))), rest))
    }

    /// Ex commands that take a line range. Returns false for any other command.
    fn ex_ranged(&mut self, range: Option<(usize, usize)>, cmd: &str) -> bool {
        let cur = self.buf().cursor_line();
        let default = if self.mode.is_visual() { self.visual_lines().unwrap_or((cur, cur)) } else { (cur, cur) };
        let (a, b) = range.unwrap_or(default);
        let name_len = cmd.chars().take_while(|c| c.is_ascii_alphabetic()).count();
        let (name, arg) = cmd.split_at(name_len);
        // `:s/a/b/`, `:s#a#b#`: any non-alphanumeric delimiter right after the name
        let subst_arg = |n: &str| matches!(n, "s" | "su" | "sub" | "substitute");
        if subst_arg(name)
            && arg.chars().next().is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace() && c != '"' && c != '|')
        {
            self.substitute(arg, a, b);
            return true;
        }
        if name.is_empty() && cmd.starts_with('&') {
            // `:&` repeats the last :s without its flags, `:&&` with them
            self.repeat_substitute(a, b, cmd.starts_with("&&"));
            return true;
        }
        let bang = arg.starts_with('!');
        let raw_arg = arg.trim_start_matches('!').trim_start();
        let arg = raw_arg.trim();
        match name {
            "d" | "de" | "del" | "dele" | "delet" | "delete" => {
                let reg = arg.chars().next();
                let lines: Vec<usize> = (a..=b).collect();
                let rope = &self.buf().doc.rope;
                let text: Vec<String> = lines.iter().map(|&l| text::line_text(rope, l)).collect();
                self.write_register(
                    reg,
                    Register { parts: vec![text.join("\n")], linewise: true, blockwise: false },
                    false,
                );
                self.exit_visual();
                ops::delete_lines(self.buf_mut(), &lines);
                let rope = &self.buf().doc.rope;
                let p = text::first_non_blank(rope, a.min(text::last_line(rope)));
                self.buf_mut().sel = Selection::point(p);
                self.commit();
            }
            "y" | "ya" | "yank" => {
                let reg = arg.chars().next();
                let rope = &self.buf().doc.rope;
                let text: Vec<String> = (a..=b).map(|l| text::line_text(rope, l)).collect();
                self.write_register(
                    reg,
                    Register { parts: vec![text.join("\n")], linewise: true, blockwise: false },
                    true,
                );
                self.exit_visual();
            }
            "" if cmd.starts_with(['>', '<']) => {
                let levels = cmd.chars().take_while(|&c| c == cmd.as_bytes()[0] as char).count();
                let lines: Vec<usize> = (a..=b).collect();
                let opts = self.opts();
                self.exit_visual();
                for _ in 0..levels {
                    if cmd.starts_with('>') {
                        ops::indent_lines(self.buf_mut(), &lines, &opts);
                    } else {
                        ops::outdent_lines(self.buf_mut(), &lines, &opts);
                    }
                }
                let rope = &self.buf().doc.rope;
                let p = text::first_non_blank(rope, b);
                self.buf_mut().sel = Selection::point(p);
                self.commit();
            }
            "j" | "jo" | "joi" | "join" => {
                let b = if range.is_some_and(|(x, y)| x != y) { b } else { b + 1 };
                let lines: Vec<usize> = (a..=b.min(text::last_line(&self.buf().doc.rope))).collect();
                self.exit_visual();
                if lines.len() > 1 {
                    ops::join_lines(self.buf_mut(), &lines, !bang, false);
                    let rope = &self.buf().doc.rope;
                    self.buf_mut().sel = Selection::point(text::first_non_blank(rope, a));
                    self.commit();
                }
            }
            "m" | "mo" | "move" | "t" | "co" | "cop" | "copy" => {
                // `0` is "above the first line"
                let dest = if arg == "0" {
                    Some(-1)
                } else {
                    match self.ex_address(arg, self.buf().cursor_line()) {
                        Ok((Some(d), _)) => Some(d as isize),
                        _ => None,
                    }
                };
                let Some(dest) = dest else {
                    self.error("invalid address");
                    return true;
                };
                self.exit_visual();
                self.move_or_copy_lines(a, b, dest, name.starts_with('m'));
            }
            "g" | "gl" | "global" | "v" | "vg" | "vglobal" => {
                let invert = name.starts_with('v') || bang;
                let (a, b) = range.unwrap_or((0, text::last_line(&self.buf().doc.rope)));
                self.global(a, b, arg, invert);
            }
            "norm" | "norma" | "normal" => {
                // the rest of the line is taken literally, trailing blanks too
                let keys = KeyPress::parse_seq(raw_arg);
                self.exit_visual();
                let id = self.buf().id;
                self.buf_mut().hold_history();
                for l in a..=b {
                    let rope = &self.buf().doc.rope;
                    if l > text::last_line(rope) {
                        break;
                    }
                    let p = text::line_start(rope, l);
                    self.buf_mut().sel = Selection::point(p);
                    for &k in &keys {
                        self.dispatch_key(k);
                    }
                    if self.mode != Mode::Normal {
                        self.dispatch_key(KeyPress::plain(Key::Esc));
                    }
                    self.pending.clear();
                }
                if let Some(buf) = self.buffers.iter_mut().find(|x| x.id == id) {
                    buf.release_history();
                }
            }
            _ => return false,
        }
        true
    }

    /// `&`, `:&&`, `g&`: the last substitution again on lines `a..=b`, with its flags or without.
    pub(crate) fn repeat_substitute(&mut self, a: usize, b: usize, keep_flags: bool) {
        let Some(last) = self.registers.get(&LAST_SUBSTITUTE).cloned() else {
            return self.error("no previous substitute");
        };
        let [pat, rep, flags] = &last.parts[..] else { return };
        let flags = if keep_flags { flags.as_str() } else { "" };
        // a delimiter that cannot clash with the pattern
        self.substitute(&format!("\u{1}{pat}\u{1}{rep}\u{1}{flags}"), a, b);
    }

    /// `@:`: the last command line again.
    pub(crate) fn repeat_last_ex(&mut self) {
        match self.cmd_history.last().cloned() {
            Some(cmd) => self.run_ex(&cmd),
            None => self.error("no previous command line"),
        }
    }

    /// `:m` / `:t`: moves (or copies) lines `a..=b` below line `dest` (-1: above the first line).
    fn move_or_copy_lines(&mut self, a: usize, b: usize, dest: isize, mv: bool) {
        if mv && dest >= a as isize && dest <= b as isize {
            return; // onto itself
        }
        let rope = &self.buf().doc.rope;
        let block: Vec<String> = (a..=b).map(|l| text::line_text(rope, l)).collect();
        let block = block.join("\n");
        let mut changes = Vec::new();
        // insert first (positions in the original text), then delete
        let ins = if dest < 0 {
            crate::change::Change::insert(0, format!("{block}\n"))
        } else {
            crate::change::Change::insert(text::line_end(rope, dest as usize), format!("\n{block}"))
        };
        changes.push(ins.clone());
        if mv {
            let last = text::last_line(rope);
            let (from, to) = if b < last {
                (text::line_start(rope, a), text::line_start(rope, b + 1))
            } else if a > 0 {
                (text::line_end(rope, a - 1), rope.len_chars()) // the last line takes the break before it
            } else {
                (0, rope.len_chars())
            };
            changes.push(crate::change::Change::delete(from, to));
        }
        let cs = crate::change::ChangeSet::new(changes);
        // the cursor goes to the last line put
        let start = cs.map(ins.from, crate::change::Assoc::Before) + usize::from(dest >= 0);
        let n = b - a;
        let b_ = self.buf_mut();
        b_.apply(cs, None);
        let rope = &b_.doc.rope;
        let first = text::line_of(rope, start.min(rope.len_chars()));
        b_.sel = Selection::point(text::first_non_blank(rope, (first + n).min(text::last_line(rope))));
        self.commit();
    }

    /// `:g/pat/cmd` (and `:v`): runs `cmd` on every line in `a..=b` that matches (or not). `d` is done in one go;
    /// other commands run bottom-up so earlier line numbers stay valid.
    fn global(&mut self, a: usize, b: usize, arg: &str, invert: bool) {
        let Some(delim) = arg.chars().next() else { return self.error("usage: :g/pattern/command") };
        let body = &arg[delim.len_utf8()..];
        let parts = split_unescaped(body, delim);
        let pat = parts.first().copied().unwrap_or("");
        let cmd = body.get(pat.len() + delim.len_utf8()..).unwrap_or("").trim();
        let pat = if pat.is_empty() { self.search_text.clone() } else { pat.to_string() };
        let Some(re) = search::compile_vim(&pat) else { return self.error("invalid pattern") };
        let rope = &self.buf().doc.rope;
        let lines: Vec<usize> = (a..=b.min(text::last_line(rope)))
            .filter(|&l| {
                let line = rope.line(l);
                let s: std::borrow::Cow<str> = line.into();
                re.is_match(s.strip_suffix('\n').unwrap_or(&s)) != invert
            })
            .collect();
        self.set_search(&pat);
        self.exit_visual();
        if lines.is_empty() {
            return self.error(format!("pattern not found: {pat}"));
        }
        let cmd = if cmd.is_empty() { "p" } else { cmd };
        let last_hit = *lines.last().unwrap();
        if matches!(cmd, "d" | "de" | "del" | "delete") {
            ops::delete_lines(self.buf_mut(), &lines);
            // the cursor ends on the line after the last deleted one
            let rope = &self.buf().doc.rope;
            let line = (last_hit + 1 - lines.len()).min(text::last_line(rope));
            self.buf_mut().sel = Selection::point(text::first_non_blank(rope, line));
            self.commit();
            return self.info(format!("{} fewer lines", lines.len()));
        }
        if cmd == "p" {
            return self.info(format!("{} matching lines", lines.len()));
        }
        // bottom-up keeps the line numbers above valid; one undo step for all of it
        let ok = self.held(|e| lines.iter().rev().all(|&l| e.ex_ranged(Some((l, l)), cmd)));
        if !ok {
            return self.error(format!("not supported in :g: {cmd}"));
        }
        let rope = &self.buf().doc.rope;
        let p = text::first_non_blank(rope, last_hit.min(text::last_line(rope)));
        self.buf_mut().sel = Selection::point(p);
    }

    pub fn goto_line(&mut self, n: usize) {
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        let line = n.saturating_sub(1).min(text::last_line(rope));
        let p = text::first_non_blank(rope, line);
        b.sel = Selection::point(p);
        if self.mode.is_visual() || self.mode == Mode::Insert && self.vim() {
            self.set_mode(Mode::Normal);
        }
        self.buf_mut().center_on_cursor();
    }

    /// [`Editor::goto_line`], then 1-based char column `col` of that line (clamped to its end).
    pub fn goto_line_col(&mut self, n: usize, col: Option<usize>) {
        self.goto_line(n);
        let Some(col) = col else { return };
        let caret = self.mode == Mode::Insert;
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        let line = text::line_of(rope, b.sel.primary().head);
        let pos = text::line_start(rope, line) + col.saturating_sub(1).min(text::line_len(rope, line));
        b.sel = Selection::point(if caret { pos } else { text::clamp_normal(rope, pos) });
        b.center_on_cursor();
    }

    /// `:s/pat/rep/flags` on lines `a..=b`. `cmd` starts at the delimiter. Patterns are vim regexes; flags `g`
    /// (every match), `i`/`I` (ignore/match case), `n` (count only), `e` (no error when nothing matches).
    fn substitute(&mut self, cmd: &str, a: usize, b: usize) {
        let Some(delim) = cmd.chars().next() else { return };
        let parts: Vec<&str> = split_unescaped(&cmd[delim.len_utf8()..], delim);
        let (pat, rep, flags) = match parts.as_slice() {
            [p] => (*p, "", ""),
            [p, r] => (*p, *r, ""),
            [p, r, f, ..] => (*p, *r, f.trim()),
            _ => return,
        };
        let pat = if pat.is_empty() { self.search_text.clone() } else { pat.to_string() };
        let case = if flags.contains('i') {
            "\\c"
        } else if flags.contains('I') {
            "\\C"
        } else {
            ""
        };
        let Some(re) = search::compile_vim(&format!("{case}{pat}")) else { return self.error("invalid pattern") };
        // `&`, `g&` and `:&&` repeat it
        self.registers.insert(
            LAST_SUBSTITUTE,
            Register {
                parts: vec![pat.clone(), rep.to_string(), flags.to_string()],
                linewise: false,
                blockwise: false,
            },
        );
        let rep = search::Replacement::parse(rep);
        let global = flags.contains('g');
        let rope = &self.buf().doc.rope;
        let b = b.min(text::last_line(rope));
        let mut changes = Vec::new();
        let mut count = 0;
        let mut hit_line = None;
        let mut out = String::new();
        // one change per match (the scan skips lines without one at memchr speed)
        search::for_each_match(rope, &re, (a, b), true, true, |line, from, to, caps| {
            if !global && hit_line == Some(line) {
                return;
            }
            hit_line = Some(line);
            out.clear();
            if let Some(caps) = caps {
                rep.expand(caps, &mut out);
            }
            changes.push(crate::change::Change::replace(from, to, out.as_str()));
            count += 1;
        });
        let first_start = changes.first().map_or(0, |c| text::line_start(rope, text::line_of(rope, c.from)));
        let Some(last_start) = changes.last().map(|c| c.from).filter(|_| count > 0) else {
            if !flags.contains('e') {
                self.error(format!("pattern not found: {pat}"));
            }
            return;
        };
        self.set_search(&pat);
        if flags.contains('n') {
            return self.info(format!("{count} match(es)"));
        }
        let nl = changes.last().map_or(0, |c| c.text.matches('\n').count());
        let cs = crate::change::ChangeSet::new(changes);
        let at = cs.map(last_start, crate::change::Assoc::Before);
        let buf = self.buf_mut();
        // vim starts the change on the first substituted line: undo comes back there
        buf.sel = Selection::point(first_start);
        buf.apply(cs, None);
        // the cursor goes to the last substituted line (its last line when the replacement split it)
        let rope = &buf.doc.rope;
        let line = text::line_of(rope, at) + nl;
        buf.sel = Selection::point(text::first_non_blank(rope, line.min(text::last_line(rope))));
        self.commit();
        self.exit_visual();
        self.info(format!("{count} substitution(s)"));
    }

    // ----- pickers -----

    fn open_palette(&mut self) {
        if !self.features.on(Feature::Palette) {
            return self.error("palette is off (:set palette)");
        }
        let items = crate::commands::COMMANDS
            .iter()
            .map(|(c, name, title)| PickerItem {
                label: (*title).to_string(),
                detail: format!(":{name}"),
                data: PickerData::Command(*c),
            })
            .collect();
        self.picker = Some(Picker::new(PickerKind::Commands, items));
    }

    fn open_buffer_picker(&mut self) {
        let items = self
            .buffers
            .iter()
            .enumerate()
            .map(|(i, b)| PickerItem {
                label: format!("{}{}", b.doc.display_name(), if b.doc.dirty { " ●" } else { "" }),
                detail: b.doc.path.as_ref().map(|p| relative(p, &self.cwd)).unwrap_or_default(),
                data: PickerData::Buffer(i),
            })
            .collect();
        self.picker = Some(Picker::new(PickerKind::Buffers, items));
    }

    pub fn open_feature_picker(&mut self) {
        let items = features::FEATURES
            .iter()
            .map(|i| PickerItem {
                label: feature_label(&self.features, i),
                detail: i.description.into(),
                data: PickerData::Feature(i.feature),
            })
            .collect();
        let mut p = Picker::new(PickerKind::Features, items);
        if let Some(old) = &self.picker
            && old.kind == PickerKind::Features
        {
            p.query = old.query.clone();
            p.refilter();
            p.selected = old.selected.min(p.matches.len().saturating_sub(1));
        }
        self.picker = Some(p);
    }

    /// Opens the picker for `choice` with the app's items, the current value highlighted.
    pub fn open_choice(&mut self, choice: Choice, items: Vec<PickerItem>, selected: usize) {
        let mut p = Picker::new(PickerKind::Choice(choice), items);
        p.selected = selected.min(p.matches.len().saturating_sub(1));
        self.picker = Some(p);
    }

    fn open_file_picker(&mut self) {
        if !self.features.on(Feature::Picker) {
            return self.error("file picker is off (:set picker)");
        }
        #[cfg(feature = "picker")]
        {
            // no project (home folder, /): the recent files instead of everything you own
            let Some(root) = self.project_root() else {
                self.open_recent_files();
                return self.info("no project folder: recent files (open a folder with srw <folder> or Ctrl+Alt+P)");
            };
            let p = Picker::files(&root, self.waker.clone());
            self.picker = Some(p);
        }
        #[cfg(not(feature = "picker"))]
        self.error("file picker is not compiled into this build");
    }

    fn picker_key(&mut self, k: KeyPress) {
        if self.picker.as_ref().is_some_and(|p| p.kind == PickerKind::Grep) && self.grep_key(k) {
            return;
        }
        let Some(p) = self.picker.as_mut() else { return };
        let m = k.mods;
        let query = p.query.clone();
        match k.key {
            Key::Esc => self.picker = None,
            Key::Char('c' | '[') if m.ctrl => self.picker = None,
            // Ctrl+P's `:42`
            Key::Enter if p.kind == PickerKind::Files && p.query.starts_with(':') => {
                let line = p.line_suffix();
                self.picker = None;
                if let Some(n) = line {
                    self.push_jump();
                    self.goto_line(n);
                }
            }
            Key::Enter => {
                let line = p.line_suffix();
                let Some(data) = p.current().map(|i| i.data.clone()) else { return };
                let kind = p.kind;
                if kind != PickerKind::Features {
                    self.picker = None;
                }
                match data {
                    PickerData::Command(c) => self.run(c),
                    PickerData::Buffer(i) => self.switch_to(i),
                    PickerData::Path(path) => {
                        if let Err(e) = self.open(&path) {
                            self.error(format!("{}: {e}", path.display()));
                        } else if let Some(n) = line.filter(|_| self.buf().doc.path.as_ref() == Some(&path)) {
                            self.goto_line(n);
                        }
                    }
                    PickerData::Feature(f) => {
                        self.toggle_feature(f);
                        self.open_feature_picker();
                    }
                    PickerData::Value(v) => {
                        if let PickerKind::Choice(c) = kind {
                            self.events.push(Event::Choose(c, v));
                        }
                    }
                    other => {
                        self.pick_location(&other);
                    }
                }
                self.after_key();
            }
            Key::Up => p.move_by(-1),
            Key::Down | Key::Tab => p.move_by(1),
            Key::Char('p' | 'k') if m.ctrl => p.move_by(-1),
            Key::Char('n' | 'j') if m.ctrl => p.move_by(1),
            Key::PageUp => p.move_by(-10),
            Key::PageDown => p.move_by(10),
            Key::Backspace if m.ctrl => {
                p.query.clear();
                p.refilter();
            }
            Key::Char('u' | 'w') if m.ctrl => {
                p.query.clear();
                p.refilter();
            }
            Key::Backspace => {
                if p.query.pop().is_none() {
                    self.picker = None;
                } else {
                    p.refilter();
                }
            }
            _ => {
                if let Some(c) = k.text() {
                    p.query.push(c);
                    p.refilter();
                }
            }
        }
        if self.picker.as_ref().is_some_and(|p| p.query != query) {
            self.picker_query_changed();
        }
    }

    /// The About panel is read-only: keys scroll it or close it.
    fn about_key(&mut self, k: KeyPress) {
        let Some(top) = self.about.as_mut() else { return };
        let m = k.mods;
        match k.key {
            Key::Esc | Key::Enter | Key::Char('q') => self.about = None,
            Key::Char('c' | '[') if m.ctrl => self.about = None,
            Key::Down | Key::Char('j') => *top += 1,
            Key::Up | Key::Char('k') => *top = top.saturating_sub(1),
            Key::Char('d') if m.ctrl => *top += 10,
            Key::Char('u') if m.ctrl => *top = top.saturating_sub(10),
            Key::PageDown | Key::Char(' ') => *top += 10,
            Key::PageUp => *top = top.saturating_sub(10),
            Key::Home | Key::Char('g') => *top = 0,
            Key::End | Key::Char('G') => *top = usize::MAX / 2,
            _ => {}
        }
    }

    /// Pulls in results from background work (file listing). Returns true when something changed.
    pub fn poll(&mut self) -> bool {
        self.picker.as_mut().is_some_and(|p| p.poll())
    }
}

/// An ex range (0-based lines, None when there was none) and the rest of the command line.
type LineRange<'a> = (Option<(usize, usize)>, &'a str);

/// Hidden register holding the last `:s` (pattern, replacement, flags) for `&`, `g&` and `:&&`.
const LAST_SUBSTITUTE: char = '\u{1}';

/// Moves `r.head` by `delta` lines keeping the sticky column. Returns (new head, column).
pub(crate) fn vertical(rope: &ropey::Rope, r: Range, delta: isize, tab_width: usize) -> (usize, usize) {
    let col = r.col.unwrap_or_else(|| text::col_of(rope, r.head, tab_width));
    let line = text::line_of(rope, r.head) as isize;
    let target = (line + delta).clamp(0, text::last_line(rope) as isize) as usize;
    if target as isize == line && delta != 0 {
        // first/last line: move to its start/end like most editors
        let p = if delta < 0 { 0 } else { rope.len_chars() };
        return (if (delta < 0 && line == 0) || (delta > 0) { p } else { r.head }, col);
    }
    (text::pos_at_col(rope, target, col, tab_width), col)
}

fn feature_label(f: &Features, i: &features::FeatureInfo) -> String {
    let state = if !f.available(i.feature) {
        "n/a"
    } else if f.on(i.feature) {
        "on "
    } else {
        "off"
    };
    format!("[{state}] {}", i.key)
}

fn split_unescaped(s: &str, delim: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == delim {
            out.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    out.push(&s[start..]);
    out
}

/// Path completion for `:e`/`:w` (Tab).
fn complete_command(text: &str, cwd: &Path) -> Option<String> {
    let (cmd, arg) = text.split_once(' ')?;
    if !matches!(cmd, "e" | "edit" | "w" | "write" | "o" | "open" | "cd") {
        return None;
    }
    let (dir_part, prefix) = match arg.rfind('/') {
        Some(i) => (&arg[..=i], &arg[i + 1..]),
        None => ("", arg),
    };
    let dir = if dir_part.starts_with('/') { PathBuf::from(dir_part) } else { cwd.join(dir_part) };
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let slash = if e.file_type().is_ok_and(|t| t.is_dir()) { "/" } else { "" };
            (name.starts_with(prefix) && (!name.starts_with('.') || prefix.starts_with('.')))
                .then(|| format!("{name}{slash}"))
        })
        .collect();
    names.sort();
    let first = names.first()?;
    // longest common prefix
    let common = names
        .iter()
        .fold(first.clone(), |acc, n| acc.chars().zip(n.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect());
    Some(format!("{cmd} {dir_part}{common}"))
}

pub(crate) fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

pub fn relative(p: &Path, base: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).display().to_string()
}
