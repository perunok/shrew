//! `--screenshot`: renders one frame without a window, after typing `--keys`, and saves it as a PNG. Used for
//! automated visual tests and the README screenshots. It uses the built-in defaults (no config.toml, no vimrc or
//! Sublime import) so the output is the same on every machine, and never writes the config.
//!
//! With `--stats` it also measures frame cost: the same frame redrawn (warm caches), one line scrolled per frame,
//! and a whole page scrolled per frame (every line shaped anew).

use crate::cli::Cli;
use crate::draw::{self, Ui};
use crate::layout::Layout;
use crate::state::State;
use shrew_core::KeyPress;
use shrew_core::features::Feature;
use shrew_render::{Renderer, Scene};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn run(cli: &Cli, out: &std::path::Path) -> Result<(), String> {
    let t0 = Instant::now();
    let mut st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
    st.config_path = None;
    st.open_request(crate::launch::request(cli, false)?, None);
    let (w, h) = cli.size;
    let mut r = Renderer::headless(w, h, cli.scale, st.font.clone()).map_err(|e| format!("renderer: {e}"))?;
    r.wait_for_fonts(Duration::from_secs(5));
    r.poll_fonts();
    let mut ui = Ui::new();
    ui.animate = false;
    let mut scene = Scene::new();
    let layout = |r: &Renderer, st: &State| {
        let ed = &st.ed;
        Layout::new(
            &r.metrics(),
            r.size(),
            r.scale(),
            ed.buf().doc.rope.len_lines(),
            ed.features.on(Feature::LineNumbers),
            ed.buffers.len() > 1,
        )
    };
    // a first layout pass sizes the views, so keys like Ctrl+D scroll by the real page
    let lay = layout(&r, &st);
    draw::build(&mut scene, &mut st, &mut ui, &lay, Instant::now());
    for k in KeyPress::parse_seq(&cli.keys) {
        st.key(k);
        if st.font_changed {
            st.font_changed = false;
            r.set_font(st.font.clone());
        }
    }
    settle(&mut st, &mut r, &mut scene, &mut ui, &layout);
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
    if !st.ed.features.on(Feature::Syntax) {
        return false;
    }
    !s.is_ready() || st.highlighters.values().any(|h| !h.is_settled())
}

#[cfg(not(feature = "syntax"))]
fn syntax_busy(_: &State) -> bool {
    false
}

fn bench(st: &mut State, r: &mut Renderer, scene: &mut Scene, ui: &mut Ui, lay: &Layout) {
    let mut run = |name: &str, step: isize, st: &mut State| {
        let mut builds = Vec::new();
        let mut cpus = Vec::new();
        for _ in 0..60 {
            if step != 0 {
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
}
