#!/usr/bin/env python3
"""ADR structure checker — validates new ADRs contain the required sections.

Checks:
1. Every ADR numbered >= 0017 (first retroactively updated) has a
   "## Relation to language definition" section.
2. No two ADR files share the same four-digit number (duplicate detection).

Usage:
    python3 tools/check_adr.py          # report + exit code
    python3 tools/check_adr.py --verbose # show pass/fail per file
"""

import argparse
import re
import sys
from pathlib import Path

ADR_DIR = Path(__file__).parent.parent / ".openspec" / "adr"

REQUIRED_SECTION = "## Relation to language definition"

# ADRs written before this requirement was introduced (all ADRs at the time of #429).
# ADR-0017 is exempt from this list because it was retroactively updated as the worked example.
# Update remaining ADRs on next touch; do not backfill en masse.
LEGACY_EXEMPT = {
    "0001", "0002", "0003", "0004", "0005",
    "0006", "0007", "0009", "0010", "0012",
    "0013", "0014", "0015", "0016",
    "0018", "0019", "0020", "0021",
}

ADR_PATTERN = re.compile(r"^(\d{4})-")

# Files in the ADR directory that are not ADR decision records.
NON_ADR_FILES = {"template.md", "README.md", "index.md"}


def collect_adrs():
    adrs = {}
    for path in sorted(ADR_DIR.glob("*.md")):
        if path.name in NON_ADR_FILES:
            continue
        m = ADR_PATTERN.match(path.name)
        if not m:
            continue
        number = m.group(1)
        adrs.setdefault(number, []).append(path)
    return adrs


def check(verbose: bool) -> bool:
    adrs = collect_adrs()
    ok = True

    # Duplicate number check
    for number, paths in adrs.items():
        if len(paths) > 1:
            names = ", ".join(p.name for p in paths)
            print(f"  FAIL  duplicate ADR number {number}: {names}")
            ok = False

    # Section presence check
    for number, paths in adrs.items():
        if number in LEGACY_EXEMPT:
            if verbose:
                print(f"  SKIP  {paths[0].name}  (legacy exempt)")
            continue
        for path in paths:
            text = path.read_text()
            if REQUIRED_SECTION not in text:
                print(f"  FAIL  {path.name}  missing '{REQUIRED_SECTION}'")
                ok = False
            elif verbose:
                print(f"  OK    {path.name}")

    return ok


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    passed = check(args.verbose)
    if not passed:
        print("\nAdd the required section to failing ADRs.")
        print("See .openspec/adr/template.md for the expected structure.")
        sys.exit(1)

    print("ADR check passed.")


if __name__ == "__main__":
    main()
