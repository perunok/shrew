//! Regression tests for mouse (and some keyboard) roughness: each one played the way a person would, with a frame
//! laid out and drawn before every event (the same driver as `tests.rs`).

use super::*;
use shrew_core::Document;
use shrew_render::{Metrics, Scene};
use std::sync::Arc;

struct Sim {
    st: State,
    ui: Ui,
    scene: Scene,
    mouse: Mouse,
    now: Instant,
    size: (u32, u32),
}

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

impl Sim {
    fn with(texts: &[&str]) -> Sim {
        let mut ui = Ui::new();
        ui.animate = false;
        let st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
        let mut s = Sim { st, ui, scene: Scene::new(), mouse: Mouse::new(), now: Instant::now(), size: (1000, 600) };
        for t in texts {
            s.st.ed.install_document(Document::from_str(t));
        }
        s.st.process_events();
        s.frame();
        s
    }

    fn frame(&mut self) -> Layout {
        let lay = crate::layout::compute_with(&metrics(), self.size, 1.0, &self.st);
        crate::draw::build(&mut self.scene, &mut self.st, &mut self.ui, &lay, Instant::now());
        lay
    }

    /// Plays `--keys` input (keys and mouse tokens).
    fn run(&mut self, input: &str) {
        for i in parse_input(input).unwrap() {
            match i {
                Input::Key(k) => self.st.key(k),
                Input::Mouse(ev) => {
                    let lay = self.frame();
                    self.now += ev.after;
                    let _ = self.mouse.feed(&mut self.st, &mut self.ui, Some(lay), &ev, 1.0, self.now);
                }
            }
        }
        self.frame();
    }

    /// One wheel event as the window delivers it.
    fn wheel(&mut self, w: Wheel) {
        let lay = self.frame();
        let _ = self.mouse.wheel(&mut self.st, &mut self.ui, Some(lay), w);
        self.frame();
    }

    /// The drag autoscroll's timer, firing for `secs` seconds at 60 frames a second.
    fn hold(&mut self, secs: f32) {
        let end = self.now + Duration::from_secs_f32(secs);
        while self.now < end {
            self.now += Duration::from_millis(16);
            let lay = self.frame();
            let _ = self.mouse.tick(&mut self.st, &mut self.ui, Some(lay), self.now);
        }
    }

    /// The middle of char `col` of screen row `row`.
    fn cell(&mut self, row: usize, col: usize) -> (f32, f32) {
        let lay = self.frame();
        (lay.text_x() + (col as f32 + 0.5) * lay.cw, lay.text.y + (row as f32 + 0.5) * lay.ch)
    }

    fn selected(&self) -> String {
        let b = self.st.ed.buf();
        let r = b.sel.primary();
        let to = if self.st.ed.mode.is_visual() { r.to() + 1 } else { r.to() };
        b.rope().slice(r.from()..to.min(b.rope().len_chars())).to_string()
    }

    fn top(&self) -> usize {
        self.st.ed.buf().view.top
    }
}

fn lines(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn a_vim_drag_shows_visual_mode_while_the_button_is_down() {
    let mut s = Sim::with(&["hello world\nsecond line\n"]);
    let (x0, y0) = s.cell(0, 0);
    let (x1, y1) = s.cell(0, 4);
    // the button still down: visual mode already, with the char under the pointer
    s.run(&format!("<down:{x0},{y0}><move:{},{y0}><move:{x1},{y1}>", x0 + 20.0));
    assert_eq!(s.st.ed.mode, Mode::Visual, "not INSERT with a bar cursor until the release");
    assert_eq!(s.selected(), "hello");
    s.run(&format!("<up:{x1},{y1}>"));
    assert_eq!((s.st.ed.mode, s.selected().as_str()), (Mode::Visual, "hello"), "the release changes nothing");
    // the right half of `o` is still `o`: the selection does not take the space after it
    let (x1, y1) = (x1 + 3.0, y1);
    s.run(&format!("<Esc><drag:{x0},{y0},{x1},{y1}>"));
    assert_eq!(s.selected(), "hello");
    // shift+click takes the char clicked
    let (x, y) = s.cell(0, 8);
    s.run(&format!("<Esc><click:{x0},{y0}><S-click:{x},{y}>"));
    assert_eq!(s.selected(), "hello wor");
}

#[test]
fn double_click_on_the_right_half_of_a_last_letter_selects_that_word() {
    let mut s = Sim::with(&["foo bar\n"]);
    s.run(":set novim<CR>");
    let (x, y) = s.cell(0, 2);
    s.run(&format!("<dclick:{},{y}>", x + 3.0));
    assert_eq!(s.selected(), "foo");
}

#[test]
fn drag_autoscroll_starts_slowly_speeds_up_and_stops_at_the_end() {
    let text = lines(600);
    let mut s = Sim::with(&[&text]);
    let lay = s.frame();
    let (x, y) = s.cell(2, 3);
    let below = lay.text.bottom() + 2.0;
    s.run(&format!("<down:{x},{y}><move:{x},{below}>"));
    let start = s.top();
    s.hold(1.0);
    let slow = s.top() - start;
    assert!((8..=25).contains(&slow), "just past the edge: about a dozen lines a second, not 60: {slow}");
    // the head follows the bottom row
    let b = s.st.ed.buf();
    let head_line = text::line_of(b.rope(), b.sel.primary().head);
    assert_eq!(head_line, s.top() + lay.rows - 1);
    // five rows further out: much faster
    s.run(&format!("<move:{x},{}>", below + 5.0 * lay.ch));
    let start = s.top();
    s.hold(1.0);
    let fast = s.top() - start;
    assert!(fast > 5 * slow, "{fast} vs {slow}");
    // it stops with the last line at the bottom, not scrolled away
    s.hold(6.0);
    assert_eq!(s.top(), 601 - lay.rows, "the last line (empty) sits at the bottom");
    assert_eq!(s.mouse.deadline(), None, "and the timer stops");
    s.run(&format!("<up:{x},{below}>"));
    // up past the top edge
    s.run(&format!("<down:{x},{y}><move:{x},{}>", lay.text.y - 3.0));
    let start = s.top();
    s.hold(0.5);
    assert!(s.top() < start);
}

#[test]
fn drag_autoscroll_goes_sideways_along_long_lines() {
    let long = format!("{}end\nshort\n", "x".repeat(300));
    let mut s = Sim::with(&[&long]);
    s.run(":set novim<CR>");
    let lay = s.frame();
    let (x, y) = s.cell(0, 2);
    let right = lay.text_x() + lay.cols as f32 * lay.cw + 10.0;
    s.run(&format!("<down:{x},{y}><move:{right},{y}>"));
    s.hold(6.0);
    let left = s.st.ed.buf().view.left;
    assert!(left > 0, "it scrolled right");
    assert!(left + lay.cols <= 304 + 1, "but not past the end of the line: {left}");
    assert_eq!(s.st.ed.buf().sel.primary().head, 303, "the selection reaches the end");
    // back past the left edge (over the gutter)
    s.run(&format!("<move:{},{y}>", lay.text.x + 2.0));
    s.hold(6.0);
    assert_eq!(s.st.ed.buf().view.left, 0);
}

#[test]
fn a_grabbed_scrollbar_thumb_does_not_jump() {
    let text = lines(500);
    let mut s = Sim::with(&[&text]);
    s.run("<wheel:300,200,40>");
    let top = s.top();
    assert_eq!(top, 120);
    let lay = s.frame();
    let (track, total) = s.ui.hits.scrollbar.unwrap();
    let thumb = crate::draw::scrollbar_thumb(&lay, track, total, top as f32);
    // pressed near the thumb's bottom edge: nothing moves
    let (x, y) = (track.x + 2.0, thumb.bottom() - 2.0);
    s.run(&format!("<down:{x},{y}>"));
    assert_eq!(s.top(), top, "a press on the thumb does not move it");
    // dragged: the thumb keeps its offset under the pointer
    s.run(&format!("<move:{x},{}>", y + 40.0));
    let lay = s.frame();
    let moved = crate::draw::scrollbar_thumb(&lay, track, total, s.top() as f32);
    assert!((moved.y - (thumb.y + 40.0)).abs() <= 2.0, "{} vs {}", moved.y, thumb.y + 40.0);
    s.run(&format!("<up:{x},{}>", y + 40.0));
    // a click on the track beside the thumb centres it there
    let y = track.bottom() - 30.0;
    s.run(&format!("<click:{x},{y}>"));
    let lay = s.frame();
    let t = crate::draw::scrollbar_thumb(&lay, track, total, s.top() as f32);
    assert!(t.y <= y && y <= t.bottom(), "the thumb is under the click");
}

#[test]
fn touchpads_and_fine_wheels_scroll_everything() {
    let text = lines(500);
    let mut s = Sim::with(&[&text]);
    // a wheel that sends quarter notches: three lines per whole notch, none lost
    for _ in 0..8 {
        s.wheel(Wheel::Lines(0.0, -0.25));
    }
    assert_eq!(s.top(), 6);
    // the palette under a touchpad: 2 pixels at a time add up
    s.run("<C-S-p>");
    for _ in 0..40 {
        s.wheel(Wheel::Pixels(0.0, -2.0));
    }
    assert!(s.ui.picker_top > 0, "the list scrolled");
    assert_eq!(s.st.ed.picker.as_ref().unwrap().selected, 0, "the highlight stays put");
    s.run("<Esc>:about<CR>");
    for _ in 0..40 {
        s.wheel(Wheel::Pixels(0.0, -2.0));
    }
    assert!(s.st.ed.about.unwrap() > 0, "the About panel scrolled");
    s.run("<Esc>");
    // Ctrl and a touchpad: zoom a step at a time
    let size = s.st.font.size;
    s.mouse.mods.ctrl = true;
    for _ in 0..30 {
        s.wheel(Wheel::Pixels(0.0, 2.0));
    }
    s.mouse.mods.ctrl = false;
    assert!(s.st.font.size > size);
}

#[test]
fn sideways_scrolling_with_shift_stops_at_the_widest_line() {
    let long = format!("{}\n", "y".repeat(200));
    let mut s = Sim::with(&[&long]);
    let lay = s.frame();
    s.run("<S-wheel:300,200,2>");
    assert_eq!(s.st.ed.buf().view.left, 12, "Shift turns the wheel sideways");
    assert_eq!(s.top(), 0);
    s.run("<S-wheel:300,200,100>");
    assert_eq!(s.st.ed.buf().view.left, 201 - lay.cols, "no further than the end of the line");
    s.run("<S-wheel:300,200,-100>");
    assert_eq!(s.st.ed.buf().view.left, 0);
    // a touchpad's small sideways moves add up too
    for _ in 0..20 {
        s.wheel(Wheel::Pixels(-2.0, 0.0));
    }
    assert!(s.st.ed.buf().view.left >= 3);
}

#[test]
fn the_wheel_scrolls_a_pickers_list_and_the_pointer_highlights_rows() {
    let mut s = Sim::with(&["text\n"]);
    s.run("<C-S-p>");
    let rows = s.ui.hits.picker_rows.len();
    s.run("<wheel:500,200,2>");
    assert_eq!(s.ui.picker_top, 6, "three rows a notch");
    assert_eq!(s.st.ed.picker.as_ref().unwrap().selected, 0);
    // the highlighted row is scrolled away and stays highlighted; a key brings it back into view
    s.run("<Down>");
    assert_eq!(s.ui.picker_top, 1);
    let (r, i) = s.ui.hits.picker_rows[3];
    s.run(&format!("<move:{},{}>", r.x + 20.0, r.y + 5.0));
    assert_eq!(s.ui.picker_hover.map(|h| h + s.ui.picker_top), Some(i), "the row under the pointer");
    assert!(rows > 3);
    // after a scroll the rows (what a click picks) are the ones drawn, and the hover moved with the list
    s.wheel(Wheel::Lines(0.0, -1.0));
    assert_eq!(s.ui.hits.picker_rows[0].1, s.ui.picker_top);
    assert_eq!(s.ui.picker_hover, Some(3), "still the fourth row on screen");
    // closing the picker forgets the hover
    s.run("<Esc>");
    assert_eq!(s.ui.picker_hover, None);
}

#[test]
fn every_menu_opens_with_alt_and_its_underlined_letter() {
    let mut s = Sim::with(&["text\n"]);
    let mut menus = vec![("<M-s>", "Selection"), ("<M-f>", "File"), ("<M-h>", "Help")];
    if cfg!(feature = "terminal") {
        menus.push(("<M-t>", "Terminal"));
    }
    if cfg!(feature = "git") {
        menus.push(("<M-i>", "Git"));
    }
    for (key, title) in menus {
        s.run(key);
        let m = s.st.titlebar.menu.as_ref().unwrap_or_else(|| panic!("{key} opened nothing"));
        assert_eq!(m.menus[m.open].title, title);
        s.run("<Esc>");
    }
    assert_eq!(s.st.ed.buf().rope().to_string(), "text\n");
}

#[test]
fn a_click_below_the_last_line_goes_to_the_end_of_the_text() {
    let mut s = Sim::with(&["one\ntwo three"]);
    s.run(":set novim<CR>");
    let (x, y) = s.cell(10, 1);
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!(s.st.ed.buf().sel.primary().head, 13);
    // vim keeps the column, on the last line
    s.st.ed.set_feature(shrew_core::Feature::Vim, true);
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!(s.st.ed.buf().sel.primary().head, 5);
}

#[test]
fn a_settings_list_scrolls_under_a_touchpad() {
    let mut s = Sim::with(&["text\n"]);
    s.size = (1000, 420);
    // Appearance > Light desktop theme: a list of every theme
    s.run("<C-,>ljjj<CR>");
    let list = |s: &Sim| s.st.settings.panel.as_ref().and_then(|p| p.list.as_ref().map(|l| (l.top, l.options.len())));
    let rows = s.st.settings.panel.as_ref().unwrap().hits.list_rows;
    let Some((before, n)) = list(&s) else { panic!("the list did not open") };
    assert!(n > rows, "{n} themes, {rows} rows: the list must scroll");
    for _ in 0..40 {
        s.wheel(Wheel::Pixels(0.0, -2.0));
    }
    assert!(list(&s).unwrap().0 > before, "2 pixels at a time add up to rows");
}

#[test]
fn the_wheel_while_selecting_extends_the_selection_and_the_pointer_stays_a_text_beam() {
    let text = lines(300);
    let mut s = Sim::with(&[&text]);
    s.run(":set novim<CR>");
    let (x0, y0) = s.cell(1, 0);
    let (x1, y1) = s.cell(3, 0);
    s.run(&format!("<down:{x0},{y0}><move:{x1},{y1}>"));
    assert_eq!(s.mouse.icon(), CursorIcon::Text);
    s.wheel(Wheel::Lines(0.0, -2.0));
    assert_eq!(s.top(), 6);
    let b = s.st.ed.buf();
    assert_eq!(text::line_of(b.rope(), b.sel.primary().head), 9, "the head is the line now under the pointer");
    // over the title bar mid-drag: still the text beam, no flicker to an arrow
    s.run(&format!("<move:{x1},12>"));
    assert_eq!(s.mouse.icon(), CursorIcon::Text);
    s.run(&format!("<up:{x1},4><move:{x1},12>"));
    assert_eq!(s.mouse.icon(), CursorIcon::Default, "an arrow over the bar once the button is up");
}

#[test]
fn moving_over_the_text_draws_nothing_and_hovers_draw_once() {
    let text = lines(200);
    let mut s = Sim::with(&[&text]);
    let moved = |s: &mut Sim, x: f32, y: f32| {
        let lay = s.frame();
        s.mouse.moved(&mut s.st, &mut s.ui, Some(lay), x, y, s.now)
    };
    let (x, y) = s.cell(3, 3);
    let _ = moved(&mut s, x, y);
    for i in 0..20 {
        let r = moved(&mut s, x + i as f32 * 7.0, y + i as f32 * 5.0);
        assert!(!r.redraw && !r.input, "a move over the text redrew ({i})");
    }
    assert_eq!(s.mouse.icon(), CursorIcon::Text);
    // onto the scrollbar: one redraw (it lights up), then none while it stays there
    let (track, _) = s.ui.hits.scrollbar.unwrap();
    assert!(moved(&mut s, track.x + 2.0, track.y + 50.0).redraw);
    assert!(!moved(&mut s, track.x + 2.0, track.y + 60.0).redraw);
    assert!(moved(&mut s, x, y).redraw, "and one when it leaves");
}

#[test]
fn a_shrinking_window_keeps_the_cursor_in_sight() {
    let text = lines(200);
    let mut s = Sim::with(&[&text]);
    let lay = s.frame();
    // the cursor on the bottom row
    s.run(&format!("{}G", lay.rows - 5));
    assert_eq!(s.top(), 0);
    s.size = (1000, 300);
    let small = s.frame();
    let b = s.st.ed.buf();
    let line = b.cursor_line();
    assert!((b.view.top..b.view.top + small.rows).contains(&line), "line {line} out of sight");
    // scrolled away on purpose: a resize leaves the view alone
    s.size = (1000, 600);
    s.run("<wheel:300,100,20>");
    let top = s.top();
    s.size = (1000, 300);
    s.frame();
    assert_eq!(s.top(), top);
}

#[test]
fn the_wheel_scrolls_a_menu_taller_than_the_window() {
    let mut s = Sim::with(&["text\n"]);
    s.size = (900, 300);
    s.run("<M-e>");
    s.frame();
    let m = s.st.titlebar.menu.as_ref().unwrap();
    assert!(m.max_top > 0, "Edit does not fit in 300 pixels");
    let panel = *m.hits.panels.last().unwrap();
    let (x, y) = (panel.x + 20.0, panel.y + panel.h / 2.0);
    s.run(&format!("<wheel:{x},{y},2>"));
    let m = s.st.titlebar.menu.as_ref().unwrap();
    assert_eq!(m.top, 6.min(m.max_top), "the hovered entry did not pull it back");
    s.run(&format!("<wheel:{x},{y},50>"));
    let m = s.st.titlebar.menu.as_ref().unwrap();
    assert_eq!(m.top, m.max_top, "no further than the last entry");
    // a key brings the highlight back into view
    s.run("<Home>");
    assert_eq!(s.st.titlebar.menu.as_ref().unwrap().top, 0);
}

#[test]
fn a_click_on_a_short_line_while_scrolled_right_shows_the_cursor() {
    let long = format!("{}\nab\n", "z".repeat(300));
    let mut s = Sim::with(&[&long]);
    s.run("<S-wheel:300,200,30>");
    assert!(s.st.ed.buf().view.left > 100);
    let (x, y) = s.cell(1, 10);
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!(s.st.ed.buf().sel.primary().head, 302, "on the short line's last char");
    assert_eq!(s.st.ed.buf().view.left, 0, "and in sight");
}

#[test]
fn a_middle_drag_or_shift_alt_selects_a_box_of_columns() {
    let mut s = Sim::with(&["abcdef\nab\nabcdef\nabcdef\n"]);
    s.run(":set novim<CR>");
    let (x0, y0) = s.cell(0, 1);
    let (x1, y1) = s.cell(2, 4);
    // cell middles: the box's edges round to the column boundaries nearest the pointer
    let (x0, x1) = (x0 - 4.0, x1 - 4.0);
    s.run(&format!("<mdown:{x0},{y0}><move:{x1},{y1}><mup:{x1},{y1}>"));
    let b = s.st.ed.buf();
    let got: Vec<(usize, usize)> = b.sel.ranges().iter().map(|r| (r.from(), r.to())).collect();
    // "ab" is too short for columns 1..4 but reaches column 1: its "b"
    assert_eq!(got, vec![(1, 4), (8, 9), (11, 14)]);
    assert_eq!(s.mouse.drag, Drag::None, "the middle button's release ends it");
    // Shift+Alt+click: from the cursor to the click
    let (x, y) = s.cell(3, 0);
    s.run(&format!("<click:{},{y}>", x - 4.0));
    let (x, y) = s.cell(0, 2);
    s.run(&format!("<S-A-click:{},{y}>", x - 4.0));
    assert_eq!(s.st.ed.buf().sel.len(), 4, "a caret span on each of the four lines");
    // a plain middle click is a caret, nothing selected
    let (x, y) = s.cell(1, 1);
    s.run(&format!("<mclick:{},{y}>", x - 4.0));
    assert_eq!(s.st.ed.buf().sel.len(), 1);
    assert_eq!(s.st.ed.buf().sel.primary().head, 8);
}
