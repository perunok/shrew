//! Drawing the Settings panel, and where its parts landed ([`Hits`]) for the mouse. Only the visible rows are laid
//! out and drawn; the panel draws in layer 2 and an open list of choices in layer 3, above it (pickers opened from
//! the panel draw in layer 3 too).

use super::rows::{Kind, Row, Section};
use super::{Focus, Panel};
use crate::colors::{Colors, mix};
use crate::draw::{elide_end, elide_start, popup};
use crate::layout::Layout;
use crate::state::State;
use shrew_core::picker::Choice;
use shrew_render::{Color, Rect, Scene, TextStyle, char_columns, columns};

/// Where the panel's parts were drawn last frame.
#[derive(Clone, Debug, Default)]
pub struct Hits {
    pub panel: Rect,
    pub search: Rect,
    /// Section rows and their index into [`Section::ALL`].
    pub sections: Vec<(Rect, usize)>,
    pub rows: Vec<RowHit>,
    /// An open list's options and their index.
    pub options: Vec<(Rect, usize)>,
    pub list_row_h: f32,
    pub list_rows: usize,
    /// How far the rows column can scroll, in pixels.
    pub max_scroll: f32,
}

/// A row on screen. `control` is its switch, stepper, list button or button; numbers have their parts too.
#[derive(Clone, Copy, Debug, Default)]
pub struct RowHit {
    pub rect: Rect,
    /// Index into [`Panel::shown`].
    pub index: usize,
    pub control: Rect,
    pub minus: Rect,
    pub plus: Rect,
    pub slider: Rect,
}

/// Draws the open panel (nothing when it is closed).
pub fn draw(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors) {
    let Some(mut p) = st.settings.panel.take() else { return };
    paint(scene, &mut p, st, lay, c);
    st.settings.panel = Some(p);
}

/// Width of the control column, in cells.
const CONTROL_COLS: f32 = 30.0;

fn paint(scene: &mut Scene, p: &mut Panel, st: &State, lay: &Layout, c: &Colors) {
    scene.layer(2);
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let line = lay.scale.round().max(1.0);
    p.hits = Hits::default();

    // a large panel over everything but the status and command lines, which keep showing messages
    let area_h = lay.status.y;
    let margin = (2.0 * cw).max(4.0 * pad);
    let w = (lay.width - 2.0 * margin).min(124.0 * cw).max((lay.width - 2.0 * pad).min(44.0 * cw)).round();
    let h = (area_h - 2.0 * margin).min(46.0 * ch).max((area_h - 2.0 * pad).min(12.0 * ch)).round();
    let x = ((lay.width - w) / 2.0).round();
    let y = ((area_h - h) / 2.0).max(0.0).round();
    let panel = Rect::new(x, y, w, h);
    popup(scene, panel, lay, c);
    p.hits.panel = panel;
    scene.push_clip(panel);

    // header: the title and the search field
    let head_h = (ch + 5.0 * pad).round();
    let hy = lay.centered(y, head_h);
    scene.text(x + 3.0 * cw, hy, "Settings", TextStyle::new(c.panel_foreground).bold(), &[]);
    let sw = (36.0 * cw).min(w * 0.45).round();
    let sh = (ch + 2.0 * pad).round();
    let sbox = Rect::new(x + w - 2.0 * cw - sw, (y + (head_h - sh) / 2.0).round(), sw, sh);
    search_box(scene, p, sbox, lay, c);
    scene.rect(Rect::new(x, y + head_h, w, line), c.panel_border);

    // footer: the config file and what the keys do here
    let foot_h = (ch + 3.0 * pad).round();
    let fy = y + h - foot_h;
    scene.rect(Rect::new(x, fy, w, line), c.panel_border);
    footer(scene, p, st, Rect::new(x, fy, w, foot_h), lay, c);

    // sections
    let body = Rect::new(x, y + head_h + line, w, fy - y - head_h - line);
    let left_w = ((Section::ALL.iter().map(|s| columns(s.label())).max().unwrap_or(10) + 7) as f32 * cw).round();
    scene.rect(Rect::new(x + left_w, body.y, line, body.h), c.panel_border);
    // every section stays in view: the rows get tighter in a short window
    let item_h = ((body.h - 4.0 * pad) / Section::ALL.len() as f32).clamp(ch, ch + 2.0 * pad).floor();
    let radius = (5.0 * lay.scale).round();
    for (i, s) in Section::ALL.iter().enumerate() {
        let r = Rect::new(x + 2.0 * pad, body.y + 2.0 * pad + i as f32 * item_h, left_w - 4.0 * pad, item_h);
        if r.bottom() > body.bottom() {
            break;
        }
        let selected = i == p.section && !p.searching();
        if selected {
            scene.rounded_rect(r, radius, c.panel_selection, None);
            if p.focus == Focus::Sections {
                let bar = (3.0 * lay.scale).round().max(2.0);
                scene.rounded_rect(Rect::new(r.x, r.y + pad, bar, r.h - 2.0 * pad), bar / 2.0, c.accent, None);
            }
        }
        let fg = if selected { TextStyle::new(c.panel_foreground).bold() } else { TextStyle::new(c.panel_muted) };
        scene.text(r.x + 2.0 * cw, lay.centered(r.y, r.h), s.label(), fg, &[]);
        p.hits.sections.push((r, i));
    }

    // rows
    let view = Rect::new(x + left_w + line, body.y, w - left_w - line, body.h);
    let rx = view.x + 3.0 * cw;
    let rw = view.w - 6.0 * cw;
    rows(scene, p, st, view, rx, rw, lay, c);
    scene.pop_clip();
    if p.list.is_some() {
        list(scene, p, st, lay, c);
    }
    scene.layer(0);
}

fn search_box(scene: &mut Scene, p: &mut Panel, sbox: Rect, lay: &Layout, c: &Colors) {
    let focused = p.focus == Focus::Search;
    let line = lay.scale.round().max(1.0);
    let border = if focused { c.accent } else { c.panel_border };
    scene.rounded_rect(sbox, (5.0 * lay.scale).round(), c.background, Some((line, border)));
    p.hits.search = sbox;
    let ty = lay.centered(sbox.y, sbox.h);
    let tx = sbox.x + lay.cw;
    scene.push_clip(sbox);
    scene.text(tx, ty, "/", TextStyle::new(if focused { c.accent } else { c.panel_muted }).bold(), &[]);
    let qx = tx + 2.0 * lay.cw;
    if p.query.is_empty() && !focused {
        scene.text(qx, ty, "Search settings", TextStyle::new(c.panel_muted), &[]);
    } else {
        let room = ((sbox.w - 4.0 * lay.cw) / lay.cw) as usize;
        let q = elide_start(&p.query, room);
        scene.text(qx, ty, &q, TextStyle::new(c.foreground), &[]);
        if focused {
            let bar = (2.0 * lay.scale).round().max(1.0);
            scene.rect(Rect::new((qx + columns(&q) as f32 * lay.cw).round(), ty, bar, lay.ch), c.cursor);
        }
    }
    scene.pop_clip();
}

fn footer(scene: &mut Scene, p: &Panel, st: &State, bar: Rect, lay: &Layout, c: &Colors) {
    let cw = lay.cw;
    let y = lay.centered(bar.y, bar.h);
    let kind = p.current().map(|i| &p.rows[i].kind);
    let hint = if p.list.is_some() {
        "j/k move   Enter choose   Esc cancel"
    } else {
        match p.focus {
            Focus::Search => "type to search   Enter results   Esc cancel",
            Focus::Sections => "j/k section   l/Enter rows   / search   R reset section   Esc close",
            Focus::Rows => match kind {
                Some(Kind::Toggle { .. }) => "j/k move   Space toggle   r reset   h sections   / search   Esc close",
                Some(Kind::Number { .. }) => "j/k move   ←/→ or -/+ change   r reset   h sections   Esc close",
                Some(Kind::Pick { .. } | Kind::Options { .. }) => {
                    "j/k move   Enter choose   r reset   h sections   / search   Esc close"
                }
                Some(Kind::Action { .. }) => "j/k move   Enter run   h sections   / search   Esc close",
                _ => "j/k move   h sections   / search   Esc close",
            },
        }
    };
    // a narrow panel keeps the last hints (Esc close always shows)
    let fit = ((bar.w / cw) as usize).saturating_sub(6);
    let mut hint = hint;
    while columns(hint) > fit
        && let Some((_, rest)) = hint.split_once("   ")
    {
        hint = rest;
    }
    let hx = bar.right() - 3.0 * cw - columns(hint) as f32 * cw;
    scene.text(hx.round(), y, hint, TextStyle::new(c.panel_muted), &[]);
    let room = ((hx - bar.x) / cw - 6.0).max(0.0) as usize;
    if room >= 12 {
        let path = st.config_path.as_ref().map_or_else(|| "not saved (no config file)".into(), |p| tilde(p));
        scene.text(bar.x + 3.0 * cw, y, &elide_start(&path, room), TextStyle::new(c.panel_muted), &[]);
    }
}

/// `~/...` for paths in the home directory.
fn tilde(p: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home.as_deref().and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// One laid-out row: where it starts in the scrolled content, its height and its description lines.
struct Placed {
    index: usize,
    y: f32,
    h: f32,
    desc: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn rows(scene: &mut Scene, p: &mut Panel, st: &State, view: Rect, rx: f32, rw: f32, lay: &Layout, c: &Colors) {
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let rows = p.rows.clone();
    let shown = p.shown();
    let ctl_w = (CONTROL_COLS * cw).min(rw * 0.42).round();
    let label_cols = (((rw - ctl_w) / cw).floor() as usize).saturating_sub(2).max(8);
    let full_cols = ((rw / cw).floor() as usize).max(8);

    // the header: the section (or the search) and what it is about
    let (title, about) = if p.searching() {
        let n = shown.len();
        (format!("{n} result{} for \u{201c}{}\u{201d}", if n == 1 { "" } else { "s" }, p.query.trim()), String::new())
    } else {
        (p.section().label().to_string(), p.section().description().to_string())
    };
    let about = wrap(&about, full_cols);
    let top = 3.0 * pad;
    let head_h = ch * (1 + about.len()) as f32 + 3.0 * pad;

    // lay out every shown row (cheap: a section has a few dozen)
    let mut placed = Vec::with_capacity(shown.len());
    let mut cy = top + head_h;
    for (n, &i) in shown.iter().enumerate() {
        let r = &rows[i];
        let mut d = r.description.clone();
        if p.searching() {
            d = if d.is_empty() { r.section.label().to_string() } else { format!("{} · {d}", r.section.label()) };
        }
        // read-only rows without a value have the whole width for their text
        let wide = match &r.kind {
            Kind::Info { get } => get(st).is_empty(),
            _ => false,
        };
        let desc = wrap(&d, if wide { full_cols } else { label_cols });
        let h = match r.kind {
            Kind::Heading => ch + 3.0 * pad,
            _ if desc.is_empty() => ch + 2.0 * pad,
            _ => ch * (1 + desc.len()) as f32 + 3.0 * pad,
        };
        placed.push(Placed { index: n, y: cy, h, desc });
        cy += h;
    }
    let content_h = cy + 2.0 * pad;
    p.hits.max_scroll = (content_h - view.h).max(0.0);
    if p.follow
        && let Some(r) = placed.get(p.row)
    {
        // the selected row in view, with a little room around it (the first one shows the header too)
        let lo = if p.row == 0 { 0.0 } else { r.y - ch };
        let hi = r.y + r.h + ch - view.h;
        p.scroll = p.scroll.max(hi).min(lo);
    }
    p.scroll = p.scroll.clamp(0.0, p.hits.max_scroll).round();
    let oy = view.y - p.scroll;

    scene.push_clip(view);
    let ty = (oy + top).round();
    scene.text(rx, ty, &title, TextStyle::new(c.panel_foreground).bold(), &[]);
    for (k, l) in about.iter().enumerate() {
        scene.text(rx, ty + (k + 1) as f32 * ch, l, TextStyle::new(c.panel_muted), &[]);
    }
    if shown.is_empty() {
        let msg = if p.searching() { "Nothing matches. Try fewer or other words." } else { "Nothing here." };
        scene.text(rx, (oy + top + head_h).round(), msg, TextStyle::new(c.panel_muted), &[]);
    }
    let radius = (5.0 * lay.scale).round();
    for pl in &placed {
        let ry = (oy + pl.y).round();
        if ry > view.bottom() || ry + pl.h < view.y {
            continue;
        }
        let i = shown[pl.index];
        let row = &rows[i];
        let rect = Rect::new(rx - 1.5 * cw, ry, rw + 3.0 * cw, pl.h.round());
        let selected = pl.index == p.row && p.focus != Focus::Sections;
        if pl.index == p.row && !matches!(row.kind, Kind::Heading) {
            let col = if selected { c.panel_selection } else { mix(c.panel_selection, c.popup, 0.5) };
            scene.rounded_rect(rect, radius, col, None);
            if selected && p.focus == Focus::Rows {
                let bar = (3.0 * lay.scale).round().max(2.0);
                scene.rounded_rect(Rect::new(rect.x, rect.y + pad, bar, rect.h - 2.0 * pad), bar / 2.0, c.accent, None);
            }
        }
        let ly = (ry + if pl.desc.is_empty() { pad } else { 1.5 * pad }).round();
        let enabled = row.editable(st) || matches!(row.kind, Kind::Info { .. } | Kind::Heading);
        let label_style = match row.kind {
            Kind::Heading => TextStyle::new(c.accent).bold(),
            _ if enabled => TextStyle::new(c.panel_foreground),
            _ => TextStyle::new(c.panel_muted),
        };
        let hy = if matches!(row.kind, Kind::Heading) { ly + pad } else { ly };
        scene.text(rx, hy, &elide_end(&row.label, label_cols), label_style, &[]);
        for (k, d) in pl.desc.iter().enumerate() {
            scene.text(rx, ly + (k + 1) as f32 * ch, d, TextStyle::new(c.panel_muted), &[]);
        }
        let label_w = (columns(&row.label).min(label_cols) + 2) as f32 * cw;
        let ctl = Rect::new(rx + rw - ctl_w, ly, ctl_w, ch);
        let mut hit = RowHit { rect: rect.intersect(&view), index: pl.index, ..RowHit::default() };
        hit.control = control(scene, p, st, row, i, ctl, rx + label_w, lay, c, &mut hit);
        p.hits.rows.push(hit);
    }
    scene.pop_clip();

    // a thin scrollbar when the rows do not all fit
    if p.hits.max_scroll > 0.0 {
        let bw = (3.0 * lay.scale).round().max(2.0);
        let th = (view.h * view.h / content_h).max(2.0 * ch).min(view.h);
        let ty = view.y + (view.h - th) * p.scroll / p.hits.max_scroll;
        let sx = view.right() - 1.5 * cw;
        scene.rounded_rect(Rect::new(sx.round(), ty.round(), bw, th.round()), bw / 2.0, c.scrollbar, None);
    }
}

/// Draws a row's control right-aligned in `ctl` (one text line high) and returns its clickable rect. `free_x`: where
/// the label ends, for read-only values that may run wider than the control column.
#[allow(clippy::too_many_arguments)]
fn control(
    scene: &mut Scene,
    p: &Panel,
    st: &State,
    row: &Row,
    i: usize,
    ctl: Rect,
    free_x: f32,
    lay: &Layout,
    c: &Colors,
    hit: &mut RowHit,
) -> Rect {
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let line = lay.scale.round().max(1.0);
    let right = ctl.right();
    let radius = (5.0 * lay.scale).round();
    match &row.kind {
        Kind::Toggle { get, .. } => match get(st) {
            None => {
                let s = "n/a";
                scene.text(right - columns(s) as f32 * cw, ctl.y, s, TextStyle::new(c.panel_muted), &[]);
                Rect::default()
            }
            Some(on) => {
                let th = (ch * 0.8).round();
                let tw = (th * 1.85).round();
                let track = Rect::new(right - tw, (ctl.y + (ch - th) / 2.0).round(), tw, th);
                let off = mix(c.panel_muted, c.popup, 0.45);
                scene.rounded_rect(track, th / 2.0, if on { c.accent } else { off }, None);
                let inset = (2.0 * lay.scale).round().max(1.0);
                let k = th - 2.0 * inset;
                let kx = if on { track.right() - inset - k } else { track.x + inset };
                scene.rounded_rect(
                    Rect::new(kx, track.y + inset, k, k),
                    k / 2.0,
                    if on { c.accent_fg } else { c.popup },
                    None,
                );
                let label = if on { "On" } else { "Off" };
                let lx = track.x - (columns(label) + 1) as f32 * cw;
                scene.text(lx.round(), ctl.y, label, TextStyle::new(c.panel_muted), &[]);
                Rect::new(lx - cw, ctl.y - pad, right - lx + cw, ch + 2.0 * pad)
            }
        },
        Kind::Number { get, range: (lo, hi), decimals, unit, .. } => {
            let v = get(st);
            let bw = 3.0 * cw;
            let bh = (ch + pad).round();
            let by = (ctl.y + (ch - bh) / 2.0).round();
            let plus = Rect::new((right - bw).round(), by, bw.round(), bh);
            let value_w = 10.0 * cw;
            let minus = Rect::new((plus.x - value_w - bw).round(), by, bw.round(), bh);
            for (r, s) in [(minus, "−"), (plus, "+")] {
                scene.rounded_rect(r, radius, c.panel_selection, Some((line, c.panel_border)));
                let tx = r.x + (r.w - columns(s) as f32 * cw) / 2.0;
                scene.text(tx.round(), ctl.y, s, TextStyle::new(c.panel_foreground), &[]);
            }
            let text =
                format!("{v:.prec$}{}{unit}", if unit.is_empty() || *unit == "×" { "" } else { " " }, prec = *decimals);
            let tx = minus.right() + (value_w - columns(&text) as f32 * cw) / 2.0;
            scene.text(tx.round(), ctl.y, &text, TextStyle::new(c.panel_foreground).bold(), &[]);
            // the slider fills what is left of the control column
            let sx = ctl.x;
            let sw = minus.x - cw - sx;
            if sw >= 6.0 * cw {
                let track_h = (4.0 * lay.scale).round().max(2.0);
                let track = Rect::new(sx, (ctl.y + (ch - track_h) / 2.0).round(), sw, track_h);
                let f = (((v - lo) / (hi - lo).max(f64::EPSILON)) as f32).clamp(0.0, 1.0);
                scene.rounded_rect(track, track_h / 2.0, mix(c.panel_muted, c.popup, 0.35), None);
                scene.rounded_rect(
                    Rect::new(track.x, track.y, (sw * f).round(), track_h),
                    track_h / 2.0,
                    c.accent,
                    None,
                );
                let k = (ch * 0.6).round();
                let knob = Rect::new((track.x + sw * f - k / 2.0).round(), (ctl.y + (ch - k) / 2.0).round(), k, k);
                scene.rounded_rect(knob, k / 2.0, c.accent, Some((line, c.popup)));
                hit.slider = Rect::new(track.x - k / 2.0, ctl.y - pad, sw + k, ch + 2.0 * pad);
            }
            hit.minus = minus;
            hit.plus = plus;
            Rect::new(ctl.x - cw, by, right - ctl.x + cw, bh)
        }
        Kind::Pick { choice, show, .. } => {
            let swatches: Vec<Color> = match choice {
                Choice::Theme => vec![c.background, c.foreground, c.accent, c.accent_alt, c.selection],
                Choice::Accent => vec![c.accent],
                Choice::Font | Choice::GitCommit | Choice::GitRef => Vec::new(),
            };
            dropdown(scene, &show(st), &swatches, ctl, lay, c)
        }
        Kind::Options { show, .. } => {
            let r = dropdown(scene, &show(st), &[], ctl, lay, c);
            if p.list.as_ref().is_some_and(|l| l.row == i) {
                scene.rounded_rect(r, radius, Color::TRANSPARENT, Some((line, c.accent)));
            }
            r
        }
        Kind::Action { button, .. } => {
            let w = ((columns(button) + 4) as f32 * cw).round();
            let bh = (ch + pad).round();
            let r = Rect::new(right - w, (ctl.y + (ch - bh) / 2.0).round(), w, bh);
            let (bg, fg) = if p.armed == Some(i) { (c.error, c.error_fg) } else { (c.accent, c.accent_fg) };
            scene.rounded_rect(r, radius, bg, None);
            scene.text((r.x + 2.0 * cw).round(), ctl.y, button, TextStyle::new(fg).bold(), &[]);
            r
        }
        Kind::Info { get } => {
            let v = get(st);
            if !v.is_empty() {
                let room = ((right - free_x) / cw).max(0.0) as usize;
                let v = if v.contains(std::path::MAIN_SEPARATOR) {
                    elide_start(&v, room)
                } else {
                    elide_end(&v, room).into()
                };
                let x = right - columns(&v) as f32 * cw;
                scene.text(x.round(), ctl.y, &v, TextStyle::new(c.panel_muted), &[]);
            }
            Rect::default()
        }
        Kind::Heading => Rect::default(),
    }
}

/// A button that opens a list: the value, an arrow, and optional color swatches.
fn dropdown(scene: &mut Scene, value: &str, swatches: &[Color], ctl: Rect, lay: &Layout, c: &Colors) -> Rect {
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let line = lay.scale.round().max(1.0);
    let bh = (ch + pad).round();
    let r = Rect::new(ctl.x, (ctl.y + (ch - bh) / 2.0).round(), ctl.w, bh);
    scene.rounded_rect(r, (5.0 * lay.scale).round(), c.background, Some((line, c.panel_border)));
    let mut x = r.x + cw;
    let sw = (ch * 0.6).round();
    for &col in swatches {
        let s = Rect::new(x.round(), (ctl.y + (ch - sw) / 2.0).round(), sw, sw);
        scene.rounded_rect(s, (2.0 * lay.scale).round(), col, Some((line, c.panel_border)));
        x += sw + (2.0 * lay.scale).round();
    }
    if !swatches.is_empty() {
        x += cw;
    }
    let room = ((r.right() - x) / cw - 3.0).max(1.0) as usize;
    scene.text(x.round(), ctl.y, &elide_end(value, room), TextStyle::new(c.foreground), &[]);
    let arrow = "▾";
    scene.text((r.right() - 2.0 * cw).round(), ctl.y, arrow, TextStyle::new(c.panel_muted), &[]);
    r
}

/// The list of choices an options row opened, under (or over) its button.
fn list(scene: &mut Scene, p: &mut Panel, st: &State, lay: &Layout, c: &Colors) {
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let rows = p.rows.clone();
    let anchor = p.hits.rows.iter().find(|h| p.shown().get(h.index).copied() == p.list.as_ref().map(|l| l.row));
    let anchor = anchor.map(|h| h.control).filter(|r| r.w > 0.0).unwrap_or(Rect::new(
        p.hits.panel.x + p.hits.panel.w / 2.0,
        p.hits.panel.y + p.hits.panel.h / 3.0,
        CONTROL_COLS * cw,
        ch,
    ));
    let Some(l) = &mut p.list else { return };
    let current = match &rows[l.row].kind {
        Kind::Options { get, .. } => get(st),
        _ => String::new(),
    };
    let row_h = (ch + pad).round();
    let longest = l.options.iter().map(|o| columns(&o.1)).max().unwrap_or(8);
    let w = ((longest + 6) as f32 * cw).max(anchor.w).min(p.hits.panel.w - 4.0 * cw).round();
    let panel = p.hits.panel;
    let below = panel.bottom() - anchor.bottom() - 2.0 * pad;
    let above = anchor.y - panel.y - 2.0 * pad;
    let room = below.max(above);
    let n = l.options.len().clamp(1, ((room - 2.0 * pad) / row_h).floor().max(1.0) as usize).min(12);
    let h = n as f32 * row_h + 2.0 * pad;
    let x = (anchor.right() - w).max(panel.x + cw).round();
    let y = if below >= h || below >= above { anchor.bottom() + pad } else { anchor.y - pad - h };
    let r = Rect::new(x, y.round(), w, h.round());
    if l.selected < l.top {
        l.top = l.selected;
    } else if l.selected >= l.top + n {
        l.top = l.selected + 1 - n;
    }
    l.top = l.top.min(l.options.len().saturating_sub(n));
    scene.layer(3);
    popup(scene, r, lay, c);
    scene.push_clip(r);
    for (k, (value, label)) in l.options.iter().enumerate().skip(l.top).take(n) {
        let oy = r.y + pad + (k - l.top) as f32 * row_h;
        let o = Rect::new(r.x + pad, oy, r.w - 2.0 * pad, row_h);
        if k == l.selected {
            scene.rounded_rect(o, (4.0 * lay.scale).round(), c.panel_selection, None);
        }
        let ty = lay.centered(oy, row_h);
        let mark = if *value == current { "•" } else { " " };
        scene.text(o.x + cw, ty, mark, TextStyle::new(c.accent).bold(), &[]);
        let fg = if *value == current { c.accent } else { c.panel_foreground };
        let room = ((o.w / cw) as usize).saturating_sub(4);
        scene.text(o.x + 3.0 * cw, ty, &elide_end(label, room), TextStyle::new(fg), &[]);
        p.hits.options.push((o, k));
    }
    scene.pop_clip();
    p.hits.list_row_h = row_h;
    p.hits.list_rows = n;
}

/// Word-wraps `text` to `cols` columns (a word longer than a line is cut).
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    if text.trim().is_empty() {
        return out;
    }
    let cols = cols.max(4);
    let mut line = String::new();
    let mut w = 0;
    for word in text.split_whitespace() {
        let ww = columns(word);
        if w > 0 && w + 1 + ww > cols {
            out.push(std::mem::take(&mut line));
            w = 0;
        }
        if w > 0 {
            line.push(' ');
            w += 1;
        }
        for ch in word.chars() {
            if w + char_columns(ch) > cols {
                break;
            }
            line.push(ch);
            w += char_columns(ch);
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}
