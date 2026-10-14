//! User key mappings, vim-style: per mode, a key sequence (`jk`, `<leader>w`, `<C-h>`) maps to other keys
//! (recursive or not), a [`Command`] or an ex command line. They come from the config's `[keys]`, an imported
//! vimrc or Sublime keymap, and run before the built-in bindings.
//!
//! Typed keys that start a longer mapping are held until the mapping completes, the keys diverge (then the held
//! keys run unmapped, vim's rule) or the app calls [`Editor::flush_pending_mappings`] after
//! [`Keymap::timeout_ms`]. Dot-repeat replays keys that were already mapped, so it skips this layer; macros
//! record and replay the typed keys, so they go through it, like vim.

use crate::commands::Command;
use crate::editor::{Editor, Mode};
use crate::input::{Key, KeyPress};
use crate::vim;
use std::sync::Arc;

/// How deep mappings may expand into other mappings (`nmap a b`, `nmap b a` stops here).
pub const MAX_DEPTH: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapMode {
    Normal,
    Insert,
    /// Every visual mode (char, line, block).
    Visual,
    /// After an operator (`d`, `c`, `y`, ...), vim's `omap`.
    OperatorPending,
}

impl MapMode {
    pub const ALL: [MapMode; 4] = [MapMode::Normal, MapMode::Insert, MapMode::Visual, MapMode::OperatorPending];

    pub fn name(self) -> &'static str {
        match self {
            MapMode::Normal => "normal",
            MapMode::Insert => "insert",
            MapMode::Visual => "visual",
            MapMode::OperatorPending => "operator",
        }
    }

    /// Accepts the config table names and vim's mode letters.
    pub fn from_name(s: &str) -> Option<MapMode> {
        Some(match s {
            "normal" | "n" => MapMode::Normal,
            "insert" | "i" => MapMode::Insert,
            "visual" | "v" | "x" => MapMode::Visual,
            "operator" | "operator_pending" | "o" => MapMode::OperatorPending,
            _ => return None,
        })
    }
}

/// What a mapping does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rhs {
    /// Keys fed to the editor. `remap` lets them trigger other mappings (`nmap`); `noremap` sets it false.
    /// No keys at all is `<Nop>`: the lhs is swallowed.
    Keys {
        keys: Vec<KeyPress>,
        remap: bool,
    },
    Command(Command),
    /// An ex command line without the `:` (`w`, `%s/a/b/g`), run directly without opening the prompt.
    Ex(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub lhs: Vec<KeyPress>,
    pub rhs: Rhs,
}

/// A parsed vim mapping command (`nnoremap <silent> <leader>w :w<CR>`, `iunmap jk`, `mapclear`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapCommand {
    Map {
        modes: &'static [MapMode],
        mapping: Mapping,
    },
    Unmap {
        modes: &'static [MapMode],
        lhs: Vec<KeyPress>,
    },
    Clear {
        modes: &'static [MapMode],
    },
    /// Valid vim, nothing for shrew to do: listing mappings, or a mode shrew lacks (select, command line,
    /// terminal, lang-arg).
    Ignored,
}

pub struct Keymap {
    maps: [Vec<Arc<Mapping>>; 4],
    /// `<leader>` in mappings defined from now on (vim resolves it at definition time too). Default `\`.
    pub leader: KeyPress,
    pub local_leader: KeyPress,
    /// How long the app waits for the rest of a mapping before [`Editor::flush_pending_mappings`] (vim's
    /// 'timeoutlen').
    pub timeout_ms: u64,
    held: Vec<KeyPress>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap {
            maps: Default::default(),
            leader: KeyPress::ch('\\'),
            local_leader: KeyPress::ch('\\'),
            timeout_ms: 1000,
            held: Vec::new(),
        }
    }
}

impl std::fmt::Debug for Keymap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keymap").field("mappings", &self.len()).field("leader", &self.leader).finish()
    }
}

/// Outcome of looking held keys up in one mode.
struct Lookup {
    /// The longest mapping whose lhs the keys start with.
    exact: Option<Arc<Mapping>>,
    /// Some mapping's lhs is longer and starts with the keys: worth waiting for more.
    longer: bool,
}

impl Keymap {
    /// Adds a mapping, replacing one with the same lhs in that mode. An empty lhs is ignored.
    pub fn map(&mut self, mode: MapMode, lhs: Vec<KeyPress>, rhs: Rhs) {
        if lhs.is_empty() {
            return;
        }
        let list = &mut self.maps[mode as usize];
        let m = Arc::new(Mapping { lhs, rhs });
        match list.iter().position(|x| x.lhs == m.lhs) {
            Some(i) => list[i] = m,
            None => list.push(m),
        }
    }

    /// Adds a mapping written in vim notation. `rhs` is keys (`y$`, `:w<CR>`), `<Nop>`, or `<Cmd>…<CR>` for an
    /// ex command.
    pub fn map_str(&mut self, mode: MapMode, lhs: &str, rhs: &str, remap: bool) {
        let lhs = self.parse_keys(lhs);
        let rhs = self.parse_rhs(rhs, remap);
        self.map(mode, lhs, rhs);
    }

    /// Removes a mapping; true if there was one.
    pub fn unmap(&mut self, mode: MapMode, lhs: &[KeyPress]) -> bool {
        let list = &mut self.maps[mode as usize];
        let n = list.len();
        list.retain(|m| m.lhs != lhs);
        list.len() != n
    }

    pub fn clear(&mut self, mode: MapMode) {
        self.maps[mode as usize].clear();
    }

    pub fn get(&self, mode: MapMode, lhs: &[KeyPress]) -> Option<&Mapping> {
        self.maps[mode as usize].iter().find(|m| m.lhs == lhs).map(|m| &**m)
    }

    /// Mappings of one mode, in definition order.
    pub fn mappings(&self, mode: MapMode) -> impl Iterator<Item = &Mapping> {
        self.maps[mode as usize].iter().map(|m| &**m)
    }

    pub fn len(&self) -> usize {
        self.maps.iter().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.maps.iter().all(Vec::is_empty)
    }

    /// Typed keys held because they start a longer mapping.
    pub fn held(&self) -> &[KeyPress] {
        &self.held
    }

    /// Sets the leader from config/vimrc text: `,`, `" "`, `<Space>`, `\`.
    pub fn set_leader(&mut self, s: &str) -> bool {
        match leader_key(s) {
            Some(k) => {
                self.leader = k;
                true
            }
            None => false,
        }
    }

    /// Applies a parsed mapping command.
    pub fn apply(&mut self, cmd: MapCommand) {
        match cmd {
            MapCommand::Map { modes, mapping } => {
                for &m in modes {
                    self.map(m, mapping.lhs.clone(), mapping.rhs.clone());
                }
            }
            MapCommand::Unmap { modes, lhs } => {
                for &m in modes {
                    self.unmap(m, &lhs);
                }
            }
            MapCommand::Clear { modes } => {
                for &m in modes {
                    self.clear(m);
                }
            }
            MapCommand::Ignored => {}
        }
    }

    /// Parses vim key notation: `jk`, `<leader>w`, `<C-w>h`, `<Esc>`, `<Space>`, `<Bar>`, `<lt>`, `<Nop>` (no
    /// keys). Text that is not a key name stays literal (`<gv` is `<`, `g`, `v`).
    pub fn parse_keys(&self, s: &str) -> Vec<KeyPress> {
        parse_notation(s, self.leader, self.local_leader)
    }

    /// Parses a mapping's right-hand side: `<Cmd>…<CR>` becomes an ex command, anything else keys.
    pub fn parse_rhs(&self, rhs: &str, remap: bool) -> Rhs {
        if let Some(line) = cmd_rhs(rhs) {
            return Rhs::Ex(line.to_string());
        }
        Rhs::Keys { keys: self.parse_keys(rhs), remap }
    }

    fn lookup(&self, mode: MapMode, keys: &[KeyPress]) -> Lookup {
        let mut exact: Option<&Arc<Mapping>> = None;
        let mut longer = false;
        for m in &self.maps[mode as usize] {
            if m.lhs.len() > keys.len() {
                longer |= m.lhs.starts_with(keys);
            } else if keys.starts_with(&m.lhs) && exact.is_none_or(|e| e.lhs.len() < m.lhs.len()) {
                exact = Some(m);
            }
        }
        Lookup { exact: exact.cloned(), longer }
    }

    fn starts_any(&self, mode: MapMode, k: KeyPress) -> bool {
        self.maps[mode as usize].iter().any(|m| m.lhs[0] == k)
    }
}

/// `<Cmd>w<CR>` → `w`.
fn cmd_rhs(rhs: &str) -> Option<&str> {
    let lower = rhs.get(..5)?.to_ascii_lowercase();
    if lower != "<cmd>" {
        return None;
    }
    let body = &rhs[5..];
    let end = body.len().checked_sub(4)?;
    body.get(end..).filter(|t| t.eq_ignore_ascii_case("<cr>")).map(|_| body[..end].trim())
}

/// The leader as written in `let mapleader = ...` or the config: one char or one key name.
pub fn leader_key(s: &str) -> Option<KeyPress> {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(KeyPress::ch(c)),
        _ => {
            let k = parse_notation(s, KeyPress::ch('\\'), KeyPress::ch('\\'));
            (k.len() == 1).then(|| k[0])
        }
    }
}

fn parse_notation(s: &str, leader: KeyPress, local_leader: KeyPress) -> Vec<KeyPress> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            if let Some(end) = rest[1..].find(['>', '<']).map(|i| i + 1).filter(|&i| rest.as_bytes()[i] == b'>') {
                let inner = &rest[1..end];
                let special = match inner.to_ascii_lowercase().as_str() {
                    "leader" => Some(Some(leader)),
                    "localleader" => Some(Some(local_leader)),
                    "bar" => Some(Some(KeyPress::ch('|'))),
                    "bslash" => Some(Some(KeyPress::ch('\\'))),
                    "nop" => Some(None),
                    "nl" => Some(Some(KeyPress::ctrl('j'))),
                    "kenter" => Some(Some(KeyPress::plain(Key::Enter))),
                    "kplus" => Some(Some(KeyPress::ch('+'))),
                    "kminus" => Some(Some(KeyPress::ch('-'))),
                    "kmultiply" => Some(Some(KeyPress::ch('*'))),
                    "kdivide" => Some(Some(KeyPress::ch('/'))),
                    "kpoint" => Some(Some(KeyPress::ch('.'))),
                    _ => None,
                };
                let key = match special {
                    Some(k) => Some(k),
                    // `<x>` alone is not a key name in vim: keep it literal
                    None if inner.chars().count() > 1 => {
                        KeyPress::parse(&rest[..=end]).map(|k| Some(normalize(k, inner)))
                    }
                    None => None,
                };
                if let Some(k) = key {
                    out.extend(k);
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        }
        out.push(KeyPress::ch(c));
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Brings parsed notation to the form the app sends: `<S-a>` is `A`, `<M-J>` is alt+shift+`j`.
fn normalize(mut k: KeyPress, inner: &str) -> KeyPress {
    let Key::Char(c) = k.key else { return k };
    let m = k.mods;
    if m.shift && !m.ctrl && !m.alt && !m.logo {
        let up = c.to_uppercase().next().unwrap_or(c);
        if up != c {
            k = KeyPress::ch(up);
        }
    } else if !m.ctrl && (m.alt || m.logo) && inner.chars().last().is_some_and(|l| l.is_uppercase()) {
        k.mods.shift = true;
    }
    k
}

// ----- vim mapping commands -----

const NVO: &[MapMode] = &[MapMode::Normal, MapMode::Visual, MapMode::OperatorPending];
const N: &[MapMode] = &[MapMode::Normal];
const V: &[MapMode] = &[MapMode::Visual];
const O: &[MapMode] = &[MapMode::OperatorPending];
const I: &[MapMode] = &[MapMode::Insert];
const NONE: &[MapMode] = &[];

enum Kind {
    Map(bool),
    Unmap,
    Clear,
}

/// (full name, shortest abbreviation, modes, kind). Modes shrew lacks map to `NONE`.
static MAP_COMMANDS: &[(&str, usize, &[MapMode], Kind)] = &[
    ("map", 3, NVO, Kind::Map(true)),
    ("nmap", 2, N, Kind::Map(true)),
    ("vmap", 2, V, Kind::Map(true)),
    ("xmap", 2, V, Kind::Map(true)),
    ("smap", 4, NONE, Kind::Map(true)),
    ("omap", 2, O, Kind::Map(true)),
    ("imap", 2, I, Kind::Map(true)),
    ("lmap", 2, NONE, Kind::Map(true)),
    ("cmap", 2, NONE, Kind::Map(true)),
    ("tmap", 3, NONE, Kind::Map(true)),
    ("noremap", 2, NVO, Kind::Map(false)),
    ("nnoremap", 2, N, Kind::Map(false)),
    ("vnoremap", 2, V, Kind::Map(false)),
    ("xnoremap", 2, V, Kind::Map(false)),
    ("snoremap", 4, NONE, Kind::Map(false)),
    ("onoremap", 3, O, Kind::Map(false)),
    ("inoremap", 3, I, Kind::Map(false)),
    ("lnoremap", 2, NONE, Kind::Map(false)),
    ("cnoremap", 3, NONE, Kind::Map(false)),
    ("tnoremap", 3, NONE, Kind::Map(false)),
    ("unmap", 3, NVO, Kind::Unmap),
    ("nunmap", 3, N, Kind::Unmap),
    ("vunmap", 2, V, Kind::Unmap),
    ("xunmap", 2, V, Kind::Unmap),
    ("sunmap", 4, NONE, Kind::Unmap),
    ("ounmap", 2, O, Kind::Unmap),
    ("iunmap", 2, I, Kind::Unmap),
    ("lunmap", 3, NONE, Kind::Unmap),
    ("cunmap", 2, NONE, Kind::Unmap),
    ("tunmap", 3, NONE, Kind::Unmap),
    ("mapclear", 4, NVO, Kind::Clear),
    ("nmapclear", 5, N, Kind::Clear),
    ("vmapclear", 5, V, Kind::Clear),
    ("xmapclear", 5, V, Kind::Clear),
    ("omapclear", 5, O, Kind::Clear),
    ("imapclear", 5, I, Kind::Clear),
];

/// Parses one vim mapping command (no `|`-separated tail): `nnoremap <silent> <leader>w :w<CR>`. `None` when the
/// line is not a mapping command; `Err` when it is one shrew cannot honour (`<expr>`, `<Plug>`, script calls).
pub fn parse_map_command(line: &str, keymap: &Keymap) -> Option<Result<MapCommand, String>> {
    let line = line.trim_start().trim_start_matches(':').trim_start();
    let name_end = line.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(line.len());
    let (name, mut rest) = line.split_at(name_end);
    let bang = rest.starts_with('!');
    if bang {
        rest = &rest[1..];
    }
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let (full, _, modes, kind) =
        MAP_COMMANDS.iter().find(|(full, min, _, _)| name.len() >= *min && full.starts_with(name))?;
    // `map!` / `noremap!` / `unmap!` cover insert and command-line mode
    let modes: &'static [MapMode] = match (bang, *full) {
        (false, _) => modes,
        (true, "map" | "noremap" | "unmap") => I,
        (true, _) => return None,
    };
    let mut rest = rest.trim_start();
    // <silent>, <buffer>, ... in any order
    loop {
        let Some(end) = rest.strip_prefix('<').and_then(|r| r.find('>')) else { break };
        let attr = rest[1..=end].to_ascii_lowercase();
        match attr.as_str() {
            "silent" | "buffer" | "nowait" | "special" | "script" | "unique" => {}
            "expr" => return Some(Err("<expr> mappings are not supported".into())),
            _ => break,
        }
        rest = rest[end + 2..].trim_start();
    }
    if let Kind::Clear = kind {
        return Some(Ok(MapCommand::Clear { modes }));
    }
    let lhs_end = lhs_end(rest);
    let lhs_text = &rest[..lhs_end];
    let rhs_text = rest[lhs_end..].trim_start();
    if lhs_text.is_empty() {
        return Some(Ok(MapCommand::Ignored)); // `:nmap` lists mappings
    }
    if modes.is_empty() {
        return Some(Ok(MapCommand::Ignored));
    }
    let lhs = keymap.parse_keys(lhs_text);
    match kind {
        Kind::Unmap => Some(Ok(MapCommand::Unmap { modes, lhs })),
        Kind::Map(_) if rhs_text.is_empty() => Some(Ok(MapCommand::Ignored)), // `:nmap x` lists one mapping
        Kind::Map(remap) => {
            let lower = rhs_text.to_ascii_lowercase();
            if lhs_text.to_ascii_lowercase().contains("<plug>") {
                return Some(Err("mapping defines a <Plug> target".into()));
            }
            for bad in
                ["<plug>", "<sid>", "<snr>", "<c-r>=", ":call ", "<cmd>call ", ":exe", "<cmd>exe", ":lua ", "<cmd>lua "]
            {
                if lower.contains(bad) {
                    return Some(Err(format!("mapping runs vim script ({})", bad.trim())));
                }
            }
            Some(Ok(MapCommand::Map { modes, mapping: Mapping { lhs, rhs: keymap.parse_rhs(rhs_text, *remap) } }))
        }
        Kind::Clear => unreachable!(),
    }
}

/// The lhs ends at the first whitespace not escaped with a backslash or Ctrl-V.
fn lhs_end(s: &str) -> usize {
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' || c == '\u{16}' {
            escaped = true;
        } else if c.is_whitespace() {
            return i;
        }
    }
    s.len()
}

// ----- dispatch -----

impl Editor {
    /// True while typed keys are held for a possibly longer mapping: the app should call
    /// [`Editor::flush_pending_mappings`] once [`Keymap::timeout_ms`] passes without another key.
    pub fn mapping_pending(&self) -> bool {
        !self.keymap.held.is_empty()
    }

    /// Parses and applies a vim mapping command typed at runtime (`:nnoremap ...`). False if `line` is not one.
    pub fn run_map_command(&mut self, line: &str) -> bool {
        match parse_map_command(line, &self.keymap) {
            None => false,
            Some(Ok(cmd)) => {
                self.keymap.apply(cmd);
                true
            }
            Some(Err(e)) => {
                self.error(e);
                true
            }
        }
    }

    /// Which mapping table applies to `k` right now; `None` while a prompt, picker, the About panel or a vim command that takes a
    /// character argument (`f`, `r`, `"`, `m`, ...) is waiting.
    fn map_mode(&self, k: KeyPress) -> Option<MapMode> {
        if self.picker.is_some() || self.prompt.is_some() || self.about.is_some() {
            return None;
        }
        if self.mode == Mode::Insert || !self.vim() {
            return Some(MapMode::Insert);
        }
        let digit =
            |k: &KeyPress, first: bool| matches!(k.text(), Some(c) if c.is_ascii_digit() && !(first && c == '0'));
        let p = &self.pending;
        let mut i = 0;
        if p.first().and_then(KeyPress::text) == Some('"') {
            if p.len() < 2 {
                return None;
            }
            i = 2;
        }
        let count_digits = |mut i: usize| {
            let start = i;
            while p.get(i).is_some_and(|k| digit(k, i == start)) {
                i += 1;
            }
            (i, i > start)
        };
        let (j, had_count) = count_digits(i);
        // "0" after a count is part of it, never a mapping
        let zero = k.text() == Some('0');
        if j == p.len() {
            if had_count && zero {
                return None;
            }
            return Some(if self.mode.is_visual() { MapMode::Visual } else { MapMode::Normal });
        }
        if self.mode.is_visual() {
            return None;
        }
        let op_len = match (p[j].text(), p.get(j + 1).and_then(KeyPress::text)) {
            (Some('d' | 'c' | 'y' | '<' | '>'), _) => 1,
            (Some('g'), Some('~' | 'u' | 'U')) => 2,
            _ => return None,
        };
        let (end, had_count) = count_digits(j + op_len);
        (end == p.len() && !(had_count && zero)).then_some(MapMode::OperatorPending)
    }

    /// The keymap's look at a typed key. True if it took the key (held it or ran a mapping).
    pub(crate) fn map_key(&mut self, k: KeyPress) -> bool {
        if self.keymap.held.is_empty() {
            if self.keymap.is_empty() {
                return false;
            }
            match self.map_mode(k) {
                Some(mode) if self.keymap.starts_any(mode, k) => {}
                _ => return false,
            }
        }
        self.keymap.held.push(k);
        self.resolve_held(false);
        true
    }

    /// Resolves held keys on timeout. True if there were any.
    pub(crate) fn flush_mappings(&mut self) -> bool {
        if self.keymap.held.is_empty() {
            return false;
        }
        self.resolve_held(true);
        true
    }

    fn resolve_held(&mut self, flush: bool) {
        while let Some(&first) = self.keymap.held.first() {
            let found = match self.map_mode(first) {
                Some(mode) => self.keymap.lookup(mode, &self.keymap.held),
                None => Lookup { exact: None, longer: false },
            };
            if found.longer && !flush {
                return;
            }
            match found.exact {
                Some(m) => {
                    self.keymap.held.drain(..m.lhs.len());
                    self.run_rhs(&m, 0);
                }
                None => {
                    let k = self.keymap.held.remove(0);
                    self.dispatch_key(k);
                }
            }
        }
    }

    /// Runs a mapping's rhs. False if it failed, which aborts the mapping that expanded to it (vim's rule).
    fn run_rhs(&mut self, m: &Mapping, depth: usize) -> bool {
        match &m.rhs {
            Rhs::Command(cmd) => {
                self.pending.clear();
                self.run(*cmd);
                true
            }
            Rhs::Ex(line) => {
                self.pending.clear();
                self.run_ex(line);
                true
            }
            Rhs::Keys { keys, remap: false } => self.feed(keys, None, depth),
            Rhs::Keys { keys, remap: true } => self.feed(keys, Some(&m.lhs), depth),
        }
    }

    /// Feeds an rhs's keys; with `remap` (the lhs that expanded to them) they may trigger further mappings.
    fn feed(&mut self, keys: &[KeyPress], remap: Option<&[KeyPress]>, depth: usize) -> bool {
        if depth >= MAX_DEPTH {
            self.error("recursive mapping");
            return false;
        }
        let mut i = 0;
        while i < keys.len() {
            let rest = &keys[i..];
            // vim: when the rhs starts with the lhs, that first key is not mapped again (`nmap x "_x` style)
            let found = remap.filter(|lhs| !(i == 0 && rest.starts_with(lhs))).and_then(|_| {
                let mode = self.map_mode(rest[0])?;
                self.keymap.lookup(mode, rest).exact
            });
            if let Some(m) = found {
                i += m.lhs.len();
                if !self.run_rhs(&m, depth + 1) {
                    return false;
                }
                continue;
            }
            let k = keys[i];
            i += 1;
            match self.vim_rejects(k) {
                Reject::No => self.dispatch_key(k),
                Reject::Abort => return false,
                // window commands (`<C-w>h`): shrew has no splits, so the pair does nothing
                Reject::SkipPair => i += 1,
            }
        }
        true
    }

    /// Would the vim layer reject `k` as an unknown command? Built-in bindings (Ctrl chords with nothing pending)
    /// are given the benefit of the doubt.
    fn vim_rejects(&self, k: KeyPress) -> Reject {
        if self.picker.is_some()
            || self.prompt.is_some()
            || self.about.is_some()
            || self.mode == Mode::Insert
            || !self.vim()
        {
            return Reject::No;
        }
        let mut keys = self.pending.clone();
        keys.push(k);
        if !matches!(vim::parse(&keys, self.mode, self.macro_rec.is_some()), vim::Parse::Invalid) {
            return Reject::No;
        }
        let m = k.mods;
        if self.pending.is_empty() && k == KeyPress::ctrl('w') {
            return Reject::SkipPair;
        }
        let maybe_binding = self.pending.is_empty() && (m.ctrl || m.alt || m.logo || !matches!(k.key, Key::Char(_)));
        if maybe_binding { Reject::No } else { Reject::Abort }
    }
}

enum Reject {
    No,
    Abort,
    SkipPair,
}

/// Formats keys back to vim notation (for listings and messages).
pub fn keys_to_string(keys: &[KeyPress]) -> String {
    keys.iter().map(|k| k.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::{Feature, Mods, Selection};

    fn ed(text: &str) -> Editor {
        let pos = text.find('|').map(|b| text[..b].chars().count()).unwrap_or(0);
        let mut e = Editor::new();
        e.install_document(Document::from_str(&text.replace('|', "")));
        e.buf_mut().sel = Selection::point(pos);
        e
    }

    fn keys(e: &mut Editor, s: &str) {
        for k in KeyPress::parse_seq(s) {
            e.handle_key(k);
        }
    }

    fn show(e: &Editor) -> String {
        let mut s = e.buf().doc.rope.to_string();
        let h = e.buf().sel.primary().head;
        let b = s.char_indices().nth(h).map_or(s.len(), |(b, _)| b);
        s.insert(b, '|');
        s
    }

    fn map(e: &mut Editor, line: &str) {
        assert!(e.run_map_command(line), "{line}");
        assert!(e.message.is_none(), "{line}: {:?}", e.message);
    }

    #[test]
    fn insert_jk_escape() {
        let mut e = ed("|");
        map(&mut e, "inoremap jk <Esc>");
        keys(&mut e, "ihijk");
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(show(&e), "h|i");
        // diverging keys come out literally, in order
        keys(&mut e, "ajxjjk");
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(show(&e), "hijx|j");
    }

    #[test]
    fn timeout_flushes_held_keys() {
        let mut e = ed("|");
        map(&mut e, "inoremap jk <Esc>");
        keys(&mut e, "ij");
        assert!(e.mapping_pending());
        assert_eq!(e.keymap.held(), &[KeyPress::ch('j')]);
        assert_eq!(show(&e), "|");
        e.flush_pending_mappings();
        assert!(!e.mapping_pending());
        assert_eq!(show(&e), "j|");
        assert_eq!(e.mode, Mode::Insert);
    }

    #[test]
    fn leader_and_ex_keys() {
        let path = std::env::temp_dir().join(format!("shrew-keymap-{}.txt", std::process::id()));
        std::fs::write(&path, "abc\n").unwrap();
        let mut e = Editor::new();
        e.open(&path).unwrap();
        e.keymap.set_leader(" ");
        map(&mut e, "nnoremap <leader>w :w<CR>");
        keys(&mut e, "x");
        assert!(e.buf().doc.dirty);
        keys(&mut e, " w");
        assert!(!e.buf().doc.dirty, "{:?}", e.message);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "bc\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn default_leader_is_backslash() {
        let mut e = ed("a\n|b\nc");
        map(&mut e, "nnoremap <Leader>d dd");
        keys(&mut e, "\\d");
        assert_eq!(show(&e), "a\n|c");
    }

    #[test]
    fn recursive_map() {
        let mut e = ed("foo |bar");
        map(&mut e, "nmap Y y$");
        keys(&mut e, "YP");
        assert_eq!(e.buf().doc.rope.to_string(), "foo barbar");
        // nmap through another mapping, nnoremap does not
        let mut e = ed("a\n|b\nc");
        map(&mut e, "nnoremap Q dd");
        map(&mut e, "nmap X Q");
        map(&mut e, "nnoremap Z Q");
        keys(&mut e, "Z");
        assert_eq!(show(&e), "a\n|b\nc");
        keys(&mut e, "X");
        assert_eq!(show(&e), "a\n|c");
    }

    #[test]
    fn recursion_is_bounded() {
        let mut e = ed("|abc");
        map(&mut e, "nmap a b");
        map(&mut e, "nmap b a");
        keys(&mut e, "a");
        assert_eq!(e.message.as_ref().map(|m| m.0.as_str()), Some("recursive mapping"));
        assert_eq!(show(&e), "|abc");
        // rhs starting with its lhs does not loop
        e.message = None;
        map(&mut e, "nmap x xl");
        keys(&mut e, "x");
        assert_eq!(show(&e), "b|c");
    }

    #[test]
    fn visual_reindent_keeps_selection() {
        let mut e = ed("|        a\n        b\nc");
        map(&mut e, "vnoremap < <gv");
        keys(&mut e, "Vj<");
        assert_eq!(e.buf().doc.rope.to_string(), "    a\n    b\nc");
        assert_eq!(e.mode, Mode::VisualLine);
        keys(&mut e, "<");
        assert_eq!(e.buf().doc.rope.to_string(), "a\nb\nc");
    }

    #[test]
    fn unsupported_target_does_nothing() {
        let mut e = ed("ab|c");
        map(&mut e, "nnoremap <C-h> <C-w>h");
        e.handle_key(KeyPress::ctrl('h'));
        assert_eq!(show(&e), "ab|c");
        // an invalid key aborts the rest of the mapping
        map(&mut e, "nnoremap K Kh");
        keys(&mut e, "K");
        assert_eq!(show(&e), "ab|c");
    }

    #[test]
    fn prefix_of_builtin_sequences() {
        let mut e = ed("a\nb\n|c");
        map(&mut e, "nnoremap gx dd");
        keys(&mut e, "gg");
        assert_eq!(show(&e), "|a\nb\nc");
        keys(&mut e, "gx");
        assert_eq!(show(&e), "|b\nc");
    }

    #[test]
    fn counts_and_operator_pending() {
        let mut e = ed("|a\nb\nc\nd");
        map(&mut e, "nnoremap Q dd");
        keys(&mut e, "2Q");
        assert_eq!(show(&e), "|c\nd");
        let mut e = ed("|foo bar");
        map(&mut e, "onoremap L $");
        keys(&mut e, "dL");
        assert_eq!(show(&e), "|");
        // `L` alone is not remapped in normal mode
        let mut e = ed("|x");
        map(&mut e, "nnoremap 0 ^");
        keys(&mut e, "10l");
        assert_eq!(show(&e), "|x");
    }

    #[test]
    fn char_arguments_are_not_mapped() {
        let mut e = ed("|abcQ");
        map(&mut e, "nnoremap Q dd");
        keys(&mut e, "fQ");
        assert_eq!(show(&e), "abc|Q");
        keys(&mut e, "rx");
        assert_eq!(show(&e), "abc|x");
    }

    #[test]
    fn command_and_ex_rhs() {
        let mut e = ed("|one\ntwo");
        e.keymap.map(MapMode::Normal, vec![KeyPress::ctrl('k')], Rhs::Command(Command::DeleteLine));
        e.keymap.map_str(MapMode::Normal, "<F5>", "<Cmd>s/two/2/<CR>", false);
        e.handle_key(KeyPress::ctrl('k'));
        assert_eq!(show(&e), "|two");
        e.handle_key(KeyPress::plain(Key::F(5)));
        assert_eq!(e.buf().doc.rope.to_string(), "2");
    }

    #[test]
    fn user_mapping_overrides_builtin_binding() {
        let mut e = ed("|a");
        e.keymap.map_str(MapMode::Insert, "<C-s>", "x", false);
        keys(&mut e, "i");
        e.handle_key(KeyPress::ctrl('s'));
        assert_eq!(show(&e), "x|a");
    }

    #[test]
    fn dot_and_macros() {
        let mut e = ed("|a\nb\nc\nd\ne");
        map(&mut e, "nnoremap Q dd");
        keys(&mut e, "Q.");
        assert_eq!(show(&e), "|c\nd\ne");
        keys(&mut e, "qaQq@a");
        assert_eq!(show(&e), "|e");
    }

    #[test]
    fn insert_mappings_without_vim() {
        let mut e = ed("|");
        e.set_feature(Feature::Vim, false);
        e.message = None;
        map(&mut e, "inoremap jk <Esc>");
        e.keymap.map_str(MapMode::Insert, "<C-k><C-u>", "UP", false);
        keys(&mut e, "a");
        e.handle_key(KeyPress::ctrl('k'));
        e.handle_key(KeyPress::ctrl('u'));
        assert_eq!(show(&e), "aUP|");
        assert_eq!(e.mode, Mode::Insert);
    }

    #[test]
    fn notation() {
        let km = Keymap { leader: KeyPress::ch(','), ..Keymap::default() };
        let k = km.parse_keys("<leader>w<Bar><bslash><Space><lt><gv<C-W><M-J><S-a><S-Tab><Nop>");
        let alt_shift_j = KeyPress::new(Key::Char('j'), Mods { alt: true, shift: true, ..Mods::default() });
        let shift_tab = KeyPress::new(Key::Tab, Mods { shift: true, ..Mods::default() });
        let expect = [
            KeyPress::ch(','),
            KeyPress::ch('w'),
            KeyPress::ch('|'),
            KeyPress::ch('\\'),
            KeyPress::ch(' '),
            KeyPress::ch('<'),
            KeyPress::ch('<'),
            KeyPress::ch('g'),
            KeyPress::ch('v'),
            KeyPress::ctrl('w'),
            alt_shift_j,
            KeyPress::ch('A'),
            shift_tab,
        ];
        assert_eq!(k, expect);
        assert_eq!(km.parse_keys("<b>"), vec![KeyPress::ch('<'), KeyPress::ch('b'), KeyPress::ch('>')]);
        assert_eq!(km.parse_rhs("<Cmd>w<CR>", false), Rhs::Ex("w".into()));
        assert_eq!(leader_key("<Space>"), Some(KeyPress::ch(' ')));
        assert_eq!(leader_key(","), Some(KeyPress::ch(',')));
    }

    #[test]
    fn map_command_parsing() {
        let km = Keymap::default();
        let p = |l: &str| parse_map_command(l, &km);
        let Some(Ok(MapCommand::Map { modes, mapping })) = p("nnoremap <silent> <nowait> <leader>w :w<CR>") else {
            panic!()
        };
        assert_eq!(modes, N);
        assert_eq!(mapping.lhs, KeyPress::parse_seq("\\w"));
        assert_eq!(mapping.rhs, Rhs::Keys { keys: KeyPress::parse_seq(":w<CR>"), remap: false });
        let Some(Ok(MapCommand::Map { modes, mapping })) = p("nmap Y y$") else { panic!() };
        assert_eq!((modes, mapping.rhs), (N, Rhs::Keys { keys: KeyPress::parse_seq("y$"), remap: true }));
        assert!(matches!(p("nn j gj"), Some(Ok(MapCommand::Map { modes: N, .. }))));
        assert!(matches!(p("ino jk <Esc>"), Some(Ok(MapCommand::Map { modes: I, .. }))));
        assert!(matches!(p("xnoremap < <gv"), Some(Ok(MapCommand::Map { modes: V, .. }))));
        assert!(matches!(p("noremap ; :"), Some(Ok(MapCommand::Map { modes: NVO, .. }))));
        assert!(matches!(p("map! <C-l> <Del>"), Some(Ok(MapCommand::Map { modes: I, .. }))));
        assert!(matches!(p("noremap! <C-l> <Del>"), Some(Ok(MapCommand::Map { modes: I, .. }))));
        assert!(matches!(p("cnoremap w!! w"), Some(Ok(MapCommand::Ignored))));
        assert!(matches!(p("nunmap Q"), Some(Ok(MapCommand::Unmap { modes: N, .. }))));
        assert!(matches!(p("mapclear"), Some(Ok(MapCommand::Clear { modes: NVO }))));
        assert!(matches!(p("nmap"), Some(Ok(MapCommand::Ignored))));
        assert!(matches!(p("nnoremap <expr> j v:count ? 'j' : 'gj'"), Some(Err(_))));
        assert!(matches!(p("nmap <leader>c <Plug>Commentary"), Some(Err(_))));
        assert!(matches!(p("nnoremap <F2> :call Foo()<CR>"), Some(Err(_))));
        assert!(p("set number").is_none());
        assert!(p("nohlsearch").is_none());
        assert!(p("n").is_none());
        assert!(p("normal! gg").is_none());
    }
}
