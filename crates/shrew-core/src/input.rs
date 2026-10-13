//! Toolkit-independent key events. The app converts winit events into these; tests build them from strings.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A printable character. Shift is already applied ('D', not shift+'d') unless ctrl/alt is held, in which case
    /// the char is lowercase and `shift` is set.
    Char(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyPress {
    pub key: Key,
    pub mods: Mods,
}

impl KeyPress {
    pub fn new(key: Key, mods: Mods) -> Self {
        KeyPress { key, mods }
    }
    pub fn plain(key: Key) -> Self {
        KeyPress { key, mods: Mods::default() }
    }
    pub fn ch(c: char) -> Self {
        KeyPress::plain(Key::Char(c))
    }
    pub fn ctrl(c: char) -> Self {
        KeyPress { key: Key::Char(c), mods: Mods { ctrl: true, ..Mods::default() } }
    }

    /// The typed character when no command modifier is held.
    #[inline]
    pub fn text(&self) -> Option<char> {
        match self.key {
            Key::Char(c) if !self.mods.ctrl && !self.mods.alt && !self.mods.logo => Some(c),
            _ => None,
        }
    }

    /// Parses vim-style notation: `x`, `<Esc>`, `<C-d>`, `<C-S-p>`, `<A-Up>`, `<lt>`.
    pub fn parse(s: &str) -> Option<KeyPress> {
        if s.chars().count() == 1 {
            return Some(KeyPress::ch(s.chars().next()?));
        }
        let inner = s.strip_prefix('<')?.strip_suffix('>')?;
        let mut mods = Mods::default();
        let mut parts: Vec<&str> = inner.split('-').collect();
        // "<C-->" means ctrl + '-'
        if inner.ends_with("--") {
            parts.pop();
            parts.pop();
            parts.push("-");
        }
        let name = parts.pop()?;
        for m in parts {
            match m.to_ascii_lowercase().as_str() {
                "c" => mods.ctrl = true,
                "a" | "m" => mods.alt = true,
                "s" => mods.shift = true,
                "d" => mods.logo = true,
                _ => return None,
            }
        }
        let key = match name.to_ascii_lowercase().as_str() {
            "esc" => Key::Esc,
            "cr" | "enter" | "return" => Key::Enter,
            "bs" | "backspace" => Key::Backspace,
            "del" | "delete" => Key::Delete,
            "tab" => Key::Tab,
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "insert" => Key::Insert,
            "space" => Key::Char(' '),
            "lt" => Key::Char('<'),
            n if n.len() > 1 && n.starts_with('f') && n[1..].parse::<u8>().is_ok() => Key::F(n[1..].parse().ok()?),
            _ if name.chars().count() == 1 => {
                let c = name.chars().next()?;
                Key::Char(if mods.ctrl || mods.alt { c.to_ascii_lowercase() } else { c })
            }
            _ => return None,
        };
        Some(KeyPress { key, mods })
    }

    /// Parses a sequence like `d2w<Esc>:wq<CR>`.
    pub fn parse_seq(s: &str) -> Vec<KeyPress> {
        let mut out = Vec::new();
        let mut rest = s;
        while let Some(c) = rest.chars().next() {
            if c == '<' {
                if let Some(end) = rest.find('>') {
                    if let Some(k) = KeyPress::parse(&rest[..=end]) {
                        out.push(k);
                        rest = &rest[end + 1..];
                        continue;
                    }
                }
            }
            out.push(KeyPress::ch(c));
            rest = &rest[c.len_utf8()..];
        }
        out
    }
}

impl fmt::Display for KeyPress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.mods;
        let name = match self.key {
            Key::Char(' ') => "Space".to_string(),
            Key::Char(c) => c.to_string(),
            Key::F(n) => format!("F{n}"),
            k => format!("{k:?}"),
        };
        if !m.ctrl && !m.alt && !m.logo && !(m.shift && !matches!(self.key, Key::Char(_))) {
            return if matches!(self.key, Key::Char(c) if c != ' ') { write!(f, "{name}") } else { write!(f, "<{name}>") };
        }
        write!(f, "<")?;
        if m.ctrl {
            write!(f, "C-")?;
        }
        if m.alt {
            write!(f, "A-")?;
        }
        if m.shift {
            write!(f, "S-")?;
        }
        if m.logo {
            write!(f, "D-")?;
        }
        write!(f, "{name}>")
    }
}

/// Mouse buttons the editor reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}
