#!/bin/sh
# Installs shrew from GitHub Releases into ~/.local/bin (Linux, macOS), verifying the published sha256. The command is
# `srw` (`srw file.rs`); `shrew` is a symlink to it.
# On Linux it also installs the .desktop entry and icon, so shrew shows up in the app launcher.
#
#   curl -fsSL https://raw.githubusercontent.com/perunok/shrew/main/scripts/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version 0.2.0 --dir /usr/local/bin --no-desktop
#   curl -fsSL .../install.sh | sh -s -- --uninstall
#
# Options (or environment variables):
#   --version X.Y.Z   SHREW_VERSION       release to install (default: the latest)
#   --dir DIR         SHREW_INSTALL_DIR   where the binary goes (default: ~/.local/bin)
#   --no-desktop      SHREW_NO_DESKTOP=1  skip the Linux .desktop entry and icon
#   --uninstall                           remove what this script installed
#                     SHREW_DOWNLOAD_URL  a mirror holding the release assets plus shrew.desktop/.svg/.png
#
# Installed this way, shrew can update itself in place (the status-bar update button), since the binary is yours.
set -eu

REPO="${SHREW_REPO:-perunok/shrew}"
VERSION="${SHREW_VERSION:-latest}"
DIR="${SHREW_INSTALL_DIR:-$HOME/.local/bin}"
DESKTOP=1
[ -n "${SHREW_NO_DESKTOP:-}" ] && DESKTOP=0
UNINSTALL=0

usage() {
  cat <<'USAGE'
Install shrew from GitHub Releases (Linux, macOS).

  install.sh [--version X.Y.Z] [--dir DIR] [--no-desktop]
  install.sh --uninstall [--dir DIR]

  --version X.Y.Z  release to install (default: latest; env SHREW_VERSION)
  --dir DIR        where the binary goes (default: ~/.local/bin; env SHREW_INSTALL_DIR)
  --no-desktop     skip the Linux .desktop entry and icon (env SHREW_NO_DESKTOP=1)
  --uninstall      remove the binary and desktop entry this script installed
USAGE
}
say() { printf 'shrew: %s\n' "$*"; }
die() { printf 'shrew: error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
    --version=*) VERSION="${1#*=}"; shift ;;
    --dir) [ $# -ge 2 ] || die "--dir needs a value"; DIR="$2"; shift 2 ;;
    --dir=*) DIR="${1#*=}"; shift ;;
    --no-desktop) DESKTOP=0; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
done

DATA="${XDG_DATA_HOME:-$HOME/.local/share}"

# ----- uninstall -----

if [ "$UNINSTALL" = 1 ]; then
  rm -f "$DIR/srw" "$DIR/shrew" "$DATA/applications/shrew.desktop" \
    "$DATA/icons/hicolor/scalable/apps/shrew.svg" "$DATA/icons/hicolor/256x256/apps/shrew.png"
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DATA/applications" 2>/dev/null || true
  say "removed $DIR/srw, $DIR/shrew and the desktop entry (settings in ~/.config/shrew are kept)"
  exit 0
fi

# ----- platform -----

os=$(uname -s)
arch=$(uname -m)
case "$arch" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) die "no prebuilt shrew for $arch; build it from source: cargo install --git https://github.com/$REPO shrew" ;;
esac
case "$os" in
  Linux)
    target="$arch-unknown-linux-gnu"
    if [ -e /etc/alpine-release ] || ldd --version 2>&1 | grep -qi musl; then
      say "warning: this looks like a musl system; the prebuilt binary needs glibc"
    fi
    ;;
  Darwin)
    # A shell running under Rosetta reports x86_64 on Apple silicon: install the native build.
    if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
      arch=aarch64
    fi
    target="$arch-apple-darwin"
    DESKTOP=0
    ;;
  MINGW* | MSYS* | CYGWIN*) die "on Windows, download shrew-x86_64-pc-windows-msvc.exe from https://github.com/$REPO/releases" ;;
  *) die "no prebuilt shrew for $os" ;;
esac

# ----- tools -----

if command -v curl >/dev/null 2>&1; then
  # https only (file: for a local mirror)
  fetch() { curl -fsSL --proto '=https,file' --tlsv1.2 --retry 3 -o "$2" "$1"; }
  fetch_text() { curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -H 'Accept: application/vnd.github+json' "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1"; }
  fetch_text() { wget -q -O - --header='Accept: application/vnd.github+json' "$1"; }
else
  die "curl or wget is needed"
fi

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
  die "sha256sum or shasum is needed to verify the download"
fi

# ----- which release -----

if [ -n "${SHREW_DOWNLOAD_URL:-}" ]; then
  tag="$VERSION"
elif [ "$VERSION" = latest ]; then
  tag=$(fetch_text "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null |
    sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1) || tag=""
  [ -n "$tag" ] || die "could not find the latest release of $REPO (no release published yet, or GitHub is unreachable)"
else
  tag="v${VERSION#v}"
fi

asset="shrew-$target"
base="${SHREW_DOWNLOAD_URL:-https://github.com/$REPO/releases/download/$tag}"

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t shrew)
trap 'rm -rf "$tmp"' EXIT
trap 'exit 130' INT TERM

say "downloading shrew $tag for $target"
fetch "$base/$asset" "$tmp/$asset" || die "download failed: $base/$asset"
fetch "$base/$asset.sha256" "$tmp/$asset.sha256" || die "download failed: $base/$asset.sha256"

expected=$(awk -v f="$asset" '{ n = $2; sub(/^\*/, "", n) } n == f { print tolower($1); exit }' "$tmp/$asset.sha256")
[ -n "$expected" ] || expected=$(awk 'NF == 1 { print tolower($1); exit }' "$tmp/$asset.sha256")
actual=$(sha256 "$tmp/$asset")
[ -n "$expected" ] || die "$asset.sha256 has no checksum for $asset"
[ "$expected" = "$actual" ] || die "checksum mismatch for $asset (expected $expected, got $actual); nothing was installed"

# ----- install -----

mkdir -p "$DIR"
chmod 755 "$tmp/$asset"
# Copy next to the target first, then rename: atomic, and safe while an old shrew is running.
cp "$tmp/$asset" "$DIR/.srw.new.$$"
mv -f "$DIR/.srw.new.$$" "$DIR/srw"
# `shrew` is the same program: a symlink (it also replaces the binary of installs from before the rename)
ln -sf srw "$DIR/shrew"
if [ "$os" = Darwin ]; then
  xattr -d com.apple.quarantine "$DIR/srw" 2>/dev/null || true
fi
say "installed $DIR/srw (and $DIR/shrew -> srw)"

if [ "$DESKTOP" = 1 ]; then
  raw="${SHREW_DOWNLOAD_URL:-https://raw.githubusercontent.com/$REPO/$tag/assets}"
  if fetch "$raw/shrew.desktop" "$tmp/shrew.desktop" && fetch "$raw/shrew.svg" "$tmp/shrew.svg"; then
    mkdir -p "$DATA/applications" "$DATA/icons/hicolor/scalable/apps"
    # Launchers often run without ~/.local/bin on PATH: point Exec at the binary itself.
    exe="$DIR/srw"
    case "$exe" in *' '*) exe="\"$exe\"" ;; esac
    # (the desktop files of releases before the rename say Exec=shrew)
    sed -e "s|^Exec=srw|Exec=$exe|" -e "s|^Exec=shrew|Exec=$exe|" -e "s|^TryExec=srw|TryExec=$DIR/srw|" \
      "$tmp/shrew.desktop" > "$DATA/applications/shrew.desktop"
    cp "$tmp/shrew.svg" "$DATA/icons/hicolor/scalable/apps/shrew.svg"
    if fetch "$raw/shrew.png" "$tmp/shrew.png" 2>/dev/null; then
      mkdir -p "$DATA/icons/hicolor/256x256/apps"
      cp "$tmp/shrew.png" "$DATA/icons/hicolor/256x256/apps/shrew.png"
    fi
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DATA/applications" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" 2>/dev/null || true
    say "added shrew to the application menu"
  else
    say "warning: could not fetch the desktop entry; skipped (the editor itself is installed)"
  fi
fi

case ":$PATH:" in
  *":$DIR:"*) say "run: srw [files...]   (or shrew; see srw --help)" ;;
  *)
    say "$DIR is not on your PATH. Add it, e.g. for bash/zsh:"
    printf '\n    export PATH="%s:$PATH"\n\n' "$DIR"
    ;;
esac
