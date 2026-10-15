//! The UI palette in renderer colors. Everything the app draws takes its colors from here, so the chrome always
//! matches the theme ([`shrew_theme::Theme::ui`]); the syntax colors reach the text through shrew-syntax's spans.

use shrew_render::Color;
use shrew_theme::{Rgb, Theme};

#[derive(Clone, Copy, Debug)]
// without git, its colors are unused
#[cfg_attr(not(feature = "git"), allow(dead_code))]
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
    // ----- git: gutter -----
    /// Dim hints (the inline blame).
    pub faint: Color,
    /// Change markers, file states and diff backgrounds: the theme's tokens of the same names.
    pub git_added: Color,
    pub git_modified: Color,
    pub git_deleted: Color,
    pub git_renamed: Color,
    pub git_untracked: Color,
    pub git_ignored: Color,
    pub git_conflict: Color,
    pub diff_add_bg: Color,
    pub diff_del_bg: Color,
    pub diff_add_word: Color,
    pub diff_del_word: Color,
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
            faint: k(u.text_faint),
            git_added: k(u.git_added),
            git_modified: k(u.git_modified),
            git_deleted: k(u.git_deleted),
            git_renamed: k(u.git_renamed),
            git_untracked: k(u.git_untracked),
            git_ignored: k(u.git_ignored),
            git_conflict: k(u.git_conflict),
            diff_add_bg: k(u.diff_add_bg),
            diff_del_bg: k(u.diff_del_bg),
            diff_add_word: k(u.diff_add_word),
            diff_del_word: k(u.diff_del_word),
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
