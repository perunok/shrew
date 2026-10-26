//! The app-facing state machine: every network and disk step runs on a short-lived background thread, the UI only
//! reads [`Updater::phase`] and [`Updater::take_checked`] when the wake callback tells it something changed.
//!
//! Checks and downloads are independent. A check ([`Updater::check`]) answers once through
//! [`Updater::take_checked`]; *when* to check and what to tell the user is the app's business. A download walks
//! `Idle → Downloading → Verifying → Ready` (or `Failed`); [`Updater::cancel`] goes back to `Idle` and deletes the
//! partial file. `Ready` holds a verified [`Staged`] file; nothing replaces the running binary until
//! [`Updater::install`], which the app calls right before restarting.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{Checked, Error, Feed, Installed, Release, Staged, Step, Unsupported, install};

/// Where a download is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// `total` is 0 until known; `speed` is bytes per second over the last second or so (0 until measured).
    Downloading {
        version: String,
        done: u64,
        total: u64,
        speed: u64,
    },
    /// Hashing the downloaded file against the published sha256.
    Verifying {
        version: String,
    },
    /// Downloaded and verified; [`Updater::install`] puts it in place.
    Ready(Staged),
    Failed(String),
}

/// What to check and where to install; [`Options::new`] has the production defaults.
#[derive(Clone, Debug)]
pub struct Options {
    /// The running version, `env!("CARGO_PKG_VERSION")`.
    pub current: String,
    pub feed: Feed,
    /// File to replace; `None` resolves [`crate::install_target`] (the running exe or AppImage).
    pub target: Option<PathBuf>,
    /// Where downloads are kept until installed; `None`: next to the target.
    pub downloads: Option<PathBuf>,
}

impl Options {
    pub fn new(current: &str) -> Self {
        Options { current: current.into(), feed: Feed::from_env(), target: None, downloads: crate::downloads_dir() }
    }
}

/// Progress wakes come at most this often (plus the first and the last).
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// Cheap to clone; all clones drive the same state.
#[derive(Clone)]
pub struct Updater {
    shared: Arc<Shared>,
}

struct Shared {
    opts: Options,
    phase: Mutex<Phase>,
    /// The answer of the last check, until taken.
    checked: Mutex<Option<Result<Checked, String>>>,
    checking: AtomicBool,
    downloading: AtomicBool,
    cancel: AtomicBool,
    wake: Mutex<Box<dyn Fn() + Send>>,
}

impl Updater {
    /// `wake` is called from the background thread whenever something changed (e.g. post a winit user event); keep
    /// it cheap.
    pub fn new(current: &str, wake: impl Fn() + Send + 'static) -> Self {
        Self::with_options(Options::new(current), wake)
    }

    pub fn with_options(opts: Options, wake: impl Fn() + Send + 'static) -> Self {
        Updater {
            shared: Arc::new(Shared {
                opts,
                phase: Mutex::new(Phase::Idle),
                checked: Mutex::new(None),
                checking: AtomicBool::new(false),
                downloading: AtomicBool::new(false),
                cancel: AtomicBool::new(false),
                wake: Mutex::new(Box::new(wake)),
            }),
        }
    }

    pub fn current(&self) -> &str {
        &self.shared.opts.current
    }

    pub fn phase(&self) -> Phase {
        lock(&self.shared.phase).clone()
    }

    /// A check is running.
    pub fn checking(&self) -> bool {
        self.shared.checking.load(Ordering::Acquire)
    }

    /// Asks the feed for the latest release in the background; the answer comes through [`Updater::take_checked`].
    /// False when a check is running already.
    pub fn check(&self) -> bool {
        if self.shared.checking.swap(true, Ordering::AcqRel) {
            return false;
        }
        let ok = self.spawn(|shared| {
            let result = shared.opts.feed.fetch(&shared.opts.current).map_err(|err| err.to_string());
            match &result {
                Ok(c) => log::info!("update: latest {:?}, update {:?}", c.latest, c.update.as_ref().map(|r| &r.tag)),
                Err(err) => log::info!("update: check failed: {err}"),
            }
            *lock(&shared.checked) = Some(result);
            shared.checking.store(false, Ordering::Release);
            shared.wake();
        });
        if !ok {
            self.shared.checking.store(false, Ordering::Release);
        }
        ok
    }

    /// The answer of the check that finished last, once.
    pub fn take_checked(&self) -> Option<Result<Checked, String>> {
        lock(&self.shared.checked).take()
    }

    /// Why this installation cannot update itself, if it cannot ([`crate::support`], or the configured target).
    pub fn support(&self) -> Result<PathBuf, Unsupported> {
        match &self.shared.opts.target {
            Some(target) => install::support_for(target).map(|()| target.clone()),
            None => install::support(),
        }
    }

    /// Downloads and verifies `release` in the background. False when a download is running or this release is
    /// already `Ready`.
    pub fn download(&self, release: &Release) -> bool {
        if matches!(&*lock(&self.shared.phase), Phase::Ready(s) if s.release.version == release.version) {
            return false;
        }
        if self.shared.downloading.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.shared.cancel.store(false, Ordering::Release);
        self.shared.set(Phase::Downloading {
            version: release.version.clone(),
            done: 0,
            total: release.size,
            speed: 0,
        });
        let release = release.clone();
        let ok = self.spawn(move |shared| {
            shared.run_download(&release);
            shared.downloading.store(false, Ordering::Release);
            shared.wake();
        });
        if !ok {
            self.shared.downloading.store(false, Ordering::Release);
        }
        ok
    }

    /// Stops a download (its partial file is deleted; back to `Idle`). No effect otherwise.
    pub fn cancel(&self) {
        if matches!(*lock(&self.shared.phase), Phase::Downloading { .. } | Phase::Verifying { .. }) {
            self.shared.cancel.store(true, Ordering::Release);
        }
    }

    /// Clears a `Failed` download (back to `Idle`).
    pub fn reset(&self) {
        let mut phase = lock(&self.shared.phase);
        if matches!(*phase, Phase::Failed(_)) {
            *phase = Phase::Idle;
        }
    }

    /// Puts the `Ready` download in place of the install target (blocking, but quick: a local copy and a rename).
    /// The running process is untouched; restart it with [`crate::restart`]. On failure the phase says why.
    pub fn install(&self) -> crate::Result<Installed> {
        let Phase::Ready(staged) = self.phase() else {
            return Err(Error::Feed("no verified download to install".into()));
        };
        let target = match &self.shared.opts.target {
            Some(t) => t.clone(),
            None => install::install_target()?,
        };
        install::apply(&staged, &target).inspect_err(|err| {
            log::warn!("update: install failed: {err}");
            self.shared.set(Phase::Failed(err.to_string()));
        })
    }

    fn spawn(&self, job: impl FnOnce(&Shared) + Send + 'static) -> bool {
        let shared = self.shared.clone();
        match std::thread::Builder::new().name("shrew-update".into()).spawn(move || job(&shared)) {
            Ok(_) => true,
            Err(err) => {
                self.shared.set(Phase::Failed(format!("cannot start the update thread: {err}")));
                false
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Shared {
    fn wake(&self) {
        (lock(&self.wake))();
    }

    fn set(&self, phase: Phase) {
        *lock(&self.phase) = phase;
        self.wake();
    }

    fn run_download(&self, release: &Release) {
        let dir = match (&self.opts.downloads, &self.opts.target) {
            (Some(d), _) => Ok(d.clone()),
            (None, Some(t)) => Ok(t.parent().map(PathBuf::from).unwrap_or_default()),
            (None, None) => install::install_target().map(|t| t.parent().map(PathBuf::from).unwrap_or_default()),
        };
        let version = release.version.clone();
        // measured from the first byte on, not from the connection's setup
        let mut meter: Option<Meter> = None;
        let mut last_wake: Option<Instant> = None;
        let result = dir.and_then(|dir| {
            install::download(release, &dir, &self.cancel, |step| {
                let now = Instant::now();
                let phase = match step {
                    Step::Downloading { done, total } => {
                        let speed = meter.get_or_insert_with(|| Meter::new(now)).add(now, done);
                        if last_wake.is_some_and(|t| now - t < PROGRESS_EVERY) && done < total {
                            return;
                        }
                        Phase::Downloading { version: version.clone(), done, total, speed }
                    }
                    Step::Verifying => Phase::Verifying { version: version.clone() },
                };
                last_wake = Some(now);
                self.set(phase);
            })
        });
        match result {
            Ok(staged) => self.set(Phase::Ready(staged)),
            Err(Error::Cancelled) => {
                log::info!("update: download of {} cancelled", release.tag);
                self.set(Phase::Idle);
            }
            Err(err) => {
                log::warn!("update: download failed: {err}");
                self.set(Phase::Failed(err.to_string()));
            }
        }
    }
}

/// Download speed over roughly the last second.
pub(crate) struct Meter {
    start: Instant,
    start_bytes: u64,
    speed: u64,
}

impl Meter {
    pub(crate) fn new(now: Instant) -> Self {
        Meter { start: now, start_bytes: 0, speed: 0 }
    }

    /// `done` bytes at `now`; the speed in bytes per second (0 until a first window is complete).
    pub(crate) fn add(&mut self, now: Instant, done: u64) -> u64 {
        let elapsed = now.saturating_duration_since(self.start);
        if elapsed >= Duration::from_millis(750) {
            let bytes = done.saturating_sub(self.start_bytes);
            let speed = (bytes as f64 / elapsed.as_secs_f64()) as u64;
            // smooth it a little: one slow chunk should not make the time left jump
            self.speed = if self.speed == 0 { speed } else { (self.speed + 2 * speed) / 3 };
            self.start = now;
            self.start_bytes = done;
        }
        self.speed
    }
}
