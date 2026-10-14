//! What `srw [files]` does, like Sublime's `subl`:
//!
//! 1. A shrew is running for this user: hand it the request over [`ipc`] (it opens the files and comes to the
//!    front) and exit at once, or with `--wait` once the files are closed.
//! 2. None is: start one detached from the terminal (own session, no terminal stdio) and hand it the request over
//!    its stdin/stdout pipes. The prompt comes back as soon as it has taken the request, a few milliseconds.
//! 3. `-f`: run the editor in this process instead (debugging: logs on stderr).
//!
//! The detached editor is this same binary run with [`cli::INSTANCE_FLAG`]. It tries to become the instance; when
//! another `srw` won that race it forwards the request there and exits.

use crate::cli::{self, Cli};
use crate::ipc::{self, Bound, FileArg, Reply, Request};
use std::io::{self, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub fn run(cli: Cli) -> Result<(), String> {
    if cli.instance {
        // started by `start_detached`: the request is on stdin, answers go to stdout
        let req = ipc::read_line::<Request>(&mut io::stdin().lock()).ok().flatten().unwrap_or_default();
        return gui(cli, req, Some(Box::new(io::stdout())));
    }
    let req = request(&cli, true)?;
    if cli.foreground {
        return gui(cli, req, None);
    }
    // in shrew's own terminal: the window that terminal is in
    let inside = std::env::var_os("SHREW_INSTANCE").filter(|a| !a.is_empty()).map(PathBuf::from);
    if !cli.new_window
        && let Some(client) = inside
            .as_deref()
            .and_then(ipc::connect)
            .or_else(|| ipc::default_address().as_deref().and_then(ipc::connect))
    {
        match client.request(&req) {
            Ok(()) => return Ok(()),
            Err(e) => log::warn!("the running shrew did not take the files ({e}): starting another"),
        }
    }
    start_detached(&cli, &req)
}

/// The request for the command line: paths made absolute against this process's directory (the instance may run
/// elsewhere), a directory argument as the project folder, stdin read for `-` when `read_stdin`.
pub fn request(cli: &Cli, read_stdin: bool) -> Result<Request, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut req = Request { cwd: cwd.clone(), wait: cli.wait, ..Request::default() };
    for f in &cli.files {
        let path = if f.path.is_absolute() { f.path.clone() } else { cwd.join(&f.path) };
        if path.is_dir() {
            req.dir = Some(path);
        } else {
            req.files.push(FileArg { path, ..f.clone() });
        }
    }
    if cli.stdin && read_stdin {
        let mut bytes = Vec::new();
        io::stdin().lock().read_to_end(&mut bytes).map_err(|e| format!("stdin: {e}"))?;
        req.stdin_text = Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    req.activation_token = ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|t| !t.is_empty()));
    Ok(req)
}

/// Runs the editor here. Becomes the single instance unless one is running; then, unless `-n`, the request goes
/// to that one (two `srw`s started at the same moment) and this process exits.
fn gui(cli: Cli, req: Request, reply: Option<Box<dyn Write + Send>>) -> Result<(), String> {
    let server = match ipc::default_address().map(|a| ipc::bind(&a)) {
        Some(Bound::Server(s)) => Some(s),
        Some(Bound::Running(client)) if !cli.new_window => return forward(client, &req, reply),
        Some(Bound::Running(_)) | None => None,
        Some(Bound::Unavailable(e)) => {
            log::warn!("single instance off: {e}");
            None
        }
    };
    crate::app::run(cli, req, reply, server)
}

fn forward(client: ipc::Client, req: &Request, reply: Option<Box<dyn Write + Send>>) -> Result<(), String> {
    let r = client.request(req);
    if let Some(mut w) = reply {
        let _ = match &r {
            Ok(()) => ipc::write_line(&mut w, &Reply::Ok).and_then(|_| ipc::write_line(&mut w, &Reply::Closed)),
            Err(e) => ipc::write_line(&mut w, &Reply::Error(e.clone())),
        };
    }
    r
}

/// Starts the editor detached from this terminal and hands it `req`; returns once it has taken it.
fn start_detached(cli: &Cli, req: &Request) -> Result<(), String> {
    // an AppImage's mount goes away with this process: start the AppImage itself
    let exe = std::env::var_os("APPIMAGE")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .map_or_else(std::env::current_exe, Ok)
        .map_err(|e| format!("cannot find my own executable: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.arg(cli::INSTANCE_FLAG);
    if cli.new_window {
        cmd.arg("--new-window");
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    detach(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("cannot start shrew: {e}"))?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err("cannot start shrew: no pipes".into());
    };
    log::info!("started shrew (pid {})", child.id());
    ipc::request(stdin, &mut BufReader::new(stdout), req)
        .map_err(|e| if e == ipc::EXITED { format!("{e} (run `{} -f` to see why)", cli::program_name()) } else { e })
}

/// Own session (Unix) or no console (Windows): closing the terminal does not take the editor down.
fn detach(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe; nothing else runs between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_resolves_against_the_cli_directory() {
        let cwd = std::env::current_dir().unwrap();
        let abs = cwd.join("x.rs").to_string_lossy().into_owned();
        let cli = Cli::parse(["src/main.rs:3:2".into(), "src".into(), abs, "-w".into()]).unwrap();
        let req = request(&cli, false).unwrap();
        assert_eq!(req.cwd, cwd);
        assert_eq!(
            req.files,
            vec![
                FileArg { path: cwd.join("src/main.rs"), line: Some(3), col: Some(2) },
                FileArg { path: cwd.join("x.rs"), line: None, col: None },
            ]
        );
        assert_eq!(req.dir, Some(cwd.join("src")), "a directory is the project folder");
        assert!(req.wait);
        assert!(req.stdin_text.is_none());
    }
}
