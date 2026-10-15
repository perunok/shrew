//! One error type for every operation: what failed, in words a status bar can show.

use std::fmt;

/// Why an operation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// No repository there, or the path is outside the worktree.
    NotARepo,
    /// The path, commit or ref does not exist.
    NotFound,
    /// The `git` program is not installed or could not be started.
    GitMissing,
    /// `git` ran and failed: its exit code (None when killed).
    Git(Option<i32>),
    /// A patch no longer applies (the index or file changed meanwhile): refresh and retry.
    Stale,
    /// Cancelled through a [`crate::Cancel`].
    Cancelled,
    /// Reading the repository failed (gix), or an I/O error.
    Read,
}

/// An error with a message for the user. For a failed `git` run the message is its output (hooks included).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error { kind, message: message.into() }
    }

    pub(crate) fn read(e: impl fmt::Display) -> Error {
        Error::new(ErrorKind::Read, e.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::read(e)
    }
}

impl From<gix::Error> for Error {
    fn from(e: gix::Error) -> Error {
        Error::read(e)
    }
}
