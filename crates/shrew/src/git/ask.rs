//! The git prompt: a question on the command line before something destructive ("Discard changes to a.rs? y/n"),
//! or a line of text (a branch name, a stash message). It takes every key while open; Esc cancels.

use super::textbox::{Edit, TextBox};
use crate::colors::Colors;
use crate::layout::Layout;
use shrew_core::{Key, KeyPress};
use shrew_git::Op;
use shrew_render::{Rect, Scene, TextStyle, columns};

/// What happens with the answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// Run this op on `y`.
    Op(Op),
    /// Delete this branch on `y`; `force` once git said it is not merged.
    DeleteBranch {
        name: String,
        force: bool,
    },
    DropStash(usize),
    /// The text names a new branch, made at `start` (HEAD when `None`) and checked out.
    NewBranch {
        start: Option<String>,
    },
    /// The text is a stash message; Ctrl+Enter takes untracked files too.
    StashPush,
    /// The log's author filter.
    LogAuthor(u64),
    /// The log's path filter (file history).
    LogPath(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub label: String,
    /// `None`: a y/n question.
    pub input: Option<TextBox>,
    pub then: Then,
}

/// What a key did.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Waiting,
    Cancel,
    /// Yes, or Enter with the text; `alt`: Ctrl+Enter.
    Yes {
        text: String,
        alt: bool,
    },
}

impl Ask {
    pub fn confirm(label: impl Into<String>, then: Then) -> Ask {
        Ask { label: label.into(), input: None, then }
    }

    pub fn text(label: impl Into<String>, initial: &str, then: Then) -> Ask {
        Ask { label: label.into(), input: Some(TextBox::new(initial)), then }
    }

    pub fn key(&mut self, k: KeyPress) -> Answer {
        let m = k.mods;
        if k.key == Key::Esc || (m.ctrl && matches!(k.key, Key::Char('c' | '['))) {
            return Answer::Cancel;
        }
        match &mut self.input {
            None => match k.key {
                Key::Char('y' | 'Y') if !m.ctrl && !m.alt => Answer::Yes { text: String::new(), alt: false },
                Key::Char('n' | 'N' | 'q') if !m.ctrl && !m.alt => Answer::Cancel,
                _ => Answer::Waiting,
            },
            Some(b) => match k.key {
                Key::Enter => {
                    let text = b.text.trim().to_string();
                    if text.is_empty() && !matches!(self.then, Then::StashPush | Then::LogAuthor(_)) {
                        return Answer::Waiting;
                    }
                    Answer::Yes { text, alt: m.ctrl }
                }
                Key::Backspace if b.text.is_empty() => Answer::Cancel,
                _ => {
                    let _: Edit = b.key(k, false);
                    Answer::Waiting
                }
            },
        }
    }

    /// Draws over the command line.
    pub fn draw(&self, scene: &mut Scene, lay: &Layout, c: &Colors) {
        let bar = lay.cmdline;
        scene.layer(2);
        scene.rect(bar, c.panel_background);
        scene.push_clip(bar);
        let y = lay.centered(bar.y, bar.h);
        let x = lay.pad * 2.0;
        scene.text(x, y, &self.label, TextStyle::new(c.accent).bold(), &[]);
        let tx = x + (columns(&self.label) + 1) as f32 * lay.cw;
        let caret_w = (2.0 * lay.scale).round().max(1.0);
        match &self.input {
            None => {
                let hint = "y / n";
                scene.text(tx, y, hint, TextStyle::new(c.panel_muted), &[]);
            }
            Some(b) => {
                scene.text(tx, y, &b.text, TextStyle::new(c.foreground), &[]);
                let cx = tx + columns(&b.text[..b.cursor]) as f32 * lay.cw;
                scene.rect(Rect::new(cx.round(), y, caret_w, lay.ch), c.cursor);
                let hint = match self.then {
                    Then::StashPush => "Enter stash · Ctrl+Enter with untracked files · Esc cancel",
                    _ => "Enter · Esc cancel",
                };
                let hx = bar.right() - lay.pad * 2.0 - columns(hint) as f32 * lay.cw;
                if hx > cx + 4.0 * lay.cw {
                    scene.text(hx.round(), y, hint, TextStyle::new(c.panel_muted), &[]);
                }
            }
        }
        scene.pop_clip();
        scene.layer(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(a: &mut Ask, s: &str) -> Answer {
        let mut last = Answer::Waiting;
        for k in KeyPress::parse_seq(s) {
            last = a.key(k);
        }
        last
    }

    #[test]
    fn yes_no_questions() {
        let mut a = Ask::confirm("Discard?", Then::DropStash(0));
        assert_eq!(feed(&mut a, "x"), Answer::Waiting, "other keys wait");
        assert_eq!(feed(&mut a, "y"), Answer::Yes { text: String::new(), alt: false });
        assert_eq!(feed(&mut a, "n"), Answer::Cancel);
        assert_eq!(feed(&mut a, "<Esc>"), Answer::Cancel);
    }

    #[test]
    fn text_questions() {
        let mut a = Ask::text("New branch:", "", Then::NewBranch { start: None });
        assert_eq!(feed(&mut a, "<CR>"), Answer::Waiting, "a name is needed");
        assert_eq!(feed(&mut a, "feat/x<CR>"), Answer::Yes { text: "feat/x".into(), alt: false });
        let mut a = Ask::text("Stash message:", "", Then::StashPush);
        assert_eq!(feed(&mut a, "<C-CR>"), Answer::Yes { text: String::new(), alt: true }, "no message is fine");
        let mut a = Ask::text("x", "", Then::StashPush);
        assert_eq!(feed(&mut a, "<BS>"), Answer::Cancel, "backspace on nothing leaves");
    }
}
