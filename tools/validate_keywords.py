#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
#
# validate_keywords.py — cross-check keyword lists from four sources (#706):
#
#   1. docs/grammar.ebnf          — formal grammar (Reserved Keywords section)
#   2. etc/tree-sitter-mvl/grammar.js — tree-sitter grammar
#   3. compiler/lexer.mvl         — self-hosted MVL lexer
#   4. src/mvl/parser/lexer.rs    — Rust reference lexer
#
# Usage:
#   python3 tools/validate_keywords.py
#
# Exit code: 0 if all four lists agree, 1 if any divergence is found.

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).parent.parent


# ── Extractors ────────────────────────────────────────────────────────────────

def keywords_from_ebnf(path: Path) -> set[str]:
    """Extract reserved keywords from the (* === Reserved Keywords === *) block.

    The block uses labelled comment lines:
        (* Declaration:     fn  type  const  use  pub  extern  impl  builtin  *)
    We take every word that appears after the colon on such lines, until the
    (* === Lexical === *) section starts.
    """
    text = path.read_text()
    start = text.find("=== Reserved Keywords ===")
    if start == -1:
        raise ValueError(f"{path}: could not find Reserved Keywords section")
    end = text.find("=== Lexical ===", start)
    if end == -1:
        raise ValueError(f"{path}: could not find Lexical section after Reserved Keywords")
    section = text[start:end]
    keywords: set[str] = set()
    for line in section.splitlines():
        # Category line:     (* Label:   kw1  kw2  kw3  *)
        # Continuation line: (*          kw1  kw2        *)
        m = re.match(r"\(\*\s+(?:[A-Za-z /]+:)?\s+(.*?)\s*\*\)", line)
        if m:
            words = re.findall(r"[a-zA-Z][a-zA-Z_]*", m.group(1))
            keywords.update(words)
    # Exclude terms that are in the EBNF comment but are NOT lexer reserved words:
    # - Pattern constructors (Some, None, Ok, Err) — matched by name, not lexed as keywords
    # - Spec-only refinement terms (self, old) — not in Rust TokenKind
    keywords -= {"Some", "None", "Ok", "Err", "self", "old"}
    return keywords


def keywords_from_grammar_js(path: Path) -> set[str]:
    """Extract all double-quoted lowercase keyword strings used as rule bodies.

    We look for string literals that appear in rule positions (i.e. sequences,
    choices, repeat bodies) and filter to those that look like reserved words
    (all alpha, length >= 2).  We deliberately exclude operator strings like
    "=>" and structural tokens like "{" / "}".
    """
    text = path.read_text()
    candidates = re.findall(r'"([a-zA-Z][a-zA-Z_]*)"', text)
    # Only keep words ≥ 2 chars that appear as bare string tokens in grammar
    # rules (i.e. keywords, not type names or JS identifiers).
    # We exclude uppercase-only and JS method names by requiring lowercase start.
    seen = set()
    for word in candidates:
        if len(word) >= 2:
            seen.add(word)
    return seen


def keywords_from_mvl_lexer(path: Path) -> set[str]:
    """Extract keywords from compiler/lexer.mvl keyword_kind() match arms.

    Format:  "kw" => TokenKind::KwXxx,
    """
    text = path.read_text()
    return set(re.findall(r'"([a-zA-Z][a-zA-Z_]*)" +=> TokenKind::Kw', text))


def keywords_from_rust_lexer(path: Path) -> set[str]:
    """Extract keywords from src/mvl/parser/lexer.rs keyword mapping.

    Format:  "kw" => TokenKind::Xxx,
    Only in the keyword-dispatch section (exclude Display impl etc.).
    """
    text = path.read_text()
    return set(re.findall(r'"([a-zA-Z][a-zA-Z_]*)" => TokenKind::', text))


# ── Main ──────────────────────────────────────────────────────────────────────

def main() -> int:
    ebnf_path = REPO_ROOT / "docs" / "grammar.ebnf"
    ts_path   = REPO_ROOT / "etc" / "tree-sitter-mvl" / "grammar.js"
    mvl_path  = REPO_ROOT / "compiler" / "lexer.mvl"
    rust_path = REPO_ROOT / "src" / "mvl" / "parser" / "lexer.rs"

    ebnf = keywords_from_ebnf(ebnf_path)
    ts   = keywords_from_grammar_js(ts_path)
    mvl  = keywords_from_mvl_lexer(mvl_path)
    rust = keywords_from_rust_lexer(rust_path)

    # The Rust lexer is the ground truth — compare the others against it.
    # Tree-sitter grammar includes many extra strings (node names, operators)
    # so we check that all Rust keywords appear in the tree-sitter grammar,
    # not that the sets are equal.
    errors: list[str] = []

    # 1. EBNF vs Rust
    missing_in_ebnf = rust - ebnf
    extra_in_ebnf   = ebnf - rust
    if missing_in_ebnf:
        errors.append(
            f"docs/grammar.ebnf is missing keywords: {sorted(missing_in_ebnf)}"
        )
    if extra_in_ebnf:
        errors.append(
            f"docs/grammar.ebnf has extra keywords not in Rust lexer: {sorted(extra_in_ebnf)}"
        )

    # 2. compiler/lexer.mvl vs Rust
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

    # 3. tree-sitter: every Rust keyword must appear somewhere in grammar.js
    missing_in_ts = rust - ts
    if missing_in_ts:
        errors.append(
            f"etc/tree-sitter-mvl/grammar.js is missing keywords: {sorted(missing_in_ts)}"
        )

    if errors:
        print("FAIL: keyword divergence detected\n")
        for e in errors:
            print(f"  {e}")
        print(
            "\nFix: update the diverging files to match src/mvl/parser/lexer.rs."
        )
        return 1

    print(f"OK: all {len(rust)} keywords consistent across 4 sources.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
