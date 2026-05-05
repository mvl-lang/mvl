#!/usr/bin/env bash
# Run `make test` for every example subdirectory.
# The MVL compiler is NOT recompiled here — it must be pre-built by the caller
# (root `make test-examples` depends on `build build-llvm-runtime`).
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Validate: MVL binary must exist and respond to --version ─────────────────
MVL_BIN="$REPO_ROOT/target/debug/mvl"
if [ ! -x "$MVL_BIN" ]; then
    echo ""
    echo "  ERROR: MVL compiler not found at $MVL_BIN"
    echo "  Run \`make build\` from the repo root first."
    echo ""
    exit 1
fi
MVL_VERSION=$("$MVL_BIN" --version 2>&1) || {
    echo ""
    echo "  ERROR: $MVL_BIN exists but \`--version\` failed"
    echo ""
    exit 1
}
echo ""
echo "  Using: $MVL_BIN  ($MVL_VERSION)"
echo ""

pass=0; fail=0

for dir in "$SCRIPT_DIR"/*/; do
    [ -f "$dir/Makefile" ] || continue
    name="$(basename "$dir")"
    printf "  %-20s  " "$name"
    if out=$(make -C "$dir" --no-print-directory test 2>&1); then
        printf "\033[32m✓  PASS\033[0m\n"
        pass=$((pass + 1))
    else
        printf "\033[31m✗  FAIL\033[0m\n"
        printf "%s\n" "$out" | sed 's/^/         /'
        fail=$((fail + 1))
    fi
done
echo ""

if [ "$fail" -eq 0 ]; then
    printf "  \033[32m✓  All %d example(s) passed\033[0m\n\n" "$pass"
else
    printf "  \033[31m✗  %d of %d example(s) failed\033[0m\n\n" "$fail" "$((pass + fail))"
    exit 1
fi
