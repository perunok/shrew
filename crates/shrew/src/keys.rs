//! winit keyboard events -> shrew-core [`KeyPress`]es.
//!
//! The rules shrew-core expects: a printable key is `Key::Char` with shift already applied (`'D'`, not shift+`'d'`)
//! unless ctrl/alt/logo is held, in which case the char is the unshifted, lowercase key and `shift` is set. Named
//! keys keep every modifier. AltGr (reported as ctrl+alt on Windows) types text like any other plain key.

use shrew_core::{Key, KeyPress, Mods};
use winit::keyboard::{Key as WKey, KeyCode, ModifiersState, NamedKey, PhysicalKey};

/// The parts of a winit key event the conversion looks at (kept separate so it can be tested without winit).
pub struct RawKey<'a> {
    pub logical: &'a WKey,
    /// The key as the layout produces it without modifiers (`key_without_modifiers`).
    pub unmodified: &'a WKey,
    pub physical: PhysicalKey,
    /// Text the key types (after dead keys / compose); `None` for non-text keys.
    pub text: Option<&'a str>,
    pub mods: ModifiersState,
}

pub fn mods_of(m: ModifiersState) -> Mods {
    let mut mods = Mods { ctrl: m.control_key(), alt: m.alt_key(), shift: m.shift_key(), logo: m.super_key() };
    // Cmd is the command key on macOS: it acts as Ctrl for every chord
    if cfg!(target_os = "macos") && mods.logo {
        mods.ctrl = true;
        mods.logo = false;
    }
    mods
}

/// Converts one key press. Usually one KeyPress; several when a compose sequence types more than one char; none for
/// modifier-only and unknown keys.
pub fn convert(raw: &RawKey) -> Vec<KeyPress> {
    let mods = mods_of(raw.mods);
    let named = |key: Key| vec![KeyPress::new(key, mods)];
    match raw.logical {
        WKey::Named(n) => {
            let key = match n {
                NamedKey::Enter => Key::Enter,
                NamedKey::Escape => Key::Esc,
                NamedKey::Backspace => Key::Backspace,
                NamedKey::Delete => Key::Delete,
                NamedKey::Tab => Key::Tab,
                NamedKey::ArrowLeft => Key::Left,
                NamedKey::ArrowRight => Key::Right,
                NamedKey::ArrowUp => Key::Up,
                NamedKey::ArrowDown => Key::Down,
                NamedKey::Home => Key::Home,
                NamedKey::End => Key::End,
                NamedKey::PageUp => Key::PageUp,
                NamedKey::PageDown => Key::PageDown,
                NamedKey::Insert => Key::Insert,
                NamedKey::Space => return char_key(' ', ' ', mods, raw.text),
                n => match f_key(*n) {
                    Some(f) => Key::F(f),
                    None => return Vec::new(),
                },
            };
            named(key)
        }
        WKey::Character(s) => {
            let Some(c) = s.chars().next() else { return Vec::new() };
            let base = match raw.unmodified {
                WKey::Character(u) => u.chars().next().unwrap_or(c),
                _ => c,
            };
            // non-Latin layouts: Ctrl+С on a Cyrillic layout is still Ctrl+C
            let base =
                if !base.is_ascii() && (mods.ctrl || mods.alt) { latin_of(raw.physical).unwrap_or(base) } else { base };
            char_key(c, base, mods, raw.text)
        }
        _ => Vec::new(),
    }
}

fn char_key(c: char, base: char, mods: Mods, text: Option<&str>) -> Vec<KeyPress> {
    let chord = mods.ctrl || mods.alt || mods.logo;
    if chord {
        // AltGr on Windows arrives as ctrl+alt with the composed text: type it
        let altgr_text =
            mods.ctrl && mods.alt && !mods.logo && text.is_some_and(|t| printable(t) && !t.starts_with(base));
        if !altgr_text {
            let k = if base.is_alphabetic() { base.to_lowercase().next().unwrap_or(base) } else { base };
            return vec![KeyPress::new(Key::Char(k), mods)];
        }
    }
    let typed = text.filter(|t| printable(t));
    match typed {
        Some(t) => t.chars().map(KeyPress::ch).collect(),
        None if !c.is_control() => vec![KeyPress::ch(c)],
        None => Vec::new(),
    }
}

/// Text with no control chars (winit reports Ctrl+A as "\u{1}", Backspace as "\u{8}", ...).
fn printable(t: &str) -> bool {
    !t.is_empty() && !t.chars().any(char::is_control)
}

fn f_key(n: NamedKey) -> Option<u8> {
    use NamedKey::*;
    const KEYS: [NamedKey; 24] =
        [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24];
    KEYS.iter().position(|&k| k == n).map(|i| i as u8 + 1)
}

/// The US letter or digit on a physical key, for chords typed on non-Latin layouts.
fn latin_of(p: PhysicalKey) -> Option<char> {
    let PhysicalKey::Code(code) = p else { return None };
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR,
        KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    LETTERS.iter().position(|&k| k == code).map(|i| (b'a' + i as u8) as char)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(logical: WKey, unmodified: WKey, text: Option<&str>, mods: ModifiersState) -> Vec<KeyPress> {
        convert(&RawKey {
            logical: &logical,
            unmodified: &unmodified,
            physical: PhysicalKey::Code(KeyCode::KeyC),
            text,
            mods,
        })
    }
    fn ch(s: &str) -> WKey {
        WKey::Character(s.into())
    }

    #[test]
    fn plain_and_shifted_chars() {
        assert_eq!(key(ch("d"), ch("d"), Some("d"), ModifiersState::empty()), vec![KeyPress::ch('d')]);
        assert_eq!(key(ch("D"), ch("d"), Some("D"), ModifiersState::SHIFT), vec![KeyPress::ch('D')]);
        assert_eq!(key(ch("{"), ch("["), Some("{"), ModifiersState::SHIFT), vec![KeyPress::ch('{')]);
        // compose produced two chars
        assert_eq!(key(ch("´"), ch("´"), Some("´e"), ModifiersState::empty()).len(), 2);
        assert_eq!(
            key(WKey::Named(NamedKey::Space), WKey::Named(NamedKey::Space), Some(" "), ModifiersState::SHIFT),
            vec![KeyPress::ch(' ')]
        );
    }

    #[test]
    fn ctrl_chords_are_lowercase_with_shift() {
        let ctrl = ModifiersState::CONTROL;
        assert_eq!(key(ch("p"), ch("p"), Some("\u{10}"), ctrl), vec![KeyPress::ctrl('p')]);
        let k = key(ch("P"), ch("p"), None, ctrl | ModifiersState::SHIFT);
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].key, Key::Char('p'));
        assert!(k[0].mods.ctrl && k[0].mods.shift);
        // ctrl+'+' on a US layout is ctrl+shift+'='
        let k = key(ch("+"), ch("="), None, ctrl | ModifiersState::SHIFT);
        assert_eq!(k[0].key, Key::Char('='));
        // Cyrillic layout: the physical C key
        let k = key(ch("с"), ch("с"), None, ctrl);
        assert_eq!(k[0].key, Key::Char('c'));
    }

    #[test]
    fn altgr_types_text() {
        let k = key(ch("@"), ch("q"), Some("@"), ModifiersState::CONTROL | ModifiersState::ALT);
        assert_eq!(k, vec![KeyPress::ch('@')]);
        // a real ctrl+alt chord still is one
        let k = key(ch("j"), ch("j"), None, ModifiersState::CONTROL | ModifiersState::ALT);
        assert!(k[0].mods.ctrl && k[0].mods.alt);
    }

    #[test]
    fn named_keys_keep_modifiers_and_controls_are_dropped() {
        let k = key(WKey::Named(NamedKey::Tab), WKey::Named(NamedKey::Tab), Some("\t"), ModifiersState::SHIFT);
        assert_eq!(k, vec![KeyPress::new(Key::Tab, Mods { shift: true, ..Mods::default() })]);
        let k = key(WKey::Named(NamedKey::F12), WKey::Named(NamedKey::F12), None, ModifiersState::empty());
        assert_eq!(k, vec![KeyPress::plain(Key::F(12))]);
        assert!(
            key(WKey::Named(NamedKey::Shift), WKey::Named(NamedKey::Shift), None, ModifiersState::SHIFT).is_empty()
        );
        assert!(key(ch("\u{7f}"), ch("\u{7f}"), Some("\u{7f}"), ModifiersState::empty()).is_empty());
    }
}
