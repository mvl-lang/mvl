#!/usr/bin/env python3
"""Audit `parser::ast` references in src/mvl/backends/ — TIR-first migration guard.

Background: ADR-0050 (#1594). Both emitters must consume `TirProgram → String` only —
no AST types imported or referenced inline. This tool counts every non-commented
`parser::ast` reference (both `use` imports and inline qualified paths like
`crate::mvl::parser::ast::LogicOp::And`) and enforces a budget that must be lowered
(never raised).

Usage:
    python3 tools/audit_backend_ast.py                  # report + exit code
    python3 tools/audit_backend_ast.py --verbose        # list every reference
    python3 tools/audit_backend_ast.py --budget 18      # override budget
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).parent.parent
BACKENDS_DIR = ROOT / "src" / "mvl" / "backends"

# Match any `parser::ast` reference (use imports, inline qualified paths, type aliases).
AST_REF_RE = re.compile(r"parser::ast\b")
# Line is a Rust line comment or doc comment.
COMMENT_RE = re.compile(r"^\s*//")


def collect_refs() -> list[tuple[Path, int, str]]:
    """Return (path, line_no, stripped_line) for every non-commented AST reference."""
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
            if AST_REF_RE.search(stripped):
                hits.append((path, i, stripped))
    return hits


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--budget",
        type=int,
        default=0,
        help="Max allowed parser::ast references in backends (default: 0, target: 0)",
    )
    ap.add_argument(
        "--verbose",
        action="store_true",
        help="List every reference, not just the count",
    )
    args = ap.parse_args()

    if not BACKENDS_DIR.exists():
        print(f"ERROR: backends dir not found at {BACKENDS_DIR}", file=sys.stderr)
        return 2

    hits = collect_refs()
    count = len(hits)

    print(f"Backend parser::ast references: {count:3d} / budget {args.budget} (target 0)")

    if args.verbose and hits:
        print()
        for path, lno, line in hits:
            rel = path.relative_to(ROOT)
            print(f"  {rel}:{lno}  {line[:120]}")

    if count > args.budget:
        print(
            f"\nFAIL: {count} references exceed budget {args.budget}. "
            "See ADR-0050. Raise budget only with documented justification.",
            file=sys.stderr,
        )
        return 1

    print("✓ OK (budget not exceeded)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
