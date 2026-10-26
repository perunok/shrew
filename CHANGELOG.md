# Changelog

All notable changes to shrew are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and shrew uses [Semantic Versioning](https://semver.org/). `scripts/release.sh` moves the Unreleased notes under the
new version when it tags a release.

## [Unreleased]

### Added

- Context menus: one popup for every right click (the title bar's and the Source Control panel's use it), opened at
  the pointer and kept on screen, with submenus, disabled entries, check marks, shortcut hints, mnemonics,
  type-ahead, j/k, and release-to-select (press the right button, drag onto an entry, release). The title bar's
  dropdown menus draw their entries the same way.
- `--screenshot` takes mouse tokens in `--keys` (`<click:X,Y>`, `<rclick:X,Y>`, `<drag:X1,Y1,X2,Y2>`,
  `<wheel:X,Y,DY>`, with modifiers) and `--hover X,Y`; they go through the same code as the window's events
  (docs/CLI.md, "Screenshots").
- Box (column) selection with the mouse: a middle drag or Shift+Alt+drag (vim: visual block); Shift+Alt+click
  makes the box from the cursor to the click.
- Shift+wheel scrolls sideways. The scrollbar's thumb grows and brightens under the pointer. A picker's row under the
  pointer is highlighted.

### Fixed

- Vim mode: a click put the cursor one character left of the one clicked.
- The pointer is an arrow over the title bar's open menus, not a text cursor.
- Vim mode: a drag showed INSERT and a bar cursor until the button was released, then gained a character; it now
  selects in visual mode as it goes, with the character under the pointer. Shift+click takes the clicked character.
- Shift+click and then dragging moved the anchor to the click.
- Double-clicking the right half of a word's last letter selected the space after it.
- Dragging past the edge of the text scrolled 60 lines a second even one pixel out; it now starts at about 12 and
  speeds up with the distance, also scrolls sideways along long lines, and stops with the last line at the bottom.
- Touchpads and fine-grained wheels: small movements were dropped in pickers, the About panel, Settings lists, the
  sidebar, the git panel and log, for Ctrl+zoom and sideways; they now add up. Sideways scrolling stopped nowhere.
- A picker's list and a Settings dropdown could not be scrolled with the wheel past the highlighted row (pickers
  moved the highlight one row a notch; Settings lists jumped back).
- Grabbing the scrollbar's thumb made it jump to centre on the pointer.
- The wheel while selecting left the selection behind; the pointer flickered to an arrow while selecting over the
  gutter or the title bar.
- A click below the last line put the caret at that column of the last line, not at the end of the text.
- The cursor glide wobbled on screen when the view scrolled with it (`j` on the bottom row), and flew in from far
  away after long jumps.
- Alt+S, Alt+I and Alt+T did not open Selection, Git and Terminal although their letters are underlined.
- Find in project: the "(Tab: *.rs, !tests/)" hint ran into the Alt+R/C/W hints.

## [0.2.0] - 2026-10-15

### Added

- Git integration (cargo feature `git`, on by default; pure Rust reads through gix, writes through the `git`
  program; nothing runs outside a repository or before it is needed). The parts below share one background service.
- Change markers (feature `git_gutter`): added, changed and deleted lines against the index (or HEAD, `[git]
  gutter_base`) in the gutter, the minimap and the scrollbar, following each keystroke. `]c` / `[c` (Ctrl+. /
  Ctrl+Shift+.) move between changes; `ghp` peeks at a change with its changed words marked and Stage, Revert and
  Diff buttons; `ghs` / `ghu` / `ghr` stage, unstage and revert the change or the selected lines.
- Inline blame (feature `git_blame_inline`): the cursor line's author, age and summary at its end after a pause.
- Git status in the chrome (feature `git_status`): the branch with `*` and ahead / behind in the status bar and the
  title bar (a click lists branches), and file states coloured with git's letters in the sidebar.
- The diff view: a tab for a file, the unstaged or staged changes, a commit or two revisions, side by side or
  unified, with syntax colours, changed words, folded context, live edits, and staging, unstaging and reverting
  changes or selected lines (`:Gdiff`, `git_diff_*`, `[git] diff_layout / diff_context / diff_ignore_whitespace`).
- Source Control (feature `git_panel`, cargo feature `git`, both on by default; nothing runs until it is used): the
  sidebar's Git tab (Ctrl+Shift+G, also with the sidebar off) with the branch, its upstream and a sync button, a
  commit message box (Ctrl+Enter commits, Ctrl+Shift+Enter amends, a 50/72 subject hint), and Merge Changes /
  Staged / Changes / Untracked to stage, unstage and discard by key or mouse; branch, stash and changed-file lists;
  fetch, pull and push with a progress toast and cancel; the log tab with the commit graph, ref labels, paging,
  search and author / path filters, and commit details; blame as a column left of the text; merge conflicts tinted
  with *Accept Current / Incoming / Both / Compare*, `]x` / `[x`, and mark resolved. Enter on a change, a file of a
  commit in the log (or `d` for the whole commit) and a click on the blame column open the diff view.
- A Git menu, a Git section in Settings and git colour tokens (`git-added` ... `diff-del-word`) for every theme.
- MCP git tools (with the `mcp` and `git` cargo features): `git_status`, `git_diff` (unstaged, staged, HEAD or a
  commit), `git_log` (paged, or one file's history), `git_blame` (a line range, the open buffer's text) and
  `git_show_commit`. Read-only: there is no tool that stages, commits or pushes.
- [docs/GIT.md](docs/GIT.md): how git works in shrew, what each view does, performance and troubleshooting; git
  screenshots in the README gallery (`scripts/screenshots.sh` builds a deterministic demo repository for them with
  `scripts/demo-repo.sh`).
- `srw --screenshot ... --stats` benchmarks a diff tab's scrolling too.

### Changed

- The release binary grows from about 18 MB to about 22 MB with git support (reading repositories in process costs
  about 4.5 MB). Build without the `git` cargo feature to get the smaller binary back.
- While the window has the focus inside a git repository, one background thread checks a few files every 3 seconds
  to notice changes made outside shrew; nothing runs when the window is not focused or outside a repository.

### Fixed

- The MCP stdio bridge treats a stopped shrew as "not running" on macOS too (a broken pipe, where Linux reports a
  refused connection), so assistants get the clear message instead of a socket error.

## [0.1.0] - 2026-10-15

### Added

- Compact folders in the sidebar (feature `compact_folders`, on by default): a folder whose only entry is another
  folder shares its row with it, so `src/main/java/com` is one row, as in VS Code.
- Project navigation: the project root is found from `.git` / `.hg` / `.jj` / `.svn` / `.shrew`, a Cargo workspace,
  `package.json`, `go.mod` or `pyproject.toml` (never `$HOME` or `/`); a file tree sidebar (Ctrl+B, Ctrl+Shift+E,
  `srw <folder>`) with lazy folders, `.gitignore`d entries dimmed, a filter and new / rename / trash; find in project
  (Ctrl+Shift+F, `:grep`) with regex, case, whole-word and glob options; go to symbol in the file (Ctrl+R,
  Ctrl+Shift+O, `@` in Ctrl+P) and in the project (Ctrl+Shift+R); `:42` and `name:42` in Ctrl+P; recent files and
  projects (Ctrl+Alt+P); reopen closed files (Ctrl+Shift+T); vim's jump list (Ctrl+O / Ctrl+I, `''`) and `gf`.
- Editing engine: rope buffers, multiple cursors, undo/redo with grouped edits, search and replace.
- Vim modal editing (normal, insert, visual, visual line, visual block): motions, operators, text objects, counts,
  registers, macros, marks, dot-repeat, ex commands. Can be switched off for Sublime-style always-insert editing.
- Sublime Text chords: multi-cursor (Ctrl+D, Ctrl+Shift+L, Ctrl+Alt+Up/Down), move/duplicate/delete lines, toggle
  comment, go to file (Ctrl+P), command palette (Ctrl+Shift+P).
- Differential tests against real Vim: 839 cases where shrew must leave the same text and cursor as Vim 9.1
  (`crates/shrew-core/tests/vim_diff.rs`), and latency tests on a 50 MB, million-line buffer.
- Mouse: click, double/triple-click, drag to select, Shift+click to extend, Ctrl/Alt+click for another cursor, the
  wheel and the scrollbar.
- Settings panel (Ctrl+, / Cmd+,, palette *Preferences: Settings*, `:settings [section]`, the gear in the status bar):
  Appearance, Editor, Features, Layout & window, Keybindings, Import, AI assistants, Updates and About, driven by keys
  or the mouse, with search and reset to default. Every change applies at once and is saved to `config.toml`.
  `[mcp] port` is now honoured.
- `config.toml` with comment-preserving writes: `:set`, the Features list and the pickers save their changes and keep
  your comments.
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
- An About panel (palette *About shrew*, `:about`): the version, the Shrew Manifesto, the license, credits and links,
  in a read-only overlay (`j`/`k` and the wheel scroll it, Esc closes it).
- `srw --screenshot out.png --keys ...` renders a frame headless after typing keys, and `scripts/screenshots.sh`
  regenerates every screenshot in the docs with it.
- `scripts/check-pure-rust.sh` fails the build when a dependency compiles or links C.
- README in a new shape: screenshots gallery, features, performance numbers, the Shrew Manifesto.
- A custom title bar (feature `custom_titlebar`, on by default): the buffer tabs, an application menu (File, Edit,
  Selection, View, Go, Terminal, Help; F10 and Alt+F/E/V/G/H, keyboard-navigable, with each entry's chord) and the
  window buttons in one row, arranged with `[titlebar] left / right / hidden`. Drag it to move the window,
  double-click to maximize, right-click for *Customize Title Bar…* and *Use System Title Bar*; the undecorated window
  resizes from its edges. The OS title reads "file — project — shrew".
- A Sublime-style minimap (the `minimap` feature, on by default; `[minimap] width`): each line as thin blocks in
  its syntax colors, with the visible lines boxed, the cursors, selections and search matches marked. Click to jump,
  drag to scroll. Built from the lines on show only and cached, so an unchanged frame rebuilds nothing.
- A built-in terminal (feature `terminal`, Ctrl+\`): your shell in a resizable bottom panel with tabs, drawn in the
  theme's 16 ANSI colors (new `ansi-*` theme tokens, derived when a theme does not set them), 256 and true color,
  10,000 lines of scrollback, full-screen programs, mouse reporting, bracketed paste, a vim-style normal mode to
  search and copy, Ctrl+click on `file:line:col` to open it, and `:term {command}`. `srw file` inside opens in that
  window. Emulation by alacritty_terminal (pure Rust); nothing runs until the first terminal opens.
- The About panel's links (and the repository address at its foot) open in the browser on a click, with a pointer
  cursor over them.

### Changed

- The window's parts fit together: the custom title bar spans the top, the sidebar runs down the left below it, the
  terminal panel takes the bottom of the rest and the minimap the right of the text above it (one `layout::compute`
  for drawing and hit testing). Settings > Layout & window holds the title bar, sidebar, minimap and terminal
  settings; *Customize Title Bar…* opens it there. The menus list the navigation and terminal commands with their
  chords. Terminals start in the project root. The status bar's gear hides while the title bar shows its own. The
  sidebar command is now `toggle_sidebar`, like `toggle_minimap` and `toggle_terminal`.
- Picker rows: every detail (description, path, command name) starts at one column after the labels; long labels
  and details are cut with an ellipsis.
- Faster start: the GPU is opened on a background thread while the config is read and the window is created, and
  glyphs are pre-rasterised only for as long as that takes. The first frame comes about 60 ms after start instead of
  80 (creating the custom title bar's undecorated window takes some 18 ms on Wayland, which had made it slower).
- Release builds use fat LTO: the binary is 1.1 MB smaller (17.1 MiB instead of 18.2 on Linux x86_64) and no
  slower, for a longer build. wgpu is built with only the backends each OS uses (Vulkan and GL on Linux, Metal on
  macOS, DX12, Vulkan and GL on Windows) and WGSL shaders.
- The workspace is clippy-clean, and CI now fails on any clippy warning, with every cargo feature and with none.
  Builds without default features pass their tests too.

[Unreleased]: https://github.com/perunok/shrew/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/perunok/shrew/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/perunok/shrew/releases/tag/v0.1.0
