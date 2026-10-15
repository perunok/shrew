//! Git integration (cargo feature `git`). One [`GitService`] serves the whole app: it is created by the first git
//! command and starts a thread only when it gets work, so a session that never touches git, or runs outside any
//! repository, pays nothing. Its events arrive through the app's waker; [`poll`] hands each to the part that asked.
//!
//! Parts: [`diffview`], the diff tab.

pub mod diffview;

use crate::state::{State, Waker};
use shrew_core::{Choice, Command};
use shrew_git::{GitService, Request, Ticket};
use std::sync::Arc;

/// The app's git state: the service (once created) and each part's own state.
#[derive(Default)]
pub struct Git {
    service: Option<GitService>,
    // ----- diff view -----
    pub diff: diffview::Views,
}

impl Git {
    /// The service, created on first use (no thread starts until a request arrives).
    pub fn service(&mut self, wake: &Waker) -> &GitService {
        self.service.get_or_insert_with(|| {
            let wake = wake.clone();
            GitService::new(Arc::new(move || wake()))
        })
    }

    /// The service if it exists: nothing to poll or tell otherwise.
    pub fn running(&self) -> Option<&GitService> {
        self.service.as_ref()
    }
}

/// Queues a request, creating the service on first use.
pub fn send(st: &mut State, req: Request) -> Ticket {
    let wake = st.wake.clone();
    st.git.service(&wake).send(req)
}

/// Takes the service's events and hands them out. True when the screen may have changed.
pub fn poll(st: &mut State) -> bool {
    let events = st.git.running().map(|s| s.poll()).unwrap_or_default();
    let mut changed = !events.is_empty();
    for ev in &events {
        diffview::event(st, ev);
    }
    changed |= diffview::poll(st);
    changed
}

/// A git command from the palette, a menu, a key or `:G…` (see [`shrew_core::Event::Git`]).
pub fn command(st: &mut State, cmd: Command, arg: Option<String>) {
    match cmd {
        Command::GitDiffFile
        | Command::GitDiffFileHead
        | Command::GitDiffAll
        | Command::GitDiffStaged
        | Command::GitDiffCommit
        | Command::GitDiffCompare
        | Command::GitDiffToggleLayout => diffview::command(st, cmd, arg),
        _ => {}
    }
}

/// A git list (commits, refs) to open (`value: None`) or a row picked in it.
pub fn choice(st: &mut State, choice: Choice, value: Option<String>) {
    diffview::choice(st, choice, value);
}

/// The window gained or lost the focus: the service only watches repositories while focused.
pub fn focus(st: &mut State, focused: bool) {
    if let Some(s) = st.git.running() {
        s.set_focused(focused);
    }
    diffview::focus(st, focused);
}

/// After every key, click or event: views follow their buffers.
pub fn after_input(st: &mut State) {
    diffview::after_input(st);
}

/// Work still in flight that changes the picture (headless screenshots wait for it).
pub fn busy(st: &State) -> bool {
    diffview::busy(st)
}
