#!/usr/bin/env bash
# Regenerates every screenshot in the docs: the README hero (docs/screenshot.png), the README gallery and the theme
# gallery in docs/THEMES.md (docs/screenshots/*.png). Each one is the real editor rendered headless with
# `srw --screenshot`, after typing the keys shown, on shrew's own source files. Headless runs use the built-in
# defaults (no config.toml, vimrc or Sublime import), so the result is the same on every machine.
#
#   scripts/screenshots.sh            # all of them
#   scripts/screenshots.sh about      # only the shots whose name contains "about"
#
# Needs a GPU driver (Vulkan or GL) like the editor itself. Look at the PNGs before committing them.
set -euo pipefail

cd "$(dirname "$0")/.."
root=$PWD
only=${1:-}

cargo build --release -p shrew
bin=$root/target/release/srw
size=1280x800
out=docs/screenshots
mkdir -p "$out"

# shot OUT KEYS [FILES...]
shot() {
  local file=$1 keys=$2
  shift 2
  case $file in *"$only"*) ;; *) return 0 ;; esac
  echo "  $file"
  "$bin" --screenshot "$file" --size "$size" --keys "$keys" "$@"
}

core=crates/shrew-core/src
echo "README"
shot docs/screenshot.png ":theme catppuccin-mocha<CR>zt<C-y><C-y><C-y><C-y>f{" \
  "$core/motion.rs:25" "$core/editor.rs" crates/shrew/src/draw.rs
shot $out/normal.png "/fn step<CR>zt<C-y><C-y>*" crates/shrew/src/anim.rs
shot $out/multi-cursor.png "/mut chars<CR>w<C-n><C-n><C-n><C-n><C-n>" "$core/motion.rs"
shot $out/sublime.png ":set novim<CR><C-g>12<CR><C-d><C-d><C-d><C-d><C-d>pub(crate)" crates/shrew/src/layout.rs
shot $out/file-picker.png "<C-p>rend" "$core/motion.rs"
shot $out/palette.png "<C-S-p>toggle" "$core/motion.rs"
shot $out/features.png ":features<CR>" "$core/features.rs"
shot $out/theme-picker.png ":theme<CR><Down><Down><Down><Down><Down><Down><Down><Down><Down><Down><Down><Down><Down>" crates/shrew-theme/src/color.rs
shot $out/light.png ":theme one-light<CR>/fn contrast<CR>Vjj" crates/shrew-theme/src/color.rs
shot $out/contrast.png ":theme contrast<CR>/pub fn mix<CR>zt" crates/shrew-theme/src/color.rs
shot $out/substitute.png "/Rope<CR>:%s/Rope/Text/g" "$core/motion.rs"
shot $out/about.png ":about<CR>" "$core/motion.rs"
shot $out/sidebar.png "<C-S-e>" "$core/tree.rs"
shot $out/find-in-project.png "<C-S-f>sync_tree" "$core/tree.rs"
shot $out/symbols.png "<C-p>@" "$core/tree.rs"
# the terminal: a plain bash with a short prompt, so the shot does not depend on your shell setup
term=":term env PS1='\$ ' bash --norc --noprofile<CR>"
shot $out/terminal.png "${term}ls --color=always -F crates/shrew/src<CR>git log --oneline --graph --decorate --color -n 4<CR>grep -rn --color=always 'fn carve' crates<CR>" \
  crates/shrew/src/terminal/mod.rs:182
shot $out/terminal-vim.png ":theme one-light<CR>:term vim -Nu NONE +'syntax on' +'set nu' crates/shrew/src/layout.rs<CR>:30<CR>" \
  crates/shrew/src/terminal/mod.rs
shot $out/settings.png "<C-,>l" "$core/motion.rs"
shot $out/settings-light.png ":theme one-light<CR>:settings editor<CR>ljjj" "$core/motion.rs"
# everything at once: title bar, sidebar, minimap and a terminal under the text
shot $out/workspace.png ":set sidebar<CR>${term}ls crates<CR><C-w>k/fn compute<CR>zt" \
  crates/shrew/src/layout.rs crates/shrew/src/app.rs
# the title bar's View menu (Alt+V) over the sidebar
shot $out/menu.png "<C-S-e><Esc><A-v>" crates/shrew/src/layout.rs crates/shrew/src/app.rs
# the minimap: each line in its syntax colors, the visible lines boxed, the search's matches marked
shot $out/minimap.png ":theme tokyo-night<CR>/self.mode<CR>" "$core/editor.rs:700"

# a 50 MB, one-million-line file, at line 500,000 with a search
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
case huge-file.png in *"$only"*)
  # in a folder with a stable name: the title bar shows it as the project
  mkdir -p "$tmp/big"
  awk 'BEGIN { for (i = 0; i < 1000000; i++) printf "    let value_%07d = compute(value, %d);  // ok\n", i, i % 97 }' >"$tmp/big/huge.rs"
  echo "  $out/huge-file.png"
  (cd "$tmp/big" && "$bin" --screenshot "$root/$out/huge-file.png" --size "$size" --keys "/value_0500010<CR>" huge.rs:500000)
  ;;
esac

echo "THEMES.md"
color=crates/shrew-theme/src/color.rs
shot $out/theme-shrew-dark.png ":theme<CR>" "$color"
shot $out/theme-dracula.png ":theme dracula<CR>/contrast<CR>n" "$color"
shot $out/theme-catppuccin-mocha.png ":theme catppuccin-mocha<CR>:accent teal<CR>/fn mix<CR>A // blend<Esc>a" "$color"
shot $out/theme-tokyo-night.png ":theme tokyo-night<CR>/fn luminance<CR>zt" "$color"
shot $out/theme-gruvbox-dark.png ":theme gruvbox-dark<CR>/fn best_ink<CR>v2j$" "$color"
shot $out/theme-nord.png ":theme nord<CR>:accent rose<CR>/fn to_f32<CR>" "$color"
shot $out/theme-contrast.png ":theme contrast<CR>/fn luminance<CR>" "$color"
shot $out/theme-shrew-light.png ":theme shrew-light<CR>/fn contrast<CR>" "$color"
shot $out/theme-solarized-light.png ":theme solarized-light<CR>/fn mix<CR>" "$color"
shot $out/theme-one-light.png ":theme one-light<CR>/fn luminance<CR>Vjj" "$color"
shot $out/theme-contrast-light.png ":theme contrast-light<CR>/fn best_ink<CR>" "$color"
shot $out/theme-catppuccin-latte.png ":theme catppuccin-latte<CR>:accent<CR><Down><Down><Down><Down><Down><Down>" "$color"
