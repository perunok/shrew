//! Smooth scrolling and the gliding cursor. Both ease exponentially towards their target and settle in roughly
//! 100 ms; the app redraws continuously only while [`Anim::step`] says something is still moving.

use crate::layout::Scroll;
use std::time::Instant;

/// Time constants of the exponential ease (63% of the way after this long).
const SCROLL_TAU: f32 = 0.040;
const CURSOR_TAU: f32 = 0.028;
/// Close enough to snap: well under half a pixel at any font size.
const EPS: f32 = 0.01;

/// Where things should be this frame.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub buffer: u64,
    pub scroll: Scroll,
    /// The primary cursor as (line, display column).
    pub cursor: (f32, f32),
    pub rows: usize,
    pub smooth_scroll: bool,
    pub cursor_animation: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Anim {
    /// What is drawn.
    pub scroll: Scroll,
    pub cursor: (f32, f32),
    buffer: Option<u64>,
    last: Option<Instant>,
}

impl Anim {
    /// Advances towards `t`; true while still moving (the app then schedules another frame).
    pub fn step(&mut self, t: &Target, now: Instant) -> bool {
        // the first frame of a new animation advances by a nominal frame
        let dt = self.last.map_or(1.0 / 120.0, |l| now.saturating_duration_since(l).as_secs_f32()).min(1.0 / 30.0);
        if self.buffer != Some(t.buffer) {
            self.buffer = Some(t.buffer);
            self.scroll = t.scroll;
            self.cursor = t.cursor;
        }
        if t.smooth_scroll {
            // never animate across more than a screen: jump most of a long way
            let rows = t.rows.max(1) as f32;
            let d = self.scroll.top - t.scroll.top;
            if d.abs() > rows * 1.5 {
                self.scroll.top = t.scroll.top + d.signum() * rows;
            }
            approach(&mut self.scroll.top, t.scroll.top, dt, SCROLL_TAU);
            approach(&mut self.scroll.left, t.scroll.left, dt, SCROLL_TAU);
        } else {
            self.scroll = t.scroll;
        }
        let (l, c) = t.cursor;
        // typing moves one cell at a time: never make it lag
        let typing = self.cursor.0 == l && (self.cursor.1 - c).abs() <= 1.5;
        // a jump further than a screen (G, a search, a click after scrolling away) lands at once, not flying in
        let far = (self.cursor.0 - l).abs() > t.rows.max(1) as f32;
        if t.cursor_animation && !typing && !far {
            approach(&mut self.cursor.0, l, dt, CURSOR_TAU);
            approach(&mut self.cursor.1, c, dt, CURSOR_TAU);
        } else {
            self.cursor = t.cursor;
        }
        let moving = self.scroll != t.scroll || self.cursor != t.cursor;
        self.last = moving.then_some(now);
        moving
    }

    /// Jumps straight to a scroll position (touchpad scrolling is already smooth).
    pub fn set_scroll(&mut self, s: Scroll) {
        self.scroll = s;
    }
}

fn approach(x: &mut f32, target: f32, dt: f32, tau: f32) {
    *x += (target - *x) * (1.0 - (-dt / tau).exp());
    if (target - *x).abs() < EPS {
        *x = target;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn target(top: f32, cursor: (f32, f32)) -> Target {
        Target {
            buffer: 1,
            scroll: Scroll { top, left: 0.0 },
            cursor,
            rows: 40,
            smooth_scroll: true,
            cursor_animation: true,
        }
    }

    #[test]
    fn settles_and_then_stops_asking_for_frames() {
        let mut a = Anim::default();
        let mut now = Instant::now();
        assert!(!a.step(&target(0.0, (0.0, 0.0)), now), "first frame snaps");
        assert!(a.step(&target(20.0, (25.0, 3.0)), now));
        let mut frames = 0;
        while a.step(&target(20.0, (25.0, 3.0)), now) {
            now += Duration::from_millis(16);
            frames += 1;
            assert!(frames < 40, "takes too long");
        }
        assert_eq!(a.scroll.top, 20.0);
        assert_eq!(a.cursor, (25.0, 3.0));
        assert!(frames > 3, "it did animate");
    }

    #[test]
    fn long_jumps_and_typing_do_not_crawl() {
        let mut a = Anim::default();
        let now = Instant::now();
        a.step(&target(0.0, (0.0, 0.0)), now);
        a.step(&target(10_000.0, (0.0, 0.0)), now);
        assert!(a.scroll.top > 10_000.0 - 41.0);
        // typing: the cursor moves one column and is there at once
        let mut a = Anim::default();
        a.step(&target(0.0, (3.0, 4.0)), now);
        assert!(!a.step(&target(0.0, (3.0, 5.0)), now));
        assert_eq!(a.cursor, (3.0, 5.0));
        // a cursor jump of more than a screen lands at once
        let mut a = Anim::default();
        a.step(&target(0.0, (3.0, 4.0)), now);
        assert!(!a.step(&target(0.0, (900.0, 4.0)), now));
        assert_eq!(a.cursor, (900.0, 4.0));
        // animations off: everything snaps
        let mut t = target(50.0, (60.0, 9.0));
        t.smooth_scroll = false;
        t.cursor_animation = false;
        assert!(!a.step(&t, now));
        assert_eq!(a.scroll.top, 50.0);
    }
}
