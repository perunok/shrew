//! What the program in a terminal receives: keys as xterm byte sequences, mouse events in the X10/SGR encodings,
//! and pasted text (bracketed when the program asks for it). Pure functions, so every sequence is tested here.

use shrew_core::{Key, KeyPress, Mods};

/// The terminal modes that change how keys and the mouse are encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    /// DECCKM: arrows, Home and End send `ESC O x` instead of `ESC [ x`.
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    /// SGR (1006) mouse encoding instead of X10's bytes.
    pub sgr_mouse: bool,
    /// UTF-8 (1005) coordinates in the X10 encoding.
    pub utf8_mouse: bool,
}

/// xterm's modifier parameter: 1 + shift + 2 alt + 4 ctrl.
fn modifier(m: Mods) -> u8 {
    1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl)
}

/// The bytes a key sends, or `None` for keys a terminal does not send (Super chords, unknown function keys).
pub fn encode_key(k: KeyPress, modes: Modes) -> Option<Vec<u8>> {
    let m = k.mods;
    if m.logo {
        return None;
    }
    let plain = !m.shift && !m.alt && !m.ctrl;
    let csi = |s: &str| Some(format!("\x1b[{s}").into_bytes());
    // `ESC [ 1 ; m x` with modifiers, else `ESC [ x` (or `ESC O x` in application cursor mode)
    let cursor = |x: char| {
        if !plain {
            csi(&format!("1;{}{x}", modifier(m)))
        } else if modes.app_cursor {
            Some(format!("\x1bO{x}").into_bytes())
        } else {
            csi(&x.to_string())
        }
    };
    // `ESC [ n ~` or `ESC [ n ; m ~`
    let tilde = |n: u8| if plain { csi(&format!("{n}~")) } else { csi(&format!("{n};{}~", modifier(m))) };
    let alt = |mut bytes: Vec<u8>| {
        if m.alt {
            bytes.insert(0, 0x1b);
        }
        Some(bytes)
    };
    match k.key {
        Key::Char(c) => {
            if m.ctrl {
                return alt(vec![control(c)?]);
            }
            let mut b = [0u8; 4];
            alt(c.encode_utf8(&mut b).as_bytes().to_vec())
        }
        Key::Enter => alt(vec![b'\r']),
        Key::Esc => alt(vec![0x1b]),
        Key::Tab if m.shift => csi("Z"),
        Key::Tab => alt(vec![b'\t']),
        Key::Backspace if m.ctrl => alt(vec![0x08]),
        Key::Backspace => alt(vec![0x7f]),
        Key::Up => cursor('A'),
        Key::Down => cursor('B'),
        Key::Right => cursor('C'),
        Key::Left => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        // F1-F4 are SS3 P..S, or CSI 1;m P..S with modifiers
        Key::F(n @ 1..=4) => {
            let x = (b'P' + n - 1) as char;
            if plain { Some(format!("\x1bO{x}").into_bytes()) } else { csi(&format!("1;{}{x}", modifier(m))) }
        }
        Key::F(n @ 5..=20) => {
            const CODES: [u8; 16] = [15, 17, 18, 19, 20, 21, 23, 24, 25, 26, 28, 29, 31, 32, 33, 34];
            tilde(CODES[n as usize - 5])
        }
        Key::F(_) => None,
    }
}

/// Ctrl+`c` as a C0 control byte (xterm's table, including the digit row: Ctrl+2 is NUL, Ctrl+3..7 are ESC..US,
/// Ctrl+8 is DEL). Chars without one are sent as they are.
fn control(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        'A'..='Z' => c as u8 - b'A' + 1,
        '@' | ' ' | '2' | '`' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' | '~' => 0x1e,
        '_' | '-' | '/' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        c if c.is_ascii() => c as u8,
        _ => return None,
    })
}

/// A mouse event for a program that asked for them (xterm's button numbers before modifiers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEvent {
    /// Left 0, middle 1, right 2.
    Press(u8),
    Release(u8),
    /// The pointer moved with a button held (`None`: no button, for any-motion tracking).
    Motion(Option<u8>),
    WheelUp,
    WheelDown,
}

/// The report for `ev` at 0-based cell (`col`, `row`). X10 encoding cannot say where a press past column 222 is
/// (`None` then), nor which button was released (it sends 3).
pub fn encode_mouse(ev: MouseEvent, col: usize, row: usize, mods: Mods, modes: Modes) -> Option<Vec<u8>> {
    let (mut b, release) = match ev {
        MouseEvent::Press(b) => (b, false),
        MouseEvent::Release(b) => (if modes.sgr_mouse { b } else { 3 }, true),
        MouseEvent::Motion(b) => (32 + b.unwrap_or(3), false),
        MouseEvent::WheelUp => (64, false),
        MouseEvent::WheelDown => (65, false),
    };
    b += 4 * u8::from(mods.shift) + 8 * u8::from(mods.alt) + 16 * u8::from(mods.ctrl);
    let (x, y) = (col + 1, row + 1);
    if modes.sgr_mouse {
        return Some(format!("\x1b[<{b};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes());
    }
    let mut out = b"\x1b[M".to_vec();
    out.push(32 + b);
    for v in [x, y] {
        if modes.utf8_mouse {
            let c = char::from_u32(32 + v as u32).filter(|_| v <= 2015)?;
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        } else {
            out.push(u8::try_from(32 + v).ok()?);
        }
    }
    Some(out)
}

/// Pasted text as the program should get it: inside `ESC [200~ ... ESC [201~` when it asked for bracketed paste
/// (with escapes removed, so the text cannot end the bracket early), else with line breaks as Enter.
pub fn paste(text: &str, modes: Modes) -> Vec<u8> {
    if modes.bracketed_paste {
        let mut out = b"\x1b[200~".to_vec();
        out.extend(text.bytes().filter(|&b| b != 0x1b));
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Vec<u8> {
        encode_key(KeyPress::parse(s).unwrap(), Modes::default()).unwrap()
    }

    #[test]
    fn text_and_control_keys() {
        assert_eq!(key("a"), b"a");
        assert_eq!(key("é"), "é".as_bytes());
        assert_eq!(key("<C-c>"), [3]);
        assert_eq!(key("<C-a>"), [1]);
        assert_eq!(key("<C-S-a>"), [1], "shift adds nothing to a control letter");
        assert_eq!(key("<C-[>"), [0x1b]);
        assert_eq!(key("<C-\\>"), [0x1c]);
        assert_eq!(key("<C-Space>"), [0]);
        assert_eq!(key("<C-2>"), [0]);
        assert_eq!(key("<C-/>"), [0x1f]);
        assert_eq!(key("<A-x>"), b"\x1bx");
        assert_eq!(key("<A-C-x>"), [0x1b, 0x18]);
        assert_eq!(key("<CR>"), b"\r");
        assert_eq!(key("<A-CR>"), b"\x1b\r");
        assert_eq!(key("<BS>"), [0x7f]);
        assert_eq!(key("<C-BS>"), [0x08]);
        assert_eq!(key("<A-BS>"), b"\x1b\x7f");
        assert_eq!(key("<Esc>"), [0x1b]);
        assert_eq!(key("<Tab>"), b"\t");
        assert_eq!(key("<S-Tab>"), b"\x1b[Z");
        assert_eq!(encode_key(KeyPress::parse("<D-a>").unwrap(), Modes::default()), None);
    }

    #[test]
    fn cursor_keys_follow_the_mode_and_modifiers() {
        assert_eq!(key("<Up>"), b"\x1b[A");
        assert_eq!(key("<Left>"), b"\x1b[D");
        assert_eq!(key("<Home>"), b"\x1b[H");
        assert_eq!(key("<End>"), b"\x1b[F");
        let app = Modes { app_cursor: true, ..Modes::default() };
        assert_eq!(encode_key(KeyPress::parse("<Down>").unwrap(), app).unwrap(), b"\x1bOB");
        assert_eq!(encode_key(KeyPress::parse("<End>").unwrap(), app).unwrap(), b"\x1bOF");
        // modifiers win over application mode: CSI 1 ; m x
        assert_eq!(key("<S-Up>"), b"\x1b[1;2A");
        assert_eq!(key("<A-Right>"), b"\x1b[1;3C");
        assert_eq!(key("<C-Left>"), b"\x1b[1;5D");
        assert_eq!(encode_key(KeyPress::parse("<C-S-Right>").unwrap(), app).unwrap(), b"\x1b[1;6C");
        assert_eq!(key("<C-A-S-Down>"), b"\x1b[1;8B");
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(key("<Insert>"), b"\x1b[2~");
        assert_eq!(key("<Del>"), b"\x1b[3~");
        assert_eq!(key("<PageUp>"), b"\x1b[5~");
        assert_eq!(key("<C-PageDown>"), b"\x1b[6;5~");
        assert_eq!(key("<F1>"), b"\x1bOP");
        assert_eq!(key("<F4>"), b"\x1bOS");
        assert_eq!(key("<S-F1>"), b"\x1b[1;2P");
        assert_eq!(key("<F5>"), b"\x1b[15~");
        assert_eq!(key("<F10>"), b"\x1b[21~");
        assert_eq!(key("<F12>"), b"\x1b[24~");
        assert_eq!(key("<C-F12>"), b"\x1b[24;5~");
        assert_eq!(encode_key(KeyPress::plain(Key::F(30)), Modes::default()), None);
    }

    #[test]
    fn mouse_reports() {
        let sgr = Modes { sgr_mouse: true, ..Modes::default() };
        let none = Mods::default();
        assert_eq!(encode_mouse(MouseEvent::Press(0), 4, 9, none, sgr).unwrap(), b"\x1b[<0;5;10M");
        assert_eq!(encode_mouse(MouseEvent::Release(0), 4, 9, none, sgr).unwrap(), b"\x1b[<0;5;10m");
        assert_eq!(encode_mouse(MouseEvent::WheelDown, 0, 0, none, sgr).unwrap(), b"\x1b[<65;1;1M");
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        assert_eq!(encode_mouse(MouseEvent::Motion(Some(0)), 1, 1, ctrl, sgr).unwrap(), b"\x1b[<48;2;2M");
        // X10: 32 + button, 32 + 1-based column and row; releases are button 3
        let x10 = Modes::default();
        assert_eq!(encode_mouse(MouseEvent::Press(2), 0, 0, none, x10).unwrap(), b"\x1b[M\x22\x21\x21");
        assert_eq!(encode_mouse(MouseEvent::Release(2), 0, 0, none, x10).unwrap(), b"\x1b[M\x23\x21\x21");
        assert_eq!(encode_mouse(MouseEvent::Press(0), 300, 0, none, x10), None, "past column 223");
        let utf8 = Modes { utf8_mouse: true, ..Modes::default() };
        assert_eq!(encode_mouse(MouseEvent::Press(0), 300, 0, none, utf8).unwrap(), "\x1b[M ō!".as_bytes());
    }

    #[test]
    fn pasting() {
        let br = Modes { bracketed_paste: true, ..Modes::default() };
        assert_eq!(paste("a\nb", br), b"\x1b[200~a\nb\x1b[201~");
        assert_eq!(paste("x\x1b[201~rm", br), b"\x1b[200~x[201~rm\x1b[201~", "no early end of the bracket");
        assert_eq!(paste("a\r\nb\nc", Modes::default()), b"a\rb\rc");
    }
}
