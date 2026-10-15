//! Temporary repositories built with the real `git`. Every test returns early (passing) when git is missing.
#![allow(dead_code)]

use shrew_git::Repo;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct TestRepo {
    pub dir: tempfile::TempDir,
}

/// `git` with a fixed identity and no user or system config, so results do not depend on the machine.
pub fn git_in(dir: &Path, args: &[&str]) -> String {
    git_in_env(dir, args, &[])
}

/// [`git_in`] with extra environment variables (commit dates).
pub fn git_in_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .envs(env.iter().copied())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_config())
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Like [`git_in`] but returns success and output instead of panicking.
pub fn try_git_in(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_config())
        .env("LC_ALL", "C")
        .output()
        .expect("run git");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

fn null_config() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

impl TestRepo {
    /// A fresh repository on branch `main` with local identity and settings (our library's own `git` runs read
    /// the repository config, so identity must live there).
    pub fn new() -> Option<TestRepo> {
        if !shrew_git::git_available() {
            eprintln!("git not found: skipping");
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        git_in(dir.path(), &["init", "-q", "-b", "main"]);
        let t = TestRepo { dir };
        t.configure();
        Some(t)
    }

    pub fn configure(&self) {
        for (k, v) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
            ("tag.gpgsign", "false"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", ".git/hooks"),
            ("init.defaultBranch", "main"),
            ("pull.rebase", "false"),
        ] {
            self.git(&["config", k, v]);
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn git(&self, args: &[&str]) -> String {
        git_in(self.path(), args)
    }

    pub fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        let p = self.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        String::from_utf8(std::fs::read(self.path().join(rel)).unwrap()).unwrap()
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        self.path().join(rel)
    }

    /// Commits everything. Each commit gets its own date, a minute after the previous one in this process, so
    /// date order (and "newest first") is unambiguous even when the machine commits several in one second.
    pub fn commit_all(&self, msg: &str) -> String {
        static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let when = format!("{} +0000", 1_700_000_000 + 60 * CLOCK.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
        self.git(&["add", "-A"]);
        git_in_env(
            self.path(),
            &["commit", "-q", "--allow-empty", "-m", msg],
            &[("GIT_AUTHOR_DATE", &when), ("GIT_COMMITTER_DATE", &when)],
        );
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    pub fn repo(&self) -> Repo {
        Repo::discover(self.path()).unwrap().expect("a repository")
    }

    /// The staged content of `rel`.
    pub fn staged(&self, rel: &str) -> String {
        self.git(&["show", &format!(":{rel}")])
    }

    /// `git status --porcelain=v1 -z`-like short lines, sorted, for comparing with our status.
    pub fn short_status(&self, extra: &[&str]) -> Vec<String> {
        let mut args = vec!["status", "--porcelain=v1", "--no-renames"];
        args.extend_from_slice(extra);
        let mut v: Vec<String> = self.git(&args).lines().map(str::to_string).collect();
        v.sort();
        v
    }
}

#[allow(unused_macros)]
macro_rules! repo_or_skip {
    () => {
        match common::TestRepo::new() {
            Some(r) => r,
            None => return,
        }
    };
}
#[allow(unused_imports)]
pub(crate) use repo_or_skip;
