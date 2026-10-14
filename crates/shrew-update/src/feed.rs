//! The release feed: GitHub's "latest release" REST endpoint (or any URL serving the same JSON).

use serde::Deserialize;

use crate::{Error, REPO, Result, TARGET, asset_name, checksum_name, net, version};

/// A published release newer than the running build, with the assets for this target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Semver without the `v`: `0.2.0`.
    pub version: String,
    /// The git tag: `v0.2.0`.
    pub tag: String,
    /// Release notes (GitHub markdown), possibly empty.
    pub notes: String,
    /// Publication time, RFC 3339 (`2026-10-14T12:00:00Z`), possibly empty.
    pub published: String,
    /// Download URL of the binary for this target.
    pub asset_url: String,
    /// Download URL of its `sha256sum`-format checksum file.
    pub checksum_url: String,
    /// Binary size in bytes as listed by the feed (0 when unknown).
    pub size: u64,
}

/// Where releases come from and which target's assets to pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feed {
    pub url: String,
    pub target: String,
}

impl Feed {
    /// The latest-release endpoint of GitHub repo `owner/name`, for this build's [`TARGET`].
    pub fn github(repo: &str) -> Self {
        Feed { url: format!("https://api.github.com/repos/{repo}/releases/latest"), target: TARGET.into() }
    }

    /// `SHREW_UPDATE_FEED_URL` if set, else [`Feed::github`] of `SHREW_UPDATE_REPO` (default [`REPO`]).
    pub fn from_env() -> Self {
        let var = |key| std::env::var(key).ok().filter(|v: &String| !v.trim().is_empty());
        match var("SHREW_UPDATE_FEED_URL") {
            Some(url) => Feed { url, target: TARGET.into() },
            None => Feed::github(var("SHREW_UPDATE_REPO").as_deref().unwrap_or(REPO)),
        }
    }

    /// Fetches the feed and returns the release to update to, if it is newer than `current` and ships a binary and
    /// checksum for this target. A missing feed (404: no releases published yet) is `Ok(None)`, not an error.
    pub fn check(&self, current: &str) -> Result<Option<Release>> {
        if version::parse(current).is_none() {
            return Err(Error::Feed(format!("current version {current:?} is not semver")));
        }
        let Some(json) = net::get_text(&self.url, "application/vnd.github+json", 8 << 20)? else {
            log::debug!("update: no releases at {}", self.url);
            return Ok(None);
        };
        parse_feed(&json, current, &self.target, self.url.starts_with("https://"))
    }
}

/// Checks the default feed ([`Feed::from_env`]) for a release newer than `current` (pass `env!("CARGO_PKG_VERSION")`).
/// Blocking: call it off the UI thread, or use [`crate::Updater`].
pub fn check(current: &str) -> Result<Option<Release>> {
    Feed::from_env().check(current)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Payload {
    One(GhRelease),
    // The "list releases" endpoint works too: handy for feeds that also carry older releases.
    Many(Vec<GhRelease>),
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Picks the newest stable release newer than `current` that has both assets for `target`.
/// With `https_only`, assets served over plain http are ignored (no downgrade from an https feed).
pub(crate) fn parse_feed(json: &str, current: &str, target: &str, https_only: bool) -> Result<Option<Release>> {
    let releases = match serde_json::from_str(json) {
        Ok(Payload::One(release)) => vec![release],
        Ok(Payload::Many(releases)) => releases,
        Err(err) => return Err(Error::Feed(format!("unexpected release feed: {err}"))),
    };
    let binary = asset_name(target);
    let checksum = checksum_name(target);
    let best = releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease && version::is_newer(&r.tag_name, current))
        .filter_map(|r| {
            let url_ok = |a: &&GhAsset| !https_only || a.browser_download_url.starts_with("https://");
            let bin = r.assets.iter().filter(url_ok).find(|a| a.name == binary);
            let sum = r.assets.iter().filter(url_ok).find(|a| a.name == checksum);
            if bin.is_none() || sum.is_none() {
                log::warn!("update: release {} has no {binary} + {checksum}", r.tag_name);
            }
            let (bin, sum) = (bin?, sum?);
            let version = version::parse(&r.tag_name)?;
            Some((
                version.clone(),
                Release {
                    version: version.to_string(),
                    notes: r.body.clone().unwrap_or_default(),
                    published: r.published_at.clone().unwrap_or_default(),
                    asset_url: bin.browser_download_url.clone(),
                    checksum_url: sum.browser_download_url.clone(),
                    size: bin.size,
                    tag: r.tag_name,
                },
            ))
        })
        .max_by(|a, b| a.0.cmp_precedence(&b.0));
    Ok(best.map(|(_, release)| release))
}
