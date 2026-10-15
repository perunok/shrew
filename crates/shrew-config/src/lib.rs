//! shrew's configuration: `config.toml`, plus what can be imported from a vimrc and from Sublime Text.
//!
//! Layers, lowest first: built-in defaults < Sublime Text user settings and keymap < vimrc < `config.toml`. Each
//! layer is a [`Config`] whose fields are `None` (or absent from its lists) unless that layer sets them, so a layer
//! only overrides what someone actually wrote. Nothing here fails: problems become [`Warning`]s.
//!
//! ```no_run
//! let mut editor = shrew_core::Editor::new();
//! let loaded = shrew_config::load();
//! let mut warnings = loaded.warnings.clone();
//! warnings.extend(shrew_config::apply(&loaded.config, &mut editor));
//! let font = loaded.config.editor.font_size();
//! ```

mod file;
mod persist;
pub mod sublime;
pub mod vimrc;

pub use file::{default_config_text, font_weight, parse};
pub use persist::{
    ConfigDoc, Setting, persist_feature, persist_list, persist_setting, persist_value, write_default_if_missing,
};
pub use shrew_core::keymap::{MapMode, Mapping, Rhs};

use shrew_core::commands::Command;
use shrew_core::features::Feature;
use shrew_core::{Editor, Mode};
use std::fmt;
use std::path::{Path, PathBuf};

pub const DEFAULT_FONT_FAMILY: &str = "monospace";
pub const DEFAULT_FONT_SIZE: f32 = 15.0;
/// Line height as a multiple of the font size.
pub const DEFAULT_LINE_HEIGHT: f32 = 1.4;
pub const DEFAULT_THEME: &str = "shrew-dark";
pub const DEFAULT_LIGHT_THEME: &str = "shrew-light";
/// `accent = "theme"`: the theme's own accent.
pub const DEFAULT_ACCENT: &str = "theme";
/// Regular.
pub const DEFAULT_FONT_WEIGHT: u16 = 400;
/// Same default as shrew-mcp's `DEFAULT_PORT`.
pub const DEFAULT_MCP_PORT: u16 = 7356;
/// `[minimap] width`, in logical pixels.
pub const DEFAULT_MINIMAP_WIDTH: u16 = 100;

/// Something in a config or imported file that was skipped. Never fatal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    /// The file (or a short name like `config.toml`).
    pub source: String,
    pub line: Option<usize>,
    pub message: String,
}

impl Warning {
    pub fn new(source: impl Into<String>, line: Option<usize>, message: impl Into<String>) -> Self {
        Warning { source: source.into(), line, message: message.into() }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(l) => write!(f, "{}:{}: {}", self.source, l, self.message),
            None => write!(f, "{}: {}", self.source, self.message),
        }
    }
}

/// `[editor]`, `[theme]` and `[font]`. `None` means "not set by this layer"; the getters give the effective value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorConfig {
    pub tab_width: Option<usize>,
    pub expand_tab: Option<bool>,
    pub scrolloff: Option<usize>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub line_height: Option<f32>,
    /// CSS-style weight of normal text (400 regular, 500 medium, 700 bold).
    pub font_weight: Option<u16>,
    pub ligatures: Option<bool>,
    /// A theme id, a custom theme, a scheme file path, or "system".
    pub theme: Option<String>,
    pub accent: Option<String>,
    /// The themes `theme = "system"` picks for a light and a dark desktop.
    pub system_light: Option<String>,
    pub system_dark: Option<String>,
    pub cursor_blink: Option<bool>,
    /// `<leader>` in mappings, vim notation (`\`, `,`, `<Space>`).
    pub leader: Option<String>,
    pub mapping_timeout_ms: Option<u64>,
    /// `[minimap] width` in logical pixels.
    pub minimap_width: Option<u16>,
}

impl EditorConfig {
    pub fn font_family(&self) -> &str {
        self.font_family.as_deref().unwrap_or(DEFAULT_FONT_FAMILY)
    }
    pub fn font_size(&self) -> f32 {
        self.font_size.unwrap_or(DEFAULT_FONT_SIZE)
    }
    pub fn line_height(&self) -> f32 {
        self.line_height.unwrap_or(DEFAULT_LINE_HEIGHT)
    }
    pub fn font_weight(&self) -> u16 {
        self.font_weight.unwrap_or(DEFAULT_FONT_WEIGHT)
    }
    pub fn ligatures(&self) -> bool {
        self.ligatures.unwrap_or(true)
    }
    pub fn theme(&self) -> &str {
        self.theme.as_deref().unwrap_or(DEFAULT_THEME)
    }
    pub fn accent(&self) -> &str {
        self.accent.as_deref().unwrap_or(DEFAULT_ACCENT)
    }
    pub fn system_light(&self) -> &str {
        self.system_light.as_deref().unwrap_or(DEFAULT_LIGHT_THEME)
    }
    pub fn system_dark(&self) -> &str {
        self.system_dark.as_deref().unwrap_or(DEFAULT_THEME)
    }
    pub fn cursor_blink(&self) -> bool {
        self.cursor_blink.unwrap_or(false)
    }
    pub fn mapping_timeout_ms(&self) -> u64 {
        self.mapping_timeout_ms.unwrap_or(1000)
    }
    pub fn minimap_width(&self) -> u16 {
        self.minimap_width.unwrap_or(DEFAULT_MINIMAP_WIDTH)
    }

    pub(crate) fn merge(&mut self, o: &EditorConfig) {
        fn take<T: Clone>(a: &mut Option<T>, b: &Option<T>) {
            if b.is_some() {
                *a = b.clone();
            }
        }
        take(&mut self.tab_width, &o.tab_width);
        take(&mut self.expand_tab, &o.expand_tab);
        take(&mut self.scrolloff, &o.scrolloff);
        take(&mut self.font_family, &o.font_family);
        take(&mut self.font_size, &o.font_size);
        take(&mut self.line_height, &o.line_height);
        take(&mut self.font_weight, &o.font_weight);
        take(&mut self.ligatures, &o.ligatures);
        take(&mut self.theme, &o.theme);
        take(&mut self.accent, &o.accent);
        take(&mut self.system_light, &o.system_light);
        take(&mut self.system_dark, &o.system_dark);
        take(&mut self.cursor_blink, &o.cursor_blink);
        take(&mut self.leader, &o.leader);
        take(&mut self.mapping_timeout_ms, &o.mapping_timeout_ms);
        take(&mut self.minimap_width, &o.minimap_width);
    }
}

/// Where an import reads from (`[import] vimrc = "auto" | "off" | "<path>"`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ImportSource {
    /// The usual locations (first one found).
    #[default]
    Auto,
    Off,
    /// A vimrc file, or a Sublime `Packages/User` directory.
    Path(PathBuf),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportConfig {
    pub vimrc: Option<ImportSource>,
    pub sublime: Option<ImportSource>,
}

/// A `[keys.<mode>]` entry as written; parsed when applied, so `<leader>` uses the final leader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeySpec {
    pub mode: MapMode,
    pub lhs: String,
    /// A command name (`save`), `:ex command`, or keys in vim notation (non-recursive).
    pub rhs: String,
    pub line: Option<usize>,
}

/// One configuration layer (the file, or what an import produced).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub editor: EditorConfig,
    /// Features this layer sets, in order (a later entry wins). `[update] check` lands here as `update_check`.
    pub features: Vec<(Feature, bool)>,
    /// `[keys.*]` from config.toml.
    pub keys: Vec<KeySpec>,
    /// Mappings already parsed by an importer.
    pub bindings: Vec<(MapMode, Mapping)>,
    pub import: ImportConfig,
    pub mcp_port: Option<u16>,
    /// `[titlebar]`: the custom title bar's items.
    pub titlebar: TitleBarConfig,
    /// `[sidebar] width`, logical pixels.
    pub sidebar_width: Option<f32>,
    pub terminal: TerminalConfig,
    // ----- git: gutter -----
    pub git: GitConfig,
}

// ----- git: gutter -----
/// `[git]`: the gutter's change markers and the inline blame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitConfig {
    /// `"index"` or `"head"`: what the change markers compare the buffer with.
    pub gutter_base: Option<String>,
    /// Milliseconds the cursor rests on a line before its inline blame shows.
    pub blame_delay: Option<u64>,
}

pub const DEFAULT_BLAME_DELAY_MS: u64 = 300;

impl GitConfig {
    /// The markers compare with HEAD (else the index).
    pub fn gutter_head(&self) -> bool {
        self.gutter_base.as_deref() == Some("head")
    }
    pub fn blame_delay(&self) -> u64 {
        self.blame_delay.unwrap_or(DEFAULT_BLAME_DELAY_MS)
    }
    fn merge(&mut self, o: &GitConfig) {
        if o.gutter_base.is_some() {
            self.gutter_base.clone_from(&o.gutter_base);
        }
        self.blame_delay = o.blame_delay.or(self.blame_delay);
    }
}

/// `[titlebar] left / right / hidden`: item names as written (the app knows which exist and sanitises them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TitleBarConfig {
    pub left: Option<Vec<String>>,
    pub right: Option<Vec<String>>,
    pub hidden: Option<Vec<String>>,
}

/// `[terminal]`: the built-in terminal panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalConfig {
    /// The program to run instead of `$SHELL`.
    pub shell: Option<String>,
    /// Panel height in rows (saved when the panel's top edge is dragged).
    pub height: Option<usize>,
    /// Lines kept for scrolling back, per terminal.
    pub scrollback: Option<usize>,
    /// Copy a mouse selection to the clipboard as soon as it is made.
    pub copy_on_select: Option<bool>,
    /// `[terminal.keys]`: chords that stay with shrew while a terminal has focus, as (key, command name). An
    /// empty name hands a default chord back to the terminal.
    pub keys: Vec<(String, String)>,
}

pub const DEFAULT_TERMINAL_HEIGHT: usize = 12;
pub const DEFAULT_SCROLLBACK: usize = 10_000;

impl TerminalConfig {
    pub fn height(&self) -> usize {
        self.height.unwrap_or(DEFAULT_TERMINAL_HEIGHT)
    }
    pub fn scrollback(&self) -> usize {
        self.scrollback.unwrap_or(DEFAULT_SCROLLBACK)
    }
    pub fn copy_on_select(&self) -> bool {
        self.copy_on_select.unwrap_or(true)
    }
    fn merge(&mut self, o: &TerminalConfig) {
        if o.shell.is_some() {
            self.shell = o.shell.clone();
        }
        self.height = o.height.or(self.height);
        self.scrollback = o.scrollback.or(self.scrollback);
        self.copy_on_select = o.copy_on_select.or(self.copy_on_select);
        self.keys.extend(o.keys.iter().cloned());
    }
}

impl Config {
    /// What this layer says about a feature, if anything.
    pub fn feature(&self, f: Feature) -> Option<bool> {
        self.features.iter().rev().find(|x| x.0 == f).map(|x| x.1)
    }

    /// Whether `f` ends up on: this layer's value, else the built-in default.
    pub fn feature_on(&self, f: Feature) -> bool {
        self.feature(f).unwrap_or(shrew_core::features::info(f).default)
    }

    pub fn mcp_port(&self) -> u16 {
        self.mcp_port.unwrap_or(DEFAULT_MCP_PORT)
    }

    pub fn set_feature(&mut self, f: Feature, on: bool) {
        self.features.retain(|x| x.0 != f);
        self.features.push((f, on));
    }

    /// Puts `over` on top of `self`.
    pub fn merge(&mut self, over: &Config) {
        self.editor.merge(&over.editor);
        for &(f, on) in &over.features {
            self.set_feature(f, on);
        }
        self.keys.extend(over.keys.iter().cloned());
        self.bindings.extend(over.bindings.iter().cloned());
        if over.import.vimrc.is_some() {
            self.import.vimrc = over.import.vimrc.clone();
        }
        if over.import.sublime.is_some() {
            self.import.sublime = over.import.sublime.clone();
        }
        if over.mcp_port.is_some() {
            self.mcp_port = over.mcp_port;
        }
        let (t, o) = (&mut self.titlebar, &over.titlebar);
        for (slot, v) in [(&mut t.left, &o.left), (&mut t.right, &o.right), (&mut t.hidden, &o.hidden)] {
            if v.is_some() {
                slot.clone_from(v);
            }
        }
        if over.sidebar_width.is_some() {
            self.sidebar_width = over.sidebar_width;
        }
        self.terminal.merge(&over.terminal);
        self.git.merge(&over.git);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// Where to look for files to import. [`Env::detect`] for the real machine; tests point it at fixtures.
#[derive(Clone, Debug)]
pub struct Env {
    pub home: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME` (or `~/.config`) on Linux, `~/Library/Application Support` on macOS.
    pub config_dir: Option<PathBuf>,
    /// `%APPDATA%` on Windows.
    pub appdata: Option<PathBuf>,
    pub os: Os,
}

impl Env {
    pub fn detect() -> Env {
        let os = Os::current();
        Env {
            home: dirs::home_dir(),
            config_dir: dirs::config_dir(),
            appdata: std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .or_else(|| if os == Os::Windows { dirs::config_dir() } else { None }),
            os,
        }
    }
}

/// What an importer read: a config layer and what it could not use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Imported {
    pub config: Config,
    pub report: ImportReport,
    /// Specific problems (unknown Sublime commands, ...), besides the unsupported-lines summary.
    pub warnings: Vec<Warning>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// "vimrc" or "sublime".
    pub what: &'static str,
    pub paths: Vec<PathBuf>,
    /// Settings and mappings taken over.
    pub applied: usize,
    /// Lines or settings shrew does not understand (skipped).
    pub unsupported: usize,
    /// The first few unsupported ones, with line numbers when known.
    pub samples: Vec<(Option<usize>, String)>,
}

const SAMPLES: usize = 5;

impl ImportReport {
    pub(crate) fn new(what: &'static str) -> Self {
        ImportReport { what, ..Default::default() }
    }

    pub(crate) fn skip(&mut self, line: Option<usize>, text: impl Into<String>) {
        self.unsupported += 1;
        if self.samples.len() < SAMPLES {
            let mut t: String = text.into();
            if t.chars().count() > 60 {
                t = t.chars().take(57).collect::<String>() + "...";
            }
            self.samples.push((line, t));
        }
    }

    /// One line for the status bar or log: "imported 14 settings from ~/.vimrc (3 unsupported: ...)".
    pub fn summary(&self) -> String {
        let from = self.paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
        let mut s = format!("{}: imported {} setting(s) from {from}", self.what, self.applied);
        if self.unsupported > 0 {
            let samples: Vec<String> = self
                .samples
                .iter()
                .map(|(l, t)| match l {
                    Some(l) => format!("{l}: {t}"),
                    None => t.clone(),
                })
                .collect();
            s += &format!(
                "; {} unsupported, skipped ({}{})",
                self.unsupported,
                samples.join(" | "),
                if self.unsupported > self.samples.len() { " | ..." } else { "" }
            );
        }
        s
    }
}

/// `$XDG_CONFIG_HOME/shrew/config.toml` (`~/.config/shrew/config.toml`; the platform config dir elsewhere).
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("shrew").join("config.toml"))
}

/// Everything startup needs.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    /// The effective configuration: imports with config.toml on top. Give it to [`apply`].
    pub config: Config,
    /// config.toml alone.
    pub file: Config,
    pub path: Option<PathBuf>,
    pub warnings: Vec<Warning>,
    pub imports: Vec<ImportReport>,
}

/// Reads the config file and runs the imports it asks for, on this machine.
pub fn load() -> Loaded {
    load_with(config_path().as_deref(), &Env::detect())
}

pub fn load_with(path: Option<&Path>, env: &Env) -> Loaded {
    let (file, mut warnings) = match path {
        Some(p) => read_config(p),
        None => (Config::default(), Vec::new()),
    };
    let (config, imports) = resolve(&file, env, &mut warnings);
    Loaded { config, file, path: path.map(Path::to_path_buf), warnings, imports }
}

/// Parses a config file; a missing file is an empty layer.
pub fn read_config(path: &Path) -> (Config, Vec<Warning>) {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, &path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), Vec::new()),
        Err(e) => {
            (Config::default(), vec![Warning::new(path.display().to_string(), None, format!("cannot read: {e}"))])
        }
    }
}

/// Stacks the imports `file` allows (Sublime, then vimrc) under `file`.
pub fn resolve(file: &Config, env: &Env, warnings: &mut Vec<Warning>) -> (Config, Vec<ImportReport>) {
    let mut config = Config::default();
    let mut reports = Vec::new();
    let mut take = |imp: Option<Imported>, config: &mut Config| {
        if let Some(imp) = imp {
            config.merge(&imp.config);
            warnings.extend(imp.warnings);
            if imp.report.unsupported > 0 {
                let source = imp.report.paths.first().map(|p| p.display().to_string()).unwrap_or_default();
                warnings.push(Warning::new(source, None, imp.report.summary()));
            }
            reports.push(imp.report);
        }
    };
    let src = |s: &Option<ImportSource>| s.clone().unwrap_or_default();
    if file.feature_on(Feature::SublimeImport) {
        take(sublime::import(&src(&file.import.sublime), env), &mut config);
    }
    if file.feature_on(Feature::Vimrc) {
        take(vimrc::import(&src(&file.import.vimrc), env), &mut config);
    }
    config.merge(file);
    (config, reports)
}

/// Puts a configuration into the editor: settings, features, leader and mappings. Fonts, theme, cursor blink and
/// the MCP port are the app's to read from [`Config::editor`] / [`Config::mcp_port`].
pub fn apply(config: &Config, editor: &mut Editor) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let e = &config.editor;
    if let Some(n) = e.tab_width {
        editor.settings.tab_width = n;
    }
    if let Some(b) = e.expand_tab {
        editor.settings.expand_tab = b;
    }
    if let Some(n) = e.scrolloff {
        editor.settings.scrolloff = n;
    }
    let vim_before = editor.vim();
    for &(f, on) in &config.features {
        editor.features.set(f, on);
    }
    if editor.vim() != vim_before {
        editor.set_mode(if editor.vim() { Mode::Normal } else { Mode::Insert });
    }
    let km = &mut editor.keymap;
    if let Some(l) = &e.leader
        && !km.set_leader(l)
    {
        warnings.push(Warning::new("config", None, format!("leader {l:?} is not a single key")));
    }
    km.timeout_ms = e.mapping_timeout_ms();
    for (mode, m) in &config.bindings {
        km.map(*mode, m.lhs.clone(), m.rhs.clone());
    }
    for k in &config.keys {
        let lhs = km.parse_keys(&k.lhs);
        if lhs.is_empty() {
            warnings.push(Warning::new(
                "config.toml",
                k.line,
                format!("[keys.{}] empty key {:?}", k.mode.name(), k.lhs),
            ));
            continue;
        }
        match key_rhs(km, &k.rhs) {
            Ok(rhs) => km.map(k.mode, lhs, rhs),
            Err(msg) => warnings.push(Warning::new(
                "config.toml",
                k.line,
                format!("[keys.{}] {:?}: {msg}", k.mode.name(), k.lhs),
            )),
        }
    }
    warnings
}

/// A `[keys]` value: `save` (command), `:w` / `:%s/a/b/g` (ex), `<Nop>` / `""` (nothing), else keys.
pub fn key_rhs(km: &shrew_core::Keymap, rhs: &str) -> Result<Rhs, String> {
    if let Some(ex) = rhs.strip_prefix(':') {
        let ex = ex.strip_suffix("<CR>").or_else(|| ex.strip_suffix("<cr>")).unwrap_or(ex);
        return Ok(Rhs::Ex(ex.trim().to_string()));
    }
    if let Some(c) = Command::from_name(rhs) {
        return Ok(Rhs::Command(c));
    }
    // a lowercase word with underscores is meant as a command name, not as keys to type
    if rhs.contains('_') && rhs.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
        return Err(format!("unknown command {rhs:?}"));
    }
    Ok(km.parse_rhs(rhs, false))
}

#[cfg(test)]
mod tests;
