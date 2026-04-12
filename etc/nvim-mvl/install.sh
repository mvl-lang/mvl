#!/usr/bin/env bash
# Install nvim-mvl: wire plugin into init.lua + compile tree-sitter parsers
set -euo pipefail

# Resolve the main repo (not the worktree) — follow symlinks up to the real path
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PLUGIN_DIR="$REPO_ROOT/etc/nvim-mvl"
INIT_LUA="${XDG_CONFIG_HOME:-$HOME/.config}/nvim/init.lua"

echo "==> Installing nvim-mvl from $PLUGIN_DIR"

# ── 1. Wire plugin into init.lua via rtp:prepend ─────────────────────────────
# lazy.nvim resets rtp during setup, so the pack/ symlink approach doesn't
# work. Add an explicit prepend after lazy.setup(), matching the sheerpower
# pattern already in init.lua.
if grep -q 'nvim-mvl' "$INIT_LUA" 2>/dev/null; then
  echo "    init.lua already has nvim-mvl entry"
else
  cat >> "$INIT_LUA" <<EOF

-- MVL language support (nvim-mvl)
vim.opt.runtimepath:prepend(vim.fn.expand('$PLUGIN_DIR'))
EOF
  echo "    added rtp:prepend to $INIT_LUA"
fi

# ── 2. Compile parsers via headless Neovim ────────────────────────────────────
if ! command -v nvim &>/dev/null; then
  echo "    ERROR: nvim not found in PATH"
  exit 1
fi

echo "==> Compiling tree-sitter parsers (:TSInstall mvl ebnf) ..."
nvim --headless \
  +"lua vim.opt.runtimepath:prepend('$PLUGIN_DIR')" \
  +"lua require('mvl').setup()" \
  +"TSInstall! mvl" \
  +"sleep 5000m" \
  +qa

# ebnf: install only if not already present
nvim --headless \
  +"TSInstall! ebnf" \
  +"sleep 5000m" \
  +qa

echo ""
echo "Done. Restart Neovim and open a .mvl file — syntax highlighting should be active."
echo "Verify with:  :checkhealth mvl"
