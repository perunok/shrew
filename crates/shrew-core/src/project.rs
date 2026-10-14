//! The project a file belongs to: its root folder (found from marker files), the recent files and projects, and a
//! parallel, `.gitignore`-aware walk over the project's files for search and the symbol index.
//!
//! The root is the nearest ancestor holding a `.shrew` marker or a version-control folder (`.git`, `.hg`, `.jj`,
//! `.svn`); failing that a Cargo workspace root, then the nearest `package.json`, `go.mod`, `pyproject.toml` or
//! `Cargo.toml`, then the file's own folder. The home folder and the file system root are never a project: listing
//! them would walk everything you own.

use std::path::{Path, PathBuf};

/// The first of these an ancestor has makes it the root, however far up it is.
pub const STRONG_MARKERS: &[&str] = &[".shrew", ".git", ".hg", ".jj", ".svn"];
/// Without a strong marker, the nearest folder with one of these is the root.
pub const WEAK_MARKERS: &[&str] = &["package.json", "go.mod", "pyproject.toml", "Cargo.toml"];

/// The user's home folder.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// `/`, a drive root or the home folder: far too much to list as a project.
pub fn too_broad(dir: &Path) -> bool {
    dir.parent().is_none() || home().is_some_and(|h| h == dir)
}

/// The project root for a file or folder (see the module docs). `None` when even the folder itself is too broad.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let dir = if start.is_dir() { start } else { start.parent()? };
    let (mut workspace, mut weak) = (None, None);
    for d in dir.ancestors() {
        if too_broad(d) {
            break;
        }
        let has = |m: &str| std::fs::symlink_metadata(d.join(m)).is_ok();
        if STRONG_MARKERS.iter().any(|m| has(m)) {
            return Some(d.to_path_buf());
        }
        if workspace.is_none()
            && has("Cargo.toml")
            && std::fs::read_to_string(d.join("Cargo.toml")).is_ok_and(|t| t.lines().any(|l| l.trim() == "[workspace]"))
        {
            workspace = Some(d.to_path_buf());
        }
        if weak.is_none() && WEAK_MARKERS.iter().any(|m| has(m)) {
            weak = Some(d.to_path_buf());
        }
    }
    workspace.or(weak).or_else(|| (!too_broad(dir)).then(|| dir.to_path_buf()))
}

/// Recently opened files and projects, most recent first. The app keeps them in its cache folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recent {
    pub files: Vec<PathBuf>,
    pub projects: Vec<PathBuf>,
    /// Bumped on every change, so the app knows when to save.
    pub generation: u64,
}

impl Recent {
    pub const MAX_FILES: usize = 200;
    pub const MAX_PROJECTS: usize = 40;

    pub fn add_file(&mut self, path: &Path) {
        if self.files.first().is_some_and(|f| f == path) {
            return;
        }
        front(&mut self.files, path, Self::MAX_FILES);
        self.generation += 1;
    }

    pub fn add_project(&mut self, path: &Path) {
        if self.projects.first().is_some_and(|f| f == path) {
            return;
        }
        front(&mut self.projects, path, Self::MAX_PROJECTS);
        self.generation += 1;
    }

    /// Puts `older` (read from disk) behind what this session added.
    pub fn merge_older(&mut self, older: Recent) {
        for f in older.files {
            if !self.files.contains(&f) && self.files.len() < Self::MAX_FILES {
                self.files.push(f);
            }
        }
        for p in older.projects {
            if !self.projects.contains(&p) && self.projects.len() < Self::MAX_PROJECTS {
                self.projects.push(p);
            }
        }
    }

    /// One `file <path>` or `project <path>` per line.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for p in &self.projects {
            s += &format!("project {}\n", p.display());
        }
        for f in &self.files {
            s += &format!("file {}\n", f.display());
        }
        s
    }

    pub fn parse(text: &str) -> Recent {
        let mut r = Recent::default();
        for line in text.lines() {
            match line.split_once(' ') {
                Some(("file", p)) if !p.is_empty() && r.files.len() < Self::MAX_FILES => r.files.push(p.into()),
                Some(("project", p)) if !p.is_empty() && r.projects.len() < Self::MAX_PROJECTS => {
                    r.projects.push(p.into())
                }
                _ => {}
            }
        }
        r
    }
}

fn front(list: &mut Vec<PathBuf>, path: &Path, max: usize) {
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(max);
}

/// Calls `f` for every file under `root` that `.gitignore` (and hidden-file rules) let through, on `threads`
/// threads, until `f` returns false or `stop()` is true. `globs` (`*.rs`, `!tests/`, comma or space separated)
/// narrow the walk like ripgrep's `-g`. Blocks until the walk is done: run it on a background thread.
#[cfg(feature = "project")]
pub fn walk_files(
    root: &Path,
    globs: &str,
    threads: usize,
    stop: &(dyn Fn() -> bool + Sync),
    f: &(dyn Fn(&Path) -> bool + Sync),
) -> Result<(), String> {
    use ignore::WalkState;
    let mut b = ignore::WalkBuilder::new(root);
    b.hidden(true).git_ignore(true).parents(true).threads(threads.max(1));
    if let Some(ov) = overrides(root, globs)? {
        b.overrides(ov);
    }
    b.build_parallel().run(|| {
        Box::new(|entry| {
            if stop() {
                return WalkState::Quit;
            }
            let Ok(e) = entry else { return WalkState::Continue };
            if !e.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            if f(e.path()) { WalkState::Continue } else { WalkState::Quit }
        })
    });
    Ok(())
}

/// Include/exclude globs (`*.rs`, `!tests/`, comma or space separated) as ignore overrides; `None` when empty.
#[cfg(feature = "project")]
fn overrides(root: &Path, globs: &str) -> Result<Option<ignore::overrides::Override>, String> {
    let globs: Vec<&str> = globs.split([',', ' ']).map(str::trim).filter(|g| !g.is_empty()).collect();
    if globs.is_empty() {
        return Ok(None);
    }
    let mut ov = ignore::overrides::OverrideBuilder::new(root);
    for g in &globs {
        let (neg, body) = g.strip_prefix('!').map_or((false, *g), |b| (true, b));
        let body = body.trim_end_matches('/');
        // a bare folder name (`src`) keeps everything below it
        let folder = !body.contains(['*', '?', '[', '{', '.']);
        let glob = match (neg, folder) {
            (true, _) => format!("!{body}"),
            (false, true) => format!("{body}/**"),
            (false, false) => body.to_string(),
        };
        ov.add(&glob).map_err(|e| format!("{g}: {e}"))?;
    }
    ov.build().map(Some).map_err(|e| e.to_string())
}

/// Whether `globs` are valid (see [`walk_files`]), with what is wrong.
#[cfg(feature = "project")]
pub fn check_globs(root: &Path, globs: &str) -> Result<(), String> {
    overrides(root, globs).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shrew-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn root_detection() {
        let d = temp("root");
        // a git repo with a Cargo workspace inside and a member crate
        std::fs::create_dir_all(d.join("repo/.git")).unwrap();
        std::fs::create_dir_all(d.join("repo/ws/crates/a/src")).unwrap();
        std::fs::write(d.join("repo/ws/Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        std::fs::write(d.join("repo/ws/crates/a/Cargo.toml"), "[package]\n").unwrap();
        let file = d.join("repo/ws/crates/a/src/lib.rs");
        std::fs::write(&file, "").unwrap();
        assert_eq!(find_root(&file), Some(d.join("repo")), "version control wins");
        std::fs::remove_dir_all(d.join("repo/.git")).unwrap();
        assert_eq!(find_root(&file), Some(d.join("repo/ws")), "then the workspace root");
        std::fs::write(d.join("repo/ws/Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(find_root(&file), Some(d.join("repo/ws/crates/a")), "then the nearest manifest");
        // nothing at all: the file's folder
        std::fs::create_dir_all(d.join("plain/sub")).unwrap();
        let note = d.join("plain/sub/note.txt");
        std::fs::write(&note, "").unwrap();
        assert_eq!(find_root(&note), Some(d.join("plain/sub")));
        // a .shrew marker is explicit
        std::fs::write(d.join("plain/.shrew"), "").unwrap();
        assert_eq!(find_root(&note), Some(d.join("plain")));
        assert_eq!(find_root(&d.join("plain/sub")), Some(d.join("plain")), "folders work too");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn home_and_root_are_never_projects() {
        assert!(too_broad(Path::new("/")));
        if let Some(h) = home() {
            assert!(too_broad(&h));
            assert_eq!(find_root(&h.join("some-file-in-home.txt")), None);
        }
    }

    #[test]
    fn recent_lists() {
        let mut r = Recent::default();
        r.add_file(Path::new("/a"));
        r.add_file(Path::new("/b"));
        r.add_file(Path::new("/a"));
        assert_eq!(r.files, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        r.add_project(Path::new("/p"));
        let text = r.to_text();
        let back = Recent::parse(&text);
        assert_eq!((back.files.clone(), back.projects.clone()), (r.files.clone(), r.projects.clone()));
        let mut now = Recent::default();
        now.add_file(Path::new("/c"));
        now.add_file(Path::new("/b"));
        now.merge_older(back);
        assert_eq!(now.files, vec![PathBuf::from("/b"), PathBuf::from("/c"), PathBuf::from("/a")]);
        assert_eq!(now.projects, vec![PathBuf::from("/p")]);
    }

    #[cfg(feature = "project")]
    #[test]
    fn walk_respects_globs() {
        let d = temp("walk");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("docs")).unwrap();
        std::fs::write(d.join("src/a.rs"), "").unwrap();
        std::fs::write(d.join("docs/b.md"), "").unwrap();
        let seen = std::sync::Mutex::new(Vec::new());
        let list = |globs: &str| {
            seen.lock().unwrap().clear();
            walk_files(&d, globs, 2, &|| false, &|p| {
                seen.lock().unwrap().push(p.strip_prefix(&d).unwrap().to_path_buf());
                true
            })
            .unwrap();
            let mut v = seen.lock().unwrap().clone();
            v.sort();
            v
        };
        assert_eq!(list(""), vec![PathBuf::from("docs/b.md"), PathBuf::from("src/a.rs")]);
        assert_eq!(list("*.rs"), vec![PathBuf::from("src/a.rs")]);
        assert_eq!(list("!docs"), vec![PathBuf::from("src/a.rs")]);
        assert_eq!(list("docs"), vec![PathBuf::from("docs/b.md")]);
        std::fs::remove_dir_all(&d).ok();
    }
}
