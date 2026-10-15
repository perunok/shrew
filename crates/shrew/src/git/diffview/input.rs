//! Keys and the mouse in a diff tab.
//!
//! Keys, vim-style, with plain-key fallbacks: `j`/`k` (arrows) move, Ctrl+D/U half a page, Ctrl+F/B (PageDown/Up) a
//! page, `gg`/`G` (Home/End) the ends, `]c`/`[c` (F7/Shift+F7, `n`/`N` without a search) the next or previous
//! change, `]f`/`[f` the next file, Tab the other side, `h`/`l` scroll sideways. Enter opens a fold or a file
//! header, or the file at the line; `o` opens the file, Space/`za` folds, `zR`/`zM` all folds. `s` stages, `u`
//! unstages, `-` either, `X` reverts; `v`/`V` select lines for them, `y` copies. `t` switches split/unified, `w`
//! white space, `<`/`>` context lines, `r` reloads, `/` searches, `q` closes. The palette, Ctrl+P, Ctrl+Tab and the
//! other app-wide chords keep working.

use super::view::{Button, Grab, Side};
use super::{Action, act, is_active};
use crate::state::State;
use shrew_core::diffview::{Layout, RowKind};
use shrew_core::{Command, Key, KeyPress, Mods, PromptKind};
use std::time::{Duration, Instant};

/// App-wide commands whose chords work over a diff tab.
const PASS: &[Command] = &[
    Command::CommandPalette,
    Command::OpenFile,
    Command::SwitchBuffer,
    Command::NextBuffer,
    Command::PrevBuffer,
    Command::Quit,
    Command::SaveAll,
    Command::FontBigger,
    Command::FontSmaller,
    Command::FontReset,
    Command::ToggleSidebar,
    Command::FocusSidebar,
    Command::FindInProject,
    Command::GotoSymbolInProject,
    Command::OpenRecent,
    Command::OpenRecentProject,
    Command::ReopenClosed,
    Command::JumpBack,
    Command::JumpForward,
    Command::ToggleTerminal,
    Command::NewTerminal,
    Command::Settings,
    Command::CheckForUpdates,
    Command::CloseBuffer,
    Command::GitPanel,
];

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// A key for the active diff tab. False when it is not one, or the key belongs to the editor (a prompt, a
/// picker, the sidebar, an app-wide chord).
pub fn key(st: &mut State, k: KeyPress) -> bool {
    if !is_active(st)
        || st.ed.picker.is_some()
        || st.ed.prompt.is_some()
        || st.ed.about.is_some()
        || st.ed.sidebar_focus
    {
        return false;
    }
    if let Some(cmd) = st.ed.binding(k)
        && PASS.contains(&cmd)
    {
        return false;
    }
    let buf = st.ed.buf().id;
    let so = st.ed.settings.scrolloff;
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return false };
    if v.stale {
        v.rebuild();
    }
    let m = k.mods;
    let plain = !m.ctrl && !m.alt && !m.logo;
    // a prefix waiting for its second key
    if let Some(p) = v.prefix.take()
        && let Key::Char(c) = k.key
        && plain
    {
        let count = v.count.take().unwrap_or(1);
        match (p, c) {
            ('g', 'g') => v.goto(count.saturating_sub(1), so),
            (']', 'c') | ('[', 'c') => {
                for _ in 0..count {
                    if !v.jump_change(p == ']') {
                        break;
                    }
                }
            }
            (']', 'f') | ('[', 'f') => {
                for _ in 0..count {
                    if !v.jump_file(p == ']') {
                        break;
                    }
                }
            }
            ('z', 'a' | 'o') => {
                v.toggle_at_cursor();
            }
            ('z', 'c') if v.multi => {
                let f = v.rows.rows.get(v.cursor).map(|r| r.file as usize);
                if let Some(f) = f {
                    v.sections[f].collapsed = true;
                    v.stale = true;
                    v.rebuild();
                    v.cursor = v.rows.files[f] as usize;
                    v.reveal(so);
                }
            }
            ('z', 'R') => v.unfold_all(true, st.git.diff.defaults.context),
            ('z', 'M') => v.unfold_all(false, st.git.diff.defaults.context),
            ('z', 'z') => v.center(),
            _ => {}
        }
        super::ensure_loaded(st, buf);
        return true;
    }
    let count = |v: &mut super::DiffView| v.count.take().unwrap_or(1);
    let page = v.hits.page.max(1);
    match k.key {
        Key::Char(d @ '0'..='9') if plain && (d != '0' || v.count.is_some()) => {
            let n = v.count.unwrap_or(0).saturating_mul(10).saturating_add(d as usize - '0' as usize);
            v.count = Some(n.min(1_000_000));
            return true;
        }
        Key::Char('0') if plain => v.left_col = 0,
        Key::Esc => {
            v.anchor = None;
            v.count = None;
            v.armed = false;
            st.ed.search_visible = false;
        }
        Key::Char('[') if m.ctrl => {
            v.anchor = None;
            v.count = None;
        }
        Key::Char('j') | Key::Down if plain => {
            let n = count(v);
            v.goto(v.cursor + n, so);
        }
        Key::Char('k') | Key::Up if plain => {
            let n = count(v);
            v.goto(v.cursor.saturating_sub(n), so);
        }
        Key::Char('e') if m.ctrl => {
            let n = count(v) as isize;
            v.scroll(n, so);
        }
        Key::Char('y') if m.ctrl => {
            let n = count(v) as isize;
            v.scroll(-n, so);
        }
        Key::Char('d') if m.ctrl => {
            let n = (page / 2).max(1);
            v.top += n;
            v.goto(v.cursor + n, so);
        }
        Key::Char('u') if m.ctrl => {
            let n = (page / 2).max(1);
            v.top = v.top.saturating_sub(n);
            v.goto(v.cursor.saturating_sub(n), so);
        }
        Key::Char('f') if m.ctrl => {
            v.top += page;
            v.goto(v.cursor + page, so);
        }
        Key::PageDown => {
            v.top += page;
            v.goto(v.cursor + page, so);
        }
        Key::Char('b') if m.ctrl => {
            v.top = v.top.saturating_sub(page);
            v.goto(v.cursor.saturating_sub(page), so);
        }
        Key::PageUp => {
            v.top = v.top.saturating_sub(page);
            v.goto(v.cursor.saturating_sub(page), so);
        }
        Key::Home if m.ctrl || plain => v.goto(0, so),
        Key::End if m.ctrl || plain => v.goto(usize::MAX, so),
        Key::Char('G') if plain => {
            let target = v.count.take().map_or(usize::MAX, |n| n.saturating_sub(1));
            v.goto(target, so);
        }
        Key::Char(c @ ('g' | ']' | '[' | 'z')) if plain => {
            v.prefix = Some(c);
            return true;
        }
        Key::F(7) => {
            if !v.jump_change(!m.shift) {
                st.ed.info(if m.shift { "no change above" } else { "no change below" });
            }
        }
        Key::Char(c @ ('n' | 'N')) if plain => {
            let forward = c == 'n';
            let pattern = st.ed.search.clone().filter(|_| !st.ed.search_text.is_empty());
            let found = match pattern {
                Some(re) => {
                    let v = st.git.diff.views.get_mut(&buf).expect("looked up above");
                    search(v, &|s: &str| re.is_match(s), forward)
                }
                None => {
                    let v = st.git.diff.views.get_mut(&buf).expect("looked up above");
                    v.jump_change(forward)
                }
            };
            if !found {
                st.ed.info(if forward { "nothing further down" } else { "nothing further up" });
            }
            return true;
        }
        Key::Tab => {
            v.side = if v.side == Side::New { Side::Old } else { Side::New };
        }
        Key::Char('h') | Key::Left if plain => v.left_col = v.left_col.saturating_sub(4 * count(v)),
        Key::Char('l') | Key::Right if plain => v.left_col += 4 * count(v),
        Key::Enter => {
            if !v.toggle_at_cursor() {
                super::open_file_at_cursor(st, buf);
                return true;
            }
        }
        Key::Char(' ') if plain => {
            v.toggle_at_cursor();
        }
        Key::Char('o') if plain => {
            super::open_file_at_cursor(st, buf);
            return true;
        }
        Key::Char('v' | 'V') if plain => {
            v.anchor = if v.anchor.is_some() { None } else { Some(v.cursor) };
        }
        Key::Char('s') if plain => {
            act(st, buf, Action::Stage);
            return true;
        }
        Key::Char('u') if plain => {
            act(st, buf, Action::Unstage);
            return true;
        }
        Key::Char('-') if plain => {
            act(st, buf, Action::Toggle);
            return true;
        }
        Key::Char('X') if plain => {
            act(st, buf, Action::Revert);
            return true;
        }
        Key::Char('y') if plain => {
            super::yank(st, buf);
            return true;
        }
        Key::Char('t') if plain => {
            let next = if v.opts.layout == Layout::Split { Layout::Unified } else { Layout::Split };
            super::set_layout(st, next);
            return true;
        }
        Key::Char('w') if plain => {
            super::toggle_whitespace(st, buf);
            return true;
        }
        Key::Char(c @ ('<' | '>')) if !m.ctrl && !m.alt => {
            v.change_context(if c == '>' { 1 } else { -1 });
            persist_context(st, buf);
            return true;
        }
        Key::Char('r' | 'R') if plain => {
            super::reload(st, buf);
            st.ed.info("reloaded");
            return true;
        }
        Key::Char('/' | '?') if plain => {
            let forward = k.key == Key::Char('/');
            st.ed.open_prompt(PromptKind::Search { forward });
            return true;
        }
        Key::Char('f') if m.ctrl && !st.ed.vim() => {
            st.ed.open_prompt(PromptKind::Search { forward: true });
            return true;
        }
        Key::Char(':') if !m.ctrl && !m.alt => {
            st.ed.open_prompt(PromptKind::Command);
            return true;
        }
        Key::Char('q') if plain => {
            st.ed.close_buffer(true);
            return true;
        }
        Key::Char('w') if m.ctrl && !st.ed.vim() => {
            st.ed.close_buffer(true);
            return true;
        }
        _ => {}
    }
    v.count = None;
    if !matches!(k.key, Key::Char('X')) {
        v.armed = false;
    }
    super::ensure_loaded(st, buf);
    true
}

/// Saves the context-line count as the default for new views.
fn persist_context(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get(&buf) else { return };
    let ctx = v.opts.context;
    let label = if ctx == shrew_core::diffview::ALL { "all lines".to_string() } else { format!("{ctx} lines") };
    if ctx != shrew_core::diffview::ALL {
        st.git.diff.defaults.context = ctx;
        crate::settings::save_with(st, |p| shrew_config::persist_value(p, "git", "diff_context", ctx as i64));
    }
    st.ed.info(format!("context: {label}"));
}

/// The next row (after or before the cursor, wrapping) where `is_match` finds something on either side.
pub fn search(v: &mut super::DiffView, is_match: &dyn Fn(&str) -> bool, forward: bool) -> bool {
    let n = v.rows.len();
    if n == 0 {
        return false;
    }
    for step in 1..=n {
        let row = if forward { (v.cursor + step) % n } else { (v.cursor + n - step % n) % n };
        for side in [v.side, if v.side == Side::New { Side::Old } else { Side::New }] {
            if v.line(row, side).is_some_and(|l| is_match(&l)) {
                v.cursor = row;
                v.side = side;
                v.center();
                return true;
            }
        }
    }
    false
}

/// After a search typed at the prompt (`/`): the view moves to the first match. The editor searched its empty
/// buffer and complained; that message is replaced.
pub fn follow_search(st: &mut State, buf: u64) {
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return };
    if st.ed.prompt.is_some() || v.search_seen == st.ed.search_text {
        return;
    }
    v.search_seen = st.ed.search_text.clone();
    let Some(re) = st.ed.search.clone().filter(|_| !st.ed.search_text.is_empty()) else { return };
    if v.stale {
        v.rebuild();
    }
    let found = search(v, &|s: &str| re.is_match(s), true);
    let t = st.ed.search_text.clone();
    if found {
        st.ed.message = None;
    } else {
        st.ed.error(format!("pattern not found: {t}"));
    }
}

// ----- mouse -----

/// A left press over the active diff tab. True when it took it.
pub fn press(st: &mut State, x: f32, y: f32, mods: Mods, now: Instant) -> bool {
    if !is_active(st) {
        return false;
    }
    let buf = st.ed.buf().id;
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return false };
    if let Some(&(_, b)) = v.hits.buttons.iter().find(|(r, _)| r.contains(x, y)) {
        button(st, buf, b);
        return true;
    }
    if let Some(&(_, row)) = v.hits.actions.iter().find(|(r, _)| r.contains(x, y)) {
        v.cursor = row;
        v.anchor = None;
        act(st, buf, Action::Toggle);
        return true;
    }
    let sb = v.hits.scrollbar;
    if sb.contains(x, y) {
        v.grab = Some(Grab::Scrollbar);
        scroll_to(v, y);
        return true;
    }
    if !v.hits.body.contains(x, y) {
        // the header bar and the pane labels are part of the tab too
        return v.hits.body.x <= x && x <= v.hits.body.right() + sb.w && y < v.hits.body.y;
    }
    let Some(row) = row_at(v, y) else { return true };
    if !v.hits.old_pane.is_empty() && x < v.hits.new_pane.x {
        v.side = Side::Old;
    } else {
        v.side = Side::New;
    }
    let double = v.last_click.is_some_and(|(r, t)| r == row && now.duration_since(t) < DOUBLE_CLICK);
    v.last_click = Some((row, now));
    if mods.shift {
        v.anchor.get_or_insert(v.cursor);
        v.cursor = row;
        return true;
    }
    v.anchor = None;
    v.cursor = row;
    v.grab = Some(Grab::Rows(row));
    let kind = v.rows.rows[row].kind;
    match kind {
        // one click opens a fold or a file
        RowKind::Fold | RowKind::File => {
            v.grab = None;
            v.toggle_at_cursor();
            super::ensure_loaded(st, buf);
        }
        _ if double => {
            v.grab = None;
            super::open_file_at_cursor(st, buf);
        }
        _ => {}
    }
    true
}

/// The pointer moved with the button held. True when the view changed.
pub fn drag(st: &mut State, _x: f32, y: f32) -> bool {
    let buf = st.ed.buf().id;
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return false };
    match v.grab {
        Some(Grab::Scrollbar) => {
            scroll_to(v, y);
            true
        }
        Some(Grab::Rows(from)) => {
            // past the edges: scroll a row per move
            if y < v.hits.body.y {
                v.top = v.top.saturating_sub(1);
            } else if y >= v.hits.body.bottom() {
                v.top += 1;
                v.clamp_top();
            }
            let row = row_at(v, y.clamp(v.hits.body.y, v.hits.body.bottom() - 1.0)).unwrap_or(v.cursor);
            if row != from || v.anchor.is_some() {
                v.anchor = Some(from);
            }
            v.cursor = row;
            true
        }
        None => false,
    }
}

pub fn release(st: &mut State) {
    let buf = st.ed.buf().id;
    if let Some(v) = st.git.diff.views.get_mut(&buf) {
        v.grab = None;
    }
}

/// The wheel over the active diff tab: rows, or columns with Shift or a sideways swipe. True when it took it.
pub fn wheel(st: &mut State, x: f32, y: f32, rows: f32, cols: f32, shift: bool) -> bool {
    if !is_active(st) {
        return false;
    }
    let buf = st.ed.buf().id;
    let Some(v) = st.git.diff.views.get_mut(&buf) else { return false };
    let area = v.hits.body;
    if !(area.x <= x && x <= area.right() + v.hits.scrollbar.w && area.y <= y && y <= area.bottom()) {
        return false;
    }
    let (rows, cols) = if shift { (0.0, rows) } else { (rows, cols * 2.0) };
    let (ar, ac) = (v.wheel_acc.0 + rows, v.wheel_acc.1 + cols);
    let (dr, dc) = (ar.trunc(), ac.trunc());
    v.wheel_acc = (ar - dr, ac - dc);
    if dr != 0.0 {
        v.top = (v.top as isize + dr as isize).max(0) as usize;
        v.clamp_top();
    }
    if dc != 0.0 {
        v.left_col = (v.left_col as isize + dc as isize).max(0) as usize;
    }
    true
}

/// Whether the pointer is over something clickable (a button, a fold, a file header, a hunk action).
pub fn hover(st: &State, x: f32, y: f32) -> bool {
    let Some(v) = st.git.diff.active(&st.ed) else { return false };
    if v.hits.buttons.iter().any(|(r, _)| r.contains(x, y)) || v.hits.actions.iter().any(|(r, _)| r.contains(x, y)) {
        return true;
    }
    if !v.hits.body.contains(x, y) {
        return false;
    }
    row_at(v, y).is_some_and(|r| matches!(v.rows.rows[r].kind, RowKind::Fold | RowKind::File))
}

fn row_at(v: &super::DiffView, y: f32) -> Option<usize> {
    let h = v.hits.row_h.max(1.0);
    let off = ((y - v.hits.y0) / h).floor();
    if off < 0.0 {
        return None;
    }
    let row = v.hits.first + off as usize;
    (row < v.rows.len()).then_some(row)
}

fn scroll_to(v: &mut super::DiffView, y: f32) {
    let t = v.hits.scrollbar;
    let f = ((y - t.y) / t.h.max(1.0)).clamp(0.0, 1.0);
    let page = v.hits.page.max(1);
    v.top = ((f * v.rows.len() as f32) as usize).saturating_sub(page / 2);
    v.clamp_top();
    v.cursor = v.cursor.clamp(v.top, v.top + page.saturating_sub(1)).min(v.rows.len().saturating_sub(1));
}

fn button(st: &mut State, buf: u64, b: Button) {
    match b {
        Button::Split => super::set_layout(st, Layout::Split),
        Button::Unified => super::set_layout(st, Layout::Unified),
        Button::Whitespace => super::toggle_whitespace(st, buf),
        Button::LessContext | Button::MoreContext => {
            if let Some(v) = st.git.diff.views.get_mut(&buf) {
                v.change_context(if b == Button::MoreContext { 1 } else { -1 });
            }
            persist_context(st, buf);
        }
        Button::Stage => act(st, buf, Action::Stage),
        Button::Unstage => act(st, buf, Action::Unstage),
        Button::Revert => act(st, buf, Action::Revert),
        Button::NextChange | Button::PrevChange => {
            if let Some(v) = st.git.diff.views.get_mut(&buf)
                && !v.jump_change(b == Button::NextChange)
            {
                st.ed.info("no more changes");
            }
        }
    }
}
