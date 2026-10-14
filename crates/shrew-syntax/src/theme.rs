//! A [`shrew_theme::Theme`] turned into the syntect theme the highlighter runs on: one syntect rule per entry of
//! [`SCOPE_RULES`](shrew_theme::SCOPE_RULES), colored by the theme's syntax roles.

use shrew_theme::SCOPE_RULES;
use std::str::FromStr;
use syntect::highlighting::{
    Color, FontStyle, ScopeSelectors, StyleModifier, Theme as SynTheme, ThemeItem, ThemeSettings,
};

pub use shrew_theme::Rgb;

/// A styled piece of one line: a byte range within the line's text (line ending excluded).
/// Spans are sorted and never overlap; text not covered by any span uses [`Theme::foreground`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub fg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Span {
    #[inline]
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// The highlighter's view of a theme: the syntect rules plus the colors text falls back to.
#[derive(Clone, Debug)]
pub struct Theme {
    /// The shrew theme id.
    pub name: String,
    pub dark: bool,
    pub foreground: Rgb,
    pub background: Rgb,
    syntax: SynTheme,
}

fn syn(c: Rgb) -> Color {
    Color { r: c.r, g: c.g, b: c.b, a: 0xff }
}

/// Flattens a possibly translucent syntect color over `bg`.
pub(crate) fn flatten(c: Color, bg: Rgb) -> Rgb {
    let fg = Rgb { r: c.r, g: c.g, b: c.b };
    if c.a == 0xff { fg } else { bg.mix(fg, c.a as f32 / 255.0) }
}

impl Theme {
    /// Builds the rules (about 40 selectors: tens of microseconds).
    pub fn new(t: &shrew_theme::Theme) -> Theme {
        let scopes = SCOPE_RULES
            .iter()
            .map(|&(sel, role, usual)| {
                let st = t.syntax[role];
                let f = st.font.unwrap_or(usual);
                let mut fs = FontStyle::empty();
                fs.set(FontStyle::BOLD, f.bold);
                fs.set(FontStyle::ITALIC, f.italic);
                fs.set(FontStyle::UNDERLINE, f.underline);
                ThemeItem {
                    scope: ScopeSelectors::from_str(sel).expect("built-in selectors parse"),
                    style: StyleModifier { foreground: Some(syn(st.fg)), background: None, font_style: Some(fs) },
                }
            })
            .collect();
        let u = &t.ui;
        let settings = ThemeSettings {
            foreground: Some(syn(u.text)),
            background: Some(syn(u.bg)),
            caret: Some(syn(u.cursor)),
            line_highlight: Some(syn(u.current_line)),
            selection: Some(syn(u.selection)),
            inactive_selection: Some(syn(u.selection_inactive)),
            gutter: Some(syn(u.gutter)),
            gutter_foreground: Some(syn(u.line_number)),
            guide: Some(syn(u.indent_guide)),
            active_guide: Some(syn(u.indent_guide_active)),
            find_highlight: Some(syn(u.search_match)),
            accent: Some(syn(u.accent)),
            ..ThemeSettings::default()
        };
        Theme {
            name: t.id.clone(),
            dark: t.dark(),
            foreground: u.text,
            background: u.bg,
            syntax: SynTheme { name: Some(t.name.clone()), author: None, settings, scopes },
        }
    }

    /// The default theme (shrew-dark).
    pub fn shrew() -> Theme {
        Theme::new(&shrew_theme::Theme::default())
    }

    pub fn shrew_light() -> Theme {
        Theme::builtin(shrew_theme::DEFAULT_LIGHT).expect("built in")
    }

    /// A built-in theme by id, with its own accent.
    pub fn builtin(id: &str) -> Option<Theme> {
        shrew_theme::Theme::builtin(id).map(|t| Theme::new(&t))
    }

    /// The syntect theme the highlighter runs on.
    pub fn syntect(&self) -> &SynTheme {
        &self.syntax
    }

    /// The color a scope stack like `"source.rust comment.line"` gets (for tests and theme previews).
    pub fn style_for(&self, scopes: &str) -> (Rgb, FontStyle) {
        use syntect::parsing::Scope;
        let stack: Vec<Scope> = scopes.split_whitespace().filter_map(|s| Scope::new(s).ok()).collect();
        let st = syntect::highlighting::Highlighter::new(&self.syntax).style_for_stack(&stack);
        (flatten(st.foreground, self.background), st.font_style)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::shrew()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shrew_theme::Role;

    #[test]
    fn rules_resolve() {
        let base = shrew_theme::Theme::default();
        let t = Theme::new(&base);
        let role = |r: Role| base.syntax[r].fg;
        assert_eq!(t.style_for("source.rust keyword.control.rust").0, role(Role::Control));
        assert_eq!(t.style_for("source.rust storage.type.function.rust").0, role(Role::Keyword));
        let (c, fs) = t.style_for("source.rust comment.line.double-slash.rust");
        assert_eq!(c, role(Role::Comment));
        assert!(fs.contains(FontStyle::ITALIC));
        assert_eq!(t.style_for("source.rust meta.function.rust entity.name.function.rust").0, role(Role::Function));
        assert_eq!(t.style_for("source.rust keyword.operator.arithmetic.rust").0, role(Role::Operator));
        assert_eq!(t.style_for("source.python keyword.operator.logical.python").0, role(Role::Keyword));
        assert_eq!(t.style_for("source.rust storage.type.primitive.rust").0, role(Role::BuiltinType));
        assert_eq!(t.style_for("source.rust meta.annotation.rust").0, role(Role::Attribute));
        assert_eq!(
            t.style_for("source.rust string.quoted.double.rust constant.character.escape.rust").0,
            role(Role::StringEscape)
        );
        assert_eq!(t.style_for("source.js string.regexp.js").0, role(Role::Regex));
        assert_eq!(t.style_for("source.js variable.parameter.js").0, role(Role::Parameter));
        assert_eq!(t.style_for("text.html.markdown markup.heading.1.markdown").1, FontStyle::BOLD);
        // plain text falls back to the foreground
        assert_eq!(t.style_for("source.rust").0, t.foreground);
    }

    #[test]
    fn theme_font_overrides_win() {
        let mono = shrew_theme::Theme::builtin("monokai").unwrap();
        let t = Theme::new(&mono);
        assert!(t.style_for("source.rust entity.name.struct.rust").1.contains(FontStyle::ITALIC), "Monokai types");
        assert_eq!(t.name, "monokai");
        assert!(t.dark);
    }
}
