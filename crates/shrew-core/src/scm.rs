//! Source control that needs no repository: merge conflict markers in a buffer. Finding them, jumping between
//! them (`]x` / `[x`, `git_next_conflict`) and resolving one (keep the current side, the incoming side or both) are
//! plain text edits, one undo step each. The app's git module draws the regions and their actions and hands
//! everything else (`Editor::git`) to the git backend.

use crate::commands::Command;
use crate::editor::{Editor, Event, Mode};
use crate::features::Feature;
use crate::selection::Selection;
use crate::text;
use crate::{Change, ChangeSet};
use ropey::{Rope, RopeSlice};
use std::ops::Range;

/// Conflict markers are this long (git's default `conflict-marker-size`).
const MARKER: usize = 7;

/// One conflict: the lines of its markers. `<<<<<<<` opens it, `|||||||` starts the common ancestor (diff3 and
/// zdiff3 styles only), `=======` separates the sides and `>>>>>>>` closes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub start: usize,
    pub base: Option<usize>,
    pub sep: usize,
    pub end: usize,
}

impl Conflict {
    /// The current side's lines (HEAD, "ours").
    pub fn ours(&self) -> Range<usize> {
        self.start + 1..self.base.unwrap_or(self.sep)
    }

    /// The common ancestor's lines, when the markers have them.
    pub fn base_lines(&self) -> Option<Range<usize>> {
        self.base.map(|b| b + 1..self.sep)
    }

    /// The incoming side's lines ("theirs").
    pub fn theirs(&self) -> Range<usize> {
        self.sep + 1..self.end
    }

    pub fn contains(&self, line: usize) -> bool {
        (self.start..=self.end).contains(&line)
    }
}

/// What to keep of a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    Ours,
    Theirs,
    /// Current, then incoming.
    Both,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    Start,
    Base,
    Sep,
    End,
}

/// The marker a line starts with: seven of `<|=>` followed by a space or the line's end.
fn marker(line: RopeSlice) -> Option<Marker> {
    let mut chars = line.chars();
    let first = chars.next()?;
    let kind = match first {
        '<' => Marker::Start,
        '|' => Marker::Base,
        '=' => Marker::Sep,
        '>' => Marker::End,
        _ => return None,
    };
    for _ in 1..MARKER {
        if chars.next()? != first {
            return None;
        }
    }
    match chars.next() {
        None | Some('\n' | '\r') => Some(kind),
        Some(' ') if kind != Marker::Sep => Some(kind),
        _ => None,
    }
}

/// Every well-formed conflict in the text, in order. Stray or unbalanced markers are skipped.
pub fn conflicts(rope: &Rope) -> Vec<Conflict> {
    let mut out = Vec::new();
    let (mut start, mut base, mut sep): (Option<usize>, Option<usize>, Option<usize>) = (None, None, None);
    for (i, line) in rope.lines().enumerate() {
        let Some(m) = marker(line) else { continue };
        match m {
            Marker::Start => (start, base, sep) = (Some(i), None, None),
            Marker::Base if start.is_some() && sep.is_none() && base.is_none() => base = Some(i),
            Marker::Sep if start.is_some() && sep.is_none() => sep = Some(i),
            Marker::End => {
                if let (Some(s), Some(m)) = (start, sep) {
                    out.push(Conflict { start: s, base, sep: m, end: i });
                }
                (start, base, sep) = (None, None, None);
            }
            _ => {}
        }
    }
    out
}

/// The text a conflict becomes when `keep` is kept (whole lines, with their line breaks).
pub fn resolution(rope: &Rope, c: &Conflict, keep: Keep) -> String {
    let lines = |r: Range<usize>| -> String {
        if r.is_empty() {
            return String::new();
        }
        rope.slice(text::line_start(rope, r.start)..text::line_start(rope, r.end)).to_string()
    };
    match keep {
        Keep::Ours => lines(c.ours()),
        Keep::Theirs => lines(c.theirs()),
        Keep::Both => lines(c.ours()) + &lines(c.theirs()),
    }
}

impl Editor {
    /// Hands a git command to the app (its `git` module), unless the git panel is off or not built.
    pub fn git(&mut self, cmd: Command) {
        if self.git_panel_gate() {
            self.events.push(Event::App(cmd));
        }
    }

    fn git_panel_gate(&mut self) -> bool {
        if !self.features.available(Feature::GitPanel) {
            self.error("git is not compiled into this build");
            return false;
        }
        if !self.features.on(Feature::GitPanel) {
            self.error("the git panel is off (:set git_panel)");
            return false;
        }
        true
    }

    pub(crate) fn conflict_command(&mut self, cmd: Command) {
        if !self.git_panel_gate() {
            return;
        }
        match cmd {
            Command::GitNextConflict => self.conflict_jump(true),
            Command::GitPrevConflict => self.conflict_jump(false),
            Command::GitConflictOurs => {
                self.resolve_conflict(None, Keep::Ours);
            }
            Command::GitConflictTheirs => {
                self.resolve_conflict(None, Keep::Theirs);
            }
            Command::GitConflictBoth => {
                self.resolve_conflict(None, Keep::Both);
            }
            _ => {}
        }
    }

    /// Moves the cursor to the next (or previous) conflict's `<<<<<<<` line.
    pub fn conflict_jump(&mut self, forward: bool) {
        let rope = self.buf().rope();
        let line = text::line_of(rope, self.buf().sel.primary().head);
        let all = conflicts(rope);
        let to = if forward {
            all.iter().find(|c| c.start > line)
        } else {
            all.iter().rev().find(|c| c.start < line && !c.contains(line)).or_else(|| {
                // inside one: its own start first
                all.iter().find(|c| c.contains(line) && c.start < line)
            })
        };
        match to {
            Some(c) => {
                let n = all.iter().position(|x| x == c).unwrap_or(0) + 1;
                let total = all.len();
                self.push_jump();
                self.goto_line(c.start + 1);
                self.info(format!("conflict {n} of {total}"));
            }
            None if all.is_empty() => self.info("no merge conflicts in this file"),
            None => self.info(if forward { "no more conflicts below" } else { "no more conflicts above" }),
        }
    }

    /// Resolves the conflict on `line` (the cursor's when `None`; else the next one below it) as one undo step.
    /// False when there was none.
    pub fn resolve_conflict(&mut self, line: Option<usize>, keep: Keep) -> bool {
        let rope = self.buf().rope();
        let line = line.unwrap_or_else(|| text::line_of(rope, self.buf().sel.primary().head));
        let all = conflicts(rope);
        let Some(c) = all.iter().find(|c| c.contains(line)).or_else(|| all.iter().find(|c| c.start > line)).cloned()
        else {
            self.error("no merge conflict here");
            return false;
        };
        let mut text = resolution(rope, &c, keep);
        let from = text::line_start(rope, c.start);
        let to = if c.end < text::last_line(rope) { text::line_start(rope, c.end + 1) } else { rope.len_chars() };
        // the closing marker ended the file without a line break: so does the result
        if to == rope.len_chars() && to > from && rope.char(to - 1) != '\n' && text.ends_with('\n') {
            text.pop();
        }
        if self.mode.is_visual() {
            self.set_mode(Mode::Normal);
        }
        let b = self.buf_mut();
        b.apply(ChangeSet::single(Change::replace(from, to, text)), None);
        let rope = b.rope();
        let at = text::first_non_blank(rope, c.start.min(text::last_line(rope)));
        b.sel = Selection::point(at);
        self.commit();
        let left = conflicts(self.buf().rope()).len();
        self.info(match left {
            0 => "no conflicts left: save, then mark the file resolved (git_mark_resolved)".to_string(),
            1 => "1 conflict left".to_string(),
            n => format!("{n} conflicts left"),
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, KeyPress};

    const TEXT: &str = "a\n<<<<<<< HEAD\nours 1\nours 2\n=======\ntheirs\n>>>>>>> topic\nb\n";
    const DIFF3: &str = "<<<<<<< HEAD\nx\n||||||| base\nold\n=======\ny\n>>>>>>> other\n";

    fn ed(text: &str) -> Editor {
        let mut e = Editor::new();
        e.install_document(Document::from_str(text));
        e
    }

    #[test]
    fn finds_conflicts_with_and_without_a_base() {
        let r = Rope::from_str(TEXT);
        let c = conflicts(&r);
        assert_eq!(c, vec![Conflict { start: 1, base: None, sep: 4, end: 6 }]);
        assert_eq!((c[0].ours(), c[0].theirs(), c[0].base_lines()), (2..4, 5..6, None));
        let c = conflicts(&Rope::from_str(DIFF3));
        assert_eq!(c, vec![Conflict { start: 0, base: Some(2), sep: 4, end: 6 }]);
        assert_eq!((c[0].ours(), c[0].base_lines(), c[0].theirs()), (1..2, Some(3..4), 5..6));
    }

    #[test]
    fn stray_and_lookalike_markers_are_not_conflicts() {
        // a markdown rule, a shorter marker, an unclosed conflict, a separator with text after it
        let r = Rope::from_str("=======\n<<<<<< x\n<<<<<<< a\n======= no\nb\n>>>>>>>\n<<<<<<< c\nd\n");
        assert!(conflicts(&r).is_empty());
        let r = Rope::from_str("<<<<<<<\n=======\r\n>>>>>>>");
        assert_eq!(conflicts(&r).len(), 1, "bare markers, CRLF and no final newline");
    }

    #[test]
    fn resolutions_keep_whole_lines() {
        let r = Rope::from_str(TEXT);
        let c = &conflicts(&r)[0];
        assert_eq!(resolution(&r, c, Keep::Ours), "ours 1\nours 2\n");
        assert_eq!(resolution(&r, c, Keep::Theirs), "theirs\n");
        assert_eq!(resolution(&r, c, Keep::Both), "ours 1\nours 2\ntheirs\n");
        let r = Rope::from_str(DIFF3);
        assert_eq!(resolution(&r, &conflicts(&r)[0], Keep::Ours), "x\n", "the base is dropped");
    }

    #[test]
    fn resolving_is_one_undo_step() {
        let mut e = ed(TEXT);
        e.features.set(Feature::GitPanel, true);
        e.goto_line(4);
        e.run(Command::GitConflictTheirs);
        assert_eq!(e.buf().rope().to_string(), "a\ntheirs\nb\n");
        assert_eq!(text::line_of(e.buf().rope(), e.buf().sel.primary().head), 1);
        e.handle_key(KeyPress::ch('u'));
        assert_eq!(e.buf().rope().to_string(), TEXT);
        // from above the conflict: the next one
        e.goto_line(1);
        assert!(e.resolve_conflict(None, Keep::Both));
        assert_eq!(e.buf().rope().to_string(), "a\nours 1\nours 2\ntheirs\nb\n");
        assert!(!e.resolve_conflict(None, Keep::Ours), "none left");
    }

    #[test]
    fn a_conflict_at_the_end_of_the_file_keeps_its_ending() {
        let mut e = ed("x\n<<<<<<< HEAD\na\n=======\nb\n>>>>>>> t");
        assert!(e.resolve_conflict(Some(1), Keep::Ours));
        assert_eq!(e.buf().rope().to_string(), "x\na");
    }

    #[test]
    fn jumping_between_conflicts() {
        let two = format!("{TEXT}{TEXT}");
        let mut e = ed(&two);
        let line = |e: &Editor| text::line_of(e.buf().rope(), e.buf().sel.primary().head);
        e.conflict_jump(true);
        assert_eq!(line(&e), 1);
        e.conflict_jump(true);
        assert_eq!(line(&e), 9);
        e.conflict_jump(true);
        assert_eq!(line(&e), 9, "no more below");
        e.conflict_jump(false);
        assert_eq!(line(&e), 1);
        e.goto_line(4); // inside the first: back to its start
        e.conflict_jump(false);
        assert_eq!(line(&e), 1);
    }

    #[test]
    fn git_commands_go_to_the_app_and_respect_the_feature() {
        let mut e = Editor::new();
        e.run(Command::GitLog);
        assert_eq!(e.events.last(), Some(&Event::App(Command::GitLog)));
        e.events.clear();
        e.features.set(Feature::GitPanel, false);
        e.run(Command::GitLog);
        assert!(e.events.is_empty());
        assert!(e.message.as_ref().is_some_and(|m| m.1));
        e.features.set(Feature::GitPanel, true);
        e.features.set_unavailable(Feature::GitPanel);
        e.run(Command::GitConflictOurs);
        assert!(e.message.as_ref().is_some_and(|m| m.0.contains("not compiled")));
    }
}
