//! Grid text layout: shape a line with cosmic-text (fallback fonts, emoji, combining marks, ligatures), then pin
//! every cluster to its cell, so text always lines up with the editor's columns whatever the fallback font's advances.
//!
//! Shaped lines are cached by (text, bold/italic runs); colors are not part of the key, so re-highlighting or moving a
//! selection never reshapes. Entries unused for two frames are evicted.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};

use cosmic_text::{Attrs, AttrsList, CacheKeyFlags, Family, ShapeLine, Shaping, fontdb};
use rustc_hash::{FxHashMap, FxHasher};
use unicode_width::UnicodeWidthChar;

use crate::fonts::Fonts;

/// Display width of a char in cells: 2 for wide (CJK, most emoji), 0 for combining marks, 1 otherwise (control
/// chars included). Matches `shrew_core::text::char_width` for everything but tabs, which callers expand.
#[inline]
pub fn char_columns(c: char) -> usize {
    if (c as u32) < 0x20 { 1 } else { c.width().unwrap_or(1) }
}

/// Display width of a string in cells.
pub fn columns(s: &str) -> usize {
    if s.is_ascii() { s.len() } else { s.chars().map(char_columns).sum() }
}

/// A glyph placed on the grid, relative to the line's left edge and baseline (physical pixels).
#[derive(Clone, Copy, Debug)]
pub(crate) struct GridGlyph {
    pub x: f32,
    pub y: f32,
    /// Byte offset of the cluster start; selects the color span.
    pub byte: u32,
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub weight: fontdb::Weight,
    pub flags: CacheKeyFlags,
}

/// A run of bold (bit 1) and/or italic (bit 2) text, as byte offsets.
pub(crate) type StyleRun = (u32, u32, u8);

/// Where a glyph's bitmap lands relative to the line origin's whole pixel (x) and the baseline (y), and where it
/// sits in the atlas.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placed {
    pub rel: [i32; 2],
    pub uv: [u16; 2],
    pub size: [u16; 2],
    pub color: bool,
}

pub(crate) struct ShapedLine {
    pub glyphs: Vec<GridGlyph>,
    /// Per glyph: `None` until looked up, then the atlas answer (`Some(None)` for blank glyphs).
    pub resolved: Vec<Option<Option<Placed>>>,
    /// The (origin x fraction bits, atlas epoch) `resolved` is valid for.
    pub resolved_key: Option<(u32, u64)>,
    text: Box<str>,
    runs: Box<[StyleRun]>,
    last_used: u64,
}

#[derive(Default)]
pub(crate) struct LineCache {
    map: FxHashMap<u64, ShapedLine>,
    frame: u64,
    pub hits: u32,
    pub misses: u32,
    cols: Vec<u32>,
}

impl LineCache {
    pub fn get(&mut self, fonts: &mut Fonts, text: &str, runs: &[StyleRun]) -> &mut ShapedLine {
        let mut h = FxHasher::default();
        text.hash(&mut h);
        runs.hash(&mut h);
        let mut key = h.finish();
        // Linear probing on the (astronomically rare) collision keeps lookups exact.
        loop {
            match self.map.get(&key) {
                Some(l) if &*l.text == text && &*l.runs == runs => break,
                Some(_) => key = key.wrapping_add(1),
                None => {
                    self.misses += 1;
                    let mut glyphs = Vec::new();
                    shape(fonts, text, runs, &mut self.cols, &mut glyphs);
                    let line = ShapedLine {
                        glyphs,
                        resolved: Vec::new(),
                        resolved_key: None,
                        text: text.into(),
                        runs: runs.into(),
                        last_used: self.frame,
                    };
                    return self.map.entry(key).insert_entry(line).into_mut();
                }
            }
        }
        self.hits += 1;
        let l = self.map.get_mut(&key).unwrap();
        l.last_used = self.frame;
        l
    }

    /// Ends a frame: drops lines not drawn in this frame or the one before.
    pub fn end_frame(&mut self) {
        let frame = self.frame;
        self.map.retain(|_, l| l.last_used + 2 > frame);
        self.frame += 1;
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// Bidi paragraph separators make cosmic-text split (and possibly panic on) the line; they must not reach it.
fn is_paragraph_sep(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2029}')
}

fn shape(fonts: &mut Fonts, text: &str, runs: &[StyleRun], cols: &mut Vec<u32>, out: &mut Vec<GridGlyph>) {
    let m = fonts.metrics;
    let px = m.font_size;

    let text: Cow<str> = if text.contains(is_paragraph_sep) {
        // Same byte length, so offsets keep matching the caller's text.
        Cow::Owned(
            text.chars().map(|c| if is_paragraph_sep(c) { " ".repeat(c.len_utf8()) } else { c.to_string() }).collect(),
        )
    } else {
        Cow::Borrowed(text)
    };

    // Column at every byte offset (interior bytes of a char get the char's column).
    cols.clear();
    let mut col = 0u32;
    for c in text.chars() {
        for _ in 0..c.len_utf8() {
            cols.push(col);
        }
        col += char_columns(c) as u32;
    }
    cols.push(col);

    let mut attrs = AttrsList::new(&fonts.attrs(0));
    for &(a, b, bits) in runs {
        attrs.add_span(a as usize..b as usize, &fonts.attrs(bits));
    }
    if let Some(family) = fonts.emoji_family.filter(|_| !text.is_ascii()) {
        let emoji = Attrs::new().family(Family::Name(family));
        for r in emoji_runs(&text) {
            attrs.add_span(r, &emoji);
        }
    }
    let line = ShapeLine::new(&mut fonts.system, &text, &attrs, Shaping::Advanced, 1);

    for word in line.spans.iter().flat_map(|s| &s.words) {
        let g = &word.glyphs;
        let mut i = 0;
        while i < g.len() {
            // One cluster: consecutive glyphs sharing a start byte (base + marks, or a multi-glyph emoji).
            let start = g[i].start;
            let mut j = i;
            let mut adv = 0.0;
            while j < g.len() && g[j].start == start {
                adv += g[j].x_advance * px;
                j += 1;
            }
            let end = g[i..j].iter().map(|g| g.end).max().unwrap_or(start).min(text.len());
            let c0 = cols[start.min(text.len())];
            let cells = cols[end].saturating_sub(c0).max(1) as f32;
            // Center narrower fallback glyphs in their cells; wider ones overflow to the right.
            let adjust = ((cells * m.cell_width - adv) * 0.5).max(0.0);
            let x0 = c0 as f32 * m.cell_width + adjust;
            let mut cx = 0.0;
            for gl in &g[i..j] {
                out.push(GridGlyph {
                    x: x0 + cx + gl.x_offset * px,
                    y: -gl.y_offset * px,
                    byte: start as u32,
                    font_id: gl.font_id,
                    glyph_id: gl.glyph_id,
                    weight: gl.font_weight,
                    flags: gl.cache_key_flags,
                });
                cx += gl.x_advance * px;
            }
            i = j;
        }
    }
}

/// Byte ranges of emoji sequences (an emoji-presentation char plus any joiners, variation selectors and tags).
fn emoji_runs(text: &str) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    let mut it = text.char_indices().peekable();
    std::iter::from_fn(move || {
        let (start, _) = it.find(|&(_, c)| is_emoji(c))?;
        let mut end = text.len();
        while let Some(&(i, c)) = it.peek() {
            if is_emoji(c) || matches!(c, '\u{200d}' | '\u{fe0f}' | '\u{e0020}'..='\u{e007f}') {
                it.next();
            } else {
                end = i;
                break;
            }
        }
        Some(start..end)
    })
}

/// Pictographs that default to emoji presentation (unicode-width makes exactly those wide).
fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0x1F000..=0x1FAFF) && char_columns(c) == 2
}

/// Turns a default style plus overriding spans into sorted bold/italic runs (empty for plain text, the common case).
pub(crate) fn style_runs(
    len: usize,
    default_bits: u8,
    spans: &[(std::ops::Range<u32>, crate::TextStyle)],
    bytes: &mut Vec<u8>,
    out: &mut Vec<StyleRun>,
) {
    out.clear();
    let bits = |s: &crate::TextStyle| s.bold as u8 | (s.italic as u8) << 1;
    if default_bits == 0 && spans.iter().all(|(_, s)| bits(s) == 0) {
        return;
    }
    bytes.clear();
    bytes.resize(len, default_bits);
    for (r, s) in spans {
        bytes[r.start as usize..r.end as usize].fill(bits(s));
    }
    let mut i = 0;
    while i < len {
        let b = bytes[i];
        let mut j = i + 1;
        while j < len && bytes[j] == b {
            j += 1;
        }
        if b != 0 {
            out.push((i as u32, j as u32, b));
        }
        i = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, FontConfig, TextStyle};

    fn fonts() -> Fonts {
        Fonts::new(FontConfig { system_fonts: false, ..FontConfig::default() }, 1.0)
    }

    #[test]
    fn widths() {
        assert_eq!(columns("abc"), 3);
        assert_eq!(columns("日本"), 4);
        assert_eq!(columns("e\u{301}"), 1);
        assert_eq!(columns("\x01"), 1);
    }

    #[test]
    #[cfg(feature = "embed-font")] // JetBrains Mono's metrics
    fn glyphs_sit_on_the_grid() {
        let mut f = fonts();
        let cw = f.metrics.cell_width;
        let mut cache = LineCache::default();
        let l = cache.get(&mut f, "ab日c", &[]);
        let xs: Vec<(u32, f32)> = l.glyphs.iter().map(|g| (g.byte, g.x)).collect();
        // 'a' col 0, 'b' col 1, '日' cols 2-3 (missing from the embedded font: a centered notdef), 'c' col 4.
        assert_eq!(xs[0], (0, 0.0));
        assert_eq!(xs[1], (1, cw));
        assert_eq!(xs[2].0, 2);
        assert!(xs[2].1 >= 2.0 * cw && xs[2].1 < 4.0 * cw);
        assert_eq!(xs[3], (5, 4.0 * cw));
    }

    #[test]
    #[cfg(feature = "embed-font")] // JetBrains Mono's metrics
    fn combining_marks_share_a_cell() {
        let mut f = fonts();
        let cw = f.metrics.cell_width;
        let mut cache = LineCache::default();
        let l = cache.get(&mut f, "e\u{301}x", &[]);
        let last = l.glyphs.last().unwrap();
        assert_eq!(last.byte, 3);
        assert!((last.x - cw).abs() < 0.01);
        assert!(l.glyphs.iter().all(|g| g.x < 2.0 * cw));
    }

    #[test]
    fn cache_hits_and_eviction() {
        let mut f = fonts();
        let mut cache = LineCache::default();
        cache.get(&mut f, "fn main() {}", &[]);
        cache.get(&mut f, "fn main() {}", &[]);
        cache.get(&mut f, "fn main() {}", &[(0, 2, 1)]);
        assert_eq!((cache.hits, cache.misses, cache.len()), (1, 2, 2));
        cache.end_frame();
        cache.get(&mut f, "fn main() {}", &[]);
        cache.end_frame();
        assert_eq!(cache.len(), 2, "kept for two frames");
        cache.end_frame();
        assert_eq!(cache.len(), 1);
        cache.end_frame();
        cache.end_frame();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn line_breaks_do_not_panic() {
        let mut f = fonts();
        let mut cache = LineCache::default();
        let l = cache.get(&mut f, "a\u{2029}\nb\u{85}c", &[]);
        assert_eq!(l.glyphs.last().unwrap().byte, 8);
    }

    #[test]
    fn emoji_sequences() {
        let t = "a🦀b ☕x 👍🏽! 👨\u{200d}👩\u{200d}👧 ✓";
        let runs: Vec<&str> = emoji_runs(t).map(|r| &t[r]).collect();
        assert_eq!(runs, ["🦀", "☕", "👍🏽", "👨\u{200d}👩\u{200d}👧"]);
    }

    #[test]
    fn runs_from_spans() {
        let (mut bytes, mut out) = (Vec::new(), Vec::new());
        let plain = TextStyle::new(Color::WHITE);
        style_runs(10, 0, &[(0..3, plain)], &mut bytes, &mut out);
        assert!(out.is_empty());
        style_runs(10, 0, &[(2..5, plain.bold()), (4..6, plain.italic()), (6..8, plain)], &mut bytes, &mut out);
        assert_eq!(out, vec![(2, 4, 1), (4, 6, 2)]);
        style_runs(4, 1, &[(1..2, plain)], &mut bytes, &mut out);
        assert_eq!(out, vec![(0, 1, 1), (2, 4, 1)]);
    }
}
