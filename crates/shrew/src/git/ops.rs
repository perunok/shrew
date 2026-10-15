//! Writes in flight: a toast at the bottom right while a network op (fetch, pull, push) runs, with git's latest
//! progress line and a cancel button (or `git_cancel`, or Esc in the panel); and git's whole message when an op
//! fails, until Esc or its close button. `git_output` opens the last op's output in a buffer.

use crate::colors::{Colors, mix};
use crate::layout::Layout;
use crate::state::State;
use shrew_core::Document;
use shrew_git::{Op, RepoState, Ticket};
use shrew_render::{Rect, Scene, TextStyle, columns};

/// Error lines a toast shows at most (`git_output` has them all).
const TOAST_LINES: usize = 12;

pub struct Running {
    pub ticket: Ticket,
    pub label: &'static str,
    pub progress: String,
}

/// What the toast says after a failure.
pub struct Failure {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Default, Debug)]
pub struct Hits {
    pub area: Option<Rect>,
    pub close: Option<Rect>,
}

#[derive(Default)]
pub struct Ops {
    /// The network op the toast follows.
    pub running: Option<Running>,
    /// Write tickets not answered yet.
    pending: Vec<Ticket>,
    pub failure: Option<Failure>,
    /// The last fetch, pull or push summary, or the last error.
    pub last_output: String,
    pub hits: Hits,
}

impl Ops {
    pub fn started(&mut self, ticket: Ticket, label: &'static str, network: bool) {
        self.pending.push(ticket);
        if network {
            self.failure = None;
            self.running = Some(Running { ticket, label, progress: String::new() });
        }
    }

    pub fn progress(&mut self, ticket: Ticket, line: String) {
        if let Some(r) = &mut self.running
            && r.ticket == ticket
        {
            r.progress = line;
        }
    }

    pub fn finished(&mut self, ticket: Ticket) {
        self.pending.retain(|t| *t != ticket);
        if self.running.as_ref().is_some_and(|r| r.ticket == ticket) {
            self.running = None;
        }
    }

    pub fn failed(&mut self, op: &Op, message: &str) {
        self.last_output = message.to_string();
        let lines: Vec<String> = message.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
        self.failure = Some(Failure { title: format!("git {} failed", verb(op)), lines });
    }

    /// A write is waiting for git (the panel greys its buttons).
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn toast_error(&self) -> bool {
        self.failure.is_some() && self.running.is_none()
    }

    pub fn close_toast(&mut self) {
        self.failure = None;
    }
}

/// "Pushing", for the toast.
pub fn label(op: &Op) -> &'static str {
    match op {
        Op::Fetch(_) => "Fetching",
        Op::Pull(_) => "Pulling",
        Op::Push(_) => "Pushing",
        Op::Commit(_) => "Committing",
        _ => "Working",
    }
}

/// "push", for messages.
pub fn verb(op: &Op) -> &'static str {
    match op {
        Op::Stage(_) | Op::StageAll | Op::StageLines { .. } => "add",
        Op::Unstage(_) | Op::UnstageAll | Op::UnstageLines { .. } => "restore --staged",
        Op::Discard(_) | Op::DiscardLines { .. } => "restore",
        Op::Clean(_) => "clean",
        Op::Commit(_) => "commit",
        Op::Switch(_) | Op::SwitchDetach(_) => "switch",
        Op::CreateBranch { .. } | Op::DeleteBranch { .. } | Op::RenameBranch { .. } => "branch",
        Op::CreateTag { .. } | Op::DeleteTag(_) => "tag",
        Op::StashPush(_) | Op::StashPop(_) | Op::StashApply(_) | Op::StashDrop(_) => "stash",
        Op::TakeSide { .. } => "checkout",
        Op::MarkResolved(_) => "add",
        Op::AbortOperation => "abort",
        Op::ContinueOperation => "continue",
        Op::Fetch(_) => "fetch",
        Op::Pull(_) => "pull",
        Op::Push(_) => "push",
    }
}

/// The message line after an op that printed nothing worth showing.
pub fn done_message(op: &Op) -> String {
    let files = |p: &[String]| match p {
        [one] => one.clone(),
        many => format!("{} files", many.len()),
    };
    match op {
        Op::Stage(p) => format!("staged {}", files(p)),
        Op::StageAll => "staged everything".into(),
        Op::Unstage(p) => format!("unstaged {}", files(p)),
        Op::UnstageAll => "unstaged everything".into(),
        Op::Discard(p) => format!("discarded the changes to {}", files(p)),
        Op::Clean(p) => format!("deleted {}", files(p)),
        Op::Switch(b) => format!("switched to {b}"),
        Op::SwitchDetach(r) => format!("checked out {r} (detached)"),
        Op::CreateBranch { name, switch: true, .. } => format!("switched to a new branch {name}"),
        Op::CreateBranch { name, .. } => format!("created branch {name}"),
        Op::DeleteBranch { name, .. } => format!("deleted branch {name}"),
        Op::RenameBranch { new, .. } => format!("renamed the branch to {new}"),
        Op::StashPush(_) => "stashed the changes".into(),
        Op::StashPop(i) => format!("popped stash@{{{i}}}"),
        Op::StashApply(i) => format!("applied stash@{{{i}}}"),
        Op::StashDrop(i) => format!("dropped stash@{{{i}}}"),
        Op::MarkResolved(p) => format!("marked {} resolved", files(p)),
        Op::AbortOperation => "aborted".into(),
        Op::ContinueOperation => "continued".into(),
        _ => String::new(),
    }
}

/// git's summary of a fetch, pull or push, in one line.
pub fn summary(op: &Op, text: &str) -> String {
    let last = text.lines().rev().map(str::trim).find(|l| !l.is_empty() && *l != "Done");
    match (op, last) {
        (Op::Push(_), _) if text.contains("[up to date]") || text.contains("Everything up-to-date") => {
            "push: everything up to date".into()
        }
        (Op::Pull(_), Some(l)) if l.contains("Already up to date") => "pull: already up to date".into(),
        (_, Some(l)) => format!("{}: {l}", verb(op)),
        (_, None) => format!("{}: done", verb(op)),
    }
}

/// "merge", "rebase": what the repository is in the middle of.
pub fn state_noun(s: RepoState) -> &'static str {
    match s {
        RepoState::Merging => "merge",
        RepoState::Rebasing => "rebase",
        RepoState::CherryPicking => "cherry-pick",
        RepoState::Reverting => "revert",
        RepoState::Bisecting => "bisect",
        RepoState::ApplyingPatches => "am",
        RepoState::Clean => "operation",
    }
}

/// `git_output`: the last op's output in a scratch buffer.
pub fn show_output(st: &mut State) {
    let out = &st.git.ops.last_output;
    if out.is_empty() {
        return st.ed.info("no git output yet");
    }
    let mut doc = Document::from_str(out);
    doc.title = Some("git output".into());
    st.ed.install_document(doc);
    st.process_events();
}

pub fn draw(scene: &mut Scene, st: &mut State, lay: &Layout, c: &Colors) {
    let ops = &mut st.git.ops;
    let (title, lines, error): (String, Vec<String>, bool) = match (&ops.running, &ops.failure) {
        (Some(r), _) => {
            let p = if r.progress.is_empty() { "starting git…".to_string() } else { r.progress.clone() };
            (format!("{}…", r.label), vec![p], false)
        }
        (None, Some(f)) => (f.title.clone(), f.lines.clone(), true),
        _ => return,
    };
    let (cw, ch, pad) = (lay.cw, lay.ch, lay.pad);
    let longest = lines.iter().map(|l| columns(l)).chain([columns(&title) + 4]).max().unwrap_or(10);
    let w = ((longest + 4) as f32 * cw).clamp(36.0 * cw, (lay.width * 0.5).max(36.0 * cw)).min(lay.width - 4.0 * cw);
    // long lines wrap; past the limit `git_output` has the rest
    let cols = ((w / cw) as usize).saturating_sub(4).max(10);
    let mut lines: Vec<String> = lines.iter().flat_map(|l| wrap(l, cols)).collect();
    if lines.len() > TOAST_LINES {
        lines.truncate(TOAST_LINES - 1);
        lines.push("… (git_output shows all of it)".into());
    }
    let h = (lines.len() + 1) as f32 * ch + 4.0 * pad;
    let x = (lay.width - w - 2.0 * cw).round();
    let y = (lay.status.y - h - 2.0 * pad).round();
    let panel = Rect::new(x, y, w.round(), h.round());
    scene.layer(2);
    crate::draw::popup(scene, panel, lay, c);
    if error {
        let bar = (3.0 * lay.scale).round();
        scene.rounded_rect(Rect::new(x, y, bar, panel.h), bar / 2.0, c.error, None);
    }
    scene.push_clip(panel);
    let tx = x + 2.0 * cw;
    let ty = y + 2.0 * pad;
    let tc = if error { c.error } else { c.accent };
    scene.text(tx, ty, &title, TextStyle::new(tc).bold(), &[]);
    // the close (cancel) button
    let label = if error { "✕" } else { "cancel ✕" };
    let bw = (columns(label) + 2) as f32 * cw;
    let close = Rect::new((panel.right() - bw - cw).round(), ty, bw.round(), ch);
    scene.rounded_rect(close, (4.0 * lay.scale).round(), mix(c.panel_selection, c.popup, 0.6), None);
    scene.text(close.x + cw, ty, label, TextStyle::new(c.panel_muted), &[]);
    let fg = if error { c.panel_foreground } else { c.panel_muted };
    for (i, l) in lines.iter().enumerate() {
        scene.text(tx, ty + (i + 1) as f32 * ch, l, TextStyle::new(fg), &[]);
    }
    scene.pop_clip();
    scene.layer(0);
    ops.hits = Hits { area: Some(panel), close: Some(close) };
}

/// `s` cut into pieces of at most `cols` columns.
fn wrap(s: &str, cols: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut w = 0;
    for ch in s.chars() {
        let cw = shrew_render::char_columns(ch);
        if w + cw > cols {
            out.push(String::new());
            w = 0;
        }
        out.last_mut().expect("one").push(ch);
        w += cw;
    }
    out
}

/// A click on the toast: its button cancels the op or closes the message. True when it was on the toast.
pub fn press(st: &mut State, x: f32, y: f32) -> bool {
    let h = &st.git.ops.hits;
    if !h.area.is_some_and(|a| a.contains(x, y)) {
        return false;
    }
    if h.close.is_some_and(|r| r.contains(x, y)) {
        if st.git.ops.running.is_some() {
            super::command(st, shrew_core::Command::GitCancel, None);
        } else {
            st.git.ops.close_toast();
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_are_one_line() {
        let push = Op::Push(Default::default());
        assert_eq!(
            summary(&push, "To x\n=\trefs/heads/a:refs/heads/a\t[up to date]\nDone\n"),
            "push: everything up to date"
        );
        let pull = Op::Pull(Default::default());
        assert_eq!(summary(&pull, "Already up to date.\n"), "pull: already up to date");
        assert_eq!(summary(&Op::Fetch(Default::default()), ""), "fetch: done");
        assert_eq!(done_message(&Op::Stage(vec!["a.rs".into()])), "staged a.rs");
        assert_eq!(done_message(&Op::Stage(vec!["a".into(), "b".into()])), "staged 2 files");
    }

    #[test]
    fn the_toast_follows_network_ops_only() {
        let mut o = Ops::default();
        o.started(Ticket(1), "Staging", false);
        assert!(o.running.is_none() && o.busy());
        o.started(Ticket(2), "Pushing", true);
        o.progress(Ticket(2), "Writing objects: 50%".into());
        assert_eq!(o.running.as_ref().unwrap().progress, "Writing objects: 50%");
        o.finished(Ticket(2));
        o.finished(Ticket(1));
        assert!(o.running.is_none() && !o.busy());
        o.failed(&Op::Push(Default::default()), "rejected\n\nhint: pull first\n");
        assert!(o.toast_error());
        assert_eq!(o.failure.as_ref().unwrap().lines, vec!["rejected", "hint: pull first"]);
    }
}
