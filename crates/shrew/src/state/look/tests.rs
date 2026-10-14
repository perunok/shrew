use super::*;
use shrew_core::KeyPress;
use std::sync::Arc;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("shrew-look-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A state with a real config file (and themes folder) in a temp dir.
fn state(dir: &std::path::Path) -> State {
    let path = dir.join("config.toml");
    let loaded = shrew_config::Loaded { path: Some(path), ..Default::default() };
    State::new(loaded, Arc::new(|| {}), true)
}

fn keys(st: &mut State, k: &str) {
    for k in KeyPress::parse_seq(k) {
        st.key(k);
    }
}

fn config(dir: &std::path::Path) -> shrew_config::Config {
    shrew_config::read_config(&dir.join("config.toml")).0
}

#[test]
fn theme_picker_previews_and_esc_restores() {
    let dir = temp("picker");
    let mut st = state(&dir);
    assert_eq!(st.look.theme.id, "shrew-dark");
    let bg = st.colors.background;
    keys(&mut st, ":select_theme<CR>");
    assert!(matches!(st.ed.picker.as_ref().map(|p| p.kind), Some(PickerKind::Choice(Choice::Theme))));
    keys(&mut st, "dracula");
    assert_eq!(st.look.theme.id, "dracula", "the highlighted row is previewed");
    assert_ne!(st.colors.background, bg);
    keys(&mut st, "<Esc>");
    assert_eq!(st.look.theme.id, "shrew-dark", "Esc puts the old theme back");
    assert_eq!(st.colors.background, bg);
    assert!(!dir.join("config.toml").exists(), "nothing saved");
    // Enter keeps it and saves it
    keys(&mut st, ":select_theme<CR>nord<CR>");
    assert_eq!(st.look.theme.id, "nord");
    assert_eq!(config(&dir).editor.theme(), "nord");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn accent_and_font_pickers() {
    let dir = temp("accent");
    let mut st = state(&dir);
    keys(&mut st, ":accent<CR>teal");
    assert_eq!(st.look.theme.accent, Some("teal"));
    keys(&mut st, "<Esc>");
    assert_eq!(st.look.theme.accent, None);
    keys(&mut st, ":accent rose<CR>");
    assert_eq!(st.look.theme.accent, Some("rose"));
    assert_eq!(config(&dir).editor.accent(), "rose");
    // the accent survives a theme change
    keys(&mut st, ":theme github-light<CR>");
    assert_eq!((st.look.theme.id.as_str(), st.look.theme.accent), ("github-light", Some("rose")));
    keys(&mut st, ":accent nope<CR>");
    assert!(st.ed.message.as_ref().is_some_and(|m| m.1 && m.0.contains("unknown accent")));

    st.look.font_families = Some(vec!["JetBrains Mono".into(), "Shrew Mono".into()]);
    st.font_changed = false;
    keys(&mut st, ":font<CR>");
    assert_eq!(st.ed.picker.as_ref().unwrap().items.len(), 2, "the built-in one is listed once");
    keys(&mut st, "shrew");
    assert_eq!(st.font.family, "Shrew Mono");
    assert!(st.font_changed);
    keys(&mut st, "<Esc>");
    assert_eq!(st.font.family, crate::state::family_for("monospace"));
    keys(&mut st, ":font Shrew Mono<CR>");
    assert_eq!(config(&dir).editor.font_family(), "Shrew Mono");
    keys(&mut st, ":set noligatures<CR>");
    assert!(!st.font.ligatures);
    assert!(!config(&dir).editor.ligatures());
    keys(&mut st, ":set ligatures!<CR>");
    assert!(st.font.ligatures);
    // font size changes are saved too
    st.ed.run(shrew_core::Command::FontBigger);
    st.process_events();
    assert_eq!(config(&dir).editor.font_size(), shrew_config::DEFAULT_FONT_SIZE + 1.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_theme_keeps_the_current_one() {
    let dir = temp("unknown");
    let mut st = state(&dir);
    keys(&mut st, ":theme monokai<CR>:theme nonsense<CR>");
    assert_eq!(st.look.theme.id, "monokai");
    assert!(st.ed.message.as_ref().is_some_and(|m| m.1 && m.0.contains("unknown theme")));
    assert_eq!(config(&dir).editor.theme(), "monokai");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn system_theme_follows_the_desktop() {
    let dir = temp("system");
    let mut st = state(&dir);
    keys(&mut st, ":theme system<CR>");
    st.set_os_dark(false);
    assert_eq!(st.look.theme.id, "shrew-light");
    st.set_os_dark(true);
    assert_eq!(st.look.theme.id, "shrew-dark");
    assert_eq!(config(&dir).editor.theme(), "system");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn custom_theme_reloads_when_saved_and_imports_work() {
    let dir = temp("custom");
    let themes = dir.join("themes");
    std::fs::create_dir_all(&themes).unwrap();
    let file = themes.join("mine.toml");
    std::fs::write(&file, "name = \"Mine\"\nbase = \"dracula\"\n[ui]\nbg = \"#101010\"\n").unwrap();
    let mut st = state(&dir);
    keys(&mut st, ":theme mine<CR>");
    assert_eq!(st.look.theme.name, "Mine");
    assert_eq!(st.colors.background, shrew_render::Color::from_u32(0x101010));
    st.ed.open(&file).unwrap();
    st.after_input();
    keys(&mut st, ":%s/101010/202020/<CR>:w<CR>");
    assert_eq!(st.colors.background, shrew_render::Color::from_u32(0x202020), "{:?}", st.ed.message);

    let scheme = dir.join("Tiny.tmTheme");
    std::fs::write(
        &scheme,
        "<plist><dict><key>name</key><string>Tiny Tm</string><key>settings</key><array>\
         <dict><key>settings</key><dict><key>background</key><string>#fafafa</string>\
         <key>foreground</key><string>#202020</string></dict></dict></array></dict></plist>",
    )
    .unwrap();
    keys(&mut st, &format!(":theme import {}<CR>", scheme.display()));
    assert_eq!(st.look.theme.id, "tiny-tm", "{:?}", st.ed.message);
    assert!(themes.join("tiny-tm.toml").is_file());
    assert_eq!(config(&dir).editor.theme(), "tiny-tm");
    let _ = std::fs::remove_dir_all(&dir);
}
