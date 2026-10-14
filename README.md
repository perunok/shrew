<p align="center">
  <img src="assets/shrew.svg" width="96" height="96" alt="shrew logo">
</p>

<h1 align="center">shrew</h1>

<p align="center">
  A tiny, fast, keyboard-first text editor. Vim's modal editing and Sublime Text's multi-cursor chords, drawn on the GPU, in pure Rust.
</p>

<p align="center">
  <a href="https://github.com/perunok/shrew/actions/workflows/ci.yml"><img src="https://github.com/perunok/shrew/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/perunok/shrew/releases/latest"><img src="https://img.shields.io/github/v/release/perunok/shrew?sort=semver" alt="latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

![shrew editing Rust in normal mode (shrew dark theme)](docs/screenshot.png)

A shrew is small, quick, and eats a lot for its size. The editor works the same way: one small binary that opens
instantly, stays out of your way and responds to every key at once. Latency and memory come first; every feature
beyond plain editing can be switched off, and when it is off it is never started.

- **Snappy.** It redraws only when something changes, never on a timer. Glyphs are rasterised once into a GPU atlas,
  laid-out lines are cached, and syntax highlighting runs on a background thread, so typing never waits on it.
- **Two keymaps.** Vim (normal, insert, visual, visual line and visual block, with operators, text objects, counts,
  registers, macros, marks and dot-repeat) and Sublime Text (Ctrl+D, Ctrl+Shift+L, multiple cursors, Ctrl+P,
  Ctrl+Shift+P) work together. Turn vim off and shrew is an always-typing, Sublime-style editor.
- **Brings your config along.** It reads your `~/.vimrc` / `init.vim` and Sublime Text's user settings and key
  bindings at startup, and you can add or override anything in `config.toml`.
- **Themes.** 33 built in (Dracula, Nord, Gruvbox, Catppuccin, Tokyo Night, Solarized, One Dark, high contrast...),
  18 accents over any of them, following the desktop's light/dark setting, your own TOML themes and imported Sublime
  or TextMate color schemes. Pick themes, accents and fonts with live preview. See
  **[docs/THEMES.md](docs/THEMES.md)**.
- **Pure Rust.** It builds with `cargo` alone: no C compiler, no system `-dev` packages. GPU drivers, Wayland and X11
  are loaded at runtime.
- **AI-ready, opt-in.** A built-in [MCP](https://modelcontextprotocol.io) server lets Claude Code, Claude Desktop or
  Cursor read and edit your open buffers. Every edit is one undo step, and nothing is saved without you. It is off by
  default.
- **Updates itself.** shrew checks GitHub Releases once a day and shows an update button in the status bar. The
  download is verified against its sha256 before it replaces the binary.

## Install

**Linux and macOS.** This installs the `srw` command (and a `shrew` symlink to it) to `~/.local/bin`, verifies the
checksum, and on Linux adds shrew to the application menu:

```sh
curl -fsSL https://raw.githubusercontent.com/perunok/shrew/main/scripts/install.sh | sh
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

**From source.** See [Building from source](#building-from-source).

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

## Features

Plain editing is always there: rope buffers, unlimited undo/redo with grouped edits, multiple cursors, regex
search and `:s` replace, and the vim and Sublime keymaps. Everything else is an optional feature. Every optional
feature:

- shows up in the **Features** list (`:features`, or palette *Features: List and Toggle*), where Enter toggles it;
- can be switched **at runtime** with `:set <key>`, `:set no<key>` or `:set <key>!` (toggle). Changes made this way
  are written back to `config.toml`, and your comments in the file are kept;
- can be set **in the config** under `[features]`, as `<key> = true|false`;
- and, where a cargo feature is listed, can be **compiled out** entirely. That removes its code and dependencies and
  makes the binary smaller.

A feature that is off is never started: no thread, no file watcher, no socket, no memory.

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

**Compile-time.** The `shrew` package's (the `srw` binary's) default cargo features are `syntax`, `picker`, `clipboard`, `mcp`, `update`
and `embed-font`. A minimal build:

```sh
cargo build --release -p shrew --no-default-features                     # just the editor
cargo build --release -p shrew --no-default-features --features syntax   # plus highlighting
```

A feature that is compiled out shows as unavailable in the Features list, and `:set` on it explains why. With
`embed-font` off, shrew uses a system monospace font instead of the bundled JetBrains Mono (about 830 KB).
`shrew-syntax` also has an opt-in feature: `sublime-user` loads extra `.sublime-syntax` files from Sublime's
`Packages/User`. Color schemes (`.sublime-color-scheme`, `.tmTheme`) are always readable: see
[docs/THEMES.md](docs/THEMES.md).

## Keybindings

These are the most useful ones. The full reference, generated from the code, is in
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
claude mcp add shrew -- shrew --mcp-stdio
```

**Claude Desktop, Cursor and other clients.** Add this to the client's MCP config and restart the client. For Claude
Desktop that is `~/.config/Claude/claude_desktop_config.json` on Linux,
`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS and
`%APPDATA%\Claude\claude_desktop_config.json` on Windows. For Cursor it is `~/.cursor/mcp.json`.

```json
{
  "mcpServers": {
    "shrew": { "command": "shrew", "args": ["--mcp-stdio"] }
  }
}
```

The client starts `shrew --mcp-stdio`, a small bridge that connects to the running editor and starts shrew if it is
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

## Architecture

shrew is a Cargo workspace of small crates. Only the app crate knows about windows.

| Crate | Role |
| --- | --- |
| [`shrew-core`](crates/shrew-core) | The editing engine: rope buffers (ropey), multi-cursor selections, undo history, vim and Sublime keymaps, user mappings, search, ex commands, pickers. No GPU, no windowing, fully unit-tested. |
| [`shrew-render`](crates/shrew-render) | GPU renderer on wgpu (Vulkan, Metal, DX12, GL): cosmic-text shaping, a cached glyph atlas, instanced quads. Can render headless to PNG. |
| [`shrew-syntax`](crates/shrew-syntax) | Incremental syntax highlighting with Sublime grammars (syntect, fancy-regex) on one worker thread. |
| [`shrew-theme`](crates/shrew-theme) | Themes as data: 33 built-in UI + syntax palettes, 18 accents, custom themes, Sublime/TextMate scheme import, WCAG contrast audit. |
| [`shrew-config`](crates/shrew-config) | `config.toml` (comment-preserving writes), and the vimrc and Sublime Text importers. |
| [`shrew-mcp`](crates/shrew-mcp) | MCP server: a loopback Streamable-HTTP endpoint inside the app and the `--mcp-stdio` bridge. |
| [`shrew-update`](crates/shrew-update) | Self-update from GitHub Releases: pure-Rust TLS (rustls + RustCrypto), sha256 check, atomic swap. |
| [`shrew`](crates/shrew) | The app: winit window and event loop, wiring the crates together. |

## Building from source

You need a stable Rust toolchain (edition 2024) and nothing else. There is no C compiler, `pkg-config` or `-dev`
package to install on any platform.

```sh
git clone https://github.com/perunok/shrew
cd shrew
cargo build --release -p shrew
./target/release/srw
```

```sh
cargo test --workspace                 # unit tests
cargo clippy --workspace --all-targets
scripts/check-pure-rust.sh             # fails if any dependency compiles or links C
```

At runtime, Linux needs a Vulkan or OpenGL (EGL) driver and a Wayland or X11 session. Those libraries are loaded
with `dlopen` when shrew starts, so they are not needed to build it.

## Performance notes

- **Redraw on demand.** There is no render loop. A frame is drawn only after input, a background result (syntax, file
  list, MCP edit) or an animation step, and animations stop as soon as they settle. An idle shrew uses no CPU.
- **Glyph atlas.** Each glyph is rasterised once into a GPU texture atlas (etagere packing). A frame is a single
  instanced draw of quads, with no per-frame text rasterisation or texture uploads.
- **Line layout cache.** Shaped lines are cached and reused until their text or style changes, so scrolling and
  cursor movement do not re-shape text.
- **Background syntax.** Grammars load lazily on a worker thread. Highlighting is incremental: after an edit it
  restarts at the edited line and stops as soon as the parser state matches the previous run. Lines that are not
  highlighted yet are drawn plain instead of blocking the frame.
- **Cheap edits.** A rope makes edits O(log n) even in very large files. Undo stores change sets, not snapshots.
- **Fast start.** The font is embedded, so the first frame never waits for a font scan. Optional features are not
  initialised when off. The release profile uses thin LTO, one codegen unit, `panic = "abort"` and stripped symbols.

## License

[MIT](LICENSE) © 2026 perunok. The bundled JetBrains Mono font is under the
[SIL Open Font License](crates/shrew-render/assets/OFL.txt).
