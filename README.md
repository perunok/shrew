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

Every shot is the real editor, rendered headless from shrew's own source code (the git ones from a small demo
repository, `scripts/demo-repo.sh`). Regenerate them with `scripts/screenshots.sh`.

| | |
|---|---|
| ![Normal mode](docs/screenshots/normal.png)<br>**Vim normal mode** with syntax highlighting, indent guides and `*` search | ![Multiple cursors](docs/screenshots/multi-cursor.png)<br>**Ctrl+N** adds a cursor at each next match, as in vim-visual-multi |
| ![Sublime mode](docs/screenshots/sublime.png)<br>**Sublime mode** (`:set novim`): Ctrl+D five times, then type | ![File picker](docs/screenshots/file-picker.png)<br>**Ctrl+P** fuzzy-finds files, respecting `.gitignore` |
| ![Command palette](docs/screenshots/palette.png)<br>**Ctrl+Shift+P** runs any command by its name | ![Features](docs/screenshots/features.png)<br>**Every feature is optional**: `:features` lists and toggles them |
| ![Theme picker](docs/screenshots/theme-picker.png)<br>**33 themes** with a live preview as you move through them | ![Light theme](docs/screenshots/light.png)<br>**Light themes** too, here One Light in visual line mode |
| ![High contrast](docs/screenshots/contrast.png)<br>**High Contrast** (WCAG AAA); every theme passes a contrast audit | ![Substitute](docs/screenshots/substitute.png)<br>**`:%s` substitution**, with the last search highlighted |
| ![About](docs/screenshots/about.png)<br>**About shrew** and the Shrew Manifesto (`:about`) | ![Huge file](docs/screenshots/huge-file.png)<br>**A 50 MB, million-line file**, searched at line 500,000 |
| ![Sidebar](docs/screenshots/sidebar.png)<br>**The sidebar** (Ctrl+B): the project's tree, following the active file | ![Find in project](docs/screenshots/find-in-project.png)<br>**Find in project** (Ctrl+Shift+F), grouped by file as results stream in |
| ![Symbols](docs/screenshots/symbols.png)<br>**Go to symbol** (`@` in Ctrl+P, Ctrl+R) from the grammar's scopes | ![Terminal](docs/screenshots/terminal.png)<br>**The built-in terminal** (Ctrl+\`): colors from the theme, Ctrl+click opens `file:line` |
| ![Vim in the terminal](docs/screenshots/terminal-vim.png)<br>**Full-screen programs** work: vim inside shrew's terminal, One Light | ![Settings](docs/screenshots/settings.png)<br>**Settings** (Ctrl+,): every setting by section, saved to `config.toml` |
| ![Settings in a light theme](docs/screenshots/settings-light.png)<br>**Keys or mouse**: switches, sliders, `/` to search, `r` to reset | ![Everything at once](docs/screenshots/workspace.png)<br>**Everything at once**: title bar, sidebar, minimap and terminal |
| ![Title bar menu](docs/screenshots/menu.png)<br>**The title bar** holds the tabs, the menus (F10, Alt+V) and the window buttons | ![Minimap](docs/screenshots/minimap.png)<br>**The minimap**: the file in its syntax colors, click or drag to scroll, Tokyo Night |
| ![Git gutter](docs/screenshots/git-gutter.png)<br>**Git in the gutter**: added, changed and deleted lines, the cursor line's author, file states in the sidebar | ![Git peek](docs/screenshots/git-peek.png)<br>**`ghp` peeks at a change** with the changed words marked; Stage, Revert and Diff are one key or click away |
| ![Git diff](docs/screenshots/git-diff.png)<br>**The diff view**: side by side, aligned, with changed words and folded context; stage a change with the round button or `s` | ![Source Control](docs/screenshots/git-panel.png)<br>**Source Control** (Ctrl+Shift+G): staged, changed and untracked files, the commit box, sync |
| ![Git log](docs/screenshots/git-log.png)<br>**The log** draws the commit graph, branch and tag labels; Enter shows a commit's files | ![Git blame](docs/screenshots/git-blame.png)<br>**Blame** as a column: who changed each run of lines, how long ago, unsaved lines marked |
| ![Tabs](docs/screenshots/tabs.png)<br>**Tabs**: hover, ×, middle-click, drag to reorder, the wheel scrolls them; a right click on one closes others, copies its path, reveals it | ![Context menu](docs/screenshots/context-menu.png)<br>**Right-click anywhere**: the text, gutter, tabs, sidebar, status bar, terminal, diffs, the log; Shift+F10 opens it from the keys |

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
- **Settings panel.** Ctrl+, (or `:settings`, or the gear in the status bar) shows every setting by section:
  appearance, editor, features, keybindings, imports, AI assistants, updates. Keys or mouse, `/` searches them all,
  `r` resets one, and every change is saved to `config.toml` with your comments kept.
- **Pickers.** A fuzzy file finder (Ctrl+P) that walks the project in the background and respects `.gitignore`, a
  command palette (Ctrl+Shift+P), a buffer list (Ctrl+Shift+B) and the Features list.
- **A title bar of its own.** The tabs, the menus (F10, Alt+F) and the window buttons share the top row, arranged
  as you like (`[titlebar]`). Drag it to move the window, double-click to maximize, or switch back to the system
  title bar with one toggle.
- **Minimap.** A Sublime-style overview at the right of the text: every line as thin blocks in its syntax colors,
  the visible lines boxed, cursors, selections and search matches marked. Click to jump, drag to scroll. It is
  built from the lines on show and cached, so an unchanged frame costs nothing.
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
- **Git.** The gutter marks what changed against the index (or HEAD) as you type, in the gutter, the minimap and the
  scrollbar. `]c` / `[c` (Ctrl+. / Ctrl+Shift+.) move between changes, `ghp` peeks at the old lines with the changed
  words marked, `ghs` / `ghu` / `ghr` stage, unstage and revert the change or the selected lines. The cursor line's
  author shows faintly at its end after a pause, the branch sits in the status bar and the title bar, and the
  sidebar colours files by their state. See **[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md#git)**.
- **Source Control.** Ctrl+Shift+G opens it in the sidebar: stage, unstage and discard files, write the commit
  message (Ctrl+Enter commits), switch, create and delete branches, stash, fetch, pull and push with progress and
  cancel. The log tab draws the commit graph and loads as you scroll, blame shows who changed each line, and merge
  conflicts get *Accept Current / Incoming / Both* on each one. See
  [KEYBINDINGS.md](docs/KEYBINDINGS.md#source-control).
- **Git diffs.** A diff tab (Enter on a change in Source Control, `:Gdiff`, the Git menu, the palette) shows a file,
  the staged or unstaged changes, a commit or two branches side by side or unified, with syntax colors, changed words
  marked and unchanged lines folded. It follows your unsaved edits as you type; `s`, `u` and `X` stage, unstage or
  revert a change or the selected lines. See [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md#diff-view). How it all fits
  together, and what to check when something looks wrong, is in **[docs/GIT.md](docs/GIT.md)**.

  All of it reads the repository inside shrew (gix, pure Rust) off the UI thread, and writes through the `git`
  program, so your hooks and credentials just work. Nothing runs outside a repository.
- **Updates itself.** At every start and once a day shrew asks GitHub for the latest release and says when there is
  a newer one. **Update now** shows the release notes and downloads it with a progress bar, checks it against its
  sha256, and **Restart to update** brings every open file back, unsaved changes too.
- **Pure Rust.** It builds with `cargo` alone: no C compiler, no system `-dev` packages. GPU drivers, Wayland and
  X11 are loaded at runtime.

### Switchable features

Plain editing is always there: rope buffers, unlimited undo/redo with grouped edits, multiple cursors, regex search
and `:s` replace, and the vim and Sublime keymaps. Everything else is an optional feature. Every optional feature:

- shows up in the **Features** list (`:features`, or palette *Features: List and Toggle*), where Enter toggles it, and
  in the Settings panel (Ctrl+,);
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
| `update_check` | Check GitHub for new releases at startup and once a day | on | `update` |
| `vimrc` | Read options and mappings from `~/.vimrc` or `init.vim` at startup (see `[import]`) | on | |
| `sublime_import` | Read Sublime Text's user settings and key bindings at startup (see `[import]`) | on | |
| `custom_titlebar` | shrew's own title bar: tabs, menus (F10) and window buttons in one row. Off: the system title bar | on | |
| `sidebar` | File tree of the project on the left (Ctrl+B, Ctrl+Shift+E) | off | `project` |
| `compact_folders` | Sidebar: a folder holding only one folder shares its row with it (`src/main/java`) | on | `project` |
| `project_search` | Find in project (Ctrl+Shift+F, `:grep`), respects `.gitignore` | on | `project` |
| `symbols` | Go to symbol in the file (Ctrl+R, `@` in Ctrl+P) and the project (Ctrl+Shift+R) | on | `syntax` |
| `recent` | Remember recent files and projects (Open Recent, Ctrl+Alt+P) in the cache dir | on | |
| `minimap` | Sublime-style minimap at the right of the editor: click or drag to scroll (see `[minimap]`) | on | |
| `terminal` | Built-in terminal panel (Ctrl+\`), started on first use (see `[terminal]`) | on | `terminal` |
| `settings` | Settings panel (Ctrl+,, `:settings`) and the gear in the status bar | on | |
| `git_gutter` | Git change markers in the gutter, minimap and scrollbar; `]c` / `[c`, peek, stage, unstage and revert changes (see `[git]`) | on | `git` |
| `git_blame_inline` | Who last changed the cursor line, faintly at its end after a pause | on | `git` |
| `git_status` | The branch in the status bar and title bar, file states in the sidebar | on | `git` |
| `git_panel` | Source Control (Ctrl+Shift+G): stage, commit, branches, stashes, fetch / pull / push, the log and its graph, blame, merge conflicts | on | `git` |

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
`clipboard`, `mcp`, `update`, `embed-font`, `project`, `terminal` and `git`. A minimal build:

```sh
cargo build --release -p shrew --no-default-features                     # just the editor
cargo build --release -p shrew --no-default-features --features syntax   # plus highlighting
```

`git` is its own switch for everything git: without it (`--no-default-features --features syntax,mcp`, say) the
change markers, diffs, Source Control and the MCP git tools are gone, along with gix and imara-diff. A feature that
is compiled out shows as unavailable in the Features list, and `:set` on it explains why. With
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
srw .                  # this folder as the project, shown in the sidebar
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
| Show the file tree, find in project | Ctrl+B (Ctrl+Shift+E to give it the keys), Ctrl+Shift+F |
| Go to symbol, in the project | Ctrl+R (`@` in Ctrl+P), Ctrl+Shift+R |
| Open a terminal | Ctrl+\` |
| Source Control (git) | Ctrl+Shift+G |
| Settings | Ctrl+, (or the gear) |
| The menus | F10, Alt+F |
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
| Ctrl+B, Ctrl+Shift+E | Show / hide the sidebar (Ctrl+B is page up in vim normal mode), focus it |
| Ctrl+Shift+F | Find in project |
| Ctrl+R (vim off) or Ctrl+Shift+O, Ctrl+Shift+R | Go to symbol in the file, in the project |
| Ctrl+Alt+P, Ctrl+Shift+T | Open a recent project, reopen the last closed file |
| Alt+Left / Alt+Right | Back / forward along the jump list |
| Ctrl+\`, Ctrl+Shift+\` | Show / hide the terminal, new terminal |
| Ctrl+, | Settings |
| F10, Alt+F / E / S / V / G / I / T / H | Open the menus |

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

### Mouse

The keys come first, but everything works with the mouse too, the way VS Code and Sublime do it:

- **Text:** click places the cursor, double / triple click selects a word / line (drag on to extend), Shift+click
  extends, Ctrl+click or Alt+click adds a cursor, a middle drag or Shift+Alt+drag selects a box of columns, and a
  drag past an edge scrolls on. In vim a drag is visual mode as it goes.
- **Right click** opens a menu wherever it makes sense: the text (the cursor moves there first), the gutter, a tab,
  the title bar, a sidebar row, a Source Control row, the status bar, the terminal, a diff or a log tab. Every entry
  shows its shortcut; Shift+F10 opens the same menu from the keys.
- **Middle click** pastes the primary selection on Linux, closes a tab, and opens a sidebar file in the background.
- **Tabs** light up under the pointer, close with their ×, reorder by dragging, and scroll with the wheel when they
  overflow (a list button shows them all).
- **Scrolling:** the wheel and touchpads (Shift+wheel sideways, Ctrl+wheel zooms), the scrollbar, the minimap; a
  menu taller than the window scrolls too. Drag the sidebar's or the terminal's edge to resize them, drag the title
  bar to move the window.
- **Overlays** (the Update shrew dialog, a toast) take the pointer: their buttons light up, and nothing under them
  takes a click or opens a menu. One opens at a time: the dialog closes an open menu, a title bar menu closes it.

Every click, drag and menu is listed in [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md#mouse).

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

[sidebar]
width = 250               # drag its edge and this follows

[terminal]
shell = "/bin/zsh"        # default: $SHELL
height = 12
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
the cursor. In a git repository they also get five read-only tools (`git_status`, `git_diff`, `git_log`,
`git_blame`, `git_show_commit`); nothing an assistant calls can stage, commit or push. See
[docs/GIT.md](docs/GIT.md#assistants-mcp).

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

Each time shrew starts (a few seconds after its window opens) and once a day while it runs, it asks
[GitHub Releases](https://github.com/perunok/shrew/releases) in the background which version is the latest. When there
is a newer one, a notification at the bottom right says so, with **Update now** and **Release notes**: at every start
while you have not installed it, and at most once a day per version while shrew runs. The request carries nothing
about you or your files, and nothing is downloaded until you ask. The status bar keeps a **⬆ shrew X** button until
then. Esc or ✕ dismisses the notification; Ctrl+Shift+U opens the update (so do the status bar's right-click menu,
*Check for Updates…* / *Update shrew…*, and Help in the menus).

- **Update now** (or the status-bar button, `:update`, palette *Shrew: Update shrew…*) opens **Update shrew**: your
  version and the new one, the release notes, and **Download**. The download shows a progress bar with its size,
  speed and time left; **Cancel** stops it and deletes the partial file, **Hide** (Esc) lets it go on in the
  background while the status bar shows the percentage (click it to bring the dialog back). The download is then
  checked against the release's published sha256.
- **Restart to update** puts the new binary in place (checked once more on the way) and starts shrew again. Your
  open files come back with their cursors, selections, scroll positions, the active tab and the project, **unsaved
  changes too** ("hot exit"): they are kept in `<cache dir>/shrew/session-restart.json` across the restart and come
  back as unsaved changes, so nothing is written to your files and one undo goes back to what is on disk. Open
  terminals end (their shells cannot survive the restart; the dialog says so), the MCP server restarts, and programs
  waiting on `srw --wait` are told their files are closed. After the restart a notification says *Updated to shrew X*,
  with **What's new**.
- **Later** keeps the verified download: Settings > Updates and the status bar then offer **Restart to update**. A
  later start installs nothing by itself.
- **Help > Check for Updates…** (Ctrl+Shift+U, palette *Shrew: Check for Updates*, `:checkupdate`) checks right
  away and always answers: up to date, a newer version, or why it could not check.
- **Settings > Updates**: turn *Check for new releases automatically* off (or `[update] check = false`, or
  `:set noupdate_check`), **Check now** (the answer shows there), when shrew last checked, your version, and **Update
  now**, the progress, **Restart to update** or **View release** depending on where the update is. To remove the
  code altogether, build without the `update` cargo feature.
- Where shrew cannot update itself (installed by a package manager, a binary in a folder it may not write to, a
  development build running from cargo's `target/` folder), the notification, the dialog and Settings say why and
  offer **View release** instead, to download it from the release page.
- From a script: `srw --self-update` does the same check, download and verification without a window and replaces
  the binary; a running shrew keeps its old one until it is restarted.
- Under the hood: `scripts/release.sh X.Y.Z` bumps the version, updates the changelog and tags `vX.Y.Z`. Pushing the
  tag runs [the release workflow](.github/workflows/release.yml). It builds all five targets and drafts a GitHub
  release with the raw binaries, their `.sha256` files and the archives. Once the draft is published, running shrews
  offer the update.

## Performance

Latency comes first. These are measured on a Linux laptop (Intel Iris Xe, Wayland) with a release build of every
default feature (`srw -f --stats`, the headless `--screenshot --stats` frame benchmark at 1920x1080, and the tests
below):

| What | Measured |
| --- | --- |
| Start to first frame | about 50-65 ms (median 60 ms over 15 starts), most of it Vulkan initialisation, which runs while the window is created |
| CPU while idle | 0: no render loop and no timers (0-2 CPU ticks of 10 ms over 20 s with a file open, the same with git on or off). In a git repository one thread wakes every 3 s to compare a few file timestamps, only while the window has the focus |
| Redrawing an unchanged frame at 1080p | about 0.2 ms of CPU (0.1 ms with the minimap off) |
| A frame where every visible line is shaped anew (scrolling a page at 1080p) | about 1.1 ms |
| A 50 MB, 1,000,000-line buffer | every interactive operation far below one 16 ms frame (`crates/shrew-core/tests/perf.rs`) |
| Git: change markers while typing | the diff of a 10,000-line file takes under 1 ms on a worker thread (keystroke to marks: median 0.55-0.7 ms, p95 under 0.8 ms) and runs 30 ms after you stop typing, so a keystroke never waits for it |
| Git: status | shrew's own repository about 1 ms; 10,000 files 6-9 ms and 20,000 files 12-16 ms (35 ms with a cold file cache), about what `git status` takes; a refresh after saving a file in the 20,000-file repository 14-18 ms end to end, all off the UI thread |
| Git: log | the first 200 commits of a 20,000-commit history in 0.5 ms (1.3 ms cold), the page at row 10,000 in 7-10 ms |
| Git: blame | the first lines of a 5,000-line file blamed over 200 commits after 3-18 ms, all of it in 110-220 ms (the column fills in as `git blame` streams) |
| Git: the diff view | a full 1080p frame of a 1,100-line diff (split or unified) about 0.45 ms of CPU, a row scrolled about 0.15 ms, a page 0.4-0.5 ms |
| Git: start | no difference: first frame at a median 58 ms with a file in a repository, 55 ms outside one and 58 ms with the git features off (15 starts each); git is not touched before the first frame |
| Vim compatibility | 839 differential cases agree with Vim 9.1 (`crates/shrew-core/tests/vim_diff.rs`) |
| Binary size | 17.1 MiB on Linux x86_64 with every feature; about 12 MiB without the optional ones (see below) |

How:

- **Redraw on demand.** A frame is drawn only after input, a background result (syntax, file list, MCP edit) or an
  animation step, and animations stop as soon as they settle.
- **Glyph atlas.** Each glyph is rasterised once into a GPU texture atlas (etagere packing). A frame is a single
  instanced draw of quads, with no per-frame text rasterisation or texture uploads.
- **Line layout cache.** Shaped lines are cached and reused until their text or style changes, so scrolling and
  cursor movement do not re-shape text.
- **Background syntax.** Grammars load lazily on a worker thread. Highlighting is incremental, and lines that are
  not highlighted yet are drawn plain instead of blocking the frame.
- **Git never blocks.** Reads run in-process on a worker thread (gix and imara-diff, no `git` process for status, log or
  the change markers), writes and the network in the `git` program. Requests for the same buffer or repository
  coalesce, so typing re-diffs only the newest text, and the window thread only files replies away (a reply that
  took over 4 ms to file shows in `srw -f --stats`). The service starts on the first lookup, outside a repository
  there is nothing more to do, and unfocused it stops watching. More in [docs/GIT.md](docs/GIT.md#performance).
- **Cheap edits.** A rope makes edits O(log n) even in very large files. Undo stores change sets, not snapshots.
- **Fast start.** The GPU is opened on a background thread while the config is read and the window is created,
  glyphs are rasterised for exactly as long as that takes, and the font is embedded, so the first frame never waits
  for a font scan. Optional features are not initialised when off. The release profile uses fat LTO, one codegen
  unit, `panic = "abort"` and stripped symbols.
- **Size.** Most of the binary is the GPU stack (wgpu and its shader translator), windowing and text shaping, which
  every build needs; the optional features add the rest (self-update's pure-Rust TLS about 1.5 MB, the embedded
  font 0.8 MB, highlighting 0.75 MB, MCP 0.7 MB, the clipboard 0.5 MB, the terminal 0.4 MB). Leave out what you do
  not use with cargo features (see [Switchable features](#switchable-features)).

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
| `cargo test --release -p shrew-git --test perf -- --ignored --nocapture` | Git timings: gutter diff and keystroke latency, status of 10,000 and 20,000 files, log, blame |
| `cargo clippy --workspace --all-targets -- -D warnings` | Lints (CI fails on any warning) |
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
| [`shrew-git`](crates/shrew-git) | Git without a UI: gix reads (status, log, blame, blobs), imara-diff line and word diffs, the `git` program for writes and network, and a background service the app talks to. |
| [`shrew-update`](crates/shrew-update) | Self-update from GitHub Releases, UI-free: pure-Rust TLS (rustls + RustCrypto), download with progress and cancel, sha256 check, atomic swap. |
| [`shrew`](crates/shrew) | The app (`srw`): winit window and event loop, the single-instance `srw` command, drawing, wiring the crates together. |

## Documentation

- [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md): every key, ex command and named command.
- [docs/CONFIG.md](docs/CONFIG.md): every `config.toml` setting, and the vimrc and Sublime import.
- [docs/THEMES.md](docs/THEMES.md): themes, accents, custom themes, importing color schemes, fonts.
- [docs/GIT.md](docs/GIT.md): git in shrew: how it works, what each view does, performance, troubleshooting.
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
