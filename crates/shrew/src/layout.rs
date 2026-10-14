//! Screen geometry: where the tab bar, gutter, text, status line and command line go, and the inverse (pointer ->
//! line and column). Drawing and hit testing share these numbers, so a click lands on the glyph it is drawn over.

use shrew_render::{Metrics, Rect};

/// Width of the scrollbar's hit zone in logical pixels (the thumb drawn inside it is thinner).
const SCROLLBAR: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    /// Cell size (fractional width, whole-pixel height).
    pub cw: f32,
    pub ch: f32,
    /// Padding unit: 4 logical pixels, rounded.
    pub pad: f32,
    /// Buffer tabs (zero height when there is only one buffer).
    pub tabs: Rect,
    /// The editor: gutter plus text.
    pub text: Rect,
    pub gutter_w: f32,
    /// Digits reserved for line numbers.
    pub digits: usize,
    pub scrollbar: Rect,
    pub status: Rect,
    pub cmdline: Rect,
    /// Whole text rows and columns that fit.
    pub rows: usize,
    pub cols: usize,
    /// The file tree on the left (zero width when hidden; see [`Layout::with_sidebar`]).
    pub sidebar: Rect,
}

impl Layout {
    pub fn new(m: &Metrics, size: (u32, u32), scale: f32, lines: usize, numbers: bool, tabs: bool) -> Layout {
        let (w, h) = (size.0 as f32, size.1 as f32);
        let (cw, ch) = (m.cell_width.max(1.0), m.cell_height.max(1.0));
        let pad = (4.0 * scale).round().max(1.0);
        let tabs_h = if tabs { ch + 2.0 * pad } else { 0.0 };
        let status_h = ch + 2.0 * pad;
        let cmd_h = ch + pad;
        let text_h = (h - tabs_h - status_h - cmd_h).max(ch);
        let digits = lines.max(1).ilog10() as usize + 1;
        let digits = digits.max(3);
        let gutter_w = if numbers { ((digits as f32 + 3.0) * cw).round() } else { (1.5 * cw).round() };
        let sb_w = (SCROLLBAR * scale).round();
        let text = Rect::new(0.0, tabs_h, w, text_h);
        Layout {
            width: w,
            height: h,
            scale,
            cw,
            ch,
            pad,
            tabs: Rect::new(0.0, 0.0, w, tabs_h),
            text,
            gutter_w,
            digits,
            scrollbar: Rect::new(w - sb_w, tabs_h, sb_w, text_h),
            status: Rect::new(0.0, tabs_h + text_h, w, status_h),
            cmdline: Rect::new(0.0, tabs_h + text_h + status_h, w, cmd_h),
            rows: ((text_h / ch).floor() as usize).max(1),
            cols: (((w - gutter_w - sb_w) / cw).floor() as usize).max(1),
            sidebar: Rect::new(0.0, tabs_h, 0.0, text_h),
        }
    }

    /// Makes room for the sidebar, `w` pixels on the left between the top bar and the status bar: the tabs and the
    /// text move right. At least 20 columns stay for the text.
    pub fn with_sidebar(mut self, w: f32) -> Layout {
        let w = w.min(self.width - self.gutter_w - self.scrollbar.w - 20.0 * self.cw).round();
        if w <= 0.0 {
            return self;
        }
        self.sidebar = Rect::new(0.0, self.tabs.y, w, self.status.y - self.tabs.y);
        self.tabs.x += w;
        self.tabs.w -= w;
        self.text.x += w;
        self.text.w -= w;
        self.cols = (((self.text.w - self.gutter_w - self.scrollbar.w) / self.cw).floor() as usize).max(1);
        self
    }

    /// Left edge of the text (column `view.left`).
    pub fn text_x(&self) -> f32 {
        self.text.x + self.gutter_w
    }

    /// The text area without the gutter.
    pub fn text_rect(&self) -> Rect {
        Rect::new(self.text_x(), self.text.y, self.text.w - self.gutter_w, self.text.h)
    }

    /// Top of a text row inside a bar of height `bar_h` starting at `y` (vertically centered, whole pixels).
    pub fn centered(&self, y: f32, bar_h: f32) -> f32 {
        (y + (bar_h - self.ch) / 2.0).round()
    }
}

/// The scroll position actually drawn (fractional while smooth scrolling), in lines and columns.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    pub top: f32,
    pub left: f32,
}

impl Scroll {
    pub fn first_line(&self) -> usize {
        self.top.max(0.0).floor() as usize
    }

    /// Pixels the first line is shifted up by (whole pixels, so text stays crisp).
    pub fn y_offset(&self, ch: f32) -> f32 {
        ((self.top.max(0.0) - self.top.max(0.0).floor()) * ch).round()
    }

    /// Top of `line` on screen.
    pub fn line_y(&self, lay: &Layout, line: usize) -> f32 {
        lay.text.y - self.y_offset(lay.ch) + (line as f32 - self.first_line() as f32) * lay.ch
    }

    /// X of display column `col` (whole-pixel origin).
    pub fn col_x(&self, lay: &Layout, col: f32) -> f32 {
        lay.text_x() - (self.left * lay.cw).round() + col * lay.cw
    }

    /// The line under `y` (may be negative or past the end when the pointer is outside the text).
    pub fn line_at(&self, lay: &Layout, y: f32) -> isize {
        self.first_line() as isize + ((y - lay.text.y + self.y_offset(lay.ch)) / lay.ch).floor() as isize
    }

    /// The fractional display column under `x`.
    pub fn col_at(&self, lay: &Layout, x: f32) -> f32 {
        (x - lay.text_x() + (self.left * lay.cw).round()) / lay.cw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> Metrics {
        Metrics {
            font_size: 15.0,
            cell_width: 9.0,
            cell_height: 21.0,
            baseline: 16.0,
            ascent: 14.0,
            descent: 4.0,
            underline_offset: 18.0,
            underline_thickness: 1.0,
        }
    }

    #[test]
    fn areas_stack_without_gaps() {
        let l = Layout::new(&metrics(), (800, 600), 1.0, 120, true, true);
        assert_eq!(l.tabs.bottom(), l.text.y);
        assert_eq!(l.text.bottom(), l.status.y);
        assert_eq!(l.status.bottom(), l.cmdline.y);
        assert!(l.cmdline.bottom() <= 600.0 + 0.01);
        assert_eq!(l.digits, 3);
        assert_eq!(l.rows, (l.text.h / 21.0) as usize);
        let one = Layout::new(&metrics(), (800, 600), 1.0, 120, false, false);
        assert_eq!(one.tabs.h, 0.0);
        assert!(one.rows > l.rows);
        assert!(one.gutter_w < l.gutter_w);
        assert_eq!(Layout::new(&metrics(), (800, 600), 1.0, 123_456, true, false).digits, 6);
    }

    #[test]
    fn the_sidebar_takes_the_left() {
        let l = Layout::new(&metrics(), (800, 600), 1.0, 120, true, true);
        assert_eq!(l.with_sidebar(0.0), l, "hidden: nothing moves");
        let s = l.with_sidebar(200.0);
        assert_eq!((s.sidebar.x, s.sidebar.w), (0.0, 200.0));
        assert_eq!((s.sidebar.y, s.sidebar.bottom()), (l.tabs.y, l.status.y));
        assert_eq!((s.text.x, s.tabs.x), (200.0, 200.0));
        assert_eq!(s.text.right(), l.text.right());
        assert!(s.cols < l.cols);
        assert_eq!(s.status, l.status, "the status bar keeps the whole width");
        let narrow = l.with_sidebar(10_000.0);
        assert!(narrow.cols >= 19, "the text keeps some room");
    }

    #[test]
    fn hit_testing_inverts_drawing() {
        let l = Layout::new(&metrics(), (800, 600), 2.0, 500, true, false);
        for s in [Scroll { top: 10.0, left: 0.0 }, Scroll { top: 10.4, left: 3.0 }, Scroll { top: 0.95, left: 0.5 }] {
            for line in s.first_line()..s.first_line() + 5 {
                let y = s.line_y(&l, line);
                // anywhere inside the row maps back to the line
                assert_eq!(s.line_at(&l, y + 0.5), line as isize, "{s:?}");
                assert_eq!(s.line_at(&l, y + l.ch - 0.5), line as isize, "{s:?}");
            }
            for col in 0..20 {
                let x = s.col_x(&l, col as f32);
                assert!((s.col_at(&l, x + 0.25 * l.cw) - (col as f32 + 0.25)).abs() < 1e-3);
            }
        }
        assert!(Scroll { top: 0.0, left: 0.0 }.line_at(&l, l.text.y - 1.0) < 0);
    }
}
