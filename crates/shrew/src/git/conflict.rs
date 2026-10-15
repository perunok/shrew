//! Merge conflicts in the text: while git says a file is conflicted (or a merge or rebase is going on), its
//! regions are tinted (the current side, the common ancestor, the incoming side) and the `<<<<<<<` line offers
//! Accept Current | Accept Incoming | Accept Both | Compare. The edits themselves are the core's
//! ([`shrew_core::scm`]), one undo step each; `]x` / `[x` jump between conflicts.

use super::on;
use crate::colors::{Colors, mix};
use crate::draw::Ui;
use crate::layout::{Layout, Scroll};
use crate::state::State;
use shrew_core::scm::{self, Conflict, Keep};
use shrew_core::{Document, text};
use shrew_git::RepoState;
use shrew_git::diff::{EolMode, diff, unified};
use shrew_render::{Rect, Scene, TextStyle, columns};

/// An action above a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Keep(Keep),
    Compare,
}

const ACTIONS: [(&str, Act); 4] = [
    ("Accept Current", Act::Keep(Keep::Ours)),
    ("Accept Incoming", Act::Keep(Keep::Theirs)),
    ("Accept Both", Act::Keep(Keep::Both)),
    ("Compare", Act::Compare),
];

#[derive(Default)]
pub struct Conflicts {
    /// (buffer, version) and the conflicts found in it.
    cache: Option<(u64, u64, Vec<Conflict>)>,
    /// The actions drawn in the last frame: rect, the conflict's first line, what it does.
    pub hits: Vec<(Rect, usize, Act)>,
}

/// Whether the active file is worth scanning: git lists it as conflicted, or an operation is in progress.
fn watched(st: &State) -> bool {
    let Some(s) = &st.git.status else { return false };
    if s.head.state != RepoState::Clean {
        return true;
    }
    let Some(rel) = st.ed.buf().doc.path.as_deref().and_then(|p| st.git.rel_path(p)) else { return false };
    s.get(&rel).is_some_and(|e| e.conflict.is_some())
}

/// The active buffer's conflicts (scanned once per version).
fn current(st: &mut State) -> &[Conflict] {
    let b = st.ed.buf();
    let key = (b.id, b.doc.version);
    let fresh = st.git.conflicts.cache.as_ref().is_some_and(|c| (c.0, c.1) == key);
    if !fresh {
        let found = scm::conflicts(&b.doc.rope);
        st.git.conflicts.cache = Some((key.0, key.1, found));
    }
    &st.git.conflicts.cache.as_ref().expect("just set").2
}

pub fn draw(scene: &mut Scene, st: &mut State, ui: &Ui, lay: &Layout, c: &Colors, scroll: Scroll) {
    let _ = ui;
    if !watched(st) {
        return;
    }
    let all = current(st).to_vec();
    if all.is_empty() {
        return;
    }
    let rope = st.ed.buf().doc.rope.clone();
    let tw = st.ed.settings.tab_width.max(1);
    let first = scroll.first_line();
    let end = first + lay.rows + 2;
    let area = lay.text_rect();
    let ours = c.git.diff_add_bg;
    let theirs = mix(c.accent_alt, c.background, 0.16);
    let base = mix(c.panel_muted, c.background, 0.12);
    let strong = |col| mix(col, c.background, 0.75);
    scene.push_clip(Rect::new(lay.text.x, area.y, lay.text.w, area.h));
    let band = |scene: &mut Scene, a: usize, z: usize, col| {
        let (a, z) = (a.max(first), z.min(end));
        if a < z {
            let y0 = scroll.line_y(lay, a);
            scene.rect(Rect::new(lay.text.x, y0, lay.text.w, scroll.line_y(lay, z) - y0), col);
        }
    };
    let mut hits = Vec::new();
    for k in &all {
        if k.end < first || k.start > end {
            continue;
        }
        band(scene, k.start, k.start + 1, strong(mix(c.git.added, c.background, 0.3)));
        band(scene, k.ours().start, k.ours().end, ours);
        if let Some(b) = k.base_lines() {
            band(scene, k.base.unwrap_or(0), k.base.unwrap_or(0) + 1, strong(base));
            band(scene, b.start, b.end, base);
        }
        band(scene, k.sep, k.sep + 1, strong(base));
        band(scene, k.theirs().start, k.theirs().end, theirs);
        band(scene, k.end, k.end + 1, strong(mix(c.accent_alt, c.background, 0.3)));
        // the actions, after the marker's text
        if (first..end).contains(&k.start) {
            let y = scroll.line_y(lay, k.start);
            let len = text::col_of(&rope, text::line_end(&rope, k.start), tw);
            let mut x = scroll.col_x(lay, (len + 4).max(16) as f32);
            for (i, (label, act)) in ACTIONS.iter().enumerate() {
                if i > 0 {
                    scene.text(x.round(), y, "|", TextStyle::new(c.panel_muted), &[]);
                    x += 2.0 * lay.cw;
                }
                let w = columns(label) as f32 * lay.cw;
                let r = Rect::new(x.round(), y, w.round(), lay.ch);
                scene.text(
                    x.round(),
                    y,
                    label,
                    TextStyle::new(c.accent_alt).underline(mix(c.accent_alt, c.background, 0.4)),
                    &[],
                );
                hits.push((r, k.start, *act));
                x += w + lay.cw;
            }
        }
    }
    scene.pop_clip();
    st.git.conflicts.hits = hits;
}

/// A click on a conflict's action.
pub fn press(st: &mut State, x: f32, y: f32) -> bool {
    if !on(&st.ed) {
        return false;
    }
    let Some(&(_, line, act)) = st.git.conflicts.hits.iter().find(|(r, ..)| r.contains(x, y)) else { return false };
    match act {
        Act::Keep(k) => {
            st.ed.resolve_conflict(Some(line), k);
        }
        Act::Compare => {
            st.ed.goto_line(line + 1);
            compare(st);
        }
    }
    st.after_input();
    true
}

/// `git_conflict_compare`: the conflict at the cursor as a diff from the current side to the incoming one, in a
/// buffer of its own. (A diff view, where the build has one, can take this over.)
pub fn compare(st: &mut State) {
    let b = st.ed.buf();
    let rope = &b.doc.rope;
    let line = text::line_of(rope, b.sel.primary().head);
    let all = scm::conflicts(rope);
    let Some(k) = all.iter().find(|k| k.contains(line)).or_else(|| all.iter().find(|k| k.start > line)) else {
        return st.ed.error("no merge conflict here");
    };
    let side = |r: std::ops::Range<usize>| -> String {
        if r.is_empty() {
            return String::new();
        }
        rope.slice(text::line_start(rope, r.start)..text::line_start(rope, r.end)).to_string()
    };
    let (a, z) =
        (shrew_git::Text::new(side(k.ours()).into_bytes()), shrew_git::Text::new(side(k.theirs()).into_bytes()));
    let hunks = diff(&a, &z, EolMode::Normalize);
    let name = b.doc.display_name();
    let mut out = format!("--- current ({name}, line {})\n+++ incoming\n", k.start + 1);
    out += &String::from_utf8_lossy(&unified(&a, &z, &hunks, 3));
    if hunks.is_empty() {
        out += "(both sides are the same)\n";
    }
    let mut doc = Document::from_str(&out);
    doc.title = Some(format!("Conflict: {name}"));
    st.ed.install_document(doc);
    st.process_events();
}
