//! Terminal normal mode (Ctrl+\ Ctrl+N, or Ctrl+W N with vim keys, like Vim's terminal): the program keeps running
//! while the keys move a cursor over the screen and its scrollback. `j`/`k`, `w`/`b`, `gg`/`G`, Ctrl+U/D scroll and
//! move; `v`, `V` and Ctrl+V select; `y` copies; `/` and `?` search, `n`/`N` repeat; `i`, `a`, `q` or Enter go back
//! to typing into the program. alacritty_terminal's vi mode keeps the cursor and the selection.

use super::session::Listener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::search::{Match, RegexSearch};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vi_mode::ViMotion;
use shrew_core::{Key, KeyPress};

/// Search and pending-key state of one terminal's normal mode.
#[derive(Default)]
pub struct Normal {
    /// `g`, waiting for the second `g`.
    g: bool,
    /// The pattern being typed after `/` (forward) or `?`.
    pub prompt: Option<(String, bool)>,
    /// The last search: its compiled pattern and direction.
    last: Option<(RegexSearch, bool)>,
    /// The match the cursor is on, highlighted.
    pub current: Option<Match>,
}

/// What a key in normal mode did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    /// Back to typing into the program.
    Exit,
    /// Text for the clipboard (`y`).
    Copy(String),
}

pub fn active(term: &Term<Listener>) -> bool {
    term.mode().contains(TermMode::VI)
}

pub fn enter(term: &mut Term<Listener>) {
    if !active(term) {
        term.toggle_vi_mode();
    }
}

pub fn exit(term: &mut Term<Listener>, n: &mut Normal) {
    if active(term) {
        term.toggle_vi_mode();
    }
    term.selection = None;
    term.scroll_display(Scroll::Bottom);
    n.g = false;
    n.prompt = None;
    n.current = None;
}

pub fn key(term: &mut Term<Listener>, n: &mut Normal, k: KeyPress) -> Outcome {
    if n.prompt.is_some() {
        prompt_key(term, n, k);
        return Outcome::Stay;
    }
    let g = std::mem::take(&mut n.g);
    let m = k.mods;
    let half = (term.screen_lines() / 2).max(1) as i32;
    let motion = |term: &mut Term<Listener>, v: ViMotion| term.vi_motion(v);
    match (k.key, m.ctrl) {
        (Key::Char('h') | Key::Left, false) | (Key::Backspace, _) => motion(term, ViMotion::Left),
        (Key::Char('j') | Key::Down, false) => motion(term, ViMotion::Down),
        (Key::Char('k') | Key::Up, false) => motion(term, ViMotion::Up),
        (Key::Char('l') | Key::Right, false) | (Key::Char(' '), _) => motion(term, ViMotion::Right),
        (Key::Char('w'), false) => motion(term, ViMotion::SemanticRight),
        (Key::Char('b'), false) => motion(term, ViMotion::SemanticLeft),
        (Key::Char('e'), false) => motion(term, ViMotion::SemanticRightEnd),
        (Key::Char('W'), false) => motion(term, ViMotion::WordRight),
        (Key::Char('B'), false) => motion(term, ViMotion::WordLeft),
        (Key::Char('E'), false) => motion(term, ViMotion::WordRightEnd),
        (Key::Char('0') | Key::Home, false) => motion(term, ViMotion::First),
        (Key::Char('^'), false) => motion(term, ViMotion::FirstOccupied),
        (Key::Char('$') | Key::End, false) => motion(term, ViMotion::Last),
        (Key::Char('H'), false) => motion(term, ViMotion::High),
        (Key::Char('M'), false) => motion(term, ViMotion::Middle),
        (Key::Char('L'), false) => motion(term, ViMotion::Low),
        (Key::Char('%'), false) => motion(term, ViMotion::Bracket),
        (Key::Char('{'), false) => motion(term, ViMotion::ParagraphUp),
        (Key::Char('}'), false) => motion(term, ViMotion::ParagraphDown),
        (Key::Char('g'), false) if g => term.vi_goto_point(Point::new(term.topmost_line(), Column(0))),
        (Key::Char('g'), false) => n.g = true,
        (Key::Char('G'), false) => term.vi_goto_point(Point::new(term.bottommost_line(), Column(0))),
        (Key::Char('u'), true) => scroll(term, half),
        (Key::Char('d'), true) => scroll(term, -half),
        (Key::Char('b'), true) | (Key::PageUp, _) => scroll(term, 2 * half),
        (Key::Char('f'), true) | (Key::PageDown, _) => scroll(term, -2 * half),
        (Key::Char('y'), true) => scroll(term, 1),
        (Key::Char('e'), true) => scroll(term, -1),
        (Key::Char('v'), false) => select(term, SelectionType::Simple),
        (Key::Char('V'), false) => select(term, SelectionType::Lines),
        (Key::Char('v'), true) => select(term, SelectionType::Block),
        (Key::Char('y'), false) => {
            if term.selection.as_ref().is_none_or(Selection::is_empty) {
                select(term, SelectionType::Lines);
            }
            let text = term.selection_to_string();
            term.selection = None;
            if let Some(t) = text.filter(|t| !t.is_empty()) {
                return Outcome::Copy(t);
            }
        }
        (Key::Char('/'), false) => n.prompt = Some((String::new(), true)),
        (Key::Char('?'), false) => n.prompt = Some((String::new(), false)),
        (Key::Char('n'), false) => search(term, n, false),
        (Key::Char('N'), false) => search(term, n, true),
        (Key::Esc, _) if term.selection.is_some() || n.current.is_some() => {
            term.selection = None;
            n.current = None;
        }
        (Key::Esc | Key::Enter, _)
        | (Key::Char('i' | 'a' | 'I' | 'A' | 'q'), false)
        | (Key::Char('c'), true)
        | (Key::Char('\\'), true) => return Outcome::Exit,
        _ => {}
    }
    Outcome::Stay
}

/// Scrolls the view (positive: back into the history); the cursor stays on screen.
fn scroll(term: &mut Term<Listener>, lines: i32) {
    term.scroll_display(Scroll::Delta(lines));
}

/// Starts a selection of `ty` at the cursor, or ends one of that kind.
fn select(term: &mut Term<Listener>, ty: SelectionType) {
    if term.selection.as_ref().is_some_and(|s| s.ty == ty) {
        term.selection = None;
        return;
    }
    let mut s = Selection::new(ty, term.vi_mode_cursor.point, Side::Left);
    s.include_all();
    term.selection = Some(s);
}

fn prompt_key(term: &mut Term<Listener>, n: &mut Normal, k: KeyPress) {
    let Some((text, forward)) = &mut n.prompt else { return };
    match k.key {
        Key::Esc => n.prompt = None,
        Key::Enter => {
            let (pattern, forward) = (std::mem::take(text), *forward);
            n.prompt = None;
            if pattern.is_empty() {
                // `/<CR>` repeats the last search in this direction
                if let Some((_, f)) = &mut n.last {
                    *f = forward;
                }
            } else {
                match RegexSearch::new(&pattern) {
                    Ok(re) => n.last = Some((re, forward)),
                    Err(_) => {
                        n.current = None;
                        return;
                    }
                }
            }
            search(term, n, false);
        }
        Key::Backspace => {
            if text.pop().is_none() {
                n.prompt = None;
            }
        }
        Key::Char(c) if k.text().is_some() => text.push(c),
        _ => {}
    }
}

/// Jumps to the next match of the last search (`reverse`: the other way, for `N`).
fn search(term: &mut Term<Listener>, n: &mut Normal, reverse: bool) {
    let Some((re, forward)) = &mut n.last else { return };
    let dir = if *forward != reverse { Direction::Right } else { Direction::Left };
    let cursor = term.vi_mode_cursor.point;
    let origin = match dir {
        Direction::Right => cursor.add(term, Boundary::None, 1),
        Direction::Left => cursor.sub(term, Boundary::None, 1),
    };
    n.current = term.search_next(re, origin, dir, Side::Left, None);
    if let Some(m) = &n.current {
        term.vi_goto_point(*m.start());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::session::{Shared, Size};
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    fn term(text: &str) -> Term<Listener> {
        let size = Size { cols: 20, rows: 4, cell_w: 8, cell_h: 16 };
        let mut t = Term::new(Config::default(), &size, Listener(Shared::detached()));
        Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new().advance(&mut t, text.as_bytes());
        t
    }

    fn keys(t: &mut Term<Listener>, n: &mut Normal, s: &str) -> Vec<Outcome> {
        KeyPress::parse_seq(s).into_iter().map(|k| key(t, n, k)).collect()
    }

    #[test]
    fn move_select_and_copy() {
        let mut t = term("one two\r\nthree four\r\nfive");
        let mut n = Normal::default();
        enter(&mut t);
        assert!(active(&t));
        // the cursor starts where the program's is: end of "five"
        let out = keys(&mut t, &mut n, "ggwvey");
        assert_eq!(out.last(), Some(&Outcome::Copy("two".into())));
        assert!(t.selection.is_none());
        let out = keys(&mut t, &mut n, "jy");
        assert_eq!(out.last(), Some(&Outcome::Copy("three four\n".into())), "y alone copies the line");
        assert_eq!(keys(&mut t, &mut n, "i"), vec![Outcome::Exit]);
        exit(&mut t, &mut n);
        assert!(!active(&t));
    }

    #[test]
    fn search_moves_to_matches() {
        let mut t = term("alpha\r\nbeta alpha\r\ngamma");
        let mut n = Normal::default();
        enter(&mut t);
        keys(&mut t, &mut n, "gg/alpha<CR>");
        assert_eq!(t.vi_mode_cursor.point, Point::new(alacritty_terminal::index::Line(1), Column(5)));
        assert!(n.current.is_some());
        keys(&mut t, &mut n, "n");
        assert_eq!(t.vi_mode_cursor.point, Point::new(alacritty_terminal::index::Line(0), Column(0)), "wraps");
        keys(&mut t, &mut n, "N");
        assert_eq!(t.vi_mode_cursor.point.line.0, 1);
        keys(&mut t, &mut n, "<Esc>");
        assert!(n.current.is_none());
        assert_eq!(keys(&mut t, &mut n, "<Esc>"), vec![Outcome::Exit]);
    }
}
