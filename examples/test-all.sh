#!/usr/bin/env bash
# Run `make test` (or `make test-llvm` with --llvm) for every example subdirectory.
# With --full: also runs `make check` and `make run` for each example.
# The MVL compiler is NOT recompiled here — it must be pre-built by the caller
# (root `make test-examples` depends on `build build-llvm-runtime`).
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Parse arguments ───────────────────────────────────────────────────────────
TEST_TARGET="test"
FULL=0
for arg in "$@"; do
    case "$arg" in
        --llvm) TEST_TARGET="test-llvm" ;;
        --full) FULL=1 ;;
        *) echo "Unknown argument: $arg"; exit 1 ;;
    esac
done

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

run_target() {
    local dir="$1" target="$2"
    make -C "$dir" --no-print-directory "$target" 2>&1
}

for dir in "$SCRIPT_DIR"/*/; do
    [ -f "$dir/Makefile" ] || continue
    name="$(basename "$dir")"

    if [ "$FULL" -eq 1 ]; then
        printf "  %-20s" "$name"
        example_ok=1
        failed_targets=""
        for target in check "$TEST_TARGET" run; do
            if out=$(run_target "$dir" "$target" 2>&1); then
                printf "  \033[32m%-8s✓\033[0m" "$target"
            else
                printf "  \033[31m%-8s✗\033[0m" "$target"
                failed_targets="$failed_targets\n--- $target ---\n$out"
                example_ok=0
            fi
        done
        printf "\n"
        if [ "$example_ok" -eq 0 ]; then
            printf "%b\n" "$failed_targets" | sed 's/^/         /'
            fail=$((fail + 1))
        else
            pass=$((pass + 1))
        fi
    else
        printf "  %-20s  " "$name"
        if out=$(run_target "$dir" "$TEST_TARGET" 2>&1); then
            printf "\033[32m✓  PASS\033[0m\n"
            pass=$((pass + 1))
        else
            printf "\033[31m✗  FAIL\033[0m\n"
            printf "%s\n" "$out" | sed 's/^/         /'
            fail=$((fail + 1))
        fi
    fi
done
echo ""

if [ "$fail" -eq 0 ]; then
    printf "  \033[32m✓  All %d example(s) passed\033[0m\n\n" "$pass"
else
    printf "  \033[31m✗  %d of %d example(s) failed\033[0m\n\n" "$fail" "$((pass + fail))"
    exit 1
fi
