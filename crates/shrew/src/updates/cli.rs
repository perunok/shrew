//! `srw --self-update`: the in-app update without a window, for scripts and the end-to-end test. The same
//! [`Updater`] as the dialog: ask the feed, download the binary for this platform, verify it against the published
//! sha256 and swap it in for this executable. It never restarts anything; a running shrew keeps its old binary
//! until it is restarted.

use shrew_update::{Options, Phase, Updater};
use std::sync::mpsc;
use std::time::Duration;

pub fn run() -> Result<(), String> {
    let current = super::current_version();
    let opts = Options::new(&current);
    shrew_update::cleanup(&current, opts.downloads.as_deref());
    let (tx, rx) = mpsc::channel();
    let u = Updater::with_options(opts, move || {
        let _ = tx.send(());
    });
    let wait = || rx.recv_timeout(Duration::from_secs(120)).map_err(|_| "the updater stopped answering".to_string());

    eprintln!("shrew {current}: asking {} …", shrew_update::Feed::from_env().url);
    u.check();
    let checked = loop {
        if let Some(r) = u.take_checked() {
            break r?;
        }
        wait()?;
    };
    let Some(release) = checked.update else {
        let latest = checked.latest.unwrap_or_else(|| current.clone());
        println!("shrew is up to date ({latest} is the latest release)");
        return Ok(());
    };
    println!("shrew {} is available (you have {current})", release.version);
    let target = u.support().map_err(|e| {
        format!("this installation cannot update itself: {e}. Download the new version from {}", release.page)
    })?;
    u.download(&release);
    let mut shown = None;
    let staged = loop {
        match u.phase() {
            Phase::Downloading { done, total, speed, .. } => {
                let pct = super::dialog::percent(done, total);
                if shown != Some(pct / 10) {
                    shown = Some(pct / 10);
                    let (l, r) = super::dialog::progress_text(done, total, speed);
                    eprintln!("  {l}  {r}");
                }
            }
            Phase::Verifying { .. } => {
                if shown != Some(u8::MAX) {
                    shown = Some(u8::MAX);
                    eprintln!("  verifying the sha256…");
                }
            }
            Phase::Ready(s) => break s,
            Phase::Failed(e) => return Err(e),
            Phase::Idle => return Err("the download stopped".into()),
        }
        wait()?;
    };
    let installed = u.install().map_err(|e| e.to_string())?;
    println!(
        "installed shrew {} to {} (sha256 {}); restart shrew to use it",
        installed.version,
        target.display(),
        staged.sha256_hex()
    );
    Ok(())
}
