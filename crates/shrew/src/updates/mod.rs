//! In-app updates (cargo feature `update`), modelled on the user's Slinger app:
//!
//! - [`policy`]: when to ask GitHub (every start, once a day, on request) and when to say so.
//! - a notification (the app's [`toast`](crate::toast)) at the bottom right: "shrew 0.3.0 is available" with
//!   **Update now** and **Release notes**; Esc or ✕ dismisses it, Ctrl+Shift+U opens the dialog.
//! - [`dialog`] + [`draw`]: the modal **Update shrew** dialog: versions, the release notes ([`notes`]), the download
//!   with its progress bar, speed and time left, Cancel and Hide, the sha256 check, then Restart to update or Later.
//! - the status-bar button (the percentage while a hidden download runs), and the rows of Settings > Updates.
//! - [`session`]: restarting into the new version with every buffer back, unsaved changes included.
//! - [`cli`]: `srw --self-update`, the same steps without a window (for scripts and the end-to-end test).
//!
//! The network and disk work is `shrew-update`'s ([`shrew_update::Updater`], UI-free). Nothing here polls: the
//! updater wakes the event loop when something changed, and the two timers (the startup check, the daily one) are
//! event-loop deadlines ([`deadline`] / [`tick`]).

pub mod cli;
mod dialog;
mod draw;
mod notes;
mod policy;
pub mod session;

pub use draw::draw;

use crate::state::{State, Tone};
use dialog::{Act, Button, Dialog, Stage};
use policy::{Prefs, Source};
use shrew_core::features::Feature;
use shrew_core::{Key, KeyPress};
use shrew_update::{Phase, Release, Updater};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Informational toasts go away by themselves after this.
const NOTE_FOR: Duration = Duration::from_secs(10);

/// The version this build reports to the updater: `SHREW_PRETEND_VERSION` in debug builds (only there: release
/// builds ignore it), so a test can run a build as an older shrew; otherwise the crate version.
pub fn current_version() -> String {
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var("SHREW_PRETEND_VERSION")
        && !v.trim().is_empty()
    {
        return v.trim().trim_start_matches('v').to_string();
    }
    env!("CARGO_PKG_VERSION").to_string()
}

/// The notification at the bottom right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Toast {
    /// A newer version: Update now (or View release), Release notes.
    Available(Release),
    /// Anything else: up to date, a failed check, "Updated to shrew X" after a restart, an error.
    Note {
        title: String,
        body: String,
        error: bool,
        /// A button that opens a link: ("What's new", url).
        link: Option<(&'static str, String)>,
        /// A Retry button (a failed check).
        retry: bool,
    },
}

/// The app's update state (in [`State::updates`]).
pub struct Updates {
    updater: Option<Updater>,
    current: String,
    headless: bool,
    prefs: Prefs,
    prefs_path: Option<PathBuf>,
    /// The first frame is up and the schedule runs.
    started: bool,
    /// When the startup check runs.
    start_at: Option<Instant>,
    /// When a failed automatic check tries again.
    retry_at: Option<Instant>,
    /// Who asked for the check that is running.
    source: Option<Source>,
    /// Open the dialog and download when the running check finds an update (`:update` with nothing known yet).
    install_after_check: bool,
    /// The release to update to, from the last check.
    pub available: Option<Release>,
    /// The latest version the last check saw (also when it is not newer).
    pub latest: Option<String>,
    /// Why the last check failed.
    pub error: Option<String>,
    pub toast: Option<Toast>,
    toast_until: Option<Instant>,
    pub dialog: Option<Dialog>,
    pub hits: draw::Hits,
    /// The notification's part under the pointer, and the dialog's button (drawn lit).
    pub hot: crate::toast::Hot,
    pub dialog_hot: Option<Button>,
    /// Why this installation cannot update itself (looked up once).
    support: std::cell::OnceCell<Result<(), String>>,
    /// The kind of phase Settings last built its rows for.
    shown_phase: u8,
    /// Screenshots and tests: a phase (and support) to show instead of the updater's.
    demo: Option<(Phase, Result<(), String>)>,
}

impl Updates {
    pub fn new(headless: bool) -> Updates {
        Updates {
            updater: None,
            current: current_version(),
            headless,
            prefs: Prefs::default(),
            prefs_path: None,
            started: false,
            start_at: None,
            retry_at: None,
            source: None,
            install_after_check: false,
            available: None,
            latest: None,
            error: None,
            toast: None,
            toast_until: None,
            dialog: None,
            hits: Default::default(),
            hot: Default::default(),
            dialog_hot: None,
            support: Default::default(),
            shown_phase: 0,
            demo: None,
        }
    }

    pub fn current(&self) -> &str {
        &self.current
    }

    /// Tests: an updater with its own feed, install target and download folder, and a prefs file of their own.
    #[cfg(test)]
    pub fn set_updater(&mut self, u: Updater, prefs: PathBuf) {
        self.current = u.current().to_string();
        self.updater = Some(u);
        self.prefs_path = Some(prefs);
    }

    pub fn phase(&self) -> Phase {
        if let Some((p, _)) = &self.demo {
            return p.clone();
        }
        self.updater.as_ref().map_or(Phase::Idle, Updater::phase)
    }

    pub fn checking(&self) -> bool {
        self.updater.as_ref().is_some_and(Updater::checking)
    }

    /// Whether this installation can replace itself, and if not why (a clause for "cannot update itself: …").
    pub fn support(&self) -> Result<(), String> {
        if let Some((_, s)) = &self.demo {
            return s.clone();
        }
        self.support
            .get_or_init(|| match &self.updater {
                Some(u) => u.support().map(|_| ()).map_err(|e| e.to_string()),
                None => shrew_update::support().map(|_| ()).map_err(|e| e.to_string()),
            })
            .clone()
    }

    fn stage(&self) -> Stage {
        let phase = self.phase();
        let support = self.support();
        Stage::of(&phase, self.available.is_some(), support.as_ref().map(|_| ()).map_err(String::as_str))
    }

    /// The release page for "View release": the update's, else the latest one's, else the list.
    fn page(&self) -> String {
        if let Some(r) = &self.available {
            return r.page.clone();
        }
        if let Phase::Ready(s) = self.phase() {
            return s.release.page;
        }
        match &self.latest {
            Some(v) => shrew_update::release_page(shrew_update::REPO, &format!("v{v}")),
            None => format!("https://github.com/{}/releases", shrew_update::REPO),
        }
    }

    fn show(&mut self, toast: Toast) {
        self.toast_until = match &toast {
            Toast::Note { error: false, retry: false, .. } => Some(Instant::now() + NOTE_FOR),
            _ => None,
        };
        self.toast = Some(toast);
    }

    fn note(&mut self, title: impl Into<String>, body: impl Into<String>, error: bool) {
        self.show(Toast::Note { title: title.into(), body: body.into(), error, link: None, retry: false });
    }

    fn load_prefs(&mut self) {
        if self.prefs_path.is_none() {
            self.prefs_path = Prefs::path();
            if let Some(p) = &self.prefs_path {
                self.prefs = Prefs::load(p);
            }
        }
    }

    fn save_prefs(&self) {
        if let Some(p) = &self.prefs_path {
            self.prefs.save(p);
        }
    }
}

/// The updater, made on first use (with the cleanup of earlier updates).
fn updater(st: &mut State) -> Updater {
    if let Some(u) = &st.updates.updater {
        return u.clone();
    }
    let wake = st.wake.clone();
    let opts = shrew_update::Options::new(&st.updates.current);
    shrew_update::cleanup(&st.updates.current, opts.downloads.as_deref());
    let u = Updater::with_options(opts, move || wake());
    st.updates.updater = Some(u.clone());
    u
}

// ----- the schedule -----

/// The first frame is on screen: the startup check runs in a few seconds, the daily one after it (not headless).
pub fn start(st: &mut State) {
    let u = &mut st.updates;
    if u.headless || u.started {
        return;
    }
    u.started = true;
    u.load_prefs();
    if st.ed.features.on(Feature::UpdateCheck) {
        u.start_at = Some(Instant::now() + policy::STARTUP_DELAY);
    }
}

/// The next time [`tick`] has something to do, for the event loop's `WaitUntil` (none: sleep until woken).
pub fn deadline(st: &State) -> Option<Instant> {
    let u = &st.updates;
    let mut next: Option<Instant> = None;
    let mut at = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));
    if let Some(t) = u.toast_until {
        at(t);
    }
    if let Some(t) = u.start_at {
        at(t);
    }
    // the daily check, once the startup one has run
    if u.started && u.start_at.is_none() && st.ed.features.on(Feature::UpdateCheck) && !u.checking() {
        match u.retry_at {
            Some(t) => at(t),
            None => at(Instant::now() + Duration::from_secs(u.prefs.daily_in(policy::now()))),
        }
    }
    next
}

/// Runs what is due: the startup check, the daily check, hiding a note. True when the screen changed.
pub fn tick(st: &mut State, now: Instant) -> bool {
    let mut changed = false;
    let u = &mut st.updates;
    if u.toast_until.is_some_and(|t| now >= t) {
        u.toast = None;
        u.toast_until = None;
        changed = true;
    }
    if u.start_at.is_some_and(|t| now >= t) {
        u.start_at = None;
        if st.ed.features.on(Feature::UpdateCheck) {
            check(st, Source::Startup);
        }
        return true;
    }
    let u = &st.updates;
    let due = u.started
        && u.start_at.is_none()
        && st.ed.features.on(Feature::UpdateCheck)
        && !u.checking()
        && match u.retry_at {
            Some(t) => now >= t,
            None => u.prefs.daily_in(policy::now()) == 0,
        };
    if due {
        st.updates.retry_at = None;
        if !check(st, Source::Daily) {
            // never spin on a check that cannot start
            st.updates.retry_at = Some(now + policy::RETRY_AFTER);
        }
    }
    changed
}

/// Starts a check (or joins the running one, so its answer is told to `source` too). False when none could start.
fn check(st: &mut State, source: Source) -> bool {
    if let Some((phase, _)) = &st.updates.demo {
        // screenshots: answer at once, offline
        let _ = phase;
        st.updates.source = Some(source);
        let checked = shrew_update::Checked { latest: st.updates.latest.clone(), update: st.updates.available.clone() };
        answer(st, Ok(checked));
        return true;
    }
    if st.updates.headless && st.updates.updater.is_none() && std::env::var_os("SHREW_UPDATE_FEED_URL").is_none() {
        return false; // --screenshot and tests never reach the network on their own
    }
    st.updates.load_prefs();
    let u = updater(st);
    let started = u.check();
    if started || u.checking() {
        let loudest = |s: Source| match s {
            Source::Manual => 3,
            Source::Startup => 2,
            Source::Daily => 1,
            Source::Settings => 0,
        };
        let s = st.updates.source.map_or(source, |was| if loudest(was) >= loudest(source) { was } else { source });
        st.updates.source = Some(s);
        return true;
    }
    false
}

/// Picks up what the updater finished: a check's answer, a change of phase. True when something changed.
pub fn poll(st: &mut State) -> bool {
    let Some(u) = st.updates.updater.clone() else { return false };
    let mut changed = false;
    if let Some(result) = u.take_checked() {
        answer(st, result);
        changed = true;
    }
    // Settings shows other rows for another kind of phase
    let kind = phase_kind(&st.updates.phase());
    if kind != st.updates.shown_phase {
        st.updates.shown_phase = kind;
        st.rebuild_rows();
        changed = true;
    }
    changed
}

fn phase_kind(p: &Phase) -> u8 {
    match p {
        Phase::Idle => 0,
        Phase::Downloading { .. } => 1,
        Phase::Verifying { .. } => 2,
        Phase::Ready(_) => 3,
        Phase::Failed(_) => 4,
    }
}

/// A check answered: remember it, and tell whoever should hear (see [`policy`]).
fn answer(st: &mut State, result: Result<shrew_update::Checked, String>) {
    let source = st.updates.source.take().unwrap_or(Source::Daily);
    let install = std::mem::take(&mut st.updates.install_after_check);
    let now = policy::now();
    let u = &mut st.updates;
    match result {
        Ok(c) => {
            u.error = None;
            u.retry_at = None;
            u.latest = c.latest.clone();
            u.available = c.update.clone();
            let version = c.update.as_ref().map(|r| r.version.clone());
            let notify = u.prefs.should_notify(source, version.as_deref(), now);
            u.prefs.checked(now, c.latest.clone(), if notify { version.as_deref() } else { None });
            u.save_prefs();
            match (&c.update, notify) {
                (Some(r), true) => {
                    if u.dialog.is_none() {
                        u.show(Toast::Available(r.clone()));
                    }
                }
                (Some(_), false) => {}
                (None, _) if source == Source::Manual => {
                    let latest = c.latest.clone().unwrap_or_else(|| u.current.clone());
                    if shrew_update::is_newer(&latest, &u.current) {
                        let t = shrew_update::TARGET;
                        u.note(format!("shrew {latest} is out"), format!("It has no download for {t} yet."), false);
                    } else {
                        u.note("shrew is up to date", format!("{latest} is the latest release."), false);
                    }
                }
                (None, _) => {}
            }
            if install && u.available.is_some() {
                update_command(st);
            }
        }
        Err(e) => {
            log::info!("update: check failed: {e}");
            if source == Source::Manual {
                u.show(Toast::Note {
                    title: "Could not check for updates".into(),
                    body: e.clone(),
                    error: true,
                    link: None,
                    retry: true,
                });
            }
            if !source.asked() {
                u.retry_at = Some(Instant::now() + policy::RETRY_AFTER);
            }
            u.error = Some(e);
        }
    }
    st.rebuild_rows();
}

// ----- commands -----

/// Ctrl+Shift+U, Help > Check for Updates…, the palette, `:checkupdate`: with an update in hand (its notification
/// showing, a download going or done) the dialog; otherwise a check that always answers.
pub fn check_command(st: &mut State) {
    let showing = matches!(st.updates.toast, Some(Toast::Available(_)));
    if showing || !matches!(st.updates.phase(), Phase::Idle) {
        return open_dialog(st);
    }
    if !check(st, Source::Manual) && st.updates.demo.is_none() {
        st.updates.note("Could not check for updates", "No network access in this mode.", true);
    }
}

/// `:update`, palette *Shrew: Update shrew…*: the dialog, downloading at once when there is an update; with
/// nothing known yet, a check first.
pub fn update_command(st: &mut State) {
    if st.updates.available.is_none() && matches!(st.updates.phase(), Phase::Idle) {
        st.updates.install_after_check = true;
        if !check(st, Source::Manual) {
            st.updates.install_after_check = false;
        }
        return;
    }
    open_dialog(st);
    if st.updates.stage() == Stage::Offer {
        download(st);
    }
}

/// Settings > Updates > Check now: the answer shows in Settings, not as a notification.
pub fn check_now(st: &mut State) {
    check(st, Source::Settings);
}

/// Opens the Update shrew dialog (over everything: Settings, a picker, About and any open menu close; one overlay
/// at a time).
pub fn open_dialog(st: &mut State) {
    st.settings.close();
    st.ed.picker = None;
    st.ed.about = None;
    st.titlebar.menu = None;
    st.menu = None;
    let u = &mut st.updates;
    u.dialog_hot = None;
    if matches!(u.toast, Some(Toast::Available(_))) {
        u.toast = None;
    }
    u.dialog.get_or_insert_with(Dialog::default);
}

/// One overlay at a time: when a menu or another modal panel opened while the dialog was open (the title bar still
/// works under it), the dialog closes (as Hide or Later would: a download goes on, the status bar shows it).
pub fn yield_dialog(st: &mut State) {
    if st.updates.dialog.is_some()
        && (st.menu_open() || st.settings.is_open() || st.ed.picker.is_some() || st.ed.about.is_some())
    {
        st.updates.dialog = None;
        st.updates.dialog_hot = None;
    }
}

/// The status-bar button was clicked.
pub fn status_clicked(st: &mut State) {
    open_dialog(st);
}

fn download(st: &mut State) {
    let Some(release) = st.updates.available.clone() else { return };
    if st.updates.support().is_err() {
        return;
    }
    if let Some((phase, _)) = &mut st.updates.demo {
        *phase = Phase::Downloading { version: release.version, done: 0, total: release.size, speed: 0 };
        return;
    }
    let u = updater(st);
    u.reset();
    u.download(&release);
}

fn open_link(st: &mut State, url: &str) {
    if let Err(e) = crate::open::url(url) {
        st.updates.note("Could not open the release page", format!("{url}: {e}"), true);
    }
}

fn press(st: &mut State, b: Button) {
    match b {
        Button::Close | Button::Hide | Button::Later => st.updates.dialog = None,
        Button::CheckAgain => {
            check(st, Source::Settings);
        }
        Button::ViewRelease => {
            let page = st.updates.page();
            open_link(st, &page);
        }
        Button::Download | Button::Retry => download(st),
        Button::Cancel => match &mut st.updates.demo {
            Some((phase, _)) => *phase = Phase::Idle,
            None => {
                if let Some(u) = &st.updates.updater {
                    u.cancel();
                }
            }
        },
        Button::Restart => restart(st),
    }
}

/// A key. The dialog takes every key while open; Esc also dismisses the notification (and still does what it
/// does in the editor). True when the key was taken.
pub fn key(st: &mut State, k: KeyPress) -> bool {
    if st.updates.dialog.is_some() {
        let stage = st.updates.stage();
        let page = st.updates.hits.notes_rows.max(1);
        let Some(d) = st.updates.dialog.as_mut() else { return false };
        match d.key(&stage, k, page) {
            Act::Press(b) => press(st, b),
            Act::Scroll(n) => d.scroll = d.scroll.saturating_add_signed(n),
            Act::None => {}
        }
        return true;
    }
    if k.key == Key::Esc
        && st.updates.toast.is_some()
        && st.ed.picker.is_none()
        && st.ed.prompt.is_none()
        && !st.settings.is_open()
        && st.titlebar.menu.is_none()
    {
        st.updates.toast = None;
    }
    false
}

/// Whether the dialog is open (it is modal: nothing else takes the mouse).
pub fn modal(st: &State) -> bool {
    st.updates.dialog.is_some()
}

/// A left click: on the dialog (outside it hides it), the notification or its buttons. True when taken.
pub fn click(st: &mut State, x: f32, y: f32) -> bool {
    let h = st.updates.hits.clone();
    if st.updates.dialog.is_some() {
        if let Some(&(_, b)) = h.buttons.iter().find(|(r, _)| r.contains(x, y)) {
            press(st, b);
        } else if !h.dialog.is_some_and(|r| r.contains(x, y)) {
            let stage = st.updates.stage();
            press(st, stage.escape());
        }
        return true;
    }
    let Some(toast) = st.updates.toast.clone() else { return false };
    if !h.toast.area.is_some_and(|r| r.contains(x, y)) {
        return false;
    }
    if h.toast.close.is_some_and(|r| r.contains(x, y)) {
        st.updates.toast = None;
        return true;
    }
    let Some(i) = h.toast.buttons.iter().position(|r| r.contains(x, y)) else { return true };
    match toast {
        Toast::Available(r) => {
            let supported = st.updates.support().is_ok();
            match (i, supported) {
                (0, true) => {
                    st.updates.toast = None;
                    update_command(st);
                }
                _ => open_link(st, &r.page),
            }
        }
        Toast::Note { link, retry, .. } => {
            st.updates.toast = None;
            if retry && i == 0 {
                check(st, Source::Manual);
            } else if let Some((_, url)) = link {
                open_link(st, &url);
            }
        }
    }
    true
}

/// Whether (`x`, `y`) is on the update UI, which takes the pointer: anywhere while the dialog is open (it is
/// modal), else on the notification.
pub fn over(st: &State, x: f32, y: f32) -> bool {
    let u = &st.updates;
    u.dialog.is_some() || (u.toast.is_some() && u.hits.toast.contains(x, y))
}

/// The pointer moved: the dialog's button or the notification's part under it lights up. (redraw, over a button)
pub fn hover(st: &mut State, x: f32, y: f32) -> (bool, bool) {
    let u = &mut st.updates;
    let button =
        if u.dialog.is_some() { u.hits.buttons.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1) } else { None };
    let changed = std::mem::replace(&mut u.dialog_hot, button) != button;
    if u.dialog.is_some() || u.toast.is_none() {
        let lit = std::mem::take(&mut u.hot) != crate::toast::Hot::None;
        return (changed || lit, button.is_some());
    }
    let (redraw, on) = crate::toast::hover(&u.hits.toast, &mut u.hot, x, y);
    (changed || redraw, on)
}

/// The pointer left the window. True when something was lit.
pub fn pointer_left(st: &mut State) -> bool {
    let u = &mut st.updates;
    u.dialog_hot.take().is_some() | (std::mem::take(&mut u.hot) != crate::toast::Hot::None)
}

/// The mouse wheel over the dialog scrolls its notes. True when taken.
pub fn wheel(st: &mut State, lines: f32) -> bool {
    let Some(d) = &mut st.updates.dialog else { return false };
    if lines.abs() >= 0.5 {
        d.scroll = d.scroll.saturating_add_signed(lines.round() as isize);
    }
    true
}

// ----- the status bar and Settings -----

/// The status-bar button: text and tone, when one shows.
pub fn status_label(st: &State) -> Option<(String, Tone)> {
    let u = &st.updates;
    Some(match u.phase() {
        Phase::Downloading { done, total, .. } => (format!("⬇ {}%", dialog::percent(done, total)), Tone::Muted),
        Phase::Verifying { .. } => ("Verifying update…".into(), Tone::Muted),
        Phase::Ready(_) => ("⟳ Restart to update".into(), Tone::Success),
        Phase::Failed(_) => ("Update failed".into(), Tone::Error),
        Phase::Idle if u.checking() && u.source == Some(Source::Manual) => {
            ("Checking for updates…".into(), Tone::Muted)
        }
        Phase::Idle => (format!("⬆ shrew {}", u.available.as_ref()?.version), Tone::Accent),
    })
}

/// Settings > Updates > Status.
pub fn status_text(st: &State) -> String {
    let u = &st.updates;
    match u.phase() {
        Phase::Downloading { version, done, total, .. } => {
            return format!("Downloading shrew {version}: {}", dialog::progress_text(done, total, 0).0);
        }
        Phase::Verifying { version } => return format!("Verifying shrew {version}…"),
        Phase::Ready(s) => return format!("shrew {} is downloaded and verified", s.release.version),
        Phase::Failed(_) => return "The download failed".into(),
        Phase::Idle => {}
    }
    if u.checking() {
        return "Checking…".into();
    }
    if let Some(r) = &u.available {
        return format!("shrew {} is available (you have {})", r.version, u.current);
    }
    if let Some(e) = &u.error {
        return format!("Could not check: {e}");
    }
    match &u.latest {
        Some(l) if shrew_update::is_newer(l, &u.current) => format!("shrew {l} has no download for this system yet"),
        Some(l) => format!("shrew is up to date ({l} is the latest release)"),
        None if u.prefs.last_checked.is_some() && u.updater.is_some() => {
            format!("shrew is up to date ({} is the latest release)", u.current)
        }
        None => "Not checked yet".into(),
    }
}

/// Settings > Updates > Last checked.
pub fn last_checked_text(st: &State) -> String {
    match st.updates.prefs.last_checked {
        Some(t) => policy::ago(t, policy::now()),
        None if st.updates.prefs_path.is_none() && !st.updates.headless => "not yet this session".into(),
        None => "never".into(),
    }
}

/// The update row of Settings > Updates, when there is something to do: (label, description, button).
pub fn settings_action(st: &State) -> Option<(String, String, &'static str)> {
    let stage = st.updates.stage();
    let version = match st.updates.phase() {
        Phase::Ready(s) => s.release.version,
        _ => st.updates.available.as_ref().map(|r| r.version.clone()).unwrap_or_default(),
    };
    let label = format!("shrew {version}");
    Some(match stage {
        Stage::Nothing => return None,
        Stage::Unsupported(why) => (
            label,
            format!("This installation cannot update itself: {why}. Download the new version from the release page."),
            "View release",
        ),
        Stage::Offer => (
            label,
            "Downloads it from GitHub with a progress bar and checks it against the published sha256; then Restart \
             to update."
                .into(),
            "Update now",
        ),
        Stage::Downloading { .. } | Stage::Verifying => {
            (label, "Downloading in the background; the status bar shows how far.".into(), "Show progress")
        }
        Stage::Ready => (
            label,
            "Downloaded and verified. Restart to install it: your open files come back, unsaved changes too.".into(),
            "Restart to update",
        ),
        Stage::Failed(e) => (label, format!("The download failed: {e}"), "Retry"),
    })
}

/// The update row's button.
pub fn settings_run(st: &mut State) {
    match st.updates.stage() {
        Stage::Unsupported(_) => {
            let page = st.updates.page();
            open_link(st, &page);
        }
        Stage::Ready => restart(st),
        Stage::Failed(_) => {
            open_dialog(st);
            download(st);
        }
        _ => update_command(st),
    }
}

// ----- restarting -----

/// How many terminals are open (their shells end with a restart).
fn terminals(st: &State) -> usize {
    #[cfg(feature = "terminal")]
    return st.terminals.tabs.len();
    #[cfg(not(feature = "terminal"))]
    {
        let _ = st;
        0
    }
}

/// Restart to update: saves the session (unsaved text too), puts the new binary in place, ends the children
/// (terminals, the MCP server, `srw --wait` clients) and re-executes shrew with `--restore-session`. Only returns
/// when something failed; then nothing is lost and shrew keeps running.
pub fn restart(st: &mut State) {
    let Phase::Ready(staged) = st.updates.phase() else { return };
    if st.updates.demo.is_some() {
        return st.updates.note("Restart to update", "Not in a demo.", false);
    }
    // 1. the session, before anything changes: no session, no restart
    let Some(path) = session::path() else {
        return st.updates.note("Could not restart", "There is no cache folder to keep the open files in.", true);
    };
    let s = session::capture(&st.ed, &st.updates.current, Some(staged.release.page.clone()));
    if let Err(e) = session::save(&path, &s) {
        let body = format!("{}: {e}. shrew did not restart; nothing was changed.", path.display());
        return st.updates.note("Could not save the open files", body, true);
    }
    // 2. the new binary in place (its sha256 checked again on the way)
    let Some(u) = st.updates.updater.clone() else { return };
    let installed = match u.install() {
        Ok(i) => i,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            log::warn!("update: {e}");
            st.updates.dialog.get_or_insert_with(Dialog::default);
            return;
        }
    };
    // 3. the children
    #[cfg(feature = "mcp")]
    if let Some(m) = st.mcp.take() {
        m.stop();
    }
    #[cfg(feature = "terminal")]
    st.terminals.close_for_restart(Duration::from_secs(2));
    st.release_waits();
    // 4. the new process in this one's place
    log::info!("update: restarting into {} ({})", installed.version, installed.path.display());
    let err = shrew_update::restart(&installed, restart_args(&path));
    // only on failure: the old binary is gone, so say how to get going again
    let _ = std::fs::remove_file(&path);
    if st.ed.features.on(Feature::Mcp) {
        st.set_mcp(true);
    }
    let body =
        format!("shrew {} is installed, but starting it failed: {err}. Quit and start shrew again.", installed.version);
    st.updates.note("Could not restart", body, true);
}

/// The new process: in the foreground of this one (already detached when this one is), restoring the session.
fn restart_args(session: &std::path::Path) -> Vec<std::ffi::OsString> {
    vec!["-f".into(), crate::cli::RESTORE_FLAG.into(), session.as_os_str().to_owned()]
}

/// Startup: restores the session a restart left (`--restore-session`, or one an interrupted restart left behind)
/// and says "Updated to shrew X". Call once the state exists, before the first frame.
pub fn restore_session(st: &mut State, explicit: Option<&std::path::Path>) {
    let path = explicit.map(PathBuf::from).or_else(session::path);
    let Some(path) = path.filter(|p| p.exists()) else { return };
    let Some(s) = session::take(&path) else { return };
    let n = session::restore(st, &s);
    log::info!("restored {n} buffer(s) from {}", path.display());
    let now = st.updates.current.clone();
    let title = if shrew_update::is_newer(&now, &s.from) {
        format!("Updated to shrew {now}")
    } else {
        format!("shrew {now} restarted")
    };
    let body = match n {
        0 => "Nothing was open.".to_string(),
        1 => "Your file is back as you left it.".to_string(),
        n => format!("Your {n} files are back as you left them."),
    };
    let link = s.page.clone().filter(|p| p.starts_with("https://")).map(|p| ("What's new", p));
    st.updates.show(Toast::Note { title, body, error: false, link, retry: false });
}

impl State {
    /// Answers the `srw --wait` clients (before a restart: their buffers come back in another process, which they
    /// cannot wait for).
    fn release_waits(&mut self) {
        if self.waits.is_empty() {
            return;
        }
        // dropping a wait's sender tells its client the files are closed, from a thread of its own: give those a
        // moment to write before this process is replaced
        self.waits.clear();
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ----- screenshots -----

/// `SHREW_UPDATE_DEMO=<stage>` (debug builds, headless only): shows the update UI in a stage with a made-up
/// release, offline, for `--screenshot`. Stages: `available`, `downloading`, `verifying`, `ready`, `unsupported`,
/// `failed`, `uptodate`, `checkfailed`, `updated`.
pub fn demo(st: &mut State) {
    #[cfg(debug_assertions)]
    {
        let Ok(stage) = std::env::var("SHREW_UPDATE_DEMO") else { return };
        if !st.updates.headless {
            return;
        }
        let release = demo_release();
        let staged = shrew_update::Staged { path: "/tmp/shrew-demo".into(), release: release.clone(), sha256: [0; 32] };
        let size = release.size;
        let u = &mut st.updates;
        u.current = "0.2.0".into();
        u.latest = Some(release.version.clone());
        u.prefs.last_checked = Some(policy::now() - 180);
        u.prefs_path = Some(PathBuf::from("/nonexistent/demo"));
        u.available = Some(release.clone());
        let ok = Ok(());
        let (phase, support) = match stage.as_str() {
            "downloading" => (
                Phase::Downloading {
                    version: release.version.clone(),
                    done: size * 37 / 100,
                    total: size,
                    speed: 3_400_000,
                },
                ok,
            ),
            "verifying" => (Phase::Verifying { version: release.version.clone() }, ok),
            "ready" => (Phase::Ready(staged), ok),
            "failed" => (Phase::Failed("cannot reach github.com: connection timed out".into()), ok),
            "unsupported" => {
                (Phase::Idle, Err("/usr/bin/srw belongs to a package manager; update shrew with it".to_string()))
            }
            "uptodate" | "checkfailed" => {
                u.available = None;
                u.latest = Some("0.2.0".into());
                (Phase::Idle, ok)
            }
            _ => (Phase::Idle, ok),
        };
        u.demo = Some((phase, support));
        match stage.as_str() {
            "available" | "unsupported" => u.show(Toast::Available(release)),
            "uptodate" => u.note("shrew is up to date", "0.2.0 is the latest release.", false),
            "checkfailed" => u.show(Toast::Note {
                title: "Could not check for updates".into(),
                body: "cannot reach api.github.com: connection timed out".into(),
                error: true,
                link: None,
                retry: true,
            }),
            "updated" => {
                u.available = None;
                u.current = release.version.clone();
                u.show(Toast::Note {
                    title: format!("Updated to shrew {}", release.version),
                    body: "Your 3 files are back as you left them.".into(),
                    error: false,
                    link: Some(("What's new", release.page.clone())),
                    retry: false,
                });
            }
            _ => {}
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = st;
}

#[cfg(debug_assertions)]
fn demo_release() -> Release {
    Release {
        version: "0.3.0".into(),
        tag: "v0.3.0".into(),
        notes: "## What's new\r\n\r\n- **In-app updates**: a notification when a new shrew is out, the *Update shrew* \
                dialog with the release notes, a progress bar, speed and time left, and **Restart to update**, which \
                brings every open file back, unsaved changes too.\r\n- Settings > Updates shows when shrew last \
                checked and answers *Check now* inline.\r\n- `srw --self-update` updates from a script.\r\n\r\n## \
                Fixed\r\n\r\n- The git toast and the update notifications share one look.\r\n- Release notes render \
                [links](https://example.com) as text.\r\n\r\n**Full Changelog**: \
                https://github.com/perunok/shrew/compare/v0.2.0...v0.3.0\r\n"
            .into(),
        published: "2026-10-26T12:00:00Z".into(),
        asset_url: String::new(),
        checksum_url: String::new(),
        size: 22_347_552,
        page: "https://github.com/perunok/shrew/releases/tag/v0.3.0".into(),
    }
}

#[cfg(test)]
mod tests;
