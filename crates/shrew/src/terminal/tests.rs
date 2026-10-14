//! The panel's layout, key routing, and real programs on a real PTY read back from the screen.

use super::*;
use crate::state::State;
use shrew_render::Metrics;
use std::sync::Arc;

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

fn layout() -> Layout {
    Layout::new(&metrics(), (1000, 800), 1.0, 100, true, true)
}

fn state() -> State {
    State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true)
}

/// Polls and draws until `done` or a few seconds pass.
fn wait(st: &mut State, mut done: impl FnMut(&mut State) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut scene = shrew_render::Scene::new();
    let mut ui = crate::draw::Ui::new();
    while Instant::now() < deadline {
        st.poll();
        let lay = carve(layout(), &st.terminals);
        crate::draw::build(&mut scene, st, &mut ui, &lay, Instant::now());
        if done(st) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// The text of screen row `row` of the active terminal.
fn screen_row(st: &State, row: usize) -> String {
    let tab = &st.terminals.tabs[st.terminals.active];
    view::row_text(tab, Line(row as i32)).0.trim_end().to_string()
}

#[test]
fn the_panel_sits_between_text_and_status() {
    let lay = layout();
    let mut t = Terminals::new(shrew_config::TerminalConfig { height: Some(10), ..Default::default() });
    assert_eq!(carve(lay, &t), lay, "nothing open: nothing taken");
    t.visible = true;
    // carve only cares that a terminal exists; a closed one is not drawn
    t.tabs.clear();
    assert_eq!(carve(lay, &t), lay);
    let shown = lay.with_panel(t.panel_height(&lay));
    assert_eq!(shown.panel.h, panel_h(10, &lay));
    assert_eq!(shown.text.bottom(), shown.panel.y);
    assert_eq!(shown.panel.bottom(), shown.status.y);
    assert_eq!(shown.panel.w, lay.text.w);
    assert!(shown.rows < lay.rows);
    assert_eq!(shown.scrollbar.bottom(), shown.panel.y);
    let g = Geom::new(&shown);
    assert_eq!(g.rows, 10);
    assert_eq!(g.strip.bottom(), g.body.y);
    assert!(g.y0 + g.rows as f32 * g.ch <= g.body.bottom() + 0.5);
    assert!(g.x0 + g.cols as f32 * g.cw <= g.body.right());
    assert!(g.close.right() <= g.strip.right() && g.new.x > g.strip.x && g.max.x > g.new.x);
    // a click maps to its cell
    let (col, row, _) = g.cell_at(g.x0 + 3.2 * g.cw, g.y0 + 2.5 * g.ch);
    assert_eq!((col, row), (3, 2));
    // too tall a panel leaves the editor a few rows; maximized takes them all
    t.rows = 500;
    let big = lay.with_panel(t.panel_height(&lay));
    assert!(big.text.h >= MIN_EDITOR_ROWS * lay.ch - 0.5);
    t.maximized = true;
    assert_eq!(lay.with_panel(t.panel_height(&lay)).text.h, 0.0);
}

#[test]
fn escape_chords_and_config() {
    let cfg = shrew_config::TerminalConfig {
        keys: vec![("<C-PageUp>".into(), String::new()), ("<C-j>".into(), "focus_editor".into())],
        ..Default::default()
    };
    let mut t = Terminals::new(cfg);
    let k = |s: &str| KeyPress::parse(s).unwrap();
    assert_eq!(t.escape(k("<C-`>")), Some(Command::ToggleTerminal));
    assert_eq!(t.escape(k("<C-S-`>")), Some(Command::NewTerminal));
    assert_eq!(t.escape(k("<C-PageDown>")), Some(Command::NextTerminal));
    assert_eq!(t.escape(k("<C-PageUp>")), None, "given back to the terminal");
    assert_eq!(t.escape(k("<C-j>")), Some(Command::FocusEditor));
    assert_eq!(t.escape(k("<C-c>")), None);
}

#[cfg(unix)]
#[test]
fn a_command_runs_on_a_pty_and_lands_on_the_screen() {
    let mut st = state();
    st.ed.cwd = std::env::temp_dir();
    for k in KeyPress::parse_seq(r#":term printf 'hello \033[32mgreen\033[0m\n%s %s\n' "$TERM" "$COLORTERM"; pwd<CR>"#)
    {
        st.key(k);
    }
    assert_eq!(st.terminals.tabs.len(), 1, "{:?}", st.ed.message);
    assert!(st.terminals.visible && st.terminals.has_focus());
    assert!(wait(&mut st, |st| st.terminals.tabs[0].session.exited()), "the command ends");
    wait(&mut st, |st| !st.terminals_busy());
    assert_eq!(screen_row(&st, 0), "hello green");
    assert_eq!(screen_row(&st, 1), "xterm-256color truecolor");
    let tmp = std::env::temp_dir().canonicalize().unwrap();
    assert_eq!(std::path::Path::new(&screen_row(&st, 2)).canonicalize().unwrap(), tmp, "runs in the project folder");
    // the colors went through: "green" is ANSI green from the theme
    let tab = &st.terminals.tabs[0];
    let term = tab.session.term.lock();
    let cell = &term.grid()[Line(0)][Column(6)];
    assert_eq!(cell.c, 'g');
    assert_eq!(cell.fg, alacritty_terminal::vte::ansi::Color::Named(alacritty_terminal::vte::ansi::NamedColor::Green));
    drop(term);
    // a finished command stays until dismissed
    st.poll();
    assert_eq!(st.terminals.tabs.len(), 1);
    st.key(KeyPress::plain(Key::Enter));
    assert!(st.terminals.tabs.is_empty());
    assert!(!st.terminals.visible && !st.terminals.has_focus());
}

#[cfg(unix)]
#[test]
fn keys_reach_the_program_and_escape_chords_do_not() {
    let mut st = state();
    st.ed.cwd = std::env::temp_dir();
    // `cat` echoes what it reads; the line discipline echoes too, so the text shows up
    st.terminal_command(Command::NewTerminal, Some("cat".into()));
    assert_eq!(st.terminals.tabs.len(), 1);
    wait(&mut st, |st| !st.terminals_busy());
    for k in KeyPress::parse_seq("abc<CR>") {
        st.key(k);
    }
    assert!(wait(&mut st, |st| screen_row(st, 1) == "abc"), "{:?}", screen_row(&st, 0));
    // Ctrl+` hides the panel and gives the editor the keys back; `i` then types into the buffer
    st.key(KeyPress::parse("<C-`>").unwrap());
    assert!(!st.terminals.visible && !st.terminals.has_focus());
    for k in KeyPress::parse_seq("ihi<Esc>") {
        st.key(k);
    }
    assert_eq!(st.ed.buf().rope().to_string(), "hi");
    // and Ctrl+` brings the same terminal back
    st.key(KeyPress::parse("<C-`>").unwrap());
    assert!(st.terminals.has_focus());
    assert_eq!(st.terminals.tabs.len(), 1);
    // Ctrl+D ends cat; a command's tab stays, Enter closes it
    st.key(KeyPress::parse("<C-d>").unwrap());
    assert!(wait(&mut st, |st| st.terminals.tabs[0].session.exited()));
    st.key(KeyPress::plain(Key::Enter));
    assert!(st.terminals.tabs.is_empty());
}

#[cfg(unix)]
#[test]
fn normal_mode_copies_from_the_screen() {
    let mut st = state();
    st.terminal_command(Command::NewTerminal, Some("printf 'one two three'; sleep 5".into()));
    assert!(wait(&mut st, |st| screen_row(st, 0) == "one two three"));
    for k in KeyPress::parse_seq("<C-\\><C-n>0wve") {
        st.key(k);
    }
    st.key(KeyPress::ch('y'));
    assert_eq!(st.ed.clipboard.get().as_deref(), Some("two"));
    // back to typing
    st.key(KeyPress::ch('i'));
    assert!(!view::in_normal(&st.terminals.tabs[0]));
    st.terminal_command(Command::CloseTerminal, None);
    assert!(st.terminals.tabs.is_empty());
}

#[cfg(unix)]
#[test]
fn a_shell_that_exits_closes_its_tab() {
    let mut st = state();
    st.terminals.cfg.shell = Some("/bin/sh".into());
    st.terminal_command(Command::ToggleTerminal, None);
    assert_eq!(st.terminals.tabs.len(), 1);
    wait(&mut st, |st| !st.terminals_busy());
    for k in KeyPress::parse_seq("exit<CR>") {
        st.key(k);
    }
    assert!(wait(&mut st, |st| st.terminals.tabs.is_empty()), "closed when the shell exits");
    assert!(!st.terminals.visible);
}

#[cfg(unix)]
#[test]
fn mouse_links_selection_and_history() {
    let mut st = state();
    // the shell starts in the project folder
    st.ed.set_project(&PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    st.terminal_command(Command::NewTerminal, Some("seq 1 40; printf 'error at src/main.rs:3:5 here'; sleep 5".into()));
    assert!(wait(&mut st, |st| screen_row(st, st.terminals.geom.map_or(0, |g| g.rows - 1)).contains("here")));
    let g = st.terminals.geom.unwrap();
    let row = g.rows - 1;
    let at = |col: usize| (g.x0 + (col as f32 + 0.3) * g.cw, g.y0 + (row as f32 + 0.5) * g.ch);
    let none = Mods::default();
    // hovering the path underlines it (it exists, relative to the shell's directory)
    let (x, y) = at(12);
    let (icon, redraw) = st.terminal_motion(x, y, none);
    assert!(redraw);
    assert_eq!(icon, Some(winit::window::CursorIcon::Pointer));
    let h = st.terminals.hover.clone().expect("a link under the pointer");
    assert_eq!(h.cols, 9..24);
    assert_eq!(h.target, Link::File(st.ed.cwd.join("src/main.rs"), Some(3), Some(5)));
    // a drag selects and copies "error"
    let (x0, y0) = at(0);
    assert!(st.terminal_press(x0, y0, MouseButton::Left, none));
    let (x1, y1) = at(4);
    st.terminal_motion(x1 + 0.4 * g.cw, y1, none);
    st.terminal_release(MouseButton::Left, none);
    assert_eq!(st.ed.clipboard.get().as_deref(), Some("error"));
    // the wheel scrolls back through the history, typing comes back down
    assert!(st.terminal_wheel(3.0, x0, y0, none));
    wait(&mut st, |_| true);
    assert_eq!(st.terminals.tabs[0].view.display_offset, 3);
    // Ctrl+click opens the file at its line and gives the editor the keys
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    let (x, y) = at(12);
    st.terminal_wheel(-3.0, x, y, none);
    assert!(st.terminal_press(x, y, MouseButton::Left, ctrl));
    assert_eq!(st.ed.buf().doc.path.as_deref(), Some(st.ed.cwd.join("src/main.rs").as_path()));
    let b = st.ed.buf();
    assert_eq!(shrew_core::text::line_of(b.rope(), b.sel.primary().head), 2);
    assert!(!st.terminals.has_focus());
    // a click in the editor's area leaves the terminal unfocused; one in the panel focuses it
    assert!(st.terminal_press(x0, y0, MouseButton::Left, none));
    assert!(st.terminals.has_focus());
    assert!(!st.terminal_press(x0, g.panel.y - 50.0, MouseButton::Left, none));
    assert!(!st.terminals.has_focus());
}

/// The editor stays responsive while a terminal is flooded: `yes`, then `cat` of a 50 MB file. Each frame here is
/// what the app does per frame (poll, then build), with an editor keystroke in between. Run it optimized:
/// `cargo test --release -p shrew flood -- --ignored --nocapture`.
#[cfg(unix)]
#[test]
#[ignore = "a benchmark: run it in release"]
fn flood_keeps_the_editor_responsive() {
    let big = std::env::temp_dir().join(format!("shrew-flood-{}.txt", std::process::id()));
    let line = "the quick brown fox jumps over the lazy dog 0123456789 \x1b[32mgreen\x1b[0m and more text\n";
    std::fs::write(&big, line.repeat(50_000_000 / line.len())).unwrap();
    for cmd in ["timeout 3 yes", &format!("cat {}", big.display())] {
        let mut st = state();
        st.terminal_command(Command::NewTerminal, Some(cmd.to_string()));
        st.terminal_command(Command::FocusEditor, None);
        st.key(KeyPress::ch('i'));
        let mut scene = shrew_render::Scene::new();
        let mut ui = crate::draw::Ui::new();
        let (mut frames, mut worst_key, mut worst_frame) = (0, Duration::ZERO, Duration::ZERO);
        let mut total = Duration::ZERO;
        let t0 = Instant::now();
        while !st.terminals.tabs[0].session.exited() && t0.elapsed() < Duration::from_secs(60) {
            let t = Instant::now();
            st.key(KeyPress::ch('x'));
            worst_key = worst_key.max(t.elapsed());
            let t = Instant::now();
            st.poll();
            let lay = carve(layout(), &st.terminals);
            crate::draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
            let f = t.elapsed();
            worst_frame = worst_frame.max(f);
            total += f;
            frames += 1;
            // a 120 Hz display
            std::thread::sleep(Duration::from_millis(8));
        }
        let all = t0.elapsed();
        eprintln!(
            "{cmd}: {all:?}, {frames} frames, frame build mean {:?} worst {worst_frame:?}, editor key worst {worst_key:?}",
            total / frames.max(1)
        );
        assert!(worst_key < Duration::from_millis(16), "{worst_key:?}");
        assert!(worst_frame < Duration::from_millis(16), "{worst_frame:?}");
    }
    let _ = std::fs::remove_file(&big);
}

#[test]
fn feature_off_means_no_terminal() {
    let mut st = state();
    for k in KeyPress::parse_seq(":set noterminal<CR>") {
        st.key(k);
    }
    st.key(KeyPress::parse("<C-`>").unwrap());
    assert!(st.terminals.tabs.is_empty());
    assert!(st.ed.message.as_ref().is_some_and(|m| m.0.contains("terminal is off")));
}
