//! Vim normal and visual mode: a small parser that turns pending keys into a command, and its execution on every
//! cursor at once.

use crate::change::{Assoc, Change, ChangeSet};
use crate::editor::{Editor, Mode, PromptKind, Register, vertical};
use crate::input::{Key, KeyPress};
use crate::motion;
use crate::ops;
use crate::selection::{Range, Selection};
use crate::text;
use ropey::Rope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Motion {
    Left,
    Right,
    Up,
    Down,
    NextLineStart,
    PrevLineStart,
    WordFwd(bool),
    WordBack(bool),
    WordEnd(bool),
    WordEndBack(bool),
    LineStart,
    FirstNonBlank,
    LineEnd,
    LastNonBlank,
    FileStart,
    FileEnd,
    Find {
        ch: char,
        forward: bool,
        till: bool,
    },
    RepeatFind(bool),
    MatchPair,
    ParaFwd,
    ParaBack,
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    SearchNext(bool),
    SearchWord(bool),
    Mark(char, bool),
    /// `N%`: the line N percent into the file.
    LinePercent,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Exclusive,
    Inclusive,
    Linewise,
}

impl Motion {
    fn kind(self) -> Kind {
        use Motion::*;
        match self {
            Up | Down | NextLineStart | PrevLineStart | FileStart | FileEnd | ScreenTop | ScreenMiddle
            | ScreenBottom | LinePercent => Kind::Linewise,
            Mark(_, true) => Kind::Linewise,
            WordEnd(_) | WordEndBack(_) | LineEnd | LastNonBlank | MatchPair => Kind::Inclusive,
            Find { forward: true, .. } => Kind::Inclusive,
            RepeatFind(_) => Kind::Inclusive,
            _ => Kind::Exclusive,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    Lower,
    Upper,
    ToggleCase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Obj {
    Word(bool),
    Pair(char, char),
    Quote(char),
    Para,
    /// `gn` / `gN`: the next (previous) search match, or the one under the cursor.
    Match(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Motion(Motion),
    Object(Obj, bool),
    Lines,
    /// `d/pat<CR>`: up to a search match, typed in the prompt.
    Search(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cmd {
    Move(Motion),
    Operate(Op, Target),
    /// An operator applied to the visual selection; `lines` forces linewise (`D`, `X`, `Y`, `C`, `S`, `R`).
    VisualOp(Op, bool),
    /// `D` / `C` on a visual block: from the block's left edge to every line's end.
    VisualToEol(Op),
    VisualObject(Obj, bool),
    Insert(InsertAt),
    Paste {
        after: bool,
    },
    Undo,
    Redo,
    Repeat,
    /// `J` (true: with spaces) and `gJ` (false).
    Join(bool),
    Replace(char),
    ToggleCaseChar,
    Visual(Mode),
    SwapEnds,
    Prompt(PromptKind),
    Scroll(ScrollKind),
    WriteQuit,
    ForceQuit,
    RecordMacro(char),
    StopMacro,
    PlayMacro(char),
    SetMark(char),
    Increment(i64),
    Escape,
    Reselect,
    NextBuffer(bool),
    /// `gn` / `gN` outside an operator: select the match in visual mode.
    SelectMatch(bool),
    /// `&` (this line, no flags) / `g&` (every line, same flags): the last `:s` again.
    RepeatSubstitute(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InsertAt {
    Before,
    After,
    LineStart,
    LineStartCol0,
    LineEnd,
    Below,
    Above,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollKind {
    HalfDown,
    HalfUp,
    PageDown,
    PageUp,
    LineDown,
    LineUp,
    Center,
    Top,
    Bottom,
}

impl Cmd {
    /// Commands `.` repeats.
    fn is_change(&self) -> bool {
        matches!(
            self,
            Cmd::Operate(
                Op::Delete | Op::Change | Op::Indent | Op::Outdent | Op::Lower | Op::Upper | Op::ToggleCase,
                _
            ) | Cmd::Insert(_)
                | Cmd::Paste { .. }
                | Cmd::VisualOp(
                    Op::Delete | Op::Change | Op::Indent | Op::Outdent | Op::Lower | Op::Upper | Op::ToggleCase,
                    _
                )
                | Cmd::VisualToEol(_)
                | Cmd::Join(_)
                | Cmd::Replace(_)
                | Cmd::ToggleCaseChar
                | Cmd::Increment(_)
                | Cmd::RepeatSubstitute(_)
        )
    }
}

pub(crate) struct Parsed {
    count: Option<usize>,
    reg: Option<char>,
    cmd: Cmd,
}

pub(crate) enum Parse {
    Pending,
    Invalid,
    Done(Parsed),
}

enum MotionParse {
    Done(Motion),
    Pending,
    No,
}

#[inline]
fn ch(k: &KeyPress) -> Option<char> {
    k.text()
}

/// The pending-key marker for insert-mode Ctrl+O (shift: it was typed at the end of the line).
pub(crate) fn is_insert_once(k: &KeyPress) -> bool {
    k.key == Key::Char('o') && k.mods.ctrl && k.mods.logo
}

#[inline]
fn ctrl(k: &KeyPress) -> Option<char> {
    match k.key {
        Key::Char(c) if k.mods.ctrl && !k.mods.alt => Some(c),
        _ => None,
    }
}

fn parse_count(keys: &[KeyPress], mut i: usize) -> (Option<usize>, usize) {
    let mut n: Option<usize> = None;
    while let Some(c) = keys.get(i).and_then(ch) {
        match c.to_digit(10) {
            Some(d) if d > 0 || n.is_some() => {
                n = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(d as usize).min(100_000));
                i += 1;
            }
            _ => break,
        }
    }
    (n, i)
}

fn parse_motion(keys: &[KeyPress], i: usize) -> MotionParse {
    use Motion::*;
    let k = &keys[i];
    let next = keys.get(i + 1);
    let m = match k.key {
        Key::Left | Key::Backspace => Left,
        Key::Right => Right,
        Key::Up => Up,
        Key::Down => Down,
        Key::Home => LineStart,
        Key::End => LineEnd,
        Key::Enter => NextLineStart,
        _ => {
            if let Some(c) = ctrl(k) {
                return match c {
                    'p' => MotionParse::Done(Up),
                    'j' => MotionParse::Done(Down),
                    _ => MotionParse::No,
                };
            }
            let Some(c) = ch(k) else { return MotionParse::No };
            match c {
                'h' => Left,
                'l' | ' ' => Right,
                'j' => Down,
                'k' => Up,
                '+' => NextLineStart,
                '-' => PrevLineStart,
                'w' => WordFwd(false),
                'W' => WordFwd(true),
                'b' => WordBack(false),
                'B' => WordBack(true),
                'e' => WordEnd(false),
                'E' => WordEnd(true),
                '0' => LineStart,
                '^' | '_' => FirstNonBlank,
                '$' => LineEnd,
                'G' => FileEnd,
                '%' => MatchPair,
                '}' => ParaFwd,
                '{' => ParaBack,
                'H' => ScreenTop,
                'M' => ScreenMiddle,
                'L' => ScreenBottom,
                'n' => SearchNext(false),
                'N' => SearchNext(true),
                '*' => SearchWord(true),
                '#' => SearchWord(false),
                ';' => RepeatFind(false),
                ',' => RepeatFind(true),
                'g' => {
                    let Some(n) = next else { return MotionParse::Pending };
                    return match ch(n) {
                        Some('g') => MotionParse::Done(FileStart),
                        Some('e') => MotionParse::Done(WordEndBack(false)),
                        Some('E') => MotionParse::Done(WordEndBack(true)),
                        Some('_') => MotionParse::Done(LastNonBlank),
                        Some('j') => MotionParse::Done(Down),
                        Some('k') => MotionParse::Done(Up),
                        Some('0') => MotionParse::Done(LineStart),
                        Some('$') => MotionParse::Done(LineEnd),
                        _ => MotionParse::No,
                    };
                }
                'f' | 'F' | 't' | 'T' => {
                    let Some(n) = next else { return MotionParse::Pending };
                    let Some(target) = ch(n) else { return MotionParse::No };
                    return MotionParse::Done(Find {
                        ch: target,
                        forward: c == 'f' || c == 't',
                        till: c == 't' || c == 'T',
                    });
                }
                '`' | '\'' => {
                    let Some(n) = next else { return MotionParse::Pending };
                    let Some(mark) = ch(n) else { return MotionParse::No };
                    return MotionParse::Done(Mark(mark, c == '\''));
                }
                _ => return MotionParse::No,
            }
        }
    };
    MotionParse::Done(m)
}

fn parse_object(keys: &[KeyPress], i: usize) -> Option<Result<(Obj, bool), ()>> {
    let around = match keys.get(i).and_then(ch)? {
        'i' => false,
        'a' => true,
        _ => return Some(Err(())),
    };
    let c = keys.get(i + 1)?;
    let obj = match ch(c) {
        Some('w') => Obj::Word(false),
        Some('W') => Obj::Word(true),
        Some('(' | ')' | 'b') => Obj::Pair('(', ')'),
        Some('{' | '}' | 'B') => Obj::Pair('{', '}'),
        Some('[' | ']') => Obj::Pair('[', ']'),
        Some('<' | '>') => Obj::Pair('<', '>'),
        Some(q @ ('"' | '\'' | '`')) => Obj::Quote(q),
        Some('p') => Obj::Para,
        _ => return Some(Err(())),
    };
    Some(Ok((obj, around)))
}

pub(crate) fn parse(keys: &[KeyPress], mode: Mode, recording: bool) -> Parse {
    use Cmd::*;
    let mut i = 0;
    let mut reg = None;
    if keys.first().and_then(ch) == Some('"') {
        let Some(r) = keys.get(1) else { return Parse::Pending };
        let Some(r) = ch(r) else { return Parse::Invalid };
        reg = Some(r);
        i = 2;
    }
    let (count, j) = parse_count(keys, i);
    i = j;
    let Some(k) = keys.get(i) else { return Parse::Pending };
    let visual = mode.is_visual();
    let done = |cmd: Cmd| Parse::Done(Parsed { count, reg, cmd });

    if k.key == Key::Esc || ctrl(k) == Some('c') || ctrl(k) == Some('[') {
        return done(Escape);
    }

    // operators
    let op = match (ch(k), keys.get(i + 1).and_then(ch)) {
        (Some('d'), _) => Some((Op::Delete, 1)),
        (Some('c'), _) => Some((Op::Change, 1)),
        (Some('y'), _) => Some((Op::Yank, 1)),
        (Some('>'), _) => Some((Op::Indent, 1)),
        (Some('<'), _) => Some((Op::Outdent, 1)),
        (Some('g'), Some('~')) => Some((Op::ToggleCase, 2)),
        (Some('g'), Some('u')) => Some((Op::Lower, 2)),
        (Some('g'), Some('U')) => Some((Op::Upper, 2)),
        _ => None,
    };
    if let Some((op, len)) = op {
        if visual {
            return done(VisualOp(op, false));
        }
        let (count2, j) = parse_count(keys, i + len);
        let count = match (count, count2) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(1) * b.unwrap_or(1)),
        };
        let done = |cmd: Cmd| Parse::Done(Parsed { count, reg, cmd });
        let Some(t) = keys.get(j) else { return Parse::Pending };
        // dd, cc, yy, >>, <<, g~~, guu, gUU (and g~g~ / gugu)
        let last = ch(&keys[i + len - 1]);
        if ch(t) == last || (len == 2 && ch(t) == Some('g') && keys.get(j + 1).and_then(ch) == last) {
            if len == 2 && ch(t) == Some('g') && keys.get(j + 1).is_none() {
                return Parse::Pending;
            }
            return done(Operate(op, Target::Lines));
        }
        if let Some(c @ ('/' | '?')) = ch(t) {
            return done(Operate(op, Target::Search(c == '/')));
        }
        if ch(t) == Some('g') {
            match keys.get(j + 1).map(ch) {
                None => return Parse::Pending,
                Some(Some(c @ ('n' | 'N'))) => return done(Operate(op, Target::Object(Obj::Match(c == 'n'), false))),
                _ => {}
            }
        }
        match parse_object(keys, j) {
            None => return Parse::Pending,
            Some(Ok((obj, around))) => return done(Operate(op, Target::Object(obj, around))),
            Some(Err(())) => {}
        }
        return match parse_motion(keys, j) {
            MotionParse::Done(m) => done(Operate(op, Target::Motion(m))),
            MotionParse::Pending => Parse::Pending,
            MotionParse::No => Parse::Invalid,
        };
    }

    if visual && let Some(c) = ch(k) {
        if c == 'i' || c == 'a' {
            return match parse_object(keys, i) {
                None => Parse::Pending,
                Some(Ok((obj, around))) => done(VisualObject(obj, around)),
                Some(Err(())) => Parse::Invalid,
            };
        }
        let lines = |op| done(VisualOp(op, true));
        match c {
            'x' => return done(VisualOp(Op::Delete, false)),
            's' => return done(VisualOp(Op::Change, false)),
            // in a block, D and C go to the end of every line
            'D' if mode == Mode::VisualBlock => return done(VisualToEol(Op::Delete)),
            'C' if mode == Mode::VisualBlock => return done(VisualToEol(Op::Change)),
            'X' | 'D' => return lines(Op::Delete),
            'C' | 'S' | 'R' => return lines(Op::Change),
            'Y' => return lines(Op::Yank),
            'u' => return done(VisualOp(Op::Lower, false)),
            'U' => return done(VisualOp(Op::Upper, false)),
            '~' => return done(VisualOp(Op::ToggleCase, false)),
            'o' | 'O' => return done(SwapEnds),
            'I' => return done(Insert(InsertAt::Before)),
            'A' => return done(Insert(InsertAt::After)),
            _ => {}
        }
    }

    match parse_motion(keys, i) {
        MotionParse::Done(m) => return done(Move(m)),
        MotionParse::Pending => return Parse::Pending,
        MotionParse::No => {}
    }

    if let Some(c) = ctrl(k) {
        return match c {
            'r' => done(Redo),
            'd' => done(Scroll(ScrollKind::HalfDown)),
            'u' => done(Scroll(ScrollKind::HalfUp)),
            'f' => done(Scroll(ScrollKind::PageDown)),
            'b' => done(Scroll(ScrollKind::PageUp)),
            'e' => done(Scroll(ScrollKind::LineDown)),
            'y' => done(Scroll(ScrollKind::LineUp)),
            'v' | 'q' => done(Visual(Mode::VisualBlock)),
            'a' => done(Increment(1)),
            'x' => done(Increment(-1)),
            _ => Parse::Invalid,
        };
    }
    let Some(c) = ch(k) else {
        return match k.key {
            Key::PageDown => done(Scroll(ScrollKind::PageDown)),
            Key::PageUp => done(Scroll(ScrollKind::PageUp)),
            Key::Delete => done(if visual {
                VisualOp(Op::Delete, false)
            } else {
                Operate(Op::Delete, Target::Motion(Motion::Right))
            }),
            Key::Insert => done(Insert(InsertAt::Before)),
            _ => Parse::Invalid,
        };
    };
    let next = keys.get(i + 1);
    let need_char = |f: fn(char) -> Cmd| match next {
        None => Parse::Pending,
        Some(n) => match ch(n) {
            Some(c) => done(f(c)),
            None => Parse::Invalid,
        },
    };
    match c {
        'i' => done(Insert(InsertAt::Before)),
        'a' => done(Insert(InsertAt::After)),
        'I' => done(Insert(InsertAt::LineStart)),
        'A' => done(Insert(InsertAt::LineEnd)),
        'o' => done(Insert(InsertAt::Below)),
        'O' => done(Insert(InsertAt::Above)),
        'x' => done(Operate(Op::Delete, Target::Motion(Motion::Right))),
        'X' => done(Operate(Op::Delete, Target::Motion(Motion::Left))),
        's' => done(Operate(Op::Change, Target::Motion(Motion::Right))),
        'S' => done(Operate(Op::Change, Target::Lines)),
        'C' => done(Operate(Op::Change, Target::Motion(Motion::LineEnd))),
        'D' => done(Operate(Op::Delete, Target::Motion(Motion::LineEnd))),
        'Y' => done(Operate(Op::Yank, Target::Motion(Motion::LineEnd))),
        'p' => done(Paste { after: true }),
        'P' => done(Paste { after: false }),
        'u' => done(Undo),
        '.' => done(Repeat),
        'J' => done(Join(true)),
        '&' => done(RepeatSubstitute(false)),
        '~' => done(ToggleCaseChar),
        'v' => done(Visual(Mode::Visual)),
        'V' => done(Visual(Mode::VisualLine)),
        ':' => done(Prompt(PromptKind::Command)),
        '/' => done(Prompt(PromptKind::Search { forward: true })),
        '?' => done(Prompt(PromptKind::Search { forward: false })),
        'r' => match next {
            Some(n) if n.key == Key::Enter => done(Replace('\n')),
            _ => need_char(Replace),
        },
        'm' => need_char(SetMark),
        '@' => need_char(PlayMacro),
        'q' if recording => done(StopMacro),
        'q' => need_char(RecordMacro),
        'z' => match next.map(|n| (ch(n), n.key)) {
            None => Parse::Pending,
            Some((Some('z' | '.'), _)) => done(Scroll(ScrollKind::Center)),
            Some((Some('t'), _)) | Some((_, Key::Enter)) => done(Scroll(ScrollKind::Top)),
            Some((Some('b' | '-'), _)) => done(Scroll(ScrollKind::Bottom)),
            _ => Parse::Invalid,
        },
        'Z' => match next.and_then(ch) {
            None if next.is_none() => Parse::Pending,
            Some('Z') => done(WriteQuit),
            Some('Q') => done(ForceQuit),
            _ => Parse::Invalid,
        },
        'g' => match next.and_then(ch) {
            None if next.is_none() => Parse::Pending,
            Some('v') => done(Reselect),
            Some('t') => done(NextBuffer(true)),
            Some('T') => done(NextBuffer(false)),
            Some('J') => done(Join(false)),
            Some('n') => done(SelectMatch(true)),
            Some('N') => done(SelectMatch(false)),
            Some('&') => done(RepeatSubstitute(true)),
            Some('I') => done(Insert(InsertAt::LineStartCol0)),
            _ => Parse::Invalid,
        },
        _ => Parse::Invalid,
    }
}

impl Editor {
    pub(crate) fn vim_key(&mut self, k: KeyPress) {
        self.pending.push(k);
        let mut keys = self.pending.clone();
        // Ctrl+O from insert mode: one command, then back to inserting
        let once = keys.first().copied().filter(is_insert_once);
        if once.is_some() {
            keys.remove(0);
            if keys.is_empty() {
                return;
            }
        }
        match parse(&keys, self.mode, self.macro_rec.is_some()) {
            Parse::Pending => {}
            Parse::Invalid => {
                self.pending.clear();
                if let Some(o) = once {
                    self.resume_insert(o);
                }
            }
            Parse::Done(p) => {
                self.pending.clear();
                if once.is_some() && p.cmd == Cmd::Escape {
                    return; // Esc after Ctrl+O: stay in normal mode
                }
                let visual = self.mode.is_visual();
                let record =
                    p.cmd.is_change() && !self.replaying && once.is_none() && (self.mode == Mode::Normal || visual);
                if record {
                    // a visual change repeats on as much text from the cursor
                    let mut rec = if visual { self.visual_dot_prefix() } else { Vec::new() };
                    rec.extend(keys);
                    self.dot_recording = Some(rec);
                }
                self.exec(p);
                let waiting_for_search = self.prompt.as_ref().is_some_and(|p| p.op.is_some());
                if self.mode != Mode::Insert && !waiting_for_search {
                    if record && let Some(rec) = self.dot_recording.take() {
                        self.dot = rec;
                    }
                    self.commit();
                }
                if let Some(o) = once.filter(|_| self.mode == Mode::Normal) {
                    if self.prompt.is_some() {
                        self.pending = vec![o]; // back to inserting when the command line is done
                    } else {
                        self.resume_insert(o);
                    }
                }
            }
        }
    }

    /// Insert-mode Ctrl+O: normal mode for one command. The cursor stays put (a caret past the end of the line
    /// sits on its last char until the command is done).
    pub(crate) fn insert_ctrl_o(&mut self) {
        if let Some(rec) = self.dot_recording.take() {
            self.dot = rec;
        }
        let rope = &self.buf().doc.rope;
        let h = self.buf().sel.primary().head;
        let l = text::line_of(rope, h);
        let eol = h == text::line_end(rope, l) && text::line_len(rope, l) > 0;
        self.set_mode_raw(Mode::Normal);
        self.commit();
        self.clamp_cursors();
        let mods = crate::input::Mods { ctrl: true, logo: true, shift: eol, alt: false };
        self.pending = vec![KeyPress::new(Key::Char('o'), mods)];
    }

    /// After a command line opened from insert-mode Ctrl+O: back to inserting.
    pub(crate) fn finish_insert_once(&mut self) {
        if let Some(&o) = self.pending.first().filter(|k| is_insert_once(k)) {
            self.pending.clear();
            if self.mode == Mode::Normal {
                self.resume_insert(o);
            }
        }
    }

    fn resume_insert(&mut self, once: KeyPress) {
        self.set_mode_raw(Mode::Insert);
        // after `$` the caret goes past the last char
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        b.sel = b.sel.transform(|r| {
            if r.col == Some(usize::MAX) { Range::point(text::line_end(rope, text::line_of(rope, r.head))) } else { r }
        });
        if once.mods.shift {
            // back after the last char when Ctrl+O was typed at the end of the line
            let b = self.buf_mut();
            let rope = &b.doc.rope;
            b.sel = b.sel.transform(|r| {
                let le = text::line_end(rope, text::line_of(rope, r.head));
                if text::next_grapheme(rope, r.head) == le { Range::point(le) } else { r }
            });
        }
    }

    /// Keys that select as much text as the current visual selection, starting at the cursor (for `.`).
    fn visual_dot_prefix(&self) -> Vec<KeyPress> {
        let rope = &self.buf().doc.rope;
        let tw = self.settings.tab_width;
        let mut out = Vec::new();
        let count = |out: &mut Vec<KeyPress>, n: usize, k: char| {
            if n > 0 {
                out.extend(n.to_string().chars().map(KeyPress::ch));
                out.push(KeyPress::ch(k));
            }
        };
        let (from, to) = match (self.block, self.buf().sel.ranges()) {
            (Some((a, h)), _) => (a.min(h), a.max(h)),
            (None, rs) => (rs.first().map_or(0, |r| r.from()), rs.last().map_or(0, |r| r.to())),
        };
        let (la, lz) = (text::line_of(rope, from), text::line_of(rope, to));
        let eol = to == text::line_end(rope, lz) && text::line_len(rope, lz) > 0;
        match self.mode {
            Mode::VisualLine => {
                out.push(KeyPress::ch('V'));
                count(&mut out, lz - la, 'j');
            }
            Mode::VisualBlock => {
                out.push(KeyPress::ctrl('v'));
                count(&mut out, lz - la, 'j');
                if eol {
                    out.push(KeyPress::ch('$'));
                } else {
                    let (ca, cz) = (text::col_of(rope, from, tw), text::col_of(rope, to, tw));
                    count(&mut out, ca.abs_diff(cz), 'l');
                }
            }
            _ => {
                out.push(KeyPress::ch('v'));
                if lz > la {
                    count(&mut out, lz - la, 'j');
                    out.push(KeyPress::ch('0'));
                }
                if eol {
                    out.push(KeyPress::ch('$'));
                } else {
                    let start = if lz > la { text::line_start(rope, lz) } else { from };
                    count(&mut out, to - start, 'l');
                }
            }
        }
        out
    }

    fn exec(&mut self, p: Parsed) {
        use Cmd::*;
        let count = p.count.unwrap_or(1);
        match p.cmd {
            Move(m) => self.do_move(m, p.count),
            Operate(op, Target::Search(forward)) => {
                // the prompt runs the operator when the pattern is entered
                self.open_prompt(PromptKind::Search { forward });
                if let Some(pr) = self.prompt.as_mut() {
                    pr.op = Some((op, p.reg));
                    pr.count = p.count;
                }
            }
            Operate(op, target) => {
                let spans = self.op_spans(op, target, p.count);
                self.apply_op(
                    op,
                    spans,
                    p.reg,
                    matches!(target, Target::Motion(Motion::Up | Motion::Down | Motion::FileStart)),
                    1,
                );
            }
            VisualOp(op, lines) => {
                let spans = self.visual_spans(lines);
                self.apply_op(op, spans, p.reg, false, count);
            }
            VisualToEol(op) => {
                if let Some((a, h)) = self.block {
                    let rope = &self.buf().doc.rope;
                    self.block = Some((a, text::line_end(rope, text::line_of(rope, h))));
                    self.rebuild_block();
                }
                let spans = self.visual_spans(false);
                self.apply_op(op, spans, p.reg, false, 1);
            }
            VisualObject(obj, around) => {
                let b = self.buf();
                let rope = &b.doc.rope;
                let ranges: Vec<Range> = b
                    .sel
                    .ranges()
                    .iter()
                    .map(|r| {
                        // `vipip`: a selection that already spans lines grows by the next paragraph
                        if obj == Obj::Para && r.anchor != r.head {
                            let l = text::line_of(rope, r.to()) + 1;
                            if l <= text::last_line(rope) {
                                let (_, z) = motion::paragraph_object_n(rope, text::line_start(rope, l), around, count);
                                return Range::new(r.from(), z.saturating_sub(1).max(r.from()));
                            }
                            return *r;
                        }
                        let pair = matches!(obj, Obj::Pair(..));
                        let mut n = count;
                        let mut span = object_span(rope, r.head, obj, around, n);
                        // `vi(i(`, `va(a(`: a selection that already is the block grows to the enclosing one
                        while let Some((a, z)) = span.filter(|_| pair && r.anchor != r.head && n < count + 100) {
                            if z > a && (a < r.from() || z - 1 > r.to()) {
                                break;
                            }
                            n += 1;
                            span = object_span(rope, r.head, obj, around, n);
                        }
                        match span {
                            Some((a, z)) if z > a && pair => Range::new(a, z - 1),
                            Some((a, z)) if z > a => Range::new(a.min(r.anchor), z - 1),
                            _ => *r,
                        }
                    })
                    .collect();
                let prim = b.sel.primary_index();
                self.buf_mut().sel = Selection::new(ranges, prim);
                if obj == Obj::Para && self.mode == Mode::Visual {
                    self.mode = Mode::VisualLine;
                }
            }
            Insert(at) => self.enter_insert(at),
            Paste { after } => self.paste(p.reg, after, count),
            Undo => {
                for _ in 0..count {
                    self.undo();
                }
            }
            Redo => {
                for _ in 0..count {
                    self.redo();
                }
            }
            Repeat => {
                if let Some(n) = p.count {
                    // `3.` replaces the count (and is remembered for the next `.`)
                    let mut keys = std::mem::take(&mut self.dot);
                    let reg_len = if keys.first().and_then(ch) == Some('"') { 2.min(keys.len()) } else { 0 };
                    let (_, skip) = parse_count(&keys, reg_len);
                    keys.drain(reg_len..skip);
                    let digits: Vec<KeyPress> = n.to_string().chars().map(KeyPress::ch).collect();
                    keys.splice(reg_len..reg_len, digits);
                    self.dot = keys;
                }
                let keys = self.dot.clone();
                self.replaying = true;
                for k in keys {
                    self.handle_key(k);
                }
                self.replaying = false;
                if self.mode == Mode::Insert {
                    self.set_mode(Mode::Normal);
                }
            }
            Join(spaces) => {
                let visual = self.mode.is_visual();
                let rope = &self.buf().doc.rope;
                let last = text::last_line(rope);
                let (first, lz) = if visual {
                    let spans = self.visual_spans(true);
                    let la = spans.iter().map(|s| text::line_of(rope, s.0)).min().unwrap_or(0);
                    let lz =
                        spans.iter().map(|s| text::line_of(rope, s.1.saturating_sub(1).max(s.0))).max().unwrap_or(0);
                    (la, lz.max(la + 1))
                } else {
                    let l = self.buf().cursor_line();
                    if p.count.is_some_and(|c| c > 2) || l < last {
                        (l, l + count.max(2) - 1)
                    } else {
                        (l, l) // J on the last line does nothing
                    }
                };
                let lz = lz.min(last);
                if lz > first {
                    let lines: Vec<usize> = (first..=lz).collect();
                    self.set_mode_raw(Mode::Normal);
                    // the cursor lands on the last joint, like vim
                    let joint = ops::join_lines(self.buf_mut(), &lines, spaces, false);
                    if let Some(j) = joint {
                        self.buf_mut().sel = Selection::point(j);
                    }
                    self.clamp_cursors();
                } else if visual {
                    self.set_mode(Mode::Normal);
                }
            }
            Replace(c) => self.replace_chars(c, count),
            ToggleCaseChar => {
                let spans = self.op_spans(Op::ToggleCase, Target::Motion(Motion::Right), Some(count));
                let ends: Vec<usize> = spans.iter().map(|s| s.1).collect();
                self.apply_op(Op::ToggleCase, spans, None, false, 1);
                // the cursor moves past the toggled chars (case changes keep char counts for nearly all text)
                let prim = self.buf().sel.primary_index().min(ends.len().saturating_sub(1));
                if !ends.is_empty() {
                    let b = self.buf_mut();
                    let rope = &b.doc.rope;
                    b.sel = Selection::new(ends.into_iter().map(|e| Range::point(text::clamp_normal(rope, e))), prim);
                }
            }
            Visual(mode) => {
                if self.mode == mode {
                    self.set_mode(Mode::Normal);
                } else if self.mode.is_visual() {
                    if mode == Mode::VisualBlock {
                        let p = self.buf().sel.primary();
                        self.block = Some((p.anchor, p.head));
                        self.mode = mode;
                        self.rebuild_block();
                    } else {
                        if self.mode == Mode::VisualBlock {
                            let (a, h) = self.block.unwrap_or((0, 0));
                            self.buf_mut().sel = Selection::single(Range::new(a, h));
                            self.block = None;
                        }
                        self.mode = mode;
                    }
                } else {
                    self.set_mode(mode);
                }
            }
            SwapEnds => {
                if let Some((a, h)) = self.block {
                    self.block = Some((h, a));
                    self.rebuild_block();
                } else {
                    let b = self.buf_mut();
                    b.sel = b.sel.transform(|r| Range::new(r.head, r.anchor));
                }
            }
            Prompt(kind) => {
                self.open_prompt(kind);
                if let Some(pr) = self.prompt.as_mut() {
                    pr.count = p.count;
                }
            }
            Scroll(s) => self.do_scroll(s, p.count),
            WriteQuit => self.run_ex("x"),
            ForceQuit => self.run_ex("q!"),
            RecordMacro(r) => {
                self.macro_rec = Some((r, Vec::new()));
                self.info(format!("recording @{r}"));
            }
            StopMacro => {
                if let Some((r, mut keys)) = self.macro_rec.take() {
                    keys.pop(); // the `q` that stopped it
                    self.macros.insert(r, keys);
                    self.info(format!("recorded @{r}"));
                }
            }
            PlayMacro(r) => {
                let r = if r == '@' { self.last_macro.unwrap_or('q') } else { r };
                self.last_macro = Some(r);
                if r == ':' {
                    // `@:` repeats the last command line
                    for _ in 0..count.min(10_000) {
                        self.repeat_last_ex();
                    }
                    return;
                }
                let Some(keys) = self.macros.get(&r).cloned() else { return };
                // the whole run is one undo step, like in vim
                self.held(|e| {
                    for _ in 0..count.min(10_000) {
                        for k in &keys {
                            e.handle_key(*k);
                        }
                    }
                });
            }
            SetMark(c) => {
                self.map_marks();
                let id = self.buf().id;
                let h = self.buf().sel.primary().head;
                self.marks.insert((id, c), h);
            }
            Increment(d) => self.increment(d * count as i64),
            Escape => {
                if self.mode.is_visual() {
                    self.set_mode(Mode::Normal);
                } else {
                    let b = self.buf_mut();
                    b.sel = b.sel.only_primary();
                    self.search_visible = false;
                }
            }
            Reselect => {
                if let Some((mode, sel)) = self.last_visual.clone() {
                    let len = self.buf().doc.len();
                    self.buf_mut().sel = sel.clamp(len.saturating_sub(1));
                    self.mode = mode;
                    if mode == Mode::VisualBlock {
                        let p = self.buf().sel.primary();
                        self.block = Some((self.buf().sel.ranges()[0].anchor, p.head));
                    }
                }
            }
            NextBuffer(fwd) => {
                self.run(if fwd { crate::commands::Command::NextBuffer } else { crate::commands::Command::PrevBuffer })
            }
            SelectMatch(forward) => {
                let rope = self.buf().doc.rope.clone();
                let h = self.buf().sel.primary().head;
                if let Some((a, z)) = self.match_object(&rope, h, forward) {
                    let anchor = if self.mode.is_visual() { self.buf().sel.primary().anchor } else { a };
                    if !self.mode.is_visual() {
                        self.set_mode(Mode::Visual);
                    }
                    let (anchor, head) = if forward { (anchor.min(a), z - 1) } else { (anchor.max(z - 1), a) };
                    self.buf_mut().sel = Selection::single(Range::new(anchor, head));
                }
            }
            RepeatSubstitute(all) => {
                let line = self.buf().cursor_line();
                let last = text::last_line(&self.buf().doc.rope);
                if all { self.repeat_substitute(0, last, true) } else { self.repeat_substitute(line, line, false) }
            }
        }
    }

    /// Where `m` takes a cursor at `pos`, or None when it cannot move (e.g. `f` without a match).
    fn motion_target(
        &mut self,
        rope: &Rope,
        pos: usize,
        m: Motion,
        count: Option<usize>,
        sticky: Option<usize>,
    ) -> Option<(usize, Option<usize>)> {
        use Motion::*;
        let n = count.unwrap_or(1);
        let tw = self.settings.tab_width;
        let len = rope.len_chars();
        let line = text::line_of(rope, pos);
        let last = text::last_line(rope);
        let view = self.buf().view;
        let so = self.settings.scrolloff.min(view.rows / 2);
        let repeat = |mut p: usize, f: &dyn Fn(usize) -> usize| {
            for _ in 0..n {
                p = f(p);
            }
            p
        };
        let t = match m {
            Left => {
                let ls = text::line_start(rope, line);
                let mut p = pos;
                for _ in 0..n.min(pos - ls) {
                    p = text::prev_grapheme(rope, p).max(ls);
                }
                p
            }
            Right => {
                let le = text::line_end(rope, line);
                let mut p = pos;
                for _ in 0..n.min(le.saturating_sub(pos)) {
                    p = text::next_grapheme(rope, p).min(le);
                }
                p
            }
            Up | Down => {
                let d = if m == Up { -(n as isize) } else { n as isize };
                let r = Range { anchor: pos, head: pos, col: sticky };
                let target = (line as isize + d).clamp(0, last as isize) as usize;
                if target == line {
                    return None;
                }
                let (p, c) = vertical(rope, r, d, tw);
                return Some((p, Some(c)));
            }
            NextLineStart => text::first_non_blank(rope, (line + n).min(last)),
            PrevLineStart => text::first_non_blank(rope, line.saturating_sub(n)),
            WordFwd(big) => repeat(pos, &|p| motion::next_word_start(rope, p, big)),
            WordBack(big) => repeat(pos, &|p| motion::prev_word_start(rope, p, big)),
            WordEnd(big) => repeat(pos, &|p| motion::word_end(rope, p, big)),
            WordEndBack(big) => repeat(pos, &|p| motion::prev_word_end(rope, p, big)),
            LineStart => text::line_start(rope, line),
            FirstNonBlank => text::first_non_blank(rope, line),
            LineEnd => {
                if line + n - 1 > last {
                    return None; // `3$` / `3D` past the last line fails, like vim
                }
                let l = line + n - 1;
                let e = text::line_end(rope, l);
                return Some((if e > text::line_start(rope, l) { e - 1 } else { e }, Some(usize::MAX)));
            }
            LastNonBlank => {
                let ls = text::line_start(rope, line);
                let mut e = text::line_end(rope, line);
                while e > ls && rope.char(e - 1).is_whitespace() {
                    e -= 1;
                }
                e.saturating_sub(1).max(ls)
            }
            FileStart => text::first_non_blank(rope, count.map_or(0, |c| c.saturating_sub(1)).min(last)),
            FileEnd => text::first_non_blank(rope, count.map_or(last, |c| c.saturating_sub(1)).min(last)),
            Find { ch, forward, till } => {
                self.last_find = Some((ch, forward, till));
                motion::find_in_line(rope, pos, ch, forward, till, n)?
            }
            RepeatFind(rev) => {
                let (ch, forward, till) = self.last_find?;
                let forward = forward != rev;
                // `;` after `t` must not get stuck right before the same char
                let start = if till && forward && pos + 1 < len && rope.char(pos + 1) == ch {
                    pos + 1
                } else if till && !forward && pos > 0 && rope.char(pos - 1) == ch {
                    pos - 1
                } else {
                    pos
                };
                motion::find_in_line(rope, start, ch, forward, till, n)?
            }
            MatchPair if count.is_some() => return self.motion_target(rope, pos, LinePercent, count, sticky),
            MatchPair => motion::match_pair_motion(rope, pos)?,
            LinePercent => {
                let total = last + 1;
                let l = (n.min(100) * total).div_ceil(100).saturating_sub(1);
                text::first_non_blank(rope, l.min(last))
            }
            ParaFwd => repeat(pos, &|p| motion::paragraph_forward(rope, p)),
            ParaBack => repeat(pos, &|p| motion::paragraph_backward(rope, p)),
            ScreenTop => text::first_non_blank(
                rope,
                (view.top + if view.top == 0 { 0 } else { so }).max(view.top + n - 1).min(last),
            ),
            ScreenMiddle => {
                text::first_non_blank(rope, (view.top + view.rows / 2).min(last).min(view.top + (last - view.top) / 2))
            }
            ScreenBottom => {
                let bottom = (view.top + view.rows).saturating_sub(1).min(last);
                let bottom = if bottom == last { bottom } else { bottom.saturating_sub(so) };
                text::first_non_blank(rope, bottom.saturating_sub(n - 1))
            }
            SearchNext(rev) => {
                let re = self.search.clone()?;
                self.search_visible = true;
                let forward = self.last_search_forward != rev;
                let mut p = pos;
                for _ in 0..n {
                    p = crate::search::find(rope, &re, p, forward)?.0;
                }
                p
            }
            SearchWord(forward) => {
                let (a, z) = ops::word_at(rope, pos)?;
                let word = rope.slice(a..z).to_string();
                self.set_search(&format!(r"\<{}\>", regex::escape(&word)));
                // `*` is case-sensitive even with smart case
                self.search = regex::Regex::new(&format!(r"\b{}\b", regex::escape(&word))).ok();
                self.last_search_forward = forward;
                let re = self.search.clone()?;
                // from the start of the word, so `*` on leading blanks finds the next occurrence of the word after
                let mut p = a;
                let _ = pos;
                for _ in 0..n {
                    p = crate::search::find(rope, &re, p, forward)?.0;
                }
                p
            }
            Mark(c, linewise) => {
                let id = self.buf().id;
                let p = match c {
                    // the last visual selection's start and end
                    '<' | '>' => {
                        let (vmode, sel) = self.last_visual.as_ref()?;
                        let (from, to) = (sel.ranges().first()?.from(), sel.ranges().last()?.to());
                        match (c, vmode) {
                            ('<', Mode::VisualLine) => text::line_start(rope, text::line_of(rope, from)),
                            ('<', _) => from,
                            (_, Mode::VisualLine) => {
                                text::clamp_normal(rope, text::line_end(rope, text::line_of(rope, to)))
                            }
                            _ => to,
                        }
                    }
                    c => *self.marks.get(&(id, c))?,
                }
                .min(len);
                if linewise { text::first_non_blank(rope, text::line_of(rope, p)) } else { p }
            }
        };
        Some((t, None))
    }

    fn do_move(&mut self, m: Motion, count: Option<usize>) {
        let mode = self.mode;
        if mode == Mode::VisualBlock {
            let Some((anchor, head)) = self.block else { return };
            let rope = self.buf().doc.rope.clone();
            // `$` makes the block reach every line's end; the head then sits on the line break
            let eol = head == text::line_end(&rope, text::line_of(&rope, head))
                && text::line_len(&rope, text::line_of(&rope, head)) > 0;
            let sticky = if eol { Some(usize::MAX) } else { self.buf().sel.primary().col };
            if let Some((t, col)) = self.motion_target(&rope, head, m, count, sticky) {
                let t = if col == Some(usize::MAX) {
                    text::line_end(&rope, text::line_of(&rope, t))
                } else {
                    text::clamp_normal(&rope, t)
                };
                self.block = Some((anchor, t));
                self.rebuild_block();
                // the head keeps its column across short lines
                let b = self.buf_mut();
                let p = b.sel.primary_index();
                let mut ranges = b.sel.ranges().to_vec();
                ranges[p].col = col;
                b.sel = Selection::new(ranges, p);
            }
            return;
        }
        let rope = self.buf().doc.rope.clone(); // O(1): ropes share their nodes
        let ranges: Vec<Range> = self.buf().sel.ranges().to_vec();
        let prim = self.buf().sel.primary_index();
        let mut out = Vec::with_capacity(ranges.len());
        let mut moved = false;
        for r in ranges {
            let target = self.motion_target(&rope, r.head, m, count, r.col);
            let Some((t, col)) = target else {
                out.push(r);
                continue;
            };
            moved = true;
            // in visual mode `$` takes the line break too
            let t = if mode.is_visual() && col == Some(usize::MAX) {
                text::line_end(&rope, text::line_of(&rope, t))
            } else {
                text::clamp_normal(&rope, t)
            };
            let mut nr = if mode.is_visual() { Range::new(r.anchor, t) } else { Range::point(t) };
            nr.col = col;
            out.push(nr);
        }
        if moved {
            self.buf_mut().sel = Selection::new(out, prim);
            if matches!(
                m,
                Motion::SearchNext(_)
                    | Motion::SearchWord(_)
                    | Motion::FileStart
                    | Motion::FileEnd
                    | Motion::Mark(..)
                    | Motion::MatchPair
                    | Motion::LinePercent
            ) {
                let rows = self.buf().view.rows;
                let line = self.buf().cursor_line();
                let top = self.buf().view.top;
                if line < top || line >= top + rows {
                    self.buf_mut().center_on_cursor();
                }
            }
        }
    }

    /// `[from, to)` spans for an operator, one per cursor, plus whether each is linewise.
    fn op_spans(&mut self, op: Op, target: Target, count: Option<usize>) -> Vec<(usize, usize, bool)> {
        let rope = self.buf().doc.rope.clone();
        let n = count.unwrap_or(1);
        let heads: Vec<(usize, Option<usize>)> = self.buf().sel.ranges().iter().map(|r| (r.head, r.col)).collect();
        let mut out = Vec::with_capacity(heads.len());
        for (pos, col) in heads {
            let line = text::line_of(&rope, pos);
            let span = match target {
                Target::Lines => Some(lines_span(&rope, line, line + n - 1)),
                Target::Search(_) => None,
                Target::Object(Obj::Match(forward), _) => {
                    self.match_object(&rope, pos, forward).map(|(a, z)| (a, z, false))
                }
                Target::Object(obj, around) => object_span(&rope, pos, obj, around, n).map(|(a, z)| match obj {
                    Obj::Para => {
                        let la = text::line_of(&rope, a);
                        let lz = text::line_of(&rope, z.saturating_sub(1).max(a));
                        lines_span(&rope, la, lz)
                    }
                    Obj::Pair(..) => exclusive_span(&rope, a, z),
                    _ => (a, z, false),
                }),
                Target::Motion(m) => {
                    let mut m = m;
                    if m == Motion::MatchPair && count.is_some() {
                        m = Motion::LinePercent;
                    }
                    // cw is ce (on a word)
                    if op == Op::Change
                        && let Motion::WordFwd(big) = m
                        && let Some(c) = text::char_at(&rope, pos).filter(|c| !c.is_whitespace())
                    {
                        let cls = |c: char| if big { text::big_class(c) } else { text::class(c) };
                        let at_end = text::char_at(&rope, pos + 1).is_none_or(|d| cls(d) != cls(c));
                        if at_end && n == 1 {
                            out.push((pos, text::grapheme_end(&rope, pos), false));
                            continue;
                        }
                        m = Motion::WordEnd(big);
                    }
                    if let Motion::WordFwd(big) = m {
                        // the last word moved over ends at its line end, not at the next line's first word
                        let mut prev = pos;
                        let mut t = pos;
                        for _ in 0..n {
                            prev = t;
                            t = motion::next_word_start(&rope, t, big);
                        }
                        let pl = text::line_of(&rope, prev);
                        if text::line_of(&rope, t) > pl && prev < text::line_end(&rope, pl) {
                            t = text::line_end(&rope, pl);
                        }
                        out.push(exclusive_span(&rope, pos, t));
                        continue;
                    }
                    let Some((t, _)) = self.motion_target(&rope, pos, m, count, col) else { continue };
                    match m.kind() {
                        Kind::Linewise => Some(lines_span(&rope, line, text::line_of(&rope, t))),
                        Kind::Inclusive => {
                            // an inclusive motion onto a line break (`D` on an empty line) takes nothing of it
                            let end = pos.max(t);
                            let end = if matches!(text::char_at(&rope, end), Some('\n') | None) {
                                end
                            } else {
                                text::grapheme_end(&rope, end)
                            };
                            Some((pos.min(t), end, false))
                        }
                        Kind::Exclusive => Some(exclusive_span(&rope, pos.min(t), pos.max(t))),
                    }
                }
            };
            if let Some(s) = span {
                out.push(s);
            }
        }
        out
    }

    /// Spans of the visual selection; `force_lines` widens them to whole lines.
    fn visual_spans(&self, force_lines: bool) -> Vec<(usize, usize, bool)> {
        let rope = &self.buf().doc.rope;
        let linewise = force_lines || self.mode == Mode::VisualLine;
        self.selection_spans()
            .into_iter()
            .map(|(a, z)| {
                if linewise {
                    let la = text::line_of(rope, a);
                    let lz = text::line_of(rope, z.saturating_sub(1).max(a));
                    lines_span(rope, la, lz)
                } else {
                    (a, z, false)
                }
            })
            .collect()
    }

    /// Text of the spans as a register (linewise parts are whole lines without the final break).
    fn register_of(&self, spans: &[(usize, usize, bool)], blockwise: bool) -> Register {
        let rope = &self.buf().doc.rope;
        let parts = spans
            .iter()
            .map(|&(a, z, lw)| {
                if lw {
                    let la = text::line_of(rope, a);
                    let lz = text::line_of(rope, z.saturating_sub(1).max(a));
                    rope.slice(text::line_start(rope, la)..text::line_end(rope, lz)).to_string()
                } else {
                    rope.slice(a..z).to_string()
                }
            })
            .collect();
        Register { parts, linewise: spans.iter().any(|s| s.2), blockwise }
    }

    /// The text of the visual block, one part per line, short lines included as empty parts.
    fn block_parts(&self) -> Option<Vec<String>> {
        let (anchor, head) = self.block?;
        let tw = self.settings.tab_width;
        let rope = &self.buf().doc.rope;
        let (la, lh) = (text::line_of(rope, anchor), text::line_of(rope, head));
        let (ca, ch) = (text::col_of(rope, anchor, tw), text::col_of(rope, head, tw));
        let (left, right) = (ca.min(ch), ca.max(ch));
        let right = if head == text::line_end(rope, lh) && text::line_len(rope, lh) > 0 { usize::MAX } else { right };
        let parts = (la.min(lh)..=la.max(lh))
            .map(|l| {
                let a = text::pos_at_col(rope, l, left, tw);
                let le = text::line_end(rope, l);
                if text::col_of(rope, a, tw) < left || a == le {
                    return String::new();
                }
                let h = text::grapheme_end(rope, text::clamp_normal(rope, text::pos_at_col(rope, l, right, tw)).max(a))
                    .min(le);
                rope.slice(a..h).to_string()
            })
            .collect();
        Some(parts)
    }

    /// Where vim leaves the cursor before an operator changes text (`[`-mark style): the start of a charwise span,
    /// the first line of a linewise one. Undo comes back here.
    fn place_op_cursor(&mut self, spans: &[(usize, usize, bool)], prim: usize) {
        let tw = self.settings.tab_width;
        let b = self.buf_mut();
        if b.sel.len() != spans.len() {
            return;
        }
        let rope = &b.doc.rope;
        let heads: Vec<usize> = b.sel.ranges().iter().map(|r| r.head).collect();
        let sel = Selection::new(
            spans.iter().zip(heads).map(|(&(a, _, lw), h)| {
                if lw {
                    let l = text::line_of(rope, a);
                    let col = text::col_of(rope, h, tw).min(text::col_of(rope, text::first_non_blank(rope, l), tw));
                    Range::point(text::pos_at_col(rope, l, col, tw))
                } else {
                    Range::point(a)
                }
            }),
            prim,
        );
        b.sel = sel;
    }

    /// Runs an operator on spans (one per cursor) and leaves the cursors where vim would. `levels` repeats `>`/`<`.
    fn apply_op(
        &mut self,
        op: Op,
        spans: Vec<(usize, usize, bool)>,
        reg: Option<char>,
        keep_col_on_yank: bool,
        levels: usize,
    ) {
        let was_block = self.mode == Mode::VisualBlock;
        if spans.is_empty() {
            if self.mode.is_visual() {
                self.set_mode(Mode::Normal);
            }
            return;
        }
        let rope = self.buf().doc.rope.clone();
        let len = rope.len_chars();
        let last = text::last_line(&rope);
        // block operations leave the cursor at the top-left corner
        let prim = if was_block { 0 } else { self.buf().sel.primary_index().min(spans.len() - 1) };
        let linewise = spans.iter().any(|s| s.2);
        if matches!(op, Op::Delete | Op::Change | Op::Yank) {
            let reg_val = match self.block_parts().filter(|_| was_block && !linewise) {
                Some(parts) => Register { parts, linewise: false, blockwise: true },
                None => self.register_of(&spans, was_block),
            };
            self.write_register(reg, reg_val, op == Op::Yank);
        }
        let opts = self.opts();
        match op {
            Op::Yank => {
                let heads: Vec<usize> = self.buf().sel.ranges().iter().map(|r| r.head).collect();
                self.set_mode(Mode::Normal);
                let sel = Selection::new(
                    spans.iter().zip(heads).map(|(&(a, _, lw), h)| {
                        if lw && keep_col_on_yank && text::line_of(&rope, a) == text::line_of(&rope, h) {
                            Range::point(h)
                        } else if lw {
                            let col = text::col_of(&rope, h, opts.tab_width);
                            Range::point(
                                text::clamp_normal(
                                    &rope,
                                    text::pos_at_col(&rope, text::line_of(&rope, a), col, opts.tab_width),
                                )
                                .min(h.max(a)),
                            )
                        } else {
                            Range::point(a)
                        }
                    }),
                    prim,
                );
                self.buf_mut().sel = sel;
                self.clamp_cursors();
            }
            Op::Delete => {
                let changes: Vec<Change> = spans
                    .iter()
                    .map(|&(a, z, lw)| {
                        // deleting through the last line takes the line break before it
                        let through_last = z > len || (z == len && text::line_len(&rope, last) > 0);
                        let z = z.min(len);
                        if lw && a > 0 && through_last { Change::delete(a - 1, z) } else { Change::delete(a, z) }
                    })
                    .collect();
                let cs = ChangeSet::new(changes.clone());
                let anchors: Vec<usize> = changes.iter().map(|c| cs.map(c.from, Assoc::Before)).collect();
                self.set_mode_raw(Mode::Normal);
                self.place_op_cursor(&spans, prim);
                self.buf_mut().apply(cs, Some(Selection::new(anchors.into_iter().map(Range::point), prim)));
                let b = self.buf_mut();
                let rope = &b.doc.rope;
                b.sel = b.sel.transform(|r| {
                    let p = if linewise { text::first_non_blank(rope, text::line_of(rope, r.head)) } else { r.head };
                    Range::point(text::clamp_normal(rope, p))
                });
            }
            Op::Change => {
                let changes: Vec<Change> = spans
                    .iter()
                    .map(|&(a, z, lw)| {
                        if lw {
                            let la = text::line_of(&rope, a);
                            let lz = text::line_of(&rope, z.saturating_sub(1).max(a));
                            let indent = if opts.auto_indent { text::indent_of(&rope, la) } else { String::new() };
                            Change::replace(text::line_start(&rope, la), text::line_end(&rope, lz), indent)
                        } else {
                            Change::delete(a, z)
                        }
                    })
                    .collect();
                let cs = ChangeSet::new(changes.clone());
                let carets: Vec<usize> =
                    changes.iter().map(|c| cs.map(c.from, Assoc::Before) + c.text.chars().count()).collect();
                self.set_mode_raw(Mode::Insert);
                self.place_op_cursor(&spans, prim);
                let first = carets[prim.min(carets.len() - 1)];
                self.buf_mut().apply(cs, Some(Selection::new(carets.into_iter().map(Range::point), prim)));
                self.mark_insert_start(first);
                self.block_insert = was_block;
            }
            Op::Indent | Op::Outdent if was_block && !linewise => {
                // a block shifts the text right of its left edge
                let w = opts.tab_width.max(1) * levels.max(1);
                let changes: Vec<Change> = spans
                    .iter()
                    .filter_map(|&(a, _, _)| {
                        if op == Op::Indent {
                            return Some(Change::insert(a, opts.indent.repeat(levels.max(1))));
                        }
                        let le = text::line_end(&rope, text::line_of(&rope, a));
                        let n = rope.slice(a..le).chars().take_while(|c| *c == ' ' || *c == '\t').take(w).count();
                        (n > 0).then(|| Change::delete(a, a + n))
                    })
                    .collect();
                let corner = spans[0].0;
                self.set_mode_raw(Mode::Normal);
                self.place_op_cursor(&spans, prim);
                self.buf_mut().apply(ChangeSet::new(changes), Some(Selection::point(corner)));
                self.clamp_cursors();
            }
            Op::Indent | Op::Outdent => {
                let mut lines = Vec::new();
                for &(a, z, _) in &spans {
                    let la = text::line_of(&rope, a);
                    let lz = text::line_of(&rope, z.saturating_sub(1).max(a));
                    lines.extend(la..=lz);
                }
                lines.sort_unstable();
                lines.dedup();
                self.set_mode_raw(Mode::Normal);
                self.place_op_cursor(&spans, prim);
                // vim re-indents: the width changes by a shiftwidth per level, written the 'expandtab' way
                let levels = levels.max(1) as isize;
                ops::shift_lines(self.buf_mut(), &lines, &opts, if op == Op::Indent { levels } else { -levels });
                let b = self.buf_mut();
                let rope = &b.doc.rope;
                let first: Vec<usize> = spans.iter().map(|s| text::line_of(rope, s.0.min(rope.len_chars()))).collect();
                b.sel = Selection::new(first.into_iter().map(|l| Range::point(text::first_non_blank(rope, l))), prim);
                self.clamp_cursors();
            }
            Op::Lower | Op::Upper | Op::ToggleCase => {
                let changes: Vec<Change> = spans
                    .iter()
                    .map(|&(a, z, _)| {
                        let z = z.min(len);
                        let s = rope.slice(a..z).to_string();
                        let t: String = match op {
                            Op::Lower => s.to_lowercase(),
                            Op::Upper => s.to_uppercase(),
                            _ => s
                                .chars()
                                .flat_map(|c| {
                                    if c.is_uppercase() {
                                        c.to_lowercase().collect::<Vec<_>>()
                                    } else {
                                        c.to_uppercase().collect()
                                    }
                                })
                                .collect(),
                        };
                        Change::replace(a, z, t)
                    })
                    .collect();
                let starts: Vec<usize> = spans.iter().map(|s| s.0).collect();
                self.set_mode_raw(Mode::Normal);
                self.place_op_cursor(&spans, prim);
                self.buf_mut()
                    .apply(ChangeSet::new(changes), Some(Selection::new(starts.into_iter().map(Range::point), prim)));
                self.clamp_cursors();
            }
        }
        if was_block && self.mode == Mode::Normal {
            // a block operation leaves one cursor, at its top-left corner
            let b = self.buf_mut();
            b.sel = b.sel.only_primary();
        }
    }

    /// Runs `f` with the active buffer's undo group held open, so everything it does is one undo step.
    pub(crate) fn held<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let id = self.buf().id;
        self.buf_mut().hold_history();
        let r = f(self);
        if let Some(b) = self.buffers.iter_mut().find(|b| b.id == id) {
            b.release_history();
        }
        r
    }

    /// `gn`: the search match under the cursor, else the next one (the previous one for `gN`).
    fn match_object(&self, rope: &Rope, pos: usize, forward: bool) -> Option<(usize, usize)> {
        let re = self.search.as_ref()?;
        let mut hits = Vec::new();
        crate::search::line_matches(rope, re, text::line_of(rope, pos), &mut hits);
        if let Some(&m) = hits.iter().find(|m| m.0 <= pos && pos < m.1) {
            return Some(m);
        }
        crate::search::find(rope, re, pos, forward).map(|(a, z, _)| (a, z))
    }

    /// Remembers where an insert session started (vim's `'[` mark): Ctrl+U stops there, and visual-block inserts
    /// put the cursor back there on Esc.
    pub(crate) fn mark_insert_start(&mut self, pos: usize) {
        // earlier edits move the old marks, not this new one
        self.map_marks();
        let id = self.buf().id;
        self.marks.insert((id, '['), pos);
    }

    /// `d/pat<CR>` and friends: runs `op` from the cursor to `target` (an exclusive motion).
    pub(crate) fn operate_to(&mut self, op: Op, reg: Option<char>, target: usize) {
        let rope = self.buf().doc.rope.clone();
        let pos = self.buf().sel.primary().head;
        if pos == target {
            return;
        }
        let span = exclusive_span(&rope, pos.min(target), pos.max(target));
        self.buf_mut().sel = Selection::point(pos);
        self.apply_op(op, vec![span], reg, false, 1);
        if self.mode != Mode::Insert {
            self.commit();
        }
    }

    /// Changes mode without converting the selection (the caller sets it).
    fn set_mode_raw(&mut self, mode: Mode) {
        if self.mode.is_visual() {
            self.last_visual = Some((self.mode, self.buf().sel.clone()));
        }
        self.block = None;
        self.mode = mode;
    }

    pub(crate) fn clamp_cursors(&mut self) {
        if self.mode != Mode::Normal {
            return;
        }
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        b.sel = b.sel.transform(|r| Range {
            anchor: text::clamp_normal(rope, r.head),
            head: text::clamp_normal(rope, r.head),
            col: r.col,
        });
    }

    fn enter_insert(&mut self, at: InsertAt) {
        let opts = self.opts();
        if self.mode == Mode::VisualBlock || self.mode == Mode::Visual || self.mode == Mode::VisualLine {
            // I / A on a visual selection: a caret at the start (or past the end) of every range
            let spans = self.selection_spans();
            let prim = self.buf().sel.primary_index().min(spans.len().saturating_sub(1));
            let block = self.mode == Mode::VisualBlock;
            let rope = self.buf().doc.rope.clone();
            let carets = spans.iter().map(|&(a, z)| {
                let p = if at == InsertAt::Before {
                    a
                } else if block {
                    z.min(text::line_end(&rope, text::line_of(&rope, a)))
                } else {
                    z
                };
                Range::point(p)
            });
            let sel = if block { Selection::new(carets, 0) } else { Selection::new(carets, prim) };
            self.block_insert = block;
            self.set_mode_raw(Mode::Insert);
            self.buf_mut().sel = sel;
            // Esc puts the cursor back on the top-left corner of the block
            let corner = spans.first().map_or(0, |s| s.0);
            self.mark_insert_start(corner);
            return;
        }
        self.set_mode_raw(Mode::Insert);
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        match at {
            InsertAt::Before => {}
            InsertAt::After => {
                b.sel = b.sel.transform(|r| {
                    Range::point(
                        text::next_grapheme(rope, r.head).min(text::line_end(rope, text::line_of(rope, r.head))),
                    )
                });
            }
            InsertAt::LineStart => {
                b.sel = b.sel.transform(|r| Range::point(text::first_non_blank(rope, text::line_of(rope, r.head))))
            }
            InsertAt::LineStartCol0 => {
                b.sel = b.sel.transform(|r| Range::point(text::line_start(rope, text::line_of(rope, r.head))))
            }
            InsertAt::LineEnd => {
                b.sel = b.sel.transform(|r| Range::point(text::line_end(rope, text::line_of(rope, r.head))))
            }
            InsertAt::Below => ops::open_line(b, true, &opts),
            InsertAt::Above => ops::open_line(b, false, &opts),
        }
        let h = self.buf().sel.primary().head;
        self.mark_insert_start(h);
    }

    /// `[count]i...<Esc>` and friends: types the inserted keys `count - 1` more times, like vim. Called when the
    /// insert session ends, with the keys that started it (recorded for `.`).
    pub(crate) fn repeat_insert(&mut self, keys: &[KeyPress]) {
        let Some((parsed, used)) = (1..=keys.len()).find_map(|k| match parse(&keys[..k], Mode::Normal, false) {
            Parse::Done(p) => Some((p, k)),
            _ => None,
        }) else {
            return;
        };
        let Cmd::Insert(at) = parsed.cmd else { return };
        let n = parsed.count.unwrap_or(1).min(10_000);
        if n <= 1 {
            return;
        }
        let mut body = &keys[used..];
        if let Some(last) = body.last()
            && (last.key == Key::Esc || matches!(ctrl(last), Some('[' | 'c')))
        {
            body = &body[..body.len() - 1];
        }
        let opts = self.opts();
        for _ in 1..n {
            // the copies of `O` go below the first one, like vim
            if matches!(at, InsertAt::Below | InsertAt::Above) {
                ops::open_line(self.buf_mut(), true, &opts);
            }
            for &k in body {
                self.insert_key(k);
            }
        }
    }

    fn paste(&mut self, reg: Option<char>, after: bool, count: usize) {
        let r = match self.read_register(reg) {
            Some(r) => r,
            // over a selection an empty register still deletes it, like vim
            None if self.mode.is_visual() => Register::default(),
            None => {
                self.error("register is empty");
                return;
            }
        };
        if self.mode.is_visual() {
            return self.paste_over_selection(r, reg, after, count);
        }
        self.put(r, after, count);
    }

    /// `p`/`P` in visual mode: the selection is replaced by the register. `p` puts the replaced text in the unnamed
    /// register (vim), `P` leaves the registers alone.
    fn paste_over_selection(&mut self, r: Register, reg: Option<char>, after: bool, count: usize) {
        let block = self.mode == Mode::VisualBlock;
        let line_sel = self.mode == Mode::VisualLine;
        let spans = self.visual_spans(false);
        if spans.is_empty() {
            return;
        }
        let replaced = self.register_of(&spans, block);
        let rope = self.buf().doc.rope.clone();
        let per_span = r.parts.len() == spans.len() && spans.len() > 1;
        let joined = r.parts.join("\n");
        let mut changes = Vec::with_capacity(spans.len());
        let mut cursor_at = Vec::with_capacity(spans.len());
        for (i, &(a, z, _)) in spans.iter().enumerate() {
            let part = if per_span { r.parts[i].as_str() } else { joined.as_str() };
            let body = vec![part; count.max(1)].join(if r.linewise { "\n" } else { "" });
            if line_sel {
                // whole lines are replaced; charwise text becomes its own line(s)
                let la = text::line_of(&rope, a);
                let lz = text::line_of(&rope, z.saturating_sub(1).max(a));
                let ls = text::line_start(&rope, la);
                changes.push(Change::replace(ls, text::line_end(&rope, lz), body.clone()));
                let lead = body.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                cursor_at.push((changes.len() - 1, if r.linewise { lead } else { 0 }));
            } else if r.linewise && !block {
                // lines into a charwise selection: they go on lines of their own between the two halves
                changes.push(Change::replace(a, z, format!("\n{body}\n")));
                cursor_at.push((changes.len() - 1, 1));
            } else {
                let n = body.chars().count();
                let multi_line = body.contains('\n');
                changes.push(Change::replace(a, z, body));
                cursor_at.push((changes.len() - 1, if multi_line || block { 0 } else { n.saturating_sub(1) }));
            }
        }
        let cs = ChangeSet::new(changes.clone());
        let carets: Vec<usize> =
            cursor_at.iter().map(|&(i, off)| cs.map(changes[i].from, Assoc::Before) + off).collect();
        let prim = if block { 0 } else { self.buf().sel.primary_index().min(carets.len() - 1) };
        self.set_mode_raw(Mode::Normal);
        self.place_op_cursor(&spans, prim);
        self.buf_mut().apply(cs, Some(Selection::new(carets.into_iter().map(Range::point), prim)));
        self.clamp_cursors();
        if after && reg != Some('_') {
            self.write_register(None, replaced, false);
        }
    }

    fn put(&mut self, r: Register, after: bool, count: usize) {
        let n = self.buf().sel.len();
        let per_cursor = r.parts.len() == n && n > 1;
        if r.blockwise && !per_cursor && !r.linewise {
            return self.put_block(&r, after, count);
        }
        let joined = r.parts.join("\n");
        let mut i = 0;
        let linewise = r.linewise;
        ops::edit_each(self.buf_mut(), |rope, range| {
            let part = if per_cursor { &r.parts[i] } else { &joined };
            i += 1;
            let pos = range.head;
            if linewise {
                let block = vec![part.as_str(); count].join("\n");
                let line = text::line_of(rope, pos);
                let lead = block.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                if after {
                    ops::Edit::Change(
                        Change::insert(text::line_end(rope, line), format!("\n{block}")),
                        1 + lead,
                        1 + lead,
                    )
                } else {
                    ops::Edit::Change(Change::insert(text::line_start(rope, line), format!("{block}\n")), lead, lead)
                }
            } else {
                let s = part.repeat(count);
                let le = text::line_end(rope, text::line_of(rope, pos));
                let at = if after && pos < le { text::next_grapheme(rope, pos) } else { pos };
                // the cursor ends on the last char put, or on the first when the text spans lines
                let n = if s.contains('\n') { 0 } else { s.chars().count().saturating_sub(1) };
                ops::Edit::Change(Change::insert(at, s), n, n)
            }
        });
        self.set_mode_raw(Mode::Normal);
        self.clamp_cursors();
    }

    /// Puts a blockwise register as a block: one part per line from the cursor down, at the cursor's column.
    fn put_block(&mut self, r: &Register, after: bool, count: usize) {
        let tw = self.settings.tab_width;
        let rope = self.buf().doc.rope.clone();
        let head = self.buf().sel.primary().head;
        let line = text::line_of(&rope, head);
        let last = text::last_line(&rope);
        let mut col = text::col_of(&rope, head, tw);
        if after && text::line_len(&rope, line) > 0 {
            col = text::col_of(&rope, text::next_grapheme(&rope, head), tw);
        }
        let width =
            r.parts.iter().map(|p| p.chars().map(|c| text::char_width(c, 0, tw)).sum::<usize>()).max().unwrap_or(0);
        let mut changes = Vec::new();
        let mut tail = String::new();
        for (i, part) in r.parts.iter().enumerate() {
            let part_w: usize = part.chars().map(|c| text::char_width(c, 0, tw)).sum();
            let one = part.clone() + &" ".repeat(width - part_w.min(width));
            let body = one.repeat(count.max(1));
            let l = line + i;
            if l > last {
                tail.push('\n');
                tail.push_str(&" ".repeat(col));
                tail.push_str(body.trim_end_matches(' '));
                continue;
            }
            let le = text::line_end(&rope, l);
            let line_w = text::col_of(&rope, le, tw);
            if line_w < col {
                changes.push(Change::insert(le, format!("{}{}", " ".repeat(col - line_w), body.trim_end_matches(' '))));
            } else {
                let at = text::pos_at_col(&rope, l, col, tw);
                let body = if at == le { body.trim_end_matches(' ').to_string() } else { body };
                changes.push(Change::insert(at, body));
            }
        }
        if !tail.is_empty() {
            changes.push(Change::insert(rope.len_chars(), tail));
        }
        let cs = ChangeSet::new(changes);
        let corner = text::pos_at_col(&rope, line, col, tw);
        let cursor = cs.map(corner, Assoc::Before);
        self.set_mode_raw(Mode::Normal);
        self.buf_mut().apply(cs, Some(Selection::point(cursor)));
        self.clamp_cursors();
    }

    fn replace_chars(&mut self, c: char, count: usize) {
        let spans: Vec<(usize, usize)> = if self.mode.is_visual() {
            self.selection_spans()
        } else {
            let rope = &self.buf().doc.rope;
            let mut v = Vec::new();
            for r in self.buf().sel.ranges() {
                let le = text::line_end(rope, text::line_of(rope, r.head));
                let mut z = r.head;
                for _ in 0..count {
                    if z >= le {
                        z = usize::MAX;
                        break;
                    }
                    z = text::next_grapheme(rope, z);
                }
                if z <= le {
                    v.push((r.head, z));
                }
            }
            if v.is_empty() {
                return;
            }
            v
        };
        let rope = self.buf().doc.rope.clone();
        let visual = self.mode.is_visual();
        let opts = self.opts();
        let changes: Vec<Change> = spans
            .iter()
            .map(|&(a, z)| {
                if c == '\n' && !visual {
                    // r<CR>: the chars become one line break
                    let indent =
                        if opts.auto_indent { text::indent_of(&rope, text::line_of(&rope, a)) } else { String::new() };
                    return Change::replace(a, z, format!("\n{indent}"));
                }
                // one replacement char per grapheme, line breaks stay
                let mut s = String::new();
                let mut p = a;
                while p < z {
                    let ch = rope.char(p);
                    s.push(if ch == '\n' { '\n' } else { c });
                    p = text::next_grapheme(&rope, p);
                }
                Change::replace(a, z, s)
            })
            .collect();
        let cs = ChangeSet::new(changes.clone());
        let heads: Vec<usize> = changes
            .iter()
            .map(|ch| {
                let start = cs.map(ch.from, Assoc::Before);
                let n = ch.text.chars().count();
                if visual {
                    start
                } else if c == '\n' {
                    start + n
                } else {
                    start + n - 1
                }
            })
            .collect();
        let prim = if self.mode == Mode::VisualBlock { 0 } else { self.buf().sel.primary_index().min(heads.len() - 1) };
        self.set_mode_raw(Mode::Normal);
        self.buf_mut().apply(cs, Some(Selection::new(heads.into_iter().map(Range::point), prim)));
        self.clamp_cursors();
    }

    fn increment(&mut self, delta: i64) {
        let rope = self.buf().doc.rope.clone();
        let mut changes = Vec::new();
        let mut carets = Vec::new();
        for r in self.buf().sel.ranges() {
            let line = text::line_of(&rope, r.head);
            let ls = text::line_start(&rope, line);
            let s = text::line_text(&rope, line);
            let chars: Vec<char> = s.chars().collect();
            let mut i = r.head - ls;
            // the number under or after the cursor
            while i > 0 && chars.get(i).is_some_and(|c| c.is_ascii_digit()) && chars[i - 1].is_ascii_digit() {
                i -= 1;
            }
            let Some(start) = (i..chars.len()).find(|&j| chars[j].is_ascii_digit()) else { continue };
            let mut end = start;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            let neg = start > 0 && chars[start - 1] == '-';
            let from = if neg { start - 1 } else { start };
            let digits: String = chars[start..end].iter().collect();
            let Ok(v) = digits.parse::<i64>() else { continue };
            let v = if neg { -v } else { v };
            let new = (v.saturating_add(delta)).to_string();
            let n = new.chars().count();
            changes.push(Change::replace(ls + from, ls + end, new));
            carets.push((ls + from, n));
        }
        if changes.is_empty() {
            return;
        }
        let cs = ChangeSet::new(changes);
        let prim = self.buf().sel.primary_index().min(carets.len() - 1);
        let sel = Selection::new(carets.iter().map(|&(a, n)| Range::point(cs.map(a, Assoc::Before) + n - 1)), prim);
        self.buf_mut().apply(cs, Some(sel));
    }

    fn do_scroll(&mut self, s: ScrollKind, count: Option<usize>) {
        let tw = self.settings.tab_width;
        let b = self.buf_mut();
        let rows = b.view.rows.max(2);
        let last = text::last_line(&b.doc.rope);
        let line = b.cursor_line();
        let (view_delta, cursor_delta): (isize, isize) = match s {
            ScrollKind::HalfDown => {
                let n = count.unwrap_or(rows / 2) as isize;
                (n, n)
            }
            ScrollKind::HalfUp => {
                let n = count.unwrap_or(rows / 2) as isize;
                (-n, -n)
            }
            ScrollKind::PageDown => ((rows - 2) as isize, (rows - 2) as isize),
            ScrollKind::PageUp => (-((rows - 2) as isize), -((rows - 2) as isize)),
            ScrollKind::LineDown => (count.unwrap_or(1) as isize, 0),
            ScrollKind::LineUp => (-(count.unwrap_or(1) as isize), 0),
            ScrollKind::Center => {
                b.view.top = line.saturating_sub(rows / 2);
                return;
            }
            ScrollKind::Top => {
                b.view.top = line;
                return;
            }
            ScrollKind::Bottom => {
                b.view.top = (line + 1).saturating_sub(rows);
                return;
            }
        };
        b.view.top = (b.view.top as isize + view_delta).clamp(0, last as isize) as usize;
        let mut target = (line as isize + cursor_delta).clamp(0, last as isize) as usize;
        // keep the cursor on screen when only the view moved
        target = target.clamp(b.view.top, (b.view.top + rows - 1).min(last));
        if target != line {
            let rope = &b.doc.rope;
            let r = b.sel.primary();
            let (p, c) = vertical(rope, r, target as isize - line as isize, tw);
            let mut nr = Range::point(text::clamp_normal(rope, p));
            nr.col = Some(c);
            b.sel = Selection::single(nr);
        }
    }
}

/// Text object span `[from, to)` at `pos`, `count` levels/words/paragraphs wide.
fn object_span(rope: &Rope, pos: usize, obj: Obj, around: bool, count: usize) -> Option<(usize, usize)> {
    match obj {
        Obj::Word(big) => Some(motion::word_object_n(rope, pos, big, around, count)),
        Obj::Pair(o, c) => motion::pair_object_n(rope, pos, o, c, around, count),
        Obj::Quote(q) => motion::quote_object(rope, pos, q, around),
        Obj::Para => Some(motion::paragraph_object_n(rope, pos, around, count)),
        Obj::Match(_) => None, // needs the search pattern: see Editor::match_object
    }
}

/// Whole lines `a..=b`: from the start of `a` to the start of the line after `b` (or the end of the text). When
/// they include an empty last line the end is one past the text, so it differs from stopping just before it.
fn lines_span(rope: &Rope, a: usize, b: usize) -> (usize, usize, bool) {
    let last = text::last_line(rope);
    let (a, b) = (a.min(b), a.max(b).min(last));
    let end = if b < last {
        text::line_start(rope, b + 1)
    } else {
        rope.len_chars() + usize::from(text::line_len(rope, last) == 0)
    };
    (text::line_start(rope, a), end, true)
}

/// An exclusive span `[a, z)` with vim's two adjustments for one that ends in column 0 of a later line
/// (`:h exclusive-linewise`): it becomes linewise when it starts at or before the first non-blank of its line,
/// otherwise it stops at the end of the previous line.
fn exclusive_span(rope: &Rope, a: usize, z: usize) -> (usize, usize, bool) {
    let (la, lz) = (text::line_of(rope, a), text::line_of(rope, z));
    if z > a && lz > la && z == text::line_start(rope, lz) {
        if a <= text::first_non_blank(rope, la) {
            return lines_span(rope, la, lz - 1);
        }
        return (a, text::line_end(rope, lz - 1), false);
    }
    (a, z, false)
}
