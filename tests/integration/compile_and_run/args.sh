#!/usr/bin/env bash
# tests/integration/compile_and_run/args.sh
#
# E2e tests for parse_args using the real log_analyzer and task_pipeline examples.
#
# Covers (issue #864):
#   --name value          required named flag
#   --name=value          equals form
#   optional absent       key absent from map, default used
#   optional present      value parsed and used
#   --threshold -0.5      schema-aware flag disambiguation (negative float)
#   --threshold abc       type coercion error → exit 1
#   --help / -h           exits 0 with usage text (run via compiled binary)
#   missing required arg  exits 1 with error message
#
# Not exercised here (neither example schema declares these):
#   flag (boolean presence), positional / opt_positional, Int type coercion.
# Those are covered at the unit level in tests/stdlib/args_test.mvl.
#
# Run standalone: bash tests/integration/compile_and_run/args.sh
# Run via make:   make test-integration

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
MVL="$REPO_ROOT/target/debug/mvl"
LOG_MAIN="$REPO_ROOT/examples/log_analyzer/main.mvl"
TASK_MAIN="$REPO_ROOT/examples/task_pipeline/main.mvl"
SAMPLE_CSV="$REPO_ROOT/examples/task_pipeline/sample.csv"

pass=0
fail=0
OK="\033[32m✓\033[0m"
FAIL="\033[31m✗\033[0m"

die() { echo ""; echo "  ERROR: $*"; echo ""; exit 1; }

[ -x "$MVL" ] || die "MVL compiler not found at: $MVL  (run 'make build' first)"

ok() {
    printf "  $OK  %s\n" "$1"
    pass=$((pass + 1))
}

fail_case() {
    printf "  $FAIL  %s\n" "$1"
    printf "         %s\n" "$2"
    fail=$((fail + 1))
}

assert_success() {
    local label="$1"; shift
    local out; out=$("$@" 2>&1); local rc=$?
    if [ $rc -eq 0 ]; then ok "$label"
    else fail_case "$label" "expected exit 0, got $rc: $out"
    fi
}

assert_fail() {
    local label="$1"; shift
    local out; out=$("$@" 2>&1); local rc=$?
    if [ $rc -ne 0 ]; then ok "$label"
    else fail_case "$label" "expected exit non-0 but succeeded: $out"
    fi
}

# grep -F -- guards against patterns that start with '-' (e.g. '-h, --help')
# being misinterpreted as grep flags on BSD grep (macOS).
assert_stdout_contains() {
    local label="$1"; local needle="$2"; shift 2
    local out; out=$("$@" 2>&1); local rc=$?
    if [ $rc -eq 0 ] && printf '%s' "$out" | grep -qF -- "$needle"; then
        ok "$label"
    else
        fail_case "$label" "expected '$needle' in stdout (exit $rc)"
    fi
}

assert_stderr_contains() {
    local label="$1"; local needle="$2"; shift 2
    local stderr_out; local stdout_out
    stdout_out=$("$@" 2>/tmp/mvl_args_test_stderr); local rc=$?
    stderr_out=$(cat /tmp/mvl_args_test_stderr); rm -f /tmp/mvl_args_test_stderr
    if [ $rc -ne 0 ] && printf '%s' "$stderr_out" | grep -qF -- "$needle"; then
        ok "$label"
    else
        fail_case "$label" "expected '$needle' in stderr (exit $rc): $stderr_out"
    fi
}

# ── Build log_analyzer binary for direct invocation ──────────────────────────
# mvl run intercepts --help/-h before the compiled program sees it; running the
# pre-built binary directly is the only way to test parse_args --help behaviour.
LOG_BUILD_OUT=$("$MVL" build "$LOG_MAIN" 2>&1)
LOG_TRANSPILE_DIR=$(printf '%s\n' "$LOG_BUILD_OUT" | sed -n 's/Transpiled to: //p')
LOG_BIN="${LOG_TRANSPILE_DIR}/target/debug/main"
[ -x "$LOG_BIN" ] || die "log_analyzer binary not found at $LOG_BIN"

# ── Temporary log fixture ─────────────────────────────────────────────────────
TMPLOG=$(mktemp /tmp/mvl_args_test_XXXXXX.jsonl)
trap 'rm -f "$TMPLOG"' EXIT
printf '{"level":"info","message":"started","timestamp":1000}\n'  > "$TMPLOG"
printf '{"level":"warn","message":"retrying","timestamp":1001}\n' >> "$TMPLOG"

echo ""
echo "parse_args e2e — log_analyzer (required Str + optional Str)"
echo "─────────────────────────────────────────────────────────────"

assert_success \
    "--file <path>  required named flag" \
    "$MVL" run "$LOG_MAIN" -- --file "$TMPLOG"

assert_success \
    "--file=<path>  equals form" \
    "$MVL" run "$LOG_MAIN" -- "--file=$TMPLOG"

assert_success \
    "--level <value>  optional present" \
    "$MVL" run "$LOG_MAIN" -- --file "$TMPLOG" --level warn

assert_success \
    "optional absent  (no --level)" \
    "$MVL" run "$LOG_MAIN" -- --file "$TMPLOG"

assert_stdout_contains \
    "--help  exits 0 with usage" "-h, --help" \
    "$LOG_BIN" --help

assert_stdout_contains \
    "-h  exits 0 with usage" "-h, --help" \
    "$LOG_BIN" -h

assert_stderr_contains \
    "missing required --file  exits 1" "missing required flag --file" \
    "$MVL" run "$LOG_MAIN" --

echo ""
echo "parse_args e2e — task_pipeline (required Str + optional Float)"
echo "─────────────────────────────────────────────────────────────"

assert_success \
    "--input <path>  required named flag" \
    "$MVL" run "$TASK_MAIN" -- --input "$SAMPLE_CSV"

assert_success \
    "--input=<path>  equals form" \
    "$MVL" run "$TASK_MAIN" -- "--input=$SAMPLE_CSV"

assert_success \
    "--threshold 50.0  optional float present" \
    "$MVL" run "$TASK_MAIN" -- --input "$SAMPLE_CSV" --threshold 50.0

assert_success \
    "--threshold -0.5  negative float (schema-aware)" \
    "$MVL" run "$TASK_MAIN" -- --input "$SAMPLE_CSV" --threshold -0.5

assert_success \
    "optional absent  (no --threshold)" \
    "$MVL" run "$TASK_MAIN" -- --input "$SAMPLE_CSV"

assert_stderr_contains \
    "missing required --input  exits 1" "missing required flag --input" \
    "$MVL" run "$TASK_MAIN" --

assert_stderr_contains \
    "--threshold abc  type coercion error exits 1" "expects a float" \
    "$MVL" run "$TASK_MAIN" -- --input "$SAMPLE_CSV" --threshold abc

echo ""
if [ $fail -eq 0 ]; then
    printf "  \033[32m✓  %d passed, 0 failed\033[0m\n\n" "$pass"
else
    printf "  \033[31m✗  %d passed, %d failed\033[0m\n\n" "$pass" "$fail"
    exit 1
fi
