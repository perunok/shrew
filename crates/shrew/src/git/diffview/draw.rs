//! Drawing a diff tab in the editor's place: a header bar (what is compared, the totals, the layout, white space
//! and context switches, the actions), then only the rows on screen, then an overview of every change in the
//! scrollbar. Colors come from the theme (`diff_*`, `git_*`); syntax colors from shrew-syntax.

use super::view::{Button, DiffView, Section, Side};
use super::{Rev, is_active};
use crate::colors::{Colors, mix};
use crate::draw::Ui;
use crate::layout::{Layout, Scroll};
use crate::state::State;
use shrew_core::diffview::{Layout as Lay, NONE, Row, RowKind};
use shrew_core::features::Feature;
use shrew_git::FileState;
use shrew_render::{Color, Rect, Scene, TextStyle, char_columns, columns};
use std::ops::Range;
use std::time::Instant;

/// Fills the editor area with the active diff tab. Returns true while its scroll animation runs.
pub fn draw(scene: &mut Scene, st: &mut State, ui: &mut Ui, lay: &Layout, c: &Colors, now: Instant) -> bool {
    if !is_active(st) {
        return false;
    }
    let buf = st.ed.buf().id;
    let Some(mut v) = st.git.diff.views.remove(&buf) else { return false };
    if v.stale {
        v.rebuild();
    }
    let animating = paint(scene, st, &mut v, ui, lay, c, now);
    st.git.diff.views.insert(buf, v);
    animating
}

/// Search matches in a line, as byte ranges.
type Finder<'a> = dyn Fn(&str) -> Vec<(usize, usize)> + 'a;

/// The first and last line of a side on screen.
#[cfg(feature = "syntax")]
type LineSpan = Option<(u32, u32)>;

/// Pane geometry of one frame.
struct Geo {
    cw: f32,
    ch: f32,
    scale: f32,
    body: Rect,
    /// Split: the old pane, the gutter between, the new pane. Unified: `new` spans the text, `old` is empty.
    old: Rect,
    mid: Rect,
    new: Rect,
    /// Width of a line number column.
    num_w: f32,
    first: usize,
    y0: f32,
    left: f32,
    tw: usize,
}

impl Geo {
    fn row_y(&self, row: usize) -> f32 {
        self.y0 + (row - self.first) as f32 * self.ch
    }

    /// Where a pane's text starts (after its numbers; unified: after both numbers and the sign).
    fn text_x(&self, pane: Rect, unified: bool) -> f32 {
        if unified { pane.x + 2.0 * self.num_w + 2.0 * self.cw } else { pane.x + self.num_w + self.cw }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint(scene: &mut Scene, st: &State, v: &mut DiffView, ui: &mut Ui, lay: &Layout, c: &Colors, now: Instant) -> bool {
    let area = lay.text;
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let unified = v.opts.layout == Lay::Unified;
    scene.rect(area, c.background);
    v.hits.buttons.clear();
    v.hits.actions.clear();

    // header: what is compared, the totals, the switches
    let head_h = (ch + 2.0 * pad).round();
    let sub_h = if unified { 0.0 } else { (ch + pad).round() };
    header(scene, v, Rect::new(area.x, area.y, area.w, head_h), lay, c);
    let sb_w = lay.scrollbar.w.max((10.0 * lay.scale).round());
    let body = Rect::new(area.x, area.y + head_h + sub_h, (area.w - sb_w).max(cw), (area.h - head_h - sub_h).max(ch));
    let page = ((body.h / ch).floor() as usize).max(1);
    v.hits.page = page;
    v.clamp_top();

    // smooth scrolling, as the editor does it
    let target = crate::anim::Target {
        buffer: v.id,
        scroll: Scroll { top: v.top as f32, left: v.left_col as f32 },
        cursor: (v.cursor as f32, 0.0),
        rows: page,
        smooth_scroll: ui.animate && st.ed.features.on(Feature::SmoothScroll),
        cursor_animation: false,
    };
    let animating = v.anim.step(&target, now);
    let scroll = v.anim.scroll;
    let first = scroll.first_line().min(v.rows.len().saturating_sub(1));
    let y0 = body.y - scroll.y_offset(ch);

    let digits = v
        .sections
        .iter()
        .filter_map(|s| s.data.as_ref())
        .map(|d| d.old.len_lines().max(d.new.len_lines()))
        .max()
        .unwrap_or(1)
        .max(1)
        .ilog10() as usize
        + 1;
    let num_w = ((digits.max(3) + 1) as f32 * cw).round();
    let (old, mid, new) = if unified {
        (Rect::new(body.x, body.y, 0.0, body.h), Rect::new(body.x, body.y, 0.0, body.h), body)
    } else {
        let mid_w = (3.0 * cw).round();
        let pane = ((body.w - mid_w) / 2.0).floor();
        (
            Rect::new(body.x, body.y, pane, body.h),
            Rect::new(body.x + pane, body.y, mid_w, body.h),
            Rect::new(body.x + pane + mid_w, body.y, body.w - pane - mid_w, body.h),
        )
    };
    let tw = st.ed.settings.tab_width.max(1);
    let g = Geo { cw, ch, scale: lay.scale, body, old, mid, new, num_w, first, y0, left: scroll.left, tw };
    v.hits.body = body;
    v.hits.first = first;
    v.hits.y0 = y0;
    v.hits.row_h = ch;
    v.hits.old_pane = old;
    v.hits.new_pane = new;

    if !unified {
        pane_labels(scene, v, Rect::new(area.x, area.y + head_h, area.w, sub_h), &g, c);
    }
    if v.rows.is_empty() {
        let msg = v.message.clone().unwrap_or_else(|| "no changes".into());
        let x = (body.x + (body.w - columns(&msg) as f32 * cw) / 2.0).round();
        let y = (body.y + body.h / 3.0).round();
        scene.text(x, y, &msg, TextStyle::new(c.panel_muted), &[]);
        v.hits.scrollbar = Rect::default();
        return animating;
    }

    let end = (first + page + 2).min(v.rows.len());
    #[cfg(feature = "syntax")]
    prepare_syntax(st, v, first..end);
    scene.push_clip(body);
    let sel = v.anchor.map(|a| a.min(v.cursor)..a.max(v.cursor) + 1);
    let re = st.ed.search.as_ref().filter(|_| st.ed.search_visible && st.ed.features.on(Feature::SearchHighlight));
    let find = |s: &str| -> Vec<(usize, usize)> {
        re.map(|re| re.find_iter(s).take(256).map(|m| (m.start(), m.end())).collect()).unwrap_or_default()
    };
    let search: Option<&Finder<'_>> = if re.is_some() { Some(&find) } else { None };
    let mut lb = LineBuf::default();
    for row in first..end {
        let r = v.rows.rows[row];
        let y = g.row_y(row);
        let cursor = row == v.cursor;
        match r.kind {
            RowKind::Same | RowKind::Change => {
                line_row(scene, st, v, &g, r, y, cursor, unified, search, &mut lb, c);
            }
            RowKind::Fold => fold_row(scene, &g, r, y, cursor, ui.focused, c),
            RowKind::File => file_row(scene, &v.sections[r.file as usize], &g, y, row, cursor, c),
            RowKind::Note => {
                let s = &v.sections[r.file as usize];
                let note = s.note.as_deref().unwrap_or("loading…");
                let x = (body.x + 2.0 * cw + if v.multi { 2.0 * cw } else { 0.0 }).round();
                if cursor {
                    scene.rect(Rect::new(body.x, y, body.w, ch), c.current_line);
                }
                scene.text(x, y, note, TextStyle::new(c.panel_muted).italic(), &[]);
            }
        }
        if let Some(sel) = &sel
            && sel.contains(&row)
        {
            let col = c.selection;
            scene.rect(Rect::new(body.x, y, body.w, ch), Color::rgba(col.r, col.g, col.b, 0x90));
            let bar = (3.0 * lay.scale).round().max(2.0);
            scene.rect(Rect::new(body.x, y, bar, ch), c.accent);
        }
    }
    // the hunk actions in the gutter between the panes
    if !unified {
        gutter_marks(scene, v, &g, first..end, c);
    }
    scene.pop_clip();
    overview(scene, v, Rect::new(body.right(), body.y, sb_w, body.h), page, scroll, c, lay);
    animating
}

// ----- header -----

/// Laying out the header's buttons from the right edge.
struct Pills {
    right: f32,
    y: f32,
    top: f32,
    h: f32,
    r: f32,
    cw: f32,
}

impl Pills {
    #[allow(clippy::too_many_arguments)]
    fn pill(
        &mut self,
        scene: &mut Scene,
        hits: &mut Vec<(Rect, Button)>,
        c: &Colors,
        label: &str,
        b: Button,
        on: bool,
        tone: Option<Color>,
    ) {
        let w = (columns(label) as f32 + 2.0) * self.cw;
        self.right -= w;
        let rect = Rect::new(self.right.round(), self.top, w.round(), self.h);
        let (bg, fg) = match (on, tone) {
            (true, _) => (c.accent, c.accent_fg),
            (false, Some(t)) => (mix(t, c.panel_background, 0.16), t),
            (false, None) => (c.panel_selection, c.panel_foreground),
        };
        scene.rounded_rect(rect, self.r, bg, None);
        scene.text((self.right + self.cw).round(), self.y, label, TextStyle::new(fg), &[]);
        hits.push((rect, b));
        self.gap(0.5);
    }

    fn label(&mut self, scene: &mut Scene, s: &str, col: Color) {
        self.right -= columns(s) as f32 * self.cw;
        scene.text(self.right.round(), self.y, s, TextStyle::new(col), &[]);
        self.gap(0.5);
    }

    /// Space, in cells (negative: closer).
    fn gap(&mut self, cells: f32) {
        self.right -= cells * self.cw;
    }
}

fn header(scene: &mut Scene, v: &mut DiffView, bar: Rect, lay: &Layout, c: &Colors) {
    let (cw, pad) = (lay.cw, lay.pad);
    scene.rect(bar, c.panel_background);
    scene.rect(
        Rect::new(bar.x, bar.bottom() - lay.scale.round().max(1.0), bar.w, lay.scale.round().max(1.0)),
        c.panel_border,
    );
    let y = lay.centered(bar.y, bar.h);
    let r = (4.0 * lay.scale).round();
    let pill_h = bar.h - 2.0 * pad;

    // the right side first, from the right edge: actions, context, white space, layout, navigation
    let mut bar_x = Pills { right: bar.right() - 2.0 * pad, y, top: bar.y + pad, h: pill_h, r, cw };
    let can = v.actions();
    let hits = &mut v.hits.buttons;
    if can.revert {
        bar_x.pill(scene, hits, c, "Revert X", Button::Revert, false, Some(c.git_deleted));
    }
    if can.unstage {
        bar_x.pill(scene, hits, c, "Unstage u", Button::Unstage, false, Some(c.git_modified));
    }
    if can.stage {
        bar_x.pill(scene, hits, c, "Stage s", Button::Stage, false, Some(c.git_added));
    }
    bar_x.gap(0.75);
    let ctx = if v.opts.context == shrew_core::diffview::ALL { "all".to_string() } else { v.opts.context.to_string() };
    bar_x.pill(scene, hits, c, "+", Button::MoreContext, false, None);
    bar_x.label(scene, &format!("±{ctx}"), c.panel_muted);
    bar_x.pill(scene, hits, c, "−", Button::LessContext, false, None);
    bar_x.gap(0.75);
    bar_x.pill(scene, hits, c, "ignore space", Button::Whitespace, v.ignore_ws, None);
    bar_x.gap(0.75);
    let unified = v.opts.layout == Lay::Unified;
    bar_x.pill(scene, hits, c, "Unified", Button::Unified, unified, None);
    bar_x.gap(-0.5);
    bar_x.pill(scene, hits, c, "Split", Button::Split, !unified, None);
    bar_x.gap(0.75);
    bar_x.pill(scene, hits, c, "▼", Button::NextChange, false, None);
    bar_x.pill(scene, hits, c, "▲", Button::PrevChange, false, None);
    let right = bar_x.right;

    // the left side: what is compared, and the totals
    let left_end = right - cw;
    scene.push_clip(Rect::new(bar.x, bar.y, (left_end - bar.x).max(0.0), bar.h));
    let mut x = bar.x + 2.0 * pad;
    let text = |scene: &mut Scene, s: &str, style: TextStyle, x: &mut f32| {
        scene.text(x.round(), y, s, style, &[]);
        *x += columns(s) as f32 * cw;
    };
    let (l, rr) = (v.left.as_ref().unwrap_or(&v.spec.left).label(), v.right.as_ref().unwrap_or(&v.spec.right).label());
    if v.multi {
        let what = match (&v.spec.left, &v.spec.right) {
            (Rev::Index, Rev::Worktree) => "Changes".to_string(),
            (Rev::Head, Rev::Index) => "Staged".to_string(),
            (Rev::Parent(_), r) => format!("Commit {}", v.right.as_ref().unwrap_or(r).label()),
            _ => "Compare".to_string(),
        };
        text(scene, &what, TextStyle::new(c.panel_foreground).bold(), &mut x);
        x += cw;
        if let Some(sum) = &v.summary {
            text(scene, sum, TextStyle::new(c.panel_foreground), &mut x);
            x += cw;
        }
    } else if let Some(s) = v.sections.first() {
        text(scene, &s.path, TextStyle::new(c.panel_foreground).bold(), &mut x);
        x += cw;
    }
    // a commit names itself; the panes' labels show its parent
    if !matches!(v.spec.left, Rev::Parent(_)) {
        text(scene, &format!("{l} ↔ {rr}"), TextStyle::new(c.panel_muted), &mut x);
        x += 2.0 * cw;
    }
    let (a, d) = v.total_stats();
    if a + d > 0 || !v.sections.is_empty() {
        text(scene, &format!("+{a}"), TextStyle::new(c.git_added), &mut x);
        x += cw;
        text(scene, &format!("−{d}"), TextStyle::new(c.git_deleted), &mut x);
        x += cw;
        let changes = v.rows.starts.len();
        let mut tail = if changes == 1 { "· 1 change".to_string() } else { format!("· {changes} changes") };
        if v.multi {
            let n = v.sections.len();
            tail += &if n == 1 { " · 1 file".to_string() } else { format!(" · {n} files") };
        }
        if v.ignore_ws {
            tail += " · white space ignored";
        }
        text(scene, &tail, TextStyle::new(c.panel_muted), &mut x);
    }
    scene.pop_clip();
}

/// Split layout: which version each pane shows.
fn pane_labels(scene: &mut Scene, v: &DiffView, bar: Rect, g: &Geo, c: &Colors) {
    scene.rect(bar, mix(c.panel_background, c.background, 0.5));
    let y = (bar.y + (bar.h - g.ch) / 2.0).round();
    let single = (!v.multi).then(|| v.sections.first()).flatten();
    for (pane, rev, path, side) in [
        (g.old, v.left.as_ref().unwrap_or(&v.spec.left), single.and_then(|s| s.old_path.clone()), Side::Old),
        (g.new, v.right.as_ref().unwrap_or(&v.spec.right), None, Side::New),
    ] {
        let mut label = rev.label();
        if let Some(p) = path {
            label = format!("{label} · {p}");
        }
        let focused = v.side == side;
        let col = if focused { c.panel_foreground } else { c.panel_muted };
        let x = (pane.x + g.num_w + g.cw).round();
        scene.push_clip(Rect::new(pane.x, bar.y, pane.w, bar.h));
        scene.text(x, y, &label, TextStyle::new(col), &[]);
        scene.pop_clip();
    }
}

// ----- rows -----

#[allow(clippy::too_many_arguments)]
fn line_row(
    scene: &mut Scene,
    st: &State,
    v: &mut DiffView,
    g: &Geo,
    r: Row,
    y: f32,
    cursor: bool,
    unified: bool,
    search: Option<&Finder<'_>>,
    lb: &mut LineBuf,
    c: &Colors,
) {
    let change = r.kind == RowKind::Change;
    let s = &mut v.sections[r.file as usize];
    let words = r.hunk().and_then(|k| super::words(s, k));
    let filler = mix(c.panel_background, c.background, 0.55);
    let s = &v.sections[r.file as usize];
    let Some(d) = &s.data else { return };
    let hl_live = s.live.as_ref().filter(|l| l.buffer != u64::MAX).map(|l| l.buffer);
    let sides: &[(Side, u32)] = if unified {
        if r.old != NONE && r.new == NONE { &[(Side::Old, r.old)] } else { &[(Side::New, r.new)] }
    } else {
        &[(Side::Old, r.old), (Side::New, r.new)]
    };
    for &(side, line) in sides {
        let pane = if unified || side == Side::New { g.new } else { g.old };
        let row_rect = Rect::new(pane.x, y, pane.w, g.ch);
        let (bg, word_bg, sign_col) = match side {
            Side::Old => (c.diff_del_bg, c.diff_del_word, c.git_deleted),
            Side::New => (c.diff_add_bg, c.diff_add_word, c.git_added),
        };
        if change && line == NONE {
            scene.rect(row_rect, filler);
            continue;
        }
        if change {
            scene.rect(row_rect, bg);
        } else if cursor && (unified || v.side == side) {
            scene.rect(row_rect, c.current_line);
        }
        if cursor && (unified || v.side == side) {
            let bar = (2.0 * scene_scale(g)).round().max(2.0);
            scene.rect(Rect::new(pane.x, y, bar, g.ch), c.accent);
        }
        // line numbers (unified: both columns, the other one blank on a change)
        let num_col = if change { sign_col } else { c.gutter_foreground };
        let num = |scene: &mut Scene, n: u32, x: f32| {
            if n == NONE {
                return;
            }
            let s = (n + 1).to_string();
            let nx = x + g.num_w - (columns(&s) as f32 + 0.5) * g.cw;
            scene.text(nx.round(), y, &s, TextStyle::new(num_col), &[]);
        };
        if unified {
            let (o, n) =
                if change { if side == Side::Old { (line, NONE) } else { (NONE, line) } } else { (r.old, r.new) };
            num(scene, o, pane.x);
            num(scene, n, pane.x + g.num_w);
            if change {
                let sign = if side == Side::Old { "−" } else { "+" };
                scene.text(
                    (pane.x + 2.0 * g.num_w + 0.5 * g.cw).round(),
                    y,
                    sign,
                    TextStyle::new(sign_col).bold(),
                    &[],
                );
            }
        } else {
            num(scene, line, pane.x);
        }
        // the text, its changed words and search matches behind it
        let tx = g.text_x(pane, unified);
        let clip = Rect::new(tx - g.cw * 0.25, y, (pane.right() - tx + g.cw * 0.25).max(0.0), g.ch);
        let text = match side {
            Side::Old => d.old.line(line as usize),
            Side::New => d.new.line(line as usize),
        };
        let cols = ((pane.right() - tx) / g.cw).ceil() as usize + 2;
        let start_col = lb.layout(&text, g.tw, g.left.max(0.0) as usize, cols);
        let x_of = |col: u32| tx - (g.left * g.cw).round() + col as f32 * g.cw;
        scene.push_clip(clip);
        if let Some(w) = &words {
            let spans = if side == Side::Old { &w.old } else { &w.new };
            for sp in spans.iter().filter(|sp| sp.line == line) {
                let (a, z) = (lb.col_at(sp.start as usize), lb.col_at(sp.end as usize));
                if z > a {
                    let rr = Rect::new(x_of(a), y, (z - a) as f32 * g.cw, g.ch);
                    scene.rounded_rect(rr, (2.0 * scene_scale(g)).round(), word_bg, None);
                }
            }
        }
        if let Some(find) = search
            && text.len() <= 10_000
        {
            for (ma, mz) in find(&text) {
                let (a, z) = (lb.col_at(ma), lb.col_at(mz.max(ma + 1)));
                let rr = Rect::new(x_of(a), y, (z.max(a + 1) - a) as f32 * g.cw, g.ch);
                scene.rounded_rect(rr, (2.0 * scene_scale(g)).round(), c.search_match, None);
            }
        }
        if !lb.disp.is_empty() {
            lb.spans.clear();
            #[cfg(feature = "syntax")]
            if st.ed.features.on(Feature::Syntax) {
                let spans = match (side, hl_live) {
                    (Side::New, Some(b)) => st.spans(b, line as usize),
                    (Side::Old, _) => s.hl[0].as_ref().and_then(|h| h.spans_for_line(line as usize)),
                    (Side::New, None) => s.hl[1].as_ref().and_then(|h| h.spans_for_line(line as usize)),
                };
                if let Some(spans) = spans {
                    lb.syntax(spans);
                }
            }
            #[cfg(not(feature = "syntax"))]
            let _ = (st, hl_live);
            scene.text(x_of(start_col as u32), y, &lb.disp, TextStyle::new(c.foreground), &lb.spans);
        }
        scene.pop_clip();
    }
}

fn scene_scale(g: &Geo) -> f32 {
    g.scale.max(1.0)
}

fn fold_row(scene: &mut Scene, g: &Geo, r: Row, y: f32, cursor: bool, focused: bool, c: &Colors) {
    let band = Rect::new(g.body.x, y, g.body.w, g.ch);
    let bg = if cursor { c.panel_selection } else { mix(c.panel_background, c.background, 0.7) };
    scene.rect(band, bg);
    let line = (scene_scale(g)).round().max(1.0);
    scene.rect(Rect::new(band.x, y, band.w, line), mix(c.panel_border, bg, 0.5));
    scene.rect(Rect::new(band.x, y + g.ch - line, band.w, line), mix(c.panel_border, bg, 0.5));
    let n = r.n;
    let label =
        if n == 1 { "⋯  1 unchanged line  ⋯".to_string() } else { format!("⋯  {n} unchanged lines  ⋯") };
    let x = (band.x + (band.w - columns(&label) as f32 * g.cw) / 2.0).round();
    let col = if cursor && focused { c.accent } else { c.panel_muted };
    scene.text(x, y, &label, TextStyle::new(col), &[]);
    if cursor {
        scene.text((band.x + g.cw).round(), y, "⏎ expand", TextStyle::new(c.panel_muted), &[]);
    }
}

fn file_row(scene: &mut Scene, s: &Section, g: &Geo, y: f32, row: usize, cursor: bool, c: &Colors) {
    let band = Rect::new(g.body.x, y, g.body.w, g.ch);
    scene.rect(band, if cursor { c.panel_selection } else { c.panel_background });
    let line = (scene_scale(g)).round().max(1.0);
    if row > 0 {
        scene.rect(Rect::new(band.x, y, band.w, line), c.panel_border);
    }
    let mut x = band.x + g.cw;
    let chevron = if s.collapsed { "▸" } else { "▾" };
    scene.text(x.round(), y, chevron, TextStyle::new(c.panel_muted), &[]);
    x += 2.0 * g.cw;
    let (letter, col) = match s.state {
        Some(FileState::Added) => ("A", c.git_added),
        Some(FileState::Deleted) => ("D", c.git_deleted),
        Some(FileState::Renamed) => ("R", c.git_renamed),
        Some(FileState::Copied) => ("C", c.git_renamed),
        Some(FileState::TypeChanged) => ("T", c.git_modified),
        Some(FileState::Modified) | None => ("M", c.git_modified),
    };
    scene.text(x.round(), y, letter, TextStyle::new(col).bold(), &[]);
    x += 2.0 * g.cw;
    // the folder dimmed, the name bright
    let (dir, name) = s.path.rsplit_once('/').map_or(("", s.path.as_str()), |(d, n)| (d, n));
    if !dir.is_empty() {
        let d = format!("{dir}/");
        scene.text(x.round(), y, &d, TextStyle::new(c.panel_muted), &[]);
        x += columns(&d) as f32 * g.cw;
    }
    scene.text(x.round(), y, name, TextStyle::new(c.panel_foreground).bold(), &[]);
    x += (columns(name) + 1) as f32 * g.cw;
    if let Some(op) = &s.old_path {
        let from = format!("← {op}");
        scene.text(x.round(), y, &from, TextStyle::new(c.panel_muted), &[]);
    }
    // +A −R at the right
    let (a, d) = s.stats;
    let stats = if s.binary { "binary".to_string() } else { format!("+{a} −{d}") };
    let mut rx = band.right() - (columns(&stats) as f32 + 1.0) * g.cw;
    if s.binary {
        scene.text(rx.round(), y, &stats, TextStyle::new(c.panel_muted), &[]);
    } else {
        let plus = format!("+{a}");
        scene.text(rx.round(), y, &plus, TextStyle::new(c.git_added), &[]);
        rx += (columns(&plus) + 1) as f32 * g.cw;
        scene.text(rx.round(), y, &format!("−{d}"), TextStyle::new(c.git_deleted), &[]);
    }
}

/// The bar of each change in the gutter between the panes, with its action on the first row.
fn gutter_marks(scene: &mut Scene, v: &mut DiffView, g: &Geo, rows: Range<usize>, c: &Colors) {
    let can = v.actions();
    let glyph = if can.stage {
        Some("+")
    } else if can.unstage {
        Some("−")
    } else {
        None
    };
    let w = (3.0 * scene_scale(g)).round().max(2.0);
    let mut row = rows.start;
    while row < rows.end {
        let Some(span) = v.rows.change_rows(row) else {
            row += 1;
            continue;
        };
        let r = v.rows.rows[row];
        let hunk = v.sections[r.file as usize].data.as_ref().and_then(|d| d.hunks.get(r.n as usize));
        let col = match hunk.map(|h| (h.removed() > 0, h.added() > 0)) {
            Some((false, true)) => c.git_added,
            Some((true, false)) => c.git_deleted,
            _ => c.git_modified,
        };
        let top = g.row_y(span.start.max(rows.start));
        let bottom = g.row_y(span.end.min(rows.end) - 1) + g.ch;
        let x = (g.mid.x + (g.mid.w - w) / 2.0).round();
        scene.rounded_rect(Rect::new(x, top, w, bottom - top), w / 2.0, col, None);
        // the action sits on the change's first row
        if let Some(gl) = glyph
            && span.start >= rows.start
        {
            let y = g.row_y(span.start);
            let size = g.ch.min(g.mid.w - 2.0 * scene_scale(g));
            let b = Rect::new((g.mid.x + (g.mid.w - size) / 2.0).round(), y + (g.ch - size) / 2.0, size, size);
            scene.rounded_rect(b, size / 2.0, col, None);
            let tx = (b.x + (b.w - columns(gl) as f32 * g.cw) / 2.0).round();
            scene.text(tx, y, gl, TextStyle::new(c.background).bold(), &[]);
            v.hits.actions.push((b, span.start));
        }
        row = span.end;
    }
}

/// Every change as a mark in the scrollbar, and the visible part on top.
#[allow(clippy::too_many_arguments)]
fn overview(scene: &mut Scene, v: &mut DiffView, track: Rect, page: usize, scroll: Scroll, c: &Colors, lay: &Layout) {
    v.hits.scrollbar = track;
    let total = v.rows.len().max(1) as f32;
    let px = |row: f32| track.y + row / total * track.h;
    let mark_w = (track.w * 0.5).round().max(2.0);
    let mut last: Option<(i32, Color)> = None;
    for &start in &v.rows.starts {
        let r = v.rows.rows[start as usize];
        let Some(h) = v.sections[r.file as usize].data.as_ref().and_then(|d| d.hunks.get(r.n as usize)) else {
            continue;
        };
        let len = match v.opts.layout {
            Lay::Split => h.removed().max(h.added()),
            Lay::Unified => h.removed() + h.added(),
        } as f32;
        let col = match (h.removed() > 0, h.added() > 0) {
            (false, true) => c.git_added,
            (true, false) => c.git_deleted,
            _ => c.git_modified,
        };
        let y = px(start as f32).floor();
        if last == Some((y as i32, col)) {
            continue;
        }
        last = Some((y as i32, col));
        let h_px = (px(start as f32 + len) - y).max(2.0 * lay.scale.max(1.0)).round();
        scene.rect(Rect::new(track.x + (track.w - mark_w) / 2.0, y, mark_w, h_px), col);
    }
    if v.rows.len() > page {
        let min_h = (24.0 * lay.scale).round();
        let h = (track.h * page as f32 / total).max(min_h).min(track.h);
        let frac = (scroll.top / (total - 1.0).max(1.0)).clamp(0.0, 1.0);
        let y = (track.y + frac * (track.h - h)).round();
        let col = c.scrollbar;
        scene.rounded_rect(
            Rect::new(track.x + 1.0, y, track.w - 2.0, h),
            3.0,
            Color::rgba(col.r, col.g, col.b, 0x90),
            None,
        );
    }
}

// ----- syntax -----

/// Makes the highlighters of the sections on screen and tells them which lines are.
#[cfg(feature = "syntax")]
fn prepare_syntax(st: &State, v: &mut DiffView, rows: Range<usize>) {
    if !st.ed.features.on(Feature::Syntax) {
        return;
    }
    let Some(syntaxes) = &st.syntax else { return };
    // per section: the old and new lines on screen
    let mut seen: Vec<(usize, [LineSpan; 2])> = Vec::new();
    for r in &v.rows.rows[rows] {
        let f = r.file as usize;
        if seen.last().is_none_or(|x| x.0 != f) {
            seen.push((f, [None, None]));
        }
        let e = &mut seen.last_mut().expect("just pushed").1;
        for (k, l) in [(0, r.old), (1, r.new)] {
            if l != NONE {
                e[k] = Some(e[k].map_or((l, l), |(a, z)| (a.min(l), z.max(l))));
            }
        }
    }
    for (f, ranges) in seen {
        let s = &mut v.sections[f];
        let Some(d) = &s.data else { continue };
        let path = v.repo.as_ref().map(|r| r.join(&s.path));
        if !s.hl_tried {
            s.hl_tried = true;
            let live = s.live.as_ref().is_some_and(|l| l.buffer != u64::MAX);
            for (k, text) in [(0, &d.old), (1, &d.new)] {
                if (k == 1 && live) || text.bytes_len() > shrew_syntax::WINDOWED_BYTES {
                    continue;
                }
                let rope = match text {
                    super::view::SideText::Bytes(t) => {
                        shrew_core::ropey::Rope::from_str(&String::from_utf8_lossy(t.bytes()))
                    }
                    super::view::SideText::Rope(r) => r.clone(),
                };
                s.hl[k] = Some(shrew_syntax::Highlighter::new(syntaxes, &rope, 0, path.as_deref()));
            }
        }
        for (k, range) in ranges.into_iter().enumerate() {
            if let (Some(h), Some((a, z))) = (&mut s.hl[k], range) {
                h.set_viewport(a as usize, (z - a) as usize + 1);
            }
        }
    }
}

// ----- one line of text -----

/// Scratch for one line: its visible part with tabs expanded, and maps from source bytes to that text and to
/// display columns.
#[derive(Default)]
struct LineBuf {
    disp: String,
    /// Source byte -> byte in `disp` (for syntax spans).
    map: Vec<u32>,
    /// Source byte -> display column (for word and search highlights); one entry past the end.
    col: Vec<u32>,
    spans: Vec<(Range<usize>, TextStyle)>,
}

impl LineBuf {
    /// Lays out the columns `left..left + cols` of `s`. Returns the column `disp` starts at.
    fn layout(&mut self, s: &str, tw: usize, left: usize, cols: usize) -> usize {
        self.disp.clear();
        self.map.clear();
        self.col.clear();
        let end = left + cols;
        let mut col = 0usize;
        let mut start: Option<usize> = None;
        for ch in s.chars() {
            let w = if ch == '\t' { tw - col % tw } else { char_columns(ch) };
            let n = ch.len_utf8();
            self.col.extend(std::iter::repeat_n(col as u32, n));
            if col + w <= left || col >= end {
                self.map.extend(std::iter::repeat_n(self.disp.len() as u32, n));
                col += w;
                continue;
            }
            start.get_or_insert(col);
            self.map.extend(std::iter::repeat_n(self.disp.len() as u32, n));
            match ch {
                '\t' => self.disp.extend(std::iter::repeat_n(' ', w)),
                '\u{7f}' => self.disp.push('␡'),
                c if (c as u32) < 0x20 => self.disp.push(char::from_u32(0x2400 + c as u32).unwrap_or('?')),
                c if c.is_control() => self.disp.push('\u{fffd}'),
                c => self.disp.push(c),
            }
            col += w;
        }
        self.col.push(col as u32);
        self.map.push(self.disp.len() as u32);
        start.unwrap_or(col)
    }

    /// The display column of source byte `b` (the end column past the text).
    fn col_at(&self, b: usize) -> u32 {
        self.col.get(b).or(self.col.last()).copied().unwrap_or(0)
    }

    #[cfg(feature = "syntax")]
    fn syntax(&mut self, spans: &[shrew_syntax::Span]) {
        for s in spans {
            let (sa, sz) = (s.start as usize, s.end as usize);
            if sa >= self.map.len() {
                break;
            }
            let a = self.map[sa] as usize;
            let z = self.map.get(sz).map_or(self.disp.len(), |&z| z as usize);
            if a < z && z <= self.disp.len() {
                let col = Color::rgb(s.fg.r, s.fg.g, s.fg.b);
                let mut style = TextStyle::new(col);
                style.bold = s.bold;
                style.italic = s.italic;
                style.underline = s.underline.then_some(col);
                self.spans.push((a..z, style));
            }
        }
    }
}
