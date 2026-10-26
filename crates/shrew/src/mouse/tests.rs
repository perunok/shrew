//! The mouse driven the way the window drives it: a frame is laid out and drawn (on the CPU, no renderer) before
//! each event, so hit rects are the ones a person would see.

use super::*;
use crate::titlebar::WindowAction;
use shrew_core::{Command, Document, Feature};
use shrew_render::{Metrics, Rect, Scene};
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
    fn new() -> Sim {
        let mut ui = Ui::new();
        ui.animate = false;
        let st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
        Sim { st, ui, scene: Scene::new(), mouse: Mouse::new(), now: Instant::now(), size: (1000, 600) }
    }

    /// One scratch buffer per text, the last one active.
    fn with(texts: &[&str]) -> Sim {
        let mut s = Sim::new();
        for t in texts {
            s.st.ed.install_document(Document::from_str(t));
        }
        s.st.process_events();
        s.frame();
        s
    }

    /// Lays out and draws a frame, as the window does between events.
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

    /// The pixel at the middle of char `col` of line `line` (no scroll).
    fn cell(&mut self, line: usize, col: usize) -> (f32, f32) {
        let lay = self.frame();
        (lay.text_x() + (col as f32 + 0.5) * lay.cw, lay.text.y + (line as f32 + 0.5) * lay.ch)
    }

    fn selected(&self) -> String {
        let b = self.st.ed.buf();
        let r = b.sel.primary();
        let to = if self.st.ed.mode.is_visual() { r.to() + 1 } else { r.to() };
        b.rope().slice(r.from()..to.min(b.rope().len_chars())).to_string()
    }
}

fn mid(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// A point on the empty part of the title bar, right of the tabs.
fn empty_bar(s: &mut Sim) -> (f32, f32) {
    s.frame();
    let tabs = s.st.titlebar.placed.iter().find(|p| p.0 == crate::titlebar::items::Item::Tabs).unwrap().1;
    (tabs.right() - 20.0, tabs.y + tabs.h / 2.0)
}

#[test]
fn tokens_parse_into_events() {
    let i = parse_input("ix<C-click:10,20><Esc><wheel:5,6,2><drag:0,0,80,8>").unwrap();
    assert_eq!(i[0], Input::Key(KeyPress::ch('i')));
    assert_eq!(i[1], Input::Key(KeyPress::ch('x')));
    let Input::Mouse(e) = &i[2] else { panic!() };
    assert_eq!((e.kind, e.x, e.y, e.mods.ctrl), (Kind::Move, 10.0, 20.0, true));
    assert!(matches!(i[3], Input::Mouse(Event { kind: Kind::Press(MouseButton::Left), .. })));
    assert!(matches!(i[4], Input::Mouse(Event { kind: Kind::Release(MouseButton::Left), .. })));
    assert_eq!(i[5], Input::Key(KeyPress::plain(Key::Esc)));
    assert!(matches!(i[7], Input::Mouse(Event { kind: Kind::Wheel(0.0, 2.0), .. })));
    let Input::Mouse(last) = i.last().unwrap() else { panic!() };
    assert_eq!((last.kind, last.x, last.y), (Kind::Release(MouseButton::Left), 80.0, 8.0));
    // keys with angle brackets stay keys; a broken mouse token is an error
    assert_eq!(parse_input("<C-p>a<lt>").unwrap().len(), 3);
    assert!(parse_input("<click:1>").is_err());
    assert!(parse_input("<click:a,b>").is_err());
    let d = parse_input("<dclick:1,2>").unwrap();
    assert_eq!(d.iter().filter(|e| matches!(e, Input::Mouse(Event { kind: Kind::Press(_), .. }))).count(), 2);
}

#[test]
fn clicking_a_tab_switches_to_it() {
    let mut s = Sim::with(&["one\n", "two\n", "three\n"]);
    assert_eq!(s.st.ed.active, 2);
    let tab = s.ui.hits.tabs.iter().find(|t| t.1 == 0).unwrap().0;
    let (x, y) = mid(tab);
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!(s.st.ed.active, 0);
    let lay = s.frame();
    assert_eq!(target_at(&s.st, &s.ui, &lay, x, y), Target::Tab(0));
}

#[test]
fn dragging_selects_text_and_double_click_a_word() {
    let mut s = Sim::with(&["hello world\nsecond line\n"]);
    let (x0, y0) = s.cell(0, 0);
    let (x1, y1) = s.cell(0, 4);
    s.run(&format!("<drag:{x0},{y0},{x1},{y1}>"));
    assert!(s.st.ed.mode.is_visual(), "a drag selects in visual mode");
    assert_eq!(s.selected(), "hello");
    let (x, y) = s.cell(1, 2);
    s.run(&format!("<dclick:{x},{y}>"));
    assert_eq!(s.selected(), "second");
    s.run(&format!("<tclick:{x},{y}>"));
    assert!(s.selected().starts_with("second line"));
    // a click puts the cursor on the char clicked, in normal mode
    let (x, y) = s.cell(0, 6);
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!((s.st.ed.mode, s.st.ed.buf().sel.primary().head), (Mode::Normal, 6));
    let (x, y) = s.cell(1, 3);
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!(s.st.ed.buf().sel.primary().head, 12 + 3);
}

#[test]
fn scrollbar_drag_and_wheel_scroll() {
    let text: String = (0..500).map(|i| format!("line {i}\n")).collect();
    let mut s = Sim::with(&[&text]);
    s.run("<wheel:300,200,2>");
    assert_eq!(s.st.ed.buf().view.top, 6, "two notches, three lines each");
    s.run("<wheel:300,200,-1>");
    assert_eq!(s.st.ed.buf().view.top, 3);
    let lay = s.frame();
    let (track, _) = s.ui.hits.scrollbar.unwrap();
    // (its right half is the window's resize border)
    let x = track.x + 2.0;
    s.run(&format!("<drag:{x},{},{x},{}>", track.y + 2.0, track.y + track.h / 2.0));
    let top = s.st.ed.buf().view.top;
    assert!((200..300).contains(&top), "the thumb follows the pointer to the middle: {top}");
    assert_eq!(s.st.ed.mode, Mode::Normal, "nothing was selected");
    assert_eq!(target_at(&s.st, &s.ui, &lay, x, track.y + 5.0), Target::Scrollbar);
    // Ctrl+wheel zooms
    let size = s.st.font.size;
    s.run("<C-wheel:300,200,-1>");
    assert_eq!(s.st.font.size, size + 1.0);
}

#[test]
fn targets_under_the_pointer() {
    let mut s = Sim::with(&["fn main() {}\n", "two\n"]);
    let lay = s.frame();
    let (x, y) = s.cell(0, 3);
    assert_eq!(target_at(&s.st, &s.ui, &lay, x, y), Target::Text(3));
    assert_eq!(target_at(&s.st, &s.ui, &lay, lay.text.x + 2.0, y), Target::Gutter(0));
    let (x, y) = mid(lay.status);
    assert_eq!(target_at(&s.st, &s.ui, &lay, x, y), Target::StatusBar);
    let (x, y) = empty_bar(&mut s);
    assert_eq!(target_at(&s.st, &s.ui, &lay, x, y), Target::TitleBar);
    // under a modal panel: nothing
    s.run(":about<CR>");
    let (x, y) = s.cell(0, 3);
    assert_eq!(target_at(&s.st, &s.ui, &lay, x, y), Target::Nothing);
}

#[test]
fn right_click_on_the_title_bar_opens_its_menu_and_closes_it() {
    let mut s = Sim::new();
    let (x, y) = empty_bar(&mut s);
    s.run(&format!("<rclick:{x},{y}>"));
    let m = s.st.menu.as_ref().expect("the bar's menu");
    assert_eq!((m.at, m.target, m.selected), ((x, y), Target::TitleBar, None));
    assert!(m.items.iter().any(|e| e.label() == "Use System Title Bar"));
    // a second right click on the same spot closes it
    s.run(&format!("<rclick:{x},{y}>"));
    assert!(s.st.menu.is_none());
    // a right click elsewhere on the bar moves it there
    s.run(&format!("<rclick:{x},{y}><rclick:{},{y}>", x - 60.0));
    assert_eq!(s.st.menu.as_ref().unwrap().at, (x - 60.0, y));
    // a left click outside closes it and goes no further: the cursor stays
    let (cx, cy) = s.cell(0, 0);
    s.run(&format!("<click:{cx},{cy}>"));
    assert!(s.st.menu.is_none());
    // the middle button closes it too
    s.run(&format!("<rclick:{x},{y}><mclick:{cx},{cy}>"));
    assert!(s.st.menu.is_none());
    // the project name has the bar's menu too
    let project = s.st.titlebar.placed.iter().find(|p| p.0 == crate::titlebar::items::Item::Project).map(|p| mid(p.1));
    if let Some((px, py)) = project {
        s.run(&format!("<rclick:{px},{py}>"));
        assert_eq!(s.st.menu.as_ref().map(|m| m.target), Some(Target::TitleBar));
        s.run("<Esc>");
    }
    // a click on a window button opens nothing
    let close = s.st.titlebar.placed.iter().find(|p| p.0 == crate::titlebar::items::Item::Close).map(|p| mid(p.1));
    if let Some((bx, by)) = close {
        s.run(&format!("<rclick:{bx},{by}>"));
        assert!(s.st.menu.is_none());
    }
}

#[test]
fn menu_items_run_on_click_and_on_release_after_a_drag() {
    let mut s = Sim::new();
    let (x, y) = empty_bar(&mut s);
    // press, hold, release on the spot: the menu stays open
    s.run(&format!("<rdown:{x},{y}><rup:{x},{y}>"));
    assert!(s.st.menu.is_some());
    s.run("<Esc>");
    assert!(s.st.menu.is_none(), "Esc closes it");
    // press the right button, drag onto "Use System Title Bar", release: it runs
    s.run(&format!("<rdown:{x},{y}>"));
    s.frame();
    let m = s.st.menu.as_ref().unwrap();
    let row = m.hits[0].rows.iter().find(|r| m.items[r.1].label() == "Use System Title Bar").unwrap().0;
    let (ix, iy) = mid(row);
    assert!(s.st.ed.features.on(Feature::CustomTitlebar));
    s.run(&format!("<move:{},{}><move:{ix},{iy}><rup:{ix},{iy}>", x + 10.0, y + 10.0));
    assert!(s.st.menu.is_none());
    assert!(!s.st.ed.features.on(Feature::CustomTitlebar), "the item ran");
    // a plain click on an item runs it too
    s.st.ed.features.set(Feature::CustomTitlebar, true);
    let (x, y) = empty_bar(&mut s);
    s.run(&format!("<rclick:{x},{y}>"));
    s.frame();
    let m = s.st.menu.as_ref().unwrap();
    let row = m.hits[0].rows.iter().find(|r| m.items[r.1].label() == "Use System Title Bar").unwrap().0;
    let (ix, iy) = mid(row);
    s.run(&format!("<move:{ix},{iy}>"));
    assert_eq!(s.st.menu.as_ref().unwrap().selected, Some(2), "hovering highlights");
    assert_eq!(s.mouse.icon(), CursorIcon::Default, "an arrow over the menu");
    s.run(&format!("<click:{ix},{iy}>"));
    assert!(!s.st.ed.features.on(Feature::CustomTitlebar));
}

#[test]
fn menu_keys_are_modal() {
    let mut s = Sim::with(&["text\n"]);
    let (x, y) = empty_bar(&mut s);
    s.run(&format!("<rclick:{x},{y}>xdd"));
    assert_eq!(s.st.ed.buf().rope().to_string(), "text\n", "keys went to the menu");
    assert!(s.st.menu.is_some());
    // the title bar's menu and a context menu are never open together
    s.run("<F10>");
    assert!(s.st.menu.is_none() && s.st.titlebar.menu.is_none(), "F10 is the menu's: it closes it");
    s.run("<F10>");
    assert!(s.st.titlebar.menu.is_some());
    s.run(&format!("<rclick:{x},{y}>"));
    assert!(s.st.titlebar.menu.is_none(), "a right click closes the menu bar");
}

#[test]
fn the_empty_part_of_the_bar_moves_and_maximizes_the_window() {
    let mut s = Sim::new();
    let (x, y) = empty_bar(&mut s);
    s.run(&format!("<drag:{x},{y},{},{y}>", x - 40.0));
    assert_eq!(s.st.titlebar.actions, vec![WindowAction::Drag]);
    s.st.titlebar.actions.clear();
    s.run(&format!("<dclick:{x},{y}>"));
    assert_eq!(s.st.titlebar.actions, vec![WindowAction::ToggleMaximize]);
    s.st.titlebar.actions.clear();
    // the window's bottom edge resizes it
    s.run("<down:500,599><up:500,599>");
    assert!(matches!(s.st.titlebar.actions.as_slice(), [WindowAction::Resize(_)]) || cfg!(target_os = "macos"));
}

#[test]
fn a_menu_for_a_target_with_app_actions() {
    use crate::context_menu::{Action, ContextMenu, MenuItem, Submenu, open};
    fn hello(st: &mut State, t: &Target, arg: usize) {
        st.ed.info(format!("{t:?} {arg}"));
    }
    let mut s = Sim::with(&["text\n"]);
    let items: Vec<MenuItem> = vec![
        Action::command(&s.st.ed, Command::SelectAll).into(),
        MenuItem::Separator,
        Submenu::new("More", vec![Action::app_with("Hello", hello, 7).into()]).into(),
    ];
    open(&mut s.st, ContextMenu::new(Target::Text(2), items), (100.0, 100.0));
    s.run("<Down><Down><Right><CR>");
    assert!(s.st.menu.is_none());
    assert_eq!(s.st.ed.message.as_ref().map(|m| m.0.as_str()), Some("Text(2) 7"));
}

// ----- updates: the notification and the Update shrew dialog are overlays -----

#[cfg(feature = "update")]
fn note(s: &mut Sim) -> crate::toast::Hits {
    s.st.updates.toast = Some(crate::updates::Toast::Note {
        title: "shrew is up to date".into(),
        body: "0.2.0 is the latest release.".into(),
        error: false,
        link: Some(("What's new", "https://example.com".into())),
        retry: false,
    });
    s.frame();
    s.st.updates.hits.toast.clone()
}

#[cfg(feature = "update")]
#[test]
fn the_update_notification_takes_the_pointer() {
    use crate::toast::Hot;
    let text: String = (0..60).map(|i| format!("a line of text number {i:<40}\n")).collect();
    let mut s = Sim::with(&[&text]);
    let hits = note(&mut s);
    let area = hits.area.expect("drawn");
    let lay = s.frame();
    assert!(lay.text.contains(area.x + 4.0, area.y + 4.0), "it covers the text");
    // over its body: an arrow, nothing under it is a target
    let (x, y) = (area.x + 4.0, area.y + 4.0);
    s.run(&format!("<move:{x},{y}>"));
    assert_eq!(s.mouse.icon(), CursorIcon::Default);
    assert_eq!(target_at(&s.st, &s.ui, &lay, x, y), Target::Nothing);
    // a right or middle click there opens nothing and changes nothing
    s.run(&format!("<rclick:{x},{y}><mclick:{x},{y}>"));
    assert!(s.st.menu.is_none());
    assert_eq!(s.st.ed.mode, Mode::Normal);
    // its button lights up under the pointer, with a hand
    let (bx, by) = mid(hits.buttons[0]);
    s.run(&format!("<move:{bx},{by}>"));
    assert_eq!((s.st.updates.hot, s.mouse.icon()), (Hot::Button(0), CursorIcon::Pointer));
    let (cx, cy) = mid(hits.close.unwrap());
    s.run(&format!("<move:{cx},{cy}>"));
    assert_eq!(s.st.updates.hot, Hot::Close);
    // off it: nothing lit, the text's I-beam
    let (tx, ty) = s.cell(0, 2);
    s.run(&format!("<move:{tx},{ty}>"));
    assert_eq!((s.st.updates.hot, s.mouse.icon()), (Hot::None, CursorIcon::Text));
    // ✕ closes it
    s.run(&format!("<click:{cx},{cy}>"));
    assert!(s.st.updates.toast.is_none());
}

#[cfg(feature = "update")]
#[test]
fn the_update_dialog_is_modal_for_the_mouse() {
    let mut s = Sim::with(&["fn main() {}\n"]);
    // a context menu is open: the dialog closes it (one overlay at a time)
    let (x, y) = s.cell(0, 3);
    s.run(&format!("<rclick:{x},{y}>"));
    assert!(s.st.menu.is_some());
    crate::updates::open_dialog(&mut s.st);
    assert!(s.st.menu.is_none());
    let lay = s.frame();
    // under it: no target, no menu, no middle-click paste, an arrow
    assert_eq!(target_at(&s.st, &s.ui, &lay, x, y), Target::Nothing);
    s.run(&format!("<rclick:{x},{y}><mclick:{x},{y}>"));
    assert!(s.st.menu.is_none());
    assert_eq!(s.st.ed.buf().doc.rope.to_string(), "fn main() {}\n");
    let (sx, sy) = mid(lay.status);
    s.run(&format!("<rclick:{sx},{sy}>"));
    assert!(s.st.menu.is_none(), "not even the status bar's menu");
    assert!(s.st.updates.dialog.is_some(), "a right click leaves it open");
    s.run(&format!("<move:{x},{y}>"));
    assert_eq!(s.mouse.icon(), CursorIcon::Default, "no I-beam over the text under it");
    // its buttons light up with a hand
    let (r, b) = s.st.updates.hits.buttons[0];
    let (bx, by) = mid(r);
    s.run(&format!("<move:{bx},{by}>"));
    assert_eq!((s.st.updates.dialog_hot, s.mouse.icon()), (Some(b), CursorIcon::Pointer));
    // a click outside it closes it, and goes no further
    // the title bar still works over it, and what it opens closes the dialog
    let menu = s.st.titlebar.placed.iter().find(|p| p.0 == crate::titlebar::items::Item::Menu);
    let (mx, my) = mid(menu.expect("the menu button").1);
    s.run(&format!("<click:{mx},{my}>"));
    assert!(s.st.titlebar.menu.is_some() && s.st.updates.dialog.is_none());
    s.run("<Esc>");
    crate::updates::open_dialog(&mut s.st);
    s.frame();
    let head = s.st.ed.buf().sel.primary().head;
    let (tx, ty) = s.cell(0, 8);
    s.run(&format!("<click:{tx},{ty}>"));
    assert!(s.st.updates.dialog.is_none());
    assert_eq!(s.st.ed.buf().sel.primary().head, head, "the click did not move the cursor");
}

#[cfg(feature = "update")]
#[test]
fn the_status_bar_menu_checks_for_updates() {
    let mut s = Sim::with(&["text\n"]);
    let lay = s.frame();
    let (x, y) = (lay.status.x + 40.0, lay.status.y + lay.status.h / 2.0);
    s.run(&format!("<rclick:{x},{y}>"));
    let m = s.st.menu.as_ref().expect("the status bar's menu");
    let labels: Vec<String> = m.items.iter().map(|e| e.label().to_string()).collect();
    assert!(labels.iter().any(|l| l == "Check for Updates…"), "{labels:?}");
    assert!(!labels.iter().any(|l| l == "Update shrew…"), "no update known: {labels:?}");
}
// -----
