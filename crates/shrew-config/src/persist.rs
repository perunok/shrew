//! Writing values back into config.toml without disturbing the user's comments and layout: an existing key keeps
//! its place and decoration, a commented-out `# key = default` line in the right table is uncommented in place,
//! and only otherwise is a new key added.

use crate::file::default_config_text;
use shrew_core::features::{self, Feature};
use std::io;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Value};

/// config.toml opened for editing.
pub struct ConfigDoc {
    pub path: PathBuf,
    text: String,
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("config is not valid TOML, not touching it: {e}"))
}

impl ConfigDoc {
    /// Opens the file; a missing one starts from [`default_config_text`].
    pub fn open(path: &Path) -> io::Result<ConfigDoc> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => default_config_text(),
            Err(e) => return Err(e),
        };
        Ok(ConfigDoc::from_text(path, text))
    }

    pub fn from_text(path: &Path, text: String) -> ConfigDoc {
        ConfigDoc { path: path.to_path_buf(), text }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether `[table] key` holds a value (commented-out lines do not count).
    pub fn has(&self, table: &str, key: &str) -> bool {
        let Ok(doc) = self.text.parse::<DocumentMut>() else { return false };
        let mut t: Option<&Item> = Some(doc.as_item());
        for p in table.split('.') {
            t = t.and_then(|t| t.get(p));
        }
        t.and_then(|t| t.get(key)).is_some_and(|v| v.is_value())
    }

    /// Sets a setting where the file keeps it: its old `[editor]` key if the file uses that, else its own table.
    pub fn set_setting(&mut self, s: Setting, value: impl Into<Value>) -> io::Result<()> {
        let (table, key, legacy) = s.place();
        match legacy {
            Some(old) if self.has("editor", old) && !self.has(table, key) => self.set("editor", old, value),
            _ => self.set(table, key, value),
        }
    }

    pub fn set_feature(&mut self, f: Feature, on: bool) -> io::Result<()> {
        self.set("features", features::info(f).key, on)
    }

    /// Sets `[table] key = value` (`table` may be dotted: `keys.normal`).
    pub fn set(&mut self, table: &str, key: &str, value: impl Into<Value>) -> io::Result<()> {
        let value: Value = value.into();
        let mut doc: DocumentMut = self.text.parse().map_err(invalid)?;
        let path: Vec<&str> = table.split('.').collect();

        if let Some(Item::Value(v)) = table_mut(&mut doc, &path, false).and_then(|t| t.get_mut(key)) {
            let decor = v.decor().clone();
            *v = value;
            *v.decor_mut() = decor;
            self.text = doc.to_string();
            return Ok(());
        }
        if let Some(text) = uncomment(&self.text, table, key, &value) {
            let ok = text.parse::<DocumentMut>().ok().is_some_and(|d| {
                let mut t: Option<&Item> = Some(d.as_item());
                for p in &path {
                    t = t.and_then(|t| t.get(p));
                }
                t.and_then(|t| t.get(key))
                    .and_then(Item::as_value)
                    .is_some_and(|v| v.to_string().trim() == bare(&value))
            });
            if ok {
                self.text = text;
                return Ok(());
            }
        }
        let t = table_mut(&mut doc, &path, true).ok_or_else(|| invalid(format!("{table} is not a table")))?;
        t.insert(key, Item::Value(value));
        self.text = doc.to_string();
        Ok(())
    }

    /// Writes the file (via a temporary file and a rename, so a crash never leaves half a config).
    pub fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, &self.text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

fn table_mut<'a>(doc: &'a mut DocumentMut, path: &[&str], create: bool) -> Option<&'a mut toml_edit::Table> {
    let mut t = doc.as_table_mut();
    for p in path {
        if create && !t.contains_key(p) {
            let mut new = toml_edit::Table::new();
            new.set_implicit(true);
            t.insert(p, Item::Table(new));
        }
        t = t.get_mut(p)?.as_table_mut()?;
    }
    Some(t)
}

fn bare(v: &Value) -> String {
    let mut v = v.clone();
    v.decor_mut().clear();
    v.to_string()
}

/// Replaces a `# key = ...` line inside `[table]` with `key = value`.
fn uncomment(text: &str, table: &str, key: &str, value: &Value) -> Option<String> {
    let header = format!("[{table}]");
    let mut in_table = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') {
            in_table = t.split('#').next().is_some_and(|h| h.trim() == header);
        } else if in_table {
            if let Some(rest) = t.strip_prefix('#') {
                let rest = rest.trim_start_matches('#').trim_start();
                let is_key = rest.strip_prefix(key).is_some_and(|r| r.trim_start().starts_with('='));
                if is_key {
                    let indent = &line[..line.len() - line.trim_start().len()];
                    let nl = if line.ends_with("\r\n") {
                        "\r\n"
                    } else if line.ends_with('\n') {
                        "\n"
                    } else {
                        ""
                    };
                    let mut out = String::with_capacity(text.len());
                    out += &text[..offset];
                    out += &format!("{indent}{key} = {}{nl}", bare(value));
                    out += &text[offset + line.len()..];
                    return Some(out);
                }
            }
        }
        offset += line.len();
    }
    None
}

/// Writes the commented default config if there is no file yet (for `:config`). True if it wrote one.
pub fn write_default_if_missing(path: &Path) -> io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    ConfigDoc::from_text(path, default_config_text()).save()?;
    Ok(true)
}

/// Opens, sets one feature, saves.
pub fn persist_feature(path: &Path, f: Feature, on: bool) -> io::Result<()> {
    let mut doc = ConfigDoc::open(path)?;
    doc.set_feature(f, on)?;
    doc.save()
}

/// Settings the app writes back when they change at runtime (pickers, `:theme`, Ctrl+=).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Theme,
    Accent,
    FontFamily,
    FontSize,
    FontWeight,
    LineHeight,
    Ligatures,
}

impl Setting {
    /// (table, key, the key's old name in `[editor]`).
    fn place(self) -> (&'static str, &'static str, Option<&'static str>) {
        match self {
            Setting::Theme => ("theme", "name", Some("theme")),
            Setting::Accent => ("theme", "accent", None),
            Setting::FontFamily => ("font", "family", Some("font_family")),
            Setting::FontSize => ("font", "size", Some("font_size")),
            Setting::FontWeight => ("font", "weight", None),
            Setting::LineHeight => ("font", "line_height", Some("line_height")),
            Setting::Ligatures => ("font", "ligatures", None),
        }
    }
}

/// Opens, sets one setting where the file keeps it, saves.
pub fn persist_setting(path: &Path, s: Setting, value: impl Into<Value>) -> io::Result<()> {
    let mut doc = ConfigDoc::open(path)?;
    doc.set_setting(s, value)?;
    doc.save()
}

/// Opens, sets `[table] key = value`, saves (for example `("editor", "font_size", 16.0)`).
pub fn persist_value(path: &Path, table: &str, key: &str, value: impl Into<Value>) -> io::Result<()> {
    let mut doc = ConfigDoc::open(path)?;
    doc.set(table, key, value)?;
    doc.save()
}
