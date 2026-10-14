//! The built-in themes: TOML files compiled in, parsed only when one is used (startup parses just the active one).

use crate::theme::{Scheme, Theme, ThemeFile};

pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    pub scheme: Scheme,
    pub high_contrast: bool,
    pub source: &'static str,
}

macro_rules! builtins {
    ($(($id:literal, $name:literal, $scheme:ident $(, $hc:ident)?),)*) => {
        /// Every built-in theme, light ones first.
        pub static BUILTIN: &[Builtin] = &[$(Builtin {
            id: $id,
            name: $name,
            scheme: Scheme::$scheme,
            high_contrast: builtins!(@hc $($hc)?),
            source: include_str!(concat!("../themes/", $id, ".toml")),
        },)*];
    };
    (@hc hc) => { true };
    (@hc) => { false };
}

builtins! {
    ("shrew-light", "Shrew Light", Light),
    ("light", "Light", Light),
    ("paper", "Paper", Light),
    ("github-light", "GitHub Light", Light),
    ("solarized-light", "Solarized Light", Light),
    ("gruvbox-light", "Gruvbox Light", Light),
    ("catppuccin-latte", "Catppuccin Latte", Light),
    ("rose-pine-dawn", "Rosé Pine Dawn", Light),
    ("one-light", "One Light", Light),
    ("nord-light", "Nord Light", Light),
    ("ayu-light", "Ayu Light", Light),
    ("contrast-light", "High Contrast Light", Light, hc),
    ("shrew-dark", "Shrew Dark", Dark),
    ("dark", "Dark", Dark),
    ("midnight", "Midnight", Dark),
    ("solarized", "Solarized Dark", Dark),
    ("github-dark", "GitHub Dark", Dark),
    ("dracula", "Dracula", Dark),
    ("nord", "Nord", Dark),
    ("gruvbox-dark", "Gruvbox Dark", Dark),
    ("catppuccin-mocha", "Catppuccin Mocha", Dark),
    ("catppuccin-frappe", "Catppuccin Frappé", Dark),
    ("tokyo-night", "Tokyo Night", Dark),
    ("one-dark", "One Dark", Dark),
    ("monokai", "Monokai", Dark),
    ("rose-pine", "Rosé Pine", Dark),
    ("ayu-mirage", "Ayu Mirage", Dark),
    ("ayu-dark", "Ayu Dark", Dark),
    ("everforest", "Everforest", Dark),
    ("kanagawa", "Kanagawa", Dark),
    ("synthwave-84", "Synthwave '84", Dark),
    ("oceanic", "Oceanic", Dark),
    ("contrast", "High Contrast", Dark, hc),
}

/// Older names that still work.
fn alias(id: &str) -> &str {
    match id {
        "shrew" => "shrew-dark",
        "solarized-dark" => "solarized",
        "high-contrast" => "contrast",
        "high-contrast-light" => "contrast-light",
        "synthwave" | "synthwave84" => "synthwave-84",
        other => other,
    }
}

pub fn find(id: &str) -> Option<&'static Builtin> {
    let id = alias(id);
    BUILTIN.iter().find(|b| b.id == id)
}

/// The built-in theme file, parsed.
pub fn file(id: &str) -> Option<ThemeFile> {
    let b = find(id)?;
    let (f, warnings) = ThemeFile::parse(b.source).expect("built-in themes are valid TOML");
    debug_assert!(warnings.is_empty(), "{}: {warnings:?}", b.id);
    Some(f)
}

impl Theme {
    /// A built-in theme with its own accent.
    pub fn builtin(id: &str) -> Option<Theme> {
        let b = find(id)?;
        Some(Theme::resolve(b.id, file(b.id)?, None))
    }
}
