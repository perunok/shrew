//! The Settings panel: descriptors' get/set/persist round trips, keys, search, reset and the mouse.

use super::rows::{Kind, Section};
use super::*;
use crate::draw::{self, Ui};
use crate::layout::Layout;
use shrew_core::features::{self, Feature};
use shrew_render::{Metrics, Scene};
use std::path::PathBuf;
use std::sync::Arc;

fn temp_config(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shrew-settings-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, shrew_config::default_config_text()).unwrap();
    path
}

fn state(config: Option<PathBuf>) -> State {
    let mut st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
    st.config_path = config;
    st
}

fn keys(st: &mut State, keys: &str) {
    for k in KeyPress::parse_seq(keys) {
        st.key(k);
    }
}

fn file(path: &Path) -> (shrew_config::Config, String) {
    let text = std::fs::read_to_string(path).unwrap();
    let (c, w) = shrew_config::parse(&text, "config.toml");
    assert!(w.is_empty(), "{w:?}");
    (c, text)
}

/// The row with config key `key` (index into the panel's rows).
fn row(st: &State, key: &str) -> usize {
    let p = st.settings.panel.as_ref().expect("open");
    p.rows.iter().position(|r| r.key == key).unwrap_or_else(|| panic!("no row {key}"))
}

/// Selects a row as the keys would: its section, then the row.
fn select(st: &mut State, key: &str) {
    let i = row(st, key);
    let p = st.panel();
    let s = p.rows[i].section;
    p.set_section(Section::ALL.iter().position(|&x| x == s).unwrap());
    p.row = p.shown().iter().position(|&x| x == i).unwrap();
    p.focus = Focus::Rows;
}

fn frame(st: &mut State, ui: &mut Ui) -> Layout {
    let m = Metrics {
        font_size: 15.0,
        cell_width: 9.0,
        cell_height: 21.0,
        baseline: 16.0,
        ascent: 14.0,
        descent: 4.0,
        underline_offset: 18.0,
        underline_thickness: 1.0,
    };
    let lay = Layout::new(&m, (1280, 800), 1.0, st.ed.buf().doc.rope.len_lines(), true, false);
    let mut scene = Scene::new();
    draw::build(&mut scene, st, ui, &lay, std::time::Instant::now());
    lay
}

#[test]
fn opens_from_everywhere_and_closes() {
    let mut st = state(None);
    keys(&mut st, "<C-,>");
    assert!(st.settings.is_open(), "Ctrl+,");
    keys(&mut st, "<Esc>");
    assert!(!st.settings.is_open());
    keys(&mut st, ":settings ai<CR>");
    assert_eq!(st.settings.panel.as_ref().unwrap().section(), Section::Ai);
    keys(&mut st, "q");
    assert!(!st.settings.is_open());
    // the palette's entry
    st.ed.run(shrew_core::Command::Settings);
    st.process_events();
    assert_eq!(st.settings.panel.as_ref().unwrap().section(), Section::Ai, "reopens where it was");
    keys(&mut st, "<C-,>");
    assert!(!st.settings.is_open(), "Ctrl+, closes it again");
    keys(&mut st, ":settings nope<CR>");
    assert!(st.ed.message.as_ref().is_some_and(|m| m.1));
    // a feature like the others: switched off (here from its own row), it closes and stays closed
    st.open_settings("features");
    select(&mut st, "features.settings");
    keys(&mut st, "<Space>");
    assert!(!st.settings.is_open());
    keys(&mut st, "<C-,>");
    assert!(!st.settings.is_open());
    keys(&mut st, ":set settings<CR><C-,>");
    assert!(st.settings.is_open());
}

#[test]
fn every_section_has_rows_and_every_feature_is_listed() {
    let mut st = state(None);
    st.open_settings("");
    let p = st.settings.panel.as_ref().unwrap();
    for s in Section::ALL {
        assert!(p.rows.iter().any(|r| r.section == s), "{s:?} is empty");
    }
    for i in features::FEATURES {
        let key = format!("features.{}", i.key);
        assert!(p.rows.iter().any(|r| r.section == Section::Features && r.key == key), "{key}");
    }
    // the built-in chords are listed, Ctrl+, among them
    let chords = p.rows.iter().filter(|r| r.section == Section::Keybindings);
    assert!(chords.clone().any(|r| r.label == pretty_key(KeyPress::ctrl(','))));
    assert!(chords.clone().any(|r| r.label == pretty_key(KeyPress::ctrl('s'))));
}

#[test]
fn rows_round_trip_through_config_toml() {
    let path = temp_config("round-trip");
    let mut st = state(Some(path.clone()));
    st.open_settings("editor");

    // a number: tab width
    select(&mut st, "editor.tab_width");
    keys(&mut st, "+++");
    assert_eq!(st.ed.settings.tab_width, 7);
    assert_eq!(file(&path).0.editor.tab_width, Some(7));
    keys(&mut st, "<<");
    assert_eq!(st.ed.settings.tab_width, 1, "clamped to the range");

    // a feature: the same event :set uses, persisted under [features]
    select(&mut st, "features.whitespace");
    keys(&mut st, "<Space>");
    assert!(st.ed.features.on(Feature::Whitespace));
    assert_eq!(file(&path).0.feature(Feature::Whitespace), Some(true));

    // a plain bool
    select(&mut st, "editor.expand_tab");
    keys(&mut st, "<CR>");
    assert!(!st.ed.settings.expand_tab);
    assert_eq!(file(&path).0.editor.expand_tab, Some(false));

    // an options list inside the panel
    select(&mut st, "editor.leader");
    keys(&mut st, "<CR>");
    assert!(st.settings.panel.as_ref().unwrap().list.is_some());
    keys(&mut st, "j<CR>");
    assert_eq!(st.ed.keymap.leader, KeyPress::ch(','));
    assert_eq!(file(&path).0.editor.leader.as_deref(), Some(","));

    // fonts: size and line height (snapped to its step)
    st.open_settings("appearance");
    select(&mut st, "font.size");
    keys(&mut st, "<Right><Right>");
    assert_eq!(st.font.size, shrew_config::DEFAULT_FONT_SIZE + 2.0);
    assert!(st.font_changed);
    assert_eq!(file(&path).0.editor.font_size(), st.font.size);
    select(&mut st, "font.line_height");
    keys(&mut st, "-");
    assert!((st.font.line_height - 1.35).abs() < 1e-4, "{}", st.font.line_height);
    assert!((file(&path).0.editor.line_height() - 1.35).abs() < 1e-4);

    // themes for a light and a dark desktop
    select(&mut st, "theme.system_dark");
    keys(&mut st, "<CR>");
    let list = st.settings.panel.as_ref().unwrap().list.as_ref().unwrap();
    let nord = list.options.iter().position(|o| o.0 == "nord").expect("nord is offered");
    st.panel().list.as_mut().unwrap().selected = nord;
    keys(&mut st, "<CR>");
    assert_eq!(st.look.cfg.system_dark, "nord");
    assert_eq!(file(&path).0.editor.system_dark(), "nord");

    // picks open the app's pickers, which preview and save as always
    select(&mut st, "theme.accent");
    keys(&mut st, "<CR>");
    assert!(st.ed.picker.is_some(), "the accent picker is over the panel");
    keys(&mut st, "teal<CR>");
    assert!(st.ed.picker.is_none() && st.settings.is_open());
    assert_eq!(file(&path).0.editor.accent(), "teal");

    // r puts one row back, R the whole section
    select(&mut st, "theme.accent");
    keys(&mut st, "r");
    assert_eq!(file(&path).0.editor.accent(), shrew_config::DEFAULT_ACCENT);
    keys(&mut st, "R");
    assert_eq!(st.font.size, shrew_config::DEFAULT_FONT_SIZE);
    assert_eq!(st.look.cfg.system_dark, shrew_config::DEFAULT_THEME);
    let (c, text) = file(&path);
    assert_eq!(c.editor.font_size(), shrew_config::DEFAULT_FONT_SIZE);
    assert!(text.contains("# Columns per indent level"), "comments are kept");
    st.open_settings("editor");
    keys(&mut st, "R");
    assert_eq!(st.ed.settings.tab_width, 4);
    assert!(st.ed.settings.expand_tab);
    assert!(!st.ed.features.on(Feature::Whitespace));
    assert_eq!(file(&path).0.feature(Feature::Whitespace), Some(false));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn keys_move_between_columns_and_search_finds_rows() {
    let mut st = state(None);
    st.open_settings("appearance");
    keys(&mut st, "jj");
    assert_eq!(st.settings.panel.as_ref().unwrap().section(), Section::Features);
    keys(&mut st, "ljj");
    {
        let p = st.settings.panel.as_ref().unwrap();
        assert_eq!((p.focus, p.row), (Focus::Rows, 2));
    }
    keys(&mut st, "h<Tab>G");
    let p = st.settings.panel.as_ref().unwrap();
    assert_eq!(p.row + 1, p.shown().len());

    keys(&mut st, "/scroll");
    let p = st.settings.panel.as_ref().unwrap();
    assert_eq!(p.focus, Focus::Search);
    let found: Vec<&str> = p.shown().iter().map(|&i| p.rows[i].key.as_str()).collect();
    assert!(found.contains(&"editor.scrolloff") && found.contains(&"features.smooth_scroll"), "{found:?}");
    keys(&mut st, "<CR>");
    assert_eq!(st.settings.panel.as_ref().unwrap().focus, Focus::Rows);
    // Esc ends the search, the next one closes the panel
    keys(&mut st, "<Esc>");
    assert!(st.settings.is_open() && !st.settings.panel.as_ref().unwrap().searching());
    keys(&mut st, "<Esc>");
    assert!(!st.settings.is_open());
}

#[test]
fn actions_can_ask_first() {
    let mut st = state(None);
    st.open_settings("ai");
    if !cfg!(feature = "mcp") {
        return;
    }
    select(&mut st, "mcp.claude_code");
    keys(&mut st, "<CR>");
    let copied = st.ed.clipboard.get().unwrap_or_default();
    assert!(copied.starts_with("claude mcp add shrew -- ") && copied.ends_with(" --mcp-stdio"), "{copied}");
    select(&mut st, "mcp.json");
    keys(&mut st, "<CR>");
    assert!(st.ed.clipboard.get().unwrap_or_default().contains("\"mcpServers\""));
    // the token is only made new on a second Enter; anything else in between disarms it
    select(&mut st, "mcp.token");
    keys(&mut st, "<CR>");
    assert_eq!(st.settings.panel.as_ref().unwrap().armed, Some(row(&st, "mcp.token")));
    keys(&mut st, "j");
    assert_eq!(st.settings.panel.as_ref().unwrap().armed, None);
}

#[test]
fn key_mappings_open_config_toml_at_keys() {
    let path = temp_config("keys");
    let mut st = state(Some(path.clone()));
    st.open_settings("keybindings");
    select(&mut st, "keys");
    keys(&mut st, "<CR>");
    assert!(!st.settings.is_open());
    let b = st.ed.buf();
    assert_eq!(b.doc.path.as_deref(), Some(path.as_path()));
    let line = shrew_core::text::line_of(b.rope(), b.sel.primary().head);
    assert!(b.rope().line(line).to_string().starts_with("[keys.normal]"));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn every_section_draws_and_clicks_hit_their_controls() {
    let path = temp_config("mouse");
    let mut st = state(Some(path.clone()));
    let mut ui = Ui::new();
    ui.animate = false;
    for s in Section::ALL {
        st.open_settings(s.label());
        frame(&mut st, &mut ui);
        let p = st.settings.panel.as_ref().unwrap();
        assert!(!p.hits.rows.is_empty(), "{s:?} drew no rows");
        assert_eq!(p.hits.sections.len(), Section::ALL.len());
    }
    // a click on a switch flips it
    st.open_settings("editor");
    select(&mut st, "features.whitespace");
    frame(&mut st, &mut ui);
    let i = row(&st, "features.whitespace");
    let p = st.settings.panel.as_ref().unwrap();
    let n = p.shown().iter().position(|&x| x == i).unwrap();
    let hit = *p.hits.rows.iter().find(|h| h.index == n).expect("on screen");
    let (cx, cy) = (hit.control.x + hit.control.w - 2.0, hit.control.y + hit.control.h / 2.0);
    assert!(!st.settings_click(cx, cy));
    assert!(st.ed.features.on(Feature::Whitespace));
    // a slider follows the pointer
    st.open_settings("editor");
    frame(&mut st, &mut ui);
    let i = row(&st, "editor.tab_width");
    let p = st.settings.panel.as_ref().unwrap();
    let n = p.shown().iter().position(|&x| x == i).unwrap();
    let hit = *p.hits.rows.iter().find(|h| h.index == n).expect("on screen");
    assert!(st.settings_click(hit.slider.right() - 1.0, hit.slider.y + hit.slider.h / 2.0), "grabbed the slider");
    assert_eq!(st.ed.settings.tab_width, 16);
    st.settings_drag(hit.slider.x);
    st.settings_release();
    assert_eq!(st.ed.settings.tab_width, 1);
    assert!(!st.settings_click(hit.plus.x + 1.0, hit.plus.y + 1.0));
    assert_eq!(st.ed.settings.tab_width, 2);
    assert_eq!(file(&path).0.editor.tab_width, Some(2));
    // the wheel scrolls a long section, a click outside closes
    st.open_settings("keybindings");
    frame(&mut st, &mut ui);
    st.settings_wheel(400.0);
    assert!(st.settings.panel.as_ref().unwrap().scroll > 0.0);
    assert!(!st.settings_click(1.0, 1.0));
    assert!(!st.settings.is_open());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn rows_without_a_feature_are_left_out() {
    assert!(rows::feature_key(Section::Layout, "no_such_feature", "x").is_none());
    let r = rows::feature_key(Section::Layout, "linenumbers", "Line numbers").expect("underscores do not matter");
    assert!(matches!(r.kind, Kind::Toggle { .. }));
}

#[test]
fn snapping_and_key_names() {
    assert_eq!(snap(1.37, (1.0, 2.5,), 0.05), 1.35);
    assert_eq!(snap(99.0, (1.0, 16.0), 1.0), 16.0);
    assert_eq!(snap(-3.0, (0.0, 30.0), 1.0), 0.0);
    let k = KeyPress::new(Key::Char('p'), shrew_core::Mods { ctrl: true, shift: true, ..Default::default() });
    if !cfg!(target_os = "macos") {
        assert_eq!(pretty_key(k), "Ctrl+Shift+P");
    }
    assert_eq!(pretty_key(KeyPress::new(Key::F(3), shrew_core::Mods { alt: true, ..Default::default() })), "Alt+F3");
    assert_eq!(view::wrap("one two three", 7), vec!["one two", "three"]);
}
