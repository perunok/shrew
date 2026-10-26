//! A diff tab's context menu: a right click on a row (or Shift+F10 at the cursor row). The row clicked becomes the
//! cursor, on the side clicked, unless it is inside the selected rows, which are kept. The entries are what the
//! tab's keys do (shown as their hints, the letters underlined): stage, unstage, revert the change or the selected
//! lines; open the file there; copy the old or new text; the layout and white space.

use super::view::Side;
use super::{Action as Act, DiffView, act, is_active};
use crate::context_menu::{Action, ContextMenu, MenuItem};
use crate::mouse::Target;
use crate::state::State;
use shrew_core::diffview::{Layout, RowKind};

/// The menu after a right click at `at` on `row` (none: elsewhere on the tab).
pub fn menu(st: &mut State, row: Option<usize>, at: (f32, f32)) -> Option<ContextMenu> {
    if !is_active(st) {
        return None;
    }
    let buf = st.ed.buf().id;
    let v = st.git.diff.views.get_mut(&buf)?;
    if v.stale {
        v.rebuild();
    }
    if let Some(row) = row {
        let kept = v.anchor.is_some_and(|a| a.min(v.cursor) <= row && row <= a.max(v.cursor));
        if !kept {
            v.anchor = None;
            v.cursor = row;
        }
        v.side = if !v.hits.old_pane.is_empty() && at.0 < v.hits.new_pane.x { Side::Old } else { Side::New };
    }
    let v = st.git.diff.views.get(&buf)?;
    Some(items(v, Target::Diff(row)))
}

/// Shift+F10 on a diff tab: its menu, and where it goes (see [`cursor_at`]).
pub fn cursor_menu(st: &State) -> Option<(ContextMenu, (f32, f32))> {
    let v = st.git.diff.active(&st.ed)?;
    Some((items(v, Target::Diff(Some(v.cursor))), cursor_at(st)?))
}

/// Under the active diff tab's cursor row, on its side, as last drawn.
pub fn cursor_at(st: &State) -> Option<(f32, f32)> {
    let v = st.git.diff.active(&st.ed)?;
    let h = &v.hits;
    let off = v.cursor.saturating_sub(h.first) as f32 + 1.0;
    let y = (h.y0 + off * h.row_h).clamp(h.body.y, h.body.bottom());
    let pane = if v.side == Side::Old && !h.old_pane.is_empty() { h.old_pane } else { h.new_pane };
    Some((pane.x + h.row_h, y))
}

fn items(v: &DiffView, target: Target) -> ContextMenu {
    let can = v.actions();
    let change = v.target_rows().is_some();
    let what = if v.anchor.is_some() { "Selected Lines" } else { "Change" };
    let key = |k: &str| Some(k.to_string());
    let line = v.rows.rows.get(v.cursor).is_some_and(|r| matches!(r.kind, RowKind::Same | RowKind::Change));
    let split = v.opts.layout == Layout::Split;
    let items: Vec<MenuItem> = vec![
        Action::app_with(format!("Stage {what}"), run_act, 0)
            .hint(key("s"))
            .enabled(can.stage && change)
            .mnemonic('s')
            .into(),
        Action::app_with(format!("Unstage {what}"), run_act, 1)
            .hint(key("u"))
            .enabled(can.unstage && change)
            .mnemonic('u')
            .into(),
        Action::app_with(format!("Revert {what}"), run_act, 2).hint(key("X")).enabled(can.revert && change).into(),
        MenuItem::Separator,
        Action::app("Open File at This Line", open_file)
            .hint(key("o"))
            .enabled(v.repo.is_some() && line)
            .mnemonic('o')
            .into(),
        Action::app_with("Copy Old Text", copy_side, 0).enabled(side_text(v, Side::Old).is_some()).into(),
        Action::app_with("Copy New Text", copy_side, 1).enabled(side_text(v, Side::New).is_some()).into(),
        MenuItem::Separator,
        Action::app(if split { "Switch to Unified" } else { "Switch to Split" }, layout)
            .hint(key("t"))
            .mnemonic('t')
            .into(),
        Action::app("Ignore White Space", whitespace).hint(key("w")).checked(v.ignore_ws).mnemonic('w').into(),
    ];
    ContextMenu::new(target, items)
}

/// The lines of one side of the selected rows, else of the change at the cursor, else of the cursor's row.
fn side_text(v: &DiffView, side: Side) -> Option<String> {
    let rows = v.target_rows().unwrap_or(v.cursor..v.cursor + 1);
    let lines: Vec<String> = rows.filter_map(|r| v.line(r, side).map(|l| l.into_owned())).collect();
    if lines.is_empty() {
        return None;
    }
    let mut s = lines.join("\n");
    s.push('\n');
    Some(s)
}

fn run_act(st: &mut State, _: &Target, which: usize) {
    let a = [Act::Stage, Act::Unstage, Act::Revert][which.min(2)];
    let buf = st.ed.buf().id;
    act(st, buf, a);
}

fn open_file(st: &mut State, _: &Target, _: usize) {
    let buf = st.ed.buf().id;
    super::open_file_at_cursor(st, buf);
}

fn copy_side(st: &mut State, _: &Target, which: usize) {
    let side = if which == 0 { Side::Old } else { Side::New };
    let Some(text) = st.git.diff.active(&st.ed).and_then(|v| side_text(v, side)) else { return };
    let n = text.lines().count();
    st.ed.clipboard.set(text);
    let what = if side == Side::Old { "old" } else { "new" };
    st.ed.info(if n == 1 { format!("copied 1 {what} line") } else { format!("copied {n} {what} lines") });
}

fn layout(st: &mut State, _: &Target, _: usize) {
    let Some(v) = st.git.diff.active(&st.ed) else { return };
    let next = if v.opts.layout == Layout::Split { Layout::Unified } else { Layout::Split };
    super::set_layout(st, next);
}

fn whitespace(st: &mut State, _: &Target, _: usize) {
    let buf = st.ed.buf().id;
    super::toggle_whitespace(st, buf);
}
