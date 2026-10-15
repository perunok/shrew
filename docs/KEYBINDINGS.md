# Keybindings

This is every key shrew knows by default. It was written from the source: global chords in `Editor::binding`, insert
mode in `insert_key` and `caret_move`, prompts in `prompt_key`, pickers in `picker_key`, ex commands in `run_ex`
(`crates/shrew-core/src/editor.rs`), and vim in `parse` / `parse_motion` (`crates/shrew-core/src/vim.rs`). If you
change a binding there, update this file too.

**Notation.** `Ctrl+X` is a chord. `gg` means press `g`, then `g`. `{motion}`, `{char}` and `{reg}` stand for any
motion, character or register name. "Ctrl" also means ⌘ Cmd on macOS. Vim keys are case-sensitive: `G` is Shift+g.

**Order of precedence** for a key press:

1. An open menu (the title bar's), then an open picker (file finder, palette, buffers, features), the Settings or
   About panel or a prompt (`:`, `/`, `?`) takes every key. Then a focused terminal or the sidebar takes the keys
   they use (see [Terminal](#terminal), [Sidebar](#sidebar)). F10 and Alt+F/E/V/G/H open the menus unless you
   mapped them or a terminal has the focus.
2. Your mappings (`[keys.*]` in `config.toml`, imported vimrc and Sublime bindings) come next. See
   [CONFIG.md](CONFIG.md#keys).
3. Then the global chords below, unless a vim command is half-typed (after `d`, `"a` or `f`, the next key belongs
   to vim).
4. Last comes the mode: vim normal or visual, or insert. With the `vim` feature off, shrew is always in insert mode.

**Keys that mean different things in different places**, on purpose:

| Keys | Where | What |
| --- | --- | --- |
| Ctrl+B | vim normal / insert or vim off | Page up (vim's) / toggle the sidebar; Ctrl+Shift+E and the title bar button work everywhere |
| Ctrl+R | vim / vim off | Redo, insert a register (vim's) / go to symbol; Ctrl+Shift+O works everywhere |
| Ctrl+O, Tab | vim normal / vim off | Jump back, jump forward (vim's) / Ctrl+O opens a file; Alt+Left, Alt+Right jump everywhere |
| Ctrl+PageUp / PageDown | editor / terminal | Previous / next buffer / previous / next terminal |
| Alt+F, Alt+E, Alt+V, Alt+G, Alt+H, F10 | editor / terminal | Open the menus / sent to the program (readline's Alt+F moves a word); the menu button still works |
| Ctrl+Shift+E, Ctrl+Shift+F, Ctrl+, … | editor / terminal | The global chord / sent to the program; add it to `[terminal.keys]` to keep it for shrew |
| `q`, `r`, `R`, `/`, `:` | sidebar, Settings panel | Their own meaning while they have the keys (see each section) |

**Contents:** [Global chords](#global-chords) · [Insert mode](#insert-mode) · [Normal mode](#normal-mode) ·
[Visual mode](#visual-mode) · [Prompts](#prompts--and-) · [Pickers](#pickers) · [Sidebar](#sidebar) ·
[Find in project](#find-in-project) · [Settings](#settings-panel) · [About panel](#about-panel) · [Menus](#menus) ·
[Ex commands](#ex-commands) · [Named commands](#named-commands) · [Mouse](#mouse) · [Title bar](#title-bar) ·
[Terminal](#terminal) · [Git](#git) · [Diff view](#diff-view)

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
| Ctrl+, | Settings panel (Cmd+, on macOS) | always |

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
| Ctrl+. / Ctrl+Shift+. | Next / previous git change ([Git](#git); Sublime's Next Modification) | always |

### Project navigation

The project is the folder `srw <folder>` opened, or else the nearest folder above the file with `.git` (`.hg`, `.jj`,
`.svn`, a `.shrew` marker), a Cargo workspace root, or `package.json` / `go.mod` / `pyproject.toml` / `Cargo.toml`.
The home folder and `/` are never a project: without one, Ctrl+P lists your recent files.

| Keys | Action | When |
| --- | --- | --- |
| Ctrl+P | Go to file in the project. `:42` goes to line 42, `name:42` opens at line 42, `@` lists the file's symbols | always |
| Ctrl+B | Show / hide the [sidebar](#sidebar) | insert (in normal mode, a page up) |
| Ctrl+Shift+E | Focus the sidebar (shows it); again: back to the text | always |
| Ctrl+Shift+F | [Find in project](#find-in-project) (the selection, or the last search) | always |
| Ctrl+R | Go to symbol in the file | vim off (in vim it is redo / insert a register) |
| Ctrl+Shift+O | Go to symbol in the file | always |
| Ctrl+Shift+R | Go to symbol in the project (indexed in the background) | always |
| Ctrl+Alt+P | Open a recent project | always |
| Ctrl+Shift+T | Reopen the last closed file | always |
| Alt+Left / Alt+Right, Alt+- / Alt+Shift+- | Go back / forward along the [jump list](#other-commands) | always |

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
| `]c` `[c`, `]h` `[h` | Next / previous git change, wrapping around the file ([Git](#git)) |
| `*` `#` | Search the word under the cursor forward / backward |
| `` `{a-z} `` / `'{a-z}` | To mark: exact position / its line |
| ``` `` ``` / `''` | Back to where the last jump started (and again to return) |

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
| Ctrl+O / Ctrl+I, Tab | Older / newer place in the jump list. Jumps are `G`, `gg`, `%`, `n`, `N`, `*`, `#`, `/`, `?`, `{`, `}`, `H`, `M`, `L`, marks, `:42`, and opening or switching files |
| `ghs` / `ghu` / `ghr` | Stage / unstage / revert the git change at the cursor (in visual mode: the selected lines) |
| `ghp` / `ghb` | Peek at the change at the cursor / toggle the inline blame |
| `gf` | Open the file named under the cursor (`src/main.rs:42:5` goes to line 42, column 5); tried next to the file, in the project and in `:pwd` |
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

In the go-to-file picker (Ctrl+P), a query starting with `:` is a line number (`:42` Enter), `name:42` opens the
match at line 42, and `@` switches to the file's symbols (delete the `@` to get the files back).

## Sidebar

The project's file tree: Ctrl+B shows it, Ctrl+Shift+E (or `:Ex`) gives it the keys, and `srw <folder>` opens with it.
Folders are read when they are first opened, `.gitignore`d entries are dimmed, and the active file is always
revealed and highlighted. Nothing watches the disk: the tree is read again when the window gets the focus back, on
`R`, and after its own file operations.

| Keys | Action |
| --- | --- |
| `j` / `k`, Down / Up, Ctrl+N / Ctrl+P | Next / previous entry |
| `g`, Home / `G`, End | First / last entry |
| PageDown / PageUp, Ctrl+D / Ctrl+U | A page / half a page down / up |
| `l`, Right | Open a folder (on an open folder: its first entry); on a file, open it and stay in the sidebar |
| `h`, Left | Close the folder, or go to the parent folder |
| Enter, `o` | Open the file and go to it; open / close a folder |
| Space | Show the file but stay in the sidebar |
| `a` / `A` | New file / new folder in the selected folder (`dir/name` makes the folders too) |
| `r`, F2 | Rename (the name is there to edit) |
| `d`, Delete | Move to the trash after a `y` (Linux; elsewhere it deletes, also after a `y`) |
| `R`, F5 | Read the folders again |
| `/` | Filter the open folders (fuzzy); Enter keeps the filter, Esc clears it |
| `:` | The command line |
| `q` | Hide the sidebar |
| Esc, Tab, Ctrl+C | Back to the text |
| Ctrl+… | Global chords (Ctrl+P, Ctrl+Shift+F, Ctrl+B, …) work as in the text |

With the mouse: a click opens a file (and goes to it) or opens / closes a folder, the wheel scrolls, and dragging the
right edge resizes the sidebar (the width is saved as `[sidebar] width`).

## Find in project

Ctrl+Shift+F, `:grep {pattern}` or `:rg {pattern}`: searches every file of the project that `.gitignore` lets
through, on all cores, and shows the matches grouped by file as they come. The search is literal and smart-case (an
upper-case letter makes it case-sensitive) unless you switch that; it starts again on every key and stops at 5000
matches.

| Keys | Action |
| --- | --- |
| typing | The pattern |
| Alt+R / Alt+C / Alt+W | Regex / always case-sensitive / whole words (the chips on the right light up) |
| Tab | Edit the file globs (`*.rs, !tests/`, `src`); Tab or Enter goes back to the pattern |
| Up / Down, Ctrl+P / Ctrl+N, PageUp / PageDown | Move through the matches (file headers are skipped) |
| Enter | Go to the match (Ctrl+O comes back) |
| Esc | Close |

## Settings panel

Ctrl+, (Cmd+, on macOS), palette *Preferences: Settings*, `:settings [section]` or the gear at the right end of the
status bar. Sections on the left (Appearance, Editor, Features, Layout & window, Keybindings, Import, AI assistants,
Updates, About), their settings on the right. Every change applies at once and is saved to `config.toml` (comments
kept), exactly like `:set`, the pickers and the Features list.

| Keys | Action |
| --- | --- |
| `j` / `k`, Down / Up, Ctrl+N / Ctrl+P | Next / previous section or setting |
| Tab, `h` / `l`, Left / Right | Between the sections and the settings |
| Enter, Space | Toggle a switch, open a list or a picker (theme, accent and font preview as you move), press a button |
| Left / Right, `-` / `+` on a number | Smaller / bigger (`<` / `>`: ten steps) |
| `r` / `R` | Reset the setting / the whole section to its default |
| `/` | Search every setting (Enter goes to the results, Esc ends the search) |
| `g`, Home / `G`, End, Ctrl+D / Ctrl+U, PageDown / PageUp | First / last, eight down / up |
| Esc, `q`, Ctrl+C, Ctrl+[, Ctrl+, , a click outside | Close (Esc first ends a search or closes an open list) |

The mouse works too: click a section, a switch, a list or a button, drag a slider, use the steppers, scroll with the
wheel. The `settings` feature switches the panel and the gear off (`:set nosettings`).

## About panel

*About shrew* in the palette, or `:about`, shows the version, the Shrew Manifesto, the license, credits and links. It is
read-only; a click on a link (or on the repository address at the bottom) opens it in the browser.

| Keys | Action |
| --- | --- |
| `j` / `k`, Down / Up, the wheel | Scroll a line (the wheel: three) |
| Space, PageDown, Ctrl+D / PageUp, Ctrl+U | Scroll ten lines |
| `g`, Home / `G`, End | Top / bottom |
| Esc, Enter, `q`, Ctrl+C, Ctrl+[, a click outside | Close |

## Menus

The custom title bar (feature `custom_titlebar`, on by default) has an application menu: File, Edit, Selection,
View, Go, Git, Terminal and Help. Each entry shows the chord that runs it in the current mode; entries for commands this
build does not have are left out.

| Keys | Action |
| --- | --- |
| F10, palette *View: Open Menu* | Open the menu (the focus on the list of menus) |
| Alt+F / Alt+E / Alt+V / Alt+G / Alt+H | Open File / Edit / View / Go / Help |
| Up / Down | Previous / next menu, or entry once inside one |
| Right / Enter | Into the highlighted menu; inside one, Right goes to the next menu |
| Left | Back to the list of menus |
| Home / End | First / last entry |
| typing | The next entry starting with what you typed; on the list of menus, a menu's letter opens it |
| Enter, Space | Run the entry |
| Esc, F10, a click outside | Close |

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
| `:cd [dir]` / `:pwd` | Make `dir` the project and working directory / show it (for `:e`, Ctrl+P and the sidebar) |
| `:grep {pattern}`, `:rg`, `:vimgrep` | [Find in project](#find-in-project) |
| `:Ex`, `:Explore`, `:Lex`, `:NERDTree` | Focus the [sidebar](#sidebar) |
| `:update` | Install the available update and restart |
| `:version` | Show shrew's version |
| `:about` | The About panel: version, manifesto, license, credits |
| `:settings [section]`, `:preferences`, `:prefs` | The Settings panel, on a section when one is named (`:settings ai`) |
| `:theme` / `:theme {id}`, `:colorscheme`, `:colo` | Theme picker (live preview) / switch to a theme and save it ([THEMES.md](THEMES.md)) |
| `:theme import {file}` | Convert a `.sublime-color-scheme` or `.tmTheme` into a custom theme and switch to it |
| `:accent` / `:accent {id}` | Accent picker / switch to an accent (`theme` for the theme's own) and save it |
| `:font` / `:font {family}` | Font picker (installed monospace families, live preview) / switch to a family and save it |
| `:term`, `:terminal` / `:term {command}` | A new terminal with your shell / one running `command` (it stays open when the command ends; Enter closes it) |
| `:Gdiff`, `:Gdiffsplit`, `:Gvdiffsplit` / `:Gdiff {rev}` | [Diff](#diff-view) the file: index ↔ working tree / `rev` (`HEAD`, a branch, a hash) ↔ working tree |
| `:git_diff_commit [rev]`, `:Gshow [rev]` | A commit against its parent (`HEAD~2`, a hash; no argument: pick one) |
| `:git_diff_compare [a] [b]`, `[a..b]` | Every file that differs between two revisions (one: against the working tree; none: pick them) |

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
| `menu` | View: Open Menu | F10 (custom title bar) |
| `system_titlebar` | View: Toggle System Title Bar | `:set custom_titlebar!` |
| `customize_titlebar` | View: Customize Title Bar… | right-click the title bar |
| `minimize` | Window: Minimize | the title bar's button |
| `maximize` | Window: Maximize / Restore | double-click the title bar |
| `toggle_sidebar` | View: Toggle Sidebar | Ctrl+B (insert or vim off), the title bar button |
| `focus_sidebar` | View: Focus Sidebar | Ctrl+Shift+E |
| `refresh_sidebar` | View: Refresh Sidebar | `R` in the sidebar |
| `find_in_project` | Find in Project… | Ctrl+Shift+F |
| `symbols` | Go to Symbol… | Ctrl+Shift+O, Ctrl+R (vim off), `@` in Ctrl+P |
| `project_symbols` | Go to Symbol in Project… | Ctrl+Shift+R |
| `recent` | File: Open Recent… | |
| `recent_projects` | Project: Open Recent… | Ctrl+Alt+P |
| `reopen_closed` | File: Reopen Closed File | Ctrl+Shift+T |
| `jump_back` | Go Back | Ctrl+O (normal), Alt+Left, Alt+- |
| `jump_forward` | Go Forward | Ctrl+I, Tab (normal), Alt+Right, Alt+Shift+- |
| `goto_file` | Go to File Under Cursor | `gf` |
| `toggle_minimap` | View: Toggle Minimap | `:set minimap!` |
| `toggle_terminal` | Terminal: Toggle Panel | Ctrl+\` |
| `new_terminal` | Terminal: New Terminal | Ctrl+Shift+\`, `:term` |
| `close_terminal` | Terminal: Close Terminal | the panel's × |
| `next_terminal` | Terminal: Next Terminal | Ctrl+PageDown (in a terminal) |
| `prev_terminal` | Terminal: Previous Terminal | Ctrl+PageUp (in a terminal) |
| `maximize_terminal` | Terminal: Toggle Maximized Panel | the panel's □ |
| `focus_terminal` | Terminal: Focus Terminal | |
| `focus_editor` | Terminal: Focus Editor | Ctrl+W k (in a terminal, vim keys) |
| `terminal_normal_mode` | Terminal: Normal Mode (scroll, select, search) | Ctrl+\\ Ctrl+N (in a terminal) |
| `settings` | Preferences: Settings | Ctrl+,, `:settings`, the status-bar gear |
| `git_next_hunk` | Git: Next Change | `]c`, Ctrl+. |
| `git_prev_hunk` | Git: Previous Change | `[c`, Ctrl+Shift+. |
| `git_preview_hunk` | Git: Preview Change | `ghp`, a click on a marker |
| `git_stage_hunk` | Git: Stage Change (or Selected Lines) | `ghs` |
| `git_unstage_hunk` | Git: Unstage Change (or Selected Lines) | `ghu` |
| `git_revert_hunk` | Git: Revert Change (or Selected Lines) | `ghr` |
| `git_gutter_base` | Git: Compare Markers With HEAD / the Index | |
| `git_toggle_gutter` | Git: Toggle Change Markers | `:set git_gutter!` |
| `git_toggle_blame_inline` | Git: Toggle Inline Blame | `ghb` |
| `git_diff_file` | Git: Diff File (Index ↔ Working Tree) | `:Gdiff`, the Git menu |
| `git_diff_file_head` | Git: Diff File Against HEAD | `:Gdiff HEAD` |
| `git_diff_all` | Git: Diff All Unstaged Changes | |
| `git_diff_staged` | Git: Diff Staged Changes | |
| `git_diff_commit` | Git: Diff Commit… | |
| `git_diff_compare` | Git: Compare Branches or Commits… | |
| `git_diff_layout` | Git: Toggle Diff Layout (Split / Unified) | `t` in a diff |

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
| Click on the minimap | Scroll so the line under the pointer is centred |
| Drag on the minimap | Scroll continuously; the viewport box follows the pointer |
| Click a link in the About panel | Open it in the browser |

## Title bar

With the custom title bar (feature `custom_titlebar`, on by default), the top row is the window's title bar and
holds the buffer tabs. Which items it shows, and where, is set in [`[titlebar]`](CONFIG.md#titlebar).

| Action | Effect |
| --- | --- |
| Drag an empty part | Move the window (through the window manager, so snapping and tiling work) |
| Double-click an empty part | Maximize / restore |
| Right-click an empty part | *Customize Title Bar…*, *Use System Title Bar* |
| Drag a window edge or corner | Resize (5 px borders, wider at the corners; not while maximized) |
| Click the menu button (three lines) | Open the menus; hovering a menu opens it |
| Click a tab | Switch to that buffer |
| Minimize, maximize/restore, close | The window buttons; close asks once when there are unsaved changes |

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

## Git

In a git repository (cargo feature `git`), the gutter marks what changed against the index (feature `git_gutter`;
`[git] gutter_base = "head"` compares with the last commit): a bar for added lines (`git-added`) and changed lines
(`git-modified`), a triangle where lines were deleted (`git-deleted`). The same marks run along the minimap and the
scrollbar. They follow your typing a moment after each pause.

| Keys | Action |
| --- | --- |
| `]c` / `[c` (also `]h` / `[h`), Ctrl+. / Ctrl+Shift+. | Next / previous change. A count works (`3]c`), it wraps around the file, and it is a jump (Ctrl+O comes back). With an operator, the lines up to it (`d]c`) |
| `ghp`, a click on a marker | Peek at the change: its old and new lines under it, the changed words marked, with Stage and Revert buttons. It follows `]c` / `[c`; Esc closes it |
| `ghs` | Stage the change at the cursor. In visual mode: just the selected lines (a changed line takes its old line with it) |
| `ghu` | Unstage the change at the cursor, or the selected lines |
| `ghr` | Revert the change at the cursor, or the selected lines, to the marked version: an ordinary edit, so `u` undoes it |
| `ghb` | Inline blame on / off |

The `gh` keys are gitsigns' hunk mappings without a leader (shrew has none by default; map `<leader>hs` to
`git_stage_hunk` and so on in `[keys.normal]` if you prefer). With vim off, the palette has every command (*Git: …*)
and the title bar's Git menu lists them with their keys.

**Inline blame** (feature `git_blame_inline`): when the cursor rests on a line for 300 ms (`[git] blame_delay`), its
last author, when and the commit's summary show faintly after the line ("Ann, 3 days ago • Fix the parser"; your own
uncommitted lines read "You • Uncommitted changes").

**Status** (feature `git_status`): the branch shows in the status bar and next to the project in the title bar
(`main*` when the work tree has changes, `↑2 ↓1` against the upstream, MERGING and the like during an operation);
clicking it opens the branch picker when the build has one. The sidebar colours files by state with git's letter at
the right: M modified, A added, D deleted, R renamed, ? untracked, U conflict, ! ignored; a folder holding changes
is tinted and gets a dot.
## Diff view

A diff opens as a tab of its own (cargo feature `git`): one file, or every changed file of the unstaged or staged
changes, a commit or a comparison, each with a header that folds it. Side by side (old left, new right, aligned,
blank where one side has no line) or unified, with syntax colors, the changed words marked, and unchanged lines
folded into one row ("··· 42 unchanged lines ···"). When the working-tree side is open in a buffer the diff shows
your edits as you type. Open one from the Git menu, the palette (*Git: …*) or `:Gdiff`; the code is in
`crates/shrew/src/git/diffview/`. While a diff tab is active it takes these keys; the palette, Ctrl+P, Ctrl+Tab,
`:` and the other app-wide chords still work.

| Keys | Action |
| --- | --- |
| `j` / `k`, Down / Up | Next / previous row (a count works: `5j`) |
| Ctrl+D / Ctrl+U, Ctrl+F / Ctrl+B, PageDown / PageUp | Half a page / a page down / up |
| Ctrl+E / Ctrl+Y | Scroll a row |
| `gg`, Home / `G`, End | First / last row (`{n}G`: row `n`) |
| `]c` / `[c`, F7 / Shift+F7 | Next / previous change (`n` / `N` too, when there is no search) |
| `]f` / `[f` | Next / previous file |
| Tab | The other side (where Enter, `y` and searches start) |
| `h` / `l`, Left / Right, `0` | Scroll sideways / back to the first column |
| Enter | Open the fold or the file header under the cursor; on a line, open the file there |
| `o` | Open the working-tree file at the line under the cursor |
| Space, `za`, `zo` / `zc` | Open or close the fold or file header / fold the file |
| `zR` / `zM` | Show every line / fold the unchanged ones again |
| `s` / `u` / `-` | Stage / unstage / either, the change under the cursor or the selected lines |
| `X` | Revert it: in an open buffer as an edit (undoable there); from the file on disk after a second `X` |
| `v`, `V` / Esc | Select rows for `s`, `u`, `X` and `y` / clear the selection |
| `y` | Copy the lines of the cursor's side |
| `t` | Split / unified (saved as `[git] diff_layout`) |
| `w` | Ignore white space, or not |
| `<` / `>` | Fewer / more context lines around each change |
| `/`, `?`, `n` / `N` | Search both sides / next, previous match |
| `r` | Reload |
| `q` | Close the tab |

Staging works in the unstaged diff (index ↔ working tree), unstaging in the staged one (HEAD ↔ index); line
selections stage just those lines. With the mouse: click a row to put the cursor there (Shift+click extends, a drag
selects rows), click a fold or a file header to open it, double-click a line to open the file, click the round button
on a change in the middle gutter to stage or unstage it, and use the header's buttons (layout, white space, context,
stage, unstage, revert, previous / next change). The wheel scrolls (Shift+wheel sideways); the scrollbar shows every
change and can be dragged.
