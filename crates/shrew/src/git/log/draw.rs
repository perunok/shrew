//! Drawing a log tab: a header line, then one row per commit (graph, ref chips, summary, author, age, id) and,
//! when a commit is open, its message and changed files below.

use super::graph::{self, Frame, Quarter, Shape};
use super::{Row, View, active};
use crate::colors::{Colors, mix};
use crate::draw::{elide_end, elide_start};
use crate::layout::Layout;
use crate::state::State;
use shrew_git::{RefKind, RefLabel};
use shrew_render::{Color, Rect, Scene, TextStyle, columns};

/// The palette lanes take turns with, from the theme.
pub fn lane_colors(c: &Colors) -> [Color; 8] {
    let a = &c.ansi;
    let bright = |i: usize| if c.dark { a[i + 8] } else { a[i] };
    [c.accent, bright(4), bright(2), bright(5), bright(3), bright(6), bright(1), c.accent_alt]
}

/// Draws the active log tab over the editor area. False when the active buffer is not one.
pub fn draw(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors) -> bool {
    if !super::showing(st) {
        return false;
    }
    let Some(id) = active(st).map(|v| v.buffer) else { return false };
    if st.git.views.get(id).is_some_and(|v| v.stale && !v.loading) {
        super::reload(st, id);
    }
    let area = Rect::new(
        lay.text.x,
        lay.text.y,
        (lay.scrollbar.right().max(lay.text.right()) - lay.text.x).max(0.0),
        lay.text.h,
    );
    let Some(v) = st.git.views.get_mut(id) else { return false };
    scene.rect(area, c.background);
    scene.push_clip(area);
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    v.hits.area = Some(area);

    // header: what this is, how much is loaded, the filters, the keys
    let head_h = (ch + 2.0 * pad).round();
    scene.rect(Rect::new(area.x, area.y, area.w, head_h), c.panel_background);
    let hy = lay.centered(area.y, head_h);
    let mut x = area.x + 2.0 * cw;
    let title = v.title();
    scene.text(x, hy, &title, TextStyle::new(c.panel_foreground).bold(), &[]);
    x += (columns(&title) + 3) as f32 * cw;
    let count = if v.loading && v.rows.is_empty() {
        "loading…".to_string()
    } else {
        format!("{}{} commits{}", v.shown.len(), if v.more { "+" } else { "" }, if v.loading { " …" } else { "" })
    };
    scene.text(x, hy, &count, TextStyle::new(c.panel_muted), &[]);
    x += (columns(&count) + 3) as f32 * cw;
    if let Some(a) = &v.author {
        let chip = format!("author: {a}");
        x = chip_at(scene, x, hy, &chip, c.accent, c, lay) + cw;
    }
    if let Some(b) = &v.search {
        let s = format!("/{}", b.text);
        scene.text(x, hy, &s, TextStyle::new(c.foreground), &[]);
        let cx = x + columns(&s) as f32 * cw;
        scene.rect(Rect::new(cx.round(), hy, (2.0 * lay.scale).round().max(1.0), ch), c.cursor);
    } else if !v.last_search.is_empty() {
        scene.text(x, hy, &format!("/{}", v.last_search), TextStyle::new(c.panel_muted), &[]);
    }
    let keys = "Enter commit  / search  a author  p path  b branch  y copy id  q close";
    let kx = area.right() - (columns(keys) + 2) as f32 * cw;
    if kx > x + 12.0 * cw {
        scene.text(kx.round(), hy, keys, TextStyle::new(mix(c.panel_muted, c.panel_background, 0.75)), &[]);
    }

    // the list, and the details under it
    let body_y = area.y + head_h;
    let body_h = area.bottom() - body_y;
    let details_h =
        if v.details.is_some() { (body_h * 0.45).max(6.0 * ch).min(body_h - 3.0 * ch).round() } else { 0.0 };
    let list = Rect::new(area.x, body_y, area.w, (body_h - details_h).max(0.0));
    v.hits.list = Some(list);
    let row_h = (ch * 1.3).round();
    let visible = ((list.h / row_h).floor() as usize).max(1);
    v.fit(visible);
    if v.rows.is_empty() || v.shown.is_empty() {
        let msg = match (&v.error, v.loading) {
            (Some(e), _) => e.clone(),
            (None, true) => "reading the history…".into(),
            (None, false) if v.author.is_some() => "no commits by that author in what is loaded".into(),
            _ => "no commits yet".into(),
        };
        let col = if v.error.is_some() { c.error } else { c.panel_muted };
        scene.text(area.x + 2.0 * cw, lay.centered(body_y + pad, row_h), &msg, TextStyle::new(col), &[]);
    }
    rows(scene, v, list, row_h, lay, c);
    if v.details.is_some() {
        let d = Rect::new(area.x, list.bottom(), area.w, details_h);
        details(scene, v, d, lay, c);
    }
    scene.pop_clip();
    let wants = v.wants_more();
    if wants {
        super::more(st, id);
    }
    true
}

/// A rounded label; returns where it ends.
fn chip_at(scene: &mut Scene, x: f32, y: f32, s: &str, col: Color, c: &Colors, lay: &Layout) -> f32 {
    let w = (columns(s) as f32 + 1.0) * lay.cw;
    let r = Rect::new(x.round(), y, w.round(), lay.ch);
    scene.rounded_rect(r, (4.0 * lay.scale).round(), mix(col, c.background, 0.22), None);
    scene.text((x + lay.cw / 2.0).round(), y, s, TextStyle::new(col), &[]);
    x + w
}

fn ref_color(l: &RefLabel, c: &Colors) -> Color {
    match l.kind {
        RefKind::Head | RefKind::Branch => c.accent,
        RefKind::RemoteBranch => c.accent_alt,
        RefKind::Tag => c.warning,
        RefKind::Stash => c.panel_muted,
    }
}

fn rows(scene: &mut Scene, v: &mut View, list: Rect, row_h: f32, lay: &Layout, c: &Colors) {
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let colors = lane_colors(c);
    let lane_w = (cw * 1.6).round();
    let graph_on = v.author.is_none() && v.rows.iter().any(|r| r.graph.is_some());
    // the graph column is as wide as the widest row on screen (it changes as you scroll, never mid-screen)
    let lanes = if graph_on {
        v.shown
            .iter()
            .skip(v.top)
            .take(v.page + 1)
            .filter_map(|&i| v.rows[i].graph.as_ref())
            .map(|g| g.width)
            .max()
            .unwrap_or(1)
    } else {
        0
    };
    let graph_w = if graph_on { (lanes.min(24) as f32 * lane_w + cw).round() } else { 2.0 * cw };
    // right-hand columns: author, age, id
    let id_w = 9.0 * cw;
    let age_w = 15.0 * cw;
    let author_cols = 18usize.min(((list.w / cw) as usize / 6).max(8));
    let author_w = (author_cols + 2) as f32 * cw;
    let right_w = id_w + age_w + author_w;
    let x0 = list.x + cw;
    let text_x = x0 + graph_w + cw;
    let summary_right = (list.right() - right_w).max(text_x + 10.0 * cw);
    let t = (1.6 * lay.scale).round().max(1.0);
    let dot = (ch * 0.22).round().max(3.0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    for (k, &ri) in v.shown.iter().enumerate().skip(v.top).take(v.page + 1) {
        let row: &Row = &v.rows[ri];
        let y = (list.y + (k - v.top) as f32 * row_h).round();
        let rect = Rect::new(list.x, y, list.w, row_h);
        if y >= list.bottom() {
            break;
        }
        let selected = k == v.selected;
        if selected {
            let bg = if v.in_details { mix(c.panel_selection, c.background, 0.5) } else { c.panel_selection };
            scene.rounded_rect(
                Rect::new(list.x + pad, y, list.w - 2.0 * pad, row_h),
                (4.0 * lay.scale).round(),
                bg,
                None,
            );
        }
        let open = v.details.as_ref().is_some_and(|d| d.id == row.commit.id);
        if open && !selected {
            scene.rect(Rect::new(list.x + pad, y, (2.0 * lay.scale).round(), row_h), c.accent);
        }
        v.hits.rows.push((rect, k));
        let ty = lay.centered(y, row_h);
        // the graph
        if let Some(g) = row.graph.as_ref().filter(|_| graph_on) {
            let f = Frame {
                x0: x0 + lane_w / 2.0,
                lane: lane_w,
                top: y,
                mid: (y + row_h / 2.0).round(),
                bottom: y + row_h,
            };
            scene.push_clip(Rect::new(x0, y, graph_w, row_h));
            for (s, ci) in graph::row(&f, g) {
                shape(scene, s, t, colors[ci as usize % colors.len()]);
            }
            let (dx, dy) = (f.x(g.node), f.mid);
            let col = colors[g.color as usize % colors.len()];
            let head = row.refs.iter().any(|r| r.is_head || r.kind == RefKind::Head);
            let r = if head { dot + t } else { dot };
            let disc = Rect::new(dx - r, dy - r, 2.0 * r, 2.0 * r);
            if g.merge || head {
                // a ring: merges and HEAD
                let fill = if selected { c.panel_selection } else { c.background };
                scene.rounded_rect(disc, r, fill, Some((t + if head { 1.0 } else { 0.0 }, col)));
            } else {
                scene.rounded_rect(disc, r, col, None);
            }
            scene.pop_clip();
        } else if !graph_on {
            let r = dot;
            let col = if v.author.is_some() { c.panel_muted } else { c.accent };
            scene.rounded_rect(Rect::new(x0 + lane_w / 2.0 - r, y + row_h / 2.0 - r, 2.0 * r, 2.0 * r), r, col, None);
        }
        // refs, then the summary
        let mut x = text_x;
        scene.push_clip(Rect::new(text_x, y, (summary_right - text_x - cw).max(0.0), row_h));
        for l in &row.refs {
            let col = ref_color(l, c);
            let w = (columns(&l.name) as f32 + 1.0) * cw;
            let chip = Rect::new(x.round(), ty, w.round(), ch);
            if l.is_head {
                scene.rounded_rect(chip, (4.0 * lay.scale).round(), col, None);
                scene.text((x + cw / 2.0).round(), ty, &l.name, TextStyle::new(c.accent_fg).bold(), &[]);
            } else {
                scene.rounded_rect(
                    chip,
                    (4.0 * lay.scale).round(),
                    mix(col, c.background, 0.2),
                    Some((1.0, mix(col, c.background, 0.5))),
                );
                scene.text((x + cw / 2.0).round(), ty, &l.name, TextStyle::new(col), &[]);
            }
            x += w + cw / 2.0;
        }
        let room = ((summary_right - x) / cw).max(0.0) as usize;
        let mut summary = row.commit.summary.clone();
        if let Some(p) = &row.path
            && v.rows.first().and_then(|r| r.path.as_ref()) != Some(p)
        {
            summary = format!("{summary}   ({p})");
        }
        let fg = if selected { c.foreground } else { mix(c.foreground, c.panel_muted, 0.85) };
        scene.text(x.round(), ty, &elide_end(&summary, room), TextStyle::new(fg), &[]);
        scene.pop_clip();
        // author, age, id
        let ax = list.right() - right_w;
        let author = elide_end(&row.commit.author.name, author_cols);
        scene.text(ax.round(), ty, &author, TextStyle::new(c.panel_muted), &[]);
        let age = super::super::ago_at(row.commit.author.time, now);
        scene.text((ax + author_w).round(), ty, &age, TextStyle::new(c.panel_muted), &[]);
        let short = row.commit.id.short();
        scene.text(
            (ax + author_w + age_w).round(),
            ty,
            &short,
            TextStyle::new(mix(c.panel_muted, c.background, 0.7)),
            &[],
        );
    }
    if v.loading && !v.rows.is_empty() && v.top + v.page >= v.shown.len() {
        let y = list.y + (v.shown.len() - v.top) as f32 * row_h;
        if y < list.bottom() {
            scene.text(text_x, lay.centered(y, row_h), "loading more…", TextStyle::new(c.panel_muted), &[]);
        }
    }
}

/// One graph shape: a line as a thin rect, an arc as a clipped circle outline.
fn shape(scene: &mut Scene, s: Shape, t: f32, col: Color) {
    match s {
        Shape::V { x, y0, y1 } => scene.rect(Rect::new((x - t / 2.0).round(), y0, t, (y1 - y0).max(0.0)), col),
        Shape::H { y, x0, x1 } => {
            let (a, b) = if x0 < x1 { (x0, x1) } else { (x1, x0) };
            scene.rect(Rect::new(a, (y - t / 2.0).round(), (b - a).max(0.0), t), col)
        }
        Shape::Arc { cx, cy, r, q } => {
            let rr = r + t / 2.0;
            let circle = Rect::new(cx - rr, cy - rr, 2.0 * rr, 2.0 * rr);
            let clip = match q {
                Quarter::TopLeft => Rect::new(cx - rr, cy - rr, rr, rr),
                Quarter::TopRight => Rect::new(cx, cy - rr, rr, rr),
                Quarter::BottomLeft => Rect::new(cx - rr, cy, rr, rr),
                Quarter::BottomRight => Rect::new(cx, cy, rr, rr),
            };
            scene.push_clip(clip);
            scene.rounded_rect(circle, rr, Color::TRANSPARENT, Some((t, col)));
            scene.pop_clip();
        }
    }
}

/// The open commit: id, author, date, the whole message and the changed files with their line counts.
fn details(scene: &mut Scene, v: &mut View, area: Rect, lay: &Layout, c: &Colors) {
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let Some(d) = &mut v.details else { return };
    v.hits.details = Some(area);
    scene.rect(area, c.panel_background);
    scene.rect(Rect::new(area.x, area.y, area.w, lay.scale.round().max(1.0)), c.panel_border);
    scene.push_clip(area);
    let x = area.x + 2.0 * cw;
    let mut y = area.y + pad;
    let Some(data) = d.data.clone() else {
        let (msg, col) = match &d.error {
            Some(e) => (e.clone(), c.error),
            None => (format!("reading {}…", d.id.short()), c.panel_muted),
        };
        scene.text(x, y, &msg, TextStyle::new(col), &[]);
        scene.pop_clip();
        return;
    };
    let info = &data.info;
    let head = format!(
        "{}  {} <{}>  {}",
        info.id,
        info.author.name,
        info.author.email,
        date(info.author.time, info.author.offset)
    );
    scene.text(x, y, &head, TextStyle::new(c.panel_muted), &[(0..40, TextStyle::new(c.accent).bold())]);
    y += ch;
    // the message and the files share the rest; the files get at least a third
    let lines: Vec<&str> = data.message.trim_end().lines().collect();
    let room = (((area.bottom() - y - pad) / ch).floor() as usize).max(2);
    let files_rows = data.files.len().min(room.saturating_sub(2).max(room / 3));
    let msg_rows = lines.len().min(room.saturating_sub(files_rows + 1)).max(1);
    y += pad;
    for (i, l) in lines.iter().take(msg_rows).enumerate() {
        let style = if i == 0 { TextStyle::new(c.panel_foreground).bold() } else { TextStyle::new(c.panel_foreground) };
        scene.text(x, y, l, style, &[]);
        y += ch;
    }
    y += pad;
    // changed files, scrolled to keep the highlighted one in view
    let shown = ((area.bottom() - y) / ch).floor().max(1.0) as usize;
    if d.file < d.top {
        d.top = d.file;
    } else if d.file >= d.top + shown {
        d.top = d.file + 1 - shown;
    }
    d.top = d.top.min(data.files.len().saturating_sub(shown));
    let path_cols = ((area.w / cw) as usize).saturating_sub(24);
    for (i, f) in data.files.iter().enumerate().skip(d.top).take(shown) {
        let r = Rect::new(area.x + pad, y, area.w - 2.0 * pad, ch);
        if i == d.file && v.in_details {
            scene.rounded_rect(r, (3.0 * lay.scale).round(), c.panel_selection, None);
        }
        let (letter, col) = super::super::panel::draw::state_style(f.state, c);
        scene.text(x, y, &letter.to_string(), TextStyle::new(col).bold(), &[]);
        let path = match &f.old_path {
            Some(o) => format!("{o} → {}", f.path),
            None => f.path.clone(),
        };
        scene.text(x + 2.0 * cw, y, &elide_start(&path, path_cols), TextStyle::new(c.panel_foreground), &[]);
        let counts = if f.binary { "binary".to_string() } else { format!("+{}  −{}", f.added, f.removed) };
        let cx = area.right() - (columns(&counts) + 2) as f32 * cw;
        let plus = counts.find(' ').unwrap_or(counts.len());
        let spans = if f.binary {
            vec![]
        } else {
            vec![(0..plus, TextStyle::new(c.git_added)), (plus..counts.len(), TextStyle::new(c.git_deleted))]
        };
        scene.text(cx.round(), y, &counts, TextStyle::new(c.panel_muted), &spans);
        v.hits.files.push((r, i));
        y += ch;
    }
    scene.pop_clip();
}

/// "2024-03-05 14:07" in the author's time zone.
pub fn date(time: i64, offset: i32) -> String {
    let t = time + offset as i64;
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}
