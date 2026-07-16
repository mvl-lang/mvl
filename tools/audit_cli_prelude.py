#!/usr/bin/env python3
"""Audit direct loader calls in src/cli/ — canonical-prelude-assembler guard.

Background: ADR-0050 extension (2026-07-16, #1803). Every CLI subcommand that
assembles a stdlib prelude for the checker or transpile pipeline must route
through `pipeline::load_full_prelude`. Historically each subcommand picked
between `loader::load_stdlib_prelude` and `loader::load_mvl_native_stdlib_extras`
on its own — that produced three silent-failure incidents (`mvl mcdc`,
`mvl tir` / `mvl mutate` — #1788).

This lint fails if any file under `src/cli/` references the internal loaders
directly. Test fixtures and integration tests in `tests/` remain free to call
them — this guard is scoped to the CLI entry points.

Usage:
    python3 tools/audit_cli_prelude.py                  # report + exit code
    python3 tools/audit_cli_prelude.py --verbose        # list every reference
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).parent.parent
CLI_DIR = ROOT / "src" / "cli"

BANNED = ("load_stdlib_prelude", "load_mvl_native_stdlib_extras")
LOADER_CALL_RE = re.compile(r"loader::(load_stdlib_prelude|load_mvl_native_stdlib_extras)\b")
COMMENT_RE = re.compile(r"^\s*//")


def collect_refs() -> list[tuple[Path, int, str]]:
    """Return (path, line_no, stripped_line) for every non-commented banned call."""
    hits: list[tuple[Path, int, str]] = []
    for path in sorted(CLI_DIR.rglob("*.rs")):
        try:
            lines = path.read_text().splitlines()
        except (OSError, UnicodeDecodeError):
            continue
        for i, line in enumerate(lines, 1):
            stripped = line.strip()
            if COMMENT_RE.match(stripped):
                continue
            if LOADER_CALL_RE.search(stripped):
                hits.append((path, i, stripped))
    return hits


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--verbose",
        action="store_true",
        help="List every reference, not just the count",
    )
    args = ap.parse_args()

    if not CLI_DIR.exists():
        print(f"ERROR: CLI dir not found at {CLI_DIR}", file=sys.stderr)
        return 2

    hits = collect_refs()
    count = len(hits)

    print(f"CLI direct loader::{{{','.join(BANNED)}}} calls: {count} / target 0")

    if args.verbose and hits:
        print()
        for path, lno, line in hits:
            rel = path.relative_to(ROOT)
            print(f"  {rel}:{lno}  {line[:120]}")

    if count > 0:
        print(
            "\nFAIL: CLI subcommands must route prelude assembly through "
            "`pipeline::load_full_prelude` (ADR-0050 extension, #1803). "
            "Rerun with --verbose to see the offending lines.",
            file=sys.stderr,
        )
        return 1

    print("✓ OK (all CLI prelude assembly routes through pipeline::load_full_prelude)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
