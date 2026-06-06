#!/usr/bin/env bash
# benchmark.sh — Compare gzip performance: MVL vs system gzip vs Rust/flate2
#
# Usage: ./benchmark.sh [10|100|1000]
#        make -C tests/spikes/003-gzip benchmark ITERS=100
set -euo pipefail

ITERS="${1:-10}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
MVL="$ROOT/target/debug/mvl"
BENCH_DIR="$SCRIPT_DIR/bench"
PAYLOAD="/tmp/gzip_bench_payload.bin"
COMPRESSED="/tmp/gzip_bench_payload.bin.gz"

# ── Generate payload ──────────────────────────────────────────────────────────
python3 -c "
import sys
pat = b'Hello World!'
data = (pat * 50)[:512]
sys.stdout.buffer.write(data)
" > "$PAYLOAD"
PAYLOAD_SIZE=$(wc -c < "$PAYLOAD" | tr -d ' ')

echo "═══════════════════════════════════════════════════════════════"
echo "  gzip benchmark — $ITERS iterations, ${PAYLOAD_SIZE}-byte payload"
echo "═══════════════════════════════════════════════════════════════"
echo ""

# ── Helper: time a command, return seconds ────────────────────────────────────
time_cmd() {
    python3 -c "
import subprocess, time, sys
start = time.monotonic()
r = subprocess.run(sys.argv[1:], capture_output=True)
elapsed = time.monotonic() - start
print(f'{elapsed:.6f}')
if r.returncode != 0:
    sys.stderr.write(r.stderr.decode())
    sys.exit(r.returncode)
" "$@"
}

# ── 1. System gzip (C) ───────────────────────────────────────────────────────
echo "▸ system gzip (C)..."
GZIP_SECS=$(python3 -c "
import subprocess, time, sys
start = time.monotonic()
for _ in range($ITERS):
    subprocess.run(['gzip', '-c', '$PAYLOAD'], stdout=open('$COMPRESSED', 'wb'), check=True)
    subprocess.run(['gzip', '-d', '-c', '$COMPRESSED'], stdout=subprocess.DEVNULL, check=True)
elapsed = time.monotonic() - start
print(f'{elapsed:.6f}')
")
GZIP_US=$(python3 -c "print(f'{$GZIP_SECS / $ITERS * 1_000_000:.1f}')")
echo "  system gzip: ${ITERS} iters, ${GZIP_SECS}s, ${GZIP_US}µs/iter"
echo ""

# ── 2. Rust/flate2 ───────────────────────────────────────────────────────────
echo "▸ rust/flate2 (building release binary)..."
(cd "$BENCH_DIR" && cargo build --release --quiet 2>&1)
FLATE2_BIN="$BENCH_DIR/target/release/gzip-bench"
FLATE2_OUT=$("$FLATE2_BIN" "$ITERS" "$PAYLOAD" 2>&1)
echo "  $FLATE2_OUT"
FLATE2_SECS=$(echo "$FLATE2_OUT" | grep -oE '[0-9]+\.[0-9]+s total' | grep -oE '[0-9]+\.[0-9]+')
FLATE2_US=$(echo "$FLATE2_OUT" | grep -oE '[0-9]+\.[0-9]+µs/iter' | grep -oE '[0-9]+\.[0-9]+')
echo ""

# ── 3. MVL gzip (debug) ──────────────────────────────────────────────────────
echo "▸ mvl/gzip debug (pre-building)..."
BUILD_OUT=$("$MVL" build "$SCRIPT_DIR/gzip_perf.mvl" 2>&1)
BUILD_DIR=$(echo "$BUILD_OUT" | grep "Transpiled to:" | awk '{print $3}')
MVL_BIN_DBG="$BUILD_DIR/target/debug/gzip_perf"

if [ -x "$MVL_BIN_DBG" ]; then
    MVL_DBG_SECS=$(time_cmd "$MVL_BIN_DBG" --iterations "$ITERS")
    MVL_DBG_US=$(python3 -c "print(f'{$MVL_DBG_SECS / $ITERS * 1_000_000:.1f}')")
    echo "  mvl/gzip debug: ${ITERS} iters, ${MVL_DBG_SECS}s, ${MVL_DBG_US}µs/iter"
else
    echo "  ERROR: could not find MVL binary at $MVL_BIN_DBG"
    MVL_DBG_SECS="N/A"
    MVL_DBG_US="N/A"
fi

# ── 4. MVL gzip (release) ───────────────────────────────────────────────────
echo "▸ mvl/gzip release (pre-building)..."
BUILD_OUT=$("$MVL" build --release "$SCRIPT_DIR/gzip_perf.mvl" 2>&1)
BUILD_DIR=$(echo "$BUILD_OUT" | grep "Transpiled to:" | awk '{print $3}')
MVL_BIN_REL="$BUILD_DIR/target/release/gzip_perf"

if [ -x "$MVL_BIN_REL" ]; then
    MVL_REL_SECS=$(time_cmd "$MVL_BIN_REL" --iterations "$ITERS")
    MVL_REL_US=$(python3 -c "print(f'{$MVL_REL_SECS / $ITERS * 1_000_000:.1f}')")
    echo "  mvl/gzip release: ${ITERS} iters, ${MVL_REL_SECS}s, ${MVL_REL_US}µs/iter"
else
    echo "  ERROR: could not find MVL binary at $MVL_BIN_REL"
    MVL_REL_SECS="N/A"
    MVL_REL_US="N/A"
fi
echo ""

# ── Summary ───────────────────────────────────────────────────────────────────
echo "═══════════════════════════════════════════════════════════════"
echo "  Summary ($ITERS iterations, ${PAYLOAD_SIZE}B payload)"
echo "───────────────────────────────────────────────────────────────"
printf "  %-20s %10s %14s\n" "Implementation" "Total (s)" "Per-iter (µs)"
printf "  %-20s %10s %14s\n" "──────────────────" "─────────" "─────────────"
printf "  %-20s %10s %14s\n" "system gzip (C)" "$GZIP_SECS" "$GZIP_US"
printf "  %-20s %10s %14s\n" "rust/flate2" "$FLATE2_SECS" "$FLATE2_US"
printf "  %-20s %10s %14s\n" "mvl/gzip (debug)" "$MVL_DBG_SECS" "$MVL_DBG_US"
printf "  %-20s %10s %14s\n" "mvl/gzip (release)" "$MVL_REL_SECS" "$MVL_REL_US"
echo "═══════════════════════════════════════════════════════════════"

# Cleanup
rm -f "$PAYLOAD" "$COMPRESSED"
