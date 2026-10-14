//! The app-facing state machine: every network and disk step runs on a short-lived background thread, the UI only
//! polls [`Updater::status`] when the wake callback tells it something changed.
//!
//! `Idle → Checking → Available → Downloading(%) → Ready → (restart)`, with `UpToDate` / `Failed` as dead ends
//! the user can dismiss or retry. Automatic checks are silent: no `Checking` flash, failures only logged, and skipped
//! entirely when rate limited or when shrew is package-managed (the user could not update in-app anyway).

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{Error, Feed, Installed, Release, install, ratelimit};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    /// A check the user asked for is running (automatic ones stay `Idle`).
    Checking,
    /// A forced check found nothing newer.
    UpToDate,
    Available(Release),
    /// Download progress, 0-100.
    Downloading(u8),
    /// Verified and swapped in; takes effect on restart.
    Ready(Installed),
    Failed(String),
}

impl Status {
    /// Text for the status-bar button; `None` hides it.
    pub fn label(&self) -> Option<String> {
        Some(match self {
            Status::Idle => return None,
            Status::Checking => "Checking for updates…".into(),
            Status::UpToDate => "shrew is up to date".into(),
            Status::Available(release) => format!("Update to {}", release.tag),
            Status::Downloading(pct) => format!("Updating… {pct}%"),
            Status::Ready(installed) => format!("Restart for {}", installed.version),
            Status::Failed(_) => "Update failed".into(),
        })
    }
}

/// What to check and where to install; [`Options::new`] has the production defaults.
#[derive(Clone, Debug)]
pub struct Options {
    /// The running version, `env!("CARGO_PKG_VERSION")`.
    pub current: String,
    pub feed: Feed,
    /// File to replace; `None` resolves [`crate::install_target`] (the running exe or AppImage).
    pub target: Option<PathBuf>,
    /// Rate-limit timestamp file; `None` makes every automatic check run.
    pub stamp: Option<PathBuf>,
}

impl Options {
    pub fn new(current: &str) -> Self {
        Options { current: current.into(), feed: Feed::from_env(), target: None, stamp: ratelimit::stamp_path() }
    }
}

/// Cheap to clone; all clones drive the same state.
#[derive(Clone)]
pub struct Updater {
    shared: Arc<Shared>,
}

struct Shared {
    opts: Options,
    status: Mutex<Status>,
    /// A background job is running (also covers silent automatic checks, which don't show `Checking`).
    busy: AtomicBool,
    wake: Mutex<Box<dyn Fn() + Send>>,
}

impl Updater {
    /// `wake` is called from the background thread whenever [`Updater::status`] changes (e.g. post a winit user
    /// event); keep it cheap.
    pub fn new(current: &str, wake: impl Fn() + Send + 'static) -> Self {
        Self::with_options(Options::new(current), wake)
    }

    pub fn with_options(opts: Options, wake: impl Fn() + Send + 'static) -> Self {
        Updater {
            shared: Arc::new(Shared {
                opts,
                status: Mutex::new(Status::Idle),
                busy: AtomicBool::new(false),
                wake: Mutex::new(Box::new(wake)),
            }),
        }
    }

    pub fn status(&self) -> Status {
        self.shared.status().clone()
    }

    /// Starts a check in the background. Automatic (`force == false`) checks run at most once per
    /// [`crate::CHECK_INTERVAL`] and are silent unless they find an update; forced ones (the user asked) always run
    /// and report `Checking`, `UpToDate` or `Failed`. Returns whether a check was started.
    pub fn check(&self, force: bool) -> bool {
        if matches!(*self.shared.status(), Status::Downloading(_) | Status::Ready(_)) || !self.shared.begin() {
            return false;
        }
        if force {
            self.shared.set(Status::Checking);
        }
        self.spawn(move |shared| shared.run_check(force))
    }

    /// Downloads and installs the available release in the background. Returns whether it started.
    pub fn install(&self) -> bool {
        let Status::Available(release) = self.status() else { return false };
        if !self.shared.begin() {
            return false;
        }
        self.shared.set(Status::Downloading(0));
        self.spawn(move |shared| shared.run_install(&release))
    }

    /// The status-bar button was clicked: install, restart, retry or dismiss depending on the state.
    pub fn activate(&self) {
        match self.status() {
            Status::Available(_) => {
                self.install();
            }
            Status::Ready(_) => {
                if let Some(err) = self.restart() {
                    self.shared.set(Status::Failed(format!("restart failed: {err}")));
                }
            }
            Status::Failed(_) => {
                self.check(true);
            }
            Status::UpToDate => self.dismiss(),
            Status::Idle | Status::Checking | Status::Downloading(_) => {}
        }
    }

    /// Hides an `UpToDate`, `Failed` or `Available` button (back to `Idle`).
    pub fn dismiss(&self) {
        let mut status = self.shared.status();
        if matches!(*status, Status::UpToDate | Status::Failed(_) | Status::Available(_)) {
            *status = Status::Idle;
        }
    }

    /// When `Ready`, replaces the process with the new binary (see [`crate::restart`]); save state first.
    /// Returns `None` when there is nothing to restart into, `Some(err)` when re-executing failed.
    pub fn restart(&self) -> Option<Error> {
        let Status::Ready(installed) = self.status() else { return None };
        Some(crate::restart(&installed))
    }

    fn spawn(&self, job: impl FnOnce(&Shared) + Send + 'static) -> bool {
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new().name("shrew-update".into()).spawn(move || {
            job(&shared);
            shared.busy.store(false, Ordering::Release);
        });
        if let Err(err) = spawned {
            self.shared.busy.store(false, Ordering::Release);
            self.shared.set(Status::Failed(format!("cannot start update thread: {err}")));
            return false;
        }
        true
    }
}

impl Shared {
    fn status(&self) -> MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Claims the single background slot.
    fn begin(&self) -> bool {
        !self.busy.swap(true, Ordering::AcqRel)
    }

    fn set(&self, status: Status) {
        *self.status() = status;
        (self.wake.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))();
    }

    fn run_check(&self, force: bool) {
        if !force {
            let now = std::time::SystemTime::now();
            if self.opts.stamp.as_deref().is_some_and(|stamp| !ratelimit::due(stamp, now)) {
                return;
            }
            if self.opts.target.is_none()
                && let Err(err) = install::install_target()
            {
                log::info!("update: automatic checks off: {err}");
                return;
            }
        }
        match self.opts.feed.check(&self.opts.current) {
            Ok(Some(release)) => {
                // Not stamped: until it is installed, every start offers it again.
                log::info!("update: {} available", release.tag);
                self.set(Status::Available(release));
            }
            Ok(None) => {
                if let Some(stamp) = &self.opts.stamp {
                    ratelimit::mark(stamp, std::time::SystemTime::now());
                }
                if force {
                    self.set(Status::UpToDate);
                }
            }
            Err(err) if force => self.set(Status::Failed(err.to_string())),
            Err(err) => log::info!("update: check failed: {err}"),
        }
    }

    fn run_install(&self, release: &Release) {
        let target = match &self.opts.target {
            Some(target) => Ok(target.clone()),
            None => install::install_target(),
        };
        let mut last = 0;
        let result = target.and_then(|target| {
            install::install_to(release, &target, |done, total| {
                let pct = if total == 0 { 0 } else { (done.min(total) * 100 / total) as u8 };
                // One wake per percent, not per 64 KiB chunk.
                if pct != last {
                    last = pct;
                    self.set(Status::Downloading(pct));
                }
            })
        });
        match result {
            Ok(installed) => self.set(Status::Ready(installed)),
            Err(err) => {
                log::warn!("update: install failed: {err}");
                self.set(Status::Failed(err.to_string()));
            }
        }
    }
}
