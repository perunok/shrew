//! What to draw in one frame: rects and text lines in ordered layers, all in physical pixels.
//!
//! A [`Scene`] is a retained buffer the app clears and refills every frame; its vectors keep their capacity, so a
//! steady-state frame allocates nothing. Inside a layer, rects are drawn first and text on top. Layers draw in
//! index order, so overlays (pickers, menus, tooltips) go in a higher layer than the editor.

use std::ops::Range;

use crate::Color;

/// An axis-aligned rectangle in physical pixels, origin top-left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Large enough to mean "no clipping".
    pub const EVERYTHING: Rect = Rect { x: -1.0e7, y: -1.0e7, w: 2.0e7, h: 2.0e7 };

    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The overlap of two rects (empty, with zero size, when they do not touch).
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect { x, y, w: (r - x).max(0.0), h: (b - y).max(0.0) }
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    /// `[x0, y0, x1, y1]`, the form the shaders use.
    pub(crate) fn corners(&self) -> [f32; 4] {
        [self.x, self.y, self.right(), self.bottom()]
    }
}

/// How a run of text looks. Only `bold` and `italic` change shaping; colors are applied per glyph at draw time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextStyle {
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
    /// Draws a line under the run's cells in this color.
    pub underline: Option<Color>,
}

impl TextStyle {
    pub const fn new(color: Color) -> Self {
        TextStyle { color, bold: false, italic: false, underline: None }
    }

    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub const fn underline(mut self, color: Color) -> Self {
        self.underline = Some(color);
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RectCmd {
    pub rect: Rect,
    pub color: Color,
    pub radius: f32,
    pub border: f32,
    pub border_color: Color,
    pub clip: Rect,
}

#[derive(Clone, Debug)]
pub(crate) struct TextCmd {
    pub x: f32,
    pub y: f32,
    /// Into [`Scene::text`].
    pub text: Range<u32>,
    pub style: TextStyle,
    /// Into [`Scene::spans`]; span ranges are byte offsets into this command's text.
    pub spans: Range<u32>,
    pub clip: Rect,
}

#[derive(Default)]
pub(crate) struct Layer {
    pub rects: Vec<RectCmd>,
    pub texts: Vec<TextCmd>,
}

impl Layer {
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty() && self.texts.is_empty()
    }
}

#[derive(Default)]
pub struct Scene {
    pub(crate) background: Option<Color>,
    pub(crate) layers: Vec<Layer>,
    current: usize,
    clips: Vec<Rect>,
    pub(crate) text: String,
    pub(crate) spans: Vec<(Range<u32>, TextStyle)>,
}

impl Scene {
    pub fn new() -> Self {
        Scene::default()
    }

    /// Empties the scene for the next frame, keeping its allocations.
    pub fn clear(&mut self) {
        self.background = None;
        for l in &mut self.layers {
            l.rects.clear();
            l.texts.clear();
        }
        self.current = 0;
        self.clips.clear();
        self.text.clear();
        self.spans.clear();
    }

    /// The color the frame is cleared to (black when never set).
    pub fn background(&mut self, color: Color) {
        self.background = Some(color);
    }

    /// Selects the layer later calls draw into. Layer 0 is the default; higher layers draw on top.
    pub fn layer(&mut self, n: usize) {
        if self.layers.len() <= n {
            self.layers.resize_with(n + 1, Layer::default);
        }
        self.current = n;
    }

    /// Moves to a new layer above every layer used so far and returns its index.
    pub fn push_layer(&mut self) -> usize {
        let n = self.layers.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1).max(self.current + 1);
        self.layer(n);
        n
    }

    pub fn current_layer(&self) -> usize {
        self.current
    }

    /// Clips everything drawn until the matching [`Scene::pop_clip`] to `rect` (intersected with the enclosing clip).
    pub fn push_clip(&mut self, rect: Rect) {
        let r = rect.intersect(&self.clip());
        self.clips.push(r);
    }

    pub fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn clip(&self) -> Rect {
        self.clips.last().copied().unwrap_or(Rect::EVERYTHING)
    }

    fn cur(&mut self) -> &mut Layer {
        if self.layers.is_empty() {
            self.layers.push(Layer::default());
        }
        &mut self.layers[self.current]
    }

    pub fn rect(&mut self, rect: Rect, color: Color) {
        self.rounded_rect(rect, 0.0, color, None);
    }

    /// A rect with rounded corners (`radius` in pixels) and an optional inner border `(width, color)`.
    /// A transparent `color` with a border draws an outline.
    pub fn rounded_rect(&mut self, rect: Rect, radius: f32, color: Color, border: Option<(f32, Color)>) {
        let clip = self.clip();
        let (border, border_color) = border.unwrap_or((0.0, Color::TRANSPARENT));
        if rect.is_empty()
            || !rect.intersects(&clip)
            || (color.is_transparent() && (border <= 0.0 || border_color.is_transparent()))
        {
            return;
        }
        let radius = radius.clamp(0.0, rect.w.min(rect.h) * 0.5);
        self.cur().rects.push(RectCmd { rect, color, radius, border, border_color, clip });
    }

    /// One line of grid text. `x` is the left edge of column 0 and `y_top` the top of the line's cell row; the text
    /// is drawn on [`crate::Metrics::baseline`]. `text` must already be tab-expanded and hold no line breaks; each
    /// char advances by its display width (wide chars take two cells, combining marks none), exactly like
    /// [`crate::columns`]. `spans` are byte ranges into `text` overriding `style`; later spans win where they overlap.
    pub fn text(&mut self, x: f32, y_top: f32, text: &str, style: TextStyle, spans: &[(Range<usize>, TextStyle)]) {
        if text.is_empty() {
            return;
        }
        let clip = self.clip();
        if y_top >= clip.bottom() || x >= clip.right() {
            return;
        }
        let t0 = self.text.len() as u32;
        self.text.push_str(text);
        let s0 = self.spans.len() as u32;
        let len = text.len();
        for (r, st) in spans {
            let (a, b) = (r.start.min(len), r.end.min(len));
            if a < b {
                self.spans.push((a as u32..b as u32, *st));
            }
        }
        let cmd =
            TextCmd { x, y: y_top, text: t0..self.text.len() as u32, style, spans: s0..self.spans.len() as u32, clip };
        self.cur().texts.push(cmd);
    }

    pub(crate) fn text_of(&self, cmd: &TextCmd) -> &str {
        &self.text[cmd.text.start as usize..cmd.text.end as usize]
    }

    pub(crate) fn spans_of(&self, cmd: &TextCmd) -> &[(Range<u32>, TextStyle)] {
        &self.spans[cmd.spans.start as usize..cmd.spans.end as usize]
    }

    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(Layer::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_math() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersect(&b), Rect::new(5.0, 5.0, 5.0, 5.0));
        assert!(a.intersects(&b));
        assert!(a.intersect(&Rect::new(20.0, 0.0, 1.0, 1.0)).is_empty());
        assert!(!a.intersects(&Rect::new(10.0, 0.0, 1.0, 1.0)));
        assert!(a.contains(0.0, 9.9) && !a.contains(10.0, 0.0));
    }

    #[test]
    fn layers_clips_and_reuse() {
        let mut s = Scene::new();
        s.rect(Rect::new(0.0, 0.0, 5.0, 5.0), Color::WHITE);
        s.push_clip(Rect::new(0.0, 0.0, 100.0, 20.0));
        s.text(
            0.0,
            0.0,
            "hello",
            TextStyle::new(Color::BLACK),
            &[(1..99, TextStyle::new(Color::WHITE)), (3..3, TextStyle::default())],
        );
        s.text(0.0, 50.0, "culled below the clip", TextStyle::default(), &[]);
        s.push_clip(Rect::new(50.0, 0.0, 100.0, 100.0));
        s.rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::WHITE); // outside the nested clip
        s.pop_clip();
        s.pop_clip();
        let top = s.push_layer();
        assert_eq!(top, 1);
        s.rect(Rect::new(0.0, 0.0, 5.0, 5.0), Color::TRANSPARENT); // invisible
        s.rounded_rect(Rect::new(0.0, 0.0, 6.0, 4.0), 9.0, Color::WHITE, None);

        assert_eq!(s.layers[0].rects.len(), 1);
        assert_eq!(s.layers[0].texts.len(), 1);
        let t = &s.layers[0].texts[0];
        assert_eq!(s.text_of(t), "hello");
        assert_eq!(s.spans_of(t), &[(1..5, TextStyle::new(Color::WHITE))]);
        assert_eq!(t.clip, Rect::new(0.0, 0.0, 100.0, 20.0));
        assert_eq!(s.layers[1].rects.len(), 1);
        assert_eq!(s.layers[1].rects[0].radius, 2.0);

        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.current_layer(), 0);
        assert!(s.layers[0].rects.capacity() > 0);
    }
}
