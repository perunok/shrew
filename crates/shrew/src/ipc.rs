//! Single instance: the running shrew listens on a per-user endpoint and `srw` hands it what to open.
//!
//! The protocol is one JSON [`Request`] line from the client, then [`Reply`] lines from the instance: `"ok"` once the
//! request is queued for the UI thread (or `{"error": ...}`), then, for `--wait`, `"closed"` once every buffer the
//! request opened is closed. The same exchange runs over the stdin/stdout pipes of an instance `srw` starts itself.
//!
//! - Unix: a socket at `$XDG_RUNTIME_DIR/shrew/instance.sock`, else `/tmp/shrew-<uid>/instance.sock` (the directory
//!   is created 0700 and must belong to us). A socket nobody accepts on is stale: removed, and the next shrew binds.
//! - Windows: loopback TCP on a random port; `%LOCALAPPDATA%\shrew\instance` holds `<port> <token>` and every
//!   request must carry the token, so other users' processes cannot drive the editor.
//!
//! Only std (plus serde for the JSON): one thread blocked in `accept`, one short-lived thread per connection.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

/// A request line (stdin text included) bigger than this is refused.
const MAX_LINE: u64 = 1 << 30;
/// The error when the instance went away without answering.
pub const EXITED: &str = "shrew exited before opening anything";
/// How long a peer may take to send its request or the first answer.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A file to open, with an optional 1-based line and char column.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileArg {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col: Option<usize>,
}

/// What one `srw` invocation asks for. Paths are absolute (resolved against the client's `cwd`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub cwd: PathBuf,
    pub files: Vec<FileArg>,
    /// A directory argument: the new project folder, where the file picker opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    /// `srw -`: text for a new scratch buffer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin_text: Option<String>,
    /// Answer `closed` once the buffers this opened are closed.
    pub wait: bool,
    /// The client's `XDG_ACTIVATION_TOKEN` / `DESKTOP_STARTUP_ID`, for raising the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation_token: Option<String>,
    /// The endpoint's secret (Windows; empty on Unix, where the socket's directory is private).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reply {
    Ok,
    Closed,
    Error(String),
}

/// Hands a request to the UI thread, with where to report that its buffers closed (`--wait`). False when the app
/// is going away.
pub type Deliver = Arc<dyn Fn(Request, Option<Sender<()>>) -> bool + Send + Sync>;

pub fn write_line<T: Serialize>(mut w: impl Write, v: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(v).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// The next JSON line, `None` at the end of the stream.
pub fn read_line<T: DeserializeOwned>(r: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut buf = Vec::new();
    r.take(MAX_LINE).read_until(b'\n', &mut buf)?;
    if buf.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(&buf).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Sends `req` over `w` and reads the answers from `r`: returns once the request is taken (and, for `--wait`, once
/// its buffers are closed or the editor exited), or with the instance's error.
pub fn request(w: impl Write, r: &mut impl BufRead, req: &Request) -> Result<(), String> {
    send(w, req)?;
    if !first_answer(r)? && req.wait {
        wait_closed(r);
    }
    Ok(())
}

fn send(mut w: impl Write, req: &Request) -> Result<(), String> {
    write_line(&mut w, req).map_err(|e| format!("cannot reach shrew: {e}"))
}

/// `ok` or an error. True when the instance already answered `closed` (nothing to wait for).
fn first_answer(r: &mut impl BufRead) -> Result<bool, String> {
    match read_line::<Reply>(r) {
        Ok(Some(Reply::Ok)) => Ok(false),
        Ok(Some(Reply::Closed)) => Ok(true),
        Ok(Some(Reply::Error(e))) => Err(e),
        Ok(None) => Err(EXITED.into()),
        Err(e) => Err(format!("no answer from shrew: {e}")),
    }
}

/// Until `closed`, or the end of the stream: an editor that went away (closed, crashed) has nothing left open.
fn wait_closed(r: &mut impl BufRead) {
    while let Ok(Some(reply)) = read_line::<Reply>(r) {
        if reply == Reply::Closed {
            return;
        }
    }
}

/// The instance side of [`request`]: hands `req` to `apply`, answers `ok` on `w`, and for `--wait` answers `closed`
/// from a thread once `apply`'s sender fires (or is dropped).
pub fn answer<W: Write + Send + 'static>(
    req: Request,
    apply: impl FnOnce(Request, Option<Sender<()>>) -> bool,
    mut w: W,
) {
    let wait = req.wait;
    let (tx, rx) = mpsc::channel();
    if !apply(req, wait.then_some(tx)) {
        let _ = write_line(&mut w, &Reply::Error("shrew is closing".into()));
        return;
    }
    if write_line(&mut w, &Reply::Ok).is_err() || !wait {
        return;
    }
    let _ = std::thread::Builder::new().name("shrew-wait".into()).spawn(move || {
        let _ = rx.recv();
        let _ = write_line(&mut w, &Reply::Closed);
    });
}

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(unix)]
type Listener = std::os::unix::net::UnixListener;
#[cfg(not(unix))]
type Stream = std::net::TcpStream;
#[cfg(not(unix))]
type Listener = std::net::TcpListener;

/// A connection to the running instance.
pub struct Client {
    stream: Stream,
    token: String,
}

impl Client {
    pub fn request(self, req: &Request) -> Result<(), String> {
        let mut req = req.clone();
        req.token = self.token;
        send(self.stream.try_clone().map_err(|e| e.to_string())?, &req)?;
        // the first answer is quick; then wait as long as the buffers stay open
        let _ = self.stream.set_read_timeout(Some(TIMEOUT));
        let mut r = BufReader::new(self.stream);
        if !first_answer(&mut r)? && req.wait {
            let _ = r.get_ref().set_read_timeout(None);
            wait_closed(&mut r);
        }
        Ok(())
    }
}

/// The listening side, before it serves.
pub struct Server {
    listener: Listener,
    addr: PathBuf,
    token: String,
}

pub enum Bound {
    /// This process is the instance now.
    Server(Server),
    /// Another shrew answers on the endpoint.
    Running(Client),
    /// No endpoint (no runtime directory, unsafe permissions, ...): run without single-instance.
    Unavailable(String),
}

/// Removes the endpoint when the instance exits, if it is still ours.
pub struct Guard {
    addr: PathBuf,
    #[cfg(unix)]
    ino: u64,
    #[cfg(not(unix))]
    contents: String,
}

impl Guard {
    /// The endpoint (`SHREW_INSTANCE` in the built-in terminal's shells).
    #[cfg_attr(not(feature = "terminal"), allow(dead_code))]
    pub fn addr(&self) -> &Path {
        &self.addr
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if fs::symlink_metadata(&self.addr).is_ok_and(|m| m.ino() == self.ino) {
                let _ = fs::remove_file(&self.addr);
            }
        }
        #[cfg(not(unix))]
        if fs::read_to_string(&self.addr).is_ok_and(|s| s == self.contents) {
            let _ = fs::remove_file(&self.addr);
        }
    }
}

impl Server {
    /// Accepts connections on a thread and hands their requests to `deliver`.
    pub fn serve(self, deliver: Deliver) -> Guard {
        let guard = Guard {
            addr: self.addr.clone(),
            #[cfg(unix)]
            ino: {
                use std::os::unix::fs::MetadataExt;
                fs::symlink_metadata(&self.addr).map_or(0, |m| m.ino())
            },
            #[cfg(not(unix))]
            contents: fs::read_to_string(&self.addr).unwrap_or_default(),
        };
        let Server { listener, token, .. } = self;
        let spawned = std::thread::Builder::new().name("shrew-ipc".into()).spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (deliver, token) = (deliver.clone(), token.clone());
                let _ = std::thread::Builder::new().name("shrew-ipc-conn".into()).spawn(move || {
                    let _ = handle(stream, &deliver, &token);
                });
            }
        });
        if let Err(e) = spawned {
            log::warn!("single instance: {e}");
        }
        guard
    }
}

fn handle(stream: Stream, deliver: &Deliver, token: &str) -> io::Result<()> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    let mut r = BufReader::new(stream.try_clone()?);
    let Some(req) = read_line::<Request>(&mut r)? else { return Ok(()) };
    if req.token != token {
        return write_line(&stream, &Reply::Error("wrong instance token".into()));
    }
    log::info!(
        "request from srw: {} file(s), folder {}, stdin {}, wait {}",
        req.files.len(),
        req.dir.is_some(),
        req.stdin_text.is_some(),
        req.wait
    );
    answer(req, |req, done| deliver(req, done), stream);
    Ok(())
}

/// Where this user's instance listens: the socket (Unix) or the `port token` file (Windows).
#[cfg(unix)]
pub fn default_address() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()).map(PathBuf::from);
    let dir = match runtime.filter(|d| d.is_absolute() && d.is_dir()) {
        Some(d) => d.join("shrew"),
        None => PathBuf::from(format!("/tmp/shrew-{}", uid())),
    };
    if let Err(e) = private_dir(&dir) {
        log::warn!("single instance off: {}: {e}", dir.display());
        return None;
    }
    Some(dir.join("instance.sock"))
}

#[cfg(unix)]
fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Creates `dir` 0700, and checks that it (also when it already existed) is ours and private.
#[cfg(unix)]
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let m = fs::symlink_metadata(dir)?;
    if !m.is_dir() || m.uid() != uid() || m.mode() & 0o077 != 0 {
        return Err(io::Error::other("not a private directory of this user"));
    }
    Ok(())
}

#[cfg(windows)]
pub fn default_address() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty())?).join("shrew");
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("instance"))
}

#[cfg(not(any(unix, windows)))]
pub fn default_address() -> Option<PathBuf> {
    None
}

/// Connects to the instance at `addr`, if one answers there.
#[cfg(unix)]
pub fn connect(addr: &Path) -> Option<Client> {
    Stream::connect(addr).ok().map(|stream| Client { stream, token: String::new() })
}

#[cfg(not(unix))]
pub fn connect(addr: &Path) -> Option<Client> {
    let text = fs::read_to_string(addr).ok()?;
    let (port, token) = text.trim().split_once(' ')?;
    let port: u16 = port.parse().ok()?;
    let sock = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let stream = Stream::connect_timeout(&sock, Duration::from_millis(500)).ok()?;
    Some(Client { stream, token: token.to_string() })
}

/// Becomes the instance at `addr`, unless a live one answers there. A stale endpoint is cleaned up.
///
/// Two shrews starting at once must not both take a stale endpoint for theirs (the second would delete the first's
/// fresh socket): the whole check runs under a lock on `<addr>.lock`.
pub fn bind(addr: &Path) -> Bound {
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(addr.with_extension("lock"));
    let _locked = lock.as_ref().ok().filter(|f| f.lock().is_ok());
    bind_locked(addr)
}

#[cfg(unix)]
fn bind_locked(addr: &Path) -> Bound {
    match Listener::bind(addr) {
        Ok(listener) => return Bound::Server(Server { listener, addr: addr.into(), token: String::new() }),
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {}
        Err(e) => return Bound::Unavailable(e.to_string()),
    }
    if let Some(c) = connect(addr) {
        return Bound::Running(c);
    }
    // left behind by a shrew that crashed or was killed
    log::info!("removing the stale socket {}", addr.display());
    let _ = fs::remove_file(addr);
    match Listener::bind(addr) {
        Ok(listener) => Bound::Server(Server { listener, addr: addr.into(), token: String::new() }),
        Err(e) => Bound::Unavailable(e.to_string()),
    }
}

#[cfg(not(unix))]
fn bind_locked(addr: &Path) -> Bound {
    if let Some(c) = connect(addr) {
        return Bound::Running(c);
    }
    let listener = match Listener::bind(("127.0.0.1", 0)) {
        Ok(l) => l,
        Err(e) => return Bound::Unavailable(e.to_string()),
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => return Bound::Unavailable(e.to_string()),
    };
    let token = new_token();
    let tmp = addr.with_extension(format!("tmp{}", std::process::id()));
    let written = fs::write(&tmp, format!("{port} {token}")).and_then(|_| fs::rename(&tmp, addr));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Bound::Unavailable(e.to_string());
    }
    Bound::Server(Server { listener, addr: addr.into(), token })
}

#[cfg(not(unix))]
fn new_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // weaker, still unguessable from outside this process
        use std::hash::{BuildHasher, Hasher};
        let h = |i: u64| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(i ^ std::process::id() as u64);
            h.finish()
        };
        bytes[..8].copy_from_slice(&h(1).to_le_bytes());
        bytes[8..].copy_from_slice(&h(2).to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shrew-ipc-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(wait: bool) -> Request {
        Request {
            cwd: "/home/me/src".into(),
            files: vec![FileArg { path: "/home/me/src/a.rs".into(), line: Some(42), col: Some(7) }],
            dir: None,
            stdin_text: Some("hi\n".into()),
            wait,
            activation_token: Some("tok".into()),
            token: String::new(),
        }
    }

    #[test]
    fn json_lines() {
        let mut out = Vec::new();
        write_line(&mut out, &sample(true)).unwrap();
        write_line(&mut out, &Reply::Ok).unwrap();
        write_line(&mut out, &Reply::Error("x".into())).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert_eq!(text.lines().count(), 3, "one value per line");
        assert!(text.contains(r#""line":42"#) && text.contains("\"ok\"") && text.contains(r#"{"error":"x"}"#));
        let mut r = &out[..];
        assert_eq!(read_line::<Request>(&mut r).unwrap(), Some(sample(true)));
        assert_eq!(read_line::<Reply>(&mut r).unwrap(), Some(Reply::Ok));
        assert_eq!(read_line::<Reply>(&mut r).unwrap(), Some(Reply::Error("x".into())));
        assert_eq!(read_line::<Reply>(&mut r).unwrap(), None);
        // missing fields take their defaults
        let mut r = &br#"{"files":[{"path":"/a"}]}"#[..];
        let q = read_line::<Request>(&mut r).unwrap().unwrap();
        assert_eq!(q.files[0], FileArg { path: "/a".into(), line: None, col: None });
        assert!(!q.wait);
    }

    /// Serves on a temp endpoint, collecting what arrives (and the `--wait` senders).
    #[allow(clippy::type_complexity)]
    fn serve(dir: &Path) -> (Guard, PathBuf, mpsc::Receiver<(Request, Option<Sender<()>>)>) {
        let addr = dir.join("instance.sock");
        let Bound::Server(server) = bind(&addr) else { panic!("should bind") };
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let deliver: Deliver = Arc::new(move |req, done| tx.lock().unwrap().send((req, done)).is_ok());
        (server.serve(deliver), addr, rx)
    }

    #[test]
    fn round_trip() {
        let dir = temp("round");
        let (_guard, addr, rx) = serve(&dir);
        let client = connect(&addr).expect("connects");
        let req = sample(false);
        client.request(&req).unwrap();
        let (got, done) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(got.files, req.files);
        assert_eq!(got.stdin_text.as_deref(), Some("hi\n"));
        assert!(done.is_none(), "no --wait: nothing to report later");
        // a second instance finds this one
        assert!(matches!(bind(&addr), Bound::Running(_)));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wait_returns_once_the_buffers_close() {
        let dir = temp("wait");
        let (_guard, addr, rx) = serve(&dir);
        let client = connect(&addr).unwrap();
        let (fin_tx, fin_rx) = mpsc::channel();
        std::thread::spawn(move || fin_tx.send(client.request(&sample(true))).unwrap());
        let (_, done) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let done = done.expect("--wait gets a sender");
        assert!(fin_rx.recv_timeout(Duration::from_millis(200)).is_err(), "still waiting while the buffer is open");
        done.send(()).unwrap();
        assert_eq!(fin_rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
        // the editor exiting (the sender dropped) also ends the wait
        let client = connect(&addr).unwrap();
        let (fin_tx, fin_rx) = mpsc::channel();
        std::thread::spawn(move || fin_tx.send(client.request(&sample(true))).unwrap());
        let (_, done) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(done);
        assert_eq!(fin_rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pipe_exchange() {
        // what `srw` and the instance it starts say over the child's stdin/stdout
        let mut to_instance = Vec::new();
        let (w, mut r) = (std::io::Cursor::new(Vec::new()), &b"\"ok\"\n\"closed\"\n"[..]);
        request(w, &mut r, &sample(true)).unwrap();
        write_line(&mut to_instance, &sample(true)).unwrap();
        let req = read_line::<Request>(&mut &to_instance[..]).unwrap().unwrap();
        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>();
        struct Chan(Sender<Vec<u8>>);
        impl Write for Chan {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                let _ = self.0.send(b.to_vec());
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let held = Mutex::new(None);
        answer(
            req,
            |_, done| {
                *held.lock().unwrap() = done;
                true
            },
            Chan(out_tx),
        );
        assert_eq!(out_rx.recv_timeout(Duration::from_secs(5)).unwrap(), b"\"ok\"\n");
        held.lock().unwrap().take().unwrap().send(()).unwrap();
        assert_eq!(out_rx.recv_timeout(Duration::from_secs(5)).unwrap(), b"\"closed\"\n");
        // an instance that dies early: the client sees an error, not a hang
        let mut empty = &b""[..];
        assert!(request(std::io::sink(), &mut empty, &sample(false)).is_err());
        let mut err = &b"{\"error\":\"cannot open a window\"}\n"[..];
        assert_eq!(request(std::io::sink(), &mut err, &sample(false)), Err("cannot open a window".into()));
    }

    #[cfg(unix)]
    #[test]
    fn stale_socket_is_replaced() {
        let dir = temp("stale");
        let addr = dir.join("instance.sock");
        // a socket file whose listener is gone, as after a crash
        drop(Listener::bind(&addr).unwrap());
        assert!(addr.exists());
        assert!(connect(&addr).is_none());
        let Bound::Server(server) = bind(&addr) else { panic!("the stale socket should be replaced") };
        let guard = server.serve(Arc::new(|_, _| true));
        assert!(connect(&addr).is_some());
        drop(guard);
        assert!(!addr.exists(), "the instance removes its socket when it exits");
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn private_dir_is_checked() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("private");
        let d = dir.join("rt");
        private_dir(&d).unwrap();
        assert_eq!(fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_dir(&d).is_err(), "readable by others: refused");
        fs::remove_dir_all(&dir).ok();
    }
}
