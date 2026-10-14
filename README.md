<p align="center">
  <img src="assets/shrew.svg" width="96" height="96" alt="shrew logo">
</p>

# shrew

[![CI](https://github.com/perunok/shrew/actions/workflows/ci.yml/badge.svg)](https://github.com/perunok/shrew/actions/workflows/ci.yml)
[![latest release](https://img.shields.io/github/v/release/perunok/shrew?sort=semver)](https://github.com/perunok/shrew/releases/latest)
[![MIT license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

A tiny, fast, keyboard-first text editor in pure Rust, drawn on the GPU. Vim's modal editing and Sublime Text's
multi-cursor chords work side by side, your `.vimrc` and Sublime settings come along, and the mouse works when you
reach for it. A shrew is small, quick and eats a lot for its size: shrew opens in a blink, answers every key within
a frame, uses no CPU while it waits, and every feature beyond plain editing can be switched off, at which point it
is never started. No account, no telemetry, nothing to install but one binary.

Small, quick, yours: read [the Shrew Manifesto](#manifesto).

![shrew editing Rust in vim normal mode (Catppuccin Mocha theme): three files open in tabs, syntax highlighting, indent guides, the cursor on an opening brace with its matching brace highlighted, and the NORMAL badge in the status bar](docs/screenshot.png)

## Screenshots

Every shot is the real editor, rendered headless from shrew's own source code. Regenerate them with
`scripts/screenshots.sh`.

| | |
|---|---|
| ![Normal mode](docs/screenshots/normal.png)<br>**Vim normal mode** with syntax highlighting, indent guides and `*` search | ![Multiple cursors](docs/screenshots/multi-cursor.png)<br>**Ctrl+N** adds a cursor at each next match, as in vim-visual-multi |
| ![Sublime mode](docs/screenshots/sublime.png)<br>**Sublime mode** (`:set novim`): Ctrl+D five times, then type | ![File picker](docs/screenshots/file-picker.png)<br>**Ctrl+P** fuzzy-finds files, respecting `.gitignore` |
| ![Command palette](docs/screenshots/palette.png)<br>**Ctrl+Shift+P** runs any command by its name | ![Features](docs/screenshots/features.png)<br>**Every feature is optional**: `:features` lists and toggles them |
| ![Theme picker](docs/screenshots/theme-picker.png)<br>**33 themes** with a live preview as you move through them | ![Light theme](docs/screenshots/light.png)<br>**Light themes** too, here One Light in visual line mode |
| ![High contrast](docs/screenshots/contrast.png)<br>**High Contrast** (WCAG AAA); every theme passes a contrast audit | ![Substitute](docs/screenshots/substitute.png)<br>**`:%s` substitution**, with the last search highlighted |
| ![About](docs/screenshots/about.png)<br>**About shrew** and the Shrew Manifesto (`:about`) | ![Huge file](docs/screenshots/huge-file.png)<br>**A 50 MB, million-line file**, searched at line 500,000 |
| ![Sidebar](docs/screenshots/sidebar.png)<br>**The sidebar** (Ctrl+B): the project's tree, following the active file | ![Find in project](docs/screenshots/find-in-project.png)<br>**Find in project** (Ctrl+Shift+F), grouped by file as results stream in |
| ![Symbols](docs/screenshots/symbols.png)<br>**Go to symbol** (`@` in Ctrl+P, Ctrl+R) from the grammar's scopes | |
| ![Terminal](docs/screenshots/terminal.png)<br>**The built-in terminal** (Ctrl+\`): colors from the theme, Ctrl+click opens `file:line` | ![Vim in the terminal](docs/screenshots/terminal-vim.png)<br>**Full-screen programs** work: vim inside shrew's terminal, One Light |

## Features

- **Snappy by design.** shrew redraws only when something changes, never on a timer, so an idle shrew uses no CPU.
  Glyphs are rasterised once into a GPU atlas, shaped lines are cached, and syntax highlighting runs on a
  background thread, so typing never waits on it. See [Performance](#performance) for measured numbers.
- **Vim, for real.** Normal, insert, visual, visual line and visual block modes, with motions, operators, text
  objects, counts, registers, macros, marks, dot-repeat, `:s`, `:g`, ranges and more. 839 differential test cases
  check shrew against Vim 9.1 itself (`crates/shrew-core/tests/vim_diff.rs`).
- **Sublime Text chords.** Ctrl+D, Ctrl+Shift+L, Ctrl+Alt+Up/Down, Ctrl+P, Ctrl+Shift+P, move, duplicate and
  delete lines, toggle comments. They work in every mode. Turn vim off (`:set novim`) and shrew is an
  always-typing, Sublime-style editor with Ctrl+S, Ctrl+Z, Ctrl+C/X/V and Ctrl+F.
- **Multiple cursors** everywhere: Ctrl+D and Ctrl+N add the next occurrence, Alt+F3 selects them all, Ctrl+click
  adds one, and vim commands run at every cursor.
- **Brings your config along.** At startup shrew reads your `~/.vimrc` / `init.vim` (options, leader, the whole
  `map` / `noremap` family) and Sublime Text's user settings and key bindings. You add or override anything in
  `config.toml`, and shrew's own changes to it (`:set`, theme and font pickers) keep your comments.
- **Pickers.** A fuzzy file finder (Ctrl+P) that walks the project in the background and respects `.gitignore`, a
  command palette (Ctrl+Shift+P), a buffer list (Ctrl+Shift+B) and the Features list.
- **A title bar of its own.** The tabs, the menus (F10, Alt+F) and the window buttons share the top row, arranged
  as you like (`[titlebar]`). Drag it to move the window, double-click to maximize, or switch back to the system
  title bar with one toggle.
- **Project navigation.** shrew finds the project a file belongs to (`.git`, a Cargo workspace, `package.json`, …,
  never your home folder). A file tree sidebar (Ctrl+B), find in project (Ctrl+Shift+F, pure-Rust parallel grep),
  go to symbol in the file (Ctrl+R / `@` in Ctrl+P) and in the project (Ctrl+Shift+R) from the grammars' scopes,
  recent files and projects (Ctrl+Alt+P), reopen closed files (Ctrl+Shift+T), vim's jump list (Ctrl+O / Ctrl+I) and
  `gf`. See [KEYBINDINGS.md](docs/KEYBINDINGS.md#project-navigation).
- **Themes.** 33 built in (Dracula, Nord, Gruvbox, Catppuccin, Tokyo Night, Solarized, One Dark, two high-contrast
  ones and more), 18 accents over any of them, following the desktop's light/dark setting, your own TOML themes,
  and imported Sublime or TextMate color schemes. Theme, accent and font pickers preview live. See
  **[docs/THEMES.md](docs/THEMES.md)**.
- **Syntax highlighting** with Sublime Text grammars (syntect on fancy-regex, pure Rust), incremental: after an
  edit it restarts at the edited line and stops as soon as the result matches the previous run.
- **The `srw` command.** Like Sublime's `subl`: `srw file.rs:42` hands files to the running shrew and brings it to
  the front, or starts one detached from the terminal, and returns at once. `srw -w` makes shrew your `$EDITOR`.
  See **[docs/CLI.md](docs/CLI.md)**.
- **Huge files.** A rope keeps edits O(log n), files over 1 MB load on a background thread, and only visible lines
  are drawn, so a 50 MB, million-line file stays interactive.
- **AI-ready, opt-in.** A built-in [MCP](https://modelcontextprotocol.io) server lets Claude Code, Claude Desktop
  or Cursor read and edit your open buffers. Every edit is one undo step and nothing is saved without you. Off by
  default.
- **A terminal inside.** Ctrl+\` opens your shell in a panel under the text, drawn in the theme's colors: tabs,
  scrollback, vim, htop and less work, a vim-style normal mode to search and copy, and Ctrl+click on
  `src/main.rs:12:5` in the output opens it. Nothing runs until you open one. See
  **[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md#terminal)**.
- **Updates itself.** shrew checks GitHub Releases once a day and shows an update button in the status bar. The
  download is verified against its sha256 before it replaces the binary.
- **Pure Rust.** It builds with `cargo` alone: no C compiler, no system `-dev` packages. GPU drivers, Wayland and
  X11 are loaded at runtime.

### Switchable features

Plain editing is always there: rope buffers, unlimited undo/redo with grouped edits, multiple cursors, regex search
and `:s` replace, and the vim and Sublime keymaps. Everything else is an optional feature. Every optional feature:

- shows up in the **Features** list (`:features`, or palette *Features: List and Toggle*), where Enter toggles it;
- can be switched **at runtime** with `:set <key>`, `:set no<key>` or `:set <key>!` (toggle). Changes made this way
  are written back to `config.toml`, and your comments in the file are kept;
- can be set **in the config** under `[features]`, as `<key> = true|false`;
- and, where a cargo feature is listed, can be **compiled out** entirely. That removes its code and dependencies and
  makes the binary smaller.

A feature that is off is never started: no thread, no file watcher, no socket, no memory. `srw --features` prints
the table below for your build and config.

| Key | What it does | Default | Cargo feature |
| --- | --- | :---: | --- |
| `vim` | Modal editing (normal/insert/visual). Off: always typing, Sublime-style | on | |
| `syntax` | Syntax highlighting with Sublime grammars (syntect, pure Rust), incremental, on a background thread | on | `syntax` |
| `bracket_match` | Highlight the bracket matching the one at the cursor | on | |
| `auto_pairs` | Close brackets and quotes as you type them | on | |
| `auto_indent` | Keep and extend indentation on new lines | on | |
| `line_numbers` | Line number gutter | on | |
| `relative_numbers` | Line numbers relative to the cursor (vim's `relativenumber`) | off | |
| `current_line` | Highlight the line the cursor is on | on | |
| `indent_guides` | Thin vertical lines at each indent level | on | |
| `whitespace` | Show tabs and trailing spaces | off | |
| `search_highlight` | Highlight every match of the last search | on | |
| `word_highlight` | Highlight other occurrences of the selected word | on | |
| `smooth_scroll` | Animated scrolling (redraws only while moving) | on | |
| `cursor_animation` | Cursor glides to its new position | on | |
| `picker` | Fuzzy file finder (Ctrl+P), respects `.gitignore` | on | `picker` |
| `palette` | Command palette (Ctrl+Shift+P) | on | |
| `clipboard` | Share yanks and pastes with the system clipboard | on | `clipboard` |
| `mcp` | MCP server so AI assistants can read and edit open files | **off** | `mcp` |
| `update_check` | Check GitHub for new releases at startup (at most once a day) | on | `update` |
| `vimrc` | Read options and mappings from `~/.vimrc` or `init.vim` at startup (see `[import]`) | on | |
| `sublime_import` | Read Sublime Text's user settings and key bindings at startup (see `[import]`) | on | |
| `custom_titlebar` | shrew's own title bar: tabs, menus (F10) and window buttons in one row. Off: the system title bar | on | |
| `sidebar` | File tree of the project on the left (Ctrl+B, Ctrl+Shift+E) | off | `project` |
| `project_search` | Find in project (Ctrl+Shift+F, `:grep`), respects `.gitignore` | on | `project` |
| `symbols` | Go to symbol in the file (Ctrl+R, `@` in Ctrl+P) and the project (Ctrl+Shift+R) | on | `syntax` |
| `recent` | Remember recent files and projects (Open Recent, Ctrl+Alt+P) in the cache dir | on | |
| `minimap` | Sublime-style minimap at the right of the editor: click or drag to scroll (see `[minimap]`) | on | |
| `terminal` | Built-in terminal panel (Ctrl+\`), started on first use (see `[terminal]`) | on | `terminal` |

Keys also work without the underscore (`:set nolinenumbers`). The usual vim spellings work too: `:set nu`, `rnu`,
`hls`, `list`, `et`, `ts=2`, `so=8`.

```toml
# ~/.config/shrew/config.toml
[features]
vim = false            # Sublime-style, always typing
cursor_animation = false
smooth_scroll = false
mcp = true
```

**Compile-time.** The `shrew` package's (the `srw` binary's) default cargo features are `syntax`, `picker`,
`clipboard`, `mcp`, `update`, `embed-font`, `project` and `terminal`. A minimal build:

```sh
cargo build --release -p shrew --no-default-features                     # just the editor
cargo build --release -p shrew --no-default-features --features syntax   # plus highlighting
```

A feature that is compiled out shows as unavailable in the Features list, and `:set` on it explains why. With
`embed-font` off, shrew uses a system monospace font instead of the bundled JetBrains Mono (about 830 KB).
`shrew-syntax` also has an opt-in feature: `sublime-user` loads extra `.sublime-syntax` files from Sublime's
`Packages/User`. Color schemes (`.sublime-color-scheme`, `.tmTheme`) are always readable: see
[docs/THEMES.md](docs/THEMES.md).

## Requirements

- **To run:** Linux with a Vulkan or OpenGL (EGL) driver and a Wayland or X11 session, macOS (Metal), or Windows
  (DX12 or Vulkan). On Linux those libraries are loaded with `dlopen` at startup, so nothing else is needed.
- **To build:** a stable Rust toolchain that supports edition 2024, and nothing else. There is no C compiler,
  `pkg-config` or `-dev` package to install on any platform.

## Install

**Linux and macOS.** This installs the `srw` command (and a `shrew` symlink to it) to `~/.local/bin`, verifies the
checksum, and on Linux adds shrew to the application menu:

```sh
curl -fsSL https://raw.githubusercontent.com/perunok/shrew/master/scripts/install.sh | sh
```

Options: `sh -s -- --version 0.2.0`, `--dir /some/bin`, `--no-desktop`, `--uninstall`.

**Windows.** Download `shrew-x86_64-pc-windows-msvc.exe` (or the `.zip`) from the
[latest release](https://github.com/perunok/shrew/releases/latest), rename it to `srw.exe` and put it on your `PATH`.

**By hand.** Every [release](https://github.com/perunok/shrew/releases) has these files:

| Platform | Raw binary | Archive |
| --- | --- | --- |
| Linux x86_64 | `shrew-x86_64-unknown-linux-gnu` | `shrew-v<version>-x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64 | `shrew-aarch64-unknown-linux-gnu` | `shrew-v<version>-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple silicon | `shrew-aarch64-apple-darwin` | `shrew-v<version>-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `shrew-x86_64-apple-darwin` | `shrew-v<version>-x86_64-apple-darwin.tar.gz` |
| Windows x86_64 | `shrew-x86_64-pc-windows-msvc.exe` | `shrew-v<version>-x86_64-pc-windows-msvc.zip` |

Each raw binary has a `.sha256` file next to it (check it with `sha256sum -c`). The Linux archives also contain
`shrew.desktop` and the icon. The builds are not signed yet. On macOS, run `xattr -d com.apple.quarantine srw` if
Gatekeeper refuses to open it. On Windows, choose SmartScreen > More info > Run anyway.

**From source.** See [Development](#development).

## Quick start

```sh
srw                    # a scratch buffer; Ctrl+P finds files under the current directory
srw src/main.rs:42     # open files (at a line); the prompt comes back at once
srw .                  # this folder as the project: the file picker opens in it
git config --global core.editor "srw -w"   # commit messages in shrew
```

Like Sublime's `subl`, `srw` hands the files to the shrew that is already running and brings its window to the
front, or starts one in the background. `shrew` is the same command under its longer name. See
[docs/CLI.md](docs/CLI.md) for every option.

shrew starts in vim's normal mode. If you do not use vim, run `:set novim` or toggle **Vim** in the Features list
(palette, *Features: List and Toggle*). From then on shrew always types, and the familiar shortcuts work: Ctrl+S,
Ctrl+Z, Ctrl+C/X/V, Ctrl+F, Ctrl+D.

| | |
| --- | --- |
| Find a file | Ctrl+P |
| Run any command | Ctrl+Shift+P (command palette) |
| Switch buffer | Ctrl+Shift+B, Ctrl+Tab |
| Save / quit | Ctrl+S / Ctrl+Q (or `:w`, `:q`, `:wq`) |
| Add a cursor at the next match | Ctrl+D (insert) / Ctrl+N (normal) |
| See and toggle features | `:features` |
| Edit your config | palette: *Shrew: Open Config File* (`:config`) |
| Version, manifesto, credits | palette: *About shrew* (`:about`) |

## Keybindings

These are the most useful ones. The full reference, written from the code, is in
**[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md)**. "Ctrl" means ⌘ Cmd on macOS as well.

**Everywhere (Sublime-style chords)**

| Keys | Action |
| --- | --- |
| Ctrl+S, Ctrl+Shift+S | Save, save all |
| Ctrl+P, Ctrl+Shift+P | Go to file, command palette |
| Ctrl+Shift+B, Ctrl+Tab / Ctrl+Shift+Tab | Switch buffer, next / previous buffer |
| Ctrl+Z, Ctrl+Shift+Z | Undo, redo |
| Ctrl+/ | Toggle comment |
| Ctrl+Shift+D, Ctrl+Shift+K | Duplicate line, delete line |
| Alt+Up/Down (or Ctrl+Shift+Up/Down) | Move line up/down |
| Ctrl+Alt+Up/Down (or Alt+Shift+Up/Down) | Add a cursor above/below |
| Ctrl+L, Ctrl+Shift+L | Select line, split selection into one cursor per line |
| Alt+F3 | Select every occurrence |
| Ctrl+M, Ctrl+Shift+M | Go to matching bracket, select inside brackets |
| Ctrl+G | Go to line |
| Ctrl+Enter, Ctrl+Shift+Enter | Insert line below, above |
| Ctrl+= / Ctrl+- / Ctrl+0 | Font bigger / smaller / reset |

**Insert mode, or anywhere with vim off:** Ctrl+C/X/V copy, cut and paste (the whole line when nothing is
selected). Ctrl+A selects all, Ctrl+F finds, F3 / Shift+F3 go to the next / previous match, Ctrl+D adds the next
occurrence, Ctrl+Y redoes. Shift+arrows select, Ctrl+arrows move by word, Tab / Shift+Tab indent / outdent.

**Vim normal mode** has the standard vim keys:

- motions `h j k l w b e W B E ge 0 ^ $ gg G f F t T ; , % { } H M L * # n N`, and marks with `m{a-z}`, `` `{a-z} ``
  and `'{a-z}`;
- the operators `d c y > < g~ gu gU`, with counts, motions, a search (`d/foo`) or text objects
  (`iw aW i( a{ i[ i< i" a' ip` and the backtick quote);
- `x X s S C D Y p P J gJ r ~ u`, Ctrl+R, `.`, `i a I A o O gI`, `v V gv`, Ctrl+V, `/ ? :`;
- macros `q{reg}`, `@{reg}` and `@@`, registers `"{reg}` (`"+` is the system clipboard, `"_` discards, `"A` appends);
- Ctrl+A / Ctrl+X, `zz zt zb`, Ctrl+D/U/F/B/E/Y, `ZZ ZQ`, `gt gT`.

Ctrl+N adds a cursor at the next occurrence of the word, as in vim-visual-multi.

**Mouse:** click to place the cursor, double-click to select a word, triple-click to select a line, and drag to
select (a drag in vim enters visual mode). Shift+click extends the selection, Ctrl+click or Alt+click adds a cursor,
and the wheel scrolls.

## Configuration

shrew reads `config.toml` from the platform config directory:

| OS | Path |
| --- | --- |
| Linux | `$XDG_CONFIG_HOME/shrew/config.toml` (usually `~/.config/shrew/config.toml`) |
| macOS | `~/Library/Application Support/shrew/config.toml` |
| Windows | `%APPDATA%\shrew\config.toml` |

To open it, use palette *Shrew: Open Config File* (`:config`). The first time, it is created with every setting
commented out at its default value. Settings are layered, lowest first: built-in defaults, then Sublime Text
settings, then your vimrc, then `config.toml`.

```toml
[editor]
tab_width = 4
expand_tab = true
scrolloff = 4
leader = " "              # <leader> is Space

[theme]
name = "shrew-dark"       # 33 built in, your own, or "system": see docs/THEMES.md
accent = "theme"          # or blue, teal, rose, amber, ... (18)

[font]
family = "JetBrains Mono"
size = 15.0
line_height = 1.4
ligatures = true

[features]
relative_numbers = true

[keys.normal]
"<leader>w" = ":w"        # ":" + ex command
"<leader>f" = "files"     # a command name (see :palette)
"Y" = "y$"                # keys, vim notation, not remapped

[keys.insert]
"jk" = "<Esc>"

[import]
vimrc = "auto"            # "auto" | "off" | "/path/to/vimrc"
sublime = "auto"          # "auto" | "off" | "/path/to/Packages/User"

[mcp]
port = 7356

[update]
check = true
```

**Bring your vim and Sublime setup.** At startup shrew reads the first vimrc it finds (`~/.vimrc`, `~/.vim/vimrc`,
`~/.config/nvim/init.vim`). It takes the options it has an equivalent for (`tabstop`, `expandtab`, `number`,
`relativenumber`, `scrolloff`, `hlsearch`, `list`, `cursorline`, `clipboard`, `timeoutlen`, ...), `let mapleader`,
and the whole `map` / `noremap` / `unmap` family. It also reads Sublime Text's `Preferences.sublime-settings` and
`Default (<OS>).sublime-keymap`. Anything it cannot use (plugins, functions, `<expr>` mappings, autocmds) is skipped
and summarised in one status-line message. Nothing in an imported file can stop shrew from starting.

The full reference is in **[docs/CONFIG.md](docs/CONFIG.md)**.

## Themes

33 built-in themes, light and dark, including two high-contrast ones; 18 accents to lay over any of them; or
`theme = "system"` to follow the desktop's light/dark setting with a theme of your choice for each. Pick with
palette *Theme: Select…* (`:theme`), *Accent: Select…* (`:accent`) and *Font: Select…* (`:font`): moving through a
list previews it live, Esc puts the old one back and Enter saves it to `config.toml`. Write your own themes as TOML
files (reloaded when saved), or convert a Sublime `.sublime-color-scheme` or TextMate `.tmTheme` with
`:theme import <file>`. Every built-in theme passes a WCAG contrast audit with every accent. See
**[docs/THEMES.md](docs/THEMES.md)**.

## AI assistants (MCP)

shrew can expose its open buffers to AI assistants over the
[Model Context Protocol](https://modelcontextprotocol.io). Assistants can list, read and search buffers, open files,
edit text (each edit is one undo step, and nothing is saved unless the assistant calls save), run commands and move
the cursor.

1. Turn it on: `:set mcp`, or toggle **MCP** in the Features list. The server listens on `127.0.0.1` only and uses a
   private token.
2. Connect your assistant. The palette command *Shrew: AI Assistants (MCP) Setup* (`:mcp`) shows these commands
   with the exact path of your shrew binary filled in.

**Claude Code:**

```sh
claude mcp add shrew -- srw --mcp-stdio
```

**Claude Desktop, Cursor and other clients.** Add this to the client's MCP config and restart the client. For Claude
Desktop that is `~/.config/Claude/claude_desktop_config.json` on Linux,
`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS and
`%APPDATA%\Claude\claude_desktop_config.json` on Windows. For Cursor it is `~/.cursor/mcp.json`.

```json
{
  "mcpServers": {
    "shrew": { "command": "srw", "args": ["--mcp-stdio"] }
  }
}
```

The client starts `srw --mcp-stdio`, a small bridge that connects to the running editor and starts shrew if it is
closed (only while MCP is on). To cut off access at any time, run `:set nomcp`.

## Updates

- shrew checks [GitHub Releases](https://github.com/perunok/shrew/releases) in the background at startup, at most
  once a day. When a newer version exists, an **update button** appears in the status bar. Click it (or run
  `:update`, or palette *Shrew: Install Update and Restart*) and shrew downloads the binary for your platform,
  verifies its sha256, swaps it in atomically and restarts. Ctrl+Shift+U (palette *Shrew: Check for Updates*) checks
  right away.
- To turn automatic checks off, set `[update] check = false` or `:set noupdate_check`. To remove the code
  altogether, build without the `update` cargo feature.
- If shrew was installed by a package manager, or its binary is not writable, it does not update itself. It tells
  you to use the package manager instead.
- Under the hood: `scripts/release.sh X.Y.Z` bumps the version, updates the changelog and tags `vX.Y.Z`. Pushing the
  tag runs [the release workflow](.github/workflows/release.yml). It builds all five targets and drafts a GitHub
  release with the raw binaries, their `.sha256` files and the archives. Once the draft is published, running shrews
  offer the update.

## Performance

Latency comes first. These are measured on a Linux desktop with a release build (`srw --stats`, the headless
`--screenshot --stats` frame benchmark, and the tests below):

| What | Measured |
| --- | --- |
| Start to first frame | about 55-70 ms, most of it Vulkan initialisation |
| CPU while idle | 0: no render loop, no timers, nothing polls |
| Redrawing an unchanged frame at 1080p | about 0.1 ms |
| A frame where every visible line is shaped anew (scrolling a page at 1080p) | about 0.9 ms |
| A 50 MB, 1,000,000-line buffer | every interactive operation far below one 16 ms frame (`crates/shrew-core/tests/perf.rs`) |
| Vim compatibility | 839 differential cases agree with Vim 9.1 (`crates/shrew-core/tests/vim_diff.rs`) |

How:

- **Redraw on demand.** A frame is drawn only after input, a background result (syntax, file list, MCP edit) or an
  animation step, and animations stop as soon as they settle.
- **Glyph atlas.** Each glyph is rasterised once into a GPU texture atlas (etagere packing). A frame is a single
  instanced draw of quads, with no per-frame text rasterisation or texture uploads.
- **Line layout cache.** Shaped lines are cached and reused until their text or style changes, so scrolling and
  cursor movement do not re-shape text.
- **Background syntax.** Grammars load lazily on a worker thread. Highlighting is incremental, and lines that are
  not highlighted yet are drawn plain instead of blocking the frame.
- **Cheap edits.** A rope makes edits O(log n) even in very large files. Undo stores change sets, not snapshots.
- **Fast start.** The font is embedded, so the first frame never waits for a font scan. Optional features are not
  initialised when off. The release profile uses thin LTO, one codegen unit, `panic = "abort"` and stripped symbols.

## Development

```sh
git clone https://github.com/perunok/shrew
cd shrew
cargo build --release -p shrew
./target/release/srw
```

| Command | Purpose |
| --- | --- |
| `cargo run --release -p shrew -- [files]` | Run the editor (the binary is `srw`) |
| `cargo test --workspace` | Unit and integration tests; `vim_diff` runs against `vim` (or `nvim`) when one is on `PATH` |
| `cargo test --release -p shrew-core --test perf -- --ignored --nocapture` | Latency checks on a 50 MB buffer |
| `cargo clippy --workspace --all-targets` | Lints |
| `cargo fmt --all` | Format (`rustfmt.toml`: 120 columns) |
| `scripts/check-pure-rust.sh` | Fails if any dependency compiles or links C |
| `scripts/screenshots.sh [name]` | Regenerate the README and THEMES.md screenshots in `docs/` |
| `srw --screenshot out.png --size 1200x800 --keys "<C-p>main" [files]` | Render one frame headless after typing keys (vim notation), for visual checks |
| `srw -f --stats [files]` | Run in the terminal with startup and frame timings on stderr |
| `scripts/release.sh X.Y.Z` | Bump the version, move the changelog notes and tag a release |

### Architecture

shrew is a Cargo workspace of small crates. Only the app crate knows about windows.

| Crate | Role |
| --- | --- |
| [`shrew-core`](crates/shrew-core) | The editing engine: rope buffers (ropey), multi-cursor selections, undo history, vim and Sublime keymaps, user mappings, search, ex commands, pickers, the features registry. No GPU, no windowing, fully unit-tested. |
| [`shrew-render`](crates/shrew-render) | GPU renderer on wgpu (Vulkan, Metal, DX12, GL): cosmic-text shaping, a cached glyph atlas, instanced quads. Can render headless to PNG. |
| [`shrew-syntax`](crates/shrew-syntax) | Incremental syntax highlighting with Sublime grammars (syntect, fancy-regex) on one worker thread. |
| [`shrew-theme`](crates/shrew-theme) | Themes as data: 33 built-in UI + syntax palettes, 18 accents, custom themes, Sublime/TextMate scheme import, WCAG contrast audit. |
| [`shrew-config`](crates/shrew-config) | `config.toml` (comment-preserving writes), and the vimrc and Sublime Text importers. |
| [`shrew-mcp`](crates/shrew-mcp) | MCP server: a loopback Streamable-HTTP endpoint inside the app and the `--mcp-stdio` bridge. |
| [`shrew-update`](crates/shrew-update) | Self-update from GitHub Releases: pure-Rust TLS (rustls + RustCrypto), sha256 check, atomic swap. |
| [`shrew`](crates/shrew) | The app (`srw`): winit window and event loop, the single-instance `srw` command, drawing, wiring the crates together. |

## Documentation

- [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md): every key, ex command and named command.
- [docs/CONFIG.md](docs/CONFIG.md): every `config.toml` setting, and the vimrc and Sublime import.
- [docs/THEMES.md](docs/THEMES.md): themes, accents, custom themes, importing color schemes, fonts.
- [docs/CLI.md](docs/CLI.md): the `srw` command, single instance, `$EDITOR`.
- [CHANGELOG.md](CHANGELOG.md): what changed in each release.

All of them are listed in [docs/README.md](docs/README.md).

## Manifesto

**The Shrew Manifesto**

1. **Small is a superpower.** One binary, a few megabytes, no runtime. Open before you finish blinking.
2. **A heart at a thousand beats.** Every keystroke answered within a frame. Latency is a bug.
3. **Keys first, paws welcome.** Your Vim and Sublime muscle memory just works; the mouse is there when you reach for it.
4. **Forage only what you need.** Every feature is optional. Switch off what you don't use and it costs nothing.
5. **The burrow is yours.** No account, no telemetry, no cloud. Bring your `.vimrc` and Sublime settings; they're welcome here.
6. **Rust to the bone.** Pure Rust, top to bottom. No hidden C, nothing you can't read.
7. **Sharp senses.** Syntax, search, brackets and multi-cursors: precise tools, never noise.
8. **Never stop moving.** Open source, MIT licensed, built in the open, one click to the latest version.

*Stay small. Stay quick. Shrew on.*

The same text is in the app (palette *About shrew*, or `:about`); it lives in `crates/shrew/src/about.rs`, so keep
the two in step. A test (`readme_has_the_same_manifesto`) fails when they drift apart.

## Contributing

Work on a branch, and keep `cargo test --workspace`, `cargo clippy --workspace --all-targets` and
`scripts/check-pure-rust.sh` green; run `cargo fmt --all` before committing. Add tests next to the code you change
(`tests.rs` beside each crate's modules, `crates/shrew-core/tests/vim_diff.rs` for a vim behaviour, which compares
against real Vim). shrew stays pure Rust: no dependency may compile or link C at build time (loading a system library
at runtime is fine). Latency is a feature: a change that makes typing, scrolling or startup slower needs a very good
reason. If you change a key binding, update [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md); if you change what the
editor looks like, rerun `scripts/screenshots.sh` and look at the result.

## License

Released under the [MIT License](LICENSE) © 2026 perunok. The bundled JetBrains Mono font is under the
[SIL Open Font License](crates/shrew-render/assets/OFL.txt). The open-source projects shrew builds on are thanked in
the app (*About shrew*).
