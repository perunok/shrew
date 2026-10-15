//! Running the `git` program for writes, network and the few reads gix does not cover (blame).
//!
//! Every run targets one repository explicitly (`GIT_DIR` and `GIT_WORK_TREE`), never prompts
//! (`GIT_TERMINAL_PROMPT=0`, stdin closed unless we feed it), speaks C locale for parsing, and opens no editor.
//! Paths go to pathspec commands as `:(literal)path`, so `*` or `:` in a file name is never a pattern
//! (`GIT_LITERAL_PATHSPECS` would also break git's own internal pathspecs, as in `stash -u`).

use crate::error::{Error, ErrorKind, Result};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Cancels a running operation: a `git` process (and its children, such as ssh) is killed, a gix read stops at
/// its next check. Clone it to keep a handle while the operation runs elsewhere.
#[derive(Clone, Default)]
pub struct Cancel(Arc<CancelInner>);

#[derive(Default)]
struct CancelInner {
    flag: Arc<AtomicBool>,
    child: Mutex<Option<u32>>,
}

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        if let Some(pid) = *self.0.child.lock().unwrap_or_else(|e| e.into_inner()) {
            kill_tree(pid);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    /// The flag gix polls.
    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        self.0.flag.clone()
    }

    fn set_child(&self, pid: Option<u32>) {
        *self.0.child.lock().unwrap_or_else(|e| e.into_inner()) = pid;
    }
}

#[cfg(unix)]
fn kill_tree(pid: u32) {
    // the child leads its own process group, so this reaches ssh and remote helpers too
    unsafe {
        libc::kill(-(pid as i32), libc::SIGTERM);
    }
}

#[cfg(windows)]
fn kill_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(any(unix, windows)))]
fn kill_tree(_pid: u32) {}

/// What a successful run printed.
#[derive(Debug, Default)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    /// stdout and stderr together, trimmed (what a hook or command told the user).
    pub fn text(&self) -> String {
        join_output(&self.stdout, &self.stderr)
    }
}

fn join_output(stdout: &[u8], stderr: &str) -> String {
    let out = String::from_utf8_lossy(stdout);
    let (a, b) = (out.trim(), stderr.trim());
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_string(),
        (_, true) => a.to_string(),
        _ => format!("{a}\n{b}"),
    }
}

/// Receives stdout line by line (terminator included).
pub(crate) type StdoutSink<'a> = &'a mut dyn FnMut(&[u8]);

/// One `git` invocation.
pub(crate) struct Git {
    cmd: Command,
    input: Option<Vec<u8>>,
}

/// Where `git` should operate.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub git_dir: PathBuf,
    pub workdir: PathBuf,
}

impl Git {
    pub fn new(target: &Target) -> Git {
        let mut cmd = Command::new(git_program());
        cmd.current_dir(&target.workdir)
            .env("GIT_DIR", &target.git_dir)
            .env("GIT_WORK_TREE", &target.workdir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("GIT_EDITOR", ":")
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        Git { cmd, input: None }
    }

    pub fn arg(mut self, a: impl AsRef<std::ffi::OsStr>) -> Git {
        self.cmd.arg(a);
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Git
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        self.cmd.args(args);
        self
    }

    /// For read-only commands: do not take `index.lock` to refresh stat data (`git status` from an editor must
    /// never make the user's own `git` fail).
    /// `--` and then `paths` as literal pathspecs.
    pub fn pathspecs(self, paths: &[String]) -> Git {
        self.arg("--").args(paths.iter().map(|p| format!(":(literal){p}")))
    }

    pub fn read_only(mut self) -> Git {
        self.cmd.env("GIT_OPTIONAL_LOCKS", "0");
        self
    }

    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Git {
        self.input = Some(input.into());
        self.cmd.stdin(Stdio::piped());
        self
    }

    fn spawn(&mut self, own_group: bool) -> Result<Child> {
        #[cfg(unix)]
        if own_group {
            use std::os::unix::process::CommandExt;
            self.cmd.process_group(0);
        }
        let _ = own_group;
        self.cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::new(ErrorKind::GitMissing, "git is not installed (or not on PATH)")
            } else {
                Error::new(ErrorKind::GitMissing, format!("could not start git: {e}"))
            }
        })
    }

    fn feed(&mut self, child: &mut Child) -> Option<std::thread::JoinHandle<()>> {
        let input = self.input.take()?;
        let mut stdin = child.stdin.take()?;
        Some(std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        }))
    }

    /// Runs to completion. A non-zero exit is an error carrying git's output.
    pub fn run(mut self) -> Result<Output> {
        let mut child = self.spawn(false)?;
        let feeder = self.feed(&mut child);
        let out = child.wait_with_output()?;
        if let Some(f) = feeder {
            let _ = f.join();
        }
        finish(out.status, out.stdout, String::from_utf8_lossy(&out.stderr).into_owned(), None)
    }

    /// Runs with `cancel` able to kill it, calling `on_line` for every line (or `\r`-separated progress update)
    /// git writes to stderr, and `on_stdout` for every stdout line if given.
    pub fn stream(
        mut self,
        cancel: &Cancel,
        on_line: &mut dyn FnMut(&str),
        mut on_stdout: Option<StdoutSink>,
    ) -> Result<Output> {
        if cancel.is_cancelled() {
            return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
        }
        let mut child = self.spawn(true)?;
        cancel.set_child(Some(child.id()));
        // cancelled between the check above and registering the child
        if cancel.is_cancelled() {
            kill_tree(child.id());
        }
        let feeder = self.feed(&mut child);
        let mut stderr = child.stderr.take().expect("piped");
        let mut stdout = child.stdout.take().expect("piped");

        // Both pipes are read on helper threads and delivered here, so callbacks run on this thread as data arrives.
        enum Chunk {
            Err(String),
            Out(Vec<u8>),
        }
        let (tx, rx) = std::sync::mpsc::channel::<Chunk>();
        let tx_out = tx.clone();
        let err_thread = std::thread::spawn(move || {
            let mut all = String::new();
            let mut buf = [0u8; 4096];
            let mut pending = Vec::new();
            loop {
                let n = match stderr.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        let line = String::from_utf8_lossy(&pending).into_owned();
                        pending.clear();
                        if b == b'\n' {
                            all.push_str(&line);
                            all.push('\n');
                        }
                        if !line.trim().is_empty() {
                            let _ = tx.send(Chunk::Err(line));
                        }
                    } else {
                        pending.push(b);
                    }
                }
            }
            if !pending.is_empty() {
                let line = String::from_utf8_lossy(&pending).into_owned();
                all.push_str(&line);
                let _ = tx.send(Chunk::Err(line));
            }
            all
        });
        let out_thread = std::thread::spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx_out.send(Chunk::Out(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut out = Vec::new();
        let mut line = Vec::new();
        for chunk in rx {
            match chunk {
                Chunk::Err(l) => on_line(&l),
                Chunk::Out(bytes) => {
                    if let Some(cb) = on_stdout.as_mut() {
                        for &b in &bytes {
                            line.push(b);
                            if b == b'\n' {
                                cb(&line);
                                line.clear();
                            }
                        }
                    }
                    out.extend_from_slice(&bytes);
                }
            }
        }
        if let Some(cb) = on_stdout.as_mut()
            && !line.is_empty()
        {
            cb(&line);
        }
        let all_err = err_thread.join().unwrap_or_default();
        let _ = out_thread.join();
        let status = child.wait()?;
        cancel.set_child(None);
        if let Some(f) = feeder {
            let _ = f.join();
        }
        finish(status, out, all_err, Some(cancel))
    }
}

fn finish(status: ExitStatus, stdout: Vec<u8>, stderr: String, cancel: Option<&Cancel>) -> Result<Output> {
    if status.success() {
        return Ok(Output { stdout, stderr });
    }
    if cancel.is_some_and(Cancel::is_cancelled) {
        return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
    }
    let mut message = join_output(&stdout, &stderr);
    if message.is_empty() {
        message = format!("git failed ({status})");
    }
    Err(Error::new(ErrorKind::Git(status.code()), message))
}

/// The `git` to run: `$SHREW_GIT` if set (tests, unusual installs), else `git` from PATH.
fn git_program() -> PathBuf {
    std::env::var_os("SHREW_GIT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("git"))
}

/// Whether a `git` program can be started (cached after the first call).
pub fn git_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new(git_program())
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}
