//! Importing foreign color schemes (Sublime `.sublime-color-scheme`, TextMate `.tmTheme`) as shrew themes: the
//! scheme's globals become UI tokens, and each syntax role takes the color the scheme gives that role's typical
//! scopes (see [`sample_scopes`](crate::scopes)), with the same specificity rules as Sublime.

mod selector;
mod sublime;
mod tmtheme;

use crate::color::Rgb;
use crate::scopes::sample_scopes;
use crate::theme::{Scheme, ThemeFile};
use crate::tokens::{Font, Role, SyntaxStyle};
use std::collections::HashMap;
use std::path::Path;

pub use selector::score as selector_score;

/// A color scheme as read, before conversion.
#[derive(Clone, Debug, Default)]
pub struct Foreign {
    pub name: Option<String>,
    /// `background`, `foreground`, `caret`, `line_highlight`, ... (tmTheme's camelCase keys are converted).
    pub globals: HashMap<String, (Rgb, u8)>,
    pub rules: Vec<ForeignRule>,
}

#[derive(Clone, Debug)]
pub struct ForeignRule {
    pub selector: String,
    pub fg: Option<(Rgb, u8)>,
    pub font: Option<Font>,
}

/// `"bold italic"`.
fn font_style(s: &str) -> Font {
    let mut f = Font::PLAIN;
    for w in s.split_whitespace() {
        match w {
            "bold" => f.bold = true,
            "italic" => f.italic = true,
            "underline" => f.underline = true,
            _ => {}
        }
    }
    f
}

/// File extensions the importer reads.
pub const EXTENSIONS: &[&str] = &["sublime-color-scheme", "tmTheme"];

/// Whether `path` looks like a color scheme the importer reads.
pub fn is_importable(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Reads a `.sublime-color-scheme` or `.tmTheme` file.
pub fn read(path: &Path) -> Result<Foreign, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    let mut f = if ext.eq_ignore_ascii_case("tmTheme") || text.trim_start().starts_with('<') {
        tmtheme::parse(&text)
    } else {
        sublime::parse(&text)
    }
    .map_err(|e| format!("{}: {e}", path.display()))?;
    if f.name.as_deref().is_none_or(str::is_empty) {
        f.name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    }
    Ok(f)
}

/// Parses scheme text (`.tmTheme` when it starts with `<`, else Sublime JSON).
pub fn parse(text: &str) -> Result<Foreign, String> {
    if text.trim_start().starts_with('<') { tmtheme::parse(text) } else { sublime::parse(text) }
}

impl Foreign {
    /// The shrew theme this scheme amounts to. It stands alone (no `base`): whatever it does not say is derived.
    pub fn to_theme_file(&self) -> ThemeFile {
        let g = |k: &str| self.globals.get(k).copied();
        let bg = g("background").map(|c| c.0).unwrap_or(Rgb::hex(0x1e1e1e));
        let flat = |(c, a): (Rgb, u8)| bg.mix(c, a as f32 / 255.0);
        let text =
            g("foreground").map(flat).unwrap_or_else(|| if bg.luminance() < 0.2 { Rgb::WHITE } else { Rgb::BLACK });
        let scheme = if bg.luminance() < 0.2 { Scheme::Dark } else { Scheme::Light };
        let mut f = ThemeFile {
            name: self.name.clone(),
            scheme: Some(scheme),
            base: None,
            high_contrast: None,
            ..ThemeFile::default()
        };
        // syntax: the best rule for each role's typical scopes, color and font style resolved separately as Sublime does
        for &role in Role::ALL {
            let stack = sample_scopes(role);
            let best = |want: &dyn Fn(&ForeignRule) -> bool| {
                let mut best: Option<(u64, &ForeignRule)> = None;
                for r in self.rules.iter().filter(|r| want(r)) {
                    if let Some(s) = selector::score(&r.selector, stack)
                        && best.is_none_or(|(b, _)| s >= b)
                    {
                        best = Some((s, r));
                    }
                }
                best.map(|b| b.1)
            };
            let fg = best(&|r| r.fg.is_some()).and_then(|r| r.fg).map(flat);
            let font = best(&|r| r.font.is_some()).and_then(|r| r.font);
            if let Some(fg) = fg.filter(|&c| c != text || font.is_some()) {
                f.syntax.set(role, SyntaxStyle { fg, font });
            }
        }
        let u = &mut f.ui;
        u.bg = Some(bg);
        u.text = Some(text);
        let accent = g("accent").or(g("caret")).map(flat).or_else(|| f.syntax.get(Role::Keyword).map(|s| s.fg));
        u.accent = accent;
        u.cursor = g("caret").map(flat);
        u.selection = g("selection").map(flat);
        u.selection_inactive = g("inactive_selection").map(flat);
        u.current_line = g("line_highlight").map(flat);
        u.gutter = g("gutter").map(flat);
        u.line_number = g("gutter_foreground").map(flat);
        u.indent_guide = g("guide").map(flat);
        u.indent_guide_active = g("active_guide").map(flat);
        // bright find highlights are meant for dark text on them; keep only those our text can sit on
        u.search_match = g("find_highlight").map(flat).filter(|&c| text.contrast(c) >= 4.5);
        f
    }
}

/// A theme id from a scheme name: `"Monokai Pro (Filter Spectrum)"` -> `monokai-pro-filter-spectrum`.
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            s.extend(c.to_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() { "imported".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sublime_scheme_to_theme() {
        let json = r##"{
            // comments are fine
            "name": "Tiny Mariana",
            "variables": { "blue": "#6699cc", "bg": "hsl(210, 15%, 22%)", "pink": "rgb(197, 148, 197)" },
            "globals": {
                "background": "var(bg)",
                "foreground": "#d8dee9",
                "caret": "color(var(blue) alpha(1))",
                "line_highlight": "color(#ffffff alpha(0.05))",
                "selection": "#4e5a65",
            },
            "rules": [
                { "scope": "comment, punctuation.definition.comment", "foreground": "#a6acb9", "font_style": "italic" },
                { "scope": "keyword", "foreground": "var(pink)" },
                { "scope": "keyword.operator", "foreground": "#f97b58" },
                { "scope": "string", "foreground": "#99c794" },
                { "scope": "entity.name.function", "foreground": "var(blue)" },
            ],
        }"##;
        let f = sublime::parse(json).unwrap().to_theme_file();
        assert_eq!(f.name.as_deref(), Some("Tiny Mariana"));
        assert_eq!(f.scheme, Some(Scheme::Dark));
        assert_eq!(f.ui.cursor, Some(Rgb::hex(0x6699cc)));
        assert_eq!(f.syntax.get(Role::Keyword).unwrap().fg, Rgb::hex(0xc594c5));
        assert_eq!(f.syntax.get(Role::Operator).unwrap().fg, Rgb::hex(0xf97b58));
        assert_eq!(f.syntax.get(Role::Comment).unwrap().font, Some(Font::ITALIC));
        assert_eq!(f.syntax.get(Role::Function).unwrap().fg, Rgb::hex(0x6699cc));
        // no rule for numbers: derived later
        assert_eq!(f.syntax.get(Role::Number), None);
        let bg = f.ui.bg.unwrap();
        assert!(f.ui.current_line.unwrap().contrast(bg) < 1.2);
        // and it resolves into a complete theme
        let t = crate::Theme::resolve("tiny", f.clone(), None);
        assert_eq!(t.syntax[Role::String].fg, Rgb::hex(0x99c794));
        // round-trips through TOML
        let toml = f.to_toml("imported");
        let (back, warnings) = ThemeFile::parse(&toml).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back, f);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Monokai Pro (Filter Spectrum)"), "monokai-pro-filter-spectrum");
        assert_eq!(slug("  "), "imported");
        assert_eq!(slug("Rosé Pine"), "rosé-pine");
    }
}
