#!/usr/bin/env bash
# Install nvim-mvl: copy plugin into XDG data dir + compile tree-sitter parsers
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PLUGIN_SRC="$SCRIPT_DIR"
NVIM_DATA="${XDG_DATA_HOME:-$HOME/.local/share}/nvim"
INSTALL_DIR="$NVIM_DATA/site/pack/nvim-mvl/start/nvim-mvl"
INIT_LUA="${XDG_CONFIG_HOME:-$HOME/.config}/nvim/init.lua"

# ── 0. Validate paths stay under $HOME ───────────────────────────────────────
[[ "$NVIM_DATA" == "$HOME/"* ]] || { echo "ERROR: NVIM_DATA resolves outside HOME ($NVIM_DATA)"; exit 1; }
[[ "$INIT_LUA"  == "$HOME/"* ]] || { echo "ERROR: INIT_LUA resolves outside HOME ($INIT_LUA)";  exit 1; }

# ── 1. Require nvim before touching any files ─────────────────────────────────
if ! command -v nvim &>/dev/null; then
  echo "ERROR: nvim not found in PATH"
  exit 1
fi

echo "==> Installing nvim-mvl into $INSTALL_DIR"

# ── 2. Copy plugin files into XDG pack directory ─────────────────────────────
mkdir -p "$INSTALL_DIR"
cp -r "$PLUGIN_SRC"/. "$INSTALL_DIR/"
echo "    copied plugin files"

# ── 3. Idempotently wire into init.lua via sentinel markers ──────────────────
if [[ -f "$INIT_LUA" ]]; then
  cp "$INIT_LUA" "$INIT_LUA.bak"
  echo "    backed up init.lua → $INIT_LUA.bak"
  # Remove sentinel block from prior installs
  perl -i -0pe 's/\n-- BEGIN nvim-mvl\n.*?-- END nvim-mvl\n//gs' "$INIT_LUA"
  # Remove any remaining loose nvim-mvl lines (old repo-path rtp:prepend style)
  perl -i -ne 'print unless /nvim-mvl/' "$INIT_LUA"
  echo "    cleaned up old init.lua entries"
fi

# Append sentinel-marked block (always re-written after cleanup above)
cat >> "$INIT_LUA" <<ENDLUA

-- BEGIN nvim-mvl
-- MVL language support — installed via make install-nvim
vim.opt.runtimepath:prepend("$INSTALL_DIR")
require("mvl").setup()
-- END nvim-mvl
ENDLUA
echo "    wired XDG install path into $INIT_LUA"

# ── 4. Compile and install MVL parser from source ────────────────────────────
GRAMMAR_DIR="$(cd "$SCRIPT_DIR/../tree-sitter-mvl" && pwd)"
PARSER_C="$GRAMMAR_DIR/src/parser.c"
PARSER_OUT="$GRAMMAR_DIR/mvl.so"
PARSER_DST_DIRS=(
  "$NVIM_DATA/lazy/nvim-treesitter/parser"
  "$NVIM_DATA/site/parser"
)

echo "==> Compiling MVL parser from source ..."
cc -o "$PARSER_OUT" -shared -fPIC -Os \
  -I"$GRAMMAR_DIR/src" \
  "$PARSER_C" || { echo "    ERROR: failed to compile parser"; exit 1; }
echo "    compiled $PARSER_OUT"

for dir in "${PARSER_DST_DIRS[@]}"; do
  if [[ -d "$dir" ]]; then
    cp "$PARSER_OUT" "$dir/mvl.so"
    echo "    installed to $dir/mvl.so"
  fi
done

echo "==> Installing ebnf parser ..."
nvim --headless \
  +"TSInstall! ebnf" \
  +"sleep 5000m" \
  +qa

echo ""
echo "Done. Restart Neovim and open a .mvl file — syntax highlighting should be active."
echo "Verify with:  :checkhealth mvl"
