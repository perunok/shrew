//! Importing Sublime Text's user settings (`Preferences.sublime-settings`) and key bindings
//! (`Default (<OS>).sublime-keymap`) from its `Packages/User` directory.
//!
//! Key bindings go to insert mode (which is all of shrew when vim mode is off). They also go to normal and visual
//! mode when the chord cannot shadow a vim command: alt/super chords, ctrl+shift chords, function keys, and ctrl
//! keys vim leaves alone. Bindings with a `context` are conditional in Sublime and are skipped.

use crate::{Env, ImportReport, ImportSource, Imported, Os, Warning};
use serde_json::Value;
use shrew_core::commands::Command;
use shrew_core::features::Feature;
use shrew_core::keymap::{MapMode, Mapping, Rhs};
use shrew_core::{Key, KeyPress, Mods};
use std::path::{Path, PathBuf};

/// `Packages/User` directories, newest Sublime first.
pub fn candidates(env: &Env) -> Vec<PathBuf> {
    let user = |base: &Path, name: &str| base.join(name).join("Packages").join("User");
    let mut v = Vec::new();
    match env.os {
        Os::Linux => {
            if let Some(c) = &env.config_dir {
                v.push(user(c, "sublime-text"));
                v.push(user(c, "sublime-text-3"));
            }
        }
        Os::MacOs => {
            if let Some(h) = &env.home {
                let support = h.join("Library").join("Application Support");
                v.push(user(&support, "Sublime Text"));
                v.push(user(&support, "Sublime Text 3"));
            }
        }
        Os::Windows => {
            if let Some(a) = &env.appdata {
                v.push(user(a, "Sublime Text"));
                v.push(user(a, "Sublime Text 3"));
            }
        }
    }
    v
}

pub fn keymap_file_name(os: Os) -> &'static str {
    match os {
        Os::Linux => "Default (Linux).sublime-keymap",
        Os::MacOs => "Default (OSX).sublime-keymap",
        Os::Windows => "Default (Windows).sublime-keymap",
    }
}

/// Runs the import `src` asks for. `None` when off or no Sublime install was found.
pub fn import(src: &ImportSource, env: &Env) -> Option<Imported> {
    let dir = match src {
        ImportSource::Off => return None,
        ImportSource::Path(p) => p.clone(),
        ImportSource::Auto => candidates(env).into_iter().find(|d| d.is_dir())?,
    };
    Some(import_dir(&dir, env.os))
}

/// Reads settings and keymaps from one `Packages/User` directory.
pub fn import_dir(dir: &Path, os: Os) -> Imported {
    let mut out = Imported { report: ImportReport::new("sublime"), ..Default::default() };
    let files = ["Preferences.sublime-settings", "Default.sublime-keymap", keymap_file_name(os)];
    for (i, name) in files.iter().enumerate() {
        let path = dir.join(name);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                out.warnings.push(Warning::new(path.display().to_string(), None, format!("cannot read: {e}")));
                continue;
            }
        };
        let source = path.display().to_string();
        let part = if i == 0 { parse_settings(&text, &source) } else { parse_keymap(&text, &source, os) };
        out.config.merge(&part.config);
        out.warnings.extend(part.warnings);
        out.report.applied += part.report.applied;
        out.report.unsupported += part.report.unsupported;
        for (l, s) in part.report.samples {
            if out.report.samples.len() < 5 {
                out.report.samples.push((l, format!("{name}: {s}")));
            }
        }
        out.report.paths.push(path);
    }
    out
}

/// Removes `//` and `/* */` comments and trailing commas, which Sublime's JSON allows. Line breaks inside block
/// comments are kept so serde_json's line numbers still match the file.
pub fn strip_json(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(b.len());
                out.extend_from_slice(&b[start..i]);
            }
            b'/' if b.get(i + 1) == Some(&b'/') => i = skip_line(b, i),
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = skip_block(b, i);
                out.extend(b[i..end].iter().filter(|&&c| c == b'\n'));
                i = end;
            }
            b',' => {
                if !matches!(next_significant(b, i + 1), Some(b'}' | b']')) {
                    out.push(b',');
                }
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    // only whole comments and ASCII commas were dropped, so this is still UTF-8
    String::from_utf8(out).unwrap_or_default()
}

fn skip_line(b: &[u8], i: usize) -> usize {
    b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |p| i + p)
}

fn skip_block(b: &[u8], i: usize) -> usize {
    b[i + 2..].windows(2).position(|w| w == b"*/").map_or(b.len(), |p| i + 2 + p + 2)
}

/// The next byte after blanks and comments.
fn next_significant(b: &[u8], mut i: usize) -> Option<u8> {
    while i < b.len() {
        match b[i] {
            c if c.is_ascii_whitespace() => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => i = skip_line(b, i),
            b'/' if b.get(i + 1) == Some(&b'*') => i = skip_block(b, i),
            c => return Some(c),
        }
    }
    None
}

fn parse_json(text: &str, source: &str, warnings: &mut Vec<Warning>) -> Option<Value> {
    match serde_json::from_str(&strip_json(text)) {
        Ok(v) => Some(v),
        Err(e) => {
            warnings.push(Warning::new(source, Some(e.line()), format!("not valid JSON, skipped: {e}")));
            None
        }
    }
}

/// `Preferences.sublime-settings`.
pub fn parse_settings(text: &str, source: &str) -> Imported {
    let mut out = Imported { report: ImportReport::new("sublime"), ..Default::default() };
    let Some(v) = parse_json(text, source, &mut out.warnings) else { return out };
    let Some(obj) = v.as_object() else {
        out.warnings.push(Warning::new(source, None, "expected a JSON object"));
        return out;
    };
    let c = &mut out.config;
    let r = &mut out.report;
    for (k, v) in obj {
        let mut feature = |f: Feature| match v.as_bool() {
            Some(b) => {
                c.set_feature(f, b);
                true
            }
            None => false,
        };
        let ok = match k.as_str() {
            "tab_size" => match v.as_u64() {
                Some(n @ 1..=16) => {
                    c.editor.tab_width = Some(n as usize);
                    true
                }
                _ => false,
            },
            "translate_tabs_to_spaces" => v.as_bool().map(|b| c.editor.expand_tab = Some(b)).is_some(),
            "font_face" => v
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(|s| c.editor.font_family = Some(s.to_string()))
                .is_some(),
            "font_size" => {
                v.as_f64().filter(|n| (4.0..=200.0).contains(n)).map(|n| c.editor.font_size = Some(n as f32)).is_some()
            }
            "line_numbers" => feature(Feature::LineNumbers),
            "highlight_line" => feature(Feature::CurrentLine),
            "auto_match_enabled" => feature(Feature::AutoPairs),
            "auto_indent" => feature(Feature::AutoIndent),
            "match_brackets" => feature(Feature::BracketMatch),
            "draw_indent_guides" => feature(Feature::IndentGuides),
            "draw_white_space" => {
                // "all" / "selection" / "none", or (Sublime 4) a list like ["selection", "trailing"]
                let has = |w: &str| {
                    v.as_str() == Some(w) || v.as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(w)))
                };
                c.set_feature(Feature::Whitespace, has("all") || has("trailing"));
                true
            }
            "ignored_packages" => match v.as_array() {
                Some(a) => {
                    // Sublime ignores Vintage by default: a user list without it means vim mode on
                    if !a.iter().any(|x| x.as_str() == Some("Vintage")) {
                        c.set_feature(Feature::Vim, true);
                    }
                    true
                }
                None => false,
            },
            "color_scheme" | "theme" => {
                let s = v.as_str().unwrap_or("").to_ascii_lowercase();
                let light = ["light", "breakers", "celeste", "sixteen"].iter().any(|w| s.contains(w));
                if let Some(id) = (k == "color_scheme").then(|| known_scheme(&s)).flatten() {
                    c.editor.theme = Some(id.into());
                } else if light {
                    c.editor.theme = Some("shrew-light".into());
                } else if k == "color_scheme" && !s.is_empty() {
                    c.editor.theme = Some("shrew-dark".into());
                }
                true
            }
            // meaningless for shrew, nothing to report
            "scroll_past_end"
            | "word_wrap"
            | "rulers"
            | "font_options"
            | "theme_font_size"
            | "vintage_start_in_command_mode"
            | "hot_exit"
            | "remember_open_files"
            | "update_check"
            | "show_encoding"
            | "show_line_endings" => true,
            _ => false,
        };
        if ok {
            r.applied += 1;
        } else {
            r.skip(None, format!("\"{k}\": {v}"));
        }
    }
    out
}

/// The built-in shrew theme for a well-known color scheme name (`Packages/Dracula/Dracula.sublime-color-scheme`).
pub fn known_scheme(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase().replace(['_', '-'], " ");
    let has = |w: &str| n.contains(w);
    let pick = |cond: bool, a: &'static str, b: &'static str| if cond { a } else { b };
    Some(if has("solarized") {
        pick(has("light"), "solarized-light", "solarized")
    } else if has("gruvbox") {
        pick(has("light"), "gruvbox-light", "gruvbox-dark")
    } else if has("catppuccin") {
        if has("latte") { "catppuccin-latte" } else { pick(has("frapp"), "catppuccin-frappe", "catppuccin-mocha") }
    } else if has("ayu") {
        if has("light") { "ayu-light" } else { pick(has("mirage"), "ayu-mirage", "ayu-dark") }
    } else if has("one dark") || has("onedark") {
        "one-dark"
    } else if has("one light") || has("onelight") {
        "one-light"
    } else if has("github") {
        pick(has("light"), "github-light", "github-dark")
    } else if has("rose pine") || has("ros\u{e9} pine") || has("rosepine") {
        pick(has("dawn"), "rose-pine-dawn", "rose-pine")
    } else if has("nord") {
        pick(has("light"), "nord-light", "nord")
    } else {
        [
            ("tokyo", "tokyo-night"),
            ("dracula", "dracula"),
            ("monokai", "monokai"),
            ("everforest", "everforest"),
            ("kanagawa", "kanagawa"),
            ("synthwave", "synthwave-84"),
            ("oceanic", "oceanic"),
        ]
        .iter()
        .find(|(w, _)| has(w))?
        .1
    })
}

/// A `.sublime-keymap`.
pub fn parse_keymap(text: &str, source: &str, os: Os) -> Imported {
    let mut out = Imported { report: ImportReport::new("sublime"), ..Default::default() };
    let Some(v) = parse_json(text, source, &mut out.warnings) else { return out };
    let Some(list) = v.as_array() else {
        out.warnings.push(Warning::new(source, None, "expected a JSON array of key bindings"));
        return out;
    };
    let mut unknown: Vec<String> = Vec::new();
    let mut with_context = 0;
    for b in list {
        let command = b.get("command").and_then(Value::as_str).unwrap_or("");
        let keys: Vec<&str> = b
            .get("keys")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let desc = format!("{} -> {command}", keys.join(", "));
        if b.get("context").is_some() {
            with_context += 1;
            out.report.skip(None, format!("{desc} (has a context)"));
            continue;
        }
        let Some(lhs) = keys.iter().map(|k| parse_key(k, os)).collect::<Option<Vec<_>>>().filter(|l| !l.is_empty())
        else {
            out.report.skip(None, format!("{desc} (keys)"));
            continue;
        };
        let args = b.get("args").cloned().unwrap_or(Value::Null);
        let Some((rhs, insert_only)) = command_rhs(command, &args) else {
            if !unknown.iter().any(|u| u == command) {
                unknown.push(command.to_string());
            }
            out.report.skip(None, desc);
            continue;
        };
        let mut modes = vec![MapMode::Insert];
        if !insert_only && vim_safe(&lhs[0]) {
            modes.extend([MapMode::Normal, MapMode::Visual]);
        }
        for m in modes {
            out.config.bindings.push((m, Mapping { lhs: lhs.clone(), rhs: rhs.clone() }));
        }
        out.report.applied += 1;
    }
    for u in unknown {
        out.warnings.push(Warning::new(
            source,
            None,
            format!("Sublime command {u:?} has no shrew equivalent, binding skipped"),
        ));
    }
    if with_context > 0 {
        out.warnings.push(Warning::new(
            source,
            None,
            format!("{with_context} binding(s) with a \"context\" skipped (conditional in Sublime)"),
        ));
    }
    out
}

/// Ctrl keys vim's normal mode uses (scrolling, redo, increment, window commands, jumps, visual block).
const VIM_CTRL: &str = "rdufbeyvqaxcwoi[";

/// May this chord also be bound in normal/visual mode without hiding a vim command?
fn vim_safe(k: &KeyPress) -> bool {
    let m = k.mods;
    match k.key {
        Key::F(_) => true,
        _ if m.alt || m.logo => true,
        Key::Char(_) if m.ctrl && m.shift => true,
        Key::Char(c) if m.ctrl => !VIM_CTRL.contains(c),
        _ => false,
    }
}

/// `ctrl+shift+p`, `alt+up`, `primary+k`, `f12`, `ctrl++`, `super+keypad_enter`.
pub fn parse_key(s: &str, os: Os) -> Option<KeyPress> {
    let s = s.trim();
    let (mods_part, key) = if s == "+" {
        ("", "+")
    } else if let Some(m) = s.strip_suffix("++") {
        (m, "+")
    } else {
        match s.rsplit_once('+') {
            Some((m, k)) => (m, k),
            None => ("", s),
        }
    };
    let mut mods = Mods::default();
    for m in mods_part.split('+').filter(|m| !m.is_empty()) {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" | "option" => mods.alt = true,
            "shift" => mods.shift = true,
            "super" | "command" | "cmd" => mods.logo = true,
            "primary" if os == Os::MacOs => mods.logo = true,
            "primary" => mods.ctrl = true,
            _ => return None,
        }
    }
    let key = match key.to_ascii_lowercase().as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "page_up" => Key::PageUp,
        "pagedown" | "page_down" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        "enter" | "keypad_enter" => Key::Enter,
        "escape" => Key::Esc,
        "space" => Key::Char(' '),
        "plus" | "keypad_plus" => Key::Char('+'),
        "minus" | "keypad_minus" => Key::Char('-'),
        "equals" => Key::Char('='),
        "forward_slash" | "keypad_divide" => Key::Char('/'),
        "backquote" => Key::Char('`'),
        "keypad_multiply" => Key::Char('*'),
        "keypad_period" => Key::Char('.'),
        k if k.len() > 1 && k.starts_with('f') && k[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) => {
            Key::F(k[1..].parse().ok()?)
        }
        k if k.starts_with("keypad") && k.len() == 7 && k.as_bytes()[6].is_ascii_digit() => {
            Key::Char(k.as_bytes()[6] as char)
        }
        _ => {
            let mut chars = key.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Key::Char(c)
        }
    };
    // match what the app sends: shift is applied to plain chars ('A'), chords keep lowercase + shift
    if let Key::Char(c) = key {
        if mods.ctrl || mods.alt || mods.logo {
            if c.is_ascii_uppercase() {
                mods.shift = true;
            }
            return Some(KeyPress::new(Key::Char(c.to_ascii_lowercase()), mods));
        }
        if mods.shift && c.is_ascii_alphabetic() {
            return Some(KeyPress::ch(c.to_ascii_uppercase()));
        }
    }
    Some(KeyPress::new(key, mods))
}

/// A Sublime command and its args as a shrew rhs, and whether it only makes sense in insert mode (it types or
/// moves like a text field).
pub fn command_rhs(command: &str, args: &Value) -> Option<(Rhs, bool)> {
    use Command::*;
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
    let flag = |k: &str| args.get(k).and_then(Value::as_bool);
    let cmd = |c: Command| Some((Rhs::Command(c), false));
    let keys = |s: &str| Some((Rhs::Keys { keys: KeyPress::parse_seq(s), remap: false }, true));
    match command {
        "save" => cmd(Save),
        "save_all" => cmd(SaveAll),
        "close" | "close_file" => cmd(CloseBuffer),
        "close_window" | "exit" => cmd(Quit),
        "new_file" => cmd(NewFile),
        "prompt_open_file" => cmd(OpenFile),
        "revert" => cmd(Reload),
        "undo" | "soft_undo" => cmd(Undo),
        "redo" | "redo_or_repeat" | "soft_redo" => cmd(Redo),
        "copy" => cmd(Copy),
        "cut" => cmd(Cut),
        "paste" | "paste_and_indent" => cmd(Paste),
        "select_all" => cmd(SelectAll),
        "expand_selection" => match arg("to") {
            "line" => cmd(SelectLine),
            "brackets" => cmd(SelectInsideBrackets),
            _ => None,
        },
        "run_macro_file" => {
            let file = arg("file");
            if file.ends_with("Delete Line.sublime-macro") {
                cmd(DeleteLine)
            } else if file.ends_with("Add Line Before.sublime-macro") {
                cmd(InsertLineAbove)
            } else if file.ends_with("Add Line.sublime-macro") {
                cmd(InsertLineBelow)
            } else {
                None
            }
        }
        "duplicate_line" => cmd(DuplicateLine),
        "swap_line_up" => cmd(MoveLineUp),
        "swap_line_down" => cmd(MoveLineDown),
        "toggle_comment" => cmd(ToggleComment),
        "indent" => cmd(Indent),
        "unindent" => cmd(Outdent),
        "join_lines" => cmd(JoinLines),
        "find_under_expand" => cmd(AddNextOccurrence),
        "find_under_expand_skip" => cmd(SkipOccurrence),
        "find_all_under" => cmd(SelectAllOccurrences),
        "split_selection_into_lines" => cmd(SplitSelectionIntoLines),
        "select_lines" => cmd(if flag("forward") == Some(false) { AddCursorAbove } else { AddCursorBelow }),
        "single_selection" => cmd(CollapseCursors),
        "move_to" => {
            let extend = if flag("extend") == Some(true) { "S-" } else { "" };
            match arg("to") {
                "brackets" => cmd(GotoMatchingBracket),
                "bol" | "hardbol" => keys(&format!("<{extend}Home>")),
                "eol" | "hardeol" => keys(&format!("<{extend}End>")),
                "bof" => keys(&format!("<C-{extend}Home>")),
                "eof" => keys(&format!("<C-{extend}End>")),
                _ => None,
            }
        }
        "move" => {
            let extend = if flag("extend") == Some(true) { "S-" } else { "" };
            let fwd = flag("forward") != Some(false);
            let key = match (arg("by"), fwd) {
                ("characters", true) => format!("<{extend}Right>"),
                ("characters", false) => format!("<{extend}Left>"),
                ("lines", true) => format!("<{extend}Down>"),
                ("lines", false) => format!("<{extend}Up>"),
                ("words" | "word_ends" | "subwords" | "subword_ends", true) => format!("<C-{extend}Right>"),
                ("words" | "word_ends" | "subwords" | "subword_ends", false) => format!("<C-{extend}Left>"),
                ("pages", true) => format!("<{extend}PageDown>"),
                ("pages", false) => format!("<{extend}PageUp>"),
                _ => return None,
            };
            keys(&key)
        }
        "left_delete" => keys("<BS>"),
        "right_delete" => keys("<Del>"),
        "delete_word" => keys(if flag("forward") == Some(false) { "<C-BS>" } else { "<C-Del>" }),
        "insert" => {
            let s = arg("characters");
            (!s.is_empty()).then(|| (Rhs::Keys { keys: s.chars().map(KeyPress::ch).collect(), remap: false }, true))
        }
        "show_overlay" => match (arg("overlay"), arg("text"), flag("show_files")) {
            ("command_palette", _, _) => cmd(CommandPalette),
            ("goto", ":", _) => cmd(GotoLine),
            ("goto", "", Some(true)) => cmd(OpenFile),
            _ => None,
        },
        "show_panel" => match arg("panel") {
            "find" | "incremental_find" => cmd(Find),
            _ => None,
        },
        "find_next" => cmd(FindNext),
        "find_prev" => cmd(FindPrev),
        "next_view" | "next_view_in_stack" => cmd(NextBuffer),
        "prev_view" | "prev_view_in_stack" => cmd(PrevBuffer),
        "increase_font_size" => cmd(FontBigger),
        "decrease_font_size" => cmd(FontSmaller),
        "reset_font_size" => cmd(FontReset),
        "toggle_setting" => match arg("setting") {
            "line_numbers" => cmd(ToggleLineNumbers),
            "draw_white_space" => cmd(ToggleWhitespace),
            _ => None,
        },
        "noop" => Some((Rhs::Keys { keys: Vec::new(), remap: false }, false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn kp(s: &str) -> KeyPress {
        parse_key(s, Os::Linux).unwrap()
    }

    fn find<'a>(c: &'a Config, mode: MapMode, lhs: &[KeyPress]) -> Option<&'a Rhs> {
        c.bindings.iter().rev().find(|(m, b)| *m == mode && b.lhs == lhs).map(|(_, b)| &b.rhs)
    }

    #[test]
    fn strips_comments_and_trailing_commas() {
        let src = "{\n  // c\n  \"a\": \"x // not a comment\", /* b */\n  \"b\": [1, 2,],\n}\n";
        let v: Value = serde_json::from_str(&strip_json(src)).unwrap();
        assert_eq!(v["a"], "x // not a comment");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
        // strings with escaped quotes and commas before comments
        let v: Value = serde_json::from_str(&strip_json(
            r#"["a\"b", "c", // x
        ]"#,
        ))
        .unwrap();
        assert_eq!(v, serde_json::json!(["a\"b", "c"]));
    }

    #[test]
    fn keys() {
        let ctrl_shift_p = KeyPress::new(Key::Char('p'), Mods { ctrl: true, shift: true, ..Mods::default() });
        assert_eq!(kp("ctrl+shift+p"), ctrl_shift_p);
        assert_eq!(kp("primary+s"), KeyPress::ctrl('s'));
        assert_eq!(parse_key("primary+s", Os::MacOs).unwrap().mods.logo, true);
        assert_eq!(kp("ctrl++"), KeyPress::ctrl('+'));
        assert_eq!(kp("ctrl+plus"), KeyPress::ctrl('+'));
        assert_eq!(kp("f12"), KeyPress::plain(Key::F(12)));
        assert_eq!(kp("shift+f3"), KeyPress::new(Key::F(3), Mods { shift: true, ..Mods::default() }));
        assert_eq!(kp("alt+up"), KeyPress::new(Key::Up, Mods { alt: true, ..Mods::default() }));
        assert_eq!(kp("shift+a"), KeyPress::ch('A'));
        assert_eq!(kp("keypad_enter"), KeyPress::plain(Key::Enter));
        assert!(parse_key("ctrl+hyper+x", Os::Linux).is_none());
        assert!(parse_key("<character>", Os::Linux).is_none());
    }

    #[test]
    fn known_schemes() {
        assert_eq!(known_scheme("Packages/Dracula Color Scheme/Dracula.sublime-color-scheme"), Some("dracula"));
        assert_eq!(known_scheme("Solarized (light).sublime-color-scheme"), Some("solarized-light"));
        assert_eq!(known_scheme("Catppuccin Frappe.sublime-color-scheme"), Some("catppuccin-frappe"));
        assert_eq!(known_scheme("Mariana.sublime-color-scheme"), None);
    }

    #[test]
    fn realistic_settings() {
        let imp = parse_settings(include_str!("../tests/fixtures/sublime/Preferences.sublime-settings"), "prefs");
        assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
        let c = &imp.config;
        assert_eq!(c.editor.tab_width, Some(2));
        assert_eq!(c.editor.expand_tab, Some(true));
        assert_eq!(c.editor.font_family.as_deref(), Some("Fira Code"));
        assert_eq!(c.editor.font_size, Some(12.5));
        assert_eq!(c.editor.theme.as_deref(), Some("shrew-light"));
        assert_eq!(c.feature(Feature::LineNumbers), Some(true));
        assert_eq!(c.feature(Feature::CurrentLine), Some(true));
        assert_eq!(c.feature(Feature::Whitespace), Some(true));
        assert_eq!(c.feature(Feature::AutoPairs), Some(false));
        assert_eq!(c.feature(Feature::AutoIndent), Some(true));
        assert_eq!(c.feature(Feature::Vim), Some(true), "Vintage enabled");
        assert_eq!(imp.report.unsupported, 2, "{:?}", imp.report.samples);
    }

    #[test]
    fn vintage_off_by_default() {
        let c = parse_settings(r#"{"ignored_packages": ["Vintage", "Markdown"]}"#, "p").config;
        assert_eq!(c.feature(Feature::Vim), None);
    }

    #[test]
    fn realistic_keymap() {
        let imp =
            parse_keymap(include_str!("../tests/fixtures/sublime/Default (Linux).sublime-keymap"), "keymap", Os::Linux);
        let c = &imp.config;
        let cmd = |c: Command| Some(Rhs::Command(c));
        let ctrl_shift = |ch: char| KeyPress::new(Key::Char(ch), Mods { ctrl: true, shift: true, ..Mods::default() });
        assert_eq!(find(c, MapMode::Insert, &[ctrl_shift('p')]).cloned(), cmd(Command::CommandPalette));
        assert_eq!(find(c, MapMode::Normal, &[ctrl_shift('p')]).cloned(), cmd(Command::CommandPalette));
        assert_eq!(find(c, MapMode::Insert, &[KeyPress::ctrl('t')]).cloned(), cmd(Command::OpenFile));
        assert_eq!(find(c, MapMode::Normal, &[KeyPress::ctrl('t')]).cloned(), cmd(Command::OpenFile));
        assert_eq!(find(c, MapMode::Insert, &[ctrl_shift('k')]).cloned(), cmd(Command::DeleteLine));
        // ctrl+d is vim's half-page scroll: Sublime's meaning only in insert mode
        assert_eq!(find(c, MapMode::Insert, &[KeyPress::ctrl('d')]).cloned(), cmd(Command::AddNextOccurrence));
        assert_eq!(find(c, MapMode::Normal, &[KeyPress::ctrl('d')]), None);
        // a chord of two keys
        assert_eq!(
            find(c, MapMode::Insert, &[KeyPress::ctrl('k'), KeyPress::ctrl('k')]).cloned(),
            cmd(Command::DeleteLine)
        );
        assert_eq!(find(c, MapMode::Insert, &[KeyPress::ctrl('g')]).cloned(), cmd(Command::GotoLine));
        assert_eq!(
            find(c, MapMode::Insert, &[KeyPress::ctrl('e')]),
            Some(&Rhs::Keys { keys: KeyPress::parse_seq("<End>"), remap: false })
        );
        assert_eq!(find(c, MapMode::Normal, &[KeyPress::ctrl('e')]), None, "text-field moves are insert-only");
        let alt_up = KeyPress::new(Key::Up, Mods { alt: true, ..Mods::default() });
        assert_eq!(find(c, MapMode::Visual, &[alt_up]).cloned(), cmd(Command::MoveLineUp));
        assert_eq!(find(c, MapMode::Insert, &[KeyPress::ctrl('q')]), Some(&Rhs::Keys { keys: vec![], remap: false }));
        assert_eq!(find(c, MapMode::Insert, &[KeyPress::plain(Key::F(9))]).cloned(), cmd(Command::FontReset));
        let unknown: Vec<&str> = imp.warnings.iter().map(|w| w.message.as_str()).collect();
        assert!(unknown.iter().any(|m| m.contains("\"sort_lines\"")), "{unknown:?}");
        assert!(unknown.iter().any(|m| m.contains("context")), "{unknown:?}");
        assert_eq!(imp.report.applied, 17);
    }

    #[test]
    fn finds_user_dir() {
        let root = std::env::temp_dir().join(format!("shrew-sublime-{}", std::process::id()));
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sublime");
        let user = root.join("cfg/sublime-text/Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        for f in ["Preferences.sublime-settings", "Default (Linux).sublime-keymap"] {
            std::fs::copy(fixtures.join(f), user.join(f)).unwrap();
        }
        let env =
            Env { home: Some(root.join("home")), config_dir: Some(root.join("cfg")), appdata: None, os: Os::Linux };
        let imp = import(&ImportSource::Auto, &env).unwrap();
        assert_eq!(imp.report.paths.len(), 2);
        assert_eq!(imp.config.editor.tab_width, Some(2));
        assert!(!imp.config.bindings.is_empty());
        let mac = Env { home: Some(root.join("home")), config_dir: None, appdata: None, os: Os::MacOs };
        assert!(import(&ImportSource::Auto, &mac).is_none());
        assert_eq!(candidates(&mac)[0], root.join("home/Library/Application Support/Sublime Text/Packages/User"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
