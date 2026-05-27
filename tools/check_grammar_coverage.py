#!/usr/bin/env python3
"""
check_grammar_coverage.py — Cross-validate docs/grammar.ebnf against
etc/tree-sitter-mvl/grammar.js.

Extracts all lowercase production rule names from the EBNF and all
rule names from the tree-sitter grammar, then reports:

  - EBNF rules missing from tree-sitter (likely gaps)
  - Tree-sitter rules not in EBNF (deliberate extensions or renames)

Exit codes:
  0 — no unexpected gaps or unknown extensions
  1 — unexpected gaps found (rules in EBNF with no ts counterpart)
      OR unknown tree-sitter extensions (ts rules not in EBNF or TS_KNOWN_EXTENSIONS)
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).parent.parent

EBNF_PATH = ROOT / "docs" / "grammar.ebnf"
GRAMMAR_JS_PATH = ROOT / "etc" / "tree-sitter-mvl" / "grammar.js"

# ── Known intentional divergences ────────────────────────────────────────────
# Rules that are in the EBNF but deliberately absent/renamed/inlined in
# grammar.js.  Each entry maps the EBNF name to a short reason string.
EBNF_KNOWN_ABSENT = {
    # Inlined into `declaration` — tree-sitter doesn't need the intermediate node
    "decl_body": "inlined into declaration",
    # Inlined as type_body → type_expr branch (alias_type adds no AST node)
    "alias_type": "inlined: type_body uses type_expr directly",
    # Renamed to avoid confusion with security_label
    "security": "renamed to security_modifier in grammar.js",
    # Inlined as prec.left branches inside expr
    "propagate": "inlined into expr (postfix ?)",
    "unary_expr": "inlined into expr (prec.right branches)",
    "unary_op": "inlined into expr (unary operator literals)",
    "binary_expr": "inlined into expr (prec.left branches)",
    "binary_op": "inlined into expr (binary operator literals)",
    "method_call": "inlined into expr (method call branch)",
    "field_access": "inlined into expr (field access branch)",
    # Renamed with _expr suffix for clarity
    "fn_call": "renamed to fn_call_expr",
    "lambda": "renamed to lambda_expr",
    "construct": "renamed to construct_expr",
    # lvalue is used in assign_stmt but maps to expr in tree-sitter
    "lvalue": "inlined: assign_stmt uses expr as lvalue",
    # map_literal and set_literal not yet implemented in grammar.js
    "map_literal": "not yet implemented in tree-sitter grammar",
    "set_literal": "not yet implemented in tree-sitter grammar",
    # borrow_expr and impl_decl added to EBNF (hardening #384); ts update pending
    "borrow_expr": "not yet implemented in tree-sitter grammar",
    "impl_decl": "not yet implemented in tree-sitter grammar",
    # fn_contract covers both requires/ensures and ghost let as a unit;
    # tree-sitter splits it into contract_clause + ghost_let_stmt instead
    "fn_contract": "split in tree-sitter into contract_clause and ghost_let_stmt",
    # guard_expr = expr — inlined into match_arm as optional(seq("if", $.expr))
    "guard_expr": "inlined into match_arm as optional(seq(\"if\", $.expr))",
    # Uppercase EBNF terminals map to regex patterns, not named rules
    "COMMENT": "terminal — mapped to line_comment regex",
    "DOC_COMMENT": "terminal — mapped to line_comment regex (prefix ///)",
    "IDENT": "terminal — mapped to identifier regex",
    "INTEGER": "terminal — mapped to integer_literal regex",
    "FLOAT": "terminal — mapped to float_literal regex",
    "STRING": "terminal — mapped to string_literal regex",
    "CHAR": "terminal — mapped to char_literal regex",
    "ALPHA": "terminal — character class, no named rule needed",
    "DIGIT": "terminal — character class, no named rule needed",
    "ANY_CHAR": "terminal — character class, no named rule needed",
    "NEWLINE": "terminal — handled by extras whitespace",
    "CHAR_ELEM": "terminal — inlined into string/char regex",
}

# Rules that exist in grammar.js but not in EBNF — deliberate extensions.
# These are recorded here so the report is clear about what's intentional.
TS_KNOWN_EXTENSIONS = {
    # Split from expr for clarity
    "fn_call_expr",
    "construct_expr",
    "lambda_expr",
    "declassify_expr",
    "relabel_expr",
    "sanitize_expr",
    "grouped_expr",
    "block_expr",
    "path_expr",
    # Private/inline rules (start with _)
    "_atom_expr",
    # Pattern sub-rules (split from the single `pattern` production)
    "constructor_pattern",
    "struct_pattern",
    "tuple_pattern",
    "some_pattern",
    "none_pattern",
    "ok_pattern",
    "err_pattern",
    # Renamed from security
    "security_modifier",
    # Literal sub-rules (EBNF uses uppercase terminals)
    "integer_literal",
    "float_literal",
    "string_literal",          # ← STRING (single-line)
    "multiline_string_literal",  # ← STRING (multiline form)
    "raw_string_literal",        # ← STRING (raw single-line form)
    "raw_multiline_string_literal",  # ← STRING (raw multiline form)
    "char_literal",
    "boolean_literal",
    # Renamed from uppercase EBNF terminals
    "line_comment",   # ← COMMENT
    "identifier",     # ← IDENT
    # Extensions not yet in EBNF (added to grammar.js, EBNF to follow)
    "module_decl",
    "extern_decl",
    "extern_fn_decl",
    # Session type support (#37, #134): BMC for protocol deadlocks
    "session_branch",
    "session_branches",
    "session_external_choice",
    "session_internal_choice",
    "session_op",
    "session_receive_type",
    "session_send_type",
    # Structured concurrency scope (Phase 8, #69) — EBNF to follow
    "concurrently_expr",
}


def extract_ebnf_rules(path: Path) -> set[str]:
    """Return all production rule names defined in the EBNF file.

    Matches lines of the form:  name  =  ... ;
    where name starts at column 0 (possibly with leading whitespace).
    """
    pattern = re.compile(r"^([a-zA-Z_][a-zA-Z0-9_]*)\s*=")
    rules: set[str] = set()
    for line in path.read_text().splitlines():
        m = pattern.match(line)
        if m:
            rules.add(m.group(1))
    return rules


def extract_ts_rules(path: Path) -> set[str]:
    """Return all rule names defined in the tree-sitter grammar.js rules block.

    Matches lines of the form:   rulename: ($) =>   or   rulename: ($, ...
    """
    pattern = re.compile(r"^\s+([a-zA-Z_][a-zA-Z0-9_]*):\s*\(")
    rules: set[str] = set()
    in_rules = False
    for line in path.read_text().splitlines():
        if "rules: {" in line:
            in_rules = True
            continue
        if in_rules and line.strip() == "},":
            break
        if in_rules:
            m = pattern.match(line)
            if m:
                rules.add(m.group(1))
    return rules


def main() -> int:
    ebnf_rules = extract_ebnf_rules(EBNF_PATH)
    ts_rules = extract_ts_rules(GRAMMAR_JS_PATH)

    # ── EBNF rules not in tree-sitter ────────────────────────────────────────
    missing_from_ts = ebnf_rules - ts_rules
    unexpected_gaps = missing_from_ts - set(EBNF_KNOWN_ABSENT.keys())
    known_gaps = missing_from_ts & set(EBNF_KNOWN_ABSENT.keys())

    # ── Tree-sitter rules not in EBNF ────────────────────────────────────────
    extra_in_ts = ts_rules - ebnf_rules
    unknown_extensions = extra_in_ts - TS_KNOWN_EXTENSIONS
    known_extensions = extra_in_ts & TS_KNOWN_EXTENSIONS

    # ── Report ───────────────────────────────────────────────────────────────
    print(f"EBNF productions:        {len(ebnf_rules):3d}")
    print(f"Tree-sitter rules:       {len(ts_rules):3d}")
    print()

    if unexpected_gaps:
        print("❌  EBNF rules with no tree-sitter counterpart (unexpected):")
        for name in sorted(unexpected_gaps):
            print(f"     {name}")
        print()
    else:
        print("✅  No unexpected gaps — all EBNF rules are covered or documented.")
        print()

    if known_gaps:
        print("ℹ️   Known intentional absences in tree-sitter (documented):")
        for name in sorted(known_gaps):
            reason = EBNF_KNOWN_ABSENT[name]
            print(f"     {name:<20s}  {reason}")
        print()

    if unknown_extensions:
        print("❌  Tree-sitter rules not in EBNF and not in TS_KNOWN_EXTENSIONS:")
        for name in sorted(unknown_extensions):
            print(f"     {name}")
        print()

    if known_extensions:
        print(f"ℹ️   Known tree-sitter extensions not in EBNF: {len(known_extensions)}"
              f" (see TS_KNOWN_EXTENSIONS in this script)")
        print()

    if unexpected_gaps or unknown_extensions:
        if unexpected_gaps:
            print("RESULT: FAIL — add the missing rules to grammar.js or document them"
                  " in EBNF_KNOWN_ABSENT.")
        if unknown_extensions:
            print("RESULT: FAIL — add the new tree-sitter rules to the EBNF or document"
                  " them in TS_KNOWN_EXTENSIONS.")
        return 1

    print("RESULT: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
