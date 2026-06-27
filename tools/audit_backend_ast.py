#!/usr/bin/env python3
"""Audit `use … parser::ast` imports in src/mvl/backends/ — TIR-first migration guard.

Background: ADR-0050 (#1594). Both emitters still import AST node types alongside TIR.
The target is 0. This tool counts every non-commented `use … parser::ast` line and
enforces a budget that must be lowered (never raised) as Phase 3 progresses.

Usage:
    python3 tools/audit_backend_ast.py                  # report + exit code
    python3 tools/audit_backend_ast.py --verbose        # list every import line
    python3 tools/audit_backend_ast.py --budget 18      # override budget
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).parent.parent
BACKENDS_DIR = ROOT / "src" / "mvl" / "backends"

# Match a `use … parser::ast` statement.
USE_RE = re.compile(r"\buse\b.*parser::ast")
# Line is a Rust line comment or doc comment.
COMMENT_RE = re.compile(r"^\s*//")


def collect_imports() -> list[tuple[Path, int, str]]:
    """Return (path, line_no, stripped_line) for every non-commented AST import."""
    hits: list[tuple[Path, int, str]] = []
    for path in sorted(BACKENDS_DIR.rglob("*.rs")):
        try:
            lines = path.read_text().splitlines()
        except (OSError, UnicodeDecodeError):
            continue
        for i, line in enumerate(lines, 1):
            stripped = line.strip()
            if COMMENT_RE.match(stripped):
                continue
            if USE_RE.search(stripped):
                hits.append((path, i, stripped))
    return hits


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--budget",
        type=int,
        default=14,
        help="Max allowed parser::ast use-imports in backends (default: 18, target: 0)",
    )
    ap.add_argument(
        "--verbose",
        action="store_true",
        help="List every import line, not just the count",
    )
    args = ap.parse_args()

    if not BACKENDS_DIR.exists():
        print(f"ERROR: backends dir not found at {BACKENDS_DIR}", file=sys.stderr)
        return 2

    hits = collect_imports()
    count = len(hits)

    print(f"Backend parser::ast use-imports: {count:3d} / budget {args.budget} (target 0)")

    if args.verbose and hits:
        print()
        for path, lno, line in hits:
            rel = path.relative_to(ROOT)
            print(f"  {rel}:{lno}  {line[:120]}")

    if count > args.budget:
        print(
            f"\nFAIL: {count} imports exceed budget {args.budget}. "
            "See ADR-0050. Raise budget only with documented justification.",
            file=sys.stderr,
        )
        return 1

    print("✓ OK (budget not exceeded; reduce toward 0 in Phase 3)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
