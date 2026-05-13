#!/usr/bin/env bash
# Install nvim-mvl: copy plugin into XDG data dir + compile tree-sitter parsers
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PLUGIN_SRC="$SCRIPT_DIR"
NVIM_DATA="${XDG_DATA_HOME:-$HOME/.local/share}/nvim"
INSTALL_DIR="$NVIM_DATA/site/pack/nvim-mvl/start/nvim-mvl"
INIT_LUA="${XDG_CONFIG_HOME:-$HOME/.config}/nvim/init.lua"

echo "==> Installing nvim-mvl into $INSTALL_DIR"

# ── 1. Copy plugin files into XDG pack directory ─────────────────────────────
mkdir -p "$INSTALL_DIR"
cp -r "$PLUGIN_SRC"/. "$INSTALL_DIR/"
echo "    copied plugin files"

# ── 2. Clean up old repo-path entries; wire in fixed XDG path ────────────────
if [[ -f "$INIT_LUA" ]]; then
  # Remove lazy.nvim dir= block for nvim-mvl (multi-line)
  perl -i -0pe 's/\n\s*--[^\n]*MVL[^\n]*\n\s*\{\n\s*dir\s*=\s*[^\n]*nvim-mvl[^\n]*\n[^}]*\},//g' "$INIT_LUA"
  # Remove any existing nvim-mvl rtp/require lines (repo-path or XDG)
  perl -i -ne 'print unless /nvim-mvl|require\("mvl"\)|FileType.*mvl.*treesitter/' "$INIT_LUA"
  echo "    cleaned up old init.lua entries"
fi

# Add fixed XDG-path rtp:prepend so lazy.nvim doesn't block pack loading
if ! grep -q 'nvim-mvl' "$INIT_LUA" 2>/dev/null; then
  cat >> "$INIT_LUA" <<EOF

-- MVL language support (nvim-mvl) — installed via make install-nvim
vim.opt.runtimepath:prepend("$INSTALL_DIR")
require("mvl").setup()
EOF
  echo "    wired XDG install path into $INIT_LUA"
fi

# ── 3. Compile parsers via headless Neovim ────────────────────────────────────
if ! command -v nvim &>/dev/null; then
  echo "    ERROR: nvim not found in PATH"
  exit 1
fi

# ── 4. Install pre-compiled MVL parser directly ──────────────────────────────
PARSER_SRC="$(cd "$SCRIPT_DIR/../tree-sitter-mvl" && pwd)/parser.so"
PARSER_DST_DIRS=(
  "$NVIM_DATA/lazy/nvim-treesitter/parser"
  "$NVIM_DATA/site/parser"
)
if [[ -f "$PARSER_SRC" ]]; then
  echo "==> Installing pre-compiled MVL parser ..."
  for dir in "${PARSER_DST_DIRS[@]}"; do
    if [[ -d "$dir" ]]; then
      cp "$PARSER_SRC" "$dir/mvl.so"
      echo "    copied to $dir/mvl.so"
    fi
  done
else
  echo "==> Compiling MVL parser via nvim-treesitter (no pre-built parser found) ..."
  nvim --headless \
    +"packloadall" \
    +"lua require('mvl').setup()" \
    +"TSInstall! mvl" \
    +"sleep 5000m" \
    +qa
fi

echo "==> Installing ebnf parser ..."
nvim --headless \
  +"TSInstall! ebnf" \
  +"sleep 5000m" \
  +qa

echo ""
echo "Done. Restart Neovim and open a .mvl file — syntax highlighting should be active."
echo "Verify with:  :checkhealth mvl"
