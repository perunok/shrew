//! The tree driven by the mouse as the window drives it (a frame drawn before each event): hover, clicks, the
//! middle button and the right-click menus with what their items do.

use crate::context_menu::MenuItem;
use crate::draw::Ui;
use crate::layout::Layout;
use crate::mouse::{Input, Mouse, Target, parse_input};
use crate::state::State;
use shrew_core::Feature;
use shrew_render::{Metrics, Rect, Scene};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

pub(crate) struct Sim {
    pub st: State,
    pub ui: Ui,
    scene: Scene,
    pub mouse: Mouse,
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
    /// A project in a temp folder: `files` (a path ending in `/` is a folder), the sidebar on, `open` opened.
    pub fn project(name: &str, files: &[&str], open: &str) -> (Sim, PathBuf) {
        let d = std::env::temp_dir().join(format!("shrew-sidebar-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(".shrew"), "").unwrap();
        for f in files {
            if let Some(dir) = f.strip_suffix('/') {
                std::fs::create_dir_all(d.join(dir)).unwrap();
            } else {
                if let Some(p) = d.join(f).parent() {
                    std::fs::create_dir_all(p).unwrap();
                }
                std::fs::write(d.join(f), format!("{f}\n")).unwrap();
            }
        }
        let mut ui = Ui::new();
        ui.animate = false;
        let mut st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
        st.ed.project = Some(d.clone());
        st.ed.features.set(Feature::Sidebar, true);
        st.ed.features.set(Feature::CompactFolders, false);
        st.open_at(&d.join(open), None, None);
        st.after_input();
        let mut s = Sim { st, ui, scene: Scene::new(), mouse: Mouse::new(), now: Instant::now() };
        s.frame();
        (s, d)
    }

    pub fn frame(&mut self) -> Layout {
        let lay = crate::layout::compute_with(&metrics(), (1000, 600), 1.0, &self.st);
        crate::draw::build(&mut self.scene, &mut self.st, &mut self.ui, &lay, Instant::now());
        lay
    }

    pub fn run(&mut self, input: &str) {
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

    /// The middle of the row showing `name`.
    fn row(&mut self, name: &str) -> (f32, f32) {
        self.frame();
        let t = self.st.ed.tree.as_ref().unwrap();
        let i = t.rows.iter().position(|r| r.name == name).unwrap_or_else(|| panic!("no row {name}"));
        let r = self.ui.hits.sidebar.rows.iter().find(|h| h.1 == i).unwrap().0;
        mid(r)
    }

    fn names(&self) -> Vec<String> {
        self.st.ed.tree.as_ref().unwrap().rows.iter().map(|r| r.name.clone()).collect()
    }

    fn selected(&self) -> String {
        self.st.ed.tree.as_ref().unwrap().current().unwrap().name.clone()
    }

    /// The labels of the open menu's top level.
    fn menu(&self) -> Vec<String> {
        let m = self.st.menu.as_ref().expect("a menu is open");
        m.items.iter().map(|e| if e.is_separator() { "-".into() } else { e.label().to_string() }).collect()
    }

    fn hint(&self, label: &str) -> Option<String> {
        self.st.menu.as_ref()?.items.iter().find_map(|e| match e {
            MenuItem::Action(a) if a.label == label => a.hint.clone(),
            _ => None,
        })
    }

    /// Clicks the open menu's item `label`.
    pub fn choose(&mut self, label: &str) {
        self.frame();
        let m = self.st.menu.as_ref().expect("a menu is open");
        let row = m.hits[0].rows.iter().find(|r| m.items[r.1].label() == label).unwrap_or_else(|| panic!("{label}"));
        let (x, y) = mid(row.0);
        self.run(&format!("<click:{x},{y}>"));
    }
}

pub fn mid(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn done(d: &Path) {
    std::fs::remove_dir_all(d).ok();
}

#[test]
fn hovering_lights_a_row_and_leaving_unlights_it() {
    let (mut s, d) = Sim::project("hover", &["a.txt", "b.txt"], "a.txt");
    let (x, y) = s.row("b.txt");
    s.run(&format!("<move:{x},{y}>"));
    let t = s.st.ed.tree.as_ref().unwrap();
    assert_eq!(t.hover.map(|i| t.rows[i].name.as_str()), Some("b.txt"));
    let lay = s.frame();
    let r = s.mouse.left(&mut s.st, &mut s.ui, Some(lay));
    assert!(r.redraw, "leaving the window unlights it");
    assert_eq!(s.st.ed.tree.as_ref().unwrap().hover, None);
    done(&d);
}

#[test]
fn clicks_open_files_and_fold_folders_once() {
    let (mut s, d) = Sim::project("clicks", &["src/main.rs", "src/lib.rs", "z.txt"], "z.txt");
    // a click on a closed folder opens it; the second click of a double click does not close it again
    let (x, y) = s.row("src");
    s.run(&format!("<dclick:{x},{y}>"));
    assert!(s.names().contains(&"main.rs".to_string()), "{:?}", s.names());
    assert!(s.st.ed.sidebar_focus);
    // a click on a file opens it and gives it the keys
    let (x, y) = s.row("main.rs");
    s.run(&format!("<click:{x},{y}>"));
    assert!(s.st.ed.buf().doc.path.as_ref().unwrap().ends_with("src/main.rs"));
    assert!(!s.st.ed.sidebar_focus);
    // the chevron folds the folder and leaves the selection where it was
    let (x, y) = s.row("z.txt");
    s.run(&format!("<click:{x},{y}>"));
    let (_, y) = s.row("src");
    let (x0, _) = s.ui.hits.sidebar.indent;
    s.run(&format!("<click:{},{y}>", x0 + 4.0));
    assert!(!s.names().contains(&"main.rs".to_string()), "folded");
    assert_eq!(s.selected(), "z.txt");
    s.run(&format!("<click:{},{y}>", x0 + 4.0));
    assert!(s.names().contains(&"main.rs".to_string()), "unfolded");
    assert_eq!(s.selected(), "z.txt");
    done(&d);
}

#[test]
fn a_middle_click_opens_a_file_behind_the_current_one() {
    let (mut s, d) = Sim::project("middle", &["a.txt", "b.txt"], "a.txt");
    let n = s.st.ed.buffers.len();
    let (x, y) = s.row("b.txt");
    s.run(&format!("<mclick:{x},{y}>"));
    assert_eq!(s.st.ed.buffers.len(), n + 1);
    assert!(s.st.ed.buf().doc.path.as_ref().unwrap().ends_with("a.txt"), "the current file stays in front");
    done(&d);
}

#[test]
fn right_click_selects_the_row_and_opens_its_menu() {
    let (mut s, d) = Sim::project("menu", &["src/main.rs", "a.txt", "b.txt"], "a.txt");
    let (x, y) = s.row("b.txt");
    let lay = s.frame();
    let i = s.st.ed.tree.as_ref().unwrap().rows.iter().position(|r| r.name == "b.txt").unwrap();
    assert_eq!(crate::mouse::target_at(&s.st, &s.ui, &lay, x, y), Target::Sidebar(Some(i)));
    s.run(&format!("<rclick:{x},{y}>"));
    assert_eq!(s.selected(), "b.txt");
    assert!(s.st.ed.sidebar_focus, "the tree has the keys, so its keys are the hints");
    let m = s.menu();
    for want in ["New File…", "New Folder…", "Rename…", "Duplicate…", "Copy Path", "Copy Relative Path", "Copy Name"]
    {
        assert!(m.contains(&want.to_string()), "{want} in {m:?}");
    }
    assert!(m.contains(&"Collapse All".to_string()) && m.contains(&"Refresh".to_string()));
    assert!(!m.contains(&"Find in Folder…".to_string()), "a file has no Find in Folder");
    assert_eq!(s.hint("Rename…").as_deref(), Some("F2"));
    assert_eq!(s.hint("New File…").as_deref(), Some("a"));
    assert_eq!(s.hint("Refresh").as_deref(), Some("F5"));
    // Copy Relative Path
    s.choose("Copy Relative Path");
    assert!(s.st.menu.is_none());
    assert_eq!(s.st.ed.clipboard.get().as_deref(), Some("b.txt"));
    // a folder has Find in Folder
    let (x, y) = s.row("src");
    s.run(&format!("<rclick:{x},{y}>"));
    assert!(s.menu().contains(&"Find in Folder…".to_string()), "{:?}", s.menu());
    s.run("<Esc>");
    assert!(s.st.menu.is_none());
    assert!(s.st.ed.sidebar_focus, "Esc leaves the keys with the tree");
    done(&d);
}

#[test]
fn the_empty_part_makes_new_entries_at_the_root() {
    let (mut s, d) = Sim::project("empty", &["src/main.rs", "a.txt"], "src/main.rs");
    // below the last row
    let last = s.ui.hits.sidebar.rows.last().unwrap().0;
    let (x, y) = (last.x + 40.0, last.bottom() + 60.0);
    s.run(&format!("<rclick:{x},{y}>"));
    let m = s.menu();
    assert_eq!(s.st.menu.as_ref().unwrap().target, Target::Sidebar(None));
    assert!(m.contains(&"New File…".to_string()) && !m.contains(&"Rename…".to_string()), "{m:?}");
    assert!(m.contains(&"Find in Project…".to_string()), "{m:?}");
    s.choose("New File…");
    let p = s.st.ed.prompt.as_ref().expect("asks for the name");
    assert_eq!(p.label, "New file: ", "at the root, not in src/ where the selection is");
    s.run("new.txt<CR>");
    assert!(d.join("new.txt").is_file());
    done(&d);
}

#[test]
fn duplicate_rename_and_collapse_from_the_menu() {
    let (mut s, d) = Sim::project("dup", &["src/main.rs", "notes.md"], "notes.md");
    let (x, y) = s.row("notes.md");
    s.run(&format!("<rclick:{x},{y}>"));
    s.choose("Duplicate…");
    let p = s.st.ed.prompt.as_ref().expect("asks for the name");
    assert_eq!(p.text, "notes copy.md");
    assert_eq!(p.cursor, "notes copy".len(), "the caret before the extension");
    s.run("<CR>");
    assert_eq!(std::fs::read_to_string(d.join("notes copy.md")).unwrap(), "notes.md\n");
    assert_eq!(s.selected(), "notes copy.md");
    // Collapse All closes src/ (opened to show main.rs? not here: open it first)
    let (x, y) = s.row("src");
    s.run(&format!("<click:{x},{y}>"));
    assert!(s.names().contains(&"main.rs".to_string()));
    let (x, y) = s.row("main.rs");
    s.run(&format!("<rclick:{x},{y}>"));
    s.choose("Collapse All");
    assert!(!s.names().contains(&"main.rs".to_string()));
    assert_eq!(s.selected(), "src", "the selection goes up to its top-level folder");
    done(&d);
}

#[cfg(feature = "project")]
#[test]
fn find_in_folder_searches_only_that_folder() {
    let (mut s, d) = Sim::project("find", &["src/main.rs", "docs/x.md"], "docs/x.md");
    let (x, y) = s.row("src");
    s.run(&format!("<rclick:{x},{y}>"));
    s.choose("Find in Folder…");
    let p = s.st.ed.picker.as_ref().expect("find in project opens");
    assert_eq!(p.kind, shrew_core::picker::PickerKind::Grep);
    let g = p.grep.as_ref().unwrap();
    assert_eq!(g.scope.as_deref(), Some(d.join("src").as_path()));
    assert!(!s.st.ed.sidebar_focus);
    // the results come from src/ only (".rs" is in both names; the files hold their own paths)
    s.run("<Tab><BS>");
    let g = s.st.ed.picker.as_ref().unwrap().grep.as_ref().unwrap();
    assert_eq!(g.scope, None, "Backspace in the empty globs: the whole project again");
    done(&d);
}

// ----- merge: the keyboard opener -----

#[test]
fn shift_f10_in_the_sidebar_opens_the_selected_rows_menu_under_it() {
    let (mut s, d) = Sim::project("shiftf10", &["a.rs", "b.rs"], "a.rs");
    let (x, y) = s.row("b.rs");
    s.run(&format!("<click:{x},{y}>"));
    s.st.ed.sidebar_focus = true;
    s.run("<S-F10>");
    assert_eq!(
        s.st.menu.as_ref().map(|m| m.target),
        Some(Target::Sidebar(Some(s.st.ed.tree.as_ref().unwrap().selected)))
    );
    let m = s.st.menu.as_ref().unwrap();
    assert!(m.selected.is_some(), "the first entry is highlighted for the keys");
    assert!(m.at.1 > y, "under the row");
    assert!(s.st.ed.sidebar_focus, "the sidebar keeps the keys");
    s.run("<Esc>");
    assert!(s.st.menu.is_none());
    let _ = std::fs::remove_dir_all(d);
}
