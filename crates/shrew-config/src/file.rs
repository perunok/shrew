//! Reading `config.toml` into a [`Config`] layer, and the commented default file.
//!
//! Parsing is by hand over `toml_edit`'s tree rather than serde derive: a wrong type or unknown key costs one
//! warning (with its line), never the rest of the file.

use crate::{Config, EditorConfig, ImportSource, KeySpec, Warning};
use shrew_core::commands::COMMANDS;
use shrew_core::features::{self, FEATURES, Feature};
use shrew_core::keymap::{MapMode, leader_key};
use std::path::PathBuf;
use toml_edit::{Document, Item, TableLike};

struct Ctx<'a> {
    text: &'a str,
    source: &'a str,
    warnings: Vec<Warning>,
}

impl Ctx<'_> {
    fn line(&self, item: &Item) -> Option<usize> {
        item.span().map(|s| self.text[..s.start.min(self.text.len())].matches('\n').count() + 1)
    }
    fn warn(&mut self, item: &Item, msg: String) {
        let line = self.line(item);
        self.warnings.push(Warning::new(self.source, line, msg));
    }
    fn int(&mut self, item: &Item, name: &str, range: std::ops::RangeInclusive<i64>) -> Option<i64> {
        match item.as_integer() {
            Some(n) if range.contains(&n) => Some(n),
            _ => {
                self.warn(item, format!("{name}: expected a whole number from {} to {}", range.start(), range.end()));
                None
            }
        }
    }
    fn num(&mut self, item: &Item, name: &str, lo: f64, hi: f64) -> Option<f32> {
        match item.as_float().or_else(|| item.as_integer().map(|n| n as f64)) {
            Some(n) if (lo..=hi).contains(&n) => Some(n as f32),
            _ => {
                self.warn(item, format!("{name}: expected a number from {lo} to {hi}"));
                None
            }
        }
    }
    fn bool(&mut self, item: &Item, name: &str) -> Option<bool> {
        let b = item.as_bool();
        if b.is_none() {
            self.warn(item, format!("{name}: expected true or false"));
        }
        b
    }
    fn str<'i>(&mut self, item: &'i Item, name: &str) -> Option<&'i str> {
        let s = item.as_str();
        if s.is_none() {
            self.warn(item, format!("{name}: expected a string"));
        }
        s
    }
    fn table<'i>(&mut self, item: &'i Item, name: &str) -> Option<&'i dyn TableLike> {
        let t = item.as_table_like();
        if t.is_none() {
            self.warn(item, format!("[{name}] should be a table"));
        }
        t
    }
}

/// Parses config.toml text. `source` names the file in warnings.
pub fn parse(text: &str, source: &str) -> (Config, Vec<Warning>) {
    let mut cx = Ctx { text, source, warnings: Vec::new() };
    let mut config = Config::default();
    let doc = match Document::parse(text) {
        Ok(d) => d,
        Err(e) => {
            let line = e.span().map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
            let msg = e.message().trim().to_string();
            cx.warnings.push(Warning::new(source, line, format!("not valid TOML, the whole file is ignored: {msg}")));
            return (config, cx.warnings);
        }
    };
    for (key, item) in doc.as_table().iter() {
        match key {
            "editor" => {
                if let Some(t) = cx.table(item, key) {
                    let e = editor(&mut cx, t);
                    config.editor.merge(&e);
                }
            }
            "theme" => {
                let Some(t) = cx.table(item, key) else { continue };
                let e = &mut config.editor;
                for (k, v) in t.iter() {
                    let slot = match k {
                        "name" => &mut e.theme,
                        "accent" => &mut e.accent,
                        "system_light" => &mut e.system_light,
                        "system_dark" => &mut e.system_dark,
                        _ => {
                            cx.warn(v, format!("unknown key theme.{k}"));
                            continue;
                        }
                    };
                    *slot = theme_name(&mut cx, v, k).or(slot.take());
                }
            }
            "font" => {
                let Some(t) = cx.table(item, key) else { continue };
                for (k, v) in t.iter() {
                    let e = &mut config.editor;
                    match k {
                        "family" => e.font_family = family(&mut cx, v, k).or(e.font_family.take()),
                        "size" => e.font_size = cx.num(v, "font.size", 4.0, 200.0).or(e.font_size),
                        "line_height" => e.line_height = cx.num(v, "font.line_height", 0.5, 4.0).or(e.line_height),
                        "weight" => e.font_weight = weight(&mut cx, v).or(e.font_weight),
                        "ligatures" => e.ligatures = cx.bool(v, "font.ligatures").or(e.ligatures),
                        _ => cx.warn(v, format!("unknown key font.{k}")),
                    }
                }
            }
            "features" => {
                let Some(t) = cx.table(item, key) else { continue };
                for (k, v) in t.iter() {
                    match features::by_key(k) {
                        Some(info) => {
                            if let Some(b) = cx.bool(v, k) {
                                config.set_feature(info.feature, b);
                            }
                        }
                        None => cx.warn(v, format!("unknown feature {k:?} (see the list in the default config)")),
                    }
                }
            }
            "keys" => {
                let Some(t) = cx.table(item, key) else { continue };
                for (mode_name, sub) in t.iter() {
                    let Some(mode) = MapMode::from_name(mode_name) else {
                        cx.warn(sub, format!("unknown [keys.{mode_name}]: use normal, insert, visual or operator"));
                        continue;
                    };
                    let Some(sub) = cx.table(sub, &format!("keys.{mode_name}")) else { continue };
                    for (lhs, rhs) in sub.iter() {
                        if let Some(r) = cx.str(rhs, lhs) {
                            let line = cx.line(rhs);
                            config.keys.push(KeySpec { mode, lhs: lhs.to_string(), rhs: r.to_string(), line });
                        }
                    }
                }
            }
            "import" => {
                let Some(t) = cx.table(item, key) else { continue };
                for (k, v) in t.iter() {
                    let src = match (v.as_str(), v.as_bool()) {
                        (Some("auto"), _) | (_, Some(true)) => ImportSource::Auto,
                        (Some("off"), _) | (_, Some(false)) => ImportSource::Off,
                        (Some(p), _) if !p.is_empty() => ImportSource::Path(expand_home(p)),
                        _ => {
                            cx.warn(v, format!("{k}: expected \"auto\", \"off\" or a path"));
                            continue;
                        }
                    };
                    match k {
                        "vimrc" => config.import.vimrc = Some(src),
                        "sublime" => config.import.sublime = Some(src),
                        _ => cx.warn(v, format!("unknown key import.{k}")),
                    }
                }
            }
            "mcp" => {
                let Some(t) = cx.table(item, key) else { continue };
                for (k, v) in t.iter() {
                    match k {
                        "port" => config.mcp_port = cx.int(v, "mcp.port", 1..=65535).map(|n| n as u16),
                        _ => cx.warn(v, format!("unknown key mcp.{k}")),
                    }
                }
            }
            "update" => {
                let Some(t) = cx.table(item, key) else { continue };
                for (k, v) in t.iter() {
                    match k {
                        "check" => {
                            if let Some(b) = cx.bool(v, "update.check") {
                                config.set_feature(Feature::UpdateCheck, b);
                            }
                        }
                        _ => cx.warn(v, format!("unknown key update.{k}")),
                    }
                }
            }
            "sidebar" => {
                let Some(t) = cx.table(item, key) else { continue };
                for (k, v) in t.iter() {
                    match k {
                        "width" => {
                            config.sidebar_width = cx.num(v, "sidebar.width", 80.0, 2000.0).or(config.sidebar_width)
                        }
                        _ => cx.warn(v, format!("unknown key sidebar.{k}")),
                    }
                }
            }
            _ => cx.warn(item, format!("unknown section or key {key:?}")),
        }
    }
    (config, cx.warnings)
}

fn editor(cx: &mut Ctx, t: &dyn TableLike) -> EditorConfig {
    let mut e = EditorConfig::default();
    for (k, v) in t.iter() {
        match k {
            "tab_width" => e.tab_width = cx.int(v, k, 1..=16).map(|n| n as usize),
            "expand_tab" => e.expand_tab = cx.bool(v, k),
            "scrolloff" => e.scrolloff = cx.int(v, k, 0..=999).map(|n| n as usize),
            "font_family" => e.font_family = family(cx, v, k),
            "font_size" => e.font_size = cx.num(v, k, 4.0, 200.0),
            "line_height" => e.line_height = cx.num(v, k, 0.5, 4.0),
            "theme" => e.theme = theme_name(cx, v, k),
            "cursor_blink" => e.cursor_blink = cx.bool(v, k),
            "leader" => {
                if let Some(s) = cx.str(v, k) {
                    if leader_key(s).is_some() {
                        e.leader = Some(s.to_string());
                    } else {
                        cx.warn(v, format!("leader {s:?} is not a single key"));
                    }
                }
            }
            "mapping_timeout_ms" => e.mapping_timeout_ms = cx.int(v, k, 0..=10_000).map(|n| n as u64),
            _ => cx.warn(v, format!("unknown key editor.{k}")),
        }
    }
    e
}

/// A theme or accent id. Which ids exist is only known at runtime (custom themes), so the app checks that.
fn theme_name(cx: &mut Ctx, v: &Item, k: &str) -> Option<String> {
    let s = cx.str(v, k)?.trim();
    if s.is_empty() {
        cx.warn(v, format!("{k}: empty"));
        return None;
    }
    Some(s.to_string())
}

fn family(cx: &mut Ctx, v: &Item, k: &str) -> Option<String> {
    cx.str(v, k).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// `"regular"`, `"medium"`, `"bold"` (and the other CSS names), or 100..=900.
fn weight(cx: &mut Ctx, v: &Item) -> Option<u16> {
    if let Some(n) = v.as_integer() {
        if (100..=900).contains(&n) {
            return Some(n as u16);
        }
    } else if let Some(w) = v.as_str().and_then(font_weight) {
        return Some(w);
    }
    cx.warn(v, "font.weight: expected \"regular\", \"medium\", \"bold\" or a number from 100 to 900".into());
    None
}

/// CSS weight names.
pub fn font_weight(name: &str) -> Option<u16> {
    Some(match name.trim().to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
        "thin" => 100,
        "extralight" | "ultralight" => 200,
        "light" => 300,
        "regular" | "normal" | "book" => 400,
        "medium" => 500,
        "semibold" | "demibold" => 600,
        "bold" => 700,
        "extrabold" | "ultrabold" => 800,
        "black" | "heavy" => 900,
        _ => return None,
    })
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/").zip(dirs::home_dir()) {
        Some((rest, home)) => home.join(rest),
        None => PathBuf::from(p),
    }
}

/// The file written on first `:config`: every setting, commented out at its default, with what it does.
pub fn default_config_text() -> String {
    let mut s = String::from(
        r#"# shrew configuration.
#
# Every setting below is commented out and shows its default: uncomment a line to change it.
# shrew itself only ever edits values here (for example when you toggle a feature with :set or the
# palette) and keeps your comments and layout.
#
# Precedence, lowest first: built-in defaults < Sublime Text settings < ~/.vimrc < this file.

[editor]
# Columns per indent level, and how wide a tab is drawn.
# tab_width = 4
# Indent with spaces (true) or tabs (false).
# expand_tab = true
# Lines kept visible above and below the cursor.
# scrolloff = 4
# cursor_blink = false
# The key <leader> stands for in mappings: "\\" (vim's default), "," or " " (space).
# leader = "\\"
# How long to wait (ms) for the rest of a mapping such as "jk" before taking the keys as typed.
# mapping_timeout_ms = 1000

# Colors. Pick one live with the palette (Theme: Select…, Accent: Select…) or :theme / :accent;
# shrew writes the choice here.
[theme]
# A theme id (shrew-dark, shrew-light, dracula, nord, gruvbox-dark, catppuccin-mocha, tokyo-night,
# github-light, solarized-light, contrast, ... the list is in docs/THEMES.md), a custom theme from
# the themes folder next to this file, a .sublime-color-scheme / .tmTheme path, or "system" to
# follow the desktop's light/dark setting.
# name = "shrew-dark"
# An accent laid over the theme: "theme" (its own), blue, indigo, violet, purple, fuchsia, pink,
# rose, red, orange, amber, yellow, lime, green, emerald, teal, cyan, sky or slate.
# accent = "theme"
# With name = "system": the themes for a light and for a dark desktop.
# system_light = "shrew-light"
# system_dark = "shrew-dark"

[font]
# "monospace" is the bundled JetBrains Mono; any installed family works (palette: Font: Select…).
# family = "monospace"
# In points. Ctrl+= / Ctrl+- / Ctrl+0 change it and save it here.
# size = 15.0
# Line height as a multiple of the font size.
# line_height = 1.4
# "regular", "medium", "bold" (or 100-900), where the family has that weight.
# weight = "regular"
# Programming ligatures (->, !=, >=) in fonts that have them. Runtime: :set (no)ligatures
# ligatures = true

# Features: anything turned off here is never started, so it costs nothing.
[features]
"#,
    );
    for f in FEATURES {
        s += &format!("# {}\n# {} = {}\n", f.description, f.key, f.default);
    }
    s += r#"
# Key mappings, vim-style, one table per mode: normal, insert, visual, operator.
# Left: keys in vim notation (<C-x>, <A-x>, <S-Tab>, <Esc>, <CR>, <Space>, <leader>).
# Right: a command name (listed below), ":" + an ex command, or keys to type (not remapped).
# These win over mappings imported from a vimrc or Sublime Text, and over built-in bindings.
[keys.normal]
# "<leader>w" = ":w"
# "Y" = "y$"
# "<C-p>" = "files"

[keys.insert]
# "jk" = "<Esc>"

[keys.visual]
# "<" = "<gv"
# ">" = ">gv"

# Commands for the right-hand side of a mapping:
"#;
    for (_, name, title) in COMMANDS {
        s += &format!("#   {name:<24} {title}\n");
    }
    s += r#"
# Read settings and mappings from other editors at startup. "auto" looks in the usual places,
# "off" skips it, anything else is a path. Turn the features vimrc / sublime_import off to skip both.
[import]
# ~/.vimrc, ~/.vim/vimrc or ~/.config/nvim/init.vim (init.lua is not supported).
# vimrc = "auto"
# Sublime Text's Packages/User directory (Preferences.sublime-settings and Default (<OS>).sublime-keymap).
# sublime = "auto"

[mcp]
# Localhost port of the server AI assistants connect to (when the mcp feature is on).
# port = 7356

[update]
# Same as the update_check feature.
# check = true

[sidebar]
# Width of the file tree in logical pixels (Ctrl+B shows it; drag its edge to change this).
# width = 250
"#;
    s
}
