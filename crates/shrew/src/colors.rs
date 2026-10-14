//! The UI palette in renderer colors. Everything the app draws takes its colors from here, so the chrome always
//! matches the syntax theme (shrew-syntax's [`Palette`](shrew_syntax::Palette)); without the `syntax` feature a
//! copy of the default dark palette stands in.

use shrew_render::Color;

#[derive(Clone, Copy, Debug)]
pub struct Colors {
    pub dark: bool,
    pub background: Color,
    pub foreground: Color,
    pub gutter_background: Color,
    pub gutter_foreground: Color,
    pub gutter_active: Color,
    pub current_line: Color,
    pub selection: Color,
    pub selection_inactive: Color,
    pub cursor: Color,
    pub cursor_text: Color,
    pub indent_guide: Color,
    pub indent_guide_active: Color,
    pub whitespace: Color,
    pub bracket_match: Color,
    pub search_match: Color,
    pub search_current: Color,
    pub word_highlight: Color,
    pub panel_background: Color,
    pub panel_foreground: Color,
    pub panel_muted: Color,
    pub panel_border: Color,
    pub panel_selection: Color,
    pub fuzzy_match: Color,
    pub accent: Color,
    pub accent_alt: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub mode_normal: Color,
    pub mode_insert: Color,
    pub mode_visual: Color,
}

const fn c(v: u32) -> Color {
    Color::from_u32(v)
}

impl Colors {
    /// shrew's default dark palette (the same values as `shrew_syntax::Theme::shrew()`).
    pub const FALLBACK: Colors = Colors {
        dark: true,
        background: c(0x1f1b18),
        foreground: c(0xd8cec2),
        gutter_background: c(0x1f1b18),
        gutter_foreground: c(0x5d544b),
        gutter_active: c(0xb3a698),
        current_line: c(0x27221e),
        selection: c(0x3d342c),
        selection_inactive: c(0x312a24),
        cursor: c(0xeaa35e),
        cursor_text: c(0x1f1b18),
        indent_guide: c(0x2f2924),
        indent_guide_active: c(0x4a4038),
        whitespace: c(0x3f3730),
        bracket_match: c(0x4a3d30),
        search_match: c(0x4b3f26),
        search_current: c(0x7a5a2c),
        word_highlight: c(0x343029),
        panel_background: c(0x181512),
        panel_foreground: c(0xcdc2b5),
        panel_muted: c(0x857a6e),
        panel_border: c(0x332c26),
        panel_selection: c(0x342c25),
        fuzzy_match: c(0xeaa35e),
        accent: c(0xeaa35e),
        accent_alt: c(0x93bcae),
        error: c(0xea7468),
        warning: c(0xe2b45e),
        success: c(0xadbd7e),
        mode_normal: c(0xeaa35e),
        mode_insert: c(0xadbd7e),
        mode_visual: c(0xc9a2c6),
    };

    #[cfg(feature = "syntax")]
    pub fn from_theme(t: &shrew_syntax::Theme) -> Colors {
        let p = &t.palette;
        let k = |c: shrew_syntax::Rgb| Color::rgb(c.r, c.g, c.b);
        Colors {
            dark: t.dark,
            background: k(p.background),
            foreground: k(p.foreground),
            gutter_background: k(p.gutter_background),
            gutter_foreground: k(p.gutter_foreground),
            gutter_active: k(p.gutter_active),
            current_line: k(p.current_line),
            selection: k(p.selection),
            selection_inactive: k(p.selection_inactive),
            cursor: k(p.cursor),
            cursor_text: k(p.cursor_text),
            indent_guide: k(p.indent_guide),
            indent_guide_active: k(p.indent_guide_active),
            whitespace: k(p.whitespace),
            bracket_match: k(p.bracket_match),
            search_match: k(p.search_match),
            search_current: k(p.search_current),
            word_highlight: k(p.word_highlight),
            panel_background: k(p.panel_background),
            panel_foreground: k(p.panel_foreground),
            panel_muted: k(p.panel_muted),
            panel_border: k(p.panel_border),
            panel_selection: k(p.panel_selection),
            fuzzy_match: k(p.fuzzy_match),
            accent: k(p.accent),
            accent_alt: k(p.accent_alt),
            error: k(p.error),
            warning: k(p.warning),
            success: k(p.success),
            mode_normal: k(p.mode_normal),
            mode_insert: k(p.mode_insert),
            mode_visual: k(p.mode_visual),
        }
    }
}

/// `a` blended over `b` (`t = 0` is `b`, `t = 1` is `a`).
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let m = |x: u8, y: u8| (y as f32 + (x as f32 - y as f32) * t.clamp(0.0, 1.0)).round() as u8;
    Color::rgb(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

#[cfg(all(test, feature = "syntax"))]
mod tests {
    use super::*;

    #[test]
    fn fallback_matches_the_default_theme() {
        let t = Colors::from_theme(&shrew_syntax::Theme::shrew());
        let f = Colors::FALLBACK;
        assert_eq!(
            (t.background, t.foreground, t.cursor, t.panel_background, t.mode_visual),
            (f.background, f.foreground, f.cursor, f.panel_background, f.mode_visual)
        );
    }
}
