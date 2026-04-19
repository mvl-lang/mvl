#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EXT_DIR="$SCRIPT_DIR/vscode-mvl"

cd "$EXT_DIR"

echo "Packaging MVL VSCode extension..."
npx @vscode/vsce package --no-dependencies

echo "Installing extension..."
code --install-extension vscode-mvl-0.0.1.vsix

echo "Done. Restart VS Code to activate syntax highlighting for .mvl files."
