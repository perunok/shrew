use super::*;
use std::sync::Arc;

fn ed() -> Editor {
    Editor::new()
}

fn label(m: &ContextMenu) -> &str {
    m.selected.map_or("", |i| m.level(m.path.len())[i].label())
}

fn key(m: &mut ContextMenu, s: &str) -> Outcome {
    let mut out = Outcome::Stay;
    for k in KeyPress::parse_seq(s) {
        out = m.key(k, Instant::now());
    }
    out
}

fn act(label: &str) -> MenuItem {
    Action::ex(label, label).into()
}

/// Copy | Cut (disabled) | - | Paste | Find ▸ (Next, Previous) | Join Lines
fn sample() -> ContextMenu {
    let items = vec![
        act("Copy"),
        Action::ex("Cut", "cut").enabled(false).into(),
        MenuItem::Separator,
        act("Paste"),
        Submenu::new("Find", vec![act("Next"), act("Previous")]).into(),
        act("Join Lines"),
    ];
    ContextMenu::new(Target::Nothing, items)
}

#[test]
fn spec_items_and_tidy() {
    let spec = ["-", "customize_titlebar", "-", "-", "system_titlebar|Use System Title Bar", "no_such", "-"];
    let items = items_from_spec(&ed(), &spec);
    let labels: Vec<&str> = items.iter().map(MenuItem::label).collect();
    assert_eq!(labels, ["Customize Title Bar…", "", "Use System Title Bar"], "no separator first, last or twice");
    let mut nested = vec![Submenu::new("S", vec![MenuItem::Separator, act("a"), MenuItem::Separator]).into()];
    tidy(&mut nested);
    let MenuItem::Submenu(s) = &nested[0] else { panic!() };
    assert_eq!(s.items.len(), 1);
    // a command's hint is the chord that runs it, its check mark the toggle's state
    let a = Action::command(&ed(), Command::Save);
    assert_eq!((a.label.as_str(), a.hint.clone()), ("Save", Some(chord_label(KeyPress::parse("<C-s>").unwrap()))));
    assert!(Action::command(&ed(), Command::from_name("toggle_line_numbers").unwrap()).checked);
    // the builder
    fn nop(_: &mut State, _: &Target, _: usize) {}
    let a = Action::app("Pin", nop).hint(Some("F2".into())).checked(true).enabled(false).mnemonic('P');
    assert_eq!((a.hint.as_deref(), a.checked, a.enabled, a.mnemonic), (Some("F2"), true, false, Some('p')));
    assert!(matches!(a.run, Run::App(_, 0)));
}

#[test]
fn shortcuts_follow_the_mode_and_your_mappings() {
    let mut e = ed();
    let ctrl = |s: &str| Some(chord_label(KeyPress::parse(s).unwrap()));
    assert_eq!(shortcut(&e, Command::CommandPalette), ctrl("<C-S-p>"));
    assert_eq!(shortcut(&e, Command::SelectAllOccurrences), ctrl("<M-F3>"));
    // Ctrl+C copies in insert mode only: vim normal mode shows no chord for it
    assert_eq!(shortcut(&e, Command::Copy), None);
    e.set_mode(Mode::Insert);
    assert_eq!(shortcut(&e, Command::Copy), ctrl("<C-c>"));
    e.keymap.map(MapMode::Insert, KeyPress::parse_seq("<F5>"), Rhs::Command(Command::Copy));
    assert_eq!(shortcut(&e, Command::Copy), Some("F5".into()));
    assert_eq!(shortcut(&e, Command::ShowMenu), Some("F10".into()));
    assert!(chord_label(KeyPress::parse("<C-S-Tab>").unwrap()).ends_with("Shift+Tab"));
}

#[test]
fn keyboard_navigation_skips_separators_and_disabled_items() {
    let mut m = sample();
    assert_eq!(m.selected, None, "opened by the mouse: nothing highlighted");
    key(&mut m, "<Down>");
    assert_eq!(label(&m), "Copy");
    key(&mut m, "<Down>");
    assert_eq!(label(&m), "Paste", "Cut is disabled, then a separator");
    key(&mut m, "jj");
    assert_eq!(label(&m), "Join Lines", "j and k move too");
    key(&mut m, "<Down>");
    assert_eq!(label(&m), "Copy", "wraps around");
    key(&mut m, "k");
    assert_eq!(label(&m), "Join Lines");
    key(&mut m, "<Home>");
    assert_eq!(label(&m), "Copy");
    key(&mut m, "<End>");
    assert_eq!(label(&m), "Join Lines");
    assert!(matches!(key(&mut m, "<CR>"), Outcome::Run(Run::Ex(l)) if l == "Join Lines"));
    assert!(matches!(key(&mut m, "<Esc>"), Outcome::Close));
    let m = sample().select_first();
    assert_eq!(label(&m), "Copy", "opened from the keyboard: the first item");
}

#[test]
fn submenus_open_and_close_with_the_keys() {
    let mut m = sample();
    key(&mut m, "<End><Up>");
    assert_eq!(label(&m), "Find");
    key(&mut m, "<Right>");
    assert_eq!((m.path.as_slice(), label(&m)), (&[4][..], "Next"));
    key(&mut m, "<Down>");
    assert_eq!(label(&m), "Previous");
    key(&mut m, "<Left>");
    assert_eq!((m.path.len(), label(&m)), (0, "Find"));
    key(&mut m, "<CR>");
    assert_eq!(label(&m), "Next", "Enter opens a submenu");
    assert!(matches!(key(&mut m, "<Esc>"), Outcome::Stay), "Esc closes the submenu first");
    assert_eq!(label(&m), "Find");
    assert!(matches!(key(&mut m, "<Esc>"), Outcome::Close));
    // Right on an action does nothing
    let mut m = sample().select_first();
    key(&mut m, "<Right>");
    assert_eq!((m.path.len(), label(&m)), (0, "Copy"));
}

#[test]
fn mnemonics_and_type_ahead() {
    let mut m = ContextMenu::new(
        Target::Nothing,
        vec![
            Action::ex("Close", "close").mnemonic('c').into(),
            Action::ex("Close Others", "only").mnemonic('o').into(),
            act("Copy Path"),
            act("Copy Relative Path"),
            act("Keep Open"),
        ],
    );
    // a mnemonic runs its item at once
    assert!(matches!(key(&mut m, "o"), Outcome::Run(Run::Ex(l)) if l == "only"));
    assert!(matches!(key(&mut m, "C"), Outcome::Run(Run::Ex(l)) if l == "close"), "either case");
    // other letters jump: a prefix typed together, the same letter again cycles
    key(&mut m, "p");
    assert_eq!(label(&m), "", "nothing starts with p");
    let mut m = ContextMenu::new(Target::Nothing, vec![act("Reveal"), act("Rename"), act("Refresh"), act("Open")]);
    key(&mut m, "ren");
    assert_eq!(label(&m), "Rename", "letters typed together make a prefix");
    key(&mut m, "<Down>r");
    assert_eq!(label(&m), "Refresh", "a key in between starts over, from the highlighted item");
    key(&mut m, "r");
    assert_eq!(label(&m), "Reveal", "the same letter again: the next match");
    // k is a move, not type-ahead
    key(&mut m, "<Esc>");
    let mut m = ContextMenu::new(Target::Nothing, vec![act("Copy Path"), act("Keep Open")]);
    key(&mut m, "k");
    assert_eq!(label(&m), "Keep Open", "k from nothing goes to the last item");
}

#[test]
fn placement_keeps_the_menu_on_screen() {
    let b = Rect::new(0.0, 0.0, 800.0, 600.0);
    assert_eq!(place((100.0, 100.0), 200.0, 150.0, b), Rect::new(100.0, 100.0, 200.0, 150.0), "at the pointer");
    assert_eq!(place((700.0, 100.0), 200.0, 150.0, b).x, 500.0, "no room on the right: opens to the left");
    assert_eq!(place((100.0, 550.0), 200.0, 150.0, b).y, 400.0, "no room below: opens upwards");
    let r = place((790.0, 590.0), 200.0, 150.0, b);
    assert_eq!((r.x, r.y), (590.0, 440.0), "the corner: both");
    let r = place((100.0, 300.0), 200.0, 400.0, b);
    assert_eq!(r.y, 200.0, "room on neither side: moved up just enough, near the pointer");
    let r = place((50.0, 100.0), 200.0, 900.0, b);
    assert_eq!((r.y, r.h), (0.0, 600.0), "taller than the window: all of it, scrolling");
    let r = place((10.0, 10.0), 900.0, 100.0, b);
    assert_eq!(r.x, 0.0, "wider than the window: from its left edge");
    // a submenu opens beside its row, on the left when the right has no room, moved up to fit
    let parent = Rect::new(100.0, 100.0, 200.0, 300.0);
    let row = Rect::new(104.0, 200.0, 192.0, 24.0);
    assert_eq!(place_sub(parent, row, 150.0, 100.0, b, 4.0), Rect::new(300.0, 196.0, 150.0, 100.0));
    let parent = Rect::new(550.0, 100.0, 200.0, 300.0);
    assert_eq!(place_sub(parent, row, 150.0, 100.0, b, 4.0).x, 400.0);
    let low = Rect::new(104.0, 560.0, 192.0, 24.0);
    assert_eq!(place_sub(parent, low, 150.0, 100.0, b, 4.0).y, 500.0);
}

/// The sample menu as drawn at (100, 100) in rows 20 high (`Find`'s submenu open beside it when `sub`).
fn drawn(m: &mut ContextMenu, sub: bool) {
    let rows: Vec<(Rect, usize)> = [0, 1, 3, 4, 5]
        .iter()
        .enumerate()
        .map(|(k, &i)| (Rect::new(100.0, 100.0 + k as f32 * 20.0, 200.0, 20.0), i))
        .collect();
    m.hits = vec![Level { panel: Rect::new(96.0, 96.0, 208.0, 108.0), rows }];
    if sub {
        let rows = vec![(Rect::new(310.0, 160.0, 100.0, 20.0), 0), (Rect::new(310.0, 180.0, 100.0, 20.0), 1)];
        m.hits.push(Level { panel: Rect::new(306.0, 156.0, 108.0, 48.0), rows });
    }
}

#[test]
fn hovering_highlights_and_opens_submenus() {
    let mut m = sample();
    drawn(&mut m, false);
    assert!(m.hover(150.0, 110.0));
    assert_eq!(label(&m), "Copy");
    assert!(!m.hover(160.0, 112.0), "the same item: nothing to draw");
    assert!(m.hover(150.0, 130.0));
    assert_eq!(m.selected, None, "a disabled item is not highlighted");
    assert!(m.hover(150.0, 170.0));
    assert_eq!((m.path.as_slice(), m.selected), (&[4][..], None), "hovering a submenu opens it");
    drawn(&mut m, true);
    assert!(m.hover(350.0, 190.0));
    assert_eq!(label(&m), "Previous");
    assert!(m.hover(150.0, 170.0));
    assert_eq!((m.path.as_slice(), m.selected), (&[4][..], None), "back on its row: it stays open");
    assert!(m.hover(150.0, 190.0));
    assert_eq!((m.path.len(), label(&m)), (0, "Join Lines"), "another row closes it");
    assert!(!m.hover(500.0, 500.0), "outside: nothing changes");
    assert!(m.inside(100.0, 100.0) && !m.inside(500.0, 500.0));
}

#[test]
fn release_to_select() {
    let mut m = sample();
    drawn(&mut m, false);
    // the press that opened the menu, released on the spot: it stays open
    m.hold(MouseButton::Right, (98.0, 98.0), true);
    assert!(matches!(m.release(99.0, 99.0, MouseButton::Right, 3.0), Outcome::Stay));
    // dragged onto an item and released: it runs
    m.hold(MouseButton::Right, (98.0, 98.0), true);
    assert!(matches!(m.release(150.0, 110.0, MouseButton::Left, 3.0), Outcome::Stay), "another button");
    assert!(matches!(m.release(150.0, 110.0, MouseButton::Right, 3.0), Outcome::Run(Run::Ex(l)) if l == "Copy"));
    // onto a disabled item: nothing; dragged out of the menu: it closes
    m.hold(MouseButton::Right, (98.0, 98.0), true);
    assert!(matches!(m.release(150.0, 130.0, MouseButton::Right, 3.0), Outcome::Stay));
    m.hold(MouseButton::Right, (98.0, 98.0), true);
    assert!(matches!(m.release(500.0, 500.0, MouseButton::Right, 3.0), Outcome::Close));
    // a click on the menu: press and release on an item
    assert!(matches!(m.press(150.0, 150.0, MouseButton::Left), Outcome::Stay));
    assert_eq!(label(&m), "Paste");
    assert!(matches!(m.release(150.0, 150.0, MouseButton::Left, 3.0), Outcome::Run(Run::Ex(l)) if l == "Paste"));
    assert!(matches!(m.release(150.0, 150.0, MouseButton::Left, 3.0), Outcome::Stay), "no press, no run");
    assert!(matches!(m.press(500.0, 500.0, MouseButton::Left), Outcome::Close), "a press outside closes it");
}

#[test]
fn opening_and_running_through_the_state() {
    let mut st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
    assert!(!open(&mut st, ContextMenu::new(Target::Nothing, vec![MenuItem::Separator]), (0.0, 0.0)), "empty");
    assert!(st.menu.is_none());
    crate::titlebar::open_menu(&mut st, None);
    let m = ContextMenu::from_spec(&st.ed, Target::Nothing, &["toggle_line_numbers|Line Numbers"]);
    assert!(open(&mut st, m, (5.0, 6.0)));
    assert!(st.titlebar.menu.is_none(), "one menu at a time");
    assert_eq!(st.menu.as_ref().unwrap().at, (5.0, 6.0));
    assert!(st.ed.features.on(Feature::LineNumbers));
    for k in KeyPress::parse_seq("<Down><CR>") {
        st.key(k);
    }
    assert!(st.menu.is_none());
    assert!(!st.ed.features.on(Feature::LineNumbers), "the command ran");
    // an ex line with its argument
    run(&mut st, &Target::Nothing, Run::Ex("set tabstop=7".into()));
    assert_eq!(st.ed.settings.tab_width, 7);
}
