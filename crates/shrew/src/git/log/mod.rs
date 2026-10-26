//! The log tab (`git_log`, `git_log_all`) and file history (`git_file_history`): a buffer whose tab shows the
//! commits instead of text. Rows come in pages of [`PAGE`] as you scroll; the commit graph's lanes come laid out
//! by the backend. Keys: `j`/`k` move, Enter shows the commit (message and changed files; Enter on a file opens
//! its diff in the commit), `d` opens the commit's diff, `/` searches the loaded rows (`n`/`N`), `a` filters by
//! author, `p` shows the history of a path, `b` makes a branch at the commit, `y` copies its id, `r` reloads, `q`
//! closes. The mouse clicks, double-clicks and scrolls.

pub mod draw;
pub mod graph;
#[cfg(test)]
mod tests;

use super::ask::{Ask, Then};
use super::diffview::{DiffSpec, Rev};
use super::textbox::TextBox;
use super::{Purpose, on};
use crate::state::State;
use shrew_core::{Command, Document, Editor, Key, KeyPress};
use shrew_git::graph::GraphRow;
use shrew_git::{CommitDetails, CommitInfo, HistoryPage, LogPage, LogQuery, Oid, RefLabel, Request};
use shrew_render::Rect;
use std::path::PathBuf;
use std::sync::Arc;

/// Commits per request.
pub const PAGE: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// HEAD's history, or every branch's (`all`).
    Log { all: bool },
    /// One file's, following renames.
    History { path: String },
}

#[derive(Clone, Debug)]
pub struct Row {
    pub commit: CommitInfo,
    pub graph: Option<GraphRow>,
    pub refs: Vec<RefLabel>,
    /// The file's name at this commit (file history).
    pub path: Option<String>,
}

/// A page from either kind of request.
pub struct Page {
    pub rows: Vec<Row>,
    pub more: bool,
}

impl Page {
    pub fn from_log(p: &LogPage) -> Page {
        let rows = p
            .rows
            .iter()
            .map(|r| Row { commit: r.commit.clone(), graph: r.graph.clone(), refs: r.refs.clone(), path: None })
            .collect();
        Page { rows, more: p.more }
    }

    pub fn from_history(p: &HistoryPage) -> Page {
        let rows = p
            .rows
            .iter()
            .map(|r| Row { commit: r.commit.clone(), graph: None, refs: Vec::new(), path: Some(r.path.clone()) })
            .collect();
        Page { rows, more: p.more }
    }
}

/// The commit shown under the list.
pub struct Details {
    pub id: Oid,
    pub data: Option<Arc<CommitDetails>>,
    pub error: Option<String>,
    /// The highlighted changed file.
    pub file: usize,
    pub top: usize,
}

#[derive(Default, Debug)]
pub struct Hits {
    pub area: Option<Rect>,
    pub list: Option<Rect>,
    /// Rows: rect, index into `shown`.
    pub rows: Vec<(Rect, usize)>,
    pub files: Vec<(Rect, usize)>,
    pub details: Option<Rect>,
}

pub struct View {
    pub buffer: u64,
    pub repo: PathBuf,
    pub kind: Kind,
    pub rows: Vec<Row>,
    /// Indices into `rows` that pass the author filter (all of them without one).
    pub shown: Vec<usize>,
    pub more: bool,
    pub loading: bool,
    /// Bumped by a reload: answers to older requests are dropped.
    generation: u64,
    pub error: Option<String>,
    /// Into `shown`.
    pub selected: usize,
    pub top: usize,
    pub page: usize,
    pub follow: bool,
    pub search: Option<TextBox>,
    /// The last search, for `n` and `N`.
    pub last_search: String,
    /// Searching ran out of loaded rows: carry on when more come.
    search_more: bool,
    pub author: Option<String>,
    pub details: Option<Details>,
    /// Keys go to the details' file list.
    pub in_details: bool,
    /// HEAD or the refs moved since the rows were read.
    pub stale: bool,
    pub hits: Hits,
    last_click: Option<(std::time::Instant, usize)>,
}

impl View {
    fn new(buffer: u64, repo: PathBuf, kind: Kind) -> View {
        View {
            buffer,
            repo,
            kind,
            rows: Vec::new(),
            shown: Vec::new(),
            more: true,
            loading: false,
            generation: 0,
            error: None,
            selected: 0,
            top: 0,
            page: 20,
            follow: true,
            search: None,
            last_search: String::new(),
            search_more: false,
            author: None,
            details: None,
            in_details: false,
            stale: false,
            hits: Hits::default(),
            last_click: None,
        }
    }

    pub fn title(&self) -> String {
        match &self.kind {
            Kind::Log { all: false } => "Git Log".into(),
            Kind::Log { all: true } => "Git Log (all)".into(),
            Kind::History { path } => format!("History: {}", path.rsplit('/').next().unwrap_or(path)),
        }
    }

    pub fn current(&self) -> Option<&Row> {
        self.rows.get(*self.shown.get(self.selected)?)
    }

    /// Applies the author filter to the rows.
    fn refilter(&mut self) {
        let keep = self.current().map(|r| r.commit.id);
        self.shown = match &self.author {
            None => (0..self.rows.len()).collect(),
            Some(a) => {
                let a = a.to_lowercase();
                let hit = |r: &Row| {
                    r.commit.author.name.to_lowercase().contains(&a)
                        || r.commit.author.email.to_lowercase().contains(&a)
                };
                (0..self.rows.len()).filter(|&i| hit(&self.rows[i])).collect()
            }
        };
        self.selected = keep
            .and_then(|id| self.shown.iter().position(|&i| self.rows[i].commit.id == id))
            .unwrap_or(self.selected)
            .min(self.shown.len().saturating_sub(1));
        self.follow = true;
    }

    /// Takes a page asked for at `skip` (0: a reload, which replaces the rows). False when it no longer fits.
    fn add(&mut self, page: Page, skip: usize) -> bool {
        if skip != 0 && skip != self.rows.len() {
            return false;
        }
        if skip == 0 {
            let keep = self.current().map(|r| r.commit.id);
            self.rows = page.rows;
            self.refilter();
            if let Some(i) = keep.and_then(|id| self.shown.iter().position(|&i| self.rows[i].commit.id == id)) {
                self.selected = i;
            }
        } else {
            self.rows.extend(page.rows);
            self.refilter();
        }
        self.more = page.more;
        self.loading = false;
        self.error = None;
        true
    }

    pub fn move_by(&mut self, d: isize) {
        if self.shown.is_empty() {
            return;
        }
        let last = self.shown.len() as isize - 1;
        self.selected = (self.selected as isize + d).clamp(0, last) as usize;
        self.follow = true;
    }

    /// Keeps the selection in `visible` rows.
    pub fn fit(&mut self, visible: usize) {
        self.page = visible.max(1);
        if self.follow {
            if self.selected < self.top {
                self.top = self.selected;
            } else if self.selected >= self.top + self.page {
                self.top = self.selected + 1 - self.page;
            }
            self.follow = false;
        }
        self.top = self.top.min(self.shown.len().saturating_sub(self.page));
    }

    /// Close enough to the end of what is loaded to ask for the next page.
    pub fn wants_more(&self) -> bool {
        self.more && !self.loading && self.error.is_none() && self.top + 3 * self.page.max(10) >= self.shown.len()
    }

    /// A search hit from the selection on (`back`: before it). Matches the summary, author, id and refs.
    pub fn find(&self, needle: &str, back: bool, from_next: bool) -> Option<usize> {
        if needle.is_empty() || self.shown.is_empty() {
            return None;
        }
        let n = needle.to_lowercase();
        let hit = |i: usize| {
            let r = &self.rows[self.shown[i]];
            r.commit.summary.to_lowercase().contains(&n)
                || r.commit.author.name.to_lowercase().contains(&n)
                || r.commit.id.to_string().starts_with(&n)
                || r.refs.iter().any(|l| l.name.to_lowercase().contains(&n))
        };
        let len = self.shown.len();
        let step = usize::from(from_next);
        if back {
            (0..self.selected + 1 - step.min(self.selected + 1)).rev().find(|&i| hit(i))
        } else {
            (self.selected + step..len).find(|&i| hit(i))
        }
    }

    pub fn details_files(&self) -> usize {
        self.details.as_ref().and_then(|d| d.data.as_ref()).map_or(0, |d| d.files.len())
    }
}

#[derive(Default)]
pub struct Views {
    list: Vec<View>,
}

impl Views {
    pub fn get(&self, buffer: u64) -> Option<&View> {
        self.list.iter().find(|v| v.buffer == buffer)
    }

    pub fn get_mut(&mut self, buffer: u64) -> Option<&mut View> {
        self.list.iter_mut().find(|v| v.buffer == buffer)
    }

    pub fn ids(&self) -> Vec<u64> {
        self.list.iter().map(|v| v.buffer).collect()
    }

    /// Forgets views whose tab was closed (or reused for a file).
    pub fn prune(&mut self, ed: &Editor) {
        if self.list.is_empty() {
            return;
        }
        self.list.retain(|v| ed.buffers.iter().any(|b| b.id == v.buffer && b.doc.title.as_deref() == Some(&v.title())));
    }

    pub fn clear_hits(&mut self) {
        for v in &mut self.list {
            v.hits = Hits::default();
        }
    }
}

/// The view of the active buffer, if it is one.
pub fn active(st: &State) -> Option<&View> {
    let b = st.ed.buf();
    st.git.views.get(b.id).filter(|v| b.doc.title.as_deref() == Some(&v.title()))
}

fn active_id(st: &State) -> Option<u64> {
    active(st).map(|v| v.buffer)
}

/// Opens (or goes to) the log tab.
pub fn open(st: &mut State, repo: PathBuf, all: bool) {
    open_kind(st, repo, Kind::Log { all });
}

pub fn open_history(st: &mut State, repo: PathBuf, path: String) {
    open_kind(st, repo, Kind::History { path });
}

fn open_kind(st: &mut State, repo: PathBuf, kind: Kind) {
    let existing = st.git.views.list.iter().find(|v| v.repo == repo && v.kind == kind).map(|v| v.buffer);
    if let Some(id) = existing
        && let Some(i) = st.ed.buffers.iter().position(|b| b.id == id)
    {
        st.ed.switch_to(i);
        return reload(st, id);
    }
    let view = View::new(0, repo, kind);
    let mut doc = Document::from_str("git log\n");
    doc.title = Some(view.title());
    st.ed.install_document(doc);
    st.process_events();
    let id = st.ed.buf().id;
    st.git.views.list.push(View { buffer: id, ..view });
    request(st, id, 0);
}

/// Asks for the rows from `skip` on.
fn request(st: &mut State, id: u64, skip: usize) {
    let Some(v) = st.git.views.get_mut(id) else { return };
    v.loading = true;
    if skip == 0 {
        v.generation += 1;
    }
    let generation = v.generation;
    let repo = v.repo.clone();
    let req = match &v.kind {
        Kind::Log { all } => {
            Request::Log { repo, query: LogQuery { all: *all, skip, limit: PAGE, graph: true, ..LogQuery::default() } }
        }
        Kind::History { path } => Request::FileHistory { repo, path: path.clone(), skip, limit: PAGE },
    };
    st.git.send(req, Purpose::Log(id, generation, skip));
}

/// Reads the rows again from the top (keeping the selected commit).
pub fn reload(st: &mut State, id: u64) {
    if let Some(v) = st.git.views.get_mut(id) {
        v.stale = false;
        v.more = true;
    }
    request(st, id, 0);
}

/// Asks for the next page when the view is near the end of what it has.
pub fn more(st: &mut State, id: u64) {
    let Some(v) = st.git.views.get(id) else { return };
    if v.wants_more() {
        let skip = v.rows.len();
        request(st, id, skip);
    }
}

pub fn page(st: &mut State, (id, generation, skip): (u64, u64, usize), r: shrew_git::Result<Page>) {
    let Some(v) = st.git.views.get_mut(id) else { return };
    if generation != v.generation {
        return; // a reload came since
    }
    match r {
        Ok(p) => {
            if !v.add(p, skip) {
                return;
            }
            if v.search_more {
                v.search_more = false;
                let needle = v.last_search.clone();
                search(st, id, &needle, false, true);
            }
        }
        Err(e) => {
            v.loading = false;
            v.error = Some(e.message);
        }
    }
}

pub fn details(st: &mut State, id: u64, r: shrew_git::Result<Arc<CommitDetails>>) {
    let Some(d) = st.git.views.get_mut(id).and_then(|v| v.details.as_mut()) else { return };
    match r {
        Ok(data) if data.info.id == d.id => d.data = Some(data),
        Ok(_) => {}
        Err(e) => d.error = Some(e.message),
    }
}

/// HEAD or the refs moved: the visible view reloads now, the others when shown.
pub fn repo_moved(st: &mut State) {
    let active = active_id(st);
    for v in &mut st.git.views.list {
        v.stale = true;
    }
    if let Some(id) = active {
        reload(st, id);
    }
}

/// Shows the selected commit's details (Enter); again on the same commit, hides them.
fn toggle_details(st: &mut State, id: u64) {
    let Some(v) = st.git.views.get_mut(id) else { return };
    let Some(cid) = v.current().map(|r| r.commit.id) else { return };
    if v.details.as_ref().is_some_and(|d| d.id == cid) {
        v.details = None;
        v.in_details = false;
        return;
    }
    show_details(st, id, cid);
}

fn show_details(st: &mut State, id: u64, cid: Oid) {
    let Some(v) = st.git.views.get_mut(id) else { return };
    v.details = Some(Details { id: cid, data: None, error: None, file: 0, top: 0 });
    let repo = v.repo.clone();
    st.git.send(Request::CommitDetails { repo, id: cid }, Purpose::Details(id));
}

/// Jumps to the next (`back`: previous) row matching `needle`; out of loaded rows, loads more and carries on.
fn search(st: &mut State, id: u64, needle: &str, back: bool, from_next: bool) {
    let Some(v) = st.git.views.get_mut(id) else { return };
    v.last_search = needle.to_string();
    match v.find(needle, back, from_next) {
        Some(i) => {
            v.selected = i;
            v.follow = true;
            v.search_more = false;
        }
        None if !back && v.more && v.error.is_none() => {
            v.search_more = true;
            let skip = v.rows.len();
            if !v.loading {
                // jump to the end so the next page continues from there
                v.selected = v.shown.len().saturating_sub(1);
                request(st, id, skip);
            }
            st.ed.info(format!("searching for {needle:?} further down…"));
        }
        None => st.ed.info(format!("no more commits match {needle:?}")),
    }
}

/// The author or path filter typed at the git prompt.
pub fn filter(st: &mut State, then: Then, text: String) {
    match then {
        Then::LogAuthor(id) => {
            if let Some(v) = st.git.views.get_mut(id) {
                v.author = (!text.is_empty()).then_some(text);
                v.refilter();
            }
            more(st, id);
        }
        Then::LogPath(id) => {
            let Some(repo) = st.git.views.get(id).map(|v| v.repo.clone()) else { return };
            let path = text.trim_matches('/').replace('\\', "/");
            open_history(st, repo, path);
        }
        _ => {}
    }
}

/// Opens what the commit did to its changed file `i` in the diff view.
fn open_detail_file(st: &mut State, id: u64, i: usize) {
    let Some(v) = st.git.views.get(id) else { return };
    let Some(d) = &v.details else { return };
    let Some(f) = d.data.as_ref().and_then(|data| data.files.get(i)) else { return };
    let commit = Rev::Commit(d.id);
    let mut spec =
        DiffSpec::file(v.repo.join(&f.path), Rev::Parent(Box::new(commit.clone())), commit).in_repo(v.repo.clone());
    spec.old_path = f.old_path.clone();
    st.open_diff(spec);
}

/// Opens the whole diff of the commit under the cursor.
fn open_commit_diff(st: &mut State, id: u64) {
    let Some(v) = st.git.views.get(id) else { return };
    let Some(cid) = v.current().map(|r| r.commit.id) else { return };
    let spec = DiffSpec::commit(Rev::Commit(cid)).in_repo(v.repo.clone());
    st.open_diff(spec);
}

/// Keys in a log tab. True when taken (everything but chords and `:` is).
pub fn key(st: &mut State, k: KeyPress) -> bool {
    let Some(id) = active_id(st) else { return false };
    if st.git.views.get(id).is_some_and(|v| v.stale && !v.loading) {
        reload(st, id);
    }
    let m = k.mods;
    // typing a search
    if let Some(v) = st.git.views.get_mut(id)
        && let Some(b) = &mut v.search
    {
        match k.key {
            Key::Esc => v.search = None,
            Key::Enter => {
                let needle = b.text.clone();
                v.search = None;
                search(st, id, &needle, false, false);
            }
            Key::Backspace if b.text.is_empty() => v.search = None,
            _ => {
                b.key(k, false);
                let needle = b.text.clone();
                if let Some(i) = v.find(&needle, false, false) {
                    v.selected = i;
                    v.follow = true;
                }
            }
        }
        return true;
    }
    if m.ctrl || m.alt || m.logo {
        let Some(v) = st.git.views.get_mut(id) else { return false };
        let page = v.page.max(2) as isize;
        match k.key {
            Key::Char('d') if m.ctrl && !m.shift => v.move_by(page / 2),
            Key::Char('u') if m.ctrl && !m.shift => v.move_by(-page / 2),
            Key::Char('f') if m.ctrl && !m.shift => v.move_by(page),
            Key::Char('b') if m.ctrl && !m.shift => v.move_by(-page),
            Key::Char('n' | 'j') if m.ctrl && !m.shift => v.move_by(1),
            Key::Char('p' | 'k') if m.ctrl && !m.shift => v.move_by(-1),
            _ => return false,
        }
        more(st, id);
        return true;
    }
    if k.key == Key::Char(':') {
        return false;
    }
    let in_details = st.git.views.get(id).is_some_and(|v| v.in_details && v.details_files() > 0);
    if in_details && details_key(st, id, k) {
        return true;
    }
    let Some(v) = st.git.views.get_mut(id) else { return false };
    let page = v.page.max(2) as isize;
    match k.key {
        Key::Down | Key::Char('j') => v.move_by(1),
        Key::Up | Key::Char('k') => v.move_by(-1),
        Key::Home | Key::Char('g') => v.move_by(isize::MIN / 2),
        Key::End | Key::Char('G') => v.move_by(isize::MAX / 2),
        Key::PageDown => v.move_by(page),
        Key::PageUp => v.move_by(-page),
        Key::Enter | Key::Char(' ') | Key::Char('l') => toggle_details(st, id),
        Key::Tab if v.details_files() > 0 => v.in_details = true,
        Key::Esc => {
            if v.details.take().is_none() {
                v.last_search.clear();
            }
            v.in_details = false;
        }
        Key::Char('/') => v.search = Some(TextBox::default()),
        Key::Char('n' | 'N') => {
            let needle = v.last_search.clone();
            search(st, id, &needle, k.key == Key::Char('N'), true);
        }
        Key::Char('a') => {
            let current = v.author.clone().unwrap_or_default();
            st.git.ask = Some(Ask::text("Filter by author (empty: everyone):", &current, Then::LogAuthor(id)));
        }
        Key::Char('p') => {
            let current = match &v.kind {
                Kind::History { path } => path.clone(),
                Kind::Log { .. } => String::new(),
            };
            st.git.ask = Some(Ask::text("History of the path:", &current, Then::LogPath(id)));
        }
        Key::Char('b') => {
            if let Some(cid) = v.current().map(|r| r.commit.id) {
                st.git.ask = Some(Ask::text(
                    format!("New branch at {}:", cid.short()),
                    "",
                    Then::NewBranch { start: Some(cid.to_string()) },
                ));
            }
        }
        Key::Char('d') => open_commit_diff(st, id),
        Key::Char('y') => {
            if let Some(cid) = v.current().map(|r| r.commit.id.to_string()) {
                st.ed.clipboard.set(cid.clone());
                st.ed.info(format!("copied {cid}"));
            }
        }
        Key::Char('r') | Key::F(5) => reload(st, id),
        Key::Char('q') => {
            st.ed.run(Command::CloseBuffer);
            st.git.views.prune(&st.ed);
            return true;
        }
        _ => {}
    }
    more(st, id);
    true
}

/// Keys in the details' file list. False for those the list leaves to the log.
fn details_key(st: &mut State, id: u64, k: KeyPress) -> bool {
    let n = st.git.views.get(id).map_or(0, View::details_files);
    let Some(d) = st.git.views.get_mut(id).and_then(|v| v.details.as_mut()) else { return false };
    match k.key {
        Key::Down | Key::Char('j') => d.file = (d.file + 1).min(n.saturating_sub(1)),
        Key::Up | Key::Char('k') => d.file = d.file.saturating_sub(1),
        Key::Enter | Key::Char('o') => {
            let i = d.file;
            open_detail_file(st, id, i);
        }
        Key::Tab | Key::Esc => {
            if let Some(v) = st.git.views.get_mut(id) {
                v.in_details = false;
            }
        }
        _ => return false,
    }
    true
}

/// A click in a log tab. True when it was there.
pub fn press(st: &mut State, x: f32, y: f32) -> bool {
    let Some(id) = active_id(st) else { return false };
    let Some(v) = st.git.views.get_mut(id) else { return false };
    if !v.hits.area.is_some_and(|a| a.contains(x, y)) {
        return false;
    }
    if let Some(&(_, i)) = v.hits.files.iter().find(|(r, _)| r.contains(x, y)) {
        if let Some(d) = &mut v.details {
            d.file = i;
        }
        v.in_details = true;
        open_detail_file(st, id, i);
        return true;
    }
    if let Some(&(_, i)) = v.hits.rows.iter().find(|(r, _)| r.contains(x, y)) {
        let now = std::time::Instant::now();
        let double = v.last_click.is_some_and(|(t, j)| j == i && now - t < std::time::Duration::from_millis(400));
        v.last_click = Some((now, i));
        v.selected = i;
        v.in_details = false;
        let cid = v.current().map(|r| r.commit.id);
        // a click shows the commit; a second click (or a double click) on it hides it
        match cid {
            Some(c) if !v.details.as_ref().is_some_and(|d| d.id == c) || double => {
                if double && v.details.as_ref().is_some_and(|d| d.id == c && d.data.is_some()) {
                    return true;
                }
                show_details(st, id, c);
            }
            _ => {}
        }
        more(st, id);
        return true;
    }
    v.in_details = v.hits.details.is_some_and(|d| d.contains(x, y)) && v.details_files() > 0;
    true
}

/// The wheel over a log tab: the list (or the details) scroll.
pub fn wheel(st: &mut State, x: f32, y: f32, lines: f32) -> bool {
    let Some(id) = active_id(st) else { return false };
    let Some(v) = st.git.views.get_mut(id) else { return false };
    if !v.hits.area.is_some_and(|a| a.contains(x, y)) {
        return false;
    }
    let n = lines.round() as isize;
    if n == 0 {
        return true;
    }
    if v.hits.details.is_some_and(|d| d.contains(x, y)) {
        if let Some(d) = &mut v.details {
            d.top = (d.top as isize + n).max(0) as usize;
        }
        return true;
    }
    let max_top = v.shown.len().saturating_sub(v.page) as isize;
    v.top = (v.top as isize + n).clamp(0, max_top.max(0)) as usize;
    // the selection stays on screen
    v.selected = v.selected.clamp(v.top, (v.top + v.page).saturating_sub(1).max(v.top));
    more(st, id);
    true
}

/// Over a log tab: the commit row under the pointer (`Some(None)` elsewhere on the tab). `None` off the tab.
pub fn row_under(st: &State, x: f32, y: f32) -> Option<Option<usize>> {
    let v = active(st)?;
    if !v.hits.area.is_some_and(|a| a.contains(x, y)) {
        return None;
    }
    Some(v.hits.rows.iter().find(|(r, _)| r.contains(x, y)).map(|h| h.1))
}

pub fn hover(st: &State, x: f32, y: f32) -> (bool, bool) {
    let over = active(st).is_some_and(|v| v.hits.rows.iter().chain(&v.hits.files).any(|(r, _)| r.contains(x, y)));
    (false, over)
}

/// The feature is on and the active buffer is a log tab.
pub fn showing(st: &State) -> bool {
    on(&st.ed) && active(st).is_some()
}
