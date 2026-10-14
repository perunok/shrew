# Keybindings

This is every key shrew knows by default. It was written from the source: global chords in `Editor::binding`, insert
mode in `insert_key` and `caret_move`, prompts in `prompt_key`, pickers in `picker_key`, ex commands in `run_ex`
(`crates/shrew-core/src/editor.rs`), and vim in `parse` / `parse_motion` (`crates/shrew-core/src/vim.rs`). If you
change a binding there, update this file too.

**Notation.** `Ctrl+X` is a chord. `gg` means press `g`, then `g`. `{motion}`, `{char}` and `{reg}` stand for any
motion, character or register name. "Ctrl" also means ⌘ Cmd on macOS. Vim keys are case-sensitive: `G` is Shift+g.

**Order of precedence** for a key press:

1. An open picker (file finder, palette, buffers, features), the About panel or a prompt (`:`, `/`, `?`) takes
   every key.
2. Your mappings (`[keys.*]` in `config.toml`, imported vimrc and Sublime bindings) come next. See
   [CONFIG.md](CONFIG.md#keys).
3. Then the global chords below, unless a vim command is half-typed (after `d`, `"a` or `f`, the next key belongs
   to vim).
4. Last comes the mode: vim normal or visual, or insert. With the `vim` feature off, shrew is always in insert mode.

**Contents:** [Global chords](#global-chords) · [Insert mode](#insert-mode) · [Normal mode](#normal-mode) ·
[Visual mode](#visual-mode) · [Prompts](#prompts--and-) · [Pickers](#pickers) · [About panel](#about-panel) ·
[Ex commands](#ex-commands) ·
[Named commands](#named-commands) · [Mouse](#mouse) · [Terminal](#terminal)

## Global chords

These are Sublime Text-style and work in every mode. In the **When** column, "insert" means vim insert mode or vim
turned off, and "normal" means vim normal mode.

### Files and buffers

| Keys | Action | When |
| --- | --- | --- |
| Ctrl+S | Save | always |
| Ctrl+Shift+S | Save all | always |
| Ctrl+Q | Quit (refused while a buffer is unsaved: save, or `:q!` to discard) | always |
| Ctrl+P | Go to file: fuzzy finder, respects `.gitignore` | always |
| Ctrl+O | Go to file | insert |
| Ctrl+N | New scratch buffer | insert |
| Ctrl+W | Close buffer | vim off (in vim insert mode it deletes the word before the cursor) |
| Ctrl+Shift+B | Switch buffer (picker) | always |
| Ctrl+Tab, Ctrl+PageDown | Next buffer | always |
| Ctrl+Shift+Tab, Ctrl+PageUp | Previous buffer | always |
| Ctrl+Shift+P | Command palette | always |
| Ctrl+Shift+U | Check for updates | always |

### Editing

| Keys | Action | When |
| --- | --- | --- |
| Ctrl+Z | Undo | always |
| Ctrl+Shift+Z | Redo | always |
| Ctrl+Y | Redo | insert (in normal mode, scroll up a line) |
| Ctrl+C | Copy (the whole line when nothing is selected) | insert, visual |
| Ctrl+X | Cut (the whole line when nothing is selected) | insert (in normal mode, decrement the number) |
| Ctrl+V | Paste | insert (in normal mode, visual block) |
| Ctrl+A | Select all | insert (in normal mode, increment the number) |
| Ctrl+/ | Toggle line comment (the language's token, `#` otherwise) | always |
| Ctrl+Shift+K | Delete line(s) | always |
| Ctrl+Shift+D | Duplicate line(s) | always |
| Alt+Up / Alt+Down | Move line(s) up / down | always |
| Ctrl+Shift+Up / Ctrl+Shift+Down | Move line(s) up / down | always |
| Ctrl+Enter | Insert a line below and go to it | always |
| Ctrl+Shift+Enter | Insert a line above and go to it | always |
| Ctrl+] | Indent | always |
| Ctrl+[ | Outdent | vim off (in vim it is Esc) |

### Selection and multiple cursors

| Keys | Action | When |
| --- | --- | --- |
| Ctrl+D | Select the word, then add a cursor at its next occurrence | insert (in normal mode, half page down) |
| Ctrl+N | Select the word, then add a cursor at its next occurrence | normal, visual |
| Ctrl+Shift+X | Skip this occurrence and go to the next one | normal, visual |
| Alt+F3 | Select every occurrence | always |
| Ctrl+Alt+Up / Ctrl+Alt+Down | Add a cursor on the line above / below | always |
| Alt+Shift+Up / Alt+Shift+Down | Add a cursor on the line above / below | always |
| Ctrl+L | Select the line (repeat to add the next lines) | always |
| Ctrl+Shift+L | Split the selection into one cursor per line | always |
| Ctrl+M | Go to the matching bracket | always |
| Ctrl+Shift+M | Select inside the enclosing brackets | always |
| Esc | Back to one cursor (vim off); in vim, see [normal mode](#other-commands) | |

### Search and navigation

| Keys | Action | When |
| --- | --- | --- |
| Ctrl+F | Find (incremental, regex) | insert (in normal mode, page down) |
| F3 / Shift+F3 | Next / previous match | always |
| Ctrl+G | Go to line: opens the `:` prompt, type a number | always |

### View

| Keys | Action |
| --- | --- |
| Ctrl+= or Ctrl++ | Bigger font |
| Ctrl+- | Smaller font |
| Ctrl+0 | Reset font size |
| Ctrl+\` | Show and focus the terminal panel (starting a shell the first time); from the terminal, hide it. See [Terminal](#terminal) |
| Ctrl+Shift+\` | New terminal |

## Insert mode

This is vim insert mode. With the `vim` feature off, it is the only mode.

| Keys | Action |
| --- | --- |
| any character | Insert it at every cursor. With `auto_pairs` on, brackets and quotes are closed for you |
| Enter | New line, keeping indentation (`auto_indent`) |
| Tab | Indent the selected lines if the selection spans lines. Otherwise insert spaces to the next tab stop (`expand_tab`), or a tab |
| Shift+Tab | Outdent the selected lines |
| Backspace, Ctrl+H | Delete the character before the cursor (a whole indent level inside leading spaces) |
| Ctrl+Backspace, Alt+Backspace | Delete the word before the cursor |
| Ctrl+W | Delete the word before the cursor (vim mode; with vim off it closes the buffer) |
| Ctrl+U | Delete to the start of the line (vim mode) |
| Delete | Delete the character after the cursor |
| Ctrl+Delete | Delete the word after the cursor |
| Left / Right | Move one character. With a selection, go to its start / end |
| Ctrl+Left / Ctrl+Right | Move one word |
| Up / Down, PageUp / PageDown | Move by line / by page, keeping the column |
| Home | First non-blank character; press again for column 0 |
| End | End of the line |
| Ctrl+Home / Ctrl+End | Start / end of the file |
| Shift + any of the moves above | Extend the selection |
| Esc, Ctrl+[ | Vim: back to normal mode. Vim off: back to one cursor, selection cleared |

Moving the cursor ends the current undo step, so the next typing is undone on its own.

## Normal mode

Most commands take a count: `3j`, `2dw`, `d3w`, `5x`, `3.`.

### Motions

Motions move the cursor in normal mode, extend the selection in visual mode, and give the range for an operator.

| Keys | Motion |
| --- | --- |
| `h` `l`, Left Right, Backspace, Space | Character left / right |
| `j` `k`, Down Up, `gj` `gk`, Ctrl+J | Line down / up |
| `+`, Enter / `-` | First non-blank of the next / previous line |
| `w` `b` `e` `ge` | Next word start / previous word start / word end / previous word end |
| `W` `B` `E` `gE` | The same for WORDs (separated by whitespace only) |
| `0`, `g0`, Home | Start of the line |
| `^`, `_` | First non-blank character of the line |
| `$`, `g$`, End | End of the line |
| `g_` | Last non-blank character of the line |
| `gg` / `G` | First / last line (`5G`, `5gg`: line 5) |
| `f{char}` `F{char}` | To the next / previous `{char}` on the line |
| `t{char}` `T{char}` | Till (just before) the next / previous `{char}` |
| `;` `,` | Repeat the last `f`/`t`/`F`/`T` forward / backward |
| `%` | Matching bracket |
| `}` `{` | Next / previous paragraph (blank line) |
| `H` `M` `L` | Top / middle / bottom of the screen |
| `n` `N` | Next / previous search match |
| `*` `#` | Search the word under the cursor forward / backward |
| `` `{a-z} `` / `'{a-z}` | To mark: exact position / its line |

### Operators

An operator is followed by a motion or text object, or doubled to act on whole lines (`dd`, `>>`, `gUU`).

| Keys | Operator |
| --- | --- |
| `d` | Delete |
| `c` | Change (delete, then insert mode) |
| `y` | Yank (copy) |
| `>` / `<` | Indent / outdent |
| `g~` | Toggle case |
| `gu` / `gU` | Lowercase / uppercase |

An operator can also take a search as its target: `d/foo<Enter>` deletes up to the next `foo`, and `c?bar<Enter>`
changes back to the previous `bar`.

### Text objects

Use them after an operator (`diw`, `ci"`, `ya(`) or in visual mode (`viw`). `i` means inner and `a` means around,
which takes the brackets, quotes or trailing space too. A count picks an outer pair (`2di(`).

| Keys | Object |
| --- | --- |
| `iw` `aw` | word |
| `iW` `aW` | WORD |
| `i(` `a(`, `i)` `ib` | `( … )` |
| `i{` `a{`, `i}` `iB` | `{ … }` |
| `i[` `a[`, `i]` | `[ … ]` |
| `i<` `a<`, `i>` | `< … >` |
| `i"` `a"`, `i'` `a'`, `` i` `` `` a` `` | quoted string |
| `ip` `ap` | paragraph |

### Shortcuts

| Keys | Same as | Action |
| --- | --- | --- |
| `x`, Delete | `dl` | Delete the character under the cursor |
| `X` | `dh` | Delete the character before the cursor |
| `s` | `cl` | Substitute a character |
| `S` | `cc` | Substitute the line |
| `C` | `c$` | Change to the end of the line |
| `D` | `d$` | Delete to the end of the line |
| `Y` | `y$` | Yank to the end of the line (Neovim's default) |

### Inserting

| Keys | Insert mode at |
| --- | --- |
| `i`, Insert / `a` | Before / after the cursor |
| `I` / `A` | First non-blank / end of the line |
| `gI` | Column 0 |
| `o` / `O` | A new line below / above |

### Other commands

| Keys | Action |
| --- | --- |
| `p` / `P` | Put the register after / before the cursor (line-wise registers go below / above the line) |
| `u` / Ctrl+R | Undo / redo |
| `.` | Repeat the last change (`3.` repeats it with count 3) |
| `J` / `gJ` | Join lines with / without a space |
| `r{char}` | Replace the character(s) under the cursor (`3rx`); `r` Enter splits the line |
| `~` | Toggle the case of the character(s) under the cursor and move right |
| Ctrl+A / Ctrl+X | Add / subtract the count to the number under or after the cursor |
| `v` / `V` / Ctrl+V or Ctrl+Q | Visual / visual line / visual block mode |
| `gv` | Reselect the last visual selection |
| `:` | Command prompt (ex commands) |
| `/` / `?` | Search forward / backward (regex, incremental). `3/foo` finds the third match |
| `"{reg}` | Use register `{reg}` for the next yank, delete or put (see below) |
| `q{reg}` … `q` | Record a macro into `{reg}`, then stop |
| `@{reg}` / `@@` | Play a macro / the last played one (`5@q`) |
| `m{a-z}` | Set a mark |
| `zz` or `z.` / `zt` or `z` Enter / `zb` or `z-` | Scroll so the cursor line is centered / at the top / at the bottom |
| Ctrl+D / Ctrl+U | Half a page down / up |
| Ctrl+F / Ctrl+B, PageDown / PageUp | A page down / up |
| Ctrl+E / Ctrl+Y | Scroll one line down / up |
| `ZZ` / `ZQ` | Save and close / close without saving |
| `gt` / `gT` | Next / previous buffer |
| Esc, Ctrl+C, Ctrl+[ | Cancel a half-typed command; with several cursors, back to one; hide search highlights |

### Registers

| Register | Contents |
| --- | --- |
| `"` (unnamed) | The last yank or delete. With the `clipboard` feature on (the default), it *is* the system clipboard |
| `0` | The last yank |
| `1` … `9` | The last deletes of whole lines or multi-line text, newest first |
| `-` | The last small (within one line) delete |
| `a` … `z` | Yours; `A` … `Z` append to them |
| `+`, `*` | The system clipboard, even with the `clipboard` feature off |
| `_` | The black hole: the text is discarded |

Macros live in their own registers. `qa` records into macro `a` and `@a` plays it.

## Visual mode

Enter visual mode with `v` (characters), `V` (lines) or Ctrl+V / Ctrl+Q (block). Every [motion](#motions) and
[text object](#text-objects) extends the selection. Pressing the same key again leaves visual mode, and pressing one
of the others switches the kind of visual mode.

| Keys | Action |
| --- | --- |
| `o`, `O` | Go to the other end of the selection |
| `d`, `x`, Delete | Delete the selection |
| `D`, `X` | Delete the selected lines |
| `c`, `s` | Change the selection |
| `C`, `S`, `R` | Change the selected lines |
| `y` | Yank the selection |
| `Y` | Yank the selected lines |
| `>` / `<` | Indent / outdent the lines (`3>` three levels) |
| `~`, `g~` / `u`, `gu` / `U`, `gU` | Toggle case / lowercase / uppercase |
| `J` / `gJ` | Join the selected lines with / without spaces |
| `p` / `P` | Replace the selection with the register. `p` puts the replaced text in the register, `P` keeps the register |
| `r{char}` | Replace every selected character with `{char}` |
| `I` / `A` | Insert before / after the selection. In block mode this inserts on every line |
| `:` | Ex command on the selected lines (`:s/a/b/g`) |
| `"{reg}` | Register for the next `y`, `d`, `c` or `p` |
| Ctrl+C | Copy (Sublime-style) and leave visual mode |
| Ctrl+N / Ctrl+Shift+X | Add / skip the next occurrence of the selection (multiple cursors) |
| Esc, Ctrl+[ | Back to normal mode |

## Prompts (`:`, `/` and `?`)

| Keys | Action |
| --- | --- |
| Enter | Run the command / search. An empty search repeats the last one |
| Esc | Cancel. A search puts the cursor back where it was |
| Ctrl+C, Ctrl+[ | Cancel |
| Backspace | Delete a character. On an empty prompt, cancel |
| Ctrl+Backspace, Ctrl+W | Delete a word |
| Ctrl+U | Delete to the start |
| Delete | Delete the character under the cursor |
| Left / Right, Home / End | Move the cursor |
| Up / Down | Older / newer history entries |
| Tab | Complete a file or directory name after `:e`, `:w`, `:o` or `:cd` |

The search jumps to the first match as you type (incremental) and highlights every match while `search_highlight` is
on. A pattern with no capital letters matches case insensitively (smart case). In vim mode, `/` and `?` take vim
patterns: magic by default, plus `\v` (very magic), `\V` (very nomagic), `\(a\|b\)`, `\{2,3}`, `\+`, `\=`,
`\<word\>`, classes such as `\a`, `\l` and `\u`, and `\c` / `\C` to force case. With vim off, and in `:s`, patterns are
Rust `regex` syntax (where `\<` and `\>` also work).

## Pickers

These are the file finder (Ctrl+P), command palette (Ctrl+Shift+P), buffer list (Ctrl+Shift+B) and Features list
(`:features`).

| Keys | Action |
| --- | --- |
| typing | Fuzzy filter |
| Enter | Open / run / switch. In the Features list it toggles the feature and keeps the list open |
| Up / Down, Tab | Previous / next item |
| Ctrl+P / Ctrl+N, Ctrl+K / Ctrl+J | Previous / next item |
| PageUp / PageDown | Ten items up / down |
| Backspace | Delete a character. On an empty filter, close |
| Ctrl+Backspace, Ctrl+U, Ctrl+W | Clear the filter |
| Esc, Ctrl+C, Ctrl+[ | Close |

## About panel

*About shrew* in the palette, or `:about`, shows the version, the Shrew Manifesto, the license, credits and links. It is
read-only.

| Keys | Action |
| --- | --- |
| `j` / `k`, Down / Up, the wheel | Scroll a line (the wheel: three) |
| Space, PageDown, Ctrl+D / PageUp, Ctrl+U | Scroll ten lines |
| `g`, Home / `G`, End | Top / bottom |
| Esc, Enter, `q`, Ctrl+C, Ctrl+[, a click outside | Close |

## Ex commands

Type them at the `:` prompt. Every [named command](#named-commands) also works as `:name`, for example `:palette`
or `:duplicate_line`.

| Command | Action |
| --- | --- |
| `:{number}` | Go to that line |
| `:w [file]`, `:write`, `:w!` | Save, or save as `file` |
| `:wa`, `:wall` | Save all |
| `:q`, `:quit`, `:close` | Close the buffer when others are open and it is saved; otherwise quit (refused while anything is unsaved) |
| `:q!`, `:quit!` | Quit without saving |
| `:qa`, `:qall` / `:qa!`, `:qall!` | Quit / quit without saving |
| `:wq` / `:x`, `:xit` | Save (`:x` only if changed) and close the buffer, or quit if it is the last one |
| `:wqa`, `:xa` | Save all and quit |
| `:e {file}`, `:edit`, `:o`, `:open` | Open a file (relative to `:pwd`; `~` works) |
| `:e` / `:e!` | Reload from disk / reload and drop your changes |
| `:enew`, `:new` | New scratch buffer |
| `:bn`, `:bnext` / `:bp`, `:bprev`, `:bprevious` | Next / previous buffer |
| `:b {n}`, `:b {name}`, `:buffer` | Go to buffer number `n`, or the first whose name contains `name`. With no match, open the buffer list |
| `:ls`, `:buffers` | Buffer list |
| `:bd`, `:bdelete` / `:bd!` | Close the buffer / close it and drop its changes |
| `:s/pat/rep/[g]` | Substitute on the cursor lines (or the visual selection). `\1` and `&` work in `rep`, as do `\n` and `\t` |
| `:%s/pat/rep/[g]` | Substitute in every line. An empty `pat` reuses the last search |
| `:noh`, `:nohlsearch` | Hide search highlights until the next search |
| `:set {option} …`, `:se` | Set options and features (see below) |
| `:features` | The Features list |
| `:syntax on` / `:syntax off` / `:syntax` | Syntax highlighting on / off / toggle |
| `:cd [dir]` / `:pwd` | Change / show the working directory (for `:e` and Ctrl+P) |
| `:update` | Install the available update and restart |
| `:version` | Show shrew's version |
| `:about` | The About panel: version, manifesto, license, credits |
| `:theme` / `:theme {id}`, `:colorscheme`, `:colo` | Theme picker (live preview) / switch to a theme and save it ([THEMES.md](THEMES.md)) |
| `:theme import {file}` | Convert a `.sublime-color-scheme` or `.tmTheme` into a custom theme and switch to it |
| `:accent` / `:accent {id}` | Accent picker / switch to an accent (`theme` for the theme's own) and save it |
| `:font` / `:font {family}` | Font picker (installed monospace families, live preview) / switch to a family and save it |
| `:term`, `:terminal` / `:term {command}` | A new terminal with your shell / one running `command` (it stays open when the command ends; Enter closes it) |

### `:set`

| Option | Effect |
| --- | --- |
| `{feature}` / `no{feature}` / `{feature}!` | Turn a [feature](../README.md#features) on / off / toggle it: `:set nosyntax`, `:set mcp`, `:set vim!` |
| `ts=N`, `tabstop=N`, `sw=N`, `shiftwidth=N`, `tab_width=N` | Indent width, 1–16 |
| `so=N`, `scrolloff=N` | Lines kept visible around the cursor |
| `et`, `expandtab` / `noet`, `noexpandtab` | Indent with spaces / tabs |
| `nu`, `number` / `nonu` | Line numbers |
| `rnu`, `relativenumber` / `nornu` | Relative line numbers |
| `hls`, `hlsearch` / `nohls` | Search highlighting |
| `list` / `nolist` | Show whitespace |
| `lig`, `ligatures` / `nolig`, `noligatures` / `ligatures!` | Font ligatures on / off / toggle (saved to `[font] ligatures`) |

Several options can go on one line: `:set ts=2 et nowhitespace`.

## Named commands

Every command has a name. Use it as `:name`, as the right-hand side of a mapping in `[keys]`, or find it in the
command palette by its title.

| Name | Palette title | Default keys |
| --- | --- | --- |
| `save` | File: Save | Ctrl+S |
| `save_all` | File: Save All | Ctrl+Shift+S |
| `quit` | Quit | Ctrl+Q |
| `force_quit` | Quit Without Saving | `ZQ`, `:q!` |
| `new` | File: New Scratch Buffer | Ctrl+N (insert) |
| `close` | File: Close Buffer | Ctrl+W (vim off) |
| `next_buffer` | Buffer: Next | Ctrl+Tab |
| `prev_buffer` | Buffer: Previous | Ctrl+Shift+Tab |
| `reload` | File: Reload From Disk | `:e` |
| `undo` | Edit: Undo | Ctrl+Z, `u` |
| `redo` | Edit: Redo | Ctrl+Shift+Z, Ctrl+R |
| `copy` | Edit: Copy | Ctrl+C |
| `cut` | Edit: Cut | Ctrl+X |
| `paste` | Edit: Paste | Ctrl+V |
| `select_all` | Selection: Select All | Ctrl+A |
| `select_line` | Selection: Expand to Line | Ctrl+L |
| `delete_line` | Edit: Delete Line | Ctrl+Shift+K |
| `duplicate_line` | Edit: Duplicate Line | Ctrl+Shift+D |
| `move_line_up` | Edit: Move Line Up | Alt+Up |
| `move_line_down` | Edit: Move Line Down | Alt+Down |
| `toggle_comment` | Edit: Toggle Comment | Ctrl+/ |
| `indent` | Edit: Indent | Ctrl+] |
| `outdent` | Edit: Outdent | Ctrl+[ (vim off) |
| `join_lines` | Edit: Join Lines | `J` |
| `insert_line_below` | Edit: Insert Line Below | Ctrl+Enter |
| `insert_line_above` | Edit: Insert Line Above | Ctrl+Shift+Enter |
| `add_cursor_above` | Cursors: Add Above | Ctrl+Alt+Up |
| `add_cursor_below` | Cursors: Add Below | Ctrl+Alt+Down |
| `add_next_occurrence` | Cursors: Add Next Occurrence | Ctrl+D (insert), Ctrl+N (normal) |
| `skip_occurrence` | Cursors: Skip Occurrence | Ctrl+Shift+X (normal) |
| `select_all_occurrences` | Cursors: Select All Occurrences | Alt+F3 |
| `split_lines` | Cursors: Split Selection Into Lines | Ctrl+Shift+L |
| `single_cursor` | Cursors: Single Cursor | Esc |
| `goto_bracket` | Go to Matching Bracket | Ctrl+M, `%` |
| `select_brackets` | Selection: Expand to Brackets | Ctrl+Shift+M |
| `goto_line` | Go to Line… | Ctrl+G |
| `find` | Find… | Ctrl+F (insert), `/` |
| `find_next` | Find Next | F3, `n` |
| `find_prev` | Find Previous | Shift+F3, `N` |
| `nohlsearch` | Clear Search Highlight | `:noh` |
| `palette` | Command Palette | Ctrl+Shift+P |
| `files` | Go to File… | Ctrl+P |
| `buffers` | Switch Buffer… | Ctrl+Shift+B |
| `features` | Features: List and Toggle… | `:features` |
| `toggle_vim` | Features: Toggle Vim Mode | `:set vim!` |
| `toggle_line_numbers` | View: Toggle Line Numbers | |
| `toggle_relative_numbers` | View: Toggle Relative Line Numbers | |
| `toggle_whitespace` | View: Toggle Whitespace | |
| `toggle_syntax` | View: Toggle Syntax Highlighting | `:syntax` |
| `font_bigger` | View: Increase Font Size | Ctrl+= |
| `font_smaller` | View: Decrease Font Size | Ctrl+- |
| `font_reset` | View: Reset Font Size | Ctrl+0 |
| `select_theme` | Theme: Select… | |
| `select_accent` | Accent: Select… | |
| `select_font` | Font: Select… | |
| `toggle_ligatures` | Font: Toggle Ligatures | |
| `check_updates` | Shrew: Check for Updates | Ctrl+Shift+U |
| `update` | Shrew: Install Update and Restart | `:update`, the status-bar button |
| `config` | Shrew: Open Config File | |
| `mcp` | Shrew: AI Assistants (MCP) Setup | |
| `about` | About shrew | `:about` |
| `toggle_terminal` | Terminal: Toggle Panel | Ctrl+\` |
| `new_terminal` | Terminal: New Terminal | Ctrl+Shift+\`, `:term` |
| `close_terminal` | Terminal: Close Terminal | the panel's × |
| `next_terminal` | Terminal: Next Terminal | Ctrl+PageDown (in a terminal) |
| `prev_terminal` | Terminal: Previous Terminal | Ctrl+PageUp (in a terminal) |
| `maximize_terminal` | Terminal: Toggle Maximized Panel | the panel's □ |
| `focus_terminal` | Terminal: Focus Terminal | |
| `focus_editor` | Terminal: Focus Editor | Ctrl+W k (in a terminal, vim keys) |
| `terminal_normal_mode` | Terminal: Normal Mode (scroll, select, search) | Ctrl+\\ Ctrl+N (in a terminal) |

## Mouse

| Action | Effect |
| --- | --- |
| Click | Place the cursor. In vim, a click keeps normal mode |
| Double-click | Select a word; dragging then extends by words |
| Triple-click | Select a line; dragging then extends by lines |
| Drag | Select. In vim, a drag selection enters visual mode |
| Shift+click | Extend the selection to the click |
| Ctrl+click, Alt+click | Add a cursor (or a selection, when dragging) |
| Wheel | Scroll without moving the cursor |

## Terminal

The built-in terminal (feature `terminal`) is a panel under the text with a tab per terminal. Ctrl+\` opens it with
your shell, in the project folder. While a terminal has focus **every key goes to the program in it**, except these
(the code is in `crates/shrew/src/terminal/`):

| Keys | Action |
| --- | --- |
| Ctrl+\` | Hide the panel and go back to the editor (Ctrl+\` again brings it back) |
| Ctrl+Shift+\` | New terminal |
| Ctrl+PageUp / Ctrl+PageDown | Previous / next terminal |
| Ctrl+Shift+P | Command palette |
| Ctrl+Shift+C / Ctrl+Shift+V, Shift+Insert | Copy the selection / paste (bracketed when the program asks for it) |
| Shift+PageUp / Shift+PageDown, Shift+Home / Shift+End | Scroll the history by a page / to the top or bottom (not in full-screen programs) |
| Ctrl+\\ Ctrl+N | Terminal normal mode (below) |
| Ctrl+W k, Ctrl+W Up, Ctrl+W Ctrl+W | Back to the editor (vim keys on; Ctrl+W then any other key sends both, Ctrl+W . sends Ctrl+W) |
| Ctrl+W N / Ctrl+W : / Ctrl+W c | Normal mode / the `:` prompt / close the terminal (vim keys on) |
| Enter, Esc, q | Close a `:term` terminal whose command has ended |

`[terminal.keys]` in config.toml adds chords that stay with shrew, or gives these back to the terminal (see
[CONFIG.md](CONFIG.md#terminal)). Typing scrolls back to the bottom. Clicking the editor gives it the keys back.

**Normal mode** freezes nothing: the program keeps running while you move over its screen and history.

| Keys | Action |
| --- | --- |
| `h` `j` `k` `l`, arrows | Move |
| `w` `b` `e` / `W` `B` `E` | Words / whitespace-separated words |
| `0` `^` `$`, `H` `M` `L`, `gg` `G`, `%`, `{` `}` | Line start, first non-blank, line end; screen top, middle, bottom; top and bottom of the history; bracket; paragraph |
| Ctrl+U / Ctrl+D, Ctrl+B / Ctrl+F, PageUp / PageDown, Ctrl+Y / Ctrl+E | Scroll half a page, a page, a line |
| `v` / `V` / Ctrl+V | Select characters / lines / a block |
| `y` | Copy the selection (the line when nothing is selected) |
| `/` `?` then Enter, `n` / `N` | Search the screen and history forward / backward (regex; lowercase ignores case), next / previous |
| Esc | Clear the selection or search; again, leave normal mode |
| `i`, `a`, `q`, Enter | Back to the program |

**Mouse:** drag to select (double-click: a word, triple-click: a line, Alt+drag: a block); the selection is copied
when you let go (`copy_on_select`). Middle click pastes. The wheel scrolls the history, or goes to the program when
it asked for the mouse (vim, htop) or runs full-screen (less, man). Shift+drag and Shift+wheel always select and
scroll. File paths in the output (`src/main.rs:12:5`, `  --> a.rs:3:1`, `File "x.py", line 7`) and URLs are underlined
under the pointer; **Ctrl+click** opens a file in the editor at that line, a URL in the browser. Drag the panel's top
edge to resize it (the height is saved); the buttons on the right open a terminal, maximize the panel and close the
terminal.

`srw file` in a shrew terminal opens the file in that window, and `srw -w` waits for it, so `EDITOR="srw -w"` works
for `git commit` run there.
