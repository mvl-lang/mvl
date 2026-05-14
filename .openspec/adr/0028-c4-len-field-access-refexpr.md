# C4 Context: Field-Access Support in len() RefExpr

> Source: #726 — feat: support field-access in len() RefExpr for decreases clauses

## System Context

The MVL compiler's refinement type system uses `RefExpr` AST nodes to represent
predicate expressions in `requires`, `ensures`, `decreases`, and `invariant`
clauses. The `len()` form was restricted to simple identifiers, preventing the
self-hosted parser from expressing its termination measures as formal
`decreases` clauses.

## Containers

| Container | Path | Role |
|-----------|------|------|
| RefExpr parser | `src/mvl/parser/types.rs` | Parses `len(a.b.c)` by consuming dot-chain after ident |
| RefExpr AST | `src/mvl/parser/ast.rs` | `Len { ident: String }` — ident now holds dotted paths |
| Rust emitter | `src/mvl/backends/rust/emit_types.rs` | Emits `a.b.len()` from `ident = "a.b"` (no change needed) |
| Self-hosted parser | `compiler/parser.mvl` | Uses `decreases len(curr.tokens) - curr.pos` on while loops |

## Constraints

Must use the existing MVL RefExpr infrastructure in `src/mvl/parser/types.rs`.
`RefExpr::Len { ident: String, span }` keeps the same shape — `ident` now holds
dotted paths (e.g. `"curr.tokens"`) by consuming dot-chains inside
`parse_ref_atom`. All downstream formatters (`format!("len({ident})")`) and
emitters (`format!("{ident}.len()")`) work correctly without changes.
No AST schema change needed.

## Failure Conditions

- `len(ps.tokens)` fails to parse in a requires/ensures/decreases clause
- Existing `len(simple_ident)` usages break
- `make check-compiler` or `make test-mvl` fail after the changes
- `cargo test` fails after the parser change

## Full Prompt Contract

```
GOAL:
Extend the `len()` expression in RefExpr to accept field-access arguments
(e.g., `len(ps.tokens)`) so that `decreases len(ps.tokens) - curr.pos` can be
written in parser while loops. Currently `len()` only accepts simple identifiers,
not field accesses, so termination proofs for the self-hosted parser must remain
as documentation comments rather than verified decreases clauses.

CONSTRAINTS:
Must use the existing MVL RefExpr infrastructure in `src/mvl/parser/types.rs`.
`RefExpr::Len { ident: String, span }` can keep the same shape — extend `ident`
to hold a dotted path (e.g. "curr.tokens") by consuming dot-chains inside
`parse_ref_atom`. The `emit_ref_expr` function in
`src/mvl/backends/rust/emit_types.rs` already emits `format!("{ident}.len()")`,
which works correctly for dotted paths without changes.

FORMAT:
- `src/mvl/parser/types.rs` — inside `parse_ref_atom`, `len(ident)` arm: after
  `expect_ident()`, add a `while matches!(self.peek_kind(), TokenKind::Dot)`
  loop that consumes `.field` tokens and appends ".{field}" to `ident`.
- `compiler/parser.mvl` — replace doc-comment termination measures with actual
  `decreases len(curr.tokens) - curr.pos` clauses on the while loops.

FAILURE CONDITIONS:
- `len(ps.tokens)` fails to parse in a requires/ensures/decreases clause
- Existing `len(simple_ident)` usages break
- `make check-compiler` or `make test-mvl` fail after the changes
- `cargo test` fails after the parser change
```

---

## Relation to language definition

This change extends the `len()` sub-expression inside refinement predicates (`requires`,
`ensures`, `decreases`, `invariant`) to accept dotted field-access paths (e.g.
`len(curr.tokens)`) in addition to simple identifiers.

### Eleven Requirements (ADR-0001)

The change directly **strengthens** Requirement 6 (Termination / totality): `decreases`
clauses can now express structural measures over struct fields, enabling verified
termination proofs for while-loops in the self-hosted parser. No other requirement is
affected.

### Design Principles (README)

- **Verifiability** — strengthens: more termination measures can be formally expressed
- **Minimal surface area** — consistent with: extends one RefExpr arm, no new syntax
- All other design principles — consistent with (parser change is purely internal)
