# Changelog

All notable changes to shrew are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and shrew uses [Semantic Versioning](https://semver.org/). `scripts/release.sh` moves the Unreleased notes under the
new version when it tags a release.

## [Unreleased]

### Added

- Editing engine: rope buffers, multiple cursors, undo/redo with grouped edits, search and replace.
- Vim modal editing (normal, insert, visual, visual line, visual block): motions, operators, text objects, counts,
  registers, macros, marks, dot-repeat, ex commands. Can be switched off for Sublime-style always-insert editing.
- Sublime Text chords: multi-cursor (Ctrl+D, Ctrl+Shift+L, Ctrl+Alt+Up/Down), move/duplicate/delete lines, toggle
  comment, go to file (Ctrl+P), command palette (Ctrl+Shift+P).
- User key mappings from `config.toml`, an existing `~/.vimrc` / `init.vim` and Sublime Text's user key bindings.
- GPU renderer (wgpu) with a cached glyph atlas and an embedded JetBrains Mono font.
- Syntax highlighting with Sublime grammars (syntect, pure Rust) on a background thread.
- 33 themes (shrew's own plus Slinger's 31 with their editor palettes), 18 accents over any theme, `theme = "system"`
  to follow the desktop's light/dark setting, custom themes in `themes/*.toml` (reloaded when saved), and
  `:theme import` for Sublime `.sublime-color-scheme` and TextMate `.tmTheme` files. Every built-in theme and accent
  passes a WCAG contrast audit.
- Theme, accent and font pickers with live preview (Esc restores, Enter saves to config.toml); `:theme`, `:accent`,
  `:font`, `:set (no)ligatures`; `[theme]` and `[font]` config sections with font weight; font size changes are
  saved.
- Every optional feature listed in the Features palette and switchable at runtime, in the config or at compile time.
- MCP server so AI assistants (Claude Code, Claude Desktop, Cursor) can read and edit open buffers; off by default.
- Self-update from GitHub Releases with sha256 verification and a one-click update button.
- Release workflow for Linux (x86_64, aarch64), macOS (Intel, Apple silicon) and Windows, plus a `curl | sh`
  installer.
- The `srw` command (`shrew` is a symlink to it), like Sublime's `subl`: `srw file.rs:42:7` opens files in the running
  shrew and brings it to the front, or starts one detached from the terminal and returns at once. `srw dir`, `srw -`
  (stdin), `-w/--wait` for `EDITOR="srw -w"`, `-n/--new-window`, `-f/--foreground`. See docs/CLI.md.
- Files over 1 MB open on a background thread from everywhere (`:e`, the file picker, `srw`, drag and drop).
- Files dropped on the window open.

### Changed

- Picker rows: every detail (description, path, command name) starts at one column after the labels; long labels
  and details are cut with an ellipsis.

[Unreleased]: https://github.com/perunok/shrew/commits/main
