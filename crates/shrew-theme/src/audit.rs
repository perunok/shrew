//! WCAG contrast audit: every foreground/background pair shrew actually draws, with its target ratio. The built-in
//! themes pass it with every accent (a test enforces that); custom and imported themes get its failures as
//! warnings.
//!
//! Targets: text 4.5:1 (AA) wherever it sits; syntax colors 4.5:1 on the background and the current line, except
//! comments at 3:1 (they are meant to recede, and every classic palette puts them near 3:1); faint UI text, line
//! numbers, the cursor and accent fills against the background 3:1 (WCAG's non-text minimum). High-contrast themes
//! need 7:1 (AAA) for text and syntax, 4.5:1 for comments and the 3:1 group. Syntax colors are not checked on the
//! selection: it is a tint behind text, held to 4.5:1 against plain text only.

use crate::color::Rgb;
use crate::theme::Theme;
use crate::tokens::Role;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    /// 4.5, or 7 in high contrast.
    Text,
    /// 3, or 4.5 in high contrast.
    Ui,
}

/// A pair that misses its target.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub fg: &'static str,
    pub bg: &'static str,
    pub ratio: f32,
    pub target: f32,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} on {}: contrast {:.2}:1, wants {}:1", self.fg, self.bg, self.ratio, self.target)
    }
}

const TEXT_ON: &[&str] = &[
    "bg",
    "surface",
    "surface-raised",
    "surface-hover",
    "current-line",
    "selection",
    "accent-soft",
    "bracket-match",
    "search-match",
    "search-current",
    "word-highlight",
    "popup",
    "danger-soft",
    "success-soft",
    "warning-soft",
];

/// (fg token, bg tokens, level).
const UI_CHECKS: &[(&str, &[&str], Level)] = &[
    ("text", TEXT_ON, Level::Text),
    ("text-muted", &["bg", "surface", "surface-raised", "popup"], Level::Text),
    ("text-faint", &["bg", "surface"], Level::Ui),
    ("line-number", &["gutter"], Level::Ui),
    ("line-number-active", &["gutter"], Level::Text),
    ("accent-fg", &["accent"], Level::Text),
    ("accent", &["bg", "surface"], Level::Ui),
    ("accent-text", &["bg", "surface", "accent-soft", "popup"], Level::Text),
    // fuzzy-match letters on the selected picker row: bold, and the row's text is there to read too
    ("accent-text", &["surface-hover"], Level::Ui),
    ("cursor", &["bg", "current-line"], Level::Ui),
    ("cursor-fg", &["cursor"], Level::Text),
    ("danger", &["bg", "surface", "danger-soft"], Level::Text),
    ("success", &["bg", "surface", "success-soft"], Level::Text),
    ("warning", &["bg", "surface", "warning-soft"], Level::Text),
    ("danger-fg", &["danger"], Level::Text),
    ("success-fg", &["success"], Level::Text),
    ("info", &["surface"], Level::Text),
    ("mode-normal-fg", &["mode-normal"], Level::Text),
    ("mode-insert-fg", &["mode-insert"], Level::Text),
    ("mode-visual-fg", &["mode-visual"], Level::Text),
];

fn target(level: Level, hc: bool) -> f32 {
    match (level, hc) {
        (Level::Text, false) => 4.5,
        (Level::Text, true) => 7.0,
        (Level::Ui, false) => 3.0,
        (Level::Ui, true) => 4.5,
    }
}

/// Every pair of `theme` that misses its target.
pub fn audit(theme: &Theme) -> Vec<Failure> {
    let hc = theme.high_contrast;
    let ui = |k: &str| theme.ui.get(k).expect("audited tokens exist");
    let mut out = Vec::new();
    let mut check = |fg: &'static str, c: Rgb, bg: &'static str, level: Level| {
        let ratio = c.contrast(ui(bg));
        let target = target(level, hc);
        if ratio + 1e-4 < target {
            out.push(Failure { fg, bg, ratio, target });
        }
    };
    for &(fg, bgs, level) in UI_CHECKS {
        for &bg in bgs {
            check(fg, ui(fg), bg, level);
        }
    }
    for &r in Role::ALL {
        let level = if matches!(r, Role::Comment | Role::DocComment) { Level::Ui } else { Level::Text };
        for bg in ["bg", "current-line"] {
            check(r.key(), theme.syntax[r].fg, bg, level);
        }
    }
    out
}
