//! Vim normal and visual mode: a small parser that turns pending keys into a command, and its execution on every
//! cursor at once.

use crate::change::{Assoc, Change, ChangeSet};
use crate::editor::{Editor, Mode, PromptKind, Register, vertical};
use crate::input::{Key, KeyPress};
use crate::motion;
use crate::ops;
use crate::selection::{Range, Selection};
use crate::text::{self, CharClass};
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
    Find { ch: char, forward: bool, till: bool },
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
            Up | Down | NextLineStart | PrevLineStart | FileStart | FileEnd | ScreenTop | ScreenMiddle | ScreenBottom => Kind::Linewise,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Motion(Motion),
    Object(Obj, bool),
    Lines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cmd {
    Move(Motion),
    Operate(Op, Target),
    /// An operator applied to the visual selection; `lines` forces linewise (`D`, `X`, `Y`, `C`, `S`, `R`).
    VisualOp(Op, bool),
    VisualObject(Obj, bool),
    Insert(InsertAt),
    Paste { after: bool },
    Undo,
    Redo,
    Repeat,
    Join,
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
            Cmd::Operate(Op::Delete | Op::Change | Op::Indent | Op::Outdent | Op::Lower | Op::Upper | Op::ToggleCase, _)
                | Cmd::Insert(_)
                | Cmd::Paste { .. }
                | Cmd::Join
                | Cmd::Replace(_)
                | Cmd::ToggleCaseChar
                | Cmd::Increment(_)
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
    Done(Motion, usize),
    Pending,
    No,
}

#[inline]
fn ch(k: &KeyPress) -> Option<char> {
    k.text()
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
                    'p' => MotionParse::Done(Up, i + 1),
                    'j' => MotionParse::Done(Down, i + 1),
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
                        Some('g') => MotionParse::Done(FileStart, i + 2),
                        Some('e') => MotionParse::Done(WordEndBack(false), i + 2),
                        Some('E') => MotionParse::Done(WordEndBack(true), i + 2),
                        Some('_') => MotionParse::Done(LastNonBlank, i + 2),
                        Some('j') => MotionParse::Done(Down, i + 2),
                        Some('k') => MotionParse::Done(Up, i + 2),
                        Some('0') => MotionParse::Done(LineStart, i + 2),
                        Some('$') => MotionParse::Done(LineEnd, i + 2),
                        _ => MotionParse::No,
                    };
                }
                'f' | 'F' | 't' | 'T' => {
                    let Some(n) = next else { return MotionParse::Pending };
                    let Some(target) = ch(n) else { return MotionParse::No };
                    return MotionParse::Done(Find { ch: target, forward: c == 'f' || c == 't', till: c == 't' || c == 'T' }, i + 2);
                }
                '`' | '\'' => {
                    let Some(n) = next else { return MotionParse::Pending };
                    let Some(mark) = ch(n) else { return MotionParse::No };
                    return MotionParse::Done(Mark(mark, c == '\''), i + 2);
                }
                _ => return MotionParse::No,
            }
        }
    };
    MotionParse::Done(m, i + 1)
}

fn parse_object(keys: &[KeyPress], i: usize) -> Option<Result<(Obj, bool), ()>> {
    let around = match keys.get(i).and_then(ch)? {
        'i' => false,
        'a' => true,
        _ => return Some(Err(())),
    };
    let Some(c) = keys.get(i + 1) else { return None };
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
        match parse_object(keys, j) {
            None => return Parse::Pending,
            Some(Ok((obj, around))) => return done(Operate(op, Target::Object(obj, around))),
            Some(Err(())) => {}
        }
        return match parse_motion(keys, j) {
            MotionParse::Done(m, _) => done(Operate(op, Target::Motion(m))),
            MotionParse::Pending => Parse::Pending,
            MotionParse::No => Parse::Invalid,
        };
    }

    if visual {
        if let Some(c) = ch(k) {
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
    }

    match parse_motion(keys, i) {
        MotionParse::Done(m, _) => return done(Move(m)),
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
            Key::Delete => done(if visual { VisualOp(Op::Delete, false) } else { Operate(Op::Delete, Target::Motion(Motion::Right)) }),
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
        'J' => done(Join),
        '~' => done(ToggleCaseChar),
        'v' => done(Visual(Mode::Visual)),
        'V' => done(Visual(Mode::VisualLine)),
        ':' => done(Prompt(PromptKind::Command)),
        '/' => done(Prompt(PromptKind::Search { forward: true })),
        '?' => done(Prompt(PromptKind::Search { forward: false })),
        'r' => need_char(Replace),
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
            Some('J') => done(Join),
            Some('I') => done(Insert(InsertAt::LineStartCol0)),
            _ => Parse::Invalid,
        },
        _ => Parse::Invalid,
    }
}

impl Editor {
    pub(crate) fn vim_key(&mut self, k: KeyPress) {
        self.pending.push(k);
        let keys = self.pending.clone();
        match parse(&keys, self.mode, self.macro_rec.is_some()) {
            Parse::Pending => {}
            Parse::Invalid => self.pending.clear(),
            Parse::Done(p) => {
                self.pending.clear();
                let record = p.cmd.is_change() && !self.replaying && self.mode == Mode::Normal;
                if record {
                    self.dot_recording = Some(keys);
                }
                self.exec(p);
                if self.mode != Mode::Insert {
                    if record {
                        if let Some(rec) = self.dot_recording.take() {
                            self.dot = rec;
                        }
                    }
                    self.commit();
                }
            }
        }
    }

    fn exec(&mut self, p: Parsed) {
        use Cmd::*;
        let count = p.count.unwrap_or(1);
        match p.cmd {
            Move(m) => self.do_move(m, p.count),
            Operate(op, target) => {
                let spans = self.op_spans(op, target, p.count);
                self.apply_op(op, spans, p.reg, matches!(target, Target::Motion(Motion::Up | Motion::Down | Motion::FileStart)));
            }
            VisualOp(op, lines) => {
                let spans = self.visual_spans(lines);
                let rept = if matches!(op, Op::Indent | Op::Outdent) { count } else { 1 };
                for _ in 0..rept {
                    self.apply_op(op, spans.clone(), p.reg, false);
                }
            }
            VisualObject(obj, around) => {
                let b = self.buf();
                let rope = &b.doc.rope;
                let ranges: Vec<Range> = b
                    .sel
                    .ranges()
                    .iter()
                    .map(|r| match object_span(rope, r.head, obj, around) {
                        Some((a, z)) if z > a => Range::new(a.min(r.anchor), z - 1),
                        _ => *r,
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
                let mut keys = self.dot.clone();
                if let Some(n) = p.count {
                    let (_, skip) = parse_count(&keys, 0);
                    keys.drain(..skip);
                    let digits: Vec<KeyPress> = n.to_string().chars().map(KeyPress::ch).collect();
                    keys.splice(0..0, digits);
                }
                self.replaying = true;
                for k in keys {
                    self.handle_key(k);
                }
                self.replaying = false;
                if self.mode == Mode::Insert {
                    self.set_mode(Mode::Normal);
                }
            }
            Join => {
                let spans = if self.mode.is_visual() { self.visual_spans(true) } else { Vec::new() };
                let b = self.buf();
                let rope = &b.doc.rope;
                let lines: Vec<usize> = if spans.is_empty() {
                    let l = b.cursor_line();
                    (l..=(l + count.max(2) - 1).min(text::last_line(rope))).collect()
                } else {
                    let mut v = Vec::new();
                    for (a, z, _) in spans {
                        let la = text::line_of(rope, a);
                        let lz = text::line_of(rope, z.saturating_sub(1)).max(la + 1).min(text::last_line(rope));
                        v.extend(la..=lz);
                    }
                    v
                };
                // the cursor lands on the first joint, like vim
                let joint = lines.first().map(|&l| {
                    let rope = &self.buf().doc.rope;
                    let ls = text::line_start(rope, l);
                    let mut e = text::line_end(rope, l);
                    while e > ls && matches!(rope.char(e - 1), ' ' | '\t') {
                        e -= 1;
                    }
                    e
                });
                ops::join_lines(self.buf_mut(), &lines);
                self.set_mode_raw(Mode::Normal);
                if let Some(j) = joint {
                    self.buf_mut().sel = Selection::point(j);
                }
                self.clamp_cursors();
            }
            Replace(c) => self.replace_chars(c, count),
            ToggleCaseChar => {
                let spans = self.op_spans(Op::ToggleCase, Target::Motion(Motion::Right), Some(count));
                self.apply_op(Op::ToggleCase, spans, None, false);
                let b = self.buf_mut();
                let rope = &b.doc.rope;
                b.sel = b.sel.transform(|r| Range::point(text::clamp_normal(rope, (r.head + count).min(text::line_end(rope, text::line_of(rope, r.head))))));
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
            Prompt(kind) => self.open_prompt(kind),
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
                let Some(keys) = self.macros.get(&r).cloned() else { return };
                for _ in 0..count.min(10_000) {
                    for k in &keys {
                        self.handle_key(*k);
                    }
                }
            }
            SetMark(c) => {
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
            NextBuffer(fwd) => self.run(if fwd { crate::commands::Command::NextBuffer } else { crate::commands::Command::PrevBuffer }),
        }
    }

    /// Where `m` takes a cursor at `pos`, or None when it cannot move (e.g. `f` without a match).
    fn motion_target(&mut self, rope: &Rope, pos: usize, m: Motion, count: Option<usize>, sticky: Option<usize>) -> Option<(usize, Option<usize>)> {
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
            Left => pos.saturating_sub(n).max(text::line_start(rope, line)),
            Right => (pos + n).min(text::line_end(rope, line)),
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
                let l = (line + n - 1).min(last);
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
            MatchPair => motion::match_pair_motion(rope, pos)?,
            ParaFwd => repeat(pos, &|p| motion::paragraph_forward(rope, p)),
            ParaBack => repeat(pos, &|p| motion::paragraph_backward(rope, p)),
            ScreenTop => text::first_non_blank(rope, (view.top + if view.top == 0 { 0 } else { so }).max(view.top + n - 1).min(last)),
            ScreenMiddle => text::first_non_blank(rope, (view.top + view.rows / 2).min(last).min(view.top + (last - view.top) / 2)),
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
                let mut p = if forward { pos } else { a };
                for _ in 0..n {
                    p = crate::search::find(rope, &re, p, forward)?.0;
                }
                p
            }
            Mark(c, linewise) => {
                let id = self.buf().id;
                let p = (*self.marks.get(&(id, c))?).min(len);
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
            if let Some((t, _)) = self.motion_target(&rope, head, m, count, None) {
                self.block = Some((anchor, text::clamp_normal(&rope, t)));
                self.rebuild_block();
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
            let t = text::clamp_normal(&rope, t);
            let mut nr = if mode.is_visual() { Range::new(r.anchor, t) } else { Range::point(t) };
            nr.col = col;
            out.push(nr);
        }
        if moved {
            self.buf_mut().sel = Selection::new(out, prim);
            if matches!(m, Motion::SearchNext(_) | Motion::SearchWord(_) | Motion::FileStart | Motion::FileEnd | Motion::Mark(..) | Motion::MatchPair) {
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
        let len = rope.len_chars();
        let last = text::last_line(&rope);
        let n = count.unwrap_or(1);
        let heads: Vec<(usize, Option<usize>)> = self.buf().sel.ranges().iter().map(|r| (r.head, r.col)).collect();
        let lines_span = |a: usize, b: usize| {
            let (a, b) = (a.min(b), a.max(b).min(last));
            let end = if b < last { text::line_start(&rope, b + 1) } else { len };
            (text::line_start(&rope, a), end, true)
        };
        let mut out = Vec::with_capacity(heads.len());
        for (pos, col) in heads {
            let line = text::line_of(&rope, pos);
            let span = match target {
                Target::Lines => Some(lines_span(line, line + n - 1)),
                Target::Object(obj, around) => object_span(&rope, pos, obj, around).map(|(a, z)| {
                    if obj == Obj::Para {
                        let la = text::line_of(&rope, a);
                        let lz = text::line_of(&rope, z.saturating_sub(1).max(a));
                        lines_span(la, lz)
                    } else {
                        (a, z, false)
                    }
                }),
                Target::Motion(m) => {
                    let mut m = m;
                    // cw is ce (on a word)
                    if op == Op::Change {
                        if let Motion::WordFwd(big) = m {
                            if let Some(c) = text::char_at(&rope, pos).filter(|c| !c.is_whitespace()) {
                                let cls = |c: char| if big { text::big_class(c) } else { text::class(c) };
                                let at_end = text::char_at(&rope, pos + 1).is_none_or(|d| cls(d) != cls(c));
                                if at_end && n == 1 {
                                    out.push((pos, pos + 1, false));
                                    continue;
                                }
                                m = Motion::WordEnd(big);
                            }
                        }
                    }
                    let Some((t, _)) = self.motion_target(&rope, pos, m, count, col) else { continue };
                    match m.kind() {
                        Kind::Linewise => Some(lines_span(line, text::line_of(&rope, t))),
                        Kind::Inclusive => Some((pos.min(t), (pos.max(t) + 1).min(len), false)),
                        Kind::Exclusive => {
                            let mut t = t;
                            if matches!(m, Motion::WordFwd(_)) && text::line_of(&rope, t) > line {
                                // dw on the last word of a line stops at the line end
                                let le = text::line_end(&rope, line);
                                if le > pos {
                                    t = le;
                                }
                            }
                            Some((pos.min(t), pos.max(t), false))
                        }
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
        let len = rope.len_chars();
        let last = text::last_line(rope);
        let linewise = force_lines || self.mode == Mode::VisualLine;
        self.selection_spans()
            .into_iter()
            .map(|(a, z)| {
                if linewise {
                    let la = text::line_of(rope, a);
                    let lz = text::line_of(rope, z.saturating_sub(1).max(a));
                    let end = if lz < last { text::line_start(rope, lz + 1) } else { len };
                    (text::line_start(rope, la), end, true)
                } else {
                    (a, z, false)
                }
            })
            .collect()
    }

    /// Runs an operator on spans (one per cursor) and leaves the cursors where vim would.
    fn apply_op(&mut self, op: Op, spans: Vec<(usize, usize, bool)>, reg: Option<char>, keep_col_on_yank: bool) {
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
        let prim = self.buf().sel.primary_index().min(spans.len() - 1);
        let linewise = spans.iter().any(|s| s.2);
        let text_of = |&(a, z, lw): &(usize, usize, bool)| {
            if lw {
                let la = text::line_of(&rope, a);
                let lz = text::line_of(&rope, z.saturating_sub(1).max(a));
                rope.slice(text::line_start(&rope, la)..text::line_end(&rope, lz)).to_string()
            } else {
                rope.slice(a..z).to_string()
            }
        };
        if matches!(op, Op::Delete | Op::Change | Op::Yank) {
            let reg_val = Register { parts: spans.iter().map(text_of).collect(), linewise };
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
                            Range::point(text::clamp_normal(&rope, text::pos_at_col(&rope, text::line_of(&rope, a), col, opts.tab_width)).min(h.max(a)))
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
                        if lw && z == len && a > 0 && text::line_of(&rope, a) <= last { Change::delete(a - 1, z) } else { Change::delete(a, z) }
                    })
                    .collect();
                let cs = ChangeSet::new(changes.clone());
                let anchors: Vec<usize> = changes.iter().map(|c| cs.map(c.from, Assoc::Before)).collect();
                self.set_mode_raw(Mode::Normal);
                self.buf_mut().apply(cs, Some(Selection::new(anchors.into_iter().map(Range::point), prim)));
                let b = self.buf_mut();
                let rope = &b.doc.rope;
                b.sel = b.sel.transform(|r| {
                    let p = if linewise {
                        let l = text::line_of(rope, r.head);
                        let l = if r.head > 0 && r.head == rope.len_chars() && rope.char(r.head - 1) == '\n' { l } else { l };
                        let p = text::line_start(rope, l);
                        let p = if p > 0 && p >= rope.len_chars() { text::line_start(rope, text::line_of(rope, p - 1)) } else { p };
                        text::first_non_blank(rope, text::line_of(rope, p))
                    } else {
                        r.head
                    };
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
                self.buf_mut().apply(cs, Some(Selection::new(carets.into_iter().map(Range::point), prim)));
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
                if op == Op::Indent {
                    ops::indent_lines(self.buf_mut(), &lines, &opts);
                } else {
                    ops::outdent_lines(self.buf_mut(), &lines, &opts);
                }
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
                        let s = rope.slice(a..z).to_string();
                        let t: String = match op {
                            Op::Lower => s.to_lowercase(),
                            Op::Upper => s.to_uppercase(),
                            _ => s.chars().flat_map(|c| if c.is_uppercase() { c.to_lowercase().collect::<Vec<_>>() } else { c.to_uppercase().collect() }).collect(),
                        };
                        Change::replace(a, z, t)
                    })
                    .collect();
                let starts: Vec<usize> = spans.iter().map(|s| s.0).collect();
                self.set_mode_raw(Mode::Normal);
                self.buf_mut().apply(ChangeSet::new(changes), Some(Selection::new(starts.into_iter().map(Range::point), prim)));
                self.clamp_cursors();
            }
        }
        let _ = was_block;
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
        b.sel = b.sel.transform(|r| Range { anchor: text::clamp_normal(rope, r.head), head: text::clamp_normal(rope, r.head), col: r.col });
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
                let p = if at == InsertAt::Before { a } else if block { z.min(text::line_end(&rope, text::line_of(&rope, a))) } else { z };
                Range::point(p)
            });
            let sel = if block { Selection::new(carets, 0) } else { Selection::new(carets, prim) };
            self.block_insert = block;
            self.set_mode_raw(Mode::Insert);
            self.buf_mut().sel = sel;
            return;
        }
        self.set_mode_raw(Mode::Insert);
        let b = self.buf_mut();
        let rope = &b.doc.rope;
        match at {
            InsertAt::Before => {}
            InsertAt::After => {
                b.sel = b.sel.transform(|r| Range::point((r.head + 1).min(text::line_end(rope, text::line_of(rope, r.head)))));
            }
            InsertAt::LineStart => b.sel = b.sel.transform(|r| Range::point(text::first_non_blank(rope, text::line_of(rope, r.head)))),
            InsertAt::LineStartCol0 => b.sel = b.sel.transform(|r| Range::point(text::line_start(rope, text::line_of(rope, r.head)))),
            InsertAt::LineEnd => b.sel = b.sel.transform(|r| Range::point(text::line_end(rope, text::line_of(rope, r.head)))),
            InsertAt::Below => ops::open_line(b, true, &opts),
            InsertAt::Above => ops::open_line(b, false, &opts),
        }
    }

    fn paste(&mut self, reg: Option<char>, after: bool, count: usize) {
        let Some(r) = self.read_register(reg) else {
            self.error("register is empty");
            return;
        };
        if self.mode.is_visual() {
            // replace the selection; the register keeps its contents
            let spans = self.visual_spans(false);
            let saved = self.registers_snapshot();
            self.apply_op(Op::Delete, spans, Some('_'), false);
            self.restore_registers(saved);
            let after = r.linewise;
            if r.linewise {
                // the deletion removed whole lines: put the new ones above the cursor line
                return self.put(r, false, count);
            }
            return self.put(r, after && false, count);
        }
        self.put(r, after, count);
    }

    fn put(&mut self, r: Register, after: bool, count: usize) {
        let n = self.buf().sel.len();
        let per_cursor = r.parts.len() == n && n > 1;
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
                    ops::Edit::Change(Change::insert(text::line_end(rope, line), format!("\n{block}")), 1 + lead, 1 + lead)
                } else {
                    ops::Edit::Change(Change::insert(text::line_start(rope, line), format!("{block}\n")), lead, lead)
                }
            } else {
                let s = part.repeat(count);
                let le = text::line_end(rope, text::line_of(rope, pos));
                let at = if after && pos < le { pos + 1 } else { pos };
                let n = s.chars().count();
                ops::Edit::Change(Change::insert(at, s), n.saturating_sub(1), n.saturating_sub(1))
            }
        });
        self.set_mode_raw(Mode::Normal);
        self.clamp_cursors();
    }

    fn registers_snapshot(&self) -> Option<Register> {
        None
    }

    fn restore_registers(&mut self, _saved: Option<Register>) {}

    fn replace_chars(&mut self, c: char, count: usize) {
        let spans: Vec<(usize, usize)> = if self.mode.is_visual() {
            self.selection_spans()
        } else {
            let rope = &self.buf().doc.rope;
            let mut v = Vec::new();
            for r in self.buf().sel.ranges() {
                let le = text::line_end(rope, text::line_of(rope, r.head));
                if r.head + count <= le {
                    v.push((r.head, r.head + count));
                }
            }
            if v.is_empty() {
                return;
            }
            v
        };
        let rope = self.buf().doc.rope.clone();
        let visual = self.mode.is_visual();
        let changes: Vec<Change> = spans
            .iter()
            .map(|&(a, z)| {
                let s: String = rope.slice(a..z).chars().map(|x| if x == '\n' { '\n' } else { c }).collect();
                Change::replace(a, z, s)
            })
            .collect();
        let heads: Vec<usize> = spans.iter().map(|&(a, z)| if visual { a } else { z - 1 }).collect();
        let prim = self.buf().sel.primary_index().min(heads.len() - 1);
        self.set_mode_raw(Mode::Normal);
        self.buf_mut().apply(ChangeSet::new(changes), Some(Selection::new(heads.into_iter().map(Range::point), prim)));
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

/// Text object span `[from, to)` at `pos`.
fn object_span(rope: &Rope, pos: usize, obj: Obj, around: bool) -> Option<(usize, usize)> {
    match obj {
        Obj::Word(big) => Some(motion::word_object(rope, pos, big, around)),
        Obj::Pair(o, c) => {
            let (a, z) = motion::pair_object(rope, pos, o, c, around)?;
            if !around {
                // `i{` on a block whose braces sit on their own lines: just the inner lines
                let la = text::line_of(rope, a);
                if text::char_at(rope, a) == Some('\n') && a < z {
                    let lz = text::line_of(rope, z);
                    if lz > la + 1 && text::first_non_blank(rope, lz) == z {
                        return Some((a + 1, text::line_start(rope, lz)));
                    }
                }
            }
            Some((a, z))
        }
        Obj::Quote(q) => motion::quote_object(rope, pos, q, around),
        Obj::Para => Some(motion::paragraph_object(rope, pos, around)),
    }
}

#[allow(dead_code)]
fn class_at(rope: &Rope, pos: usize) -> Option<CharClass> {
    text::char_at(rope, pos).map(text::class)
}
