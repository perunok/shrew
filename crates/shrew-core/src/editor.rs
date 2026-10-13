//! The editor: buffers, mode, registers, prompt and picker, and the key dispatch that ties the vim and Sublime
//! layers together. Everything here is toolkit-free; the app feeds it keys and mouse positions and draws its state.

use crate::commands::Command;
use crate::document::{Buffer, Document};
use crate::features::{self, Feature, Features};
use crate::input::{Key, KeyPress, Mods};
use crate::lang;
use crate::ops::{self, Opts};
use crate::picker::{Picker, PickerData, PickerItem, PickerKind};
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
            Some(body) => Register { parts: vec![body.to_string()], linewise: true },
            None => Register { parts: vec![t], linewise: false },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Command,
    Search { forward: bool },
}

pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    /// Caret as a char index into `text`.
    pub cursor: usize,
    saved_sel: Selection,
    saved_top: usize,
    history_pos: Option<usize>,
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
            message: None,
            search: None,
            search_text: String::new(),
            search_visible: true,
            last_search_forward: true,
            events: Vec::new(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            waker: None,
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

    /// Opens `path` (or switches to it when already open). Missing files open as new, empty buffers.
    pub fn open(&mut self, path: &Path) -> std::io::Result<()> {
        let path = self.absolute(path);
        if let Some(i) = self.buffers.iter().position(|b| b.doc.path.as_deref() == Some(&path)) {
            self.switch_to(i);
            return Ok(());
        }
        let doc = Document::load(&path)?;
        self.install_document(doc);
        Ok(())
    }

    /// Puts a loaded document in a buffer: replaces an untouched scratch buffer, else adds one.
    pub fn install_document(&mut self, doc: Document) {
        let reuse = self.buffers.len() == 1 && self.buffers[0].doc.path.is_none() && !self.buffers[0].doc.dirty && self.buffers[0].doc.is_empty();
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

    pub(crate) fn wake(&self) {
        if let Some(w) = &self.waker {
            w();
        }
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
                        p -= 1;
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
        let mut ranges = Vec::new();
        let mut primary = 0;
        let lines: Vec<usize> = if la <= lh { (la..=lh).collect() } else { (lh..=la).rev().collect() };
        for l in lines {
            let a = text::pos_at_col(rope, l, left, tw);
            if text::col_of(rope, a, tw) < left && text::line_len(rope, l) > 0 {
                continue; // line too short
            }
            let h = text::clamp_normal(rope, text::pos_at_col(rope, l, right, tw)).max(a);
            if l == lh {
                primary = ranges.len();
            }
            ranges.push(if ca <= ch { Range::new(a, h) } else { Range::new(h, a) });
        }
        if ranges.is_empty() {
            ranges.push(Range::point(head));
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
                Mode::Visual | Mode::VisualBlock => Some((r.from(), (r.to() + 1).min(len))),
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
        self.pending.iter().map(|k| k.to_string()).collect()
    }

    pub fn recording_macro(&self) -> Option<char> {
        self.macro_rec.as_ref().map(|m| m.0)
    }

    // ----- registers -----

    pub(crate) fn write_register(&mut self, name: Option<char>, reg: Register, yank: bool) {
        let name = name.unwrap_or('"');
        if name == '_' {
            return;
        }
        if yank {
            self.registers.insert('0', reg.clone());
        }
        if name != '"' && name != '+' && name != '*' {
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
        let from_clipboard = name == '+' || name == '*' || (name == '"' && self.features.on(Feature::Clipboard));
        if from_clipboard {
            if let Some(t) = self.clipboard.get() {
                if let Some(r) = self.registers.get(&'"') {
                    if r.joined() == t {
                        return Some(r.clone());
                    }
                }
                return Some(Register::from_text(t));
            }
        }
        self.registers.get(&name).cloned()
    }

    // ----- input -----

    pub fn handle_key(&mut self, k: KeyPress) {
        if let Some((_, keys)) = &mut self.macro_rec {
            keys.push(k);
        }
        if !self.replaying {
            self.message = None;
        }
        if self.picker.is_some() {
            self.picker_key(k);
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
        if self.prompt.is_some() || self.picker.is_some() {
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
        let b = self.buf_mut();
        b.sel = b.sel.clamp(b.doc.len());
        b.scroll_to_cursor(so, tw);
    }

    /// Key bindings that work in every mode (Sublime-style chords), checked before the vim layer.
    fn binding(&self, k: KeyPress) -> Option<Command> {
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
            _ => return None,
        };
        if !m.ctrl || m.alt {
            return None;
        }
        Some(match (c, m.shift) {
            ('s', false) => Save,
            ('s', true) => SaveAll,
            ('q', _) => Quit,
            ('p', true) => CommandPalette,
            ('p', false) => OpenFile,
            ('o', false) if insert => OpenFile,
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
            ('b', true) => SwitchBuffer,
            ('u', true) => CheckForUpdates,
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
                let n = self.buffers.iter_mut().filter(|b| b.doc.dirty && b.doc.path.is_some()).map(|b| b.doc.save()).filter(Result::is_ok).count();
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
                ops::join_lines(self.buf_mut(), &lines);
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
                self.with_carets(|b, _| {
                    ops::add_next_occurrence(b, word, cmd == SkipOccurrence);
                }, true);
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
            CheckForUpdates => self.events.push(Event::CheckUpdates),
            InstallUpdate => self.events.push(Event::InstallUpdate),
            OpenConfig => self.events.push(Event::OpenConfig),
            McpSetup => self.events.push(Event::McpSetup),
        }
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
                b.sel = Selection::new(spans.iter().zip(back).map(|(&(a, z), bk)| if bk { Range::new(z, a) } else { Range::new(a, z) }), prim);
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
        if !self.buf_mut().undo() {
            self.info("already at oldest change");
        }
        self.fix_cursor_after_history();
    }

    pub fn redo(&mut self) {
        if !self.buf_mut().redo() {
            self.info("already at newest change");
        }
        self.fix_cursor_after_history();
    }

    fn fix_cursor_after_history(&mut self) {
        if self.mode.is_visual() {
            self.set_mode(Mode::Normal);
        }
        if self.mode == Mode::Normal {
            let b = self.buf_mut();
            let rope = &b.doc.rope;
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
            Register { parts: lines.iter().map(|&l| text::line_text(rope, l)).collect(), linewise: true }
        } else {
            Register { parts: spans.iter().map(|&(a, z)| rope.slice(a..z).to_string()).collect(), linewise: self.mode == Mode::VisualLine }
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
        let Some(reg) = self.read_register(None) else { return };
        let n = self.buf().sel.len();
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
        if !force {
            if let Some(b) = self.buffers.iter().find(|b| b.doc.dirty) {
                let name = b.doc.display_name();
                self.error(format!("\"{name}\" has unsaved changes (:q! to discard, :wq to save)"));
                return;
            }
        }
        self.events.push(Event::Quit);
    }

    // ----- insert mode (also the whole of non-vim mode) -----

    fn insert_key(&mut self, k: KeyPress) {
        let opts = self.opts();
        let m = k.mods;
        let tw = self.settings.tab_width;
        if let Some(c) = k.text() {
            ops::insert_char(self.buf_mut(), c, &opts);
            return;
        }
        let shift = m.shift;
        match k.key {
            Key::Esc => self.insert_escape(),
            Key::Char('[') if m.ctrl && self.vim() => self.insert_escape(),
            Key::Char('c') if m.ctrl && self.vim() && self.buf().sel.ranges().iter().all(|r| r.is_empty()) => self.insert_escape(),
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
            Key::Char('w') if m.ctrl => ops::delete_word_back(self.buf_mut()),
            Key::Char('u') if m.ctrl && self.vim() => ops::delete_to_line_start(self.buf_mut()),
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

    fn insert_escape(&mut self) {
        if let Some(rec) = self.dot_recording.take() {
            self.dot = rec;
        }
        if self.vim() {
            self.set_mode(Mode::Normal);
            if std::mem::take(&mut self.block_insert) {
                let b = self.buf_mut();
                b.sel = b.sel.only_primary();
            }
        } else {
            let b = self.buf_mut();
            b.sel = if b.sel.len() > 1 { b.sel.only_primary().collapse() } else { b.sel.collapse() };
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
                Key::Left => r.head.saturating_sub(1),
                Key::Right => (r.head + 1).min(len),
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
        if self.picker.is_some() {
            return;
        }
        self.commit();
        let b = self.buf();
        let rope = &b.doc.rope;
        let pos = pos.min(rope.len_chars());
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
        let pos = pos.min(rope.len_chars());
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
        self.search = search::compile(pattern);
        self.search_visible = true;
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
            self.info(if forward { "search hit BOTTOM, continuing at TOP" } else { "search hit TOP, continuing at BOTTOM" });
        }
    }

    // ----- prompt -----

    pub fn open_prompt(&mut self, kind: PromptKind) {
        let b = self.buf();
        self.prompt = Some(Prompt { kind, text: String::new(), cursor: 0, saved_sel: b.sel.clone(), saved_top: b.view.top, history_pos: None });
    }

    fn prompt_key(&mut self, k: KeyPress) {
        let Some(p) = self.prompt.as_mut() else { return };
        let m = k.mods;
        let byte = |s: &str, c: usize| s.char_indices().nth(c).map_or(s.len(), |(b, _)| b);
        match k.key {
            Key::Esc => {
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
                        self.run_ex(&p.text);
                    }
                    PromptKind::Search { forward } => {
                        let pat = if p.text.is_empty() { self.search_history.last().cloned().unwrap_or_default() } else { p.text.clone() };
                        if !pat.is_empty() {
                            self.search_history.retain(|h| h != &pat);
                            self.search_history.push(pat.clone());
                            self.set_search(&pat);
                            let b = self.buf_mut();
                            b.sel = p.saved_sel;
                            self.search_next(!forward, 1);
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
        if let Some(p) = &self.prompt {
            if let PromptKind::Search { forward } = p.kind {
                let pat = p.text.clone();
                let saved = p.saved_sel.clone();
                let top = p.saved_top;
                self.search = search::compile(&pat);
                self.search_text = pat;
                self.search_visible = true;
                let b = self.buf_mut();
                b.sel = saved.clone();
                b.view.top = top;
                if let Some(re) = &self.search {
                    let rope = &self.buffers[self.active].doc.rope;
                    if let Some((a, _, _)) = search::find(rope, re, saved.primary().head, forward) {
                        let visual = self.mode.is_visual();
                        let b = self.buf_mut();
                        b.sel = if visual { Selection::single(Range::new(saved.primary().anchor, a)) } else { Selection::point(a) };
                    }
                }
            }
        }
    }

    /// Runs an ex command line (`:w`, `:%s/a/b/g`, `:42`, `:set nowrap`, ...).
    pub fn run_ex(&mut self, line: &str) {
        let line = line.trim().trim_start_matches(':');
        if line.is_empty() {
            return;
        }
        if let Ok(n) = line.parse::<usize>() {
            self.goto_line(n);
            return;
        }
        if let Some(rest) = line.strip_prefix('%').filter(|r| r.starts_with('s')) {
            return self.substitute(rest, true);
        }
        if line.starts_with("s/") || line.starts_with("s#") {
            return self.substitute(line, false);
        }
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
                    self.cwd = p;
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

    /// `:s/pat/rep/flags` on the cursor lines, or every line with `%`.
    fn substitute(&mut self, cmd: &str, all_lines: bool) {
        let body = &cmd[1..];
        let Some(delim) = body.chars().next() else { return };
        let parts: Vec<&str> = split_unescaped(&body[delim.len_utf8()..], delim);
        let (pat, rep, flags) = match parts.as_slice() {
            [p] => (*p, "", ""),
            [p, r] => (*p, *r, ""),
            [p, r, f, ..] => (*p, *r, *f),
            _ => return,
        };
        let pat = if pat.is_empty() { self.search_text.clone() } else { pat.to_string() };
        let Some(re) = search::compile(&pat) else { return self.error("invalid pattern") };
        // vim's \1 and & in the replacement
        let mut rep_out = String::new();
        let mut chars = rep.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some(d) if d.is_ascii_digit() => rep_out.push_str(&format!("${{{d}}}")),
                    Some('n') => rep_out.push('\n'),
                    Some('t') => rep_out.push('\t'),
                    Some(o) => rep_out.push(o),
                    None => {}
                },
                '&' => rep_out.push_str("${0}"),
                '$' => rep_out.push_str("$$"),
                c => rep_out.push(c),
            }
        }
        let global = flags.contains('g');
        let b = self.buf();
        let rope = &b.doc.rope;
        let lines: Vec<usize> = if all_lines {
            (0..=text::last_line(rope)).collect()
        } else if self.mode.is_visual() {
            self.target_lines()
        } else {
            ops::selected_lines(rope, &b.sel)
        };
        let mut changes = Vec::new();
        let mut count = 0;
        for l in lines {
            let s = text::line_text(rope, l);
            let new = if global { re.replace_all(&s, rep_out.as_str()) } else { re.replace(&s, rep_out.as_str()) };
            if new != s {
                count += if global { re.find_iter(&s).count() } else { 1 };
                let ls = text::line_start(rope, l);
                changes.push(crate::change::Change::replace(ls, ls + s.chars().count(), new.into_owned()));
            }
        }
        if count == 0 {
            return self.error(format!("pattern not found: {pat}"));
        }
        self.set_search(&pat);
        let b = self.buf_mut();
        b.apply(crate::change::ChangeSet::new(changes), None);
        let rope = &b.doc.rope;
        b.sel = b.sel.transform(|r| Range::point(text::clamp_normal(rope, r.head)));
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
            .map(|(c, name, title)| PickerItem { label: (*title).to_string(), detail: format!(":{name}"), data: PickerData::Command(*c) })
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
            .map(|i| PickerItem { label: feature_label(&self.features, i), detail: i.description.into(), data: PickerData::Feature(i.feature) })
            .collect();
        let mut p = Picker::new(PickerKind::Features, items);
        if let Some(old) = &self.picker {
            if old.kind == PickerKind::Features {
                p.query = old.query.clone();
                p.refilter();
                p.selected = old.selected.min(p.matches.len().saturating_sub(1));
            }
        }
        self.picker = Some(p);
    }

    fn open_file_picker(&mut self) {
        if !self.features.on(Feature::Picker) {
            return self.error("file picker is off (:set picker)");
        }
        #[cfg(feature = "picker")]
        {
            let p = Picker::files(&self.cwd, self.waker.clone());
            self.picker = Some(p);
        }
        #[cfg(not(feature = "picker"))]
        self.error("file picker is not compiled into this build");
    }

    fn picker_key(&mut self, k: KeyPress) {
        let Some(p) = self.picker.as_mut() else { return };
        let m = k.mods;
        match k.key {
            Key::Esc => self.picker = None,
            Key::Char('c' | '[') if m.ctrl => self.picker = None,
            Key::Enter => {
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
                        }
                    }
                    PickerData::Feature(f) => {
                        self.toggle_feature(f);
                        self.open_feature_picker();
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
    }

    /// Pulls in results from background work (file listing). Returns true when something changed.
    pub fn poll(&mut self) -> bool {
        self.picker.as_mut().is_some_and(|p| p.poll())
    }
}

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
            (name.starts_with(prefix) && (!name.starts_with('.') || prefix.starts_with('.'))).then(|| format!("{name}{slash}"))
        })
        .collect();
    names.sort();
    let first = names.first()?;
    // longest common prefix
    let common = names.iter().fold(first.clone(), |acc, n| acc.chars().zip(n.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect());
    Some(format!("{cmd} {dir_part}{common}"))
}

fn normalize(p: &Path) -> PathBuf {
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
