#!/usr/bin/env bash
# run-tests.sh — for each test-cases/<name>/, run its main.mvl through the
# WASM harness and compare stdout against expected-stdout.txt (captured
# from `mvl run main.mvl` — the native/Rust-backend execution, treated as
# ground truth for correct program behavior).
#
# A test case failing here is not a harness bug report — it's the harness
# doing its job. Report the real result, including a known/tracked crash,
# rather than skip or soften it.
set -uo pipefail  # not -e: a failing test case is an expected, reported outcome, not a script error
cd "$(dirname "${BASH_SOURCE[0]}")"

# Every other experiment in this repo installs its own deps from build.sh —
# this one didn't, and it showed: verified from throwaway worktrees that
# already had `npm install` run in them, never from a genuinely fresh
# `git clone`/`git pull`, which is exactly how this was first actually used
# for real and immediately failed with ERR_MODULE_NOT_FOUND.
if [ ! -d harness/node_modules ]; then
  echo "→ Installing harness dependencies (first run)..."
  (cd harness && npm install --no-audit --no-fund >/dev/null)
  echo
fi

FAIL=0
for dir in test-cases/*/; do
  name="$(basename "$dir")"
  echo "=== $name ==="

  actual="$(node harness/run-example.mjs "$dir/main.mvl" 2>/tmp/harness_stderr_$name.log)"
  status=$?

  if [ -f "$dir/expected-stdout.txt" ]; then
    expected="$(cat "$dir/expected-stdout.txt")"
    if [ "$actual" = "$expected" ]; then
      echo "  PASS — output matches mvl run (native backend) exactly"
    elif [ "$(echo "$actual" | sort)" = "$(echo "$expected" | sort)" ]; then
      # Same lines, different order: native actor-mailbox interleaving is
      # itself nondeterministic (see README "actor console-output ordering"),
      # so an exact sequence match isn't a meaningful bar here. Same lines
      # printed means the same values were produced — that's what matters.
      echo "  PASS — output matches mvl run (native backend), lines reordered"
      echo "  --- diff (expected vs actual, order only) ---"
      diff <(echo "$expected") <(echo "$actual") | sed 's/^/  /'
    else
      echo "  FAIL — output diverges from mvl run (native backend)"
      echo "  --- diff (expected vs actual) ---"
      diff <(echo "$expected") <(echo "$actual") | sed 's/^/  /'
      if [ "$status" -ne 0 ]; then
        echo "  --- runtime error (see full trace in /tmp/harness_stderr_$name.log) ---"
        tail -5 "/tmp/harness_stderr_$name.log" | sed 's/^/  /'
      fi
      FAIL=1
    fi
  else
    echo "  no expected-stdout.txt — printing captured output for inspection:"
    echo "$actual" | sed 's/^/  /'
  fi
  echo
done

exit $FAIL
