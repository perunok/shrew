//! Drawing the Source Control panel in the sidebar, and its mouse: the Files | Git tabs in the sidebar's header,
//! the branch line with the sync button, the commit box with its buttons and subject-length hint, and the sections
//! of changed files whose rows show their buttons (stage, unstage, discard) on hover or when selected.

use super::{Action, Button, Focus, Row, SUBJECT_HARD, SUBJECT_SOFT, Section};
use crate::colors::{Colors, mix};
use crate::draw::elide_end;
use crate::layout::Layout;
use crate::state::State;
use shrew_core::Command;
use shrew_git::{FileState, Head, Mark, RepoState};
use shrew_render::{Color, Rect, Scene, TextStyle, columns};

/// The Git tab shows (the feature is on, the tab is chosen, and the sidebar or the panel alone is out).
pub fn shown(st: &State) -> bool {
    super::super::on(&st.ed) && st.git.panel.active && (st.ed.sidebar_on() || st.git.panel.alone)
}

/// The panel wants the sidebar region although the sidebar feature is off (Ctrl+Shift+G).
pub fn alone(st: &State) -> bool {
    shown(st) && !st.ed.sidebar_on()
}

pub fn mark_color(m: Mark, c: &Colors) -> Color {
    match m {
        Mark::Added => c.git_added,
        Mark::Modified => c.git_modified,
        Mark::Deleted => c.git_deleted,
        Mark::Renamed => c.git_renamed,
        Mark::Untracked => c.git_untracked,
        Mark::Ignored => c.git_ignored,
        Mark::Conflict => c.git_conflict,
    }
}

/// A file state's letter and colour.
pub fn state_style(s: FileState, c: &Colors) -> (char, Color) {
    let col = match s {
        FileState::Added => c.git_added,
        FileState::Deleted => c.git_deleted,
        FileState::Renamed | FileState::Copied => c.git_renamed,
        FileState::Modified | FileState::TypeChanged => c.git_modified,
    };
    (s.letter(), col)
}

/// The Files tab's label: the project folder's name, as the tree's header shows it.
fn files_label(st: &State) -> String {
    match &st.ed.tree {
        Some(t) => {
            t.root.file_name().map_or_else(|| t.root.display().to_string(), |n| n.to_string_lossy().to_uppercase())
        }
        None => "FILES".into(),
    }
}

/// What the sync button does now, and its label.
fn sync(head: &Head, running: bool) -> Option<(String, Command)> {
    if running {
        return None;
    }
    head.branch.as_ref()?;
    Some(match &head.upstream {
        None => ("Publish".into(), Command::GitPush),
        Some(u) if u.gone => ("Publish".into(), Command::GitPush),
        Some(u) if u.behind > 0 => (format!("↓{} Pull", u.behind), Command::GitPull),
        Some(u) if u.ahead > 0 => (format!("↑{} Push", u.ahead), Command::GitPush),
        Some(_) => ("Fetch".into(), Command::GitFetch),
    })
}

/// The repository has a remote to sync with (assumed until its refs are read).
fn has_remote(st: &State) -> bool {
    st.git.scm.refs.as_ref().is_none_or(|r| !r.remotes.is_empty())
}

/// Draws the tabs in the sidebar's header and, on the Git tab, the panel. True when the Git tab took the sidebar
/// (the tree is not drawn).
pub fn sidebar(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors) -> bool {
    let area = lay.sidebar;
    if !super::super::on(&st.ed) || area.w <= 0.0 {
        return false;
    }
    let git = shown(st);
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let head_h = (ch + 3.0 * pad).round();
    let x0 = area.x + 3.0 * pad;
    let hy = lay.centered(area.y, head_h);
    let label = files_label(st);
    let label = elide_end(&label, ((area.w / cw) as usize).saturating_sub(12).max(4)).into_owned();
    let files_w = columns(&label) as f32 * cw;
    let total = st.git.panel.total();
    let git_label = if total > 0 && st.git.scm.repo.is_some() { format!("GIT {total}") } else { "GIT".to_string() };
    let gx = x0 + files_w + 3.0 * cw;
    let underline = |scene: &mut Scene, x: f32, w: f32| {
        let t = (2.0 * lay.scale).round().max(1.0);
        scene.rect(Rect::new(x.round(), (area.y + head_h - t - pad / 2.0).round(), w.round(), t), c.accent);
    };
    let hits = &mut st.git.panel.hits;
    hits.buttons.push((Rect::new(area.x, area.y, gx - area.x - cw, head_h), Button::FilesTab));
    hits.buttons.push((Rect::new(gx - cw, area.y, (columns(&git_label) + 2) as f32 * cw, head_h), Button::GitTab));
    if !git {
        // the tree's header draws its own label; this adds the Git tab beside it
        scene.text(gx, hy, &git_label, TextStyle::new(c.panel_muted).bold(), &[]);
        if st.ed.tree.is_some() {
            underline(scene, x0, files_w);
        }
        return false;
    }
    hits.area = Some(area);
    scene.text(x0, hy, &label, TextStyle::new(c.panel_muted), &[]);
    let gl = TextStyle::new(c.accent).bold();
    let spans =
        git_label.find(' ').map(|i| vec![(i..git_label.len(), TextStyle::new(c.panel_muted))]).unwrap_or_default();
    scene.text(gx, hy, &git_label, gl, &spans);
    underline(scene, gx, columns("GIT") as f32 * cw);
    let body = Rect::new(area.x, area.y + head_h, area.w - lay.scale.round().max(1.0), area.h - head_h);
    panel(scene, st, body, lay, c);
    true
}

fn panel(scene: &mut Scene, st: &mut State, body: Rect, lay: &Layout, c: &Colors) {
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let x0 = body.x + 3.0 * pad;
    let inner_w = body.w - 6.0 * pad;
    let mut y = body.y;
    let row_h = (ch + pad).round();
    let radius = (4.0 * lay.scale).round();
    let focused = st.git.panel.focused;
    let git = &st.git;
    let line = |scene: &mut Scene, y: f32, s: &str, col: Color| {
        scene.text(x0, lay.centered(y, row_h), s, TextStyle::new(col), &[])
    };
    if git.scm.repo.is_none() {
        let looking = git.scm.target.as_ref().is_some_and(|t| git.known(t).is_none());
        if looking {
            line(scene, y, "looking for a repository…", c.panel_muted);
        } else {
            line(scene, y, "Not a git repository", c.panel_foreground);
            line(scene, y + row_h, "git init in a terminal", c.panel_muted);
            line(scene, y + 2.0 * row_h, "starts one here", c.panel_muted);
        }
        return;
    }
    let busy = git.ops.busy();
    // branch, upstream, sync
    let head = git.scm.status.as_ref().map(|s| s.head.clone()).unwrap_or_default();
    let by = lay.centered(y, row_h);
    let branch = if git.scm.status.is_none() {
        "…".to_string()
    } else if head.detached {
        format!("detached at {}", head.label())
    } else if head.is_unborn() {
        format!("{} (no commits yet)", head.label())
    } else {
        head.label()
    };
    let icon = "\u{e0a0}";
    scene.text(x0, by, icon, TextStyle::new(c.accent), &[]);
    let mut bx = x0 + 2.0 * cw;
    // the sync button on the right, then what fits of the branch and its upstream
    let mut right = body.right() - 3.0 * pad;
    let running = git.ops.running.as_ref().map(|r| r.label);
    if let Some(label) = running {
        let s = format!("{label}… ✕");
        let w = (columns(&s) + 2) as f32 * cw;
        right -= w;
        let r = Rect::new(right.round(), y + pad / 2.0, w.round(), row_h - pad);
        scene.rounded_rect(r, radius, mix(c.accent, c.panel_background, 0.2), None);
        scene.text((right + cw).round(), by, &s, TextStyle::new(c.accent), &[]);
        st.git.panel.hits.buttons.push((r, Button::Cancel));
        right -= cw;
    } else if let Some((s, _)) = sync(&head, running.is_some()).filter(|_| has_remote(st)) {
        let w = (columns(&s) + 2) as f32 * cw;
        right -= w;
        let r = Rect::new(right.round(), y + pad / 2.0, w.round(), row_h - pad);
        scene.rounded_rect(r, radius, c.panel_selection, None);
        scene.text((right + cw).round(), by, &s, TextStyle::new(c.panel_foreground), &[]);
        st.git.panel.hits.buttons.push((r, Button::Sync));
        right -= cw;
    }
    let upstream = match &head.upstream {
        Some(u) if u.gone => format!("{} gone", u.name),
        // the sync button says it
        Some(u) if (u.ahead == 0) != (u.behind == 0) && running.is_none() => String::new(),
        Some(u) if u.ahead + u.behind > 0 => {
            let mut s = String::new();
            if u.ahead > 0 {
                s += &format!("↑{}", u.ahead);
            }
            if u.behind > 0 {
                s += &format!(" ↓{}", u.behind);
            }
            s.trim().to_string()
        }
        Some(_) => "✓".into(),
        None if head.branch.is_some() && !head.is_unborn() && has_remote(st) => "no upstream".into(),
        None => String::new(),
    };
    let room = ((right - bx) / cw).max(0.0) as usize;
    let b = elide_end(&branch, room.saturating_sub(columns(&upstream) + 1).max(4));
    scene.text(bx, by, &b, TextStyle::new(c.panel_foreground).bold(), &[]);
    bx += (columns(&b) + 1) as f32 * cw;
    if !upstream.is_empty() && bx + columns(&upstream) as f32 * cw <= right {
        scene.text(bx, by, &upstream, TextStyle::new(c.panel_muted), &[]);
    }
    y += row_h;
    // a merge, rebase... in progress
    if head.state != RepoState::Clean {
        // two rows: what is going on, then its buttons
        let noun = super::super::ops::state_noun(head.state).to_uppercase();
        let r = Rect::new(body.x + pad, y, body.w - 2.0 * pad, 2.0 * row_h + pad);
        scene.rounded_rect(r, radius, mix(c.warning, c.panel_background, 0.15), None);
        let ty = lay.centered(y, row_h);
        scene.text(x0, ty, &format!("{noun} IN PROGRESS"), TextStyle::new(c.warning).bold(), &[]);
        let mut bx = x0 - pad;
        let by = y + row_h;
        for (s, b) in [("Continue", Button::Continue), ("Abort", Button::Abort)] {
            let w = (columns(s) + 2) as f32 * cw;
            let br = Rect::new(bx.round(), by, w.round(), row_h);
            scene.rounded_rect(br, radius, mix(c.panel_selection, c.panel_background, 0.8), None);
            scene.text((bx + cw).round(), lay.centered(by, row_h), s, TextStyle::new(c.panel_foreground), &[]);
            st.git.panel.hits.buttons.push((br, b));
            bx += w + cw / 2.0;
        }
        y += 2.0 * row_h + 2.0 * pad;
    }
    let git = &st.git;
    // the commit message box
    y += pad;
    let box_cols = ((inner_w - 2.0 * cw) / cw).max(4.0) as usize;
    let msg = &git.panel.message;
    let rows = msg.rows(box_cols);
    let shown_rows = rows.len().clamp(2, 6);
    let box_h = (shown_rows as f32 * ch + 2.0 * pad).round();
    let mbox = Rect::new(x0 - pad, y, inner_w + 2.0 * pad, box_h);
    let typing = focused && git.panel.focus == Focus::Message;
    let border = if typing { c.accent } else { c.panel_border };
    scene.rounded_rect(mbox, radius, c.background, Some((lay.scale.round().max(1.0), border)));
    let tx = mbox.x + cw;
    let ty = mbox.y + pad;
    scene.push_clip(Rect::new(mbox.x, mbox.y + pad, mbox.w, box_h - 2.0 * pad));
    let (crow, ccol) = msg.caret(box_cols);
    let first = crow.saturating_sub(shown_rows - 1);
    if msg.text.is_empty() {
        let on = head.branch.as_deref().unwrap_or("HEAD");
        let faint = TextStyle::new(mix(c.panel_muted, c.background, 0.75));
        scene.text(tx, ty, "Message", faint, &[]);
        let hint = format!("{} commits on {on}", crate::context_menu::platform_label("Ctrl+Enter"));
        scene.text(tx, ty + ch, &elide_end(&hint, box_cols), faint, &[]);
    }
    for (i, &(a, z)) in rows.iter().enumerate().skip(first).take(shown_rows) {
        let ry = ty + (i - first) as f32 * ch;
        // the subject past 72 columns in the error colour, past 50 in the warning colour
        let mut spans = Vec::new();
        if a < msg.subject().len() {
            let s = &msg.text[a..z.min(msg.subject().len())];
            let start_col = columns(&msg.text[..a]);
            let mut col = start_col;
            for (bi, chr) in s.char_indices() {
                if col >= SUBJECT_SOFT {
                    let tone = if col >= SUBJECT_HARD { c.error } else { c.warning };
                    spans.push((bi..bi + chr.len_utf8(), TextStyle::new(tone)));
                }
                col += shrew_render::char_columns(chr);
            }
        }
        scene.text(tx, ry, &msg.text[a..z], TextStyle::new(c.foreground), &spans);
    }
    if typing {
        let cx = tx + ccol as f32 * cw;
        let cy = ty + (crow - first) as f32 * ch;
        scene.rect(Rect::new(cx.round(), cy, (2.0 * lay.scale).round().max(1.0), ch), c.cursor);
    }
    scene.pop_clip();
    st.git.panel.hits.message = Some(mbox);
    st.git.panel.hits.grid = Some(super::Grid { x: tx, y: ty, cw, ch, cols: box_cols, first });
    y += box_h + pad;
    // the commit button, amend, and how long the subject is
    let git = &st.git;
    let staged = git.scm.status.as_ref().is_some_and(|s| s.staged().next().is_some());
    let ready = staged && !git.panel.message.is_blank() && !busy;
    let label = "✓ Commit";
    let w = (columns(label) + 2) as f32 * cw;
    let cr = Rect::new(x0 - pad, y, w.round(), row_h);
    let (bg, fg) = if ready { (c.accent, c.accent_fg) } else { (c.panel_selection, c.panel_muted) };
    scene.rounded_rect(cr, radius, bg, None);
    let by = lay.centered(y, row_h);
    scene.text((cr.x + cw).round(), by, label, TextStyle::new(fg).bold(), &[]);
    let ar = Rect::new(cr.right() + cw / 2.0, y, 7.0 * cw, row_h);
    scene.text((ar.x + cw).round(), by, "Amend", TextStyle::new(c.panel_muted), &[]);
    let n = columns(git.panel.message.subject());
    if n > 0 {
        let hint = format!("{n}/{SUBJECT_SOFT}");
        let tone = match n {
            n if n > SUBJECT_HARD => c.error,
            n if n > SUBJECT_SOFT => c.warning,
            _ => c.panel_muted,
        };
        let hx = body.right() - 3.0 * pad - columns(&hint) as f32 * cw;
        if hx > ar.right() {
            scene.text(hx.round(), by, &hint, TextStyle::new(tone), &[]);
        }
    }
    st.git.panel.hits.buttons.push((cr, Button::Commit));
    st.git.panel.hits.buttons.push((ar, Button::Amend));
    y += row_h + pad;
    let git = &st.git;
    if let Some(e) = &git.scm.status_error {
        line(scene, y, &elide_end(e, (inner_w / cw) as usize), c.error);
        y += row_h;
    }
    scene.rect(Rect::new(body.x, y.round(), body.w, lay.scale.round().max(1.0)), c.panel_border);
    y += pad;
    if git.scm.status.is_some() && git.panel.rows.is_empty() {
        line(scene, y, "No changes", c.panel_muted);
        return;
    }
    // the sections
    let visible = (((body.bottom() - y - pad) / row_h).floor() as usize).max(1);
    let p = &mut st.git.panel;
    p.fit(visible);
    let list_top = y;
    let hover = p.hover;
    for (i, row) in p.rows.iter().enumerate().skip(p.top).take(visible) {
        let ry = (list_top + (i - p.top) as f32 * row_h).round();
        let rect = Rect::new(body.x, ry, body.w, row_h);
        let selected = i == p.selected;
        if selected {
            let bg = if focused && p.focus == Focus::List {
                c.panel_selection
            } else {
                mix(c.panel_selection, c.panel_background, 0.5)
            };
            scene.rounded_rect(Rect::new(body.x + pad, ry, body.w - 2.0 * pad, row_h), radius, bg, None);
        } else if hover == Some(i) {
            let fill = crate::statusbar::hover_fill(c);
            scene.rounded_rect(Rect::new(body.x + pad, ry, body.w - 2.0 * pad, row_h), radius, fill, None);
        }
        let ty = lay.centered(ry, row_h);
        let mut right = body.right() - 3.0 * pad;
        let show_buttons = hover == Some(i) || (selected && focused);
        match row {
            Row::Header(s, n) => {
                let chevron = if p.is_collapsed(*s) { "▸" } else { "▾" };
                scene.text(x0, ty, chevron, TextStyle::new(c.panel_muted), &[]);
                scene.text(x0 + 2.0 * cw, ty, s.title(), TextStyle::new(c.panel_muted).bold(), &[]);
                let count = n.to_string();
                let w = (columns(&count) + 1) as f32 * cw;
                right -= w;
                let pill = Rect::new(right.round(), ty, w.round(), ch);
                scene.rounded_rect(pill, ch / 2.0, mix(c.panel_muted, c.panel_background, 0.25), None);
                scene.text((right + cw / 2.0).round(), ty, &count, TextStyle::new(c.panel_foreground), &[]);
                right -= cw / 2.0;
                if show_buttons {
                    buttons(scene, &mut p.hits.buttons, *s, i, &mut right, ty, lay, c);
                }
            }
            Row::File(f) => {
                let col = mark_color(f.mark, c);
                let letter = f.letter.to_string();
                right -= cw;
                scene.text(right.round(), ty, &letter, TextStyle::new(col).bold(), &[]);
                right -= cw;
                if show_buttons {
                    buttons(scene, &mut p.hits.buttons, f.section, i, &mut right, ty, lay, c);
                }
                let nx = x0 + 2.0 * cw;
                let room = ((right - nx) / cw).max(0.0) as usize;
                let name = elide_end(f.name(), room);
                let fg = if f.mark == Mark::Deleted { c.panel_muted } else { mix(col, c.panel_foreground, 0.35) };
                scene.text(nx, ty, &name, TextStyle::new(fg), &[]);
                let dir = match &f.orig_path {
                    Some(o) => format!("← {o}"),
                    None => f.dir().to_string(),
                };
                let used = columns(&name) + 1;
                if !dir.is_empty() && room > used + 3 {
                    let d = crate::draw::elide_start(&dir, room - used);
                    scene.text(nx + used as f32 * cw, ty, &d, TextStyle::new(c.panel_muted), &[]);
                }
            }
        }
        p.hits.rows.push((rect, i));
    }
    // a thin scrollbar when the list does not fit
    if p.rows.len() > visible {
        let track_h = visible as f32 * row_h;
        let th = (track_h * visible as f32 / p.rows.len() as f32).max(ch);
        let tyy = list_top + (track_h - th) * p.top as f32 / (p.rows.len() - visible).max(1) as f32;
        let bw = (3.0 * lay.scale).round().max(2.0);
        scene.rounded_rect(
            Rect::new(body.right() - bw - pad / 2.0, tyy.round(), bw, th.round()),
            bw / 2.0,
            c.scrollbar,
            None,
        );
    }
}

/// A row's buttons, right to left: stage (+), unstage (−), discard (↺).
#[allow(clippy::too_many_arguments)]
fn buttons(
    scene: &mut Scene,
    hits: &mut Vec<(Rect, Button)>,
    s: Section,
    i: usize,
    right: &mut f32,
    y: f32,
    lay: &Layout,
    c: &Colors,
) {
    let list: &[(&str, Button)] = match s {
        Section::Merge => &[("+", Button::Stage(i))],
        Section::Staged => &[("−", Button::Unstage(i))],
        Section::Changes | Section::Untracked => &[("+", Button::Stage(i)), ("↺", Button::Discard(i))],
    };
    for (label, b) in list {
        let w = 2.0 * lay.cw;
        *right -= w;
        let r = Rect::new(right.round(), y, w.round(), lay.ch);
        scene.rounded_rect(r, (3.0 * lay.scale).round(), mix(c.panel_muted, c.panel_background, 0.18), None);
        scene.text((*right + lay.cw / 2.0).round(), y, label, TextStyle::new(c.panel_foreground), &[]);
        hits.push((r, *b));
        *right -= lay.cw / 2.0;
    }
}

// ----- the mouse -----

/// A left click on the sidebar's tabs or the panel. True when taken.
pub fn press(st: &mut State, x: f32, y: f32) -> bool {
    if !super::super::on(&st.ed) {
        return false;
    }
    let hits = &st.git.panel.hits;
    if let Some(&(_, b)) = hits.buttons.iter().find(|(r, _)| r.contains(x, y)) {
        button(st, b);
        st.after_input();
        return true;
    }
    if !shown(st) || !hits.area.is_some_and(|a| a.contains(x, y)) {
        return false;
    }
    let p = &mut st.git.panel;
    p.focused = true;
    st.ed.sidebar_focus = false;
    #[cfg(feature = "terminal")]
    st.terminals.set_focus(false);
    let p = &mut st.git.panel;
    if p.hits.message.is_some_and(|r| r.contains(x, y)) {
        p.focus = Focus::Message;
        // the caret goes to the clicked place
        if let Some(g) = p.hits.grid {
            let row = g.first + ((y - g.y) / g.ch).max(0.0) as usize;
            let col = ((x - g.x) / g.cw).round().max(0.0) as usize;
            p.message.cursor = p.message.index_at(g.cols, row, col);
        }
        return true;
    }
    let Some(&(_, i)) = p.hits.rows.iter().find(|(r, _)| r.contains(x, y)) else { return true };
    p.focus = Focus::List;
    p.selected = i;
    let now = std::time::Instant::now();
    let double = p.last_click.is_some_and(|(t, j)| j == i && now - t < std::time::Duration::from_millis(400));
    p.last_click = Some((now, i));
    match p.rows[i].clone() {
        Row::Header(s, _) => p.toggle_section(s),
        Row::File(f) if double => super::super::perform(st, Action::OpenFile(f.path, false)),
        Row::File(f) => super::super::perform(st, Action::OpenChange(f, true)),
    }
    st.after_input();
    true
}

fn button(st: &mut State, b: Button) {
    use super::super::{command, perform, show_files, show_panel};
    let p = &st.git.panel;
    let action = match b {
        Button::FilesTab => return show_files(st),
        Button::GitTab => return show_panel(st, Focus::List),
        Button::Sync => {
            let head = st.git.scm.status.as_ref().map(|s| s.head.clone()).unwrap_or_default();
            match sync(&head, false) {
                Some((_, cmd)) => return command(st, cmd, None),
                None => return,
            }
        }
        Button::Commit => return command(st, Command::GitCommit, None),
        Button::Amend => return command(st, Command::GitAmend, None),
        Button::Abort => return command(st, Command::GitAbort, None),
        Button::Continue => return command(st, Command::GitContinue, None),
        Button::Cancel => return command(st, Command::GitCancel, None),
        Button::Stage(i) => p.stage_action(i),
        Button::Unstage(i) => p.unstage_action(i),
        Button::Discard(i) => p.discard_action(i),
    };
    if let Button::Stage(i) | Button::Unstage(i) | Button::Discard(i) = b {
        st.git.panel.selected = i;
    }
    if let Some(a) = action {
        perform(st, a);
    }
}

/// A right click on a row: its menu (the commands act on the selected row).
pub fn right_press(st: &mut State, x: f32, y: f32) -> bool {
    if !shown(st) || !st.git.panel.hits.area.is_some_and(|a| a.contains(x, y)) {
        return false;
    }
    let p = &mut st.git.panel;
    use crate::mouse::{Panel, Target};
    let Some(&(_, i)) = p.hits.rows.iter().find(|(r, _)| r.contains(x, y)) else {
        // ----- the rest of the panel (the branch line, the commit box, below the rows): the panel's own commands
        p.focused = true;
        st.ed.sidebar_focus = false;
        let spec: &[&str] = &[
            "git_commit|Commit",
            "git_amend|Commit (Amend)",
            "-",
            "git_stage_all|Stage All Changes",
            "git_unstage_all|Unstage All Changes",
            "-",
            "git_pull|Pull",
            "git_push|Push",
            "git_fetch|Fetch",
            "-",
            "git_branches|Checkout Branch…",
            "git_new_branch|Create Branch…",
            "git_stash|Stash Changes…",
            "-",
            "git_log|Log",
            "git_refresh|Refresh",
        ];
        let menu = crate::context_menu::ContextMenu::from_spec(&st.ed, Target::Panel(Panel::Git(None)), spec);
        crate::context_menu::open(st, menu, (x, y));
        return true;
    };
    p.selected = i;
    p.focused = true;
    p.focus = Focus::List;
    let entries: &[&str] = match &p.rows[i] {
        Row::Header(Section::Changes, _) => {
            &["git_stage|Stage All", "git_discard|Discard All Changes…", "-", "git_refresh|Refresh"]
        }
        Row::Header(Section::Untracked, _) => {
            &["git_stage|Stage All", "git_discard|Delete All…", "-", "git_refresh|Refresh"]
        }
        Row::Header(Section::Staged, _) => {
            &["git_unstage|Unstage All", "git_commit|Commit", "-", "git_refresh|Refresh"]
        }
        Row::Header(Section::Merge, _) => {
            &["git_stage|Mark All Resolved", "git_abort|Abort…", "-", "git_refresh|Refresh"]
        }
        Row::File(f) => match f.section {
            Section::Changes => &[
                "git_open_change|Open Change",
                "git_open_file|Open File",
                "-",
                "git_stage|Stage",
                "git_discard|Discard Changes…",
                "-",
                "git_file_history|File History",
            ],
            Section::Staged => &[
                "git_open_change|Open Change",
                "git_open_file|Open File",
                "-",
                "git_unstage|Unstage",
                "-",
                "git_file_history|File History",
            ],
            Section::Untracked => &["git_open_file|Open File", "-", "git_stage|Stage", "git_discard|Delete…"],
            Section::Merge => &["git_open_file|Open File", "-", "git_stage|Mark Resolved", "git_abort|Abort…"],
        },
    };
    let menu = crate::context_menu::ContextMenu::from_spec(&st.ed, Target::Panel(Panel::Git(Some(i))), entries);
    crate::context_menu::open(st, menu, (x, y));
    true
}

pub fn wheel(st: &mut State, x: f32, y: f32, lines: f32) -> bool {
    if !shown(st) || !st.git.panel.hits.area.is_some_and(|a| a.contains(x, y)) {
        return false;
    }
    if lines.abs() >= 0.5 {
        st.git.panel.scroll(lines.round() as isize);
    }
    true
}

/// The row under the pointer shows its buttons. (redraw, over something clickable)
pub fn hover(st: &mut State, x: f32, y: f32) -> (bool, bool) {
    let p = &mut st.git.panel;
    let over_button = p.hits.buttons.iter().any(|(r, _)| r.contains(x, y));
    let row = if p.hits.area.is_some_and(|a| a.contains(x, y)) {
        p.hits.rows.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1)
    } else {
        None
    };
    let changed = row != p.hover;
    p.hover = row;
    (changed, over_button || row.is_some())
}
