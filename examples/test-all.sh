#!/usr/bin/env bash
# Run `make test` (or `make test-llvm` with --llvm) for every example subdirectory.
# With --full: also runs `make check`, `make test-solver`, and `make smoke` for each example.
# The MVL compiler is NOT recompiled here — it must be pre-built by the caller
# (root `make test-examples` depends on `build`).
#
# Examples run in parallel (bounded by CPU count) — each is an independent
# `mvl`/`make` invocation with no shared state. A couple of examples (heavy
# refinement arithmetic that falls through to Z3) dominate the serial runtime
# by 10-50x over the rest; running them concurrently with everything else
# instead of after it cuts wall time toward the slowest single example
# instead of the sum of all of them. Output is still printed in the original
# per-directory order, buffered per example so concurrent output never
# interleaves mid-line.
set -uo pipefail

# Prevent the mvl binary from re-execing to the installed pinned toolchain.
# Without this, a stale installed binary would silently run instead of the
# freshly-built dev binary this script requires (see Makefile:136-139).
export MVL_NO_REEXEC=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Parse arguments ───────────────────────────────────────────────────────────
TEST_TARGET="test"
FULL=0
for arg in "$@"; do
    case "$arg" in
        --llvm) TEST_TARGET="test-llvm" ;;
        --wasm) TEST_TARGET="test-wasm" ;;
        --smoke) TEST_TARGET="smoke" ;;
        --full) FULL=1 ;;
        -h|--help)
            echo ""
            echo "Usage: test-all.sh [OPTIONS]"
            echo ""
            echo "Run \`make test\` for every example subdirectory (in parallel, bounded by"
            echo "CPU count — override with MAX_JOBS=N)."
            echo ""
            echo "Options:"
            echo "  --llvm    Use LLVM backend (runs \`make test-llvm\` instead of \`make test\`)"
            echo "  --wasm    Use WASM backend (runs \`make test-wasm\` instead of \`make test\`)"
            echo "  --smoke   Run Rust transpiler smoke build (runs \`make smoke\` instead of \`make test\`)"
            echo "  --full    Also run \`make check\`, \`make test-solver\`, and \`make smoke\` per example"
            echo "  -h, --help  Show this help and exit"
            echo ""
            echo "The MVL compiler must be pre-built (\`make build\` from repo root)."
            echo ""
            exit 0 ;;
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

# Portable CPU count (getconf works on both macOS and Linux; nproc doesn't
# exist on macOS, sysctl doesn't exist on Linux). MAX_JOBS lets a caller
# override — e.g. a CI runner with a known, smaller core count.
MAX_JOBS="${MAX_JOBS:-$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)}"

echo ""
echo "  Using: $MVL_BIN  ($MVL_VERSION)"
echo "  Parallelism: $MAX_JOBS"
echo ""

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

run_target() {
    local dir="$1" target="$2"
    make -C "$dir" --no-print-directory "$target" 2>&1
}

has_target() {
    local dir="$1" target="$2"
    grep -q "^${target}:" "$dir/Makefile" 2>/dev/null
}

# Runs one example's full check (buffered to $outfile, never printed directly
# — parallel jobs writing straight to the terminal would interleave mid-line).
# Writes "pass"/"fail"/"skip" to $statusfile so the parent can tally results
# without relying on subshell variable mutation (which doesn't propagate back
# to the parent shell across a background job).
run_example() {
    local dir="$1" outfile="$2" statusfile="$3"
    local name
    name="$(basename "$dir")"

    if [ "$FULL" -eq 1 ]; then
        {
            printf "  %-20s" "$name"
            example_ok=1
            failed_targets=""
            for target in check test-solver "$TEST_TARGET" smoke; do
                if out=$(run_target "$dir" "$target" 2>&1); then
                    printf "  \033[32m%-10s✓\033[0m" "$target"
                else
                    printf "  \033[31m%-10s✗\033[0m" "$target"
                    failed_targets="$failed_targets\n--- $target ---\n$out"
                    example_ok=0
                fi
            done
            printf "\n"
            if [ "$example_ok" -eq 0 ]; then
                printf "%b\n" "$failed_targets" | sed 's/^/         /'
                echo "fail" > "$statusfile"
            else
                echo "pass" > "$statusfile"
            fi
        } > "$outfile" 2>&1
    else
        if ! has_target "$dir" "$TEST_TARGET"; then
            printf "  %-20s  \033[33m-  SKIP\033[0m\n" "$name" > "$outfile"
            echo "skip" > "$statusfile"
            return
        fi
        {
            printf "  %-20s  " "$name"
            if out=$(run_target "$dir" "$TEST_TARGET" 2>&1); then
                printf "\033[32m✓  PASS\033[0m\n"
                echo "pass" > "$statusfile"
            else
                printf "\033[31m✗  FAIL\033[0m\n"
                printf "%s\n" "$out" | sed 's/^/         /'
                echo "fail" > "$statusfile"
            fi
        } > "$outfile" 2>&1
    fi
}

# ── Launch, bounded by MAX_JOBS ────────────────────────────────────────────────
names=()
outfiles=()
statusfiles=()
pids=()
running=0

for dir in "$SCRIPT_DIR"/*/; do
    [ -f "$dir/Makefile" ] || continue
    name="$(basename "$dir")"
    outfile="$WORKDIR/$name.out"
    statusfile="$WORKDIR/$name.status"

    names+=("$name")
    outfiles+=("$outfile")
    statusfiles+=("$statusfile")

    run_example "$dir" "$outfile" "$statusfile" &
    pids+=("$!")
    running=$((running + 1))

    # Sliding-window throttle: once at capacity, wait for the oldest launched
    # job before starting another. Portable to bash 3.2+ (no `wait -n`, which
    # macOS's system /bin/bash doesn't have — this repo doesn't assume bash 4+
    # anywhere else either).
    if [ "$running" -ge "$MAX_JOBS" ]; then
        wait "${pids[0]}" 2>/dev/null || true
        pids=("${pids[@]:1}")
        running=$((running - 1))
    fi
done

# Drain remaining jobs.
wait 2>/dev/null || true

# ── Print buffered output + tally, in original directory order ───────────────
pass=0; fail=0; skip=0
for i in "${!names[@]}"; do
    cat "${outfiles[$i]}"
    case "$(cat "${statusfiles[$i]}" 2>/dev/null)" in
        pass) pass=$((pass + 1)) ;;
        fail) fail=$((fail + 1)) ;;
        skip) skip=$((skip + 1)) ;;
        *) fail=$((fail + 1)) ;; # missing status file — treat as failure, not silent skip
    esac
done
echo ""

skip_msg=""
if [ "$skip" -gt 0 ]; then
    skip_msg=" ($skip skipped)"
fi
if [ "$fail" -eq 0 ]; then
    printf "  \033[32m✓  All %d example(s) passed%s\033[0m\n\n" "$pass" "$skip_msg"
else
    printf "  \033[31m✗  %d of %d example(s) failed%s\033[0m\n\n" "$fail" "$((pass + fail))" "$skip_msg"
    exit 1
fi
