//! Themes, accents and fonts at runtime: applying them from the config, the Theme/Accent/Font pickers with live
//! preview (Esc puts the old value back, Enter keeps it and writes it to config.toml), `:theme`, `:accent`, `:font`,
//! `:theme import`, ligatures, following the desktop's light/dark setting, and reloading a custom theme when its
//! file is saved in shrew.
//!
//! Switching themes never reshapes text: the renderer caches shaped lines by text and bold/italic only, so a new
//! palette is just new colors on the next frame (syntax colors follow as soon as the highlighter has re-run the
//! visible lines, a few milliseconds).

use super::{State, os_theme};
use crate::colors::Colors;
use shrew_config::Setting;
use shrew_core::picker::{Choice, PickerData, PickerItem, PickerKind};
use shrew_theme::{ACCENTS, Accent, Library, Source, THEME_ACCENT, Theme};
use std::collections::HashMap;
use std::path::PathBuf;

#[cfg(test)]
mod tests;

/// Theme settings from the config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeCfg {
    pub name: String,
    pub accent: String,
    pub system_light: String,
    pub system_dark: String,
}

impl ThemeCfg {
    pub fn from(cfg: &shrew_config::EditorConfig) -> ThemeCfg {
        ThemeCfg {
            name: cfg.theme().to_string(),
            accent: cfg.accent().to_string(),
            system_light: cfg.system_light().to_string(),
            system_dark: cfg.system_dark().to_string(),
        }
    }
}

/// The value a picker started from, and the one on screen now.
struct Preview {
    choice: Choice,
    original: String,
    shown: String,
}

pub struct Look {
    pub cfg: ThemeCfg,
    pub lib: Library,
    /// The theme on screen, accent applied.
    pub theme: Theme,
    /// The desktop prefers dark (`None`: unknown).
    os_dark: Option<bool>,
    watcher: Option<os_theme::Watcher>,
    preview: Option<Preview>,
    /// Custom theme files open in buffers: buffer id -> the version last seen.
    custom_seen: HashMap<u64, u64>,
    /// Installed monospace families, once the renderer's font scan is done.
    pub font_families: Option<Vec<String>>,
}

impl Look {
    pub fn new(config_path: Option<&std::path::Path>) -> Look {
        let dir = config_path.and_then(|p| p.parent()).map(|d| d.join("themes"));
        let scheme_dirs =
            if dir.is_some() { shrew_config::sublime::candidates(&shrew_config::Env::detect()) } else { Vec::new() };
        Look {
            cfg: ThemeCfg::from(&shrew_config::EditorConfig::default()),
            lib: Library::new(dir, scheme_dirs),
            theme: Theme::default(),
            os_dark: None,
            watcher: None,
            preview: None,
            custom_seen: HashMap::new(),
            font_families: None,
        }
    }

    /// The theme id `name` stands for (`system` resolved).
    fn effective(&self, name: &str) -> String {
        if name != shrew_theme::SYSTEM {
            return name.to_string();
        }
        if self.os_dark.unwrap_or(true) { self.cfg.system_dark.clone() } else { self.cfg.system_light.clone() }
    }
}

fn accent_of(id: &str) -> Result<Option<&'static Accent>, String> {
    if id == THEME_ACCENT {
        return Ok(None);
    }
    shrew_theme::accent(id).map(Some).ok_or_else(|| {
        let ids: Vec<&str> = ACCENTS.iter().map(|a| a.id).collect();
        format!("unknown accent {id:?} (theme, {})", ids.join(", "))
    })
}

impl State {
    /// Applies the config's theme settings (startup and config reload).
    pub(super) fn apply_theme_config(&mut self, cfg: ThemeCfg) {
        let system = cfg.name == shrew_theme::SYSTEM;
        self.look.cfg = cfg;
        if system && self.look.os_dark.is_none() && !self.headless {
            self.look.os_dark = os_theme::guess_dark();
        }
        if system && self.look.watcher.is_none() && !self.headless {
            let wake = self.wake.clone();
            self.look.watcher = os_theme::watch(move || wake());
        } else if !system {
            self.look.watcher = None;
        }
        let (name, accent) = (self.look.cfg.name.clone(), self.look.cfg.accent.clone());
        if let Err(e) = self.show_theme(&name, &accent) {
            self.ed.error(e);
            self.look.theme = Theme::default();
            self.paint();
        }
    }

    /// Loads and shows a theme with an accent, without saving anything.
    fn show_theme(&mut self, name: &str, accent: &str) -> Result<(), String> {
        let accent = accent_of(accent)?;
        let id = self.look.effective(name);
        let loaded = self.look.lib.load(&id, accent)?;
        if let Some(w) = loaded.warnings.first() {
            let more = if loaded.warnings.len() > 1 {
                format!(" (+{} more)", loaded.warnings.len() - 1)
            } else {
                String::new()
            };
            self.ed.error(format!("{w}{more}"));
        }
        self.look.theme = loaded.theme;
        self.paint();
        Ok(())
    }

    /// Hands the current theme to the chrome and the highlighter.
    fn paint(&mut self) {
        self.colors = Colors::from_theme(&self.look.theme);
        #[cfg(feature = "syntax")]
        {
            let t = shrew_syntax::Theme::new(&self.look.theme);
            if let Some(s) = &self.syntax {
                s.set_theme(t);
            } else if self.ed.features.on(shrew_core::Feature::Syntax) {
                let wake = self.wake.clone();
                self.syntax = Some(shrew_syntax::Syntaxes::new(t, std::sync::Arc::new(move || wake())));
            }
        }
    }

    /// The desktop switched between light and dark (winit's `ThemeChanged`, or the portal on Linux).
    pub fn set_os_dark(&mut self, dark: bool) {
        if self.look.os_dark == Some(dark) {
            return;
        }
        self.look.os_dark = Some(dark);
        if self.look.cfg.name == shrew_theme::SYSTEM && self.look.preview.is_none() {
            let (name, accent) = (self.look.cfg.name.clone(), self.look.cfg.accent.clone());
            if let Err(e) = self.show_theme(&name, &accent) {
                self.ed.error(e);
            }
        }
    }

    /// Picks up answers from the desktop-appearance watcher. True when the theme changed.
    pub(super) fn poll_os_theme(&mut self) -> bool {
        let Some(w) = &self.look.watcher else { return false };
        let Some(dark) = w.rx.try_iter().last() else { return false };
        let before = self.look.theme.id.clone();
        self.set_os_dark(dark);
        self.look.theme.id != before
    }

    // ----- pickers -----

    pub(super) fn open_choice(&mut self, choice: Choice) {
        let item = |label: String, detail: String, value: &str| PickerItem {
            label,
            detail,
            data: PickerData::Value(value.to_string()),
        };
        let (items, current): (Vec<PickerItem>, String) = match choice {
            Choice::Theme => {
                let now = self.look.effective(shrew_theme::SYSTEM);
                let mut items =
                    vec![item("System (follow the desktop)".into(), format!("now {now}"), shrew_theme::SYSTEM)];
                for t in self.look.lib.list() {
                    let scheme = t.scheme.map_or("", |s| s.name());
                    let detail = match &t.source {
                        Source::Builtin => scheme.to_string(),
                        Source::Custom(p) => format!("{scheme} · {}", p.display()),
                        Source::Scheme(p) => format!("import · {}", p.display()),
                    };
                    let label = if t.source == Source::Builtin { t.name } else { format!("{} ({})", t.name, t.id) };
                    items.push(item(label, detail, &t.id));
                }
                (items, self.look.cfg.name.clone())
            }
            Choice::Accent => {
                let own = self.look.theme.with_accent(None).ui.accent;
                let mut items = vec![item("Theme default".into(), format!("{own}"), THEME_ACCENT)];
                let dark = self.look.theme.dark();
                for a in ACCENTS {
                    let fill = if dark { a.dark.fill } else { a.light.fill };
                    items.push(item(a.label.into(), format!("{fill}"), a.id));
                }
                (items, self.look.cfg.accent.clone())
            }
            Choice::Font => {
                let mut items = vec![item(
                    shrew_render::DEFAULT_FAMILY.into(),
                    "built in · monospace".into(),
                    shrew_config::DEFAULT_FONT_FAMILY,
                )];
                match &self.look.font_families {
                    Some(fams) => items.extend(
                        fams.iter()
                            .filter(|f| *f != shrew_render::DEFAULT_FAMILY)
                            .map(|f| item(f.clone(), "installed".into(), f)),
                    ),
                    None => self.ed.info("installed fonts are still loading: reopen the list in a moment"),
                }
                (items, self.font_choice_value())
            }
        };
        let selected = items.iter().position(|i| matches!(&i.data, PickerData::Value(v) if *v == current)).unwrap_or(0);
        self.ed.open_choice(choice, items, selected);
        self.look.preview = Some(Preview { choice, original: current.clone(), shown: current });
    }

    /// The font family as the config writes it ("monospace" for the built-in one).
    fn font_choice_value(&self) -> String {
        let f = &self.font.family;
        if f.is_empty() || f == shrew_render::DEFAULT_FAMILY {
            shrew_config::DEFAULT_FONT_FAMILY.to_string()
        } else {
            f.clone()
        }
    }

    /// Previews the highlighted row of an open theme/accent/font picker, and puts the old value back once the picker
    /// closes without a choice. Cheap when nothing changed; called after input and before each frame.
    pub fn sync_preview(&mut self) {
        let Some(pv) = &self.look.preview else { return };
        let choice = pv.choice;
        let row = match &self.ed.picker {
            Some(p) if p.kind == PickerKind::Choice(choice) => Some(p.current().map(|i| &i.data)),
            _ => None,
        };
        match row {
            Some(Some(PickerData::Value(v))) if *v != pv.shown => {
                let v = v.clone();
                self.preview(choice, &v);
                if let Some(pv) = &mut self.look.preview {
                    pv.shown = v;
                }
            }
            Some(_) => {}
            None => {
                // closed without Enter (Esc, a click outside)
                let pv = self.look.preview.take().expect("checked above");
                if pv.shown != pv.original {
                    self.preview(choice, &pv.original);
                }
            }
        }
    }

    fn preview(&mut self, choice: Choice, value: &str) {
        let r = match choice {
            Choice::Theme => {
                let accent = self.look.cfg.accent.clone();
                self.show_theme(value, &accent)
            }
            Choice::Accent => accent_of(value).map(|a| {
                self.look.theme = self.look.theme.with_accent(a);
                self.paint();
            }),
            Choice::Font => {
                self.set_font_family(value);
                Ok(())
            }
        };
        if let Err(e) = r {
            self.ed.error(e);
        }
    }

    fn set_font_family(&mut self, value: &str) {
        let family = super::family_for(value);
        if family != self.font.family {
            self.font.family = family;
            self.font_changed = true;
        }
    }

    /// Enter in a picker, or `:theme <id>` / `:accent <id>` / `:font <family>`: apply and save.
    pub(super) fn choose(&mut self, choice: Choice, value: String) {
        self.look.preview = None;
        let value = value.trim().to_string();
        match choice {
            Choice::Theme => {
                let accent = self.look.cfg.accent.clone();
                if let Err(e) = self.show_theme(&value, &accent) {
                    let accent = self.look.cfg.accent.clone();
                    let name = self.look.cfg.name.clone();
                    let _ = self.show_theme(&name, &accent);
                    return self.ed.error(e);
                }
                if value == shrew_theme::SYSTEM && self.look.watcher.is_none() && !self.headless {
                    self.look.os_dark = self.look.os_dark.or_else(os_theme::guess_dark);
                    let wake = self.wake.clone();
                    self.look.watcher = os_theme::watch(move || wake());
                }
                self.look.cfg.name = value.clone();
                self.persist(Setting::Theme, &value);
                self.ed.info(format!("theme {} ({})", self.look.theme.name, value));
            }
            Choice::Accent => match accent_of(&value) {
                Ok(a) => {
                    self.look.theme = self.look.theme.with_accent(a);
                    self.paint();
                    self.look.cfg.accent = value.clone();
                    self.persist(Setting::Accent, &value);
                    self.ed.info(format!("accent {value}"));
                }
                Err(e) => self.ed.error(e),
            },
            Choice::Font => {
                self.set_font_family(&value);
                self.persist(Setting::FontFamily, &value);
                self.ed.info(format!("font {value}"));
            }
        }
    }

    fn persist(&mut self, s: Setting, value: &str) {
        if let Some(path) = &self.config_path
            && let Err(e) = shrew_config::persist_setting(path, s, value)
        {
            self.ed.error(format!("could not save to {}: {e}", path.display()));
        }
    }

    /// `:set ligatures`, `:set noligatures`, `:set ligatures!`.
    pub(super) fn set_ligatures(&mut self, on: Option<bool>) {
        let on = on.unwrap_or(!self.font.ligatures);
        if on != self.font.ligatures {
            self.font.ligatures = on;
            self.font_changed = true;
        }
        if let Some(path) = &self.config_path
            && let Err(e) = shrew_config::persist_setting(path, Setting::Ligatures, on)
        {
            self.ed.error(format!("could not save to {}: {e}", path.display()));
            return;
        }
        self.ed.info(if on { "ligatures on" } else { "ligatures off" });
    }

    /// Persists a font size change (Ctrl+= / Ctrl+- / Ctrl+0).
    pub(super) fn persist_font_size(&mut self) {
        if let Some(path) = &self.config_path
            && let Err(e) = shrew_config::persist_setting(path, Setting::FontSize, self.font.size as f64)
        {
            self.ed.error(format!("could not save to {}: {e}", path.display()));
        }
    }

    /// `:theme import <path>`: converts a Sublime/TextMate scheme into a custom theme and switches to it.
    pub(super) fn import_theme(&mut self, path: &str) {
        if path.is_empty() {
            return self.ed.error("usage: :theme import <file.sublime-color-scheme | file.tmTheme>");
        }
        match self.look.lib.import(&PathBuf::from(path)) {
            Ok((id, file, warnings)) => {
                self.choose(Choice::Theme, id.clone());
                let mut msg = format!("imported as theme {id} ({})", file.display());
                if let Some(w) = warnings.first() {
                    msg += &format!("; {} contrast warning(s), e.g. {w}", warnings.len());
                }
                self.ed.info(msg);
            }
            Err(e) => self.ed.error(e),
        }
    }

    /// Reloads the theme when a custom theme file open in shrew is saved.
    pub(super) fn check_theme_saved(&mut self) {
        let mut saved = false;
        for b in &self.ed.buffers {
            let Some(path) = b.doc.path.as_deref() else { continue };
            if !self.look.lib.is_custom_file(path) {
                continue;
            }
            match self.look.custom_seen.insert(b.id, b.doc.version) {
                Some(v) if v != b.doc.version && !b.doc.dirty => saved = true,
                Some(v) if v != b.doc.version => {
                    // still dirty: remember the version from before the edits
                    self.look.custom_seen.insert(b.id, v);
                }
                _ => {}
            }
        }
        if saved {
            let (name, accent) = (self.look.cfg.name.clone(), self.look.cfg.accent.clone());
            match self.show_theme(&name, &accent) {
                Ok(()) => self.ed.info(format!("theme {} reloaded", self.look.theme.name)),
                Err(e) => self.ed.error(e),
            }
        }
    }
}
