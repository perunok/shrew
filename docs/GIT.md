# Git in shrew

shrew works with the repository of the files you open, without a plugin and without leaving the keyboard (the mouse
works everywhere too). This page explains how it is built, what each part does, how to switch parts off and what to
check when something looks wrong. Every key is in [KEYBINDINGS.md](KEYBINDINGS.md#git), every setting in
[CONFIG.md](CONFIG.md#git).

![The change markers, a peeked change, the cursor line's author and the sidebar's file states](screenshots/git-gutter.png)

- [How it works](#how-it-works)
- [What each part does](#what-each-part-does)
- [Switching parts off](#switching-parts-off)
- [Assistants (MCP)](#assistants-mcp)
- [Performance](#performance)
- [Troubleshooting](#troubleshooting)

## How it works

**Reads happen inside shrew, writes go through the `git` program.** Everything shrew only looks at (status, the
base text for change markers, log, refs, commit details, blobs for diffs) is read in-process by
[gix](https://github.com/GitoxideLabs/gitoxide), a pure-Rust implementation of git's storage. Line and word diffs are
computed with `imara-diff`. Everything that changes the repository or talks to the network (stage, unstage, commit,
checkout, stash, fetch, pull, push, merge conflict resolution) runs your `git`, with the repository's own config, so
hooks, signing, credential helpers, ssh and `core.editor` behave exactly as in a terminal. `git blame` also runs
through it. shrew never prompts for a password itself (`GIT_TERMINAL_PROMPT=0`): a push that needs one fails with
git's message, so configure a credential helper or ssh key as usual.

**Nothing blocks the window.** One background service (`crates/shrew-git`) runs four lanes, each a thread that
sleeps until it has work: fast reads (finding the repository, the change markers), other reads (status, log, refs,
diffs of trees), blame and writes, so slow work never holds up fast work. The window sends a request and carries on; the answer arrives through the event loop, and every part of the UI keeps
the ticket of what it asked and ignores replies that are not its own. Requests coalesce: while you type, only the
newest version of a buffer is diffed.

**Lazy, and only where it matters.**

- Nothing starts before the first frame is on screen and a file or folder has been looked up. A file outside a
  repository costs one lookup (the answer is remembered per folder) and nothing more; shrew keeps no timer for it.
- The service watches the repositories in use: the open files', the sidebar project's, the open diff tabs' and the
  Source Control panel's. It looks at a handful of file stamps every 3 seconds, **only while the window has the
  focus**, and re-reads everything when the focus comes back, after saving a file of the repository, and after every
  git command. A repository nobody uses any more is dropped.
- Unfocused, shrew does nothing at all.

**Every part is optional.** Cargo feature `git` compiles all of it out, gix and imara-diff included (the features below
then show as unavailable in `srw --features` and Settings). At runtime each part has its own switch, see [Switching parts off](#switching-parts-off).

## What each part does

### Change markers

![Change markers and the peek](screenshots/git-gutter.png)

The gutter, the minimap and the scrollbar mark lines added (bar), changed (bar) and deleted (triangle) against the
**index** by default, so what you staged drops out, as in VS Code and Sublime Merge; `[git] gutter_base = "head"`
compares with the last commit instead. The marks follow your typing 30 ms after a pause (at the latest every 150 ms
while you keep typing), computed on the buffer's text, not the file on disk.

`]c` / `[c` jump between changes, `ghp` (or a click on a marker) peeks at the old lines under the change with the
changed words marked, and `ghs` / `ghu` / `ghr` stage, unstage or revert the change, or only the selected lines in
visual mode. A revert is an ordinary edit, so `u` undoes it. Untracked, ignored, binary, conflicted and files over
8 MB get no markers.

### Inline blame and status

When the cursor rests on a line for 300 ms (`[git] blame_delay`), its last author, when and the commit's summary show
faintly after the line. The branch (with `*` for changes and `↑2 ↓1` against the upstream) sits in the status bar and
next to the project in the title bar, and the sidebar colours files by state with git's letters.

### Diff view

![A file diff, side by side](screenshots/git-diff.png)

A diff is a tab of its own: one file against the index or HEAD (`:Gdiff`, `:Gdiff HEAD`), all unstaged or staged
changes, a commit (`:Gshow`), or two revisions (`:git_diff_compare a b`). Side by side or unified (`t`), with syntax
colours, the changed words marked, unchanged lines folded and a header per file that folds. When the working-tree
side is open in a buffer, the diff shows your unsaved edits as you type. `s`, `u` and `X` stage, unstage or revert
the change under the cursor or the selected lines; `w` ignores white space; `<` and `>` change the context.

### Source Control

![The Source Control panel](screenshots/git-panel.png)

Ctrl+Shift+G opens the sidebar's Git tab: the branch with its upstream and a sync button (pull, push, publish or
fetch), the commit message box (Ctrl+Enter commits, Ctrl+Shift+Enter amends, a 50/72 hint for the subject), and the
changes in sections: Merge Changes, Staged Changes, Changes and Untracked. Stage, unstage and discard by key or
button; Enter opens the change in the diff view. Branches, stashes and the changed files are lists (`b`, `z`);
fetch, pull and push show a toast with git's progress and a cancel button, and a failed command keeps git's whole
message on screen until you dismiss it.

### Log

![The log with its graph](screenshots/git-log.png)

`git_log` (or `L` in the panel) draws the history with its graph, branch and tag labels, authors and ages, and loads
more as you scroll. `git_log_all` starts from every branch. Enter shows a commit's message and files, `d` opens its
whole diff, `/` searches, `a` filters by author, `p` shows one path's history (following renames), `b` creates a
branch there and `y` copies the id.

### Blame

![The blame column](screenshots/git-blame.png)

`git_blame` adds a column left of the text: each run of lines from one commit shows its id, author, age and summary,
with a bar coloured by age. The first lines appear while git is still walking a long history, and the buffer is
blamed as it is, so unsaved lines read as not committed yet. A click on a run opens what that commit did to the file.

### Merge conflicts

While git lists the file as conflicted, the current side, the base and the incoming side are tinted, and each
`<<<<<<<` line offers *Accept Current*, *Accept Incoming*, *Accept Both* and *Compare* (every one is an undo step).
`]x` / `[x` jump between conflicts, and *Mark Resolved* saves and stages the file.

### Colours

Git has theme tokens of its own (`git-added` to `git-conflict`, `diff-add-bg`, `diff-del-bg`, `diff-add-word`,
`diff-del-word`), derived for every built-in theme and checked for contrast like the rest; custom themes can set them
([THEMES.md](THEMES.md#ui-tokens)).

## Switching parts off

| Feature (`:set`, Settings › Git) | Part |
| --- | --- |
| `git_gutter` | change markers, `]c` / `[c`, the peek, staging from the gutter |
| `git_blame_inline` | the cursor line's author |
| `git_status` | branch in the status bar and title bar, file states in the sidebar |
| `git_panel` | Source Control, the log, the blame column, conflicts, branches and the network commands |

The diff view belongs to cargo feature `git` alone: its commands are in the palette whenever the build has it.
`cargo build --no-default-features --features "..."` without `git` removes the dependency on gix and imara-diff
entirely; the git features then read as unavailable in `:features` and Settings. Settings is under
**Settings › Git** (Ctrl+,), where every switch has its related settings next to it.

## Assistants (MCP)

With the MCP server on ([README](../README.md#ai-assistants-mcp)), assistants also get five **read-only** git tools.
They use the repository of shrew's working directory, run on the server's own threads (the window is only asked for
the directory and, for a file, the open buffer), and cannot stage, commit, check out, fetch or push.

| Tool | Gives |
| --- | --- |
| `git_status` | The branch, upstream, ahead / behind, a merge or rebase in progress, and every changed file with its two-letter code |
| `git_diff` | A unified diff: `unstaged` (default), `staged`, `head`, or `commit` (`rev`, optionally against `base`), for the whole repository or one path; capped at 200 KB |
| `git_log` | A page of history (`skip`, `limit`), optionally from a `rev`, for all branches or one file's history, each commit with its labels |
| `git_blame` | Who last changed each line of a file, for a line range; uses the open buffer's text when it has unsaved edits |
| `git_show_commit` | One commit: message, author, date, labels and changed files, with its diff when asked |

Files outside the repository are refused. Unsaved edits are not part of status and diffs (the answer says so when the
file has some); blame is the exception. There is deliberately no tool that changes the repository: stage and commit
in the Source Control panel, where you see what you commit.

## Performance

The numbers (and how to repeat them) are in the README's [Performance](../README.md#performance) section; `cargo test
--release -p shrew-git --test perf -- --ignored --nocapture` measures the git side, and `srw --screenshot out.png
--stats --keys ":Gdiff<CR>" file` the diff view's frames. The shape of it: the window thread only hands out
requests and files replies away (a reply that takes it over 4 ms is logged by `srw -f --stats`); the diff behind the
change markers of a 10,000-line file takes under a millisecond, and the markers are ready about 31 ms after you stop
typing; the status of a 20,000-file repository takes about 15 ms on the worker (as long as `git status` does); the
first 200 commits of a 20,000-commit history take half a millisecond; a blame streams in while git works; and a full
frame of a big diff costs under half a millisecond of CPU. Starting shrew is not slower in a repository: git is
not touched before the first frame, and the idle cost is one wake-up every 3 seconds.

## Troubleshooting

**Nothing git-related shows up.** The file must be inside a repository: run `git rev-parse --show-toplevel` in its
folder. Bare repositories, files inside `.git` and SHA-256 repositories are not supported. Then check that the
feature is on (`:features`, Settings › Git) and that this build has git (`srw --features` lists what is compiled in; a
build without cargo feature `git` shows the settings as "not compiled into this build").

**Committing, pushing or switching branches fails.** shrew needs a `git` program on the `PATH` for writes; without
one, reading still works and the toast says that git could not be started. The toast keeps git's whole message
(hooks included) until you dismiss it, and *Git: Show Output* opens the last output in a buffer. Pushes that ask
for a password cannot prompt: use a credential helper or `ssh-agent`. `commit.gpgsign` and
other signing setups work as in a terminal, but need an agent or a cached passphrase for the same reason.

**The markers or the branch are out of date.** The repository is re-read when the window gets the focus back and a
few seconds after anything moves while it has it. Press `r` in the Source Control panel (or run *Git: Refresh*) to
read it now. If a tool replaced the index in an unusual way (`git update-index`, an IDE's own writes), a refresh
shows the same.

**A file has no markers.** Untracked, ignored, binary, conflicted or very large files (over 8 MB) are not diffed.
Staged-but-unchanged lines are not marked either, by design: `[git] gutter_base = "head"` marks everything since the
last commit. With `core.autocrlf` or CRLF files, line endings alone do not count as changes in the markers and in diffs against the working tree.

**Status is slow in a huge repository.** Status reads the index and stats the files in parallel, so it is bound by
your disk's metadata speed. Ignored folders are listed as one entry and never walked into; untracked folders are
shown collapsed. Large untracked trees that are not ignored are the usual cause: add them to `.gitignore`. If the status still
costs too much, the git features can be switched off in Settings › Git (or `:set nogit_gutter nogit_status
nogit_blame_inline`) and the Source Control panel closed; nothing then reads the repository until you ask.

**Submodules, worktrees.** Linked worktrees and submodules are separate repositories as far as shrew is concerned
(they are found from the file you open, with their own status). A submodule shows as one entry in its parent's
status.

**Idle CPU.** With the window focused, a repository in use costs one wake-up every 3 seconds that stats about five
files. Unfocused, or with no file inside a repository open, nothing wakes at all.
