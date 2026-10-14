//! Config file parsing, applying to an editor, layer precedence and comment-preserving writes.

use super::*;
use shrew_core::document::Document;
use shrew_core::{KeyPress, Selection};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("shrew-config-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn typed(e: &mut Editor, keys: &str) {
    for k in KeyPress::parse_seq(keys) {
        e.handle_key(k);
    }
}

#[test]
fn parses_config_with_warnings() {
    let (c, w) = parse(include_str!("../tests/fixtures/config.toml"), "config.toml");
    assert_eq!(c.editor.tab_width, Some(3));
    assert_eq!(c.editor.font_size, Some(16.0));
    assert_eq!(c.editor.theme, None);
    assert_eq!(c.editor.leader.as_deref(), Some(","));
    assert_eq!(c.feature(Feature::RelativeNumbers), Some(true));
    assert_eq!(c.feature(Feature::Whitespace), None);
    assert_eq!(c.feature(Feature::UpdateCheck), Some(false));
    assert_eq!(c.keys.len(), 5);
    assert_eq!(c.import.vimrc, Some(ImportSource::Off));
    assert_eq!(c.import.sublime, Some(ImportSource::Path("/nonexistent/sublime/User".into())));
    assert_eq!(c.mcp_port, None);
    let msgs: Vec<String> = w.iter().map(|w| w.to_string()).collect();
    assert_eq!(w.len(), 6, "{msgs:#?}");
    assert!(msgs.iter().any(|m| m.starts_with("config.toml:5: theme: expected a string")), "{msgs:#?}");
    assert!(msgs.iter().any(|m| m.contains("editor.bogus")));
    assert!(msgs.iter().any(|m| m.contains("\"nonexistent\"")));
    assert!(msgs.iter().any(|m| m.contains("keys.select")));
    assert!(msgs.iter().any(|m| m.contains("mcp.port")));
}

#[test]
fn broken_toml_is_a_warning() {
    let (c, w) = parse("[editor]\ntab_width = = 4\n", "c");
    assert_eq!(c, Config::default());
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].line, Some(2));
}

#[test]
fn default_file_parses_to_nothing() {
    let text = default_config_text();
    let (c, w) = parse(&text, "default");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(c, Config::default());
    // every feature and command is documented
    for f in shrew_core::features::FEATURES {
        assert!(text.contains(&format!("# {} = {}\n", f.key, f.default)), "{}", f.key);
        assert!(text.contains(f.description));
    }
    for (_, name, _) in shrew_core::commands::COMMANDS {
        assert!(text.contains(name));
    }
}

#[test]
fn apply_to_editor() {
    let (c, _) = parse(include_str!("../tests/fixtures/config.toml"), "config.toml");
    let mut e = Editor::new();
    e.install_document(Document::from_str("one\ntwo\nthree"));
    e.buf_mut().sel = Selection::point(0);
    let w = apply(&c, &mut e);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].message.contains("save_al"));
    assert_eq!(e.settings.tab_width, 3);
    assert!(e.features.on(Feature::RelativeNumbers));
    assert!(!e.features.on(Feature::UpdateCheck));
    assert_eq!(e.keymap.leader, KeyPress::ch(','));
    // <C-k> = "delete_line"
    e.handle_key(KeyPress::ctrl('k'));
    assert_eq!(e.buf().doc.rope.to_string(), "two\nthree");
    // jk = <Esc>
    typed(&mut e, "Ahijk");
    assert_eq!(e.mode, Mode::Normal);
    assert_eq!(e.buf().doc.rope.to_string(), "twohi\nthree");
    assert_eq!(e.keymap.get(MapMode::Normal, &KeyPress::parse_seq(",w")).map(|m| &m.rhs), Some(&Rhs::Ex("w".into())));
}

#[test]
fn key_rhs_forms() {
    let km = shrew_core::Keymap::default();
    assert_eq!(key_rhs(&km, "save"), Ok(Rhs::Command(Command::Save)));
    assert_eq!(key_rhs(&km, ":w<CR>"), Ok(Rhs::Ex("w".into())));
    assert_eq!(key_rhs(&km, ":%s/a/b/g"), Ok(Rhs::Ex("%s/a/b/g".into())));
    assert_eq!(key_rhs(&km, "<Nop>"), Ok(Rhs::Keys { keys: vec![], remap: false }));
    assert_eq!(key_rhs(&km, "y$"), Ok(Rhs::Keys { keys: KeyPress::parse_seq("y$"), remap: false }));
    assert!(key_rhs(&km, "delete_lines").is_err());
}

#[test]
fn precedence_sublime_vimrc_config() {
    let root = temp_dir("layers");
    let home = root.join("home");
    let cfg = root.join("cfg");
    let user = cfg.join("sublime-text/Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{ "tab_size": 8, "font_face": "Sublime Font", "font_size": 20, "line_numbers": true, "highlight_line": true, }"#,
    )
    .unwrap();
    std::fs::write(home.join(".vimrc"), "set ts=2 nonumber\nlet mapleader=\" \"\ninoremap jk <Esc>\n").unwrap();
    let config_path = root.join("shrew/config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &config_path,
        "[editor]\nfont_size = 16\n[features]\ncurrent_line = false\n[keys.normal]\n\"<leader>x\" = \"dd\"\n",
    )
    .unwrap();
    let env = Env { home: Some(home), config_dir: Some(cfg), appdata: None, os: Os::Linux };

    let l = load_with(Some(&config_path), &env);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert_eq!(l.imports.len(), 2);
    let c = &l.config;
    assert_eq!(c.editor.tab_width, Some(2), "vimrc over sublime");
    assert_eq!(c.feature(Feature::LineNumbers), Some(false), "vimrc over sublime");
    assert_eq!(c.editor.font_family(), "Sublime Font", "only sublime sets it");
    assert_eq!(c.editor.font_size(), 16.0, "config over sublime");
    assert_eq!(c.feature(Feature::CurrentLine), Some(false), "config over sublime");
    assert_eq!(c.editor.theme(), DEFAULT_THEME);

    let mut e = Editor::new();
    e.install_document(Document::from_str("a\nb"));
    assert!(apply(c, &mut e).is_empty());
    assert_eq!(e.settings.tab_width, 2);
    assert!(!e.features.on(Feature::LineNumbers));
    // config's <leader> is the vimrc's space
    typed(&mut e, " x");
    assert_eq!(e.buf().doc.rope.to_string(), "b");
    typed(&mut e, "ojk");
    assert_eq!(e.mode, Mode::Normal);

    // switching the importers off in config.toml
    std::fs::write(&config_path, "[features]\nvimrc = false\n[import]\nsublime = \"off\"\n").unwrap();
    let l = load_with(Some(&config_path), &env);
    assert!(l.imports.is_empty());
    assert_eq!(l.config.editor.tab_width, None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn import_problems_become_warnings() {
    let root = temp_dir("warn");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".vimrc"), "set ts=4\nautocmd BufRead * echo 1\nset foldmethod=marker\n").unwrap();
    let env = Env { home: Some(home.clone()), config_dir: None, appdata: None, os: Os::Linux };
    let l = load_with(None, &env);
    assert_eq!(l.imports[0].applied, 1);
    assert_eq!(l.imports[0].unsupported, 2);
    assert_eq!(l.warnings.len(), 1);
    let s = l.warnings[0].to_string();
    assert!(
        s.contains("2 unsupported") && s.contains("2: autocmd BufRead") && s.contains("set foldmethod=marker"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn persist_keeps_comments() {
    let dir = temp_dir("persist");
    let path = dir.join("shrew/config.toml");
    // no file yet: starts from the default text, uncommenting the feature's line in place
    persist_feature(&path, Feature::Vim, false).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains(
            "# Modal editing (normal/insert/visual). Off: always typing, Sublime-style\nvim = false\n# Syntax"
        ),
        "{text}"
    );
    assert!(!text.contains("# vim = true"));
    let (c, w) = parse(&text, "c");
    assert!(w.is_empty());
    assert_eq!(c.feature(Feature::Vim), Some(false));
    assert_eq!(c.features.len(), 1);

    // an existing value is replaced where it is, with its trailing comment
    let user = "# mine\n[editor]\nfont_size = 14 # big enough\n\n[features]\n# I like these\nvim = true   # modal!\n";
    std::fs::write(&path, user).unwrap();
    persist_feature(&path, Feature::Vim, false).unwrap();
    persist_value(&path, "editor", "font_size", 17.5).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        user.replace("vim = true", "vim = false").replace("14 #", "17.5 #")
    );

    // a key in neither form is added; a missing table is created
    persist_feature(&path, Feature::Whitespace, true).unwrap();
    persist_value(&path, "mcp", "port", 9000).unwrap();
    let (c, w) = parse(&std::fs::read_to_string(&path).unwrap(), "c");
    assert!(w.is_empty());
    assert_eq!(c.feature(Feature::Whitespace), Some(true));
    assert_eq!(c.feature(Feature::Vim), Some(false));
    assert_eq!(c.mcp_port, Some(9000));
    assert!(std::fs::read_to_string(&path).unwrap().starts_with("# mine\n[editor]\nfont_size = 17.5 # big enough\n"));

    assert!(!write_default_if_missing(&path).unwrap());
    let fresh = dir.join("fresh/config.toml");
    assert!(write_default_if_missing(&fresh).unwrap());
    assert_eq!(std::fs::read_to_string(&fresh).unwrap(), default_config_text());

    // a broken file is left alone
    std::fs::write(&path, "[features\nvim = true\n").unwrap();
    assert!(persist_feature(&path, Feature::Vim, false).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[features\nvim = true\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn theme_and_font_sections() {
    let text = "[editor]\ntheme = \"nord\"\nfont_size = 12\n[theme]\nname = \"dracula\"\naccent = \"rose\"\nsystem_light = \"paper\"\n\
                [font]\nfamily = \"Iosevka\"\nweight = \"medium\"\nligatures = false\nline_height = 1.2\nsize = 13.5\n";
    let (c, w) = parse(text, "c");
    assert!(w.is_empty(), "{w:?}");
    let e = &c.editor;
    assert_eq!(e.theme(), "dracula", "[theme] comes later and wins");
    assert_eq!(e.accent(), "rose");
    assert_eq!(e.system_light(), "paper");
    assert_eq!(e.system_dark(), DEFAULT_THEME);
    assert_eq!(e.font_family(), "Iosevka");
    assert_eq!(e.font_weight(), 500);
    assert!(!e.ligatures());
    assert_eq!(e.font_size(), 13.5);
    assert_eq!(e.line_height(), 1.2);
    let (_, w) = parse("[font]\nweight = \"chunky\"\nsize = 1\nnope = 1\n[theme]\nname = \"\"\n", "c");
    assert_eq!(w.len(), 4, "{w:?}");
    assert_eq!(font_weight("Semi-Bold"), Some(600));
}

#[test]
fn settings_are_written_where_the_file_keeps_them() {
    let dir = temp_dir("settings");
    let path = dir.join("config.toml");
    // an old-style [editor] key is updated in place
    std::fs::write(&path, "[editor]\ntheme = \"shrew-light\" # mine\nfont_size = 14\n").unwrap();
    persist_setting(&path, Setting::Theme, "nord").unwrap();
    persist_setting(&path, Setting::FontSize, 16.0).unwrap();
    persist_setting(&path, Setting::Accent, "teal").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("[editor]\ntheme = \"nord\" # mine\nfont_size = 16.0\n"), "{text}");
    let (c, w) = parse(&text, "c");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!((c.editor.theme(), c.editor.accent(), c.editor.font_size()), ("nord", "teal", 16.0));
    // the default file: the commented-out lines in [theme] / [font] are uncommented
    std::fs::write(&path, default_config_text()).unwrap();
    persist_setting(&path, Setting::Theme, "dracula").unwrap();
    persist_setting(&path, Setting::Ligatures, false).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[theme]\n") && text.contains("\nname = \"dracula\"\n"), "{text}");
    assert!(text.contains("\nligatures = false\n") && !text.contains("# ligatures = true"));
    let (c, w) = parse(&text, "c");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(c.editor.theme(), "dracula");
    assert!(!c.editor.ligatures());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_setting_round_trips_through_the_file() {
    let dir = temp_dir("every-setting");
    let path = dir.join("config.toml");
    std::fs::write(&path, default_config_text()).unwrap();
    persist_setting(&path, Setting::SystemLight, "github-light").unwrap();
    persist_setting(&path, Setting::SystemDark, "nord").unwrap();
    persist_setting(&path, Setting::TabWidth, 2).unwrap();
    persist_setting(&path, Setting::ExpandTab, false).unwrap();
    persist_setting(&path, Setting::Scrolloff, 8).unwrap();
    persist_setting(&path, Setting::CursorBlink, true).unwrap();
    persist_setting(&path, Setting::Leader, ",").unwrap();
    persist_setting(&path, Setting::MappingTimeout, 500).unwrap();
    persist_setting(&path, Setting::McpPort, 9100).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    // the commented defaults were uncommented in place: the explanations above them stay
    assert!(text.contains("# Columns per indent level, and how wide a tab is drawn.\ntab_width = 2\n"), "{text}");
    assert!(!text.contains("# scrolloff = 4"));
    let (c, w) = parse(&text, "c");
    assert!(w.is_empty(), "{w:?}");
    let e = &c.editor;
    assert_eq!((e.system_light(), e.system_dark()), ("github-light", "nord"));
    assert_eq!((e.tab_width, e.expand_tab, e.scrolloff), (Some(2), Some(false), Some(8)));
    assert!(e.cursor_blink());
    assert_eq!((e.leader.as_deref(), e.mapping_timeout_ms()), (Some(","), 500));
    assert_eq!(c.mcp_port(), 9100);
    let _ = std::fs::remove_dir_all(&dir);
}
