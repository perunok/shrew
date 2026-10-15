//! Blame, streamed: `git blame --incremental` reports line ranges as it attributes them, so the first lines show
//! while a big file's history is still being walked. An unsaved buffer is blamed through `--contents -`; its
//! new lines come back as the zero commit ("Not Committed Yet").

use crate::cli::Cancel;
use crate::error::Result;
use crate::log::Signature;
use crate::oid::Oid;
use crate::repo::Repo;
use crate::text::Text;
use std::collections::HashMap;
use std::sync::Arc;

/// A commit as blame describes it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BlameCommit {
    /// [`Oid::ZERO`] for lines not committed yet.
    pub id: Oid,
    pub author: Signature,
    pub committer: Signature,
    pub summary: String,
    /// The commit before this one and the file's path there (for "blame before this change").
    pub previous: Option<(Oid, String)>,
    /// The commit is a root (or the edge of a shallow clone).
    pub boundary: bool,
}

impl BlameCommit {
    pub fn is_uncommitted(&self) -> bool {
        self.id.is_zero()
    }
}

/// Consecutive lines attributed to one commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameHunk {
    /// First line in the blamed text (0-based).
    pub start: u32,
    pub len: u32,
    /// First line in the commit's version of the file (0-based).
    pub orig_start: u32,
    /// The path in that commit (differs after a rename).
    pub orig_path: String,
    pub commit: Arc<BlameCommit>,
}

/// Parses `git blame --incremental` output fed line by line.
#[derive(Default)]
pub(crate) struct Parser {
    commits: HashMap<Oid, Arc<BlameCommit>>,
    current: Option<(Oid, u32, u32, u32)>,
    pending: BlameCommit,
    previous: Option<(Oid, String)>,
    boundary: bool,
}

fn parse_tz(tz: &str) -> i32 {
    let (sign, digits) = match tz.as_bytes().first() {
        Some(b'-') => (-1, &tz[1..]),
        Some(b'+') => (1, &tz[1..]),
        _ => (1, tz),
    };
    let n: i32 = digits.parse().unwrap_or(0);
    sign * ((n / 100) * 3600 + (n % 100) * 60)
}

impl Parser {
    /// Feeds one output line; returns a hunk when an entry is complete.
    pub fn line(&mut self, line: &str) -> Option<BlameHunk> {
        let line = line.trim_end_matches(['\n', '\r']);
        if self.current.is_none() {
            let mut parts = line.split(' ');
            let id = Oid::from_hex(parts.next()?)?;
            let orig: u32 = parts.next()?.parse().ok()?;
            let fin: u32 = parts.next()?.parse().ok()?;
            let len: u32 = parts.next()?.parse().ok()?;
            self.current = Some((id, orig, fin, len));
            self.pending = BlameCommit { id, ..BlameCommit::default() };
            self.previous = None;
            self.boundary = false;
            return None;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let p = &mut self.pending;
        match key {
            "author" => p.author.name = value.to_string(),
            "author-mail" => p.author.email = value.trim_matches(['<', '>']).to_string(),
            "author-time" => p.author.time = value.parse().unwrap_or(0),
            "author-tz" => p.author.offset = parse_tz(value),
            "committer" => p.committer.name = value.to_string(),
            "committer-mail" => p.committer.email = value.trim_matches(['<', '>']).to_string(),
            "committer-time" => p.committer.time = value.parse().unwrap_or(0),
            "committer-tz" => p.committer.offset = parse_tz(value),
            "summary" => p.summary = value.to_string(),
            "boundary" => self.boundary = true,
            "previous" => {
                if let Some((id, path)) = value.split_once(' ') {
                    self.previous = Oid::from_hex(id).map(|id| (id, unquote(path)));
                }
            }
            "filename" => {
                let (id, orig, fin, len) = self.current.take()?;
                let commit = match self.commits.get(&id) {
                    Some(c) => c.clone(),
                    None => {
                        let mut c = std::mem::take(&mut self.pending);
                        c.previous = self.previous.take();
                        c.boundary = self.boundary;
                        let c = Arc::new(c);
                        self.commits.insert(id, c.clone());
                        c
                    }
                };
                return Some(BlameHunk {
                    start: fin.saturating_sub(1),
                    len,
                    orig_start: orig.saturating_sub(1),
                    orig_path: unquote(value),
                    commit,
                });
            }
            _ => {}
        }
        None
    }
}

/// Undoes git's C-quoting of a path (`"caf\303\251.txt"`).
pub(crate) fn unquote(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else { return s.to_string() };
    let b = inner.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            i += 1;
            match b[i] {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'0'..=b'7' if i + 2 < b.len() => {
                    let oct = std::str::from_utf8(&b[i..i + 3]).ok().and_then(|o| u8::from_str_radix(o, 8).ok());
                    out.push(oct.unwrap_or(b'?'));
                    i += 2;
                }
                c => out.push(c),
            }
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Repo {
    /// Blames `path` as of `rev` (HEAD and the worktree file when `None`), or the given buffer `contents`,
    /// calling `on_hunk` as ranges are attributed (in no particular order). Cancel stops git.
    pub fn blame(
        &self,
        path: &str,
        rev: Option<Oid>,
        contents: Option<&Text>,
        cancel: &Cancel,
        on_hunk: &mut dyn FnMut(BlameHunk),
    ) -> Result<()> {
        let mut cmd = self.cmd().read_only().args(["blame", "--incremental"]);
        if let Some(text) = contents {
            cmd = cmd.args(["--contents", "-"]).stdin(text.bytes().to_vec());
        }
        if let Some(r) = rev {
            cmd = cmd.arg(r.to_string());
        }
        let mut parser = Parser::default();
        let mut on_line = |l: &[u8]| {
            if let Some(h) = parser.line(&String::from_utf8_lossy(l)) {
                on_hunk(h);
            }
        };
        cmd.arg("--").arg(path).stream(cancel, &mut |_| {}, Some(&mut on_line))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_incremental_output() {
        let out = "\
1111111111111111111111111111111111111111 1 1 2
author Ann
author-mail <ann@x>
author-time 1700000000
author-tz -0130
committer Bob
committer-mail <bob@x>
committer-time 1700000100
committer-tz +0200
summary first
boundary
filename a.txt
1111111111111111111111111111111111111111 4 3 1
filename a.txt
0000000000000000000000000000000000000000 3 4 1
author Not Committed Yet
author-mail <not.committed.yet>
author-time 1700000200
author-tz +0000
committer Not Committed Yet
committer-mail <not.committed.yet>
committer-time 1700000200
committer-tz +0000
summary Version of a.txt from -
previous 1111111111111111111111111111111111111111 \"caf\\303\\251.txt\"
filename a.txt
";
        let mut p = Parser::default();
        let hunks: Vec<_> = out.lines().filter_map(|l| p.line(l)).collect();
        assert_eq!(hunks.len(), 3);
        assert_eq!((hunks[0].start, hunks[0].len, hunks[0].orig_start), (0, 2, 0));
        assert_eq!(hunks[0].commit.author.name, "Ann");
        assert_eq!(hunks[0].commit.author.email, "ann@x");
        assert_eq!(hunks[0].commit.author.offset, -5400);
        assert!(hunks[0].commit.boundary);
        assert!(Arc::ptr_eq(&hunks[0].commit, &hunks[1].commit));
        assert_eq!(hunks[1].start, 2);
        assert!(hunks[2].commit.is_uncommitted());
        assert_eq!(hunks[2].commit.previous.as_ref().unwrap().1, "café.txt");
    }
}
