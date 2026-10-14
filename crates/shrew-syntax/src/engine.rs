//! The shared side: one background thread that loads the grammars, then highlights every open buffer in short
//! time slices, visible lines first. The UI thread only ever takes a mutex for a few microseconds to hand over a
//! rope snapshot or pick up finished lines.

use crate::detect::LangHint;
use crate::highlighter::Highlighter;
use crate::theme::Theme;
use crate::work::{Input, Inval, Pass, Update, Work};
use ropey::Rope;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};
use syntect::parsing::SyntaxSet;

/// Called from the worker thread when new highlighting for visible lines is ready (e.g. post a winit user event
/// that triggers [`Highlighter::poll`] and a redraw). Coalesced: at most once until the UI polls.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Where grammars come from besides syntect's built-in set.
#[derive(Clone, Debug)]
pub struct Options {
    /// Also load `.sublime-syntax` files from Sublime Text's `Packages/User` folders (feature `sublime-user`).
    pub sublime_user: bool,
    /// More folders searched (recursively) for `.sublime-syntax` files (feature `sublime-user`).
    pub extra_syntax_dirs: Vec<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Options { sublime_user: true, extra_syntax_dirs: Vec::new() }
    }
}

/// Locks, shrugging off poisoning: the data stays consistent because no critical section can panic halfway.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Shared between one [`Highlighter`] (UI thread) and the worker.
#[derive(Default)]
pub(crate) struct Inbox {
    // UI -> worker
    pub text: Option<(Rope, u64)>,
    pub inval: Option<Inval>,
    pub hint: Option<LangHint>,
    pub viewport: (usize, usize),
    // worker -> UI, always in the coordinates of the UI's current version
    pub out: Vec<Update>,
    pub woken: bool,
    /// The version the worker has nothing left to do for.
    pub settled: Option<u64>,
}

pub(crate) struct Job {
    pub inbox: Mutex<Inbox>,
    /// Only the worker thread touches this.
    work: Mutex<Option<Work>>,
    pub closed: AtomicBool,
}

impl Job {
    pub fn new(inbox: Inbox) -> Self {
        Job { inbox: Mutex::new(inbox), work: Mutex::new(None), closed: AtomicBool::new(false) }
    }

    fn step(&self, pass: Pass, (ss, set_gen): (&SyntaxSet, u64), theme: &(Arc<Theme>, u64), wake: &Wake) -> bool {
        let mut guard = lock(&self.work);
        let work = guard.get_or_insert_with(Work::default);
        let input = {
            let mut inbox = lock(&self.inbox);
            Input {
                text: inbox.text.take(),
                inval: inbox.inval.take(),
                hint: inbox.hint.take(),
                viewport: inbox.viewport,
                theme: (&theme.0, theme.1),
                set_gen,
            }
        };
        // grammar resolution (may compile first-line regexes) happens outside the lock
        work.ingest(input, ss);
        let did = work.step(pass, ss);
        // publish atomically with whatever edits arrived meanwhile, so `out` matches the UI's version
        let mut inbox = lock(&self.inbox);
        let input = Input {
            text: inbox.text.take(),
            inval: inbox.inval.take(),
            hint: None,
            viewport: inbox.viewport,
            theme: (&theme.0, theme.1),
            set_gen,
        };
        work.ingest(input, ss);
        let notify = !work.out.is_empty() && work.out_visible() && !inbox.woken;
        inbox.out.append(&mut work.out);
        inbox.settled = (inbox.hint.is_none() && work.settled()).then_some(work.version);
        inbox.woken |= notify;
        drop(inbox);
        if notify {
            wake();
        }
        did
    }
}

pub(crate) struct Engine {
    /// The grammars and a generation that changes when user grammars replace the built-in set.
    set: RwLock<Option<(Arc<SyntaxSet>, u64)>>,
    theme: RwLock<(Arc<Theme>, u64)>,
    jobs: Mutex<Vec<Arc<Job>>>,
    signal: Mutex<bool>,
    cv: Condvar,
    wake: Wake,
    shutdown: AtomicBool,
    /// No more grammar sets are coming.
    complete: AtomicBool,
}

impl Engine {
    pub fn signal(&self) {
        *lock(&self.signal) = true;
        self.cv.notify_one();
    }

    pub fn add(&self, job: Arc<Job>) {
        lock(&self.jobs).push(job);
        self.signal();
    }

    pub fn theme(&self) -> Arc<Theme> {
        self.theme.read().unwrap_or_else(|e| e.into_inner()).0.clone()
    }

    fn grammars(&self) -> Option<(Arc<SyntaxSet>, u64)> {
        self.set.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn run(self: Arc<Self>, opts: Options) {
        // shared with the symbol finder (`symbols::default_set`)
        *self.set.write().unwrap_or_else(|e| e.into_inner()) = Some((crate::symbols::default_set(), 1));
        (self.wake)();
        // user grammars need the whole set rebuilt (~0.3 s): do that aside and swap it in when done
        let dirs = user_dirs(&opts);
        if !dirs.is_empty() {
            let engine = self.clone();
            let _ = std::thread::Builder::new().name("shrew-syntax-load".into()).spawn(move || {
                let merged = with_user_grammars(&dirs);
                *engine.set.write().unwrap_or_else(|e| e.into_inner()) = Some((Arc::new(merged), 2));
                engine.complete.store(true, Ordering::Release);
                engine.signal();
            });
        } else {
            self.complete.store(true, Ordering::Release);
        }
        let mut rr = 0;
        while !self.shutdown.load(Ordering::Acquire) {
            let (ss, set_gen) = self.grammars().expect("loaded above");
            let jobs: Vec<Arc<Job>> = {
                let mut jobs = lock(&self.jobs);
                jobs.retain(|j| !j.closed.load(Ordering::Acquire));
                jobs.clone()
            };
            let theme = self.theme.read().unwrap_or_else(|e| e.into_inner()).clone();
            let mut did = false;
            'pass: for pass in [Pass::Visible, Pass::Background] {
                for k in 0..jobs.len() {
                    let i = (rr + k) % jobs.len();
                    if jobs[i].step(pass, (&ss, set_gen), &theme, &self.wake) {
                        did = true;
                        rr = i + 1;
                        break 'pass;
                    }
                }
            }
            if !did {
                let mut signaled = lock(&self.signal);
                while !*signaled && !self.shutdown.load(Ordering::Acquire) {
                    signaled = self.cv.wait(signaled).unwrap_or_else(|e| e.into_inner());
                }
                *signaled = false;
            }
        }
    }
}

/// The highlighting service: grammars, the current theme and the worker thread. Cheap to clone; the worker stops
/// when the last clone is dropped.
#[derive(Clone)]
pub struct Syntaxes {
    handle: Arc<Handle>,
}

struct Handle {
    engine: Arc<Engine>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.engine.shutdown.store(true, Ordering::Release);
        self.engine.signal();
    }
}

impl Syntaxes {
    /// Starts the worker, which loads the grammars right away (a few ms, never on the caller's thread).
    pub fn new(theme: Theme, wake: Wake) -> Syntaxes {
        Syntaxes::with_options(theme, wake, Options::default())
    }

    pub fn with_options(theme: Theme, wake: Wake, opts: Options) -> Syntaxes {
        let engine = Arc::new(Engine {
            set: RwLock::new(None),
            theme: RwLock::new((Arc::new(theme), 0)),
            jobs: Mutex::new(Vec::new()),
            signal: Mutex::new(false),
            cv: Condvar::new(),
            wake,
            shutdown: AtomicBool::new(false),
            complete: AtomicBool::new(false),
        });
        let worker = engine.clone();
        std::thread::Builder::new()
            .name("shrew-syntax".into())
            .spawn(move || worker.run(opts))
            .expect("spawn syntax thread");
        Syntaxes { handle: Arc::new(Handle { engine }) }
    }

    pub(crate) fn engine(&self) -> &Arc<Engine> {
        &self.handle.engine
    }

    /// The built-in grammars are loaded (user grammars may still be on their way and re-highlight when they land).
    /// Until then highlighters report every line as not ready.
    pub fn is_ready(&self) -> bool {
        self.handle.engine.grammars().is_some()
    }

    /// Blocks until all grammars, user ones included, are loaded (tests and headless tools; the UI never needs
    /// this).
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while !self.handle.engine.complete.load(Ordering::Acquire) {
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        true
    }

    pub fn theme(&self) -> Arc<Theme> {
        self.handle.engine.theme()
    }

    /// Switches every buffer to `theme`. Lines keep their old colors until re-highlighted (no flash).
    pub fn set_theme(&self, theme: Theme) {
        {
            let mut t = self.handle.engine.theme.write().unwrap_or_else(|e| e.into_inner());
            let generation = t.1 + 1;
            *t = (Arc::new(theme), generation);
        }
        self.handle.engine.signal();
    }

    /// A highlighter for a document; the grammar is picked from its path and first line.
    pub fn highlighter(&self, doc: &shrew_core::Document) -> Highlighter {
        Highlighter::new(self, &doc.rope, doc.version, doc.path.as_deref())
    }

    /// Names of the selectable grammars (for a "set syntax" picker); empty until loaded.
    pub fn syntax_names(&self) -> Vec<String> {
        let Some((ss, _)) = self.handle.engine.grammars() else { return Vec::new() };
        let mut names: Vec<String> = ss.syntaxes().iter().filter(|s| !s.hidden).map(|s| s.name.clone()).collect();
        names.sort_unstable_by_key(|n| n.to_lowercase());
        names
    }
}

/// Sublime Text's `Packages/User` folders on this OS (existing or not).
pub fn sublime_user_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let mut bases: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        bases.extend(home.map(|h| h.join("Library/Application Support")));
    } else if cfg!(windows) {
        bases.extend(std::env::var_os("APPDATA").map(PathBuf::from));
    } else {
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute());
        bases.extend(xdg.or_else(|| home.map(|h| h.join(".config"))));
    }
    let names: &[&str] = if cfg!(any(target_os = "macos", windows)) {
        &["Sublime Text", "Sublime Text 3"]
    } else {
        &["sublime-text", "sublime-text-3"]
    };
    bases.iter().flat_map(|b| names.iter().map(move |n| b.join(n).join("Packages").join("User"))).collect()
}

fn has_syntax_files(dir: &Path, depth: usize) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    entries.flatten().any(|e| {
        let p = e.path();
        if p.is_dir() {
            depth > 0 && has_syntax_files(&p, depth - 1)
        } else {
            p.extension().is_some_and(|x| x == "sublime-syntax")
        }
    })
}

/// Folders with `.sublime-syntax` files to load (none without the `sublime-user` feature).
fn user_dirs(opts: &Options) -> Vec<PathBuf> {
    if !cfg!(feature = "sublime-user") {
        return Vec::new();
    }
    let mut dirs = opts.extra_syntax_dirs.clone();
    if opts.sublime_user {
        dirs.extend(sublime_user_dirs());
    }
    dirs.retain(|d| has_syntax_files(d, 4));
    dirs
}

#[cfg(feature = "sublime-user")]
fn with_user_grammars(dirs: &[PathBuf]) -> SyntaxSet {
    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    for d in dirs {
        if let Err(e) = builder.add_from_folder(d, true) {
            log::warn!("syntax: {}: {e}", d.display());
        }
    }
    builder.build()
}

#[cfg(not(feature = "sublime-user"))]
fn with_user_grammars(_: &[PathBuf]) -> SyntaxSet {
    unreachable!("user_dirs is empty without the feature")
}
