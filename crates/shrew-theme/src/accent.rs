//! Accents: one hue laid over any theme, replacing its accent, selection, cursor and normal-mode badge. Each has a
//! variant for light and for dark themes (the fill, the ink that goes on it, and a same-hue color for text), the
//! same 18 as Slinger's.

use crate::color::Rgb;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccentVariant {
    /// Fills: cursor, mode badge, picker marks.
    pub fill: Rgb,
    /// Text on the fill.
    pub fg: Rgb,
    /// Accent-colored text on the theme's background.
    pub text: Rgb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accent {
    pub id: &'static str,
    pub label: &'static str,
    pub light: AccentVariant,
    pub dark: AccentVariant,
}

/// "Use the theme's own accent".
pub const THEME_ACCENT: &str = "theme";

const fn v(fill: u32, fg: u32, text: u32) -> AccentVariant {
    AccentVariant { fill: Rgb::hex(fill), fg: Rgb::hex(fg), text: Rgb::hex(text) }
}

const fn a(id: &'static str, label: &'static str, light: AccentVariant, dark: AccentVariant) -> Accent {
    Accent { id, label, light, dark }
}

const INK: u32 = 0x10121a;
const WHITE: u32 = 0xffffff;

pub static ACCENTS: &[Accent] = &[
    a("blue", "Blue", v(0x2563eb, WHITE, 0x1652da), v(0x60a5fa, INK, 0x87bcff)),
    a("indigo", "Indigo", v(0x4f46e5, WHITE, 0x4d42e1), v(0x818cf8, INK, 0xa5b1ff)),
    a("violet", "Violet", v(0x7c3aed, WHITE, 0x742de2), v(0xa78bfa, INK, 0xbeacfe)),
    a("purple", "Purple", v(0x9333ea, WHITE, 0x8720dc), v(0xc084fc, INK, 0xd0a4ff)),
    a("fuchsia", "Fuchsia", v(0xc026d3, WHITE, 0xa202b3), v(0xe879f9, INK, 0xf193ff)),
    a("pink", "Pink", v(0xdb2777, WHITE, 0xba0360), v(0xf472b6, INK, 0xfe95c9)),
    a("rose", "Rose", v(0xe11d48, WHITE, 0xbd0137), v(0xfb7185, INK, 0xff98a3)),
    a("red", "Red", v(0xdc2626, WHITE, 0xbf0413), v(0xf87171, INK, 0xff9b97)),
    a("orange", "Orange", v(0xe25300, INK, 0xaa3d02), v(0xfb923c, INK, 0xffa562)),
    a("amber", "Amber", v(0xc46904, INK, 0x924c00), v(0xfbbf24, INK, 0xfbbf24)),
    a("yellow", "Yellow", v(0xae7501, INK, 0x845803), v(0xfacc15, INK, 0xfacc15)),
    a("lime", "Lime", v(0x589102, INK, 0x416d00), v(0xa3e635, INK, 0xa3e635)),
    a("green", "Green", v(0x039641, INK, 0x00712f), v(0x4ade80, INK, 0x4ade80)),
    a("emerald", "Emerald", v(0x049366, INK, 0x036e4c), v(0x34d399, INK, 0x39d69c)),
    a("teal", "Teal", v(0x029185, INK, 0x046d64), v(0x2dd4bf, INK, 0x2dd4bf)),
    a("cyan", "Cyan", v(0x0a8baa, INK, 0x066880), v(0x22d3ee, INK, 0x22d3ee)),
    a("sky", "Sky", v(0x0284c7, INK, 0x02669b), v(0x38bdf8, INK, 0x51c6fe)),
    a("slate", "Slate", v(0x475569, WHITE, 0x475569), v(0x94a3b8, INK, 0xaab9ce)),
];

/// An accent by id (`None` for unknown ids and for [`THEME_ACCENT`]).
pub fn accent(id: &str) -> Option<&'static Accent> {
    ACCENTS.iter().find(|a| a.id.eq_ignore_ascii_case(id))
}
