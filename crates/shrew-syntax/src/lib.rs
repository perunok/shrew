//! Syntax highlighting for shrew: Sublime grammars via syntect (pure Rust, fancy-regex), run entirely off the UI
//! thread, colored by a [`shrew_theme::Theme`] (see [`Theme::new`]).
//!
//! * [`Syntaxes`] owns one worker thread. It loads the grammar dump in the background at startup; until then (and
//!   for any line not highlighted yet) [`Highlighter::spans_for_line`] returns `None` and the line is drawn plain.
//! * One [`Highlighter`] per buffer. Hand it each edit with [`Highlighter::sync`] (O(1) rope snapshot + a few
//!   microseconds of bookkeeping), tell it what is on screen with [`Highlighter::set_viewport`], and when the
//!   [`Wake`] callback fires call [`Highlighter::poll`] and redraw.
//! * Re-highlighting is incremental: it restarts at the first edited line and stops as soon as the parser state
//!   matches what it was before the edit (see `work`).
//!
//! ```no_run
//! use shrew_syntax::{Syntaxes, Theme};
//! use std::sync::Arc;
//! let syntaxes = Syntaxes::new(Theme::shrew(), Arc::new(|| { /* request a redraw */ }));
//! let mut doc = shrew_core::Document::from_str("fn main() {}\n");
//! doc.path = Some("main.rs".into());
//! let mut hl = syntaxes.highlighter(&doc);
//! hl.set_viewport(0, 40);
//! // on wake / each frame:
//! hl.sync(&mut doc);
//! hl.poll();
//! for line in 0..doc.rope.len_lines() {
//!     match hl.spans_for_line(line) {
//!         Some(spans) => { /* draw spans; uncovered text in theme.foreground */ }
//!         None => { /* draw plain */ }
//!     }
//! }
//! ```

mod detect;
mod engine;
mod highlighter;
pub mod symbols;
mod theme;
mod work;

pub use engine::{Options, Syntaxes, Wake, sublime_user_dirs};
pub use highlighter::Highlighter;
/// For [`Theme::syntect`] without a direct dependency.
pub use syntect;
pub use theme::{Rgb, Span, Theme};
pub use work::{MAX_LINE_CHARS, WINDOWED_BYTES, WINDOWED_LINES};

#[cfg(test)]
mod tests;
