//! Undo/redo. Edits accumulate in an open group until `commit` (end of a normal-mode command, leaving insert mode,
//! a pause in typing); undo and redo work on whole groups.

use crate::change::ChangeSet;
use crate::selection::Selection;

#[derive(Clone, Debug)]
struct Revision {
    forward: ChangeSet,
    inverse: ChangeSet,
}

#[derive(Clone, Debug)]
struct Group {
    revs: Vec<Revision>,
    sel_before: Selection,
    sel_after: Selection,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Group>,
    redo: Vec<Group>,
    open: Option<Group>,
    /// While held, `commit` keeps the group open: a macro, `:g` or `:normal` is one undo step, like in vim.
    hold: u32,
}

/// Change sets to apply (in order) and the selection to restore afterwards.
pub struct Step {
    pub changes: Vec<ChangeSet>,
    pub selection: Selection,
    /// The selection when the group started (vim puts the cursor back relative to it).
    pub before: Selection,
}

impl History {
    pub fn record(&mut self, forward: ChangeSet, inverse: ChangeSet, sel_before: &Selection, sel_after: &Selection) {
        self.redo.clear();
        let g = self.open.get_or_insert_with(|| Group {
            revs: Vec::new(),
            sel_before: sel_before.clone(),
            sel_after: sel_after.clone(),
        });
        g.revs.push(Revision { forward, inverse });
        g.sel_after = sel_after.clone();
    }

    /// Updates the selection restored by redo (e.g. the cursor moved inside the same insert session).
    pub fn set_open_after(&mut self, sel: &Selection) {
        if let Some(g) = &mut self.open {
            g.sel_after = sel.clone();
        }
    }

    pub fn hold(&mut self) {
        self.hold += 1;
    }

    /// Ends a [`hold`](History::hold); the last one commits the group.
    pub fn release(&mut self) {
        self.hold = self.hold.saturating_sub(1);
        self.commit();
    }

    pub fn commit(&mut self) {
        if self.hold == 0 {
            self.force_commit();
        }
    }

    fn force_commit(&mut self) {
        if let Some(g) = self.open.take()
            && !g.revs.is_empty()
        {
            self.undo.push(g);
            // keep memory bounded on very long sessions
            if self.undo.len() > 10_000 {
                self.undo.drain(..1_000);
            }
        }
    }

    pub fn has_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn undo(&mut self) -> Option<Step> {
        self.force_commit();
        let g = self.undo.pop()?;
        let changes = g.revs.iter().rev().map(|r| r.inverse.clone()).collect();
        let selection = g.sel_before.clone();
        let before = g.sel_before.clone();
        self.redo.push(g);
        Some(Step { changes, selection, before })
    }

    pub fn redo(&mut self) -> Option<Step> {
        self.force_commit();
        let g = self.redo.pop()?;
        let changes = g.revs.iter().map(|r| r.forward.clone()).collect();
        let selection = g.sel_after.clone();
        let before = g.sel_before.clone();
        self.undo.push(g);
        Some(Step { changes, selection, before })
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.open.is_some()
    }
}
