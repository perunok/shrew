//! Everything the app owns except the window: the editor, config, syntax highlighting, background file loads, the
//! updater and the MCP server, plus the handling of the [`Event`]s the editor emits. The windowed app and the
//! headless `--screenshot` mode both drive a [`State`].

mod look;
mod os_theme;

use crate::colors::Colors;
use look::{Look, ThemeCfg};
use shrew_core::features::{self, Feature};
use shrew_core::{Document, Editor, Event, KeyPress};
use shrew_render::FontConfig;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

/// Wakes the UI thread from a background thread (an event-loop proxy in the app, a no-op headless).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Files bigger than this load on a background thread behind a loading indicator; smaller ones are read right
/// away, so opening them never flickers.
pub const BACKGROUND_LOAD_BYTES: u64 = 1 << 20;

pub const MIN_FONT_SIZE: f32 = 6.0;
pub const MAX_FONT_SIZE: f32 = 72.0;

/// How the update button looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "update"), allow(dead_code))]
pub enum Tone {
    Accent,
    Muted,
    Success,
    Error,
}

/// A file being read on a background thread.
pub struct Loading {
    pub path: PathBuf,
    pub bytes: u64,
    pub started: Instant,
    line: Option<usize>,
    col: Option<usize>,
    rx: Receiver<io::Result<Document>>,
}

pub struct State {
    pub ed: Editor,
    pub colors: Colors,
    /// The font the renderer should use; `font_changed` tells the app to hand it over.
    pub font: FontConfig,
    pub font_changed: bool,
    /// Theme, accent, theme pickers, desktop light/dark (see `look`).
    pub look: Look,
    pub cursor_blink: bool,
    /// `[minimap] width`, logical pixels.
    pub minimap_width: f32,
    pub config_path: Option<PathBuf>,
    /// The config file's buffer and the version last applied, to reload it when it is saved.
    config_seen: Option<(u64, u64)>,
    pub wake: Waker,
    pub loading: Vec<Loading>,
    /// `srw --wait` requests whose buffers are still open.
    pub waits: Vec<crate::remote::Wait>,
    /// The editor asked to quit (or the window was closed twice).
    pub quit: bool,
    close_warned: bool,
    headless: bool,
    #[cfg(feature = "syntax")]
    pub syntax: Option<shrew_syntax::Syntaxes>,
    #[cfg(feature = "syntax")]
    pub highlighters: std::collections::HashMap<u64, shrew_syntax::Highlighter>,
    #[cfg(feature = "update")]
    pub updater: Option<shrew_update::Updater>,
    #[cfg(feature = "mcp")]
    pub mcp: Option<shrew_mcp::McpServer>,
    /// How MCP jobs reach the UI thread (set by the windowed app; none headless).
    #[cfg(feature = "mcp")]
    pub mcp_host: Option<Arc<dyn shrew_mcp::Host>>,
    /// The custom title bar: its items, open menu, hover state and window actions (see `titlebar`).
    pub titlebar: crate::titlebar::TitleBar,
    /// The open context menu (see `context_menu`); one menu is open at a time, this or the title bar's.
    pub menu: Option<crate::context_menu::ContextMenu>,
    // ----- project navigation -----
    /// The sidebar's width in logical pixels (`[sidebar] width`, dragged with the mouse).
    pub sidebar_width: f32,
    /// Where recent files and projects are kept, once they were read (after the first frame).
    recent_file: Option<PathBuf>,
    /// The recent lists' generation last written.
    recent_saved: u64,
    /// The terminal panel (empty, no threads, until one is opened).
    #[cfg(feature = "terminal")]
    pub terminals: crate::terminal::Terminals,
    // ----- settings -----
    /// The Settings panel (empty until it is first opened).
    pub settings: crate::settings::Settings,
    /// What the vimrc and Sublime imports read, for the Settings panel.
    pub imports: Vec<shrew_config::ImportReport>,
    /// `[mcp] port` from config.toml, when set: the MCP server listens there.
    pub mcp_port: Option<u16>,
    // ----- git -----
    /// Git: the service, repositories, markers, blame, status, diff views, the Source Control panel, log and blame
    /// column (nothing runs until a file is looked up or a git command runs).
    #[cfg(feature = "git")]
    pub git: crate::git::Git,
}

impl State {
    /// Builds the editor from a loaded configuration. Cheap: nothing here blocks on fonts, grammars or the network.
    pub fn new(loaded: shrew_config::Loaded, wake: Waker, headless: bool) -> State {
        let mut ed = Editor::new();
        mark_unavailable(&mut ed);
        let mut warnings = loaded.warnings.clone();
        warnings.extend(shrew_config::apply(&loaded.config, &mut ed));
        for r in &loaded.imports {
            log::info!("{}", r.summary());
        }
        ed.waker = Some(wake.clone());
        if !headless {
            install_clipboard(&mut ed);
            // `:e`, the file picker, drag and drop: big files come back as Event::Load
            ed.async_load_bytes = Some(BACKGROUND_LOAD_BYTES);
        }
        let cfg = &loaded.config.editor;
        let font = font_config(cfg);
        let mut st = State {
            font,
            font_changed: false,
            look: Look::new(loaded.path.as_deref()),
            cursor_blink: cfg.cursor_blink(),
            minimap_width: cfg.minimap_width() as f32,
            colors: Colors::from_theme(&shrew_theme::Theme::default()),
            config_path: loaded.path.clone(),
            config_seen: None,
            // ----- git -----
            #[cfg(feature = "git")]
            git: crate::git::Git::new(wake.clone(), &loaded.config.git),
            wake,
            loading: Vec::new(),
            waits: Vec::new(),
            quit: false,
            close_warned: false,
            headless,
            ed,
            #[cfg(feature = "syntax")]
            syntax: None,
            #[cfg(feature = "syntax")]
            highlighters: Default::default(),
            #[cfg(feature = "update")]
            updater: None,
            #[cfg(feature = "mcp")]
            mcp: None,
            #[cfg(feature = "mcp")]
            mcp_host: None,
            titlebar: Default::default(),
            menu: None,
            sidebar_width: loaded.config.sidebar_width.unwrap_or(crate::sidebar::DEFAULT_WIDTH),
            recent_file: None,
            recent_saved: 0,
            #[cfg(feature = "terminal")]
            terminals: crate::terminal::Terminals::new(loaded.config.terminal.clone()),
            settings: Default::default(),
            imports: loaded.imports.clone(),
            mcp_port: loaded.config.mcp_port,
        };
        st.titlebar.configure(&loaded.config.titlebar);
        #[cfg(feature = "git")]
        {
            crate::git::default_mappings(&mut st.ed);
            // the window waits for nothing git: it wakes the service after its first frame (`start_deferred`)
            st.git.active = headless;
        }
        #[cfg(feature = "syntax")]
        {
            st.ed.symbols = Some(crate::symbols::provider(st.wake.clone()));
        }
        st.apply_theme_config(ThemeCfg::from(cfg));
        st.show_warnings(&warnings);
        st.ed.events.clear(); // the initial scratch buffer: highlighters are created on demand below
        st
    }

    /// Opens `path` with the cursor on 1-based `line` (and char column `col`): switches to it when it is open
    /// already, reads a big file on a background thread (the cursor moves once it is in), a small one right away.
    pub fn open_at(&mut self, path: &Path, line: Option<usize>, col: Option<usize>) {
        let path = self.ed.absolute(path);
        if let Some(l) = self.loading.iter_mut().find(|l| l.path == path) {
            if line.is_some() {
                (l.line, l.col) = (line, col);
            }
            return;
        }
        let open = self.ed.buffers.iter().any(|b| b.doc.path.as_deref() == Some(path.as_path()));
        if !open && !self.headless {
            let bytes = std::fs::metadata(&path).ok().filter(|m| m.is_file()).map_or(0, |m| m.len());
            if bytes > BACKGROUND_LOAD_BYTES {
                return self.load_in_background(path, bytes, line, col);
            }
        }
        match self.ed.open(&path) {
            Ok(()) => {
                if let Some(n) = line {
                    self.ed.goto_line_col(n, col);
                }
            }
            Err(e) => self.ed.error(format!("{}: {e}", path.display())),
        }
    }

    fn load_in_background(&mut self, path: PathBuf, bytes: u64, line: Option<usize>, col: Option<usize>) {
        let (tx, rx) = mpsc::channel();
        let wake = self.wake.clone();
        let p = path.clone();
        let spawned = std::thread::Builder::new().name("shrew-load".into()).spawn(move || {
            let _ = tx.send(Document::load(&p));
            wake();
        });
        match spawned {
            Ok(_) => self.loading.push(Loading { path, bytes, started: Instant::now(), line, col, rx }),
            Err(e) => self.ed.error(format!("{}: {e}", path.display())),
        }
    }

    /// A menu is open (a context menu or the title bar's): it takes the keys and the clicks.
    pub fn menu_open(&self) -> bool {
        self.menu.is_some() || self.titlebar.menu.is_some()
    }

    /// A key from the keyboard (or `--keys`).
    pub fn key(&mut self, k: KeyPress) {
        // an open context menu takes every key
        if crate::context_menu::key(self, k) {
            return;
        }
        let menu = self.titlebar.menu.is_some();
        // the Settings panel takes the keys, unless one of its pickers or a title bar menu is open over it
        if !menu && self.settings.is_open() && self.ed.picker.is_none() {
            self.settings_key(k);
            return self.after_input();
        }
        // a focused terminal (Alt+F, F10 belong to the program in it); then an open menu or a key that opens one
        #[cfg(feature = "terminal")]
        if !menu && self.terminal_key(k) {
            return;
        }
        if crate::titlebar::key(self, k) {
            return;
        }
        // ----- git: Esc closes the hunk peek; a diff tab takes the keys the editor would (but not its prompt,
        // pickers or chords) -----
        #[cfg(feature = "git")]
        if crate::git::key(self, k, menu) {
            return self.after_input();
        }
        self.ed.handle_key(k);
        self.after_input();
    }

    /// Text typed through an input method.
    pub fn text(&mut self, s: &str) {
        if self.menu_open() || (self.settings.is_open() && self.ed.picker.is_none()) {
            return s.chars().for_each(|c| self.key(KeyPress::ch(c)));
        }
        #[cfg(feature = "terminal")]
        if self.terminal_text(s) {
            return;
        }
        // ----- git: the commit box or the git prompt; nothing types into a diff tab, its keys come as keys -----
        #[cfg(feature = "git")]
        {
            if crate::git::text(self, s) {
                return self.after_input();
            }
            if crate::git::diffview::is_active(self) && self.ed.prompt.is_none() && self.ed.picker.is_none() {
                return s.chars().for_each(|c| self.key(KeyPress::ch(c)));
            }
        }
        if self.ed.mapping_pending() {
            self.ed.flush_pending_mappings();
        }
        self.ed.insert_text(s);
        self.after_input();
    }

    /// After anything changed the editor: run its events, reload the config if it was just saved.
    pub fn after_input(&mut self) {
        self.process_events();
        // the sidebar just took the keys (Ctrl+Shift+E, :Ex, the palette): a focused terminal gives them up
        #[cfg(feature = "terminal")]
        if self.ed.sidebar_focus && self.terminals.focused {
            self.terminals.set_focus(false);
        }
        self.check_config_saved();
        self.check_waits();
        self.check_theme_saved();
        self.sync_preview();
        self.ed.sync_tree();
        // ----- git -----
        #[cfg(feature = "git")]
        crate::git::after_input(self);
        self.save_recent();
        if self.close_warned && self.ed.buffers.iter().all(|b| !b.doc.dirty) {
            self.close_warned = false;
        }
    }

    /// The window's close button: refuses once while there are unsaved changes.
    pub fn close_requested(&mut self) {
        let dirty: Vec<String> = self.ed.buffers.iter().filter(|b| b.doc.dirty).map(|b| b.doc.display_name()).collect();
        if dirty.is_empty() || self.close_warned {
            self.quit = true;
        } else {
            self.close_warned = true;
            self.ed.error(format!("unsaved changes in {}: close again to discard them (or :wa)", dirty.join(", ")));
        }
    }

    pub fn process_events(&mut self) {
        while !self.ed.events.is_empty() {
            let events = std::mem::take(&mut self.ed.events);
            for e in events {
                self.event(e);
            }
        }
    }

    fn event(&mut self, e: Event) {
        match e {
            Event::Quit => self.quit = true,
            Event::FontSize(d) => {
                let size = (self.font.size + d as f32).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
                self.set_font_size(size);
            }
            Event::FontReset => self.set_font_size(shrew_config::DEFAULT_FONT_SIZE),
            // ----- git: the Source Control panel's lists (branches, stashes, changed files) -----
            #[cfg(feature = "git")]
            Event::Choose(c, v) if c.is_git() => crate::git::choose(self, c, v),
            Event::OpenChoice(c) => self.open_choice(c),
            Event::Choose(c, v) => self.choose(c, v),
            Event::ThemeImport(p) => self.import_theme(&p),
            Event::Ligatures(on) => self.set_ligatures(on),
            Event::CheckUpdates => self.check_updates(),
            Event::InstallUpdate => self.install_update(),
            Event::OpenConfig => self.open_config(),
            Event::McpSetup => self.mcp_setup(),
            Event::FeatureToggled(f) => {
                if let Some(path) = &self.config_path
                    && let Err(e) = shrew_config::persist_feature(path, f, self.ed.features.on(f))
                {
                    self.ed.error(format!("could not save {} to {}: {e}", features::info(f).key, path.display()));
                }
                self.feature_changed(f);
            }
            Event::BufferOpened(id) => self.buffer_opened(id),
            Event::Load(path) => self.open_at(&path, None, None),
            Event::App(cmd) => crate::titlebar::app_command(self, cmd),
            #[cfg(feature = "terminal")]
            Event::Terminal(cmd, line) => self.terminal_command(cmd, line),
            #[cfg(not(feature = "terminal"))]
            Event::Terminal(..) => self.ed.error("the terminal is not compiled into this build"),
            Event::OpenSettings(section) => self.open_settings(&section),
            // ----- git -----
            Event::Git(cmd, arg) => self.git_command(cmd, arg),
        }
    }

    // ----- git -----

    /// A git command (see `git::command`).
    fn git_command(&mut self, cmd: shrew_core::Command, arg: Option<String>) {
        #[cfg(feature = "git")]
        crate::git::command(self, cmd, arg);
        #[cfg(not(feature = "git"))]
        {
            let _ = (cmd, arg);
            self.ed.error("git is not compiled into this build");
        }
    }

    /// A git list to open (`value: None`) or a row picked in one (see `git::choice`).
    pub(crate) fn git_choice(&mut self, choice: shrew_core::Choice, value: Option<String>) {
        #[cfg(feature = "git")]
        crate::git::choice(self, choice, value);
        #[cfg(not(feature = "git"))]
        {
            let _ = (choice, value);
            self.ed.error("git is not compiled into this build");
        }
    }

    // -----

    /// Starts or stops what depends on a feature.
    fn feature_changed(&mut self, f: Feature) {
        let on = self.ed.features.on(f);
        match f {
            Feature::Syntax => self.set_syntax(on),
            Feature::Mcp => self.set_mcp(on),
            Feature::UpdateCheck if on => self.auto_update_check(),
            Feature::Sidebar if !on => self.ed.sidebar_focus = false,
            Feature::Sidebar | Feature::CompactFolders => self.ed.sync_tree(),
            #[cfg(feature = "terminal")]
            Feature::Terminal if !on => self.terminals.close_all(),
            Feature::Settings if !on => self.settings.close(),
            // ----- git -----
            #[cfg(feature = "git")]
            Feature::GitGutter | Feature::GitBlameInline | Feature::GitStatus | Feature::GitPanel => {
                crate::git::feature_changed(self, f)
            }
            _ => {}
        }
    }

    /// Ctrl+= / Ctrl+- / Ctrl+0: applied and saved to config.toml.
    fn set_font_size(&mut self, size: f32) {
        if size != self.font.size {
            self.font.size = size;
            self.font_changed = true;
            self.persist_font_size();
        }
        self.ed.info(format!("font size {size}"));
    }

    fn show_warnings(&mut self, warnings: &[shrew_config::Warning]) {
        // import summaries go to the log; the config file's own problems to the message line
        for w in warnings {
            log::warn!("{w}");
        }
        let ours: Vec<_> = warnings
            .iter()
            .filter(|w| !w.source.contains("vimrc") && !w.source.contains("init.vim") && !w.source.contains("Sublime"))
            .collect();
        if let Some(first) = ours.first() {
            let more = if ours.len() > 1 { format!(" (+{} more, see the log)", ours.len() - 1) } else { String::new() };
            self.ed.error(format!("{first}{more}"));
        }
    }

    // ----- syntax -----

    #[cfg(feature = "syntax")]
    fn set_syntax(&mut self, on: bool) {
        if !on {
            self.highlighters.clear();
            return;
        }
        if self.syntax.is_none() {
            let wake = self.wake.clone();
            let theme = shrew_syntax::Theme::new(&self.look.theme);
            self.syntax = Some(shrew_syntax::Syntaxes::new(theme, Arc::new(move || wake())));
        }
        let ids: Vec<u64> = self.ed.buffers.iter().map(|b| b.id).collect();
        for id in ids {
            self.buffer_opened(id);
        }
    }

    #[cfg(not(feature = "syntax"))]
    fn set_syntax(&mut self, _on: bool) {}

    fn buffer_opened(&mut self, id: u64) {
        #[cfg(feature = "syntax")]
        {
            if !self.ed.features.on(Feature::Syntax) {
                return;
            }
            let (Some(syntax), Some(b)) = (&self.syntax, self.ed.buffers.iter_mut().find(|b| b.id == id)) else {
                return;
            };
            // a fresh highlighter starts from the current text: edits recorded so far are not its business
            b.doc.take_edits();
            let hl = syntax.highlighter(&b.doc);
            let picked = b.doc.language.is_some();
            self.highlighters.insert(id, hl);
            // ----- status bar: a language picked by hand stays with the buffer -----
            if picked {
                crate::language::apply(self, id);
            }
        }
        #[cfg(not(feature = "syntax"))]
        let _ = id;
    }

    /// Hands every highlighter its buffer's edits and what is on screen. Called before drawing.
    pub fn sync_syntax(&mut self, top: usize, rows: usize) {
        #[cfg(feature = "syntax")]
        {
            if self.highlighters.len() > self.ed.buffers.len() {
                let ids: Vec<u64> = self.ed.buffers.iter().map(|b| b.id).collect();
                self.highlighters.retain(|id, _| ids.contains(id));
            }
            let active = self.ed.active;
            for (i, b) in self.ed.buffers.iter_mut().enumerate() {
                if let Some(hl) = self.highlighters.get_mut(&b.id) {
                    hl.sync(&mut b.doc);
                    if i == active { hl.set_viewport(top, rows) } else { hl.set_viewport(b.view.top, 0) }
                }
            }
        }
        #[cfg(not(feature = "syntax"))]
        let _ = (top, rows);
    }

    /// Syntax spans for a line of a buffer, when highlighting is on and that line is ready.
    #[cfg(feature = "syntax")]
    pub fn spans(&self, buffer: u64, line: usize) -> Option<&[shrew_syntax::Span]> {
        self.highlighters.get(&buffer)?.spans_for_line(line)
    }

    /// The active buffer's language for the status line.
    pub fn language(&self) -> Option<String> {
        #[cfg(feature = "syntax")]
        if let Some(n) = self.highlighters.get(&self.ed.buf().id).and_then(|h| h.syntax_name()) {
            return Some(n.to_string());
        }
        // ----- status bar: a language picked by hand (without highlighting) -----
        if let Some(l) = &self.ed.buf().doc.language {
            return Some(l.clone());
        }
        self.ed.lang().filter(|l| l.name != "text").map(|l| l.name.to_string())
    }

    // ----- background work -----

    /// Picks up whatever background threads finished (file lists, highlighting, loads, updates). True when the
    /// screen may have changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = self.ed.poll();
        changed |= self.poll_os_theme();
        #[cfg(feature = "syntax")]
        for hl in self.highlighters.values_mut() {
            changed |= hl.poll().is_some();
        }
        changed |= self.poll_loads();
        #[cfg(feature = "terminal")]
        {
            changed |= self.poll_terminals();
        }
        // ----- git -----
        #[cfg(feature = "git")]
        {
            changed |= crate::git::poll(self);
        }
        changed
    }

    fn poll_loads(&mut self) -> bool {
        let mut done = Vec::new();
        for (i, l) in self.loading.iter().enumerate() {
            match l.rx.try_recv() {
                Ok(r) => done.push((i, Some(r))),
                Err(TryRecvError::Disconnected) => done.push((i, None)),
                Err(TryRecvError::Empty) => {}
            }
        }
        let changed = !done.is_empty();
        for (i, r) in done.into_iter().rev() {
            let l = self.loading.remove(i);
            match r {
                Some(Ok(doc)) => {
                    log::debug!("loaded {} ({} bytes) in {:?}", l.path.display(), l.bytes, l.started.elapsed());
                    self.ed.install_document(doc);
                    if let Some(n) = l.line {
                        self.ed.goto_line_col(n, l.col);
                    }
                }
                Some(Err(e)) => self.ed.error(format!("{}: {e}", l.path.display())),
                None => self.ed.error(format!("{}: loading failed", l.path.display())),
            }
        }
        if changed {
            self.after_input();
        }
        changed
    }

    /// Work that waits until the first frame is on screen: the update check and the MCP server.
    pub fn start_deferred(&mut self) {
        if self.ed.features.on(Feature::UpdateCheck) {
            self.auto_update_check();
        }
        if self.ed.features.on(Feature::Mcp) {
            self.set_mcp(true);
        }
        self.load_recent();
        #[cfg(feature = "git")]
        {
            self.git.active = true;
            crate::git::sync(self);
        }
    }

    // ----- recent files and projects -----

    /// Reads the recent lists (behind what this session already opened). Never headless.
    fn load_recent(&mut self) {
        if self.headless || !self.ed.features.on(Feature::Recent) || self.recent_file.is_some() {
            return;
        }
        let Some(path) = crate::recent::path() else { return };
        let older = crate::recent::load(&path);
        self.ed.recent.merge_older(older);
        self.recent_file = Some(path);
        self.recent_saved = 0;
        self.save_recent();
    }

    fn save_recent(&mut self) {
        let Some(path) = &self.recent_file else { return };
        if self.ed.recent.generation == self.recent_saved || !self.ed.features.on(Feature::Recent) {
            return;
        }
        self.recent_saved = self.ed.recent.generation;
        if let Err(e) = crate::recent::save(path, &self.ed.recent) {
            log::warn!("{}: {e}", path.display());
        }
    }

    /// Saves the sidebar's width (after dragging its edge).
    pub fn persist_sidebar_width(&mut self) {
        let Some(path) = &self.config_path else { return };
        if let Err(e) = shrew_config::persist_value(path, "sidebar", "width", self.sidebar_width.round() as i64) {
            self.ed.error(format!("could not save the sidebar width to {}: {e}", path.display()));
        }
    }

    // ----- updates -----

    #[cfg(feature = "update")]
    fn updater(&mut self) -> &shrew_update::Updater {
        if self.updater.is_none() {
            shrew_update::cleanup();
            let wake = self.wake.clone();
            self.updater = Some(shrew_update::Updater::new(env!("CARGO_PKG_VERSION"), move || wake()));
        }
        self.updater.as_ref().expect("just created")
    }

    fn auto_update_check(&mut self) {
        #[cfg(feature = "update")]
        if !self.headless {
            self.updater().check(false);
        }
    }

    fn check_updates(&mut self) {
        #[cfg(feature = "update")]
        {
            self.updater().check(true);
        }
        #[cfg(not(feature = "update"))]
        self.ed.error("updates are not compiled into this build");
    }

    fn install_update(&mut self) {
        #[cfg(feature = "update")]
        {
            use shrew_update::Status;
            match self.updater().status() {
                Status::Available(_) => {
                    self.updater().install();
                }
                Status::Ready(_) | Status::Failed(_) => self.activate_update(),
                Status::Downloading(_) | Status::Checking => {}
                Status::Idle | Status::UpToDate => {
                    self.updater().check(true);
                }
            }
        }
        #[cfg(not(feature = "update"))]
        self.ed.error("updates are not compiled into this build");
    }

    /// The status-bar update button was clicked.
    pub fn activate_update(&mut self) {
        #[cfg(feature = "update")]
        {
            let ready = matches!(self.updater().status(), shrew_update::Status::Ready(_));
            if ready {
                // restarting exec's the new binary right away: nothing unsaved may be lost
                if let Some(b) = self.ed.buffers.iter().find(|b| b.doc.dirty) {
                    let name = b.doc.display_name();
                    return self.ed.error(format!("save \"{name}\" first: restarting would lose its changes"));
                }
                #[cfg(feature = "mcp")]
                if let Some(s) = self.mcp.take() {
                    s.stop();
                }
            }
            self.updater().activate();
        }
    }

    /// Text and tone of the status-bar update button, if one should show.
    #[cfg(feature = "update")]
    pub fn update_label(&self) -> Option<(String, Tone)> {
        use shrew_update::Status;
        let s = self.updater.as_ref()?.status();
        let label = s.label()?;
        Some(match s {
            Status::Available(_) => (format!("⬆ {label}"), Tone::Accent),
            Status::Ready(_) => (format!("⟳ {label}"), Tone::Success),
            Status::Failed(_) => (label, Tone::Error),
            _ => (label, Tone::Muted),
        })
    }

    #[cfg(not(feature = "update"))]
    pub fn update_label(&self) -> Option<(String, Tone)> {
        None
    }

    // ----- config -----

    fn open_config(&mut self) {
        let Some(path) = self.config_path.clone() else {
            return self.ed.error("no config directory on this system");
        };
        if let Err(e) = shrew_config::write_default_if_missing(&path) {
            return self.ed.error(format!("{}: {e}", path.display()));
        }
        match self.ed.open(&path) {
            Ok(()) => {
                let b = self.ed.buf();
                self.config_seen = Some((b.id, b.doc.version));
                self.ed.info(format!("{} (applied when saved)", path.display()));
            }
            Err(e) => self.ed.error(format!("{}: {e}", path.display())),
        }
    }

    fn check_config_saved(&mut self) {
        let Some(path) = &self.config_path else { return };
        let Some(b) = self.ed.buffers.iter().find(|b| b.doc.path.as_deref() == Some(path.as_path())) else { return };
        let seen = self.config_seen.filter(|s| s.0 == b.id);
        match seen {
            None => self.config_seen = Some((b.id, b.doc.version)),
            Some((_, v)) if v != b.doc.version && !b.doc.dirty => {
                self.config_seen = Some((b.id, b.doc.version));
                self.reload_config();
            }
            _ => {}
        }
    }

    /// Re-reads config.toml (and the imports) and applies everything that can change at runtime.
    pub fn reload_config(&mut self) {
        let loaded = shrew_config::load_with(self.config_path.as_deref(), &shrew_config::Env::detect());
        let before = self.ed.features;
        let mut warnings = loaded.warnings.clone();
        warnings.extend(shrew_config::apply(&loaded.config, &mut self.ed));
        let cfg = &loaded.config.editor;
        let font = font_config(cfg);
        if font != self.font {
            self.font = font;
            self.font_changed = true;
        }
        self.cursor_blink = cfg.cursor_blink();
        self.titlebar.configure(&loaded.config.titlebar);
        self.minimap_width = cfg.minimap_width() as f32;
        self.imports = loaded.imports.clone();
        self.mcp_port = loaded.config.mcp_port;
        let look = ThemeCfg::from(cfg);
        if look != self.look.cfg {
            self.apply_theme_config(look);
        }
        for info in features::FEATURES {
            if before.on(info.feature) != self.ed.features.on(info.feature) {
                self.feature_changed(info.feature);
            }
        }
        #[cfg(feature = "terminal")]
        self.terminals.set_config(loaded.config.terminal.clone());
        // ----- git -----
        #[cfg(feature = "git")]
        self.git.configure(&loaded.config.git);
        self.ed.info("config reloaded");
        self.show_warnings(&warnings);
    }

    // ----- MCP -----

    pub(crate) fn set_mcp(&mut self, on: bool) {
        #[cfg(feature = "mcp")]
        {
            let Some(dir) = shrew_mcp::files::mcp_dir() else { return };
            if on {
                if self.mcp.is_some() {
                    return;
                }
                let Some(host) = self.mcp_host.clone() else { return };
                // `[mcp] port` in config.toml (the Settings panel writes it) wins over the one saved with the token
                if let Some(port) = self.mcp_port {
                    let mut s = shrew_mcp::files::load_settings(&dir);
                    if s.port != port {
                        s.port = port;
                        let _ = shrew_mcp::files::save_settings(&dir, &s);
                    }
                }
                match shrew_mcp::McpServer::start(&dir, host) {
                    Ok(s) => {
                        log::info!("MCP endpoint at {}", s.url());
                        self.mcp = Some(s);
                    }
                    Err(e) => self.ed.error(format!("MCP server: {e}")),
                }
            } else {
                if let Some(s) = self.mcp.take() {
                    s.stop();
                }
                if dir.join("launch.json").exists() {
                    let _ = shrew_mcp::server::set_enabled(&dir, false);
                }
            }
        }
        #[cfg(not(feature = "mcp"))]
        let _ = on;
    }

    pub fn mcp_running(&self) -> bool {
        #[cfg(feature = "mcp")]
        let on = self.mcp.is_some();
        #[cfg(not(feature = "mcp"))]
        let on = false;
        on
    }

    fn mcp_setup(&mut self) {
        #[cfg(feature = "mcp")]
        {
            let exe = shrew_mcp::setup::launcher_exe().unwrap_or_else(|_| PathBuf::from("shrew"));
            let mut text = String::new();
            if self.mcp.is_none() {
                text.push_str(
                    "MCP is off. Turn it on with :set mcp (or the palette: Features), then run :mcp again.\n\n",
                );
            }
            text.push_str(&shrew_mcp::setup::setup_text(&exe, self.mcp.as_ref().map(|s| s.endpoint())));
            self.ed.install_document(Document::from_str(&text));
            self.process_events();
        }
        #[cfg(not(feature = "mcp"))]
        self.ed.error("MCP is not compiled into this build");
    }

    /// Runs a job an MCP tool sent to the UI thread.
    #[cfg(feature = "mcp")]
    pub fn run_job(&mut self, job: shrew_mcp::Job) {
        job(&mut self.ed);
        self.after_input();
    }
}

/// The renderer font from `[font]`: "monospace" (the default) means the embedded JetBrains Mono when it is compiled
/// in, so the first frame never waits for a system font scan.
pub fn font_config(cfg: &shrew_config::EditorConfig) -> FontConfig {
    FontConfig {
        family: family_for(cfg.font_family()),
        size: cfg.font_size(),
        line_height: cfg.line_height(),
        weight: cfg.font_weight(),
        ligatures: cfg.ligatures(),
        ..FontConfig::default()
    }
}

/// The renderer's family name for a configured one.
fn family_for(name: &str) -> String {
    match name {
        "monospace" if cfg!(feature = "embed-font") => shrew_render::DEFAULT_FAMILY.to_string(),
        "monospace" => String::new(),
        f => f.to_string(),
    }
}

/// Features whose code is not compiled into this build read as off and show as n/a.
fn mark_unavailable(ed: &mut Editor) {
    let f = &mut ed.features;
    if !cfg!(feature = "syntax") {
        f.set_unavailable(Feature::Syntax);
    }
    if !cfg!(feature = "picker") {
        f.set_unavailable(Feature::Picker);
    }
    if !cfg!(feature = "clipboard") {
        f.set_unavailable(Feature::Clipboard);
    }
    if !cfg!(feature = "mcp") {
        f.set_unavailable(Feature::Mcp);
    }
    if !cfg!(feature = "update") {
        f.set_unavailable(Feature::UpdateCheck);
    }
    if !cfg!(feature = "project") {
        f.set_unavailable(Feature::Sidebar);
        f.set_unavailable(Feature::CompactFolders);
        f.set_unavailable(Feature::ProjectSearch);
    }
    if !cfg!(feature = "syntax") {
        f.set_unavailable(Feature::Symbols);
    }
    if !cfg!(feature = "terminal") {
        f.set_unavailable(Feature::Terminal);
    }
    // ----- git -----
    if !cfg!(feature = "git") {
        f.set_unavailable(Feature::GitGutter);
        f.set_unavailable(Feature::GitBlameInline);
        f.set_unavailable(Feature::GitStatus);
        f.set_unavailable(Feature::GitPanel);
    }
}

fn install_clipboard(ed: &mut Editor) {
    #[cfg(feature = "clipboard")]
    {
        ed.clipboard = Box::new(crate::clipboard::SystemClipboard::new());
    }
    #[cfg(not(feature = "clipboard"))]
    let _ = ed;
}

/// Lists every feature with whether it is compiled in and whether the config turns it on (`--features`).
pub fn features_report(config: &shrew_config::Config) -> String {
    let mut ed = Editor::new();
    mark_unavailable(&mut ed);
    for &(f, on) in &config.features {
        ed.features.set(f, on);
    }
    let mut out = String::from("feature            build  state  description\n");
    for i in features::FEATURES {
        let built = ed.features.available(i.feature);
        let state = if !built {
            "n/a"
        } else if ed.features.on(i.feature) {
            "on"
        } else {
            "off"
        };
        out += &format!("{:<18} {:<6} {:<6} {}\n", i.key, if built { "yes" } else { "no" }, state, i.description);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true)
    }

    #[test]
    fn font_size_events_clamp_and_reset() {
        let mut st = state();
        let base = st.font.size;
        st.ed.run(shrew_core::Command::FontBigger);
        st.process_events();
        assert_eq!(st.font.size, base + 1.0);
        assert!(st.font_changed);
        for _ in 0..200 {
            st.ed.run(shrew_core::Command::FontSmaller);
        }
        st.process_events();
        assert_eq!(st.font.size, MIN_FONT_SIZE);
        st.ed.run(shrew_core::Command::FontReset);
        st.process_events();
        assert_eq!(st.font.size, base);
    }

    #[test]
    fn close_with_unsaved_changes_needs_two_attempts() {
        let mut st = state();
        st.close_requested();
        assert!(st.quit, "nothing to lose: closes at once");
        let mut st = state();
        for k in KeyPress::parse_seq("ihello<Esc>") {
            st.key(k);
        }
        st.close_requested();
        assert!(!st.quit);
        assert!(st.ed.message.as_ref().is_some_and(|m| m.1));
        st.close_requested();
        assert!(st.quit);
    }

    #[test]
    fn quit_command_quits() {
        let mut st = state();
        for k in KeyPress::parse_seq(":q<CR>") {
            st.key(k);
        }
        assert!(st.quit);
    }

    #[test]
    fn monospace_means_the_embedded_font() {
        let cfg = shrew_config::EditorConfig::default();
        let f = font_config(&cfg);
        if cfg!(feature = "embed-font") {
            assert_eq!(f.family, shrew_render::DEFAULT_FAMILY);
        }
        assert_eq!(f.size, shrew_config::DEFAULT_FONT_SIZE);
        let cfg = shrew_config::EditorConfig { font_family: Some("Iosevka".into()), ..Default::default() };
        assert_eq!(font_config(&cfg).family, "Iosevka");
    }

    #[test]
    fn config_is_reloaded_when_its_buffer_is_saved() {
        let dir = std::env::temp_dir().join(format!("shrew-app-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[editor]\ntab_width = 4\n").unwrap();
        let mut st = state();
        st.config_path = Some(path.clone());
        st.ed.run(shrew_core::Command::OpenConfig);
        st.after_input();
        assert_eq!(st.ed.buf().doc.path.as_deref(), Some(path.as_path()));
        // nothing happens until it is saved
        for k in KeyPress::parse_seq(":2s/4/7/<CR>") {
            st.key(k);
        }
        assert_eq!(st.ed.settings.tab_width, 4);
        for k in KeyPress::parse_seq(":w<CR>") {
            st.key(k);
        }
        assert_eq!(st.ed.settings.tab_width, 7, "{:?}", st.ed.message);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "syntax")]
    #[test]
    fn opened_buffers_get_highlighters_and_closed_ones_lose_them() {
        let mut st = state();
        let dir = env!("CARGO_MANIFEST_DIR");
        st.open_at(&PathBuf::from(dir).join("src/main.rs"), Some(3), None);
        st.process_events();
        let id = st.ed.buf().id;
        assert!(st.highlighters.contains_key(&id));
        assert_eq!(shrew_core::text::line_of(st.ed.buf().rope(), st.ed.buf().sel.primary().head), 2, "+3 is line 3");
        let s = st.syntax.clone().unwrap();
        assert!(s.wait_ready(std::time::Duration::from_secs(5)));
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while st.spans(id, 0).is_none() && Instant::now() < deadline {
            st.sync_syntax(0, 40);
            st.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(st.spans(id, 0).is_some(), "the first line gets highlighted");
        assert_eq!(st.language().as_deref(), Some("Rust"));
        st.key(KeyPress::parse("<C-n>").unwrap()); // new scratch buffer
        for k in KeyPress::parse_seq(":bd<CR>") {
            st.key(k);
        }
        st.sync_syntax(0, 40);
        assert_eq!(st.highlighters.len(), st.ed.buffers.len());
        // toggling syntax off drops them, on brings them back
        for k in KeyPress::parse_seq(":set nosyntax<CR>") {
            st.key(k);
        }
        assert!(st.highlighters.is_empty());
        for k in KeyPress::parse_seq(":set syntax<CR>") {
            st.key(k);
        }
        assert_eq!(st.highlighters.len(), st.ed.buffers.len());
    }

    #[test]
    fn big_files_load_in_the_background_from_everywhere() {
        let dir = std::env::temp_dir().join(format!("shrew-load-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.txt");
        let line = "0123456789".repeat(10) + "\n";
        std::fs::write(&big, line.repeat((BACKGROUND_LOAD_BYTES as usize / line.len()) + 10)).unwrap();
        let mut st = state();
        st.headless = false;
        st.ed.async_load_bytes = Some(BACKGROUND_LOAD_BYTES);
        // `:e` asks the app (Event::Load) instead of blocking
        for k in KeyPress::parse_seq(&format!(":e {}<CR>", big.display())) {
            st.key(k);
        }
        assert_eq!(st.loading.len(), 1);
        assert!(st.ed.buf().doc.path.is_none(), "nothing installed yet");
        // asking again (srw big.txt:5:3) does not load it twice, and moves the cursor when it is in
        st.open_at(&big, Some(5), Some(3));
        assert_eq!(st.loading.len(), 1);
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while !st.loading.is_empty() && Instant::now() < deadline {
            st.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let b = st.ed.buf();
        assert_eq!(b.doc.path.as_deref(), Some(big.as_path()));
        let head = b.sel.primary().head;
        assert_eq!(shrew_core::text::line_of(b.rope(), head), 4);
        assert_eq!(head - shrew_core::text::line_start(b.rope(), 4), 2);
        // small files stay synchronous
        let small = dir.join("small.txt");
        std::fs::write(&small, "hi\n").unwrap();
        st.open_at(&small, None, None);
        assert!(st.loading.is_empty());
        assert_eq!(st.ed.buf().doc.path.as_deref(), Some(small.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn runtime_mappings_work() {
        let mut st = state();
        for k in KeyPress::parse_seq(":inoremap jk <lt>Esc><CR>ihijk") {
            st.key(k);
        }
        assert_eq!(st.ed.mode, shrew_core::Mode::Normal);
        assert_eq!(st.ed.buf().rope().to_string(), "hi");
    }

    #[test]
    fn features_report_lists_everything() {
        let r = features_report(&shrew_config::Config::default());
        for i in features::FEATURES {
            assert!(r.contains(i.key));
        }
    }
}
