#!/usr/bin/env python3
"""Audit `unreachable!()` / `panic!()` sites in src/mvl/ — split PROD vs TEST.

Background: see issue #1549 (follow-up to #991). The old gate counted every
panic site equally — a unit-test `panic!("expected Struct body")` consumed the
same budget as a production compiler crash. This tool classifies each site as
TEST (inside `#[cfg(test)]` or `#[test]`) or PROD (everything else) using a
simple brace tracker and reports two counts with two budgets.

Usage:
    python3 tools/audit_panics.py                  # report + exit code
    python3 tools/audit_panics.py --verbose        # list every site
    python3 tools/audit_panics.py --prod-budget 30 --test-budget 100
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).parent.parent
SRC_DIR = ROOT / "src" / "mvl"

PANIC_RE = re.compile(r"\b(unreachable|panic)!")
# Skip string-literal mentions like "panic!" inside a diagnostic message.
SKIP_RE = re.compile(r'"panic"|"panic!')

CFG_TEST_RE = re.compile(r"#\[cfg\(test\)\]|#\[test\]")


def classify_sites(path: Path) -> list[tuple[int, str, str]]:
    """Return list of (line_no, kind, snippet) where kind is 'test' or 'prod'."""
    try:
        lines = path.read_text().splitlines()
    except (OSError, UnicodeDecodeError):
        return []

    # Stack of brace scopes preceding the current cursor:
    #   ("pending_test", None) — saw #[cfg(test)] / #[test], waiting for `{`
    #   ("test",  brace_depth)  — open test scope; pops when brace returns to depth
    #   ("normal", brace_depth) — open non-test scope
    stack: list[list] = []
    brace_depth = 0
    sites: list[tuple[int, str, str]] = []

    for i, line in enumerate(lines, 1):
        stripped = line.strip()

        if CFG_TEST_RE.search(stripped):
            stack.append(["pending_test", None])

        # Process braces character by character so per-line `{...}` balances correctly.
        for ch in line:
            if ch == "{":
                if stack and stack[-1][0] == "pending_test":
                    stack[-1] = ["test", brace_depth]
                else:
                    stack.append(["normal", brace_depth])
                brace_depth += 1
            elif ch == "}":
                brace_depth -= 1
                if stack and stack[-1][1] == brace_depth:
                    stack.pop()

        if PANIC_RE.search(line) and not SKIP_RE.search(line):
            kind = "test" if any(s[0] == "test" for s in stack) else "prod"
            sites.append((i, kind, stripped))

    return sites


def collect_all() -> dict[str, list[tuple[Path, int, str]]]:
    """Walk src/mvl/, return {'prod': [...], 'test': [...]}."""
    out: dict[str, list[tuple[Path, int, str]]] = {"prod": [], "test": []}
    for path in sorted(SRC_DIR.rglob("*.rs")):
        for lno, kind, snippet in classify_sites(path):
            out[kind].append((path, lno, snippet))
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--prod-budget", type=int, default=30,
                    help="Max allowed production panic sites (default: 30)")
    ap.add_argument("--test-budget", type=int, default=100,
                    help="Max allowed test panic sites (default: 100)")
    ap.add_argument("--verbose", action="store_true",
                    help="List every site, not just the counts")
    args = ap.parse_args()

    if not SRC_DIR.exists():
        print(f"ERROR: source tree not found at {SRC_DIR}", file=sys.stderr)
        return 2

    sites = collect_all()
    prod = sites["prod"]
    test = sites["test"]

    print(f"PRODUCTION panic sites: {len(prod):3d} / budget {args.prod_budget}")
    print(f"TEST       panic sites: {len(test):3d} / budget {args.test_budget}")

    if args.verbose:
        print()
        print("=== PROD ===")
        for path, lno, snippet in prod:
            rel = path.relative_to(ROOT)
            print(f"  {rel}:{lno}  {snippet[:100]}")
        print()
        print("=== TEST ===")
        for path, lno, snippet in test:
            rel = path.relative_to(ROOT)
            print(f"  {rel}:{lno}  {snippet[:100]}")

    failed = False
    if len(prod) > args.prod_budget:
        print(f"\nFAIL: production count {len(prod)} exceeds budget "
              f"{args.prod_budget} — see issue #1549", file=sys.stderr)
        failed = True
    if len(test) > args.test_budget:
        print(f"\nFAIL: test count {len(test)} exceeds budget "
              f"{args.test_budget} — see issue #1549", file=sys.stderr)
        failed = True

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
