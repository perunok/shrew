# Configuration

shrew is configured with one TOML file, `config.toml`. You can leave it empty: every setting has a default. shrew
also picks up what it can from your vimrc and from Sublime Text.

| OS | Path |
| --- | --- |
| Linux | `$XDG_CONFIG_HOME/shrew/config.toml` (usually `~/.config/shrew/config.toml`) |
| macOS | `~/Library/Application Support/shrew/config.toml` |
| Windows | `%APPDATA%\shrew\config.toml` |

To open the file, use palette *Shrew: Open Config File* (`:config`). If the file does not exist yet, it is created
with every setting commented out at its default value, each with a one-line explanation, so you can uncomment what
you want to change.

**Layers.** Settings stack, lowest first:

1. built-in defaults
2. Sublime Text user settings and key bindings ([`[import] sublime`](#import))
3. your vimrc ([`[import] vimrc`](#import))
4. `config.toml`

A layer only overrides what it actually sets. If your vimrc says `set ts=2` and `config.toml` does not mention
`tab_width`, you get 2.

**Mistakes never stop shrew.** A wrong type, an out-of-range value or an unknown key costs a warning with its line
number, and the rest of the file still applies. Only a file that is not valid TOML at all is ignored as a whole.
Warnings show in the status line at startup.

**shrew writes to the file too, carefully.** When you toggle a feature at runtime (`:set nosyntax`, the Features
list), pick a theme, accent or font, or change the font size, shrew updates that one value in `config.toml`, so the
choice survives a restart. Your comments, ordering and formatting are kept.

**Contents:** [`[editor]`](#editor) · [`[theme]`](#theme) · [`[font]`](#font) · [`[features]`](#features) ·
[`[keys]`](#keys) · [`[import]`](#import) · [`[mcp]`](#mcp) · [`[update]`](#update) · [`[titlebar]`](#titlebar) ·
[`[sidebar]`](#sidebar) · [Full example](#full-example)

## `[editor]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `tab_width` | integer 1–16 | `4` | Columns per indent level, and how wide a tab is drawn. Runtime: `:set ts=N` |
| `expand_tab` | bool | `true` | Indent with spaces (`true`) or tabs (`false`). Runtime: `:set et` / `:set noet` |
| `scrolloff` | integer 0–999 | `4` | Lines kept visible above and below the cursor. Runtime: `:set so=N` |
| `cursor_blink` | bool | `false` | Blink the cursor. Off by default, because blinking needs regular redraws |
| `leader` | string | `"\\"` | The key `<leader>` stands for in mappings: `"\\"` (vim's default), `","`, `" "` or `"<Space>"` |
| `mapping_timeout_ms` | integer 0–10000 | `1000` | How long to wait for the rest of a mapping such as `jk` before taking the keys as typed (vim's `timeoutlen`) |

```toml
[editor]
tab_width = 2
leader = " "
```

The keys `font_family`, `font_size`, `line_height` and `theme` used to live here and still work in `[editor]`;
`[font]` and `[theme]` are their home now (a value set in both places: the later one in the file wins). When shrew
saves a font size or theme, it updates the key where your file has it.

## `[theme]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `name` | string | `"shrew-dark"` | A built-in theme id (`dracula`, `nord`, `catppuccin-mocha`, `github-light`, ... see [THEMES.md](THEMES.md#built-in-themes)), a custom theme from the `themes` folder next to this file, a path to a `.sublime-color-scheme` / `.tmTheme`, or `"system"` to follow the desktop's light/dark setting. Runtime: palette *Theme: Select…*, `:theme <id>` |
| `accent` | string | `"theme"` | An accent over the theme: `"theme"` (its own), `blue`, `indigo`, `violet`, `purple`, `fuchsia`, `pink`, `rose`, `red`, `orange`, `amber`, `yellow`, `lime`, `green`, `emerald`, `teal`, `cyan`, `sky`, `slate`. Runtime: palette *Accent: Select…*, `:accent <id>` |
| `system_light` | string | `"shrew-light"` | With `name = "system"`: the theme for a light desktop |
| `system_dark` | string | `"shrew-dark"` | With `name = "system"`: the theme for a dark desktop |

Which ids exist depends on your `themes` folder, so an unknown theme or accent is reported when shrew starts (it then
uses `shrew-dark`), not by the config parser. Custom themes, importing and the contrast rules are in
[THEMES.md](THEMES.md).

```toml
[theme]
name = "system"
accent = "teal"
system_light = "github-light"
system_dark = "tokyo-night"
```

## `[font]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `family` | string | `"monospace"` | Font family. `"monospace"` is the bundled JetBrains Mono, which also draws the first frame; system families take over when the background font scan finishes, and JetBrains Mono stays when the family is missing. Runtime: palette *Font: Select…*, `:font <family>` |
| `size` | number 4–200 | `15.0` | Font size in points. Runtime: Ctrl+= / Ctrl+- (saved here), Ctrl+0 back to 15 |
| `line_height` | number 0.5–4 | `1.4` | Line height as a multiple of the font size |
| `weight` | string or 100–900 | `"regular"` | Weight of normal text: `"thin"`, `"extralight"`, `"light"`, `"regular"`, `"medium"`, `"semibold"`, `"bold"`, `"extrabold"`, `"black"`, or a number. Bold text is 300 heavier. A family without that weight uses its nearest one |
| `ligatures` | bool | `true` | Programming ligatures (`->`, `!=`, `>=`) in fonts that have them. Runtime: `:set ligatures` / `:set noligatures` |

```toml
[font]
family = "Iosevka"
size = 14
weight = "medium"
ligatures = false
```

## `[features]`

Every optional feature, `<key> = true | false`. A feature that is off is never started, so it costs nothing. The
README has [the full table with defaults](../README.md#features). The keys are:

`vim`, `syntax`, `bracket_match`, `auto_pairs`, `auto_indent`, `line_numbers`, `relative_numbers`, `current_line`,
`indent_guides`, `whitespace`, `search_highlight`, `word_highlight`, `smooth_scroll`, `cursor_animation`, `picker`,
`palette`, `clipboard`, `mcp`, `update_check`, `vimrc`, `sublime_import`.

```toml
[features]
vim = false              # Sublime-style: always typing
relative_numbers = true
smooth_scroll = false
cursor_animation = false
mcp = true               # let AI assistants in (see README, "AI assistants")
```

At runtime the same keys work with `:set <key>`, `:set no<key>` and `:set <key>!`, and in the Features list
(`:features`). Keys also work without underscores (`linenumbers`). A feature compiled out of your build (see the
README's cargo features) stays off whatever the config says, and shrew tells you so.

## `[keys]`

Key mappings work like vim's and are kept in one table per mode:

| Table | Mode | vim equivalent |
| --- | --- | --- |
| `[keys.normal]` | Normal mode | `nnoremap` |
| `[keys.insert]` | Insert mode, and everything when the `vim` feature is off | `inoremap` |
| `[keys.visual]` | Visual, visual line and visual block | `xnoremap` |
| `[keys.operator]` | After an operator (`d`, `c`, `y`, `>`, `<`, `g~`, `gu`, `gU`) | `onoremap` |

The short names `n`, `i`, `v`/`x` and `o` work too.

**Left-hand side.** The keys to press, in vim notation:

| Notation | Key |
| --- | --- |
| `jk`, `gx`, `Y` | Those characters in sequence (case matters) |
| `<C-x>`, `<A-x>` / `<M-x>`, `<S-x>`, `<D-x>` | Ctrl, Alt, Shift or Super (⌘) with `x`; they combine: `<C-S-p>` |
| `<leader>`, `<localleader>` | The leader key (`[editor] leader`) |
| `<Esc>`, `<CR>` / `<Enter>`, `<Tab>`, `<BS>`, `<Del>`, `<Space>` | Special keys |
| `<Up>` `<Down>` `<Left>` `<Right>`, `<Home>` `<End>`, `<PageUp>` `<PageDown>`, `<Insert>` | Movement keys |
| `<F1>` … `<F12>` | Function keys |
| `<lt>`, `<Bar>`, `<Bslash>` | `<`, `\|`, `\` |

**Right-hand side.** What the mapping does:

| Value | Meaning | Example |
| --- | --- | --- |
| a command name | Run that [named command](KEYBINDINGS.md#named-commands) | `"files"`, `"duplicate_line"` |
| `":"` + ex command | Run an [ex command](KEYBINDINGS.md#ex-commands) directly, without opening the prompt (a trailing `<CR>` is optional) | `":w"`, `":%s/\\s+$//"` |
| anything else | Keys to type, in vim notation. They are not remapped, like `noremap` | `"y$"`, `"<Esc>"`, `"<C-w>"` |
| `""` or `"<Nop>"` | Do nothing: disable the key | |

A lowercase word with underscores that is not a command name (a typo such as `"dupliate_line"`) is reported as an
unknown command rather than typed as keys.

```toml
[keys.normal]
"<leader>w" = ":w"
"<leader>q" = ":q"
"<leader>f" = "files"
"<leader>b" = "buffers"
"<C-h>" = "prev_buffer"
"<C-l>" = "next_buffer"     # replaces the default Ctrl+L (select line) in normal mode
"Y" = "y$"
"Q" = ""                     # disable

[keys.insert]
"jk" = "<Esc>"
"<C-s>" = ":w"

[keys.visual]
"<" = "<gv"                  # keep the selection after indenting
">" = ">gv"

[keys.operator]
"L" = "$"                    # dL deletes to the end of the line
```

**Precedence.** Your mappings are checked before every built-in binding, including the Sublime-style chords. Within
mappings, `config.toml` wins over mappings imported from a vimrc, and those win over imported Sublime bindings. If
one mapping is the start of a longer one (`j` and `jk`), shrew waits `mapping_timeout_ms` for the next key, as vim
does.

**Vim mapping commands** such as `nnoremap <leader>w :w<CR>`, `imap`, `vnoremap`, `xmap`, `omap`, `noremap`,
`unmap`, `mapclear` and `map!` are read from your vimrc with `<silent>`, `<buffer>` and similar attributes ignored.
Mappings that run vim script (`<expr>`, `<Plug>`, `:call`, `:lua`) are skipped and counted in the import summary.

## `[import]`

At startup shrew reads settings and mappings from vim and Sublime Text, so the editor already feels like yours.

| Key | Values | Default | What it reads |
| --- | --- | --- | --- |
| `vimrc` | `"auto"`, `"off"` or a file path | `"auto"` | The first of `~/.vimrc`, `~/.vim/vimrc`, `~/_vimrc` and `~/vimfiles/vimrc` (Windows), and `~/.config/nvim/init.vim` (`%LOCALAPPDATA%\nvim\init.vim` on Windows). `init.lua` is not supported |
| `sublime` | `"auto"`, `"off"` or a `Packages/User` directory | `"auto"` | `Preferences.sublime-settings` and `Default (<OS>).sublime-keymap` from Sublime Text 4 or 3 |

`true` and `false` mean `"auto"` and `"off"`. Paths may start with `~/`. The features `vimrc` and `sublime_import`
switch the importers off entirely (`[features] vimrc = false`).

**From a vimrc** shrew takes:

| vim | shrew |
| --- | --- |
| `set tabstop` / `shiftwidth` / `softtabstop` / `expandtab` | `tab_width`, `expand_tab` |
| `set number`, `relativenumber`, `hlsearch`, `list`, `cursorline`, `autoindent`, `showmatch` | the matching [features](#features) |
| `set scrolloff`, `timeoutlen` | `scrolloff`, `mapping_timeout_ms` |
| `set background=light\|dark` | `[theme] name = "shrew-light"` / `"shrew-dark"` |
| `set clipboard=unnamed` / `unnamedplus` | the `clipboard` feature |
| `set guifont=…` | `[font] family`, `size` |
| `syntax on` / `off` | the `syntax` feature |
| `let mapleader`, `let maplocalleader` | `leader` |
| the `map` / `noremap` / `unmap` / `mapclear` family | [mappings](#keys) |

Options that do not matter in shrew (`nocompatible`, `encoding`, `backspace`, `hidden`, `incsearch` and so on) are
accepted silently. Anything else is skipped: plugins, `autocmd`, functions, `if` blocks, `colorscheme`, unknown
options. shrew counts the skipped lines and shows one summary with a few examples, such as
`vimrc: imported 14 setting(s) from ~/.vimrc; 3 unsupported, skipped (12: Plug 'tpope/vim-surround' | ...)`.

**From Sublime Text** shrew takes:

| Sublime setting | shrew |
| --- | --- |
| `tab_size`, `translate_tabs_to_spaces` | `tab_width`, `expand_tab` |
| `font_face`, `font_size` | `[font] family`, `size` |
| `line_numbers`, `highlight_line`, `auto_match_enabled`, `auto_indent`, `match_brackets`, `draw_indent_guides`, `draw_white_space` | the matching [features](#features) |
| `ignored_packages` without `"Vintage"` | `vim = true` (Vintage is Sublime's vim mode) |
| `color_scheme`, `theme` | the built-in theme of the same name (Monokai, Dracula, Solarized, Gruvbox, Catppuccin, Nord, One Dark, Ayu, Tokyo Night, ...), else the light or dark shrew theme |

Key bindings from `Default (Linux|OSX|Windows).sublime-keymap` are translated into shrew commands: save, the
multi-cursor commands, `swap_line_up`/`down`, `toggle_comment`, `show_overlay`, `move`/`move_to`, `find_under_expand`
and more. They go into insert mode, which is all of shrew when vim is off. They also go into normal and visual mode
when the chord cannot shadow a vim command: Alt and Super chords, Ctrl+Shift chords, function keys, and Ctrl keys vim
does not use. Bindings with a `context` are conditional in Sublime, so they are skipped.

## `[mcp]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `port` | integer 1–65535 | `7356` | Port of the MCP endpoint on `127.0.0.1`, when the `mcp` feature is on |

The server itself is switched by the `mcp` feature (off by default). The endpoint listens on loopback only and needs
a token, which shrew keeps in `~/.config/shrew/mcp/`. Clients normally use the `shrew --mcp-stdio` bridge, which
finds the port and token by itself. See the README's [AI assistants](../README.md#ai-assistants-mcp) section.

## `[update]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `check` | bool | `true` | Check GitHub Releases for a newer shrew at startup (at most once a day). Same as the `update_check` feature |

Installing an update is never automatic: shrew shows an update button in the status bar and waits for you. A check
you ask for (Ctrl+Shift+U, palette *Shrew: Check for Updates*) works even when `check = false`.

These environment variables are for testing and mirrors:

| Variable | Effect |
| --- | --- |
| `SHREW_UPDATE_REPO` | GitHub `owner/name` to take releases from (default `perunok/shrew`) |
| `SHREW_UPDATE_FEED_URL` | Any URL serving GitHub's "latest release" JSON |

## `[titlebar]`

The custom title bar (feature `custom_titlebar`, on by default) replaces the system's: shrew draws the top row with the
buffer tabs in the middle and these items around them. Turn the feature off (`:set nocustom_titlebar`, palette
*View: Toggle System Title Bar*, or right-click the bar > *Use System Title Bar*) to get the system title bar back
at once; the tabs then move back to their own row. On macOS the traffic lights stay native (switching applies when
shrew restarts there).

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `left` | list of items | `["menu", "sidebar", "project", "tabs"]` | Items at the left, in order |
| `right` | list of items | `["files", "terminal", "settings", "about", "minimize", "maximize", "close"]` | Items at the right, in order |
| `hidden` | list of items | `[]` | Items not shown |

| Item | What it is |
| --- | --- |
| `menu` | The menu button: File, Edit, Selection, View, Go, Terminal, Help (also F10, Alt+F) |
| `sidebar` | Toggles the sidebar (`toggle_sidebar`) |
| `project` | The project's name; a click opens the recent projects (or the file picker) |
| `tabs` | The buffer tabs; they take the room the other items leave |
| `files` | Go to File (Ctrl+P) |
| `terminal` | Toggles the terminal panel (`toggle_terminal`) |
| `settings` | Settings (`settings`) |
| `about` | About shrew |
| `minimize`, `maximize`, `close` | The window buttons (not on macOS, which keeps its own) |

An item whose command this build lacks is not drawn. An item none of the lists mentions goes where the default puts
it, a name given twice keeps its first place, and unknown names are skipped with a warning in the log. `tabs`,
`settings` and the window buttons can move anywhere but not be hidden: a window must stay closable. Neighbouring
window buttons sit together, and one at the very edge sits in the window's corner, so this gives macOS order:

```toml
[titlebar]
left = ["close", "minimize", "maximize", "menu", "project", "tabs"]
```

## `[sidebar]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `width` | number 80–2000 | `250` | Width of the file tree in logical pixels. Dragging the sidebar's edge saves it here |

Whether the sidebar shows is the `sidebar` feature (Ctrl+B toggles it and saves that too). Recent files and projects
are kept outside this file, in `<cache dir>/shrew/recent` (`~/.cache/shrew/recent` on Linux); turn the `recent`
feature off to stop that.

## Full example

```toml
# ~/.config/shrew/config.toml

[editor]
tab_width = 4
expand_tab = true
scrolloff = 6
cursor_blink = false
leader = " "
mapping_timeout_ms = 500

[theme]
name = "shrew-dark"
accent = "theme"

[font]
family = "JetBrains Mono"
size = 15
line_height = 1.4
weight = "regular"
ligatures = true

[features]
relative_numbers = true
whitespace = false
cursor_animation = false
mcp = false

[keys.normal]
"<leader>w" = ":w"
"<leader>f" = "files"
"<leader>p" = "palette"
"<leader>/" = "toggle_comment"
"<C-d>" = "<C-d>zz"          # keep the cursor centered while paging

[keys.insert]
"jk" = "<Esc>"

[keys.visual]
"<" = "<gv"
">" = ">gv"

[import]
vimrc = "auto"
sublime = "off"

[mcp]
port = 7356

[update]
check = true
```
