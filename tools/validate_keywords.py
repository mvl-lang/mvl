#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
#
# validate_keywords.py — cross-check keyword lists between the two
# lexers that live in this repository (#706):
#
#   1. compiler/lexer.mvl         — self-hosted MVL lexer
#   2. src/mvl/parser/lexer/mod.rs — Rust reference lexer
#
# The formal grammar (docs/grammar.ebnf) and tree-sitter grammar
# (etc/tree-sitter-mvl/grammar.js) have moved to
#   https://github.com/mvl-lang/mvl-spec
# Cross-repo drift between the Rust lexer and those sources is checked
# by mvl-spec's own CI (see #1813).
#
# Usage:
#   python3 tools/validate_keywords.py
#
# Exit code: 0 if the two lists agree, 1 if any divergence is found.

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).parent.parent


# ── Extractors ────────────────────────────────────────────────────────────────

def keywords_from_mvl_lexer(path: Path) -> set[str]:
    """Extract keywords from compiler/lexer.mvl keyword_kind() match arms.

    Format:  "kw" => TokenKind::KwXxx,
    """
    text = path.read_text()
    return set(re.findall(r'"([a-zA-Z][a-zA-Z_]*)" +=> TokenKind::Kw', text))


def keywords_from_rust_lexer(path: Path) -> set[str]:
    """Extract keywords from src/mvl/parser/lexer/mod.rs keyword mapping.

    Format:  "kw" => TokenKind::Xxx,
    Only in the keyword-dispatch section (exclude Display impl etc.).
    """
    text = path.read_text()
    return set(re.findall(r'"([a-zA-Z][a-zA-Z_]*)" => TokenKind::', text))


# ── Main ──────────────────────────────────────────────────────────────────────

def main() -> int:
    mvl_path  = REPO_ROOT / "compiler" / "lexer.mvl"
    rust_path = REPO_ROOT / "src" / "mvl" / "parser" / "lexer" / "mod.rs"

    mvl  = keywords_from_mvl_lexer(mvl_path)
    rust = keywords_from_rust_lexer(rust_path)

    # The Rust lexer is the ground truth — compare compiler/lexer.mvl against it.
    errors: list[str] = []

    missing_in_mvl = rust - mvl
    extra_in_mvl   = mvl - rust
    if missing_in_mvl:
        errors.append(
            f"compiler/lexer.mvl is missing keywords: {sorted(missing_in_mvl)}"
        )
    if extra_in_mvl:
        errors.append(
            f"compiler/lexer.mvl has extra keywords not in Rust lexer: {sorted(extra_in_mvl)}"
        )

    if errors:
        print("FAIL: keyword divergence detected\n")
        for e in errors:
            print(f"  {e}")
        print(
            "\nFix: update compiler/lexer.mvl to match src/mvl/parser/lexer/mod.rs."
        )
        return 1

    print(f"OK: all {len(rust)} keywords consistent across compiler/lexer.mvl and Rust lexer.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
