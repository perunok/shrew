#!/usr/bin/env bash
# Fails if any crate shrew builds (any target, normal or build dependency) is known to compile C/C++ or link a
# C library at build time. shrew must stay pure Rust: OS libraries are only ever loaded at runtime (dlopen).
#
#   scripts/check-pure-rust.sh            # whole workspace, every target platform
#   scripts/check-pure-rust.sh -p shrew   # one package
set -euo pipefail

cd "$(dirname "$0")/.."

# Build helpers that compile C, and -sys crates that compile or link a C library.
deny='^(cc|cmake|bindgen|autotools|nasm-rs|ring|aws-lc-rs|aws-lc-sys|aws-lc-fips-sys|openssl|openssl-sys|onig|onig_sys|tree-sitter|tree-sitter-[a-z0-9_-]+|freetype|freetype-sys|freetype-rs|servo-fontconfig|servo-fontconfig-sys|yeslogic-fontconfig-sys|fontconfig-sys|harfbuzz-sys|harfbuzz_rs|libz-sys|zstd-sys|bzip2-sys|lzma-sys|libgit2-sys|libssh2-sys|curl-sys|sqlite3-src|libsqlite3-sys|xcb|x11|wayland-sys-static|glfw-sys|sdl2-sys|shaderc-sys|spirv-cross-sys|mozjpeg-sys|libwebp-sys|png-sys|xkbcommon-sys|libudev-sys|alsa-sys|dbus|libdbus-sys)$'

scope=(--workspace)
if [ "$#" -gt 0 ]; then scope=("$@"); fi

tree=$(cargo tree "${scope[@]}" --target all -e normal,build --prefix none --format '{p}' |
  sed 's/ (.*//; s/ (\*)$//' | sort -u)

bad=$(printf '%s\n' "$tree" | awk '{print $1}' | grep -E "$deny" || true)

# `cc` is tolerated only as an inert build dependency: wayland-backend compiles its C log shim only with its "log"
# feature (we never enable it), and android-activity is only built for Android targets.
if printf '%s\n' "$bad" | grep -qx cc; then
  users=$(cargo tree "${scope[@]}" --target all -e normal,build -i cc --depth 1 --prefix none --format '{p}' 2>/dev/null |
    sed 's/ (.*//' | awk '{print $1}' | grep -vx cc | sort -u)
  unexpected=$(printf '%s\n' "$users" | grep -vxE 'wayland-backend|android-activity' || true)
  log_on=$(cargo tree "${scope[@]}" --target all -e features -i wayland-backend 2>/dev/null | grep -c 'wayland-backend feature "log"' || true)
  if [ -z "$unexpected" ] && [ "$log_on" = "0" ]; then
    bad=$(printf '%s\n' "$bad" | grep -vx cc || true)
  fi
fi
if [ -n "$bad" ]; then
  echo "error: these crates compile or link C/C++ (shrew must stay pure Rust):"
  printf '  %s\n' $bad
  echo "Find who pulls one in with: cargo tree --workspace --target all -e normal,build -i <crate>"
  exit 1
fi

# wayland-sys is fine only with "dlopen" (otherwise its build script links libwayland via pkg-config).
if printf '%s\n' "$tree" | grep -q '^wayland-sys '; then
  if ! cargo tree "${scope[@]}" --target all -e features -i wayland-sys 2>/dev/null | grep -q 'wayland-sys feature "dlopen"'; then
    echo 'error: wayland-sys is built without its "dlopen" feature, so it links libwayland at build time'
    exit 1
  fi
fi

echo "pure Rust: $(printf '%s\n' "$tree" | wc -l) crates checked, none compiles or links C"
