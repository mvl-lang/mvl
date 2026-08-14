#!/usr/bin/env python3
"""Spec link resolver — every path an OpenSpec requirement names as evidence
must actually exist, and every `file::symbol` must actually be declared.

Why this exists
---------------
`tools/assurance.py` reports completeness and scenario-weighted coverage, but
it only ever existence-checks the **first** `**Implementation:**` backtick of
each requirement. `**Tests:**` and `**Corpus:**` targets are *counted*, never
resolved. That is how 90 of 259 distinct cited paths came to be dead — a
corpus renumbering (`01_syntax` -> `01_expressions`, `05_effects` ->
`06_effects`, `tests/corpus/negative/req09_data_race/` -> `tests/negative/
req09/`) and a batch of test renames that the specs never followed, with the
assurance dashboard still reporting 99% completeness throughout.

A spec that names evidence which does not exist is worse than one that names
none: it reads as covered. This closes that hole permanently.

What it checks
--------------
For every `**Implementation:**`, `**Tests:**` and `**Corpus:**` marker in
`.openspec/specs/**/spec.md`:

1. the file (or directory) part of each backticked path exists;
2. for `path::symbol` refs, that `symbol` is declared somewhere in that file
   (`fn symbol`, `struct/enum/type symbol`, ...). Symbol checking is a
   substring search on a few declaration shapes, deliberately lenient — the
   goal is catching renames and deletions, not parsing four languages.

Refs annotated `(planned)` on the same marker line are skipped: those are
honest forward declarations, not rot.

Usage
-----
    python3 tools/audit_spec_links.py            # summary + exit code
    python3 tools/audit_spec_links.py --verbose  # list every broken ref
    python3 tools/audit_spec_links.py --budget N # allow N known-broken refs
"""

import argparse
import re
import sys
from pathlib import Path

REPO = Path(__file__).parent.parent
SPEC_DIR = REPO / ".openspec" / "specs"

MARKER_RE = re.compile(
    r"^\*\*(Implementation|Tests|Corpus|Primary implementation|"
    r"Self-hosted implementation):\*\*\s*(.+)$",
    re.MULTILINE,
)
BACKTICK_RE = re.compile(r"`([^`]+)`")

# A ref is exempt when its own marker line says so. Specs use several
# spellings; keep this list tight so it cannot silently swallow real rot.
PLANNED_RE = re.compile(r"\(planned[^)]*\)|—\s*deferred|\(deferred[^)]*\)", re.IGNORECASE)

# Only treat a backticked token as a path if it looks like one. Spec prose
# backticks type names, keywords and diagnostics constantly.
PATH_HINT_RE = re.compile(r"^(src|tests|std|compiler|examples|runtime|tools|etc|vendor)/")

# Specs legitimately cite a *family* of files as shorthand — `std/*.mvl`,
# `src/cli/{check,build,run,...}.rs`, `src/mvl/{parser,checker}/`. These are
# illustrative, not resolvable, and flagging them is noise that would push
# maintainers to raise the budget rather than fix real rot.
GLOB_RE = re.compile(r"[*{}?]|\.\.\.")


def declares(haystack: str, symbol: str) -> bool:
    """Is `symbol` declared in this file's text?

    Covers Rust (`fn`, `struct`, `enum`, `trait`, `impl`, `type`, `const`,
    `macro_rules!`) and MVL (`fn`, `total fn`, `partial fn`, `test fn`,
    `type`, `effect`, `actor`). Lenient by design.
    """
    for kw in (
        "fn ", "struct ", "enum ", "trait ", "impl ", "type ", "const ",
        "static ", "effect ", "actor ", "macro_rules! ", "mod ",
    ):
        if f"{kw}{symbol}" in haystack:
            return True
    # Method-position refs like `Foo::bar` cited as `file::bar`.
    return f"::{symbol}" in haystack or f"{symbol}(" in haystack


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--verbose", action="store_true")
    ap.add_argument("--budget", type=int, default=0,
                    help="tolerate up to N broken refs (for staged paydown)")
    args = ap.parse_args()

    broken: list[tuple[str, str, str]] = []   # (spec, ref, reason)
    checked = 0
    skipped_planned = 0

    for spec in sorted(SPEC_DIR.glob("*/spec.md")):
        text = spec.read_text(encoding="utf-8")
        rel_spec = spec.relative_to(REPO)
        for marker, payload in MARKER_RE.findall(text):
            planned = bool(PLANNED_RE.search(payload))
            for ref in BACKTICK_RE.findall(payload):
                ref = ref.strip()
                if not PATH_HINT_RE.match(ref) or GLOB_RE.search(ref):
                    continue
                if planned:
                    skipped_planned += 1
                    continue
                checked += 1
                path_part, _, symbol = ref.partition("::")
                target = REPO / path_part.strip()
                if not target.exists():
                    broken.append((str(rel_spec), ref, "path does not exist"))
                    continue
                if symbol and target.is_file():
                    try:
                        body = target.read_text(encoding="utf-8", errors="replace")
                    except OSError:
                        continue
                    if not declares(body, symbol.strip()):
                        broken.append((str(rel_spec), ref, "symbol not declared in file"))

    print(f"Spec link audit: {checked} refs checked, "
          f"{skipped_planned} skipped as planned/deferred")
    if broken:
        if args.verbose:
            current = None
            for spec, ref, why in broken:
                if spec != current:
                    print(f"\n  {spec}")
                    current = spec
                print(f"    {ref}  — {why}")
            print()
        print(f"BROKEN spec links: {len(broken)} / budget {args.budget}")
    else:
        print(f"BROKEN spec links: 0 / budget {args.budget}")

    if len(broken) > args.budget:
        print(
            f"\nFAIL: {len(broken)} broken refs exceeds budget {args.budget}.\n"
            "A spec naming evidence that does not exist reads as covered when it\n"
            "is not. Fix the ref, or lower the claim — do not raise the budget\n"
            "without saying why in the same commit.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
