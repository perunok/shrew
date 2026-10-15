//! The UI palette in renderer colors. Everything the app draws takes its colors from here, so the chrome always
//! matches the theme ([`shrew_theme::Theme::ui`]); the syntax colors reach the text through shrew-syntax's spans.

use shrew_render::Color;
use shrew_theme::{Rgb, Theme};

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
    /// Tab bar and status bar.
    pub panel_background: Color,
    pub panel_foreground: Color,
    pub panel_muted: Color,
    pub panel_border: Color,
    /// The highlighted row in a picker.
    pub panel_selection: Color,
    /// Picker and palette.
    pub popup: Color,
    pub popup_border: Color,
    pub fuzzy_match: Color,
    pub accent: Color,
    pub accent_fg: Color,
    pub accent_alt: Color,
    pub error: Color,
    pub error_fg: Color,
    pub warning: Color,
    pub success: Color,
    pub success_fg: Color,
    pub mode_normal: Color,
    pub mode_normal_fg: Color,
    pub mode_insert: Color,
    pub mode_insert_fg: Color,
    pub mode_visual: Color,
    pub mode_visual_fg: Color,
    pub scrollbar: Color,
    /// The terminal's 16 ANSI colors (see [`shrew_theme::Ui::ansi`]).
    #[cfg_attr(not(feature = "terminal"), allow(dead_code))]
    pub ansi: [Color; 16],
    // ----- git: panel (placeholder until shrew-theme has the git tokens: derived here from the status colors) -----
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub git: GitColors,
}

/// File states, diff backgrounds and word highlights, named as the theme's git tokens.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub struct GitColors {
    pub added: Color,
    pub modified: Color,
    pub deleted: Color,
    pub renamed: Color,
    pub untracked: Color,
    pub ignored: Color,
    pub conflict: Color,
    pub diff_add_bg: Color,
    /// The diff view's (not drawn by the panel).
    #[allow(dead_code)]
    pub diff_del_bg: Color,
    #[allow(dead_code)]
    pub diff_add_word: Color,
    #[allow(dead_code)]
    pub diff_del_word: Color,
}

impl GitColors {
    fn derive(u: &shrew_theme::Ui) -> GitColors {
        let k = |c: Rgb| Color::rgb(c.r, c.g, c.b);
        let bg = k(u.bg);
        GitColors {
            added: k(u.success),
            modified: k(u.warning),
            deleted: k(u.danger),
            renamed: k(u.info),
            untracked: k(u.success),
            ignored: k(u.text_muted),
            conflict: mix(k(u.danger), k(u.warning), 0.6),
            diff_add_bg: mix(k(u.success), bg, 0.16),
            diff_del_bg: mix(k(u.danger), bg, 0.16),
            diff_add_word: mix(k(u.success), bg, 0.34),
            diff_del_word: mix(k(u.danger), bg, 0.34),
        }
    }
}

impl Colors {
    pub fn from_theme(t: &Theme) -> Colors {
        let u = &t.ui;
        let k = |c: Rgb| Color::rgb(c.r, c.g, c.b);
        Colors {
            dark: t.dark(),
            background: k(u.bg),
            foreground: k(u.text),
            gutter_background: k(u.gutter),
            gutter_foreground: k(u.line_number),
            gutter_active: k(u.line_number_active),
            current_line: k(u.current_line),
            selection: k(u.selection),
            selection_inactive: k(u.selection_inactive),
            cursor: k(u.cursor),
            cursor_text: k(u.cursor_fg),
            indent_guide: k(u.indent_guide),
            indent_guide_active: k(u.indent_guide_active),
            whitespace: k(u.whitespace),
            bracket_match: k(u.bracket_match),
            search_match: k(u.search_match),
            search_current: k(u.search_current),
            word_highlight: k(u.word_highlight),
            panel_background: k(u.surface),
            panel_foreground: k(u.text),
            panel_muted: k(u.text_muted),
            panel_border: k(u.border),
            panel_selection: k(u.surface_hover),
            popup: k(u.popup),
            popup_border: k(u.popup_border),
            fuzzy_match: k(u.accent_text),
            accent: k(u.accent),
            accent_fg: k(u.accent_fg),
            accent_alt: k(u.info),
            error: k(u.danger),
            error_fg: k(u.danger_fg),
            warning: k(u.warning),
            success: k(u.success),
            success_fg: k(u.success_fg),
            mode_normal: k(u.mode_normal),
            mode_normal_fg: k(u.mode_normal_fg),
            mode_insert: k(u.mode_insert),
            mode_insert_fg: k(u.mode_insert_fg),
            mode_visual: k(u.mode_visual),
            mode_visual_fg: k(u.mode_visual_fg),
            scrollbar: k(u.scrollbar),
            ansi: u.ansi().map(k),
            git: GitColors::derive(u),
        }
    }
}

/// `a` blended over `b` (`t = 0` is `b`, `t = 1` is `a`).
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let m = |x: u8, y: u8| (y as f32 + (x as f32 - y as f32) * t.clamp(0.0, 1.0)).round() as u8;
    Color::rgb(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_look() {
        let c = Colors::from_theme(&Theme::default());
        assert!(c.dark);
        assert_eq!(c.background, Color::from_u32(0x1f1b18));
        assert_eq!(c.cursor, Color::from_u32(0xeaa35e));
        assert_eq!(c.panel_background, Color::from_u32(0x181512));
    }
}
