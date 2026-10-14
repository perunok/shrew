# The `srw` command

`srw` opens files from a terminal, the way Sublime Text's `subl` does: it hands them to the shrew that is already
running (its window comes to the front) or starts one in the background, and the prompt comes back at once.

`shrew` is the same program under its longer name (the installer and the release archives add a `shrew -> srw`
symlink). Both behave identically; help and error messages use the name you typed.

## Usage

```text
srw [options] [file[:LINE[:COL]] | +LINE | DIR | -]...
```

| Command | What it does |
| --- | --- |
| `srw` | Bring the running shrew to the front, or start one with a scratch buffer |
| `srw a.rs b.rs` | Open both; `a.rs` is the active buffer |
| `srw src/main.rs:42` | Open with the cursor on line 42 |
| `srw src/main.rs:42:7` | Line 42, char column 7 |
| `srw +42 src/main.rs` | Line 42 of the first file (vim style; a file's own `:LINE` wins) |
| `srw some/dir` | Make `some/dir` the project folder and open the file picker (Ctrl+P) in it |
| `srw new.txt` | A path that does not exist opens as a new, empty buffer; `:w` creates the file |
| `echo hi \| srw -` | Read stdin into a new scratch buffer |
| `srw -w file` | Wait: return only once `file` is closed in the editor (see [EDITOR](#use-shrew-as-editor)) |
| `srw -n file` | A new shrew (a second window), even when one is running |
| `srw -f file` | Run the editor in this process, attached to the terminal (implies `-n`) |
| `srw -- -odd-name` | `--` ends the options: everything after it is a file |

Relative paths are resolved against the directory you run `srw` in, not the running editor's. A file whose name
really ends in `:digits` (it exists as typed) is opened as it is. Short flags combine: `srw -nw file`.

The other options work as before: `--version` (`-V`), `--help` (`-h`), `--features`, `--print-config`,
`--mcp-stdio`, `--stats` (with `-f`: frame timings on stderr), and `--screenshot OUT.png [--size WxH] [--scale F]
[--keys KEYS] [files]` for a headless render.

## Use shrew as $EDITOR

`-w` makes `srw` wait until every buffer it opened is closed (`:q`, `:wq`, `:bd`, closing the tab, or quitting
shrew), and it prints nothing, so tools that start an editor and wait for it work:

```sh
export EDITOR="srw -w"                       # shells, crontab -e, ...
git config --global core.editor "srw -w"     # git commit, git rebase -i, ...
```

`git commit` then opens `COMMIT_EDITMSG` in your running shrew; write the message, save and close the buffer
(`:wq` with vim keys, Ctrl+S then Ctrl+W without), and git continues. `srw -w` with nothing to open returns at once. With `-` the wait
is for the stdin buffer.

## Foreground and background

Without `-f`, a new shrew is started detached from the terminal: in its own session (`setsid`) on Unix, as a
`DETACHED_PROCESS` on Windows, with no terminal stdin/stdout/stderr. Closing the terminal leaves it running.
`srw` waits only until the new editor has taken the request (a few milliseconds, before its window is even up) and
reports an error such as "cannot open a window" when it fails that early. Its later log output goes nowhere.

`-f` keeps the editor in the `srw` process: the terminal waits until you quit and log messages appear in it
(`RUST_LOG=srw=debug srw -f` for more). Use it to debug. Whether stdout is a terminal does not change anything: a
desktop launcher, a file manager or a script running `srw file` gets the same single-instance behaviour as a shell.

## How single-instance works

The running shrew listens on a per-user endpoint. `srw` connects to it, sends one JSON line and reads the answers:

```json
{"cwd":"/home/me/src","files":[{"path":"/home/me/src/a.rs","line":42,"col":7}],"wait":true}
```

The editor answers `"ok"` as soon as the request is on its way to the UI thread (or `{"error":"..."}`), and for
`--wait` later `"closed"`. Other fields: `dir` (a folder argument), `stdin_text`, `activation_token`.

- **Unix:** a Unix domain socket at `$XDG_RUNTIME_DIR/shrew/instance.sock`, or `/tmp/shrew-<uid>/instance.sock`
  when there is no runtime directory (macOS). The directory is created with mode 0700 and must belong to you, so
  other users cannot reach the socket. A socket nobody listens on any more (shrew was killed) is stale: the next
  shrew removes it and takes over. Binding happens under a lock (`instance.lock`), so two `srw` started at the same
  moment still end up with one editor.
- **Windows:** loopback TCP on a random port. `%LOCALAPPDATA%\shrew\instance` holds the port and a random token that
  every request must carry.
- When no shrew is running, `srw` starts one (this same binary with an internal `--instance` flag) and sends it the
  same request over the new process's stdin/stdout pipes, so stdin text and `--wait` work for a fresh editor too.
- `srw -n` starts an editor that does not take over the endpoint while another one has it.
- The listener is one thread blocked in `accept`; an idle shrew still uses no CPU.

## Bringing the window to the front

- **X11, Windows, macOS:** the window is focused (`focus_window`). Some window managers flash the task bar entry
  instead when focus stealing prevention is strict.
- **Wayland** (KDE Plasma, GNOME, ...): a window cannot take the focus by itself. A new shrew uses the activation
  token that launchers and file managers pass in `XDG_ACTIVATION_TOKEN` (the `.desktop` file has
  `StartupNotify=true`), so it opens focused. For a running shrew, `srw` forwards its token, but winit 0.30 can apply
  a token only to a window it is creating, so shrew asks the compositor for attention instead: on KDE Plasma the task
  bar entry is highlighted and one click raises it; the files are already open.

## Big files

Files over 1 MB are read on a background thread wherever they are opened from (`srw`, `:e`, the file picker, drag
and drop on the window); a "Loading" indicator shows meanwhile and the cursor moves to the requested line once the
file is in. Smaller files open synchronously, so there is no flicker.
