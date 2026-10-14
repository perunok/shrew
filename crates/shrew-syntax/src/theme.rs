//! Colors: the built-in `shrew` themes (dark and light), built in code as scope-selector rules, plus the UI palette
//! the app paints its chrome with so the gutter, status bar and pickers match the text.

use std::str::FromStr;
use syntect::highlighting::{
    Color, FontStyle, ScopeSelectors, StyleModifier, Theme as SynTheme, ThemeItem, ThemeSettings,
};

/// An opaque sRGB color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// `Rgb::hex(0xd6cdc1)`.
    pub const fn hex(v: u32) -> Rgb {
        Rgb { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 }
    }

    /// Channels as 0..=1 floats, still sRGB-encoded.
    pub fn to_f32(self) -> [f32; 3] {
        [self.r as f32 / 255.0, self.g as f32 / 255.0, self.b as f32 / 255.0]
    }

    /// Linear blend: `t = 0` is `self`, `t = 1` is `other`.
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8;
        Rgb { r: m(self.r, other.r), g: m(self.g, other.g), b: m(self.b, other.b) }
    }

    /// WCAG relative luminance.
    pub fn luminance(self) -> f32 {
        let lin = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG contrast ratio (1..=21).
    pub fn contrast(self, other: Rgb) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn to_syntect(self) -> Color {
        Color { r: self.r, g: self.g, b: self.b, a: 0xff }
    }

    /// Flattens a possibly translucent syntect color over `bg`.
    pub(crate) fn from_syntect(c: Color, bg: Rgb) -> Rgb {
        let fg = Rgb { r: c.r, g: c.g, b: c.b };
        if c.a == 0xff { fg } else { bg.mix(fg, c.a as f32 / 255.0) }
    }
}

/// A styled piece of one line: a byte range within the line's text (line ending excluded).
/// Spans are sorted and never overlap; text not covered by any span uses [`Palette::foreground`].
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

/// Colors for everything that is not syntax: the app's chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: Rgb,
    pub foreground: Rgb,
    /// Line-number gutter.
    pub gutter_background: Rgb,
    pub gutter_foreground: Rgb,
    /// Number of the cursor's line.
    pub gutter_active: Rgb,
    pub current_line: Rgb,
    pub selection: Rgb,
    /// Selection in an unfocused view.
    pub selection_inactive: Rgb,
    pub cursor: Rgb,
    /// Text drawn on top of a block cursor.
    pub cursor_text: Rgb,
    pub comment: Rgb,
    pub indent_guide: Rgb,
    pub indent_guide_active: Rgb,
    /// Visible tabs / trailing spaces.
    pub whitespace: Rgb,
    /// Background behind the bracket matching the one at the cursor.
    pub bracket_match: Rgb,
    pub search_match: Rgb,
    pub search_current: Rgb,
    /// Other occurrences of the word under the cursor.
    pub word_highlight: Rgb,
    /// Status bar, picker, command palette.
    pub panel_background: Rgb,
    pub panel_foreground: Rgb,
    /// Secondary text in panels (paths, hints, key bindings).
    pub panel_muted: Rgb,
    pub panel_border: Rgb,
    /// Highlighted row in a picker list.
    pub panel_selection: Rgb,
    /// Matched characters in fuzzy-picker results.
    pub fuzzy_match: Rgb,
    pub accent: Rgb,
    pub accent_alt: Rgb,
    pub error: Rgb,
    pub warning: Rgb,
    pub info: Rgb,
    pub success: Rgb,
    /// Vim mode badge backgrounds (text on them uses `background`).
    pub mode_normal: Rgb,
    pub mode_insert: Rgb,
    pub mode_visual: Rgb,
}

/// A complete theme: syntax colors and the matching UI palette.
#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub palette: Palette,
    syntax: SynTheme,
}

/// Names accepted by [`Theme::builtin`].
pub const BUILTIN_THEMES: &[&str] = &["shrew", "shrew-light"];

/// The syntax hues of a theme; [`rules`] turns them into scope rules.
struct Tones {
    fg: Rgb,
    comment: Rgb,
    keyword: Rgb,
    string: Rgb,
    number: Rgb,
    constant: Rgb,
    function: Rgb,
    ty: Rgb,
    macro_: Rgb,
    punct: Rgb,
    escape: Rgb,
    tag: Rgb,
    attr: Rgb,
    invalid: Rgb,
}

const B: u8 = 1; // bold
const I: u8 = 2; // italic
const U: u8 = 4; // underline

/// Scope selector -> (color, style bits). Later, more specific selectors win as in Sublime.
fn rules(t: &Tones) -> Vec<(&'static str, Option<Rgb>, u8)> {
    vec![
        ("comment, punctuation.definition.comment", Some(t.comment), I),
        ("comment.block.documentation, comment.line.documentation", Some(t.comment), I),
        ("string, punctuation.definition.string, meta.string", Some(t.string), 0),
        (
            "constant.character.escape, string.regexp, constant.other.placeholder, constant.character.format",
            Some(t.escape),
            0,
        ),
        ("constant.numeric", Some(t.number), 0),
        (
            "constant.language, constant.character, constant.other, support.constant, variable.other.constant",
            Some(t.constant),
            0,
        ),
        ("keyword, storage, keyword.operator.word, keyword.operator.logical.python", Some(t.keyword), 0),
        ("storage.type.primitive, storage.type.numeric, storage.type.builtin", Some(t.ty), 0),
        ("keyword.operator, punctuation, meta.brace", Some(t.punct), 0),
        ("variable.language, variable.parameter.function.language.special", Some(t.constant), I),
        ("variable.parameter", Some(t.fg), 0),
        (
            "entity.name.function, support.function, variable.function, meta.function-call.identifier, entity.name.method",
            Some(t.function),
            0,
        ),
        (
            "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.union, entity.name.trait, entity.name.interface, entity.name.impl, support.type, support.class, entity.other.inherited-class, meta.generic",
            Some(t.ty),
            0,
        ),
        (
            "storage.type.function, storage.type.struct, storage.type.enum, storage.type.trait, storage.type.impl, storage.type.module, storage.type.class, storage.type.type, storage.type.union",
            Some(t.keyword),
            0,
        ),
        ("entity.name.namespace, entity.name.module, support.module, meta.path", Some(t.ty), 0),
        (
            "entity.name.macro, support.macro, meta.annotation, meta.attribute, meta.decorator, entity.name.function.decorator, keyword.directive, meta.preprocessor, keyword.control.import.c, keyword.control.include",
            Some(t.macro_),
            0,
        ),
        (
            "storage.modifier.lifetime, entity.name.lifetime, entity.name.label, punctuation.definition.lifetime",
            Some(t.macro_),
            I,
        ),
        ("entity.name.tag, punctuation.definition.tag", Some(t.tag), 0),
        ("entity.other.attribute-name", Some(t.attr), 0),
        (
            "meta.mapping.key string, meta.structure.dictionary.key string, support.type.property-name, meta.object-literal.key, entity.name.tag.yaml, string.unquoted.key, entity.name.key",
            Some(t.attr),
            0,
        ),
        ("entity.name.constant, entity.name.variable.constant", Some(t.constant), 0),
        ("invalid, invalid.illegal", Some(t.invalid), U),
        ("invalid.deprecated", Some(t.invalid), I),
        // markup
        ("markup.heading, markup.heading entity.name.section, entity.name.section", Some(t.function), B),
        ("markup.bold", None, B),
        ("markup.italic", None, I),
        ("markup.underline", None, U),
        ("markup.underline.link, markup.link, meta.link string.other.link", Some(t.ty), U),
        ("markup.raw, markup.inline.raw, markup.raw.block", Some(t.string), 0),
        ("markup.quote", Some(t.comment), I),
        ("markup.list punctuation.definition.list_item, punctuation.definition.list_item", Some(t.keyword), 0),
        // diffs
        ("markup.inserted, meta.diff.header.to-file, punctuation.definition.inserted", Some(t.string), 0),
        ("markup.deleted, meta.diff.header.from-file, punctuation.definition.deleted", Some(t.invalid), 0),
        ("markup.changed, punctuation.definition.changed", Some(t.number), 0),
        ("meta.diff.range, meta.diff.index, meta.separator.diff", Some(t.ty), I),
    ]
}

fn build(name: &str, dark: bool, tones: &Tones, palette: Palette) -> Theme {
    let scopes = rules(tones)
        .into_iter()
        .map(|(sel, fg, bits)| {
            let mut fs = FontStyle::empty();
            if bits & B != 0 {
                fs |= FontStyle::BOLD;
            }
            if bits & I != 0 {
                fs |= FontStyle::ITALIC;
            }
            if bits & U != 0 {
                fs |= FontStyle::UNDERLINE;
            }
            ThemeItem {
                scope: ScopeSelectors::from_str(sel).expect("built-in selectors parse"),
                style: StyleModifier { foreground: fg.map(Rgb::to_syntect), background: None, font_style: Some(fs) },
            }
        })
        .collect();
    let p = &palette;
    let settings = ThemeSettings {
        foreground: Some(p.foreground.to_syntect()),
        background: Some(p.background.to_syntect()),
        caret: Some(p.cursor.to_syntect()),
        line_highlight: Some(p.current_line.to_syntect()),
        selection: Some(p.selection.to_syntect()),
        inactive_selection: Some(p.selection_inactive.to_syntect()),
        gutter: Some(p.gutter_background.to_syntect()),
        gutter_foreground: Some(p.gutter_foreground.to_syntect()),
        guide: Some(p.indent_guide.to_syntect()),
        active_guide: Some(p.indent_guide_active.to_syntect()),
        find_highlight: Some(p.search_match.to_syntect()),
        accent: Some(p.accent.to_syntect()),
        ..ThemeSettings::default()
    };
    Theme {
        name: name.to_string(),
        dark,
        palette,
        syntax: SynTheme { name: Some(name.to_string()), author: Some("shrew".into()), settings, scopes },
    }
}

impl Theme {
    /// The default: warm, low-contrast dark (earthy browns, terracotta keywords, sage strings).
    pub fn shrew() -> Theme {
        let t = Tones {
            fg: Rgb::hex(0xd8cec2),
            comment: Rgb::hex(0x8a7f73),
            keyword: Rgb::hex(0xe0905f),
            string: Rgb::hex(0xadbd7e),
            number: Rgb::hex(0xdcae68),
            constant: Rgb::hex(0xe2a777),
            function: Rgb::hex(0xe8c785),
            ty: Rgb::hex(0x93bcae),
            macro_: Rgb::hex(0xc9a2c6),
            punct: Rgb::hex(0xa99d90),
            escape: Rgb::hex(0x8fc2a6),
            tag: Rgb::hex(0xe0905f),
            attr: Rgb::hex(0xdcae68),
            invalid: Rgb::hex(0xea7468),
        };
        let bg = Rgb::hex(0x1f1b18);
        let palette = Palette {
            background: bg,
            foreground: t.fg,
            gutter_background: bg,
            gutter_foreground: Rgb::hex(0x5d544b),
            gutter_active: Rgb::hex(0xb3a698),
            current_line: Rgb::hex(0x27221e),
            selection: Rgb::hex(0x3d342c),
            selection_inactive: Rgb::hex(0x312a24),
            cursor: Rgb::hex(0xeaa35e),
            cursor_text: bg,
            comment: t.comment,
            indent_guide: Rgb::hex(0x2f2924),
            indent_guide_active: Rgb::hex(0x4a4038),
            whitespace: Rgb::hex(0x3f3730),
            bracket_match: Rgb::hex(0x4a3d30),
            search_match: Rgb::hex(0x4b3f26),
            search_current: Rgb::hex(0x7a5a2c),
            word_highlight: Rgb::hex(0x343029),
            panel_background: Rgb::hex(0x181512),
            panel_foreground: Rgb::hex(0xcdc2b5),
            panel_muted: Rgb::hex(0x857a6e),
            panel_border: Rgb::hex(0x332c26),
            panel_selection: Rgb::hex(0x342c25),
            fuzzy_match: Rgb::hex(0xeaa35e),
            accent: Rgb::hex(0xeaa35e),
            accent_alt: Rgb::hex(0x93bcae),
            error: Rgb::hex(0xea7468),
            warning: Rgb::hex(0xe2b45e),
            info: Rgb::hex(0x8fb4d0),
            success: Rgb::hex(0xadbd7e),
            mode_normal: Rgb::hex(0xeaa35e),
            mode_insert: Rgb::hex(0xadbd7e),
            mode_visual: Rgb::hex(0xc9a2c6),
        };
        build("shrew", true, &t, palette)
    }

    /// The light variant: warm paper background, the same hues darkened for contrast.
    pub fn shrew_light() -> Theme {
        let t = Tones {
            fg: Rgb::hex(0x3a322b),
            comment: Rgb::hex(0x857a6d),
            keyword: Rgb::hex(0xa4501f),
            string: Rgb::hex(0x56692a),
            number: Rgb::hex(0x91600f),
            constant: Rgb::hex(0x9b5a26),
            function: Rgb::hex(0x7d5800),
            ty: Rgb::hex(0x2c6b5f),
            macro_: Rgb::hex(0x84468a),
            punct: Rgb::hex(0x6b6056),
            escape: Rgb::hex(0x2f7356),
            tag: Rgb::hex(0xa4501f),
            attr: Rgb::hex(0x91600f),
            invalid: Rgb::hex(0xc0392b),
        };
        let bg = Rgb::hex(0xf7f2ea);
        let palette = Palette {
            background: bg,
            foreground: t.fg,
            gutter_background: bg,
            gutter_foreground: Rgb::hex(0xb2a698),
            gutter_active: Rgb::hex(0x5f554b),
            current_line: Rgb::hex(0xefe8dc),
            selection: Rgb::hex(0xe2d6c3),
            selection_inactive: Rgb::hex(0xe9e1d4),
            cursor: Rgb::hex(0xc0662a),
            cursor_text: bg,
            comment: t.comment,
            indent_guide: Rgb::hex(0xe6ddd0),
            indent_guide_active: Rgb::hex(0xcfc3b2),
            whitespace: Rgb::hex(0xd6cbbb),
            bracket_match: Rgb::hex(0xe6d3b8),
            search_match: Rgb::hex(0xf0dca8),
            search_current: Rgb::hex(0xe8bd72),
            word_highlight: Rgb::hex(0xebe2d3),
            panel_background: Rgb::hex(0xede6da),
            panel_foreground: Rgb::hex(0x3a322b),
            panel_muted: Rgb::hex(0x857a6d),
            panel_border: Rgb::hex(0xd9cebf),
            panel_selection: Rgb::hex(0xe0d5c4),
            fuzzy_match: Rgb::hex(0xa4501f),
            accent: Rgb::hex(0xc0662a),
            accent_alt: Rgb::hex(0x2c6b5f),
            error: Rgb::hex(0xc0392b),
            warning: Rgb::hex(0xa66f00),
            info: Rgb::hex(0x2f6390),
            success: Rgb::hex(0x56692a),
            mode_normal: Rgb::hex(0xc0662a),
            mode_insert: Rgb::hex(0x56692a),
            mode_visual: Rgb::hex(0x84468a),
        };
        build("shrew-light", false, &t, palette)
    }

    /// A built-in theme by name (see [`BUILTIN_THEMES`]).
    pub fn builtin(name: &str) -> Option<Theme> {
        match name {
            "shrew" | "shrew-dark" => Some(Theme::shrew()),
            "shrew-light" => Some(Theme::shrew_light()),
            _ => None,
        }
    }

    /// The syntect theme the highlighter runs on.
    pub fn syntect(&self) -> &SynTheme {
        &self.syntax
    }

    /// Wraps a foreign syntect theme, deriving the UI palette from its settings (missing colors are mixed from the
    /// background and foreground).
    pub fn from_syntect(name: &str, syntax: SynTheme) -> Theme {
        let s = &syntax.settings;
        let base = Theme::shrew();
        let bg = s.background.map(|c| Rgb::from_syntect(c, Rgb::default())).unwrap_or(base.palette.background);
        let fg = s.foreground.map(|c| Rgb::from_syntect(c, bg)).unwrap_or(base.palette.foreground);
        let dark = bg.luminance() < 0.25;
        let get = |c: Option<Color>, t: f32| c.map(|c| Rgb::from_syntect(c, bg)).unwrap_or(bg.mix(fg, t));
        let accent = s.caret.or(s.accent).map(|c| Rgb::from_syntect(c, bg)).unwrap_or(fg);
        let comment = scope_fg(&syntax, "comment", bg).unwrap_or(bg.mix(fg, 0.5));
        let string = scope_fg(&syntax, "string", bg).unwrap_or(fg);
        let keyword = scope_fg(&syntax, "keyword", bg).unwrap_or(accent);
        let palette = Palette {
            background: bg,
            foreground: fg,
            gutter_background: get(s.gutter, 0.0),
            gutter_foreground: get(s.gutter_foreground, 0.3),
            gutter_active: bg.mix(fg, 0.7),
            current_line: get(s.line_highlight, 0.05),
            selection: get(s.selection, 0.18),
            selection_inactive: get(s.inactive_selection, 0.11),
            cursor: accent,
            cursor_text: bg,
            comment,
            indent_guide: get(s.guide, 0.08),
            indent_guide_active: get(s.active_guide, 0.2),
            whitespace: bg.mix(fg, 0.15),
            bracket_match: bg.mix(accent, 0.25),
            search_match: get(s.find_highlight, 0.2),
            search_current: bg.mix(accent, 0.45),
            word_highlight: bg.mix(fg, 0.1),
            panel_background: bg.mix(if dark { Rgb::hex(0) } else { Rgb::hex(0xffffff) }, 0.2),
            panel_foreground: fg,
            panel_muted: bg.mix(fg, 0.55),
            panel_border: bg.mix(fg, 0.15),
            panel_selection: bg.mix(fg, 0.12),
            fuzzy_match: accent,
            accent,
            accent_alt: keyword,
            error: base.palette.error,
            warning: base.palette.warning,
            info: base.palette.info,
            success: string,
            mode_normal: accent,
            mode_insert: string,
            mode_visual: keyword,
        };
        Theme { name: name.to_string(), dark, palette, syntax }
    }

    /// Loads a `.tmTheme` color scheme.
    #[cfg(feature = "tmtheme")]
    pub fn from_tmtheme(path: &std::path::Path) -> Result<Theme, String> {
        let syntax =
            syntect::highlighting::ThemeSet::get_theme(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = syntax
            .name
            .clone()
            .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        Ok(Theme::from_syntect(&name, syntax))
    }

    /// The color a scope stack like `"source.rust comment.line"` gets (for tests and theme previews).
    pub fn style_for(&self, scopes: &str) -> (Rgb, FontStyle) {
        use syntect::parsing::Scope;
        let stack: Vec<Scope> = scopes.split_whitespace().filter_map(|s| Scope::new(s).ok()).collect();
        let st = syntect::highlighting::Highlighter::new(&self.syntax).style_for_stack(&stack);
        (Rgb::from_syntect(st.foreground, self.palette.background), st.font_style)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::shrew()
    }
}

fn scope_fg(theme: &SynTheme, scope: &str, bg: Rgb) -> Option<Rgb> {
    let s = syntect::parsing::Scope::new(scope).ok()?;
    theme
        .scopes
        .iter()
        .filter(|i| i.scope.does_match(&[s]).is_some())
        .find_map(|i| i.style.foreground)
        .map(|c| Rgb::from_syntect(c, bg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_contrast() {
        for theme in [Theme::shrew(), Theme::shrew_light()] {
            let p = theme.palette;
            assert!(p.foreground.contrast(p.background) >= 7.0, "{} fg", theme.name);
            for scope in [
                "keyword.control",
                "string.quoted.double",
                "constant.numeric",
                "entity.name.function",
                "entity.name.type",
                "support.macro",
                "constant.character.escape",
                "punctuation.separator",
            ] {
                let (c, _) = theme.style_for(scope);
                let ratio = c.contrast(p.background);
                assert!(ratio >= 4.5, "{} {scope}: contrast {ratio:.2}", theme.name);
                assert!(ratio <= 13.0, "{} {scope}: too harsh {ratio:.2}", theme.name);
            }
            assert!(p.comment.contrast(p.background) >= 3.5, "{} comment", theme.name);
            assert!(p.foreground.contrast(p.selection) >= 4.5, "{} selection", theme.name);
            assert!(p.foreground.contrast(p.current_line) >= 7.0, "{} current line", theme.name);
            assert!(p.gutter_foreground.contrast(p.background) >= 1.8, "{} gutter", theme.name);
        }
    }

    #[test]
    fn rules_resolve() {
        let t = Theme::shrew();
        let (kw, _) = t.style_for("source.rust keyword.control.rust");
        assert_eq!(kw, Rgb::hex(0xe0905f));
        let (c, fs) = t.style_for("source.rust comment.line.double-slash.rust");
        assert_eq!(c, t.palette.comment);
        assert!(fs.contains(FontStyle::ITALIC));
        let (f, _) = t.style_for("source.rust meta.function.rust entity.name.function.rust");
        assert_eq!(f, Rgb::hex(0xe8c785));
        // plain text falls back to the foreground
        let (fg, _) = t.style_for("source.rust");
        assert_eq!(fg, t.palette.foreground);
    }

    #[test]
    fn foreign_theme_palette() {
        let base = Theme::shrew_light();
        let t = Theme::from_syntect("x", base.syntect().clone());
        assert!(!t.dark);
        assert_eq!(t.palette.background, base.palette.background);
        assert_eq!(t.palette.comment, base.palette.comment);
    }
}
