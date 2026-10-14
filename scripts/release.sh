#!/usr/bin/env bash
# Cuts a release locally: bumps [workspace.package] version in Cargo.toml (and Cargo.lock), moves CHANGELOG.md's
# "Unreleased" notes under the new version, commits and tags vX.Y.Z. It never pushes: it prints the command, and
# pushing the tag starts .github/workflows/release.yml, which drafts the GitHub release.
#
#   scripts/release.sh 0.2.0        # or: patch | minor | major
#   scripts/release.sh minor --no-verify   # skip fmt/clippy/tests
set -euo pipefail

cd "$(dirname "$0")/.."

die() { echo "release: error: $*" >&2; exit 1; }

[ $# -ge 1 ] || die "usage: scripts/release.sh <X.Y.Z | patch | minor | major> [--no-verify]"
bump="$1"
verify=1
[ "${2:-}" = --no-verify ] && verify=0

current=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\([^"]*\)".*/\1/p}' Cargo.toml)
[[ "$current" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || die "current version '$current' in Cargo.toml is not X.Y.Z"
major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]}

case "$bump" in
  patch) next="$major.$minor.$((patch + 1))" ;;
  minor) next="$major.$((minor + 1)).0" ;;
  major) next="$((major + 1)).0.0" ;;
  *) next="${bump#v}" ;;
esac
[[ "$next" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "'$next' is not a version (X.Y.Z)"
tag="v$next"

git rev-parse --git-dir >/dev/null 2>&1 || die "not a git repository"
[ -z "$(git status --porcelain)" ] || die "the working tree has uncommitted changes; commit or stash them first"
! git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "tag $tag already exists"
branch=$(git rev-parse --abbrev-ref HEAD)
[ "$branch" = main ] || echo "release: note: releasing from branch '$branch', not main"

if [ "$verify" = 1 ]; then
  echo "release: checking fmt, clippy and tests (skip with --no-verify)"
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --locked --quiet
  cargo test --workspace --locked --quiet
fi

if [ "$next" = "$current" ]; then
  # First release of the version already in Cargo.toml (e.g. v0.1.0): tag it as is.
  echo "release: tagging the current version $next"
else
  echo "release: $current -> $next"
fi

# Cargo.toml: only the version line inside [workspace.package].
awk -v v="$next" '
  /^\[/ { section = $0 }
  section == "[workspace.package]" && /^version *=/ && !done { print "version = \"" v "\""; done = 1; next }
  { print }
' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

# Cargo.lock carries the workspace crates' versions too; refresh just those.
cargo update --workspace --offline --quiet 2>/dev/null || cargo update --workspace --quiet

# CHANGELOG.md (Keep a Changelog): "## [Unreleased]" gets a fresh empty section above the new version's heading.
if [ -f CHANGELOG.md ]; then
  grep -q '^## \[Unreleased\]' CHANGELOG.md || die "CHANGELOG.md has no '## [Unreleased]' heading"
  notes=$(awk '/^## \[Unreleased\]/ { on = 1; next } /^## / { on = 0 } on && NF' CHANGELOG.md)
  [ -n "$notes" ] || echo "release: warning: the Unreleased section of CHANGELOG.md is empty"
  repo_url=$(git remote get-url origin 2>/dev/null | sed -E 's#^git@github.com:#https://github.com/#; s#\.git$##')
  # The new version's link compares with the newest earlier tag, or points at its release if it is the first.
  prev=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)
  awk -v v="$next" -v d="$(date +%Y-%m-%d)" -v prev="$prev" -v url="$repo_url" '
    /^## \[Unreleased\]/ { print; print ""; print "## [" v "] - " d; next }
    /^\[Unreleased\]:/ && url != "" {
      print "[Unreleased]: " url "/compare/v" v "...HEAD"
      if (prev != "") print "[" v "]: " url "/compare/" prev "...v" v
      else print "[" v "]: " url "/releases/tag/v" v
      next
    }
    { print }
  ' CHANGELOG.md > CHANGELOG.md.tmp && mv CHANGELOG.md.tmp CHANGELOG.md
fi

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -q -m "Release $tag"
git tag -a "$tag" -m "shrew $tag"

echo
echo "release: committed and tagged $tag. Push both to build and draft the release:"
echo
echo "    git push --atomic origin $branch $tag"
echo
echo "Then review the draft on GitHub and press Publish; running shrews offer the update once it is published."
