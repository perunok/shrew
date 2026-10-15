//! Git change markers as the editor sees them: the line hunks of each buffer against its git base (the app's git
//! backend computes them and stores them in [`Editor::git_hunks`]), and moving between them: vim's `]c` / `[c`
//! (and `]h` / `[h`, counts work, a jump), Ctrl+. / Ctrl+Shift+. and the palette. The hunk commands that need git
//! (preview, stage, unstage, revert) go to the app as [`Event::Git`](crate::Event::Git). No git code lives here.

use crate::commands::Command;
use crate::editor::{Editor, Event, Mode};
use crate::features::{self, Feature};
use crate::selection::{Range, Selection};
use crate::text;

/// One change of a buffer against its base: base lines `old` became buffer lines `new` (0-based, end exclusive).
/// A pure deletion has an empty `new` range anchored before buffer line `new.start` (which can be one past the
/// last line when the end was deleted); a pure addition has an empty `old` range.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Hunk {
    pub old: std::ops::Range<u32>,
    pub new: std::ops::Range<u32>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum HunkKind {
    Added,
    Modified,
    Deleted,
}

impl Hunk {
    pub fn kind(&self) -> HunkKind {
        match (self.old.is_empty(), self.new.is_empty()) {
            (true, _) => HunkKind::Added,
            (false, true) => HunkKind::Deleted,
            (false, false) => HunkKind::Modified,
        }
    }

    /// The buffer line the hunk is shown on and jumped to: its first line, or for a deletion the line above the
    /// gap (the first line when the file's start was deleted), as gitsigns does. `last`: the buffer's last line.
    pub fn line(&self, last: usize) -> usize {
        let l = if self.new.is_empty() { self.new.start.saturating_sub(1) } else { self.new.start };
        (l as usize).min(last)
    }

    /// The buffer lines the hunk covers; a deletion covers its marker line.
    pub fn lines(&self, last: usize) -> std::ops::Range<usize> {
        if self.new.is_empty() {
            let l = self.line(last);
            l..l + 1
        } else {
            (self.new.start as usize).min(last)..(self.new.end as usize).min(last + 1)
        }
    }
}

/// The hunk on `line`, as an index into `hunks` (sorted, as diffs come).
pub fn at_line(hunks: &[Hunk], line: usize, last: usize) -> Option<usize> {
    // hunks are sorted by their new range; a deletion's marker line can share a line with the hunk after it
    let i = hunks.partition_point(|h| h.lines(last).end <= line);
    (i..hunks.len()).take_while(|&i| hunks[i].lines(last).start <= line).find(|&i| hunks[i].lines(last).contains(&line))
}

/// The line `count` hunks forward (or back) from `line`, wrapping around the file like vim's `wrapscan`; the flag
/// says it wrapped. Forward goes to the first hunk starting below `line`, back to the last one starting above it
/// (so `[c` inside a hunk goes to its start). `None` when there are no hunks.
pub fn step(hunks: &[Hunk], line: usize, count: usize, forward: bool, last: usize) -> Option<(usize, bool)> {
    if hunks.is_empty() {
        return None;
    }
    let mut starts: Vec<usize> = hunks.iter().map(|h| h.line(last)).collect();
    starts.dedup();
    let mut at = line;
    let mut wrapped = false;
    for _ in 0..count.max(1) {
        at = if forward {
            match starts.iter().find(|&&s| s > at) {
                Some(&s) => s,
                None => {
                    wrapped = true;
                    starts[0]
                }
            }
        } else {
            match starts.iter().rev().find(|&&s| s < at) {
                Some(&s) => s,
                None => {
                    wrapped = true;
                    *starts.last().expect("not empty")
                }
            }
        };
    }
    Some((at, wrapped))
}

impl Editor {
    /// The active buffer's hunks (empty when it has none, or git markers are off).
    pub fn hunks(&self) -> &[Hunk] {
        if !self.features.on(Feature::GitGutter) {
            return &[];
        }
        self.git_hunks.get(&self.buf().id).map_or(&[], |h| h.as_slice())
    }

    /// The target line of a hunk motion from `line`, saying so when it wraps or there is nothing to go to.
    pub(crate) fn hunk_target(&mut self, line: usize, count: usize, forward: bool) -> Option<usize> {
        let last = text::last_line(&self.buf().doc.rope);
        let Some((to, wrapped)) = step(self.hunks(), line, count, forward, last) else {
            self.info(if self.features.on(Feature::GitGutter) { "no changes" } else { "git markers are off" });
            return None;
        };
        if wrapped {
            self.info(if forward {
                "past the last change: back at the first"
            } else {
                "before the first change: at the last"
            });
        }
        Some(to)
    }

    /// Ctrl+. / Ctrl+Shift+. and the palette: to the next or previous hunk. In vim's normal and visual modes this
    /// is the `]c` / `[c` motion (visual mode extends the selection).
    pub fn goto_hunk(&mut self, forward: bool, count: usize) {
        if self.vim() && self.mode != Mode::Insert {
            return self.do_move(crate::vim::Motion::Hunk(forward), Some(count));
        }
        let line = self.buf().cursor_line();
        let Some(to) = self.hunk_target(line, count, forward) else { return };
        self.push_jump();
        let rope = &self.buf().doc.rope;
        let p = text::first_non_blank(rope, to);
        let b = self.buf_mut();
        b.sel = Selection::single(Range::point(p));
        let rows = b.view.rows;
        if to < b.view.top || to >= b.view.top + rows {
            b.center_on_cursor();
        }
    }

    /// The lines a hunk command acts on: the selected lines (visual mode, or a selection with vim off), else the
    /// cursor line. The flag says they were selected.
    pub fn git_target_lines(&self) -> (std::ops::Range<usize>, bool) {
        let b = self.buf();
        let rope = &b.doc.rope;
        let r = b.sel.primary();
        let selected = self.mode.is_visual() || ((self.mode == Mode::Insert || !self.vim()) && !r.is_empty());
        if !selected {
            let l = b.cursor_line();
            return (l..l + 1, false);
        }
        let (a, z) = (r.from(), r.to());
        let la = text::line_of(rope, a);
        let mut lz = text::line_of(rope, z);
        // a caret selection ending at a line start does not take that line (vim's visual selections are inclusive)
        if !self.mode.is_visual() && lz > la && z == text::line_start(rope, lz) {
            lz -= 1;
        }
        (la..lz + 1, true)
    }

    /// A hunk command the app carries out, unless git markers are off or not compiled in.
    pub(crate) fn git_command(&mut self, cmd: Command) {
        let f = Feature::GitGutter;
        if !self.features.available(f) {
            return self.error("git is not compiled into this build");
        }
        if !self.features.on(f) && cmd != Command::GitGutterBase {
            return self.error(format!("git markers are off (:set {})", features::info(f).key));
        }
        self.events.push(Event::Git(cmd));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyPress;

    fn h(old: std::ops::Range<u32>, new: std::ops::Range<u32>) -> Hunk {
        Hunk { old, new }
    }

    #[test]
    fn kinds_and_lines() {
        assert_eq!(h(2..2, 2..4).kind(), HunkKind::Added);
        assert_eq!(h(2..3, 2..4).kind(), HunkKind::Modified);
        let del = h(2..5, 7..7);
        assert_eq!(del.kind(), HunkKind::Deleted);
        assert_eq!(del.line(100), 6, "on the line above the gap");
        assert_eq!(del.lines(100), 6..7);
        assert_eq!(h(0..2, 0..0).line(100), 0, "the file's start deleted: the first line");
        assert_eq!(h(8..10, 9..9).line(8), 8, "the end deleted: the last line");
        assert_eq!(h(2..2, 2..4).lines(100), 2..4);
    }

    #[test]
    fn hunk_at_a_line() {
        let hunks = [h(0..0, 0..2), h(5..6, 6..7), h(8..10, 9..9), h(10..10, 9..12)];
        assert_eq!(at_line(&hunks, 0, 50), Some(0));
        assert_eq!(at_line(&hunks, 1, 50), Some(0));
        assert_eq!(at_line(&hunks, 2, 50), None);
        assert_eq!(at_line(&hunks, 6, 50), Some(1));
        assert_eq!(at_line(&hunks, 8, 50), Some(2), "a deletion's marker line");
        assert_eq!(at_line(&hunks, 9, 50), Some(3));
        assert_eq!(at_line(&hunks, 30, 50), None);
        assert_eq!(at_line(&[], 3, 50), None);
    }

    #[test]
    fn stepping_counts_and_wraps() {
        let hunks = [h(0..0, 2..4), h(5..6, 10..11), h(8..10, 20..20)];
        let last = 40;
        assert_eq!(step(&hunks, 0, 1, true, last), Some((2, false)));
        assert_eq!(step(&hunks, 2, 1, true, last), Some((10, false)));
        assert_eq!(step(&hunks, 3, 2, true, last), Some((19, false)), "a count; the deletion is on line 19");
        assert_eq!(step(&hunks, 19, 1, true, last), Some((2, true)), "wraps to the top");
        assert_eq!(step(&hunks, 11, 1, false, last), Some((10, false)));
        assert_eq!(step(&hunks, 10, 1, false, last), Some((2, false)));
        assert_eq!(step(&hunks, 3, 1, false, last), Some((2, false)), "inside a hunk: back to its start");
        assert_eq!(step(&hunks, 2, 1, false, last), Some((19, true)), "wraps to the bottom");
        assert_eq!(step(&hunks, 5, 5, true, last), Some((19, true)));
        assert_eq!(step(&[], 5, 1, true, last), None);
    }

    fn editor(lines: usize) -> Editor {
        let mut ed = Editor::new();
        let text: String = (0..lines).map(|i| format!("  line {i}\n")).collect();
        ed.buffers[0].doc = crate::Document::from_str(&text);
        let id = ed.buf().id;
        ed.git_hunks.insert(id, vec![h(3..3, 3..5), h(7..8, 9..10), h(12..14, 15..15)]);
        ed
    }

    fn keys(ed: &mut Editor, s: &str) {
        for k in KeyPress::parse_seq(s) {
            ed.handle_key(k);
        }
    }

    #[test]
    fn vim_hunk_motions() {
        let mut ed = editor(30);
        keys(&mut ed, "]c");
        assert_eq!(ed.buf().cursor_line(), 3);
        assert_eq!(ed.buf().sel.primary().head, text::first_non_blank(ed.buf().rope(), 3));
        keys(&mut ed, "2]h");
        assert_eq!(ed.buf().cursor_line(), 14);
        keys(&mut ed, "[c");
        assert_eq!(ed.buf().cursor_line(), 9);
        // a jump: Ctrl+O comes back
        keys(&mut ed, "<C-o>");
        assert_eq!(ed.buf().cursor_line(), 14);
        // an operator takes whole lines up to the hunk
        keys(&mut ed, "gg");
        keys(&mut ed, "d]c");
        assert_eq!(ed.buf().rope().line(0).to_string(), "  line 4\n");
        // wraps, and says so
        let mut ed = editor(30);
        keys(&mut ed, "Gk]c");
        assert_eq!(ed.buf().cursor_line(), 3);
        assert!(ed.message.as_ref().is_some_and(|m| m.0.contains("first")));
        // visual mode extends
        let mut ed = editor(30);
        keys(&mut ed, "V]c");
        assert_eq!(ed.mode, Mode::VisualLine);
        assert_eq!(ed.git_target_lines(), (0..4, true));
    }

    #[test]
    fn chords_and_markers_off() {
        let mut ed = editor(30);
        ed.set_feature(Feature::Vim, false);
        keys(&mut ed, "<C-.>");
        assert_eq!(ed.buf().cursor_line(), 3);
        keys(&mut ed, "<C-.><C-.>");
        assert_eq!(ed.buf().cursor_line(), 14);
        keys(&mut ed, "<C-S-.>");
        assert_eq!(ed.buf().cursor_line(), 9);
        ed.set_feature(Feature::GitGutter, false);
        keys(&mut ed, "<C-.>");
        assert_eq!(ed.buf().cursor_line(), 9, "no markers, no hunks");
    }

    #[test]
    fn hunk_commands_go_to_the_app() {
        let mut ed = editor(30);
        ed.events.clear();
        keys(&mut ed, "ghs");
        assert_eq!(ed.events, vec![Event::Git(Command::GitStageHunk)]);
        ed.events.clear();
        keys(&mut ed, "Vjghr");
        assert_eq!(ed.events, vec![Event::Git(Command::GitRevertHunk)]);
        assert_eq!(ed.git_target_lines(), (0..2, true), "the selection is still there for the app");
        keys(&mut ed, "<Esc>");
        ed.events.clear();
        keys(&mut ed, "ghp:git_unstage_hunk<CR>");
        assert_eq!(ed.events, vec![Event::Git(Command::GitPreviewHunk), Event::Git(Command::GitUnstageHunk)]);
        assert_eq!(ed.git_target_lines(), (1..2, false));
    }
}
