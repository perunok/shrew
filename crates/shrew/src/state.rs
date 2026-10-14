//! Everything the app owns except the window: the editor, config, syntax highlighting, background file loads, the
//! updater and the MCP server, plus the handling of the [`Event`]s the editor emits. The windowed app and the
//! headless `--screenshot` mode both drive a [`State`].

use crate::colors::Colors;
use shrew_core::features::{self, Feature};
use shrew_core::{Document, Editor, Event, KeyPress};
use shrew_render::FontConfig;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

/// Wakes the UI thread from a background thread (an event-loop proxy in the app, a no-op headless).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Files bigger than this load on a background thread behind a loading indicator.
pub const BACKGROUND_LOAD_BYTES: u64 = 4 << 20;

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
    rx: Receiver<io::Result<Document>>,
}

pub struct State {
    pub ed: Editor,
    pub colors: Colors,
    /// The font the renderer should use; `font_changed` tells the app to hand it over.
    pub font: FontConfig,
    pub font_changed: bool,
    /// Font size from the config (Ctrl+0 goes back to it).
    base_font_size: f32,
    pub cursor_blink: bool,
    pub config_path: Option<PathBuf>,
    /// The config file's buffer and the version last applied, to reload it when it is saved.
    config_seen: Option<(u64, u64)>,
    pub wake: Waker,
    pub loading: Vec<Loading>,
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
        }
        let cfg = &loaded.config.editor;
        let font = font_config(cfg);
        let mut st = State {
            base_font_size: font.size,
            font,
            font_changed: false,
            cursor_blink: cfg.cursor_blink(),
            colors: Colors::FALLBACK,
            config_path: loaded.path.clone(),
            config_seen: None,
            wake,
            loading: Vec::new(),
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
        };
        st.set_theme(cfg.theme());
        st.show_warnings(&warnings);
        st.ed.events.clear(); // the initial scratch buffer: highlighters are created on demand below
        st
    }

    /// Opens the command-line files: small ones right away, big ones in the background. `line` (`+N`) applies to
    /// the first file. A directory becomes the working directory and opens the file picker.
    pub fn open_files(&mut self, files: &[PathBuf], line: Option<usize>) {
        let mut first: Option<usize> = None;
        for (i, f) in files.iter().enumerate() {
            let path = self.ed.absolute(f);
            let line = if i == 0 { line } else { None };
            if path.is_dir() {
                self.ed.cwd = path;
                self.ed.run(shrew_core::Command::OpenFile);
                continue;
            }
            let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if bytes > BACKGROUND_LOAD_BYTES && !self.headless {
                self.load_in_background(path, bytes, line);
                continue;
            }
            match self.ed.open(&path) {
                Ok(()) => {
                    first.get_or_insert(self.ed.active);
                    if let Some(n) = line {
                        self.ed.goto_line(n);
                    }
                }
                Err(e) => self.ed.error(format!("{}: {e}", path.display())),
            }
        }
        if let Some(i) = first {
            self.ed.switch_to(i);
        }
        self.process_events();
    }

    fn load_in_background(&mut self, path: PathBuf, bytes: u64, line: Option<usize>) {
        let (tx, rx) = mpsc::channel();
        let wake = self.wake.clone();
        let p = path.clone();
        let spawned = std::thread::Builder::new().name("shrew-load".into()).spawn(move || {
            let _ = tx.send(Document::load(&p));
            wake();
        });
        match spawned {
            Ok(_) => self.loading.push(Loading { path, bytes, started: Instant::now(), line, rx }),
            Err(e) => self.ed.error(format!("{}: {e}", path.display())),
        }
    }

    /// A key from the keyboard (or `--keys`).
    pub fn key(&mut self, k: KeyPress) {
        self.ed.handle_key(k);
        self.after_input();
    }

    /// Text typed through an input method.
    pub fn text(&mut self, s: &str) {
        if self.ed.mapping_pending() {
            self.ed.flush_pending_mappings();
        }
        self.ed.insert_text(s);
        self.after_input();
    }

    /// After anything changed the editor: run its events, reload the config if it was just saved.
    pub fn after_input(&mut self) {
        self.process_events();
        self.check_config_saved();
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
            Event::FontReset => self.set_font_size(self.base_font_size),
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
        }
    }

    /// Starts or stops what depends on a feature.
    fn feature_changed(&mut self, f: Feature) {
        let on = self.ed.features.on(f);
        match f {
            Feature::Syntax => self.set_syntax(on),
            Feature::Mcp => self.set_mcp(on),
            Feature::UpdateCheck if on => self.auto_update_check(),
            _ => {}
        }
    }

    fn set_font_size(&mut self, size: f32) {
        if size != self.font.size {
            self.font.size = size;
            self.font_changed = true;
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

    // ----- theme & syntax -----

    fn set_theme(&mut self, name: &str) {
        #[cfg(feature = "syntax")]
        {
            let theme = shrew_syntax::Theme::builtin(name).unwrap_or_else(|| {
                self.ed.error(format!("unknown theme {name:?} (shrew-dark, shrew-light)"));
                shrew_syntax::Theme::shrew()
            });
            self.colors = Colors::from_theme(&theme);
            if let Some(s) = &self.syntax {
                s.set_theme(theme);
            } else if self.ed.features.on(Feature::Syntax) {
                let wake = self.wake.clone();
                self.syntax = Some(shrew_syntax::Syntaxes::new(theme, Arc::new(move || wake())));
            }
        }
        #[cfg(not(feature = "syntax"))]
        {
            let _ = name;
            self.colors = Colors::FALLBACK;
        }
    }

    #[cfg(feature = "syntax")]
    fn set_syntax(&mut self, on: bool) {
        if !on {
            self.highlighters.clear();
            return;
        }
        if self.syntax.is_none() {
            let wake = self.wake.clone();
            let theme = shrew_syntax::Theme::builtin(&self.theme_name()).unwrap_or_else(shrew_syntax::Theme::shrew);
            self.syntax = Some(shrew_syntax::Syntaxes::new(theme, Arc::new(move || wake())));
        }
        let ids: Vec<u64> = self.ed.buffers.iter().map(|b| b.id).collect();
        for id in ids {
            self.buffer_opened(id);
        }
    }

    #[cfg(not(feature = "syntax"))]
    fn set_syntax(&mut self, _on: bool) {}

    #[cfg(feature = "syntax")]
    fn theme_name(&self) -> String {
        self.syntax.as_ref().map(|s| s.theme().name.clone()).unwrap_or_else(|| "shrew".into())
    }

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
            self.highlighters.insert(id, hl);
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
        self.ed.lang().filter(|l| l.name != "text").map(|l| l.name.to_string())
    }

    // ----- background work -----

    /// Picks up whatever background threads finished (file lists, highlighting, loads, updates). True when the
    /// screen may have changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = self.ed.poll();
        #[cfg(feature = "syntax")]
        for hl in self.highlighters.values_mut() {
            changed |= hl.poll().is_some();
        }
        changed |= self.poll_loads();
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
                        self.ed.goto_line(n);
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
        self.base_font_size = self.font.size;
        self.cursor_blink = cfg.cursor_blink();
        self.set_theme(cfg.theme());
        for info in features::FEATURES {
            if before.on(info.feature) != self.ed.features.on(info.feature) {
                self.feature_changed(info.feature);
            }
        }
        self.ed.info("config reloaded");
        self.show_warnings(&warnings);
    }

    // ----- MCP -----

    fn set_mcp(&mut self, on: bool) {
        #[cfg(feature = "mcp")]
        {
            let Some(dir) = shrew_mcp::files::mcp_dir() else { return };
            if on {
                if self.mcp.is_some() {
                    return;
                }
                let Some(host) = self.mcp_host.clone() else { return };
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

/// The renderer font from `[editor]`: "monospace" (the default) means the embedded JetBrains Mono when it is compiled
/// in, so the first frame never waits for a system font scan.
pub fn font_config(cfg: &shrew_config::EditorConfig) -> FontConfig {
    let family = match cfg.font_family() {
        "monospace" if cfg!(feature = "embed-font") => shrew_render::DEFAULT_FAMILY.to_string(),
        "monospace" => String::new(),
        f => f.to_string(),
    };
    FontConfig { family, size: cfg.font_size(), line_height: cfg.line_height(), ..FontConfig::default() }
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
        st.open_files(&[PathBuf::from(dir).join("src/main.rs")], Some(3));
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
