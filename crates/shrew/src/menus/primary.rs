//! The primary selection, as on X11 and Wayland: whatever is selected in the text (by the mouse or the keys, vim's
//! visual mode included) is offered to other programs, and a middle click pastes what is selected anywhere at the
//! pointer. It goes through the `clipboard` feature (and its runtime switch); macOS and Windows have no primary
//! selection, so there a middle click in the text does nothing. The terminal panel takes part too (`terminal`).
//!
//! Cost: after each input a fingerprint of the selection (no allocation) says whether it changed; only then is its
//! text gathered, and not at all past [`MAX_CHARS`] (a select-all of a huge file stays off the primary selection).

use crate::state::State;
use shrew_core::features::Feature;
use shrew_core::{Key, KeyPress, Mods};

/// Selections longer than this (chars) are not offered.
pub const MAX_CHARS: usize = 1 << 20;

/// Offers the selection as the primary selection when it changed since the last time. Called after every input;
/// a selection still being dragged waits for the button to come up.
pub fn sync(st: &mut State) {
    let ed = &st.ed;
    if !ed.features.on(Feature::Clipboard) || ed.mouse_dragging() || ed.buffers.is_empty() {
        return;
    }
    let b = ed.buf();
    let visual = ed.mode.is_visual();
    if !visual && b.sel.ranges().iter().all(|r| r.is_empty()) {
        // nothing selected: the primary selection stays (as everywhere); selecting the same text again re-offers it
        st.menus.primary = 0;
        return;
    }
    let fp = fingerprint(ed);
    if fp == st.menus.primary {
        return;
    }
    st.menus.primary = fp;
    let spans = ed.selection_spans();
    let total: usize = spans.iter().map(|&(a, z)| z - a).sum();
    if total == 0 || total > MAX_CHARS {
        return;
    }
    let rope = b.rope();
    let mut s = String::with_capacity(total + spans.len());
    for (i, &(a, z)) in spans.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        s.extend(rope.slice(a..z).chunks());
    }
    st.ed.clipboard.set_primary(s);
}

/// The buffer, its text's version, the mode and every range, folded into a number (never 0).
fn fingerprint(ed: &shrew_core::Editor) -> u64 {
    let b = ed.buf();
    // FNV-1a over a few words per range
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    mix(b.id);
    mix(b.doc.version);
    mix(ed.mode as u64);
    for r in b.sel.ranges() {
        mix(r.anchor as u64);
        mix(r.head as u64);
    }
    h.max(1)
}

/// A middle click at char `pos` of the text: the cursor goes there (a selection is dropped) and the primary
/// selection is put in, in any mode (one undo step). False when there is nothing to paste.
pub fn paste(st: &mut State, pos: usize) -> bool {
    if !st.ed.features.on(Feature::Clipboard) {
        return false;
    }
    let Some(text) = st.ed.clipboard.get_primary().filter(|t| !t.is_empty()) else { return false };
    if st.ed.prompt.is_some() {
        st.key(KeyPress::plain(Key::Esc));
    }
    st.ed.sidebar_focus = false;
    #[cfg(feature = "terminal")]
    if st.terminals.focused {
        st.terminals.set_focus(false);
    }
    st.ed.mouse_down(pos, 1, Mods::default());
    st.ed.mouse_up();
    st.ed.put_text(&text);
    st.after_input();
    true
}
