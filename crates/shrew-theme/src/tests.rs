use crate::*;
use std::time::{Duration, Instant};

const SLINGER: &[&str] = &[
    "light",
    "paper",
    "github-light",
    "solarized-light",
    "gruvbox-light",
    "catppuccin-latte",
    "rose-pine-dawn",
    "one-light",
    "nord-light",
    "ayu-light",
    "contrast-light",
    "dark",
    "midnight",
    "solarized",
    "github-dark",
    "dracula",
    "nord",
    "gruvbox-dark",
    "catppuccin-mocha",
    "catppuccin-frappe",
    "tokyo-night",
    "one-dark",
    "monokai",
    "rose-pine",
    "ayu-mirage",
    "ayu-dark",
    "everforest",
    "kanagawa",
    "synthwave-84",
    "oceanic",
    "contrast",
];

#[test]
fn registry_matches_the_files() {
    for id in SLINGER {
        assert!(BUILTIN.iter().any(|b| b.id == *id), "{id} missing");
    }
    assert_eq!(BUILTIN.len(), SLINGER.len() + 2);
    for b in BUILTIN {
        let (f, warnings) = ThemeFile::parse(b.source).unwrap_or_else(|e| panic!("{}: {e}", b.id));
        assert!(warnings.is_empty(), "{}: {warnings:?}", b.id);
        assert_eq!(f.name.as_deref(), Some(b.name), "{}", b.id);
        assert_eq!(f.scheme, Some(b.scheme), "{}", b.id);
        assert_eq!(f.high_contrast.unwrap_or(false), b.high_contrast, "{}", b.id);
        assert_eq!(f.base, None, "built-ins stand alone");
        // every syntax role is spelled out, so a theme's look never depends on derivation rules changing
        for &r in Role::ALL {
            assert!(f.syntax.get(r).is_some(), "{}: syntax.{} missing", b.id, r.key());
        }
        for k in ["bg", "surface", "text", "text-muted", "accent", "accent-fg", "selection", "danger", "success"] {
            let mut ui = f.ui.clone();
            assert!(ui.slot(k).unwrap().is_some(), "{}: ui.{k} missing", b.id);
        }
        let dark = b.scheme == Scheme::Dark;
        assert_eq!(f.ui.bg.unwrap().luminance() < 0.2, dark, "{}: scheme does not match bg", b.id);
    }
}

/// The contrast audit (see `audit`) for every built-in theme with its own accent and each of the 18 accents.
#[test]
fn every_theme_and_accent_is_readable() {
    let mut failures = Vec::new();
    for b in BUILTIN {
        let base = Theme::builtin(b.id).unwrap();
        for a in std::iter::once(None).chain(ACCENTS.iter().map(Some)) {
            let t = base.with_accent(a);
            for f in audit::audit(&t) {
                failures.push(format!("{} + {}: {f}", b.id, a.map_or("theme", |a| a.id)));
            }
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn mode_badges_are_distinct() {
    for b in BUILTIN {
        let base = Theme::builtin(b.id).unwrap();
        for a in std::iter::once(None).chain(ACCENTS.iter().map(Some)) {
            let u = base.with_accent(a).ui;
            let distinct = |x: Rgb, y: Rgb| x.hue_distance(y) >= 30.0 || x.contrast(y) >= 1.6;
            let what = format!("{} + {}", b.id, a.map_or("theme", |a| a.id));
            assert!(
                distinct(u.mode_normal, u.mode_insert),
                "{what}: normal/insert {} {}",
                u.mode_normal,
                u.mode_insert
            );
            assert!(
                distinct(u.mode_normal, u.mode_visual),
                "{what}: normal/visual {} {}",
                u.mode_normal,
                u.mode_visual
            );
            assert!(
                distinct(u.mode_insert, u.mode_visual),
                "{what}: insert/visual {} {}",
                u.mode_insert,
                u.mode_visual
            );
        }
    }
}

#[test]
fn accents_follow_the_scheme() {
    let dark = Theme::builtin("dracula").unwrap();
    let light = Theme::builtin("github-light").unwrap();
    let a = accent("teal").unwrap();
    assert_eq!(dark.with_accent(Some(a)).ui.accent, a.dark.fill);
    assert_eq!(light.with_accent(Some(a)).ui.accent, a.light.fill);
    let t = dark.with_accent(Some(a));
    assert_eq!(t.ui.cursor, a.dark.fill, "the cursor follows the accent");
    assert_eq!(t.with_accent(None), dark);
    assert!(accent(THEME_ACCENT).is_none());
    // high contrast: pushed towards the text color, background as ink
    let hc = Theme::builtin("contrast").unwrap().with_accent(Some(a));
    assert_eq!(hc.ui.accent_fg, hc.ui.bg);
}

#[test]
fn the_default_keeps_its_look() {
    let t = Theme::default();
    assert_eq!(t.id, "shrew-dark");
    assert_eq!(t.ui.bg, Rgb::hex(0x1f1b18));
    assert_eq!(t.ui.cursor, Rgb::hex(0xeaa35e));
    assert_eq!(t.syntax[Role::Keyword].fg, Rgb::hex(0xe0905f));
    assert_eq!(t.syntax[Role::Comment].font, None, "italic comes from the scope rules");
    assert_eq!(Theme::builtin("shrew").unwrap().id, "shrew-dark");
    // not harsh: the default stays soft on the eyes
    for &r in Role::ALL {
        assert!(t.syntax[r].fg.contrast(t.ui.bg) <= 13.0, "{}", r.key());
    }
}

#[test]
fn derives_what_a_file_leaves_out() {
    let (f, w) = ThemeFile::parse("scheme = \"light\"\n[ui]\nbg = \"#ffffff\"\ntext = \"#222222\"\n").unwrap();
    assert!(w.is_empty());
    let t = Theme::resolve("x", f, None);
    assert_eq!(t.scheme, Scheme::Light);
    assert_ne!(t.ui.current_line, t.ui.bg);
    assert!(t.ui.text_muted.contrast(t.ui.bg) >= 4.5);
    assert_eq!(t.syntax[Role::Variable].fg, t.ui.text);
    let (_, w) =
        ThemeFile::parse("[ui]\nbg = \"red\"\nnope = \"#000\"\n[syntax]\nkeyword = { fg = \"#000\", wavy = true }\n")
            .unwrap();
    assert_eq!(w.len(), 3, "{w:?}");
    assert!(ThemeFile::parse("[ui").is_err());
}

#[test]
fn every_scope_rule_parses() {
    for (sel, _, _) in SCOPE_RULES {
        assert!(!sel.is_empty() && !sel.contains(",,"), "{sel}");
    }
}

/// Startup parses one theme: it must stay far below a millisecond.
#[test]
fn loading_a_theme_is_cheap() {
    let _ = Theme::builtin("dracula");
    let t = Instant::now();
    let n = 20;
    for _ in 0..n {
        std::hint::black_box(Theme::builtin("catppuccin-mocha").unwrap());
    }
    let each = t.elapsed() / n;
    println!("one built-in theme: {each:?}");
    let limit = if cfg!(debug_assertions) { Duration::from_millis(5) } else { Duration::from_millis(1) };
    assert!(each < limit, "{each:?}");
}

/// The terminal's colors: red, green, yellow, blue, magenta and cyan (plain and bright) are readable on the
/// background, and each is the hue its name says rather than one of the other five.
#[test]
fn ansi_colors_are_readable_and_on_hue() {
    for b in BUILTIN {
        let t = Theme::builtin(b.id).unwrap();
        let a = t.ui.ansi();
        let hues = [0.0, 120.0, 55.0, 215.0, 295.0, 185.0];
        for (i, &c) in a[1..7].iter().chain(&a[9..15]).enumerate() {
            let what = format!("{}: ansi {} {c}", b.id, if i < 6 { i + 1 } else { i + 3 });
            assert!(c.contrast(t.ui.bg) >= 4.4, "{what} is not readable");
            let at = |h: f32| c.hue_distance(Rgb::from_hsl(h, 0.6, 0.5));
            let own = at(hues[i % 6]);
            assert!(hues.iter().all(|&h| own <= at(h) + 12.0), "{what} looks like another hue");
        }
    }
}

// ----- git: gutter -----
/// Gutter marks and sidebar names: added, changed and deleted never look alike, and diff backgrounds are tints of
/// their own color, the word marks stronger than the line's.
#[test]
fn git_colors_are_distinct() {
    for b in BUILTIN {
        let base = Theme::builtin(b.id).unwrap();
        for a in std::iter::once(None).chain(ACCENTS.iter().map(Some)) {
            let u = base.with_accent(a).ui;
            let what = format!("{} + {}", b.id, a.map_or("theme", |a| a.id));
            let distinct = |x: Rgb, y: Rgb| x.hue_distance(y) >= 25.0 || x.contrast(y) >= 1.5;
            for (n, x, y) in [
                ("added/deleted", u.git_added, u.git_deleted),
                ("added/modified", u.git_added, u.git_modified),
                ("modified/deleted", u.git_modified, u.git_deleted),
            ] {
                assert!(distinct(x, y), "{what}: {n} {x} {y}");
            }
            assert_ne!(u.diff_add_bg, u.bg, "{what}: added lines have a background");
            assert_ne!(u.diff_del_bg, u.bg, "{what}: removed lines have a background");
            assert!(u.diff_add_word.contrast(u.bg) > u.diff_add_bg.contrast(u.bg), "{what}: word marks are stronger");
            assert!(u.diff_del_word.contrast(u.bg) > u.diff_del_bg.contrast(u.bg), "{what}: word marks are stronger");
        }
    }
}
