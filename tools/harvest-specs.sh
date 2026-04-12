#!/usr/bin/env bash
# Harvest .openspec/specs/NNN-name/spec.md → docs/specs/NNN-name.md (symlinks)
# Run before mkdocs build. Idempotent — safe to run repeatedly.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(dirname "$SCRIPT_DIR")"
SPECS_SRC="${ROOT}/.openspec/specs"
SPECS_DST="${ROOT}/docs/specs"

mkdir -p "$SPECS_DST"

# Remove stale symlinks (spec was deleted or renamed)
for link in "$SPECS_DST"/*.md; do
    [ -L "$link" ] && [ ! -e "$link" ] && rm "$link"
done

# Create symlinks for each spec
count=0
for spec_dir in "$SPECS_SRC"/*/; do
    [ -d "$spec_dir" ] || continue
    spec_file="${spec_dir}spec.md"
    [ -f "$spec_file" ] || continue

    name="$(basename "$spec_dir")"
    target="../../.openspec/specs/${name}/spec.md"
    dest="${SPECS_DST}/${name}.md"

    if [ -L "$dest" ]; then
        # Already a symlink — check if target matches
        current="$(readlink "$dest")"
        [ "$current" = "$target" ] && { count=$((count + 1)); continue; }
        rm "$dest"
    elif [ -f "$dest" ]; then
        # Regular file exists — replace with symlink
        rm "$dest"
    fi

    ln -s "$target" "$dest"
    count=$((count + 1))
done

echo "Harvested ${count} specs → docs/specs/"
