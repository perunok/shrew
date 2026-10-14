//! Manual end-to-end check: `cargo run -p shrew-update --example check -- [owner/repo] [current-version]`.
//! Prints what the updater would offer; never installs anything.

fn main() {
    let mut args = std::env::args().skip(1);
    let repo = args.next().unwrap_or_else(|| shrew_update::REPO.into());
    let current = args.next().unwrap_or_else(|| env!("CARGO_PKG_VERSION").into());
    let feed = shrew_update::Feed::github(&repo);
    let start = std::time::Instant::now();
    match feed.check(&current) {
        Ok(Some(release)) => {
            println!("update {current} -> {} ({} bytes): {}", release.tag, release.size, release.asset_url)
        }
        Ok(None) => println!("{repo}: nothing newer than {current} for {}", feed.target),
        Err(err) => println!("{repo}: error: {err}"),
    }
    println!("took {:?}", start.elapsed());
}
