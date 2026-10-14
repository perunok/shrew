//! Recent files and projects (File: Open Recent…, Ctrl+Alt+P) in `<cache dir>/shrew/recent`, one `file <path>` or
//! `project <path>` per line. Read once the first frame is up, written when the lists change; never touched by
//! `--screenshot` or with the `recent` feature off.

use shrew_core::project::Recent;
use std::io;
use std::path::{Path, PathBuf};

pub fn path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("shrew").join("recent"))
}

pub fn load(path: &Path) -> Recent {
    std::fs::read_to_string(path).map(|t| Recent::parse(&t)).unwrap_or_default()
}

/// Writes the lists (to a temporary file first, so a crash never leaves half of one).
pub fn save(path: &Path, recent: &Recent) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, recent.to_text())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let d = std::env::temp_dir().join(format!("shrew-recent-{}", std::process::id()));
        let p = d.join("cache/recent");
        let mut r = Recent::default();
        r.add_file(Path::new("/x/a.rs"));
        r.add_project(Path::new("/x"));
        save(&p, &r).unwrap();
        let back = load(&p);
        assert_eq!((back.files, back.projects), (r.files, r.projects));
        assert_eq!(load(&d.join("missing")), Recent::default());
        std::fs::remove_dir_all(&d).ok();
    }
}
