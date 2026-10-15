//! [`Text`]: bytes split into lines the way git counts them. A line keeps its terminator, and a text ending in a
//! newline has no extra empty last line (`"a\nb\n"` is two lines; an editor rope shows a third, empty one).

use ropey::Rope;
use std::ops::Range;
use std::sync::Arc;

/// Immutable bytes with their line starts. Cheap to clone (shared).
#[derive(Clone)]
pub struct Text {
    bytes: Arc<[u8]>,
    /// Start offset of every line, plus `bytes.len()` at the end.
    starts: Arc<[usize]>,
    /// Lines ending in `\r\n`.
    crlf: usize,
}

impl std::fmt::Debug for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Text({} lines, {} bytes)", self.len_lines(), self.bytes.len())
    }
}

impl Default for Text {
    fn default() -> Self {
        Text::new(Vec::new())
    }
}

impl PartialEq for Text {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

/// How lines end in a text, by majority.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            LineEnding::Lf => b"\n",
            LineEnding::CrLf => b"\r\n",
        }
    }
}

impl Text {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Text {
        let bytes: Vec<u8> = bytes.into();
        let mut starts = Vec::with_capacity(bytes.len() / 32 + 2);
        starts.push(0);
        let mut crlf = 0;
        for i in memchr::memchr_iter(b'\n', &bytes) {
            crlf += usize::from(i > 0 && bytes[i - 1] == b'\r');
            starts.push(i + 1);
        }
        if *starts.last().unwrap() != bytes.len() {
            starts.push(bytes.len());
        }
        Text { bytes: bytes.into(), starts: starts.into(), crlf }
    }

    /// A snapshot of an editor buffer.
    pub fn from_rope(rope: &Rope) -> Text {
        let mut bytes = Vec::with_capacity(rope.len_bytes());
        for chunk in rope.chunks() {
            bytes.extend_from_slice(chunk.as_bytes());
        }
        Text::new(bytes)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The text as UTF-8, if it is.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Number of lines, git's way (a trailing newline does not start a new line).
    pub fn len_lines(&self) -> usize {
        self.starts.len() - 1
    }

    /// Line `i` with its terminator.
    pub fn line(&self, i: usize) -> &[u8] {
        &self.bytes[self.starts[i]..self.starts[i + 1]]
    }

    /// Line `i` without its `\n` or `\r\n`.
    pub fn line_content(&self, i: usize) -> &[u8] {
        strip_eol(self.line(i)).0
    }

    /// Whether the last line ends in a newline (true for an empty text).
    pub fn ends_with_newline(&self) -> bool {
        self.bytes.last().is_none_or(|&b| b == b'\n')
    }

    /// Byte range of lines `lines` (clamped).
    pub fn line_range(&self, lines: Range<usize>) -> Range<usize> {
        let end = lines.end.min(self.len_lines());
        let start = lines.start.min(end);
        self.starts[start]..self.starts[end]
    }

    /// Bytes of lines `lines`, terminators included.
    pub fn lines_bytes(&self, lines: Range<usize>) -> &[u8] {
        &self.bytes[self.line_range(lines)]
    }

    /// Bytes of a hunk's line range (as in [`crate::diff::Hunk`]), terminators included.
    pub fn hunk_bytes(&self, lines: &std::ops::Range<u32>) -> &[u8] {
        self.lines_bytes(lines.start as usize..lines.end as usize)
    }

    /// Like git: a NUL byte in the first 8000 bytes means binary.
    pub fn is_binary(&self) -> bool {
        self.bytes[..self.bytes.len().min(8000)].contains(&0)
    }

    /// The line ending most lines use (`Lf` when there are no line breaks).
    pub fn line_ending(&self) -> LineEnding {
        let newlines = self.len_lines() - usize::from(!self.ends_with_newline());
        if self.crlf * 2 > newlines { LineEnding::CrLf } else { LineEnding::Lf }
    }
}

/// A line's terminator.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Eol {
    None,
    Lf,
    CrLf,
}

/// Splits a line into its content and terminator.
pub(crate) fn strip_eol(line: &[u8]) -> (&[u8], Eol) {
    match line {
        [rest @ .., b'\r', b'\n'] => (rest, Eol::CrLf),
        [rest @ .., b'\n'] => (rest, Eol::Lf),
        _ => (line, Eol::None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_line_counting() {
        assert_eq!(Text::new("").len_lines(), 0);
        assert_eq!(Text::new("a").len_lines(), 1);
        assert_eq!(Text::new("a\n").len_lines(), 1);
        let t = Text::new("a\nb\r\nc");
        assert_eq!(t.len_lines(), 3);
        assert_eq!(t.line(1), b"b\r\n");
        assert_eq!(t.line_content(1), b"b");
        assert_eq!(t.line_content(2), b"c");
        assert!(!t.ends_with_newline());
        assert_eq!(t.lines_bytes(1..3), b"b\r\nc");
        assert_eq!(t.lines_bytes(2..9), b"c");
    }

    #[test]
    fn line_endings_and_binary() {
        assert_eq!(Text::new("a\r\nb\r\nc\n").line_ending(), LineEnding::CrLf);
        assert_eq!(Text::new("a\nb\r\n").line_ending(), LineEnding::Lf);
        assert!(Text::new(b"a\0b".to_vec()).is_binary());
        assert!(!Text::new("plain").is_binary());
    }

    #[test]
    fn from_rope_matches_bytes() {
        let s = "line one\nline two\n".repeat(500);
        assert_eq!(Text::from_rope(&Rope::from_str(&s)).bytes(), s.as_bytes());
    }
}
