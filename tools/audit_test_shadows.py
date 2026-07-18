#!/usr/bin/env python3
"""Audit `*_test.mvl` files for shadow declarations — pattern 006.

Background: `.openspec/patterns/006-no-test-shadows.md`. Test files must
exercise production code, not parallel implementations of it.

Two rules, applied to every `*_test.mvl` file under `examples/`:

1. **All `type` declarations are shadows.** Any type declared in a test
   file is either a duplicate of production (drift bomb — ghost variants
   escape here) or a phantom (never in production). Both are anti-
   patterns. Test files should `use module::Type` — never declare their
   own.

2. **A `fn` / `total fn` / `partial fn` declaration is a shadow if a
   sibling production `.mvl` file in the same directory declares an item
   with the same name.** Test-local helpers that don't collide with
   production (fixture builders like `normal_vitals`, harness wrappers
   like `run_roundtrip`) are legitimate. A collision — like
   `authenticate` in both `auth.mvl` and `auth_test.mvl` — is a shadow.

Scope: `examples/` only. `tests/` contains compiler-self-test MVL programs
whose declarations are legitimate (they are the test corpus).

Historical cases (all fixed on `chore/exterminate-96-workaround`):
  - flight_clearance ghost `MaintenanceStatus::Cleared`
  - log_analyzer / task_pipeline dead `RunError::MissingArg`
  - access_control effect-stripped `log_access` shim
  - csv_transactions phantom `Transaction`

Usage:
    python3 tools/audit_test_shadows.py                  # report + exit code
    python3 tools/audit_test_shadows.py --verbose        # list every hit
    python3 tools/audit_test_shadows.py --budget 1       # override budget
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).parent.parent
SCAN_ROOT = ROOT / "examples"

# Match a top-level `type` declaration; capture (is_pub, name).
TYPE_DECL_RE = re.compile(r"^\s*(pub\s+)?type\s+([A-Z][A-Za-z0-9_]*)\b")

# Match a top-level fn / total fn / partial fn declaration; capture (is_pub, name).
FN_DECL_RE = re.compile(
    r"^\s*(pub\s+)?(?:total\s+|partial\s+)?fn\s+([a-z_][A-Za-z0-9_]*)\b"
)

# `test fn` is legitimate and matches the FN_DECL_RE, so exclude it explicitly.
TEST_FN_RE = re.compile(r"^\s*test\s+fn\s+")

# Line is an MVL line comment.
COMMENT_RE = re.compile(r"^\s*//")


def read_lines(path: Path) -> list[str]:
    try:
        return path.read_text().splitlines()
    except (OSError, UnicodeDecodeError):
        return []


def collect_sibling_pub_fn_names(test_path: Path) -> set[str]:
    """Return every `pub` fn name declared in production siblings.

    Only `pub` items count — private internal helpers colliding by name
    are not an API-drift risk (they're compilation-scoped).  A production
    sibling is any `.mvl` file in the same directory that is not
    `_test.mvl` or `_smoke.mvl`.
    """
    names: set[str] = set()
    for sibling in test_path.parent.glob("*.mvl"):
        if sibling.name.endswith("_test.mvl") or sibling.name.endswith("_smoke.mvl"):
            continue
        for line in read_lines(sibling):
            if COMMENT_RE.match(line):
                continue
            if TEST_FN_RE.match(line):
                continue
            m = FN_DECL_RE.match(line)
            if m and m.group(1):  # is_pub group
                names.add(m.group(2))
    return names


def collect_hits() -> list[tuple[Path, int, str, str]]:
    """Return (path, line, kind, snippet) for every shadow declaration."""
    hits: list[tuple[Path, int, str, str]] = []
    for path in sorted(SCAN_ROOT.rglob("*_test.mvl")):
        pub_names = collect_sibling_pub_fn_names(path)
        for i, line in enumerate(read_lines(path), 1):
            if COMMENT_RE.match(line):
                continue
            # Rule 1: any `type` declaration in a test file is a shadow.
            m = TYPE_DECL_RE.match(line)
            if m:
                hits.append((path, i, f"type {m.group(2)}", line.rstrip()))
                continue
            if TEST_FN_RE.match(line):
                continue
            # Rule 2: an fn declaration is a shadow iff it collides with a
            # `pub` fn in a production sibling.
            m = FN_DECL_RE.match(line)
            if m and m.group(2) in pub_names:
                hits.append(
                    (path, i, f"fn {m.group(2)} (shadows pub production fn)", line.rstrip())
                )
    return hits


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--budget",
        type=int,
        default=0,
        help="Max allowed shadow declarations (default: 0)",
    )
    ap.add_argument(
        "--verbose",
        action="store_true",
        help="List every hit, not just the count",
    )
    args = ap.parse_args()

    if not SCAN_ROOT.exists():
        print(f"ERROR: scan root not found at {SCAN_ROOT}", file=sys.stderr)
        return 2

    hits = collect_hits()
    count = len(hits)

    print(f"Test-file shadow declarations: {count:3d} / budget {args.budget} (target 0)")

    if args.verbose and hits:
        print()
        for path, lno, kind, snippet in hits:
            rel = path.relative_to(ROOT)
            print(f"  {rel}:{lno}  [{kind}]  {snippet[:100]}")

    if count > args.budget:
        print(
            f"\nFAIL: {count} shadow declarations exceed budget {args.budget}.\n"
            f"See .openspec/patterns/006-no-test-shadows.md for the rule and fix\n"
            f"recipes. Use --verbose to list every offending site.",
            file=sys.stderr,
        )
        return 1

    print("✓ OK (no test shadows detected)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
