//! Opaque sRGB colors with the bits theming needs: parsing, blending, WCAG contrast and HSL nudges.

use std::fmt;

/// An opaque sRGB color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb::hex(0);
    pub const WHITE: Rgb = Rgb::hex(0xffffff);

    /// `Rgb::hex(0xd6cdc1)`.
    pub const fn hex(v: u32) -> Rgb {
        Rgb { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 }
    }

    /// Parses `#rgb` or `#rrggbb` (the `#` is optional). Alpha forms are refused: see [`parse_rgba`].
    pub fn parse(s: &str) -> Option<Rgb> {
        match parse_rgba(s)? {
            (c, 255) => Some(c),
            _ => None,
        }
    }

    /// Channels as 0..=1 floats, still sRGB-encoded.
    pub fn to_f32(self) -> [f32; 3] {
        [self.r as f32 / 255.0, self.g as f32 / 255.0, self.b as f32 / 255.0]
    }

    /// Linear blend: `t = 0` is `self`, `t = 1` is `other`.
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8;
        Rgb { r: m(self.r, other.r), g: m(self.g, other.g), b: m(self.b, other.b) }
    }

    /// WCAG relative luminance.
    pub fn luminance(self) -> f32 {
        let lin = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG contrast ratio (1..=21).
    pub fn contrast(self, other: Rgb) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Whichever of `a` and `b` reads better on `self`.
    pub fn best_ink(self, a: Rgb, b: Rgb) -> Rgb {
        if self.contrast(a) >= self.contrast(b) { a } else { b }
    }

    /// Hue (degrees), saturation and lightness (0..=1).
    pub fn hsl(self) -> (f32, f32, f32) {
        let [r, g, b] = self.to_f32();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d < 1e-6 {
            return (0.0, 0.0, l);
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        (h, s.clamp(0.0, 1.0), l)
    }

    pub fn from_hsl(h: f32, s: f32, l: f32) -> Rgb {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let hp = h.rem_euclid(360.0) / 60.0;
        let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
        let (r, g, b) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        let k = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
        Rgb { r: k(r), g: k(g), b: k(b) }
    }

    /// The same hue made lighter (positive) or darker, in HSL lightness steps until it reaches `target` contrast
    /// on `bg`. Returns `self` unchanged when it already does; gives up at black or white.
    pub fn readable_on(self, bg: Rgb, target: f32) -> Rgb {
        if self.contrast(bg) >= target {
            return self;
        }
        let (h, s, mut l) = self.hsl();
        let up = bg.luminance() < 0.18;
        let mut c = self;
        for _ in 0..100 {
            l = (l + if up { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
            c = Rgb::from_hsl(h, s, l);
            if c.contrast(bg) >= target || l <= 0.0 || l >= 1.0 {
                break;
            }
        }
        c
    }

    /// Angular distance between two hues in degrees (0 when either is grey).
    pub fn hue_distance(self, other: Rgb) -> f32 {
        let (h1, s1, _) = self.hsl();
        let (h2, s2, _) = other.hsl();
        if s1 < 0.15 || s2 < 0.15 {
            return if (s1 < 0.15) == (s2 < 0.15) { 0.0 } else { 180.0 };
        }
        let d = (h1 - h2).abs() % 360.0;
        d.min(360.0 - d)
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// Parses `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` into a color and its alpha.
pub fn parse_rgba(s: &str) -> Option<(Rgb, u8)> {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if !s.is_ascii() {
        return None;
    }
    let nib = |i: usize| u8::from_str_radix(&s[i..i + 1], 16).ok().map(|v| v * 17);
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    let rgb = |r, g, b| Rgb { r, g, b };
    match s.len() {
        3 => Some((rgb(nib(0)?, nib(1)?, nib(2)?), 255)),
        4 => Some((rgb(nib(0)?, nib(1)?, nib(2)?), nib(3)?)),
        6 => Some((rgb(byte(0)?, byte(2)?, byte(4)?), 255)),
        8 => Some((rgb(byte(0)?, byte(2)?, byte(4)?), byte(6)?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_print() {
        assert_eq!(Rgb::parse("#ff8000"), Some(Rgb::hex(0xff8000)));
        assert_eq!(Rgb::parse("f80"), Some(Rgb::hex(0xff8800)));
        assert_eq!(Rgb::parse("#ff800080"), None);
        assert_eq!(parse_rgba("#ff800080"), Some((Rgb::hex(0xff8000), 0x80)));
        assert_eq!(Rgb::parse("#12345"), None);
        assert_eq!(Rgb::hex(0x0a0b0c).to_string(), "#0a0b0c");
    }

    #[test]
    fn contrast_and_hsl() {
        assert!((Rgb::BLACK.contrast(Rgb::WHITE) - 21.0).abs() < 0.01);
        for c in [0x8be9fd, 0xff79c6, 0x123456, 0x808080] {
            let c = Rgb::hex(c);
            let (h, s, l) = c.hsl();
            let back = Rgb::from_hsl(h, s, l);
            assert!(c.mix(back, 1.0).contrast(c) < 1.02, "{c} -> {back}");
        }
        let bg = Rgb::hex(0x282a36);
        let dim = Rgb::hex(0x44475a);
        let fixed = dim.readable_on(bg, 4.5);
        assert!(fixed.contrast(bg) >= 4.5);
        assert!(fixed.hue_distance(dim) < 10.0);
        assert!(Rgb::hex(0xff0000).hue_distance(Rgb::hex(0x00ff00)) > 100.0);
    }
}
