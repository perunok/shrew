//! The tab row driven as a person would: a frame is laid out and drawn (on the CPU) before each mouse event.

use super::*;
use crate::mouse::{Input, Mouse, parse_input, target_at};
use shrew_core::Document;
use shrew_render::Metrics;
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
    /// Scratch buffers titled `names` (the last one active); `dirty` ones have unsaved changes.
    fn with(names: &[&str], dirty: &[&str]) -> Sim {
        let mut ui = Ui::new();
        ui.animate = false;
        let st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
        let mut s = Sim { st, ui, scene: Scene::new(), mouse: Mouse::new(), now: Instant::now(), size: (1000, 600) };
        for n in names {
            let mut d = Document::from_str(&format!("{n}\n"));
            d.title = Some((*n).to_string());
            d.dirty = dirty.contains(n);
            s.st.ed.install_document(d);
        }
        s.st.process_events();
        s.frame();
        s
    }

    /// Files at `paths` (they need not exist).
    fn files(paths: &[&str]) -> Sim {
        let mut s = Sim::with(&[], &[]);
        for p in paths {
            let mut d = Document::from_str("x\n");
            d.path = Some(PathBuf::from(p));
            s.st.ed.install_document(d);
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

    fn names(&self) -> Vec<String> {
        self.st.ed.buffers.iter().map(|b| b.doc.display_name()).collect()
    }

    fn active(&self) -> String {
        self.st.ed.buf().doc.display_name()
    }

    /// The middle of tab `i` as last drawn.
    fn tab(&self, i: usize) -> (f32, f32) {
        let r = self.st.tabs.placed.iter().find(|p| p.index == i).expect("tab on screen").rect;
        (r.x + r.w / 2.0 - 15.0, r.y + r.h / 2.0)
    }

    fn close_of(&self, i: usize) -> (f32, f32) {
        let r = self.st.tabs.placed.iter().find(|p| p.index == i).expect("tab on screen").close;
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }
}

fn many(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("file_{i:02}.rs")).collect()
}

#[test]
fn the_active_tab_is_always_whole_and_hidden_tabs_have_no_hit_rects() {
    let names = many(15);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    for custom in [true, false] {
        let mut s = Sim::with(&refs, &[]);
        s.st.ed.features.set(Feature::CustomTitlebar, custom);
        for active in [14, 0, 7, 13, 1, 14] {
            s.st.ed.switch_to(active);
            s.frame();
            let view = s.st.tabs.view;
            let p = s.st.tabs.placed.iter().find(|p| p.index == active).expect("the active tab is drawn");
            assert!(
                p.rect.x >= view.x && p.rect.right() <= view.right(),
                "custom {custom}, tab {active}: {:?} in {view:?}",
                p.rect
            );
            assert!(s.st.tabs.list.is_some(), "15 tabs overflow: the list button shows");
            // every hit rect is inside the visible strip, and not every tab has one
            assert!(s.ui.hits.tabs.iter().all(|(r, _)| r.x >= view.x && r.right() <= view.right() && r.w > 0.0));
            assert!(s.ui.hits.tabs.len() < 15);
        }
        // the title bar's items are never under a tab
        let lay = s.frame();
        for (item, r) in &s.st.titlebar.placed {
            if *item == crate::titlebar::items::Item::Tabs {
                continue;
            }
            let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            assert!(!s.ui.hits.tabs.iter().any(|(t, _)| t.contains(x, y)), "{item:?} under a tab");
            assert!(!matches!(target_at(&s.st, &s.ui, &lay, x, y), Target::Tab(_)));
        }
    }
}

#[test]
fn layout_math() {
    // gaps count: the last of n tabs ends where the widths and gaps add up
    let mut lefts = Vec::new();
    let w = offsets(&[100.0, 50.0, 70.0], 4.0, 2.0, &mut lefts);
    assert_eq!(lefts, [4.0, 106.0, 158.0]);
    assert_eq!(w, 4.0 + 220.0 + 4.0 + 4.0);
    // reveal: scrolled just enough, both ways, within bounds
    assert_eq!(reveal(0.0, 500.0, 600.0, 400.0, 10.0, 1000.0), 210.0);
    assert_eq!(reveal(300.0, 100.0, 200.0, 400.0, 10.0, 1000.0), 90.0);
    assert_eq!(reveal(100.0, 150.0, 200.0, 400.0, 10.0, 1000.0), 100.0, "in view: nothing moves");
    assert_eq!(reveal(0.0, 0.0, 50.0, 400.0, 10.0, 1000.0), 0.0);
    // widths: a minimum, a maximum
    assert_eq!(tab_width(1, 10.0), tab_width(MIN_LABEL, 10.0));
    assert_eq!(tab_width(200, 10.0), tab_width(MAX_LABEL, 10.0));
    assert!(tab_width(10, 10.0) > tab_width(MIN_LABEL, 10.0));
}

#[test]
fn namesakes_get_the_shortest_parent_path_that_tells_them_apart() {
    let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let p = |s: &'static str| Some(Path::new(s));
    let names = n(&["mod.rs", "mod.rs", "main.rs", "mod.rs"]);
    let files =
        [p("/r/src/git/log/mod.rs"), p("/r/src/git/diffview/mod.rs"), p("/r/src/main.rs"), p("/r/x/log/mod.rs")];
    let d = disambiguate(&names, &files);
    assert_eq!(d, ["git/log", "diffview", "", "x/log"]);
    // a diff tab or a scratch buffer of the same name is not a file to tell apart
    let names = n(&["a.rs", "a.rs"]);
    assert_eq!(disambiguate(&names, &[p("/r/a.rs"), None]), ["", ""]);
    // the same file twice (it cannot be, but nothing loops)
    assert_eq!(disambiguate(&names, &[p("/r/a.rs"), p("/r/a.rs")]), ["", ""]);
    // drawn with the name
    let mut s = Sim::files(&["/r/git/log/mod.rs", "/r/git/diffview/mod.rs", "/r/main.rs"]);
    s.frame();
    let dirs: Vec<&str> = s.st.tabs.labels.iter().map(|l| l.dir.as_str()).collect();
    assert_eq!(dirs, ["log", "diffview", ""]);
}

#[test]
fn a_long_name_is_cut() {
    let long = "a_very_long_file_name_that_goes_on_and_on.rs";
    let s = Sim::with(&[long, "b"], &[]);
    let l = &s.st.tabs.labels[0];
    assert_eq!(columns(&l.name), MAX_LABEL);
    assert!(l.name.ends_with('…'));
}

#[test]
fn press_switches_and_middle_click_closes() {
    let mut s = Sim::with(&["a", "b", "c"], &[]);
    let (x, y) = s.tab(0);
    s.run(&format!("<down:{x},{y}>"));
    assert_eq!(s.active(), "a", "a tab switches on press");
    s.run(&format!("<up:{x},{y}>"));
    let (x, y) = s.tab(1);
    s.run(&format!("<mclick:{x},{y}>"));
    assert_eq!(s.names(), ["a", "c"]);
    assert_eq!(s.active(), "a", "closing another tab leaves the active one");
}

#[test]
fn the_close_button_closes_and_an_unsaved_tab_asks() {
    let mut s = Sim::with(&["a", "b", "c"], &["b"]);
    // the active tab's ×
    let (x, y) = s.close_of(2);
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!(s.names(), ["a", "b"]);
    // b is unsaved: its dot is a × under the pointer, and closing it asks
    let (x, y) = s.close_of(1);
    s.run(&format!("<move:{x},{y}>"));
    assert_eq!(s.st.tabs.hover, Some(Hover::Tab(1, true)));
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!(s.names(), ["a", "b"]);
    assert!(s.st.ed.prompt.as_ref().is_some_and(|p| p.label.contains("Save changes to b")));
    s.run("c");
    assert_eq!(s.names(), ["a", "b"], "cancelled");
    s.run(&format!("<mclick:{x},{y}>d"));
    assert_eq!(s.names(), ["a"], "discarded");
    // pressed on × but released elsewhere: nothing closes
    let mut s = Sim::with(&["a", "b"], &[]);
    let (x, y) = s.close_of(1);
    s.run(&format!("<down:{x},{y}><up:{},{y}>", x - 60.0));
    assert_eq!(s.names(), ["a", "b"]);
}

#[test]
fn dragging_a_tab_reorders_and_esc_puts_it_back() {
    let mut s = Sim::with(&["a", "b", "c", "d"], &[]);
    let (x0, y) = s.tab(0);
    let (x2, _) = s.tab(2);
    s.run(&format!("<down:{x0},{y}><move:{},{y}><move:{},{y}>", x0 + 20.0, x2 + 20.0));
    assert!(s.st.tabs.dragging().is_some(), "a drag in progress");
    s.run(&format!("<up:{},{y}>", x2 + 20.0));
    assert_eq!(s.names(), ["b", "c", "a", "d"]);
    assert_eq!(s.active(), "a", "the dragged tab was pressed: it is active");
    assert!(s.st.tabs.dragging().is_none());
    // to the end
    let (x, y) = s.tab(0);
    s.run(&format!("<drag:{x},{y},900,{y}>"));
    assert_eq!(s.names(), ["c", "a", "d", "b"]);
    // Esc while dragging: back where it was
    let (x, y) = s.tab(3);
    s.run(&format!("<down:{x},{y}><move:{},{y}><move:20,{y}><Esc><up:20,{y}>", x - 10.0));
    assert_eq!(s.names(), ["c", "a", "d", "b"]);
    // a click without moving reorders nothing
    let (x, y) = s.tab(1);
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!(s.names(), ["c", "a", "d", "b"]);
}

#[test]
fn the_wheel_scrolls_the_strip_and_the_list_button_lists_buffers() {
    let names = many(15);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut s = Sim::with(&refs, &[]);
    s.st.ed.switch_to(0);
    s.frame();
    assert_eq!(s.st.tabs.offset, 0.0);
    let y = s.st.tabs.view.y + s.st.tabs.view.h / 2.0;
    let x = s.st.tabs.view.x + 100.0;
    let top = s.st.ed.buf().view.top;
    s.run(&format!("<wheel:{x},{y},2>"));
    assert!(s.st.tabs.offset > 0.0, "down scrolls on along the row");
    assert_eq!(s.st.ed.buf().view.top, top, "the text did not scroll");
    let off = s.st.tabs.offset;
    s.run(&format!("<wheel:{x},{y},0,-1>"));
    assert!(s.st.tabs.offset < off, "sideways too");
    s.run(&format!("<wheel:{x},{y},-50>"));
    assert_eq!(s.st.tabs.offset, 0.0, "not past the start");
    // the list button opens the buffer list
    let b = s.st.tabs.list.unwrap();
    s.run(&format!("<click:{},{}>", b.x + b.w / 2.0, b.y + b.h / 2.0));
    assert!(s.st.ed.picker.is_some());
    // few tabs: no button, the wheel does nothing
    let mut s = Sim::with(&["a", "b"], &[]);
    assert!(s.st.tabs.list.is_none());
    let y = s.st.tabs.view.y + 10.0;
    s.run(&format!("<wheel:300,{y},3>"));
    assert_eq!(s.st.tabs.offset, 0.0);
}

#[test]
fn the_empty_row_double_click_opens_a_scratch_buffer() {
    let mut s = Sim::with(&["a", "b"], &[]);
    s.st.ed.features.set(Feature::CustomTitlebar, false);
    let lay = s.frame();
    assert!(!lay.top_bar && lay.tabs.h > 0.0);
    let (x, y) = (lay.tabs.right() - 50.0, lay.tabs.y + lay.tabs.h / 2.0);
    assert_eq!(target_at(&s.st, &s.ui, &lay, x, y), Target::TabStrip);
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!(s.names().len(), 2, "one click does nothing");
    s.run(&format!("<dclick:{x},{y}>"));
    assert_eq!(s.names(), ["a", "b", "[scratch]"]);
    // its right-click menu
    s.run(&format!("<rclick:{x},{y}>"));
    let m = s.st.menu.as_ref().unwrap();
    assert!(m.items.iter().any(|e| e.label() == "New Scratch Buffer"));
}

#[test]
fn the_title_bar_keeps_its_empty_part_for_the_window() {
    use crate::titlebar::WindowAction;
    let mut s = Sim::with(&["a", "b"], &[]);
    let lay = s.frame();
    let tabs = s.st.titlebar.placed.iter().find(|p| p.0 == crate::titlebar::items::Item::Tabs).unwrap().1;
    let (x, y) = (tabs.right() - 30.0, tabs.y + tabs.h / 2.0);
    assert_eq!(target_at(&s.st, &s.ui, &lay, x, y), Target::TitleBar);
    s.run(&format!("<click:{x},{y}>"));
    assert!(s.st.titlebar.actions.is_empty(), "a plain click does nothing");
    s.run(&format!("<drag:{x},{y},{},{y}>", x - 50.0));
    assert_eq!(s.st.titlebar.actions, vec![WindowAction::Drag]);
    s.st.titlebar.actions.clear();
    s.run(&format!("<dclick:{x},{y}>"));
    assert_eq!(s.st.titlebar.actions, vec![WindowAction::ToggleMaximize]);
    assert_eq!(s.names().len(), 2, "no scratch buffer from the title bar");
}

#[test]
fn the_tab_menu_acts_on_the_tab_clicked() {
    let mut s = Sim::with(&["a", "b", "c", "d"], &[]);
    let (x, y) = s.tab(1);
    s.run(&format!("<rclick:{x},{y}>"));
    let m = s.st.menu.as_ref().expect("the tab's menu");
    assert_eq!(m.target, Target::Tab(1));
    let labels: Vec<&str> = m.items.iter().map(MenuItem::label).collect();
    for l in ["Close", "Close Others", "Close to the Right", "Close Saved", "Close All", "Copy Path", "Reopen Closed"] {
        assert!(labels.contains(&l), "{l} in {labels:?}");
    }
    // commands show their chords (vim mode is on: Ctrl+K chords are Sublime's)
    let hint = |m: &ContextMenu, l: &str| match m.items.iter().find(|e| e.label() == l) {
        Some(MenuItem::Action(a)) => a.hint.clone(),
        _ => None,
    };
    assert_eq!(hint(m, "Reopen Closed").as_deref(), Some("Ctrl+Shift+T"));
    // a scratch tab has no path to copy
    assert!(m.items.iter().any(|e| e.label() == "Copy Path" && !e.enabled()));
    // Close to the Right of b
    s.run("<Down><Down><Down><CR>");
    assert_eq!(s.names(), ["a", "b"]);
    // Close Others of a, from the menu by its mnemonic-free type-ahead
    let (x, y) = s.tab(0);
    s.run(&format!("<rclick:{x},{y}>"));
    let m = s.st.menu.as_ref().unwrap();
    let row = m.hits[0].rows.iter().find(|r| m.items[r.1].label() == "Close Others").unwrap().0;
    s.run(&format!("<click:{},{}>", row.x + 10.0, row.y + row.h / 2.0));
    assert_eq!(s.names(), ["a"]);
    // outside vim the chords are Sublime's
    s.run(":set novim<CR>");
    let m = menu(&s.st, 0).unwrap();
    assert_eq!(hint(&m, "Close").as_deref(), Some("Ctrl+W"));
    assert_eq!(hint(&m, "Close All").as_deref(), Some("Ctrl+K W"));
}

#[test]
fn reveal_and_copy_from_the_menu() {
    let mut s = Sim::files(&["/tmp/shrew-tabs-menu/a.rs", "/tmp/shrew-tabs-menu/b.rs"]);
    let (x, y) = s.tab(0);
    s.run(&format!("<rclick:{x},{y}>"));
    let m = s.st.menu.as_ref().unwrap();
    let i = m.items.iter().position(|e| e.label() == "Copy Path").unwrap();
    let Some(MenuItem::Action(a)) = m.items.get(i) else { panic!() };
    let run = a.run.clone();
    crate::context_menu::run(&mut s.st, &Target::Tab(0), run);
    let want = Path::new("/tmp/shrew-tabs-menu/a.rs").display().to_string();
    assert_eq!(
        s.st.ed.clipboard.get().as_deref(),
        Some(want.as_str()),
        "the right-clicked tab's, not the active one's"
    );
}
