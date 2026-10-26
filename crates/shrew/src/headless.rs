//! `--screenshot`: renders one frame without a window, after typing `--keys`, and saves it as a PNG. Used for
//! automated visual tests and the README screenshots. It uses the built-in defaults (no config.toml, no vimrc or
//! Sublime import) so the output is the same on every machine, and never writes the config.
//!
//! `--keys` may hold mouse tokens (`<click:X,Y>`, `<rclick:X,Y>`, `<drag:..>`, `<wheel:..>`, see
//! [`crate::mouse::parse_input`]); they go through the same [`Mouse`] as the window's events, with a frame drawn
//! before each so the hit rects are those on screen. `--hover X,Y` moves the pointer there last.
//!
//! With `--stats` it also measures frame cost: the same frame redrawn (warm caches), one line scrolled per frame,
//! and a whole page scrolled per frame (every line shaped anew).

use crate::cli::Cli;
use crate::draw::{self, Ui};
use crate::layout::Layout;
use crate::mouse::{Event, Input, Kind, Mouse};
use crate::state::State;
use shrew_core::Mods;
use shrew_render::{Renderer, Scene};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn run(cli: &Cli, out: &std::path::Path) -> Result<(), String> {
    let t0 = Instant::now();
    let mut st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
    st.config_path = None;
    st.open_request(crate::launch::request(cli, false)?, None);
    // ----- updates: SHREW_UPDATE_DEMO (debug builds) shows the update UI offline -----
    #[cfg(feature = "update")]
    {
        crate::updates::demo(&mut st);
        // a restart's session, given explicitly (never the one a restart may have left in the cache)
        if let Some(p) = &cli.restore {
            crate::updates::restore_session(&mut st, Some(p));
        }
    }
    // -----
    let (w, h) = cli.size;
    let mut r = Renderer::headless(w, h, cli.scale, st.font.clone()).map_err(|e| format!("renderer: {e}"))?;
    r.wait_for_fonts(Duration::from_secs(5));
    r.poll_fonts();
    st.look.font_families = Some(r.monospace_families());
    let mut ui = Ui::new();
    ui.animate = false;
    let mut scene = Scene::new();
    let layout = |r: &Renderer, st: &State| crate::layout::compute(r, st);
    // a first layout pass sizes the views, so keys like Ctrl+D scroll by the real page
    let lay = layout(&r, &st);
    draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
    #[cfg(feature = "terminal")]
    let mut line_start = true;
    let mut mouse = Mouse::new();
    // made-up time for the mouse, so double clicks and drags do not depend on how fast frames render
    let mut clock = Instant::now();
    for input in crate::mouse::parse_input(&cli.keys)? {
        let k = match input {
            Input::Key(k) => k,
            Input::Mouse(ev) => {
                // a token's first event waits for what is loading (a list, a diff), as a person would
                if ev.after >= Duration::from_secs(1) {
                    settle(&mut st, &mut r, &mut scene, &mut ui, &layout);
                }
                // the frame on screen when the event comes: its hit rects
                let lay = layout(&r, &st);
                draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
                clock += ev.after;
                let _ = mouse.feed(&mut st, &mut ui, Some(lay), &ev, cli.scale, clock);
                // the window would carry these out; there is none
                st.titlebar.actions.clear();
                font_changed(&mut st, &mut r);
                continue;
            }
        };
        // a line typed into a terminal waits until the program is ready for it, as a person would
        #[cfg(feature = "terminal")]
        {
            let focused = st.terminals.has_focus();
            if line_start && focused {
                settle(&mut st, &mut r, &mut scene, &mut ui, &layout);
            }
            line_start = k.key == shrew_core::Key::Enter || !focused;
        }
        // ----- git: a key waits for the change markers of the text before it and for a diff still
        // loading, as a person would -----
        #[cfg(feature = "git")]
        {
            crate::git::settle_markers(&mut st);
            if crate::git::busy(&st) {
                settle(&mut st, &mut r, &mut scene, &mut ui, &layout);
            }
        }
        st.key(k);
        font_changed(&mut st, &mut r);
    }
    settle(&mut st, &mut r, &mut scene, &mut ui, &layout);
    if let Some((x, y)) = cli.hover {
        let lay = layout(&r, &st);
        draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
        let ev = Event { kind: Kind::Move, x, y, mods: Mods::default(), after: Duration::from_secs(1) };
        let _ = mouse.feed(&mut st, &mut ui, Some(lay), &ev, cli.scale, clock + ev.after);
    }
    let lay = layout(&r, &st);
    let tb = Instant::now();
    draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
    let build = tb.elapsed();
    r.render(&scene, || {}).map_err(|e| e.to_string())?;
    r.save_png(out).map_err(|e| format!("{}: {e}", out.display()))?;
    if cli.stats {
        eprintln!(
            "{} in {:?} (frame build {build:?}, render cpu {:?})",
            out.display(),
            t0.elapsed(),
            r.stats().cpu_time
        );
        bench(&mut st, &mut r, &mut scene, &mut ui, &lay);
    }
    Ok(())
}

/// A key or click changed the font size: the renderer takes the new font.
fn font_changed(st: &mut State, r: &mut Renderer) {
    if st.font_changed {
        st.font_changed = false;
        r.set_font(st.font.clone());
    }
}

/// Waits (up to a few seconds) for background work that changes the picture: grammars, highlighting, file lists.
fn settle(
    st: &mut State,
    r: &mut Renderer,
    scene: &mut Scene,
    ui: &mut Ui,
    layout: &dyn Fn(&Renderer, &State) -> Layout,
) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        st.poll();
        let lay = layout(r, st);
        draw::build(scene, st, ui, &lay, Instant::now());
        let picker_busy = st.ed.picker.as_ref().is_some_and(|p| p.loading);
        #[cfg(feature = "terminal")]
        let picker_busy = picker_busy || st.terminals_busy();
        // ----- git: discovery, markers, status, blame and diffs in flight -----
        #[cfg(feature = "git")]
        let picker_busy = picker_busy || crate::git::busy(st);
        if !picker_busy && !syntax_busy(st) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    st.poll();
}

#[cfg(feature = "syntax")]
fn syntax_busy(st: &State) -> bool {
    let Some(s) = &st.syntax else { return false };
    if !st.ed.features.on(shrew_core::Feature::Syntax) {
        return false;
    }
    !s.is_ready() || st.highlighters.values().any(|h| !h.is_settled())
}

#[cfg(not(feature = "syntax"))]
fn syntax_busy(_: &State) -> bool {
    false
}

/// A diff tab scrolls itself, a row or a page per frame and back to the top now and then. False when the active tab
/// is not a diff (or this build has no git).
#[cfg(feature = "git")]
fn scroll_diff(st: &mut State, step: isize, frame: usize) -> bool {
    if !crate::git::diffview::is_active(st) {
        return false;
    }
    let keys = match (step, frame % 40) {
        (_, 0) => "gg",
        (1, _) => "<C-e>",
        _ => "<C-f>",
    };
    for k in shrew_core::KeyPress::parse_seq(keys) {
        st.key(k);
    }
    st.poll();
    true
}

#[cfg(not(feature = "git"))]
fn scroll_diff(_: &mut State, _: isize, _: usize) -> bool {
    false
}

fn bench(st: &mut State, r: &mut Renderer, scene: &mut Scene, ui: &mut Ui, lay: &Layout) {
    let mut run = |name: &str, step: isize, st: &mut State| {
        let mut builds = Vec::new();
        let mut cpus = Vec::new();
        for i in 0..60 {
            if step != 0 && !scroll_diff(st, step, i) {
                let b = st.ed.buf_mut();
                let last = shrew_core::text::last_line(b.rope());
                let top = b.view.top as isize + step;
                b.view.top = if top as usize > last.saturating_sub(lay.rows) { 0 } else { top.max(0) as usize };
                // background highlighting keeps up between frames, as it would on screen
                st.poll();
            }
            let t = Instant::now();
            draw::build(scene, st, ui, lay, Instant::now() + Duration::from_secs(1));
            builds.push(t.elapsed());
            let _ = r.render(scene, || {});
            cpus.push(r.stats().cpu_time);
        }
        builds.sort();
        cpus.sort();
        let total: Vec<Duration> = builds.iter().zip(&cpus).map(|(a, b)| *a + *b).collect();
        let mut total = total;
        total.sort();
        eprintln!(
            "{name:<16} build {:>9.1?}  render cpu {:>9.1?}  total median {:>9.1?}  p95 {:>9.1?}",
            builds[builds.len() / 2],
            cpus[cpus.len() / 2],
            total[total.len() / 2],
            total[total.len() * 95 / 100]
        );
    };
    run("same frame", 0, st);
    run("scroll 1 line", 1, st);
    run("scroll 1 page", lay.rows as isize, st);
    let mm = &ui.minimap;
    eprintln!("minimap          {} rebuilds, last {:.1?}", mm.builds, mm.last_build);
}
