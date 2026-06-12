# ADR-0045: Self-Hosting Phase 3 — Parser Recursive Type Resolution

**Status:** Accepted
**Date:** 2026-06-12
**Issues:** #1116 (Phase 3 parser), #1113 (self-hosting epic)
**Related:** ADR-0044 (self-hosting TIR-first strategy)

---

## Context

Phase 3 of the self-hosting epic (#1116) required porting the MVL expression
and statement parser to MVL itself (`compiler/parser.mvl`). Several design
decisions arose that aren't covered by the general strategy in ADR-0044.

---

## Decision 1: `List[T]` for Recursive Fields (Not `Box[T]`)

### Problem

The parser-stage `Expr` enum and its helper structs are mutually recursive:
`AstBinaryExpr.left: Expr` → `Expr::Binary(AstBinaryExpr)` → `Expr`.
In Rust, this produces E0072 (infinite size) unless the recursive field is
behind a pointer (`Box<T>`, `Rc<T>`, etc.).

MVL's type checker allows direct recursive types without `Box` — it performs
heap-allocation automatically.  But the `make test-mvl` pre-commit hook
compiles `compiler/*.mvl` to Rust via the Rust backend, and the generated
Rust code rejects infinite-size types.

### Options Considered

| Option | MVL type-checks | Compiles to Rust | Can construct in MVL |
|--------|----------------|------------------|----------------------|
| `Box[T]` in type, explicit `Box::new(x)` | ✓ | ✓ | ✗ — no Box constructor |
| `Box[T]` in type, backend auto-boxes | ✓ | ✓ | Unclear — backend doesn't auto-box |
| `List[T]` single-element list | ✓ | ✓ (Vec<T> is heap) | ✓ — `[expr]` literal |

### Decision

Use `List[T]` (= Rust `Vec<T>`) with exactly one element as the indirection
for all directly-recursive fields.  Convention:

- **Construction:** `box_expr(e)` → `[e]` (single-element list literal)
- **Access:** `unbox_expr(cell)` → `cell.get(0).unwrap_or(empty_expr())`
- **Comments:** fields are annotated with `// [1]` (always 1 element) or
  `// [0,1]` (optional — 0 = absent, 1 = present)

**Why Vec works:** Rust's `Vec<T>` is always heap-allocated regardless of `T`'s
size.  A `Vec<Expr>` with one element satisfies Rust's `Sized` requirement even
when `Expr` is large, breaking the infinite-size cycle.

### Consequence

Field accesses use `unbox_expr(field)` rather than `field` directly.  This
is more verbose than `Box<T>` field access but is the only approach that:
1. MVL type-checker accepts
2. MVL Rust backend emits correct Rust for
3. Can be constructed with standard MVL syntax (`[e]`)

Future: if the MVL compiler adds `Box::new(e)` as a first-class operation,
migrate `List[T]` fields to `Box[T]` in a follow-up PR.

---

## Decision 2: Struct Literal Disambiguation by Name Case

### Problem

In expression context, `Name { field: val }` is ambiguous:
- `Point { x: 0, y: 0 }` — struct literal (type/constructor name, uppercase)
- `match n { arm }` — `n` is lowercase; `{` belongs to `match`, not to `n`
- `for x in xs { body }` — `xs` is lowercase; `{` belongs to `for`

Without disambiguation, `parse_primary` would interpret `n { arm }` as a
struct literal and produce cascading parse errors.

### Decision

Apply the conventional Rust/MVL naming rule: **type and constructor names
start with uppercase; value-binding names start with lowercase.**

In `parse_primary`, when an `Ident` is followed by `{`:
- First char uppercase → parse as struct literal
- First char lowercase → return bare Ident; `{` consumed by outer syntax

```mvl
let first_ch: String = name.char_at(0).unwrap_or("");
let name_is_upper: Bool = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".contains(first_ch);
if ps_at_punct(curr, "{") && name_is_upper { /* struct literal */ }
else { ExprR { value: Expr::Ident(name, sp), ps: curr } }
```

### Consequence

Code like `some_fn { field: val }` would not be parsed as a struct literal.
In practice, function names are lowercase and type names are uppercase in MVL,
so this is correct for all current corpus programs.  Edge cases (single-letter
uppercase variable names) are unlikely given MVL naming conventions.

---

## Decision 3: `AstLiteral` vs `Literal` for Parser-Stage Literals

### Problem

`tir.mvl` defines `Literal::Character(Char)` using MVL's `Char` primitive.
No stdlib function converts a `String` to `Char`, so the self-hosted parser
cannot construct `Literal::Character` values from the char lexeme string.

### Decision

Introduce a separate `AstLiteral` enum for the parser stage:

```mvl
pub type AstLiteral = enum {
    Integer(Int), Floating(Float), Str(String),
    Char(String),   // raw single-char lexeme; Char primitive not constructible
    Bool(Bool), Unit,
}
```

`AstLiteral::Char(String)` stores the raw lexeme (e.g. `"a"` for `'a'`).
The checker stage (Phase 4) will convert `AstLiteral::Char(String)` to the
resolved `Ty::Char` value when it has access to the runtime.

### Consequence

The parser-stage AST uses `AstLiteral` not `Literal` for expression literals.
A `Pattern::Literal(AstLiteral, Span)` carries the same type.  This is a
deliberate divergence from the Rust `ast.rs` representation and will be
reconciled when `String → Char` conversion is available in MVL stdlib.

---

## Decision 4: Struct Enum Variants Are Not Constructible

### Observation

MVL struct enum variants (`Foo::Bar { x: 1, y: 2 }`) cannot be constructed
in MVL code at this time (the compiler reports REQ1: undefined type `Foo::Bar`).
Only tuple variants (`Foo::Baz(42)`) and unit variants (`Foo::Qux`) work.

### Decision

All `Expr`, `Stmt`, and `Pattern` enum variants use the tuple-variant form with
helper structs as payloads.  Example:

```mvl
// WRONG (struct variant — not constructible)
pub type Expr = enum {
    Binary { op: BinaryOp, left: Expr, right: Expr, span: Span },
}

// CORRECT (tuple variant with helper struct)
pub type AstBinaryExpr = struct { op: BinaryOp, left: List[Expr], right: List[Expr], span: Span }
pub type Expr = enum {
    Binary(AstBinaryExpr),
}
```

This limitation is tracked and should be fixed in the MVL compiler; the struct
variant restriction is not fundamental to the language design.

---

## Implementation Notes

- `compiler/parser.mvl` adds `box_expr`, `unbox_expr`, `box_pat`, `unbox_pat`
  as utility functions
- All `List[T]` single-element fields are documented with `// [1]` or `// [0,1]`
  comments in `compiler/tir.mvl`
- `Block.tail_expr: List[Expr]` replaces `Option[Expr]` to avoid a separate
  MVL limitation (`ref Option[T] = None` causes a lifetime error)

## References

- Issue #1116: Phase 3 implementation
- PR #1360: Implementation
- ADR-0044: General self-hosting strategy
