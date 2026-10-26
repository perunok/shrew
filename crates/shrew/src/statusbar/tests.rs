//! The status bar's items under the mouse: hover, clicks and the right-click menu.

use super::*;
use crate::mouse::Mouse;
use crate::sidebar::tests::{Sim, mid};
use shrew_core::PromptKind;
use shrew_core::picker::PickerKind;
use winit::window::CursorIcon;

fn at(s: &mut Sim, item: Item) -> (f32, f32) {
    s.frame();
    mid(s.ui.hits.status.get(item).unwrap_or_else(|| panic!("{item:?} is not drawn")))
}

#[test]
fn items_light_up_under_the_pointer() {
    let (mut s, d) = Sim::project("status-hover", &["main.rs"], "main.rs");
    s.mouse = Mouse::new();
    let (x, y) = at(&mut s, Item::Position);
    s.run(&format!("<move:{x},{y}>"));
    assert_eq!(s.ui.status_hover, Some(Item::Position));
    assert_eq!(s.mouse.icon(), CursorIcon::Pointer);
    let (x, y) = at(&mut s, Item::Language);
    s.run(&format!("<move:{x},{y}>"));
    assert_eq!(s.ui.status_hover, Some(Item::Language));
    // off the items: nothing lit, the arrow back
    let lay = s.frame();
    let (x, y) = (lay.status.x + lay.status.w / 2.0, lay.status.y + lay.status.h / 2.0);
    s.run(&format!("<move:{x},{y}>"));
    assert_eq!(s.ui.status_hover, None);
    assert_eq!(s.mouse.icon(), CursorIcon::Default);
    std::fs::remove_dir_all(d).ok();
}

#[test]
fn clicks_open_go_to_line_set_language_and_the_mode_menu() {
    let (mut s, d) = Sim::project("status-click", &["main.rs"], "main.rs");
    let (x, y) = at(&mut s, Item::Position);
    s.run(&format!("<click:{x},{y}>"));
    assert_eq!(s.st.ed.prompt.as_ref().map(|p| p.kind), Some(PromptKind::Command), "Go to Line is `:`");
    s.run("<Esc>");
    let (x, y) = at(&mut s, Item::Language);
    s.run(&format!("<click:{x},{y}>"));
    let p = s.st.ed.picker.as_ref().expect("Set Language");
    assert_eq!(p.kind, PickerKind::Choice(shrew_core::Choice::Language));
    s.run("<Esc>");
    // the vim mode badge: a small menu above it
    let (x, y) = at(&mut s, Item::Mode);
    s.run(&format!("<click:{x},{y}>"));
    let m = s.st.menu.as_ref().expect("the mode menu");
    let vim = m.items.iter().find_map(|e| match e {
        MenuItem::Action(a) if a.label == "Vim Mode" => Some(a.checked),
        _ => None,
    });
    assert_eq!(vim, Some(true), "a check mark: vim is on");
    s.frame();
    let m = s.st.menu.as_ref().unwrap();
    assert!(m.hits[0].panel.bottom() <= s.ui.hits.status.get(Item::Mode).unwrap().y + 1.0, "it opens upwards");
    s.choose("Vim Mode");
    assert!(!s.st.ed.vim(), "vim off");
    assert!(s.ui.hits.status.get(Item::Mode).is_none(), "no badge without vim");
    std::fs::remove_dir_all(d).ok();
}

#[test]
fn right_click_on_the_bar_opens_its_menu() {
    let (mut s, d) = Sim::project("status-menu", &["main.rs"], "main.rs");
    let lay = s.frame();
    let (x, y) = (lay.status.x + lay.status.w / 2.0, lay.status.y + lay.status.h / 2.0);
    s.run(&format!("<rclick:{x},{y}>"));
    let m = s.st.menu.as_ref().expect("the bar's menu");
    assert_eq!(m.target, Target::StatusBar);
    let labels: Vec<&str> = m.items.iter().map(MenuItem::label).collect();
    assert!(labels.contains(&"Go to Line…") && labels.contains(&"Set Language…"), "{labels:?}");
    let settings = m.items.iter().find_map(|e| match e {
        MenuItem::Action(a) if a.label == "Settings…" => a.hint.clone(),
        _ => None,
    });
    assert!(settings.is_some_and(|h| h.ends_with("+,")), "commands show their chord");
    std::fs::remove_dir_all(d).ok();
}
