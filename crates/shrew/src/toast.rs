//! The one toast look: a small floating panel at the bottom right, above the status bar, with a bold title, a few
//! wrapped lines, optional buttons and a close button. Git's progress and error messages and the update
//! notifications use it; each keeps its own state, hit rects and [`Hot`] part (the one under the pointer, see
//! [`hover`]), this only draws.

use crate::colors::{Colors, mix};
use crate::layout::Layout;
use shrew_render::{Color, Rect, Scene, TextStyle, columns};

/// What a toast shows.
pub struct Spec<'a> {
    pub title: &'a str,
    /// The title's color; an error also gets a bar of it down the left edge.
    pub tint: Color,
    pub error: bool,
    pub lines: &'a [String],
    /// Lines shown at most (the last one says there is more).
    pub max_lines: usize,
    /// What the last line says when lines were cut.
    pub more: &'a str,
    /// Buttons under the text, left to right; the first is drawn as the main one.
    pub buttons: &'a [&'a str],
    /// The close button's label (`✕`, `cancel ✕`).
    pub close: &'a str,
    /// The part under the pointer, drawn lit.
    pub hot: Hot,
}

/// The part of a toast under the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hot {
    #[default]
    None,
    Close,
    /// A button, by its index in [`Spec::buttons`].
    Button(usize),
}

/// Where the toast landed.
#[derive(Clone, Debug, Default)]
pub struct Hits {
    pub area: Option<Rect>,
    pub close: Option<Rect>,
    pub buttons: Vec<Rect>,
}

impl Hits {
    /// Whether (`x`, `y`) is on the toast (it takes the pointer: nothing under it lights up or opens a menu).
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.is_some_and(|a| a.contains(x, y))
    }

    /// The part at (`x`, `y`).
    pub fn hot_at(&self, x: f32, y: f32) -> Hot {
        if self.close.is_some_and(|r| r.contains(x, y)) {
            return Hot::Close;
        }
        self.buttons.iter().position(|r| r.contains(x, y)).map_or(Hot::None, Hot::Button)
    }
}

/// The pointer moved: `hot` follows it. (redraw, over a button)
pub fn hover(hits: &Hits, hot: &mut Hot, x: f32, y: f32) -> (bool, bool) {
    let now = hits.hot_at(x, y);
    (std::mem::replace(hot, now) != now, now != Hot::None)
}

/// Draws `spec` with its bottom edge at `bottom` (pass [`bottom`] for the default place) in layer 2.
pub fn draw(scene: &mut Scene, spec: &Spec, bottom: f32, lay: &Layout, c: &Colors) -> Hits {
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let buttons_w: usize = spec.buttons.iter().map(|b| columns(b) + 5).sum();
    let longest = spec.lines.iter().map(|l| columns(l)).chain([columns(spec.title) + 4, buttons_w]).max().unwrap_or(10);
    let w = ((longest + 4) as f32 * cw).clamp(36.0 * cw, (lay.width * 0.5).max(36.0 * cw)).min(lay.width - 4.0 * cw);
    // long lines wrap
    let cols = ((w / cw) as usize).saturating_sub(4).max(10);
    let mut lines: Vec<String> = spec.lines.iter().flat_map(|l| wrap(l, cols)).collect();
    if lines.len() > spec.max_lines.max(1) {
        lines.truncate(spec.max_lines.max(1) - 1);
        lines.push(spec.more.to_string());
    }
    let button_h = (ch + pad).round();
    let buttons_h = if spec.buttons.is_empty() { 0.0 } else { button_h + 2.0 * pad };
    let h = (lines.len() + 1) as f32 * ch + 4.0 * pad + buttons_h;
    let x = (lay.width - w - 2.0 * cw).round();
    let y = (bottom - h).round();
    let panel = Rect::new(x, y, w.round(), h.round());
    scene.layer(2);
    crate::draw::popup(scene, panel, lay, c);
    if spec.error {
        let bar = (3.0 * lay.scale).round();
        scene.rounded_rect(Rect::new(x, y, bar, panel.h), bar / 2.0, c.error, None);
    }
    scene.push_clip(panel);
    let tx = x + 2.0 * cw;
    let ty = y + 2.0 * pad;
    scene.text(tx, ty, spec.title, TextStyle::new(spec.tint).bold(), &[]);
    let radius = (4.0 * lay.scale).round();
    let bw = (columns(spec.close) + 2) as f32 * cw;
    let close = Rect::new((panel.right() - bw - cw).round(), ty, bw.round(), ch);
    let (close_bg, close_fg) = if spec.hot == Hot::Close {
        (c.panel_selection, c.panel_foreground)
    } else {
        (mix(c.panel_selection, c.popup, 0.6), c.panel_muted)
    };
    scene.rounded_rect(close, radius, close_bg, None);
    scene.text(close.x + cw, ty, spec.close, TextStyle::new(close_fg), &[]);
    let fg = if spec.error { c.panel_foreground } else { c.panel_muted };
    for (i, l) in lines.iter().enumerate() {
        scene.text(tx, ty + (i + 1) as f32 * ch, l, TextStyle::new(fg), &[]);
    }
    let mut hits = Hits { area: Some(panel), close: Some(close), buttons: Vec::new() };
    let by = (ty + (lines.len() + 1) as f32 * ch + 2.0 * pad).round();
    let mut bx = tx;
    let ty_b = lay.centered(by, button_h);
    for (i, label) in spec.buttons.iter().enumerate() {
        let r = Rect::new(bx.round(), by, ((columns(label) + 4) as f32 * cw).round(), button_h);
        let (bg, fg, border) = if i == 0 {
            (c.accent, c.accent_fg, None)
        } else {
            (c.panel_selection, c.panel_foreground, Some((lay.scale.round().max(1.0), c.panel_border)))
        };
        // under the pointer: a little lighter or darker, as the status bar's filled items
        let bg = if spec.hot == Hot::Button(i) { crate::statusbar::lit(bg, fg) } else { bg };
        scene.rounded_rect(r, radius, bg, border);
        scene.text((r.x + 2.0 * cw).round(), ty_b, label, TextStyle::new(fg).bold(), &[]);
        hits.buttons.push(r);
        bx = r.right() + cw;
    }
    scene.pop_clip();
    scene.layer(0);
    hits
}

/// The default bottom edge: a little above the status bar.
pub fn bottom(lay: &Layout) -> f32 {
    lay.status.y - 2.0 * lay.pad
}

/// `s` cut into pieces of at most `cols` columns: at a space when there is one, else anywhere. A line that fits
/// is left as it is (git's output aligns with runs of spaces).
pub fn wrap(s: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut out = Vec::new();
    let mut rest = s;
    while columns(rest) > cols {
        let (mut w, mut end, mut space) = (0, 0, None);
        for (i, ch) in rest.char_indices() {
            let cw = shrew_render::char_columns(ch);
            if w + cw > cols {
                break;
            }
            w += cw;
            end = i + ch.len_utf8();
            if ch == ' ' && i > 0 {
                space = Some(i);
            }
        }
        if end == 0 {
            end = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        match space {
            Some(i) => {
                out.push(rest[..i].trim_end().to_string());
                rest = rest[i..].trim_start_matches(' ');
            }
            None => {
                out.push(rest[..end].to_string());
                rest = &rest[end..];
            }
        }
    }
    out.push(rest.to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn wraps_at_spaces_and_keeps_short_lines_as_they_are() {
        assert_eq!(wrap("a  b", 10), ["a  b"]);
        assert_eq!(wrap("one two three", 8), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("x https://example.com/long", 10), ["x", "https://ex", "ample.com/", "long"]);
        assert_eq!(wrap("", 5), [""]);
    }
}
