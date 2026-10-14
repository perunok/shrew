//! shrew's themes: UI palettes and syntax colors as data, 33 built in (shrew's own plus Slinger's 31, with their
//! editor palettes), 18 accents to lay over any of them, custom themes from TOML files, and importers for Sublime
//! `.sublime-color-scheme` and TextMate `.tmTheme` schemes. No GPU, no syntect: shrew-syntax turns a [`Theme`] into
//! highlighting rules ([`SCOPE_RULES`]) and the app paints its chrome from [`Theme::ui`].
//!
//! Built-in themes are TOML compiled into the binary and parsed only when used, so startup costs one small parse
//! (tens of microseconds) whatever the number of themes.
//!
//! ```
//! use shrew_theme::{Library, Role, Theme, accent};
//! let dracula = Theme::builtin("dracula").unwrap();
//! assert!(dracula.dark());
//! let rose = dracula.with_accent(accent("rose"));
//! assert_ne!(rose.ui.accent, dracula.ui.accent);
//! let kw = rose.syntax[Role::Keyword].fg;
//! // custom themes in a folder, falling back to the built-ins
//! let lib = Library::new(Some("/home/me/.config/shrew/themes".into()), vec![]);
//! let t = lib.load("nord", None).unwrap().theme;
//! # let _ = (kw, t);
//! ```

mod accent;
pub mod audit;
mod builtin;
mod color;
pub mod import;
mod library;
mod scopes;
mod theme;
mod tokens;

pub use accent::{ACCENTS, Accent, AccentVariant, THEME_ACCENT, accent};
pub use builtin::{BUILTIN, Builtin};
pub use color::{Rgb, parse_rgba};
pub use library::{Library, Loaded, Source, ThemeInfo};
pub use scopes::SCOPE_RULES;
pub use theme::{Scheme, Theme, ThemeFile};
pub use tokens::{Font, Role, Syntax, SyntaxSpec, SyntaxStyle, UI_KEYS, Ui, UiSpec};

/// The default theme, and the defaults for following the OS appearance.
pub const DEFAULT_DARK: &str = "shrew-dark";
pub const DEFAULT_LIGHT: &str = "shrew-light";
/// `theme = "system"`: follow the OS light/dark appearance.
pub const SYSTEM: &str = "system";

#[cfg(test)]
mod tests;
