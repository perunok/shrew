//! Version parsing and comparison: tags are `vX.Y.Z` semver, prereleases never count as an update.

use semver::Version;

/// Parses `1.2.3` or `v1.2.3` (surrounding whitespace allowed).
pub(crate) fn parse(tag: &str) -> Option<Version> {
    let tag = tag.trim();
    Version::parse(tag.strip_prefix(['v', 'V']).unwrap_or(tag)).ok()
}

/// Whether release tag `candidate` is a stable version newer than `current`. Unparsable input is never newer.
/// Build metadata (`+abc`) is ignored, as semver precedence says.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(new), Some(cur)) => new.pre.is_empty() && new.cmp_precedence(&cur).is_gt(),
        _ => false,
    }
}
