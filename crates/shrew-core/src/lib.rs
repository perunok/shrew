//! shrew's editing engine: buffers, multi-cursor selections, undo, vim and Sublime keymaps, search, pickers.
//! It knows nothing about windows or GPUs: the app feeds it [`KeyPress`]es and mouse positions and draws its state.

pub mod change;
pub mod commands;
pub mod document;
pub mod editor;
pub mod features;
pub mod history;
pub mod input;
pub mod keymap;
pub mod lang;
pub mod motion;
pub mod ops;
pub mod picker;
pub mod search;
pub mod selection;
pub mod text;
mod vim;

pub use change::{Change, ChangeSet, EditSpan};
pub use commands::Command;
pub use document::{Buffer, Document, View};
pub use editor::{Clipboard, CursorShape, Editor, Event, Mode, Prompt, PromptKind, Register, Settings};
pub use features::{Feature, Features};
pub use input::{Key, KeyPress, Mods, MouseButton};
pub use keymap::{Keymap, MapMode, Mapping, Rhs};
pub use picker::Choice;
pub use selection::{Range, Selection};

pub use ropey;

#[cfg(test)]
mod tests;
