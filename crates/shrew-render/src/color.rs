//! sRGB colors as the app and themes write them: 8 bits per channel, straight (not premultiplied) alpha.

use std::fmt;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { r, g, b, a }
    }

    /// `0xRRGGBB`.
    pub const fn from_u32(rgb: u32) -> Self {
        Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }

    /// Parses `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` (the `#` is optional).
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('#').unwrap_or(s);
        if !s.is_ascii() {
            return None;
        }
        let nib = |i: usize| u8::from_str_radix(&s[i..i + 1], 16).ok().map(|v| v * 17);
        let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        match s.len() {
            3 => Some(Color::rgb(nib(0)?, nib(1)?, nib(2)?)),
            4 => Some(Color::rgba(nib(0)?, nib(1)?, nib(2)?, nib(3)?)),
            6 => Some(Color::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Color::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Color { a, ..self }
    }

    pub const fn is_transparent(self) -> bool {
        self.a == 0
    }

    /// The color as `wgpu` wants it for a clear: linear when the target encodes sRGB itself, raw otherwise.
    pub(crate) fn to_wgpu(self, linear: bool) -> wgpu::Color {
        let f = |c: u8| {
            let c = c as f64 / 255.0;
            if linear { srgb_to_linear(c) } else { c }
        };
        wgpu::Color { r: f(self.r), g: f(self.g), b: f(self.b), a: self.a as f64 / 255.0 }
    }
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

impl std::str::FromStr for Color {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Color::from_hex(s).ok_or_else(|| format!("invalid color `{s}` (want #rrggbb or #rrggbbaa)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms() {
        assert_eq!(Color::from_hex("#ff8000"), Some(Color::rgb(255, 128, 0)));
        assert_eq!(Color::from_hex("ff800080"), Some(Color::rgba(255, 128, 0, 128)));
        assert_eq!(Color::from_hex("#f80"), Some(Color::rgb(255, 136, 0)));
        assert_eq!(Color::from_hex("#f808"), Some(Color::rgba(255, 136, 0, 136)));
        assert_eq!(Color::from_hex(" #FFFFFF "), Some(Color::WHITE));
        assert_eq!(Color::from_hex("#12345"), None);
        assert_eq!(Color::from_hex("#gg0000"), None);
        assert_eq!(Color::from_hex("#ééé"), None);
        assert_eq!(Color::from_u32(0x102030), Color::rgb(0x10, 0x20, 0x30));
    }

    #[test]
    fn display_round_trips() {
        for c in [Color::rgb(1, 2, 3), Color::rgba(250, 0, 16, 7)] {
            assert_eq!(c.to_string().parse::<Color>().unwrap(), c);
        }
    }

    #[test]
    fn wgpu_conversion() {
        let c = Color::rgb(255, 128, 0).to_wgpu(true);
        assert!((c.r - 1.0).abs() < 1e-9 && (c.g - 0.2158).abs() < 1e-3 && c.b == 0.0);
        assert!((Color::rgb(0, 128, 0).to_wgpu(false).g - 128.0 / 255.0).abs() < 1e-9);
    }
}
