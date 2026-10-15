//! Git status in the chrome (feature `git_status`), from the status the service keeps fresh (it re-reads it on
//! focus, after saves and git commands, and when HEAD, the index or the refs move):
//!
//! - the status bar and the title bar show the branch (or the commit when detached), `*` when the work tree has
//!   changes, `↑2 ↓1` against the upstream and an operation in progress (MERGING, REBASING…); a click opens the
//!   branch picker (`git_branches`) when this build has it;
//! - the sidebar colours each file by its state with git's letter at the right (M modified, A added, D deleted,
//!   R renamed, ? untracked, U conflict, ! ignored), and tints a folder holding changes, with a dot.

use super::Git;
use crate::colors::Colors;
use crate::layout::Layout;
use crate::state::State;
use shrew_core::Command;
use shrew_core::features::{Feature, Features};
use shrew_git::{Mark, RepoState, Status};
use shrew_render::{Color, Rect, Scene, TextStyle, columns};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
pub struct StatusUi {
    /// Bumped by every new status: the sidebar's folder marks are rebuilt from it.
    generation: u64,
    tree: RefCell<Option<TreeCache>>,
}

impl StatusUi {
    pub fn invalidate(&mut self) {
        self.generation += 1;
    }
}

/// The sidebar's folder marks for one project root and status.
struct TreeCache {
    generation: u64,
    root: PathBuf,
    repo: PathBuf,
    /// The project root inside the repository (`""` at its top, `crates/app`, ...).
    prefix: String,
    status: Arc<Status>,
    /// Every folder holding a change, with its most notable mark.
    dirs: Arc<HashMap<String, Mark>>,
}

/// What a repository's branch looks like.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub label: String,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    /// The upstream branch was deleted.
    pub gone: bool,
    pub upstream: Option<String>,
    pub state: RepoState,
}

impl Branch {
    /// "main*".
    pub fn name(&self) -> String {
        format!("{}{}", self.label, if self.dirty { "*" } else { "" })
    }

    /// "↑2 ↓1" (empty when level with the upstream).
    pub fn sync(&self) -> String {
        let mut s = String::new();
        if self.ahead > 0 {
            s += &format!("↑{}", count(self.ahead));
        }
        if self.behind > 0 {
            if !s.is_empty() {
                s.push(' ');
            }
            s += &format!("↓{}", count(self.behind));
        }
        s
    }

    pub fn state_label(&self) -> Option<&'static str> {
        Some(match self.state {
            RepoState::Clean => return None,
            RepoState::Merging => "MERGING",
            RepoState::Rebasing => "REBASING",
            RepoState::CherryPicking => "CHERRY-PICKING",
            RepoState::Reverting => "REVERTING",
            RepoState::Bisecting => "BISECTING",
            RepoState::ApplyingPatches => "APPLYING",
        })
    }

    /// One line for the message bar.
    fn summary(&self) -> String {
        let mut s = format!("on {}", self.label);
        if let Some(u) = &self.upstream {
            s += &match (self.gone, self.ahead, self.behind) {
                (true, ..) => format!(", {u} is gone"),
                (_, 0, 0) => format!(", level with {u}"),
                (_, a, 0) => format!(", {} ahead of {u}", count(a)),
                (_, 0, b) => format!(", {} behind {u}", count(b)),
                (_, a, b) => format!(", {} ahead and {} behind {u}", count(a), count(b)),
            };
        }
        if self.dirty {
            s += ", uncommitted changes";
        }
        if let Some(op) = self.state_label() {
            s += &format!(" ({})", op.to_lowercase());
        }
        s
    }
}

fn count(n: u32) -> String {
    if n >= shrew_git::AHEAD_BEHIND_CAP { format!("{n}+") } else { n.to_string() }
}

/// The repository the chrome speaks of: the active file's, else the sidebar project's.
fn repo(st: &State) -> Option<PathBuf> {
    let b = st.ed.buf();
    if let Some(super::Place::In { repo, .. }) = b.doc.path.as_deref().and_then(|p| st.git.file_place(p)) {
        return Some(repo.clone());
    }
    let root = st.ed.tree.as_ref().filter(|_| st.ed.sidebar_on())?.root.clone();
    st.git.known_dir_repo(&root)
}

/// The branch to show, when the feature is on and its repository's status is in.
pub fn branch(st: &State) -> Option<Branch> {
    if !st.ed.features.on(Feature::GitStatus) {
        return None;
    }
    let s = st.git.status_of(&repo(st)?)?;
    let h = &s.head;
    let up = h.upstream.as_ref();
    Some(Branch {
        label: h.label(),
        dirty: !s.is_clean(),
        ahead: up.map_or(0, |u| u.ahead),
        behind: up.map_or(0, |u| u.behind),
        gone: up.is_some_and(|u| u.gone),
        upstream: up.map(|u| u.name.clone()),
        state: h.state,
    })
}

/// A click on the branch: the branch picker when this build has one, else a summary on the message line.
pub fn branch_clicked(st: &mut State) {
    if let Some(cmd) = Command::from_name("git_branches") {
        st.ed.run(cmd);
        st.after_input();
    } else if let Some(b) = branch(st) {
        st.ed.info(b.summary());
    }
}

/// A small branch glyph (two commits and a fork), drawn from dots so it looks the same in every font.
pub fn icon(scene: &mut Scene, x: f32, y: f32, ch: f32, scale: f32, col: Color) {
    let d = (ch * 0.22).round().max(3.0);
    let t = scale.round().max(1.0);
    let (x0, x1) = ((x + d / 2.0).round(), (x + d * 1.9).round());
    let (top, bottom) = ((y + ch * 0.2).round(), (y + ch * 0.8 - d).round());
    let dot = |scene: &mut Scene, cx: f32, cy: f32| {
        scene.rounded_rect(Rect::new(cx - d / 2.0, cy, d, d), d / 2.0, Color::TRANSPARENT, Some((t, col)));
    };
    dot(scene, x0, top);
    dot(scene, x0, bottom);
    dot(scene, x1, top);
    // the trunk, and the branch curving into it
    scene.rect(Rect::new((x0 - t / 2.0).round(), top + d, t, bottom - top - d), col);
    let (ax, ay) = (x1, top + d);
    let (bx, by) = (x0, bottom - d * 0.4);
    let n = ((by - ay).max(ax - bx) / (0.5 * t)).ceil().max(2.0) as usize;
    for k in 0..=n {
        let s = k as f32 / n as f32;
        // a quarter curve: down first, then across
        let px = ax + (bx - ax) * (s * s);
        let py = ay + (by - ay) * s;
        scene.rect(Rect::new((px - t / 2.0).round(), py.round(), t, t), col);
    }
}

/// Columns the icon takes (with its gap).
pub const ICON_COLS: f32 = 2.6;

/// The branch item in the status bar, laid out leftwards from `right` (which moves past it).
pub fn draw_status(
    scene: &mut Scene,
    st: &State,
    hits: &mut super::Hits,
    lay: &Layout,
    c: &Colors,
    right: &mut f32,
    y: f32,
) {
    let Some(b) = branch(st) else { return };
    let cw = lay.cw;
    let name = b.name();
    let sync = b.sync();
    let state = b.state_label();
    let cols = columns(&name) as f32
        + if sync.is_empty() { 0.0 } else { columns(&sync) as f32 + 1.0 }
        + state.map_or(0.0, |s| columns(s) as f32 + 1.0)
        + ICON_COLS;
    let w = cols * cw;
    let x0 = *right - w;
    let bar = lay.status;
    hits.branch = Some(Rect::new((x0 - cw / 2.0).round(), bar.y, (w + cw).round(), bar.h));
    icon(scene, x0, y, lay.ch, lay.scale, c.panel_muted);
    let mut x = x0 + ICON_COLS * cw;
    scene.text(x.round(), y, &name, TextStyle::new(c.panel_foreground), &[]);
    x += (columns(&name) + 1) as f32 * cw;
    if !sync.is_empty() {
        scene.text(x.round(), y, &sync, TextStyle::new(c.panel_muted), &[]);
        x += (columns(&sync) + 1) as f32 * cw;
    }
    if let Some(s) = state {
        scene.text(x.round(), y, s, TextStyle::new(c.warning).bold(), &[]);
    }
    *right = x0 - 2.0 * cw;
}

/// The title bar's branch item: its label (`None`: not shown).
pub fn title_label(st: &State) -> Option<String> {
    branch(st).map(|b| b.name())
}

/// Draws the title bar's branch item in `r`.
pub fn draw_title(scene: &mut Scene, st: &State, r: Rect, lay: &Layout, c: &Colors, ink: Color) {
    let Some(label) = title_label(st) else { return };
    let y = lay.centered(r.y, r.h);
    let x = r.x + cw_pad(lay);
    icon(scene, x, y, lay.ch, lay.scale, ink);
    scene.text((x + ICON_COLS * lay.cw).round(), y, &label, TextStyle::new(c.panel_foreground), &[]);
}

fn cw_pad(lay: &Layout) -> f32 {
    (lay.cw * 0.75).round()
}

/// The title bar item's width for its label.
pub fn title_width(label: &str, lay: &Layout) -> f32 {
    (2.0 * cw_pad(lay) + (ICON_COLS + columns(label) as f32) * lay.cw).round()
}

// ----- the sidebar -----

/// File states for the sidebar's rows, for one frame.
pub struct TreeDeco {
    root: PathBuf,
    prefix: String,
    status: Arc<Status>,
    dirs: Arc<HashMap<String, Mark>>,
}

/// A row's state: its colour and letter (`•` for a folder holding changes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowMark {
    pub color: Color,
    pub letter: &'static str,
}

/// The sidebar's decorations, when the feature is on and the project's repository has a status.
pub fn tree_deco(git: &Git, features: &Features, root: &Path) -> Option<TreeDeco> {
    if !features.on(Feature::GitStatus) {
        return None;
    }
    let repo = git.known_dir_repo(root)?;
    let status = git.status_of(&repo)?.clone();
    let ui = &git.status_ui;
    let mut cache = ui.tree.borrow_mut();
    let fresh = cache.as_ref().is_some_and(|t| t.generation == ui.generation && t.root == root && t.repo == repo);
    if !fresh {
        let canon = shrew_git::canonicalize(root).ok()?;
        let prefix = canon
            .strip_prefix(&repo)
            .ok()?
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        *cache = Some(TreeCache {
            generation: ui.generation,
            root: root.to_path_buf(),
            repo,
            prefix,
            dirs: Arc::new(folder_marks(&status)),
            status,
        });
    }
    let t = cache.as_ref()?;
    Some(TreeDeco { root: t.root.clone(), prefix: t.prefix.clone(), status: t.status.clone(), dirs: t.dirs.clone() })
}

/// Every folder above a change, with the most notable mark under it (ignored files do not count).
pub fn folder_marks(status: &Status) -> HashMap<String, Mark> {
    let mut dirs: HashMap<String, Mark> = HashMap::new();
    for e in &status.entries {
        let m = e.mark();
        if m == Mark::Ignored {
            continue;
        }
        let path = e.path.trim_end_matches('/');
        // an untracked folder listed whole marks itself too
        let mut upto: Vec<usize> = path.match_indices('/').map(|(i, _)| i).collect();
        if e.path.ends_with('/') {
            upto.push(path.len());
        }
        for i in upto {
            let slot = dirs.entry(path[..i].to_string()).or_insert(m);
            *slot = (*slot).max(m);
        }
    }
    dirs
}

/// Colour and letter of a mark.
pub fn mark_style(m: Mark, dir: bool, c: &Colors) -> RowMark {
    let (color, letter) = match m {
        Mark::Ignored => (c.git_ignored, "!"),
        Mark::Untracked => (c.git_untracked, "?"),
        Mark::Added => (c.git_added, "A"),
        Mark::Renamed => (c.git_renamed, "R"),
        Mark::Modified => (c.git_modified, "M"),
        Mark::Deleted => (c.git_deleted, "D"),
        Mark::Conflict => (c.git_conflict, "U"),
    };
    RowMark { color, letter: if dir && m != Mark::Ignored { "•" } else { letter } }
}

impl TreeDeco {
    /// The repository path of a row (`None` outside the project root).
    fn rel(&self, path: &Path) -> Option<String> {
        let inner = path.strip_prefix(&self.root).ok()?;
        let parts: Vec<String> = inner.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let inner = parts.join("/");
        Some(match (self.prefix.is_empty(), inner.is_empty()) {
            (true, _) => inner,
            (false, true) => self.prefix.clone(),
            (false, false) => format!("{}/{inner}", self.prefix),
        })
    }

    /// A row's mark: its own state (a file inside an untracked or ignored folder takes the folder's), or for a
    /// folder the most notable state under it; `ignored` is the tree's own `.gitignore` check.
    pub fn row(&self, path: &Path, dir: bool, ignored: bool, c: &Colors) -> Option<RowMark> {
        let rel = self.rel(path)?;
        let own = if dir {
            self.dirs.get(&rel).copied().or_else(|| self.status.get(&format!("{rel}/")).map(|e| e.mark()))
        } else {
            self.status.get(&rel).map(|e| e.mark())
        };
        let m = own.or(ignored.then_some(Mark::Ignored))?;
        Some(mark_style(m, dir, c))
    }
}

/// Draws a row's letter right-aligned in `row`; returns where the name must end.
pub fn draw_row_mark(scene: &mut Scene, m: RowMark, row: Rect, y: f32, lay: &Layout) -> f32 {
    let x = row.right() - 3.0 * lay.pad - lay.cw;
    scene.text(x.round(), y, m.letter, TextStyle::new(m.color).bold(), &[]);
    x - lay.cw * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use shrew_git::{FileState, StatusEntry};

    fn entry(path: &str, f: impl FnOnce(&mut StatusEntry)) -> StatusEntry {
        let mut e = StatusEntry { path: path.into(), ..Default::default() };
        f(&mut e);
        e
    }

    #[test]
    fn folders_roll_up_the_most_notable_change() {
        let status = Status {
            entries: vec![
                entry("build/", |e| e.ignored = true),
                entry("docs/new/", |e| e.untracked = true),
                entry("src/a.rs", |e| e.unstaged = Some(FileState::Modified)),
                entry("src/deep/b.rs", |e| e.staged = Some(FileState::Added)),
                entry("src/deep/c.rs", |e| e.conflict = Some(shrew_git::ConflictKind::BothModified)),
            ],
            ..Default::default()
        };
        let d = folder_marks(&status);
        assert_eq!(d.get("src"), Some(&Mark::Conflict));
        assert_eq!(d.get("src/deep"), Some(&Mark::Conflict));
        assert_eq!(d.get("docs"), Some(&Mark::Untracked));
        assert_eq!(d.get("docs/new"), Some(&Mark::Untracked), "an untracked folder marks itself");
        assert_eq!(d.get("build"), None, "ignored files do not tint folders");
        assert_eq!(d.len(), 4);
    }

    #[test]
    fn rows_find_their_state_under_a_sub_project() {
        let status = Status {
            entries: vec![
                entry("app/src/main.rs", |e| e.unstaged = Some(FileState::Modified)),
                entry("app/tmp/", |e| e.untracked = true),
            ],
            ..Default::default()
        };
        let c = Colors::from_theme(&shrew_theme::Theme::default());
        let deco = TreeDeco {
            root: PathBuf::from("/w/app"),
            prefix: "app".into(),
            dirs: Arc::new(folder_marks(&status)),
            status: Arc::new(status),
        };
        let m = deco.row(Path::new("/w/app/src/main.rs"), false, false, &c).unwrap();
        assert_eq!((m.letter, m.color), ("M", c.git_modified));
        assert_eq!(deco.row(Path::new("/w/app/src"), true, false, &c).unwrap().letter, "•");
        assert_eq!(
            deco.row(Path::new("/w/app/tmp/x.txt"), false, false, &c).unwrap().letter,
            "?",
            "inside an untracked folder"
        );
        assert_eq!(deco.row(Path::new("/w/app/README"), false, false, &c), None);
        assert_eq!(deco.row(Path::new("/w/app/target"), true, true, &c).unwrap().letter, "!");
        assert_eq!(deco.row(Path::new("/elsewhere"), false, false, &c), None);
    }

    #[test]
    fn branch_labels() {
        let b = Branch {
            label: "main".into(),
            dirty: true,
            ahead: 2,
            behind: 0,
            gone: false,
            upstream: Some("origin/main".into()),
            state: RepoState::Merging,
        };
        assert_eq!(b.name(), "main*");
        assert_eq!(b.sync(), "↑2");
        assert_eq!(b.summary(), "on main, 2 ahead of origin/main, uncommitted changes (merging)");
        let b = Branch { dirty: false, ahead: 1, behind: shrew_git::AHEAD_BEHIND_CAP, state: RepoState::Clean, ..b };
        assert_eq!(b.sync(), "↑1 ↓10000+");
        assert_eq!(b.state_label(), None);
    }
}
