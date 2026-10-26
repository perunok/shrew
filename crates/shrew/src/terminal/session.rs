//! One terminal: a shell (or a `:term` command) on a pseudo-terminal, its screen and the thread that reads it.
//!
//! alacritty_terminal does the emulation: its PTY thread reads the program's output and parses it straight into the
//! shared [`Term`], then calls our [`Listener`]. The listener answers the program's queries itself (device
//! attributes, colors, sizes) and wakes the UI only when the previous wake has been drawn, so a program flooding
//! the terminal costs at most one redraw per frame and an idle one costs nothing.

use crate::state::Waker;
use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::Rgb;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Grid size in cells and the cell size in pixels (programs may ask for both).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
    pub cell_w: u16,
    pub cell_h: u16,
}

impl Size {
    fn window(self) -> WindowSize {
        WindowSize {
            num_lines: self.rows.min(u16::MAX as usize) as u16,
            num_cols: self.cols.min(u16::MAX as usize) as u16,
            cell_width: self.cell_w,
            cell_height: self.cell_h,
        }
    }
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// The colors programs can ask for (OSC 4/10/11/12): the 16 ANSI colors, then foreground, background, cursor.
pub type Palette = [Rgb; 19];

/// What the PTY thread and the UI share besides the screen.
pub struct Shared {
    /// The screen changed since the panel last drew it (the UI clears it; set again, the next wake goes out).
    pub dirty: AtomicBool,
    /// The program exited (and its last output is parsed).
    pub exited: AtomicBool,
    pub exit_code: Mutex<Option<i32>>,
    /// The title the program set (OSC 0/2).
    pub title: Mutex<Option<String>>,
    /// Text the program copied (OSC 52), for the UI to put on the clipboard.
    pub copied: Mutex<Option<String>>,
    /// Milliseconds after `started` of the last output (0: none yet), to tell when a program has gone quiet.
    pub last_output: AtomicU64,
    /// The same for the last input sent to the program (its answer may still be on the way).
    pub last_input: AtomicU64,
    pub started: Instant,
    palette: Mutex<Palette>,
    size: Mutex<WindowSize>,
    sender: OnceLock<EventLoopSender>,
    wake: Waker,
}

impl Shared {
    fn new(palette: Palette, size: Size, wake: Waker) -> Shared {
        Shared {
            dirty: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            exit_code: Mutex::new(None),
            title: Mutex::new(None),
            copied: Mutex::new(None),
            last_output: AtomicU64::new(0),
            last_input: AtomicU64::new(0),
            started: Instant::now(),
            palette: Mutex::new(palette),
            size: Mutex::new(size.window()),
            sender: OnceLock::new(),
            wake,
        }
    }

    /// Without a program: for parsing bytes straight into a screen in tests.
    #[cfg(test)]
    pub fn detached() -> Arc<Shared> {
        let size = Size { cols: 80, rows: 24, cell_w: 8, cell_h: 16 };
        Arc::new(Shared::new([Rgb { r: 0, g: 0, b: 0 }; 19], size, Arc::new(|| {})))
    }

    fn changed(&self) {
        self.last_output.store(self.started.elapsed().as_millis().max(1) as u64, Ordering::Relaxed);
        if !self.dirty.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }

    pub fn write(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.last_input.store(self.started.elapsed().as_millis().max(1) as u64, Ordering::Relaxed);
        if let Some(s) = self.sender.get() {
            let _ = s.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    /// The color a program asked about: an index into the 256-color palette, or 256/257/258 for foreground,
    /// background and cursor.
    fn color(&self, index: usize) -> Rgb {
        let p = *self.palette.lock().unwrap_or_else(|e| e.into_inner());
        match index {
            0..16 => p[index],
            16..256 => indexed(index as u8, &p),
            256 => p[16],
            258 => p[18],
            _ => p[17],
        }
    }
}

/// The 256-color palette beyond the 16 ANSI colors: the 6x6x6 cube and the grey ramp, as xterm defines them.
pub fn indexed(i: u8, p: &Palette) -> Rgb {
    match i {
        0..16 => p[i as usize],
        16..232 => {
            let i = i - 16;
            let v = |c: u8| if c == 0 { 0 } else { 55 + 40 * c };
            Rgb { r: v(i / 36), g: v((i / 6) % 6), b: v(i % 6) }
        }
        _ => {
            let g = 8 + 10 * (i - 232);
            Rgb { r: g, g, b: g }
        }
    }
}

/// Receives the terminal's events on the PTY thread (with the screen locked: it must not lock it again).
#[derive(Clone)]
pub struct Listener(pub Arc<Shared>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let s = &self.0;
        match event {
            TermEvent::Wakeup => s.changed(),
            TermEvent::Title(t) => {
                *s.title.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
                s.changed();
            }
            TermEvent::ResetTitle => {
                *s.title.lock().unwrap_or_else(|e| e.into_inner()) = None;
                s.changed();
            }
            TermEvent::ClipboardStore(_, text) => {
                *s.copied.lock().unwrap_or_else(|e| e.into_inner()) = Some(text);
                s.changed();
            }
            TermEvent::PtyWrite(text) => s.write(text.into_bytes()),
            TermEvent::ColorRequest(index, format) => s.write(format(s.color(index)).into_bytes()),
            TermEvent::TextAreaSizeRequest(format) => {
                let size = *s.size.lock().unwrap_or_else(|e| e.into_inner());
                s.write(format(size).into_bytes());
            }
            TermEvent::ChildExit(status) => {
                *s.exit_code.lock().unwrap_or_else(|e| e.into_inner()) = status.code();
            }
            TermEvent::Exit => {
                s.exited.store(true, Ordering::Release);
                s.changed();
            }
            // pasting from OSC 52 is off; the bell, blinking and pointer shape are not ours to change
            TermEvent::ClipboardLoad(..)
            | TermEvent::Bell
            | TermEvent::CursorBlinkingChange
            | TermEvent::MouseCursorDirty => {}
        }
    }
}

/// The PTY reader thread; it returns the PTY (with the child process) when it stops.
type ReaderThread = std::thread::JoinHandle<(EventLoop<tty::Pty, Listener>, alacritty_terminal::event_loop::State)>;

/// How to start a terminal.
#[derive(Clone, Debug, Default)]
pub struct Spawn {
    /// A command line for the shell to run (`:term make`); `None` runs the shell itself.
    pub command: Option<String>,
    /// The shell instead of `$SHELL` (config `terminal.shell`).
    pub shell: Option<String>,
    pub cwd: PathBuf,
    pub scrollback: usize,
    /// Extra environment (the single-instance socket, so `srw file` opens in this window).
    pub env: Vec<(String, String)>,
}

pub struct Session {
    pub term: Arc<FairMutex<Term<Listener>>>,
    pub shared: Arc<Shared>,
    sender: EventLoopSender,
    thread: Option<ReaderThread>,
    pid: u32,
    pub size: Size,
    /// Where it started (the shell may `cd` away: see [`Session::cwd`]).
    pub start_dir: PathBuf,
    pub command: Option<String>,
}

impl Session {
    pub fn spawn(spawn: &Spawn, size: Size, palette: Palette, wake: Waker) -> std::io::Result<Session> {
        let shared = Arc::new(Shared::new(palette, size, wake));
        let listener = Listener(shared.clone());
        let config = TermConfig { scrolling_history: spawn.scrollback, ..TermConfig::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));

        let mut env: std::collections::HashMap<String, String> = [
            ("TERM", "xterm-256color"),
            ("COLORTERM", "truecolor"),
            ("TERM_PROGRAM", "shrew"),
            ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        env.extend(spawn.env.iter().cloned());
        let shell = shell(spawn);
        let options = tty::Options {
            shell,
            working_directory: Some(spawn.cwd.clone()),
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: false,
        };
        let pty = tty::new(&options, size.window(), 0)?;
        #[cfg(unix)]
        let pid = pty.child().id();
        #[cfg(not(unix))]
        let pid = 0;
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = event_loop.channel();
        let _ = shared.sender.set(sender.clone());
        let thread = event_loop.spawn();
        Ok(Session {
            term,
            shared,
            sender,
            thread: Some(thread),
            pid,
            size,
            start_dir: spawn.cwd.clone(),
            command: spawn.command.clone(),
        })
    }

    pub fn write(&self, bytes: Vec<u8>) {
        self.shared.write(bytes);
    }

    pub fn exited(&self) -> bool {
        self.shared.exited.load(Ordering::Acquire)
    }

    /// Resizes the screen and tells the program (SIGWINCH). Nothing happens when the size is the same.
    pub fn resize(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        self.size = size;
        *self.shared.size.lock().unwrap_or_else(|e| e.into_inner()) = size.window();
        self.term.lock().resize(size);
        let _ = self.sender.send(Msg::Resize(size.window()));
    }

    pub fn set_palette(&self, palette: Palette) {
        *self.shared.palette.lock().unwrap_or_else(|e| e.into_inner()) = palette;
    }

    /// The shell's current directory: `/proc` knows it on Linux; elsewhere where it started.
    pub fn cwd(&self) -> PathBuf {
        #[cfg(target_os = "linux")]
        if self.pid != 0
            && let Ok(p) = std::fs::read_link(format!("/proc/{}/cwd", self.pid))
        {
            return p;
        }
        self.start_dir.clone()
    }

    /// The tab's name: the program's title, else the command or the shell's directory.
    pub fn title(&self) -> String {
        if let Some(t) = self.shared.title.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|t| !t.is_empty()) {
            return t;
        }
        if let Some(c) = &self.command {
            return c.clone();
        }
        short_dir(&self.cwd())
    }

    // ----- updates -----
    /// Ends the program and waits for it, on this thread: before shrew re-executes itself (restart to update),
    /// where the background close of `drop` would never run and the PTY, which is not close-on-exec, would keep the
    /// shell alive.
    #[cfg(feature = "update")]
    pub fn close_now(mut self) {
        let _ = self.sender.send(Msg::Shutdown);
        if let Some(t) = self.thread.take() {
            drop(t.join());
        }
    }
    // -----
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
        // the reader thread hands the PTY back when it ends; closing it (SIGHUP to the shell, then reaping it)
        // happens on a short-lived thread so the UI never waits for a program to exit
        if let Some(t) = self.thread.take() {
            let _ = std::thread::Builder::new().name("shrew-term-close".into()).spawn(move || drop(t.join()));
        }
    }
}

/// The program: the configured shell or a command line run by it; `None` lets alacritty pick the user's shell
/// (`$SHELL`, a login shell on macOS, PowerShell on Windows).
fn shell(spawn: &Spawn) -> Option<tty::Shell> {
    let user_shell = || {
        spawn
            .shell
            .clone()
            .or_else(|| std::env::var("SHELL").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| if cfg!(windows) { "powershell.exe".to_string() } else { "/bin/sh".to_string() })
    };
    match &spawn.command {
        Some(cmd) if cfg!(windows) => Some(tty::Shell::new("cmd.exe".into(), vec!["/C".into(), cmd.clone()])),
        Some(cmd) => Some(tty::Shell::new(user_shell(), vec!["-c".into(), cmd.clone()])),
        None => spawn.shell.as_ref().map(|_| tty::Shell::new(user_shell(), Vec::new())),
    }
}

/// `~/code/shrew` for a directory under the home directory.
pub fn short_dir(p: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
    }
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_256_color_palette() {
        let p = [Rgb { r: 1, g: 2, b: 3 }; 19];
        assert_eq!(indexed(5, &p), Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(indexed(16, &p), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(indexed(196, &p), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(indexed(231, &p), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(indexed(232, &p), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(indexed(255, &p), Rgb { r: 238, g: 238, b: 238 });
    }

    #[test]
    fn home_is_shortened() {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            assert_eq!(short_dir(&home), "~");
            assert_eq!(short_dir(&home.join("a/b")), "~/a/b");
        }
        assert_eq!(short_dir(Path::new("/definitely/not/home")), "/definitely/not/home");
    }
}
