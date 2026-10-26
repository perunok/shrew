//! The text's and the gutter's menus, the cursor moving before them, the terminal's and a diff tab's menus, and
//! the primary selection, driven by mouse tokens as `--keys` plays them (a frame is drawn before each event).

use crate::context_menu::MenuItem;
use crate::draw::Ui;
use crate::layout::Layout;
use crate::mouse::{Input, Mouse, Target, parse_input};
use crate::state::State;
use shrew_core::{Document, Feature, Mode};
use shrew_render::{Metrics, Rect, Scene};
use std::sync::Arc;
use std::time::Instant;

struct Sim {
    st: State,
    ui: Ui,
    scene: Scene,
    mouse: Mouse,
    now: Instant,
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
    fn with(text: &str) -> Sim {
        let mut ui = Ui::new();
        ui.animate = false;
        let st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
        let mut s = Sim { st, ui, scene: Scene::new(), mouse: Mouse::new(), now: Instant::now() };
        s.st.ed.install_document(Document::from_str(text));
        s.st.process_events();
        s.frame();
        s
    }

    fn frame(&mut self) -> Layout {
        let lay = crate::layout::compute_with(&metrics(), (1000, 600), 1.0, &self.st);
        crate::draw::build(&mut self.scene, &mut self.st, &mut self.ui, &lay, Instant::now());
        lay
    }

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

    /// The middle of char `col` of line `line`.
    fn cell(&mut self, line: usize, col: usize) -> (f32, f32) {
        let lay = self.frame();
        (lay.text_x() + (col as f32 + 0.5) * lay.cw, lay.text.y + (line as f32 + 0.5) * lay.ch)
    }

    fn head(&self) -> usize {
        self.st.ed.buf().sel.primary().head
    }

    fn selected(&self) -> String {
        let b = self.st.ed.buf();
        let r = b.sel.primary();
        let to = if self.st.ed.mode.is_visual() { r.to() + 1 } else { r.to() };
        b.rope().slice(r.from()..to.min(b.rope().len_chars())).to_string()
    }

    /// The open menu's top-level entry labelled `label`.
    fn item(&self, label: &str) -> Option<&MenuItem> {
        self.st.menu.as_ref()?.items.iter().find(|e| e.label() == label)
    }

    fn enabled(&self, label: &str) -> bool {
        self.item(label).unwrap_or_else(|| panic!("no entry {label:?} in {:?}", self.labels())).enabled()
    }

    fn labels(&self) -> Vec<String> {
        self.st.menu.as_ref().map_or(Vec::new(), |m| m.items.iter().map(|e| e.label().to_string()).collect())
    }

    /// Clicks the open menu's entry `label` (as drawn in the last frame).
    fn click(&mut self, label: &str) {
        self.frame();
        let m = self.st.menu.as_ref().expect("a menu");
        let row = m.hits[0].rows.iter().find(|r| m.items[r.1].label() == label).expect(label).0;
        let (x, y) = mid(row);
        self.run(&format!("<click:{x},{y}>"));
    }
}

fn mid(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

// ----- the cursor before the menu -----

#[test]
fn a_right_click_moves_the_cursor_to_the_char_clicked_first() {
    let mut s = Sim::with("hello world\nsecond line\n");
    let (x, y) = s.cell(1, 3);
    s.run(&format!("<rclick:{x},{y}>"));
    assert_eq!((s.st.ed.mode, s.head()), (Mode::Normal, 12 + 3), "vim: on the char clicked, still normal mode");
    let m = s.st.menu.as_ref().expect("the text's menu");
    assert_eq!(m.target, Target::Text(15));
    assert_eq!(m.selected, None, "opened by the mouse: nothing highlighted");
    assert!(s.item("Copy Line").is_some() && s.item("Cut Line").is_some(), "nothing selected: whole lines");
    assert!(s.enabled("Select All Occurrences"), "on a word");
    // Esc closes it; the cursor stays
    s.run("<Esc>");
    assert!(s.st.menu.is_none());
    assert_eq!(s.head(), 15);
}

#[test]
fn a_right_click_inside_a_selection_keeps_it() {
    let mut s = Sim::with("hello world\nsecond line\n");
    let (x0, y0) = s.cell(0, 0);
    let (x1, y1) = s.cell(0, 4);
    s.run(&format!("<drag:{x0},{y0},{x1},{y1}>"));
    assert!(s.st.ed.mode.is_visual());
    let (x, y) = s.cell(0, 2);
    s.run(&format!("<rclick:{x},{y}>"));
    assert!(s.st.ed.mode.is_visual(), "the visual selection stays");
    assert_eq!(s.selected(), "hello");
    assert!(s.item("Copy").is_some() && s.item("Copy Line").is_none());
    assert!(s.labels().iter().any(|l| l == "Find \u{201c}hello\u{201d} in Project"), "{:?}", s.labels());
    // Copy runs on the selection
    s.click("Copy");
    assert_eq!(s.st.ed.clipboard.get().as_deref(), Some("hello"));
    // outside it: the selection goes, the cursor moves
    s.run("v4l"); // select again (Copy left visual mode)
    let (x, y) = s.cell(1, 1);
    s.run(&format!("<rclick:{x},{y}>"));
    assert_eq!((s.st.ed.mode, s.head()), (Mode::Normal, 13));
}

#[test]
fn caret_selections_and_multiple_cursors() {
    let mut s = Sim::with("alpha beta\ngamma delta\n");
    s.st.ed.set_feature(Feature::Vim, false);
    s.st.process_events();
    assert_eq!(s.st.ed.mode, Mode::Insert);
    // select "alpha"; a right click at its end (the caret boundary) keeps it, as in VS Code
    let (x0, y0) = s.cell(0, 0);
    let (x1, y1) = s.cell(0, 4);
    s.run(&format!("<drag:{},{y0},{},{y1}>", x0 - 4.0, x1 + 4.0));
    assert_eq!(s.selected(), "alpha");
    let (x, y) = s.cell(0, 4);
    s.run(&format!("<rclick:{},{y}><Esc>", x + 4.0));
    assert_eq!(s.selected(), "alpha");
    // past it: a caret where clicked
    let (x, y) = s.cell(0, 7);
    s.run(&format!("<rclick:{},{y}><Esc>", x + 4.0));
    assert_eq!(s.st.ed.buf().sel.primary(), shrew_core::Range::point(8));
    // two cursors (Ctrl+click): a right click on one keeps both, elsewhere leaves one
    let (x, y) = s.cell(1, 0);
    s.run(&format!("<C-click:{},{y}>", x - 4.0));
    assert_eq!(s.st.ed.buf().sel.len(), 2);
    s.run(&format!("<rclick:{},{y}><Esc>", x - 4.0));
    assert_eq!(s.st.ed.buf().sel.len(), 2, "on a cursor: both stay");
    let (x, y) = s.cell(1, 8);
    s.run(&format!("<rclick:{x},{y}><Esc>"));
    assert_eq!(s.st.ed.buf().sel.len(), 1);
}

#[test]
fn insert_mode_stays_in_insert_mode() {
    let mut s = Sim::with("one two\n");
    s.run("i");
    assert_eq!(s.st.ed.mode, Mode::Insert);
    let (x, y) = s.cell(0, 4);
    s.run(&format!("<rclick:{},{y}>", x - 4.0));
    assert_eq!((s.st.ed.mode, s.head()), (Mode::Insert, 4));
    // Paste from the menu types there
    s.st.ed.clipboard.set("X".into());
    s.click("Paste");
    assert_eq!(s.st.ed.buf().rope().to_string(), "one Xtwo\n");
}

// ----- what the menus hold -----

#[test]
fn entries_show_their_shortcuts_and_grey_out_when_they_cannot_act() {
    let mut s = Sim::with("word\n\nmore\n");
    let (x, y) = s.cell(1, 0);
    s.run(&format!("<rclick:{x},{y}>"));
    let m = s.st.menu.as_ref().unwrap();
    let hint = |l: &str| match m.items.iter().find(|e| e.label() == l) {
        Some(MenuItem::Action(a)) => a.hint.clone(),
        _ => None,
    };
    // the platform's modifier names (Option on macOS)
    let alt_f3 = crate::context_menu::chord_label(shrew_core::KeyPress::parse("<A-F3>").unwrap());
    assert_eq!(hint("Select All Occurrences").as_deref(), Some(alt_f3.as_str()));
    // vim's normal mode: Ctrl+C is not Copy there, so the vim keys that do the same
    assert_eq!((hint("Copy Line").as_deref(), hint("Paste").as_deref()), (Some("yy"), Some("P")));
    assert_eq!(hint("Toggle Comment").as_deref(), Some(if cfg!(target_os = "macos") { "Cmd+/" } else { "Ctrl+/" }));
    assert!(!s.enabled("Select All Occurrences"), "an empty line: no word");
    assert!(!s.enabled("Add Next Occurrence"));
    assert!(!s.enabled("Copy Path"), "a scratch buffer has no path");
    assert!(!s.enabled("Copy Relative Path"));
    assert!(s.enabled("Paste") && s.enabled("Select All"));
    assert!(s.item("Find in Project…").is_some(), "no word: the plain entry");
    // keys: j moves, Enter runs; a letter runs its mnemonic (c: Copy Line)
    s.run("<Esc>");
    let (x, y) = s.cell(2, 1);
    s.run(&format!("<rclick:{x},{y}>c"));
    assert!(s.st.menu.is_none());
    assert_eq!(s.st.ed.clipboard.get().as_deref(), Some("more\n"));
}

#[test]
fn shift_f10_opens_the_menu_at_the_cursor_with_the_first_entry_highlighted() {
    let mut s = Sim::with("hello\n");
    s.run("ll<S-F10>");
    let m = s.st.menu.as_ref().expect("opened from the keyboard");
    assert_eq!(m.selected, Some(0));
    let c = s.ui.cursor_rect.unwrap();
    assert_eq!(m.at, (c.x, c.bottom()));
    assert_eq!(s.head(), 2, "the cursor did not move");
    // the keys right after it go to it, frame or not: Down Down Enter runs the third entry (Paste)
    s.run("<Esc>");
    s.st.ed.clipboard.set("Z".into());
    s.run("<S-F10><Down><Down><CR>");
    assert_eq!(s.st.ed.buf().rope().to_string(), "heZllo\n");
}

#[test]
fn the_gutter_menu_acts_on_the_line_clicked() {
    let mut s = Sim::with("first\nsecond\nthird\n");
    let lay = s.frame();
    let (_, y) = s.cell(1, 0);
    let x = lay.text.x + 2.0;
    s.run(&format!("<rclick:{x},{y}>"));
    assert_eq!(s.st.menu.as_ref().unwrap().target, Target::Gutter(1));
    assert_eq!(s.head(), 6, "the cursor went to the line");
    assert!(s.enabled("Relative Line Numbers"));
    s.click("Copy Line");
    assert_eq!(s.st.ed.clipboard.get().as_deref(), Some("second\n"));
    s.run(&format!("<rclick:{x},{y}>"));
    s.click("Select Line");
    assert!(s.selected().starts_with("second"));
    // with the numbers off, relative numbers greys out
    s.run(&format!("<rclick:{x},{y}>"));
    s.click("Line Numbers");
    assert!(!s.st.ed.features.on(Feature::LineNumbers));
    let lay = s.frame();
    s.run(&format!("<rclick:{},{y}>", lay.text.x + 1.0));
    assert!(!s.enabled("Relative Line Numbers"));
}

// ----- the primary selection -----

#[test]
fn selecting_offers_the_primary_selection_and_a_middle_click_pastes_it() {
    let mut s = Sim::with("hello world\nsecond line\n");
    let (x0, y0) = s.cell(0, 0);
    let (x1, y1) = s.cell(0, 4);
    s.run(&format!("<drag:{x0},{y0},{x1},{y1}>"));
    assert_eq!(s.st.ed.clipboard.get_primary().as_deref(), Some("hello"));
    // a keyboard selection too (vim's visual mode)
    s.run("<Esc>wve");
    assert_eq!(s.st.ed.clipboard.get_primary().as_deref(), Some("world"));
    // a middle click pastes it at the pointer, in normal mode, as one undo step
    s.run("<Esc>");
    let (x, y) = s.cell(1, 0);
    s.run(&format!("<mclick:{x},{y}>"));
    assert_eq!(s.st.ed.buf().rope().to_string(), "hello world\nworldsecond line\n");
    assert_eq!(s.st.ed.mode, Mode::Normal);
    s.run("u");
    assert_eq!(s.st.ed.buf().rope().to_string(), "hello world\nsecond line\n");
    // the clipboard is not touched
    assert_ne!(s.st.ed.clipboard.get().as_deref(), Some("world"));
}

#[test]
fn no_primary_selection_with_the_clipboard_off() {
    let mut s = Sim::with("hello\n");
    s.st.ed.set_feature(Feature::Clipboard, false);
    s.run("ve");
    assert_eq!(s.st.ed.clipboard.get_primary(), None);
    s.st.ed.clipboard.set_primary("x".into());
    let (x, y) = s.cell(0, 0);
    s.run(&format!("<Esc><mclick:{x},{y}>"));
    assert_eq!(s.st.ed.buf().rope().to_string(), "hello\n", "a middle click does nothing");
}

#[test]
fn huge_selections_stay_off_the_primary_selection() {
    let line = "x".repeat(99) + "\n";
    let mut s = Sim::with(&line.repeat(super::primary::MAX_CHARS / 100 + 10));
    s.run(":select_all<CR>");
    assert!(s.st.ed.mode.is_visual());
    assert_eq!(s.st.ed.clipboard.get_primary(), None);
    s.run("<Esc>v");
    assert_eq!(s.st.ed.clipboard.get_primary().as_deref(), Some("x"));
}

// ----- the terminal panel -----

#[cfg(all(unix, feature = "terminal"))]
#[test]
fn the_terminal_panel_has_its_own_menu() {
    use std::time::Duration;
    let mut s = Sim::with("text\n");
    s.st.terminal_command(shrew_core::Command::NewTerminal, Some("printf 'one two'; sleep 5".into()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && s.st.terminals_busy() {
        s.st.poll();
        s.frame();
        std::thread::sleep(Duration::from_millis(5));
    }
    s.frame();
    let g = s.st.terminals.geom.expect("the panel is drawn");
    let (x, y) = (g.x0 + 3.5 * g.cw, g.y0 + 0.5 * g.ch);
    s.run(&format!("<rclick:{x},{y}>"));
    assert_eq!(s.st.menu.as_ref().map(|m| m.target), Some(Target::Terminal), "{:?}", s.st.ed.message);
    assert!(!s.enabled("Copy"), "nothing selected");
    assert!(s.enabled("Paste") && s.enabled("Kill Terminal"));
    s.click("Select All");
    s.run(&format!("<rclick:{x},{y}>"));
    assert!(s.enabled("Copy"));
    s.click("Copy");
    assert!(s.st.ed.clipboard.get().unwrap_or_default().contains("one two"));
    s.run(&format!("<rclick:{x},{y}>"));
    s.click("Close Panel");
    assert!(!s.st.terminals.visible);
    s.st.terminals.close_all();
    // a program that reads the mouse gets the right button; Shift+right click opens the menu anyway
    s.st.terminal_command(shrew_core::Command::NewTerminal, Some("printf '\\033[?1000hready'; sleep 5".into()));
    let deadline = Instant::now() + Duration::from_secs(10);
    let reads_mouse = |s: &Sim| {
        let t = &s.st.terminals;
        t.tabs.first().is_some_and(|t| {
            t.session.term.lock().mode().contains(alacritty_terminal::term::TermMode::MOUSE_REPORT_CLICK)
        })
    };
    while Instant::now() < deadline && !reads_mouse(&s) {
        s.st.poll();
        s.frame();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(reads_mouse(&s));
    s.run(&format!("<rclick:{x},{y}>"));
    assert!(s.st.menu.is_none(), "the program took it");
    s.run(&format!("<S-rclick:{x},{y}>"));
    assert_eq!(s.st.menu.as_ref().map(|m| m.target), Some(Target::Terminal));
    s.run("<Esc>");
    s.st.terminals.close_all();
}
