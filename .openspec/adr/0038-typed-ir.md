# ADR-0038: Typed Intermediate Representation (TIR)

**Status:** Accepted — Backend migration complete (2026-07-03)
**Date:** 2026-05-28
**Issues:** #1096; migration finished via #1594 (ADR-0050)
**Related:** ADR-0018 (five-stage pipeline), ADR-0027 (multi-backend architecture), ADR-0034 (monomorphization pass), ADR-0050 (backend AST-import audit)

---

## Context

After type checking and monomorphization, both backends receive two separate inputs:

1. **`ast::Program`** — the source AST with syntactic type annotations (`TypeExpr`).
2. **`CheckResult::expr_types: HashMap<Span, Ty>`** — the checker's resolved type for every expression, keyed by source span.

To make codegen decisions (e.g. which runtime dispatch function to call for `.map()` on a `List[Int]` vs a `Map[String, Int]`), backends look up `expr_types[span]` and then import `checker::types::Ty` to match on the result.  This creates a structural coupling:

- Both backends `use crate::mvl::checker::types::Ty` — checker internals leaking into codegen.
- Every expression requires a span-keyed lookup at codegen time — fragile, O(1) but indirect.
- Adding a new backend means implementing the same lookup pattern again.

**`MonoProgram` does not solve this.** Although ADR-0034 introduced `MonoProgram` for
context-sensitive analysis, `MonoFn.decl.body` is the original AST subtree with source
spans intact — body expressions still carry `TypeExpr` annotations (syntactic), not resolved
`Ty` (semantic). To determine the type of any body expression in a `MonoFn`, a consumer
still needs `expr_types` and must apply the function's type-parameter substitution itself.

The problem is not resolvable by annotation: what is needed is an IR where type resolution
has already been performed and the result is embedded at every node.

### When it becomes urgent

- **Phase 9 multi-backend (#615):** Python, Go, WASM backends would each re-implement the
  `expr_types` lookup pattern, creating three more checker-import sites.
- **Backend-independent optimization passes:** Constant folding, dead-code elimination, or
  inlining need a stable typed IR — operating on raw AST requires both AST and CheckResult.
- **Long-term checker independence:** Checker internals (`Ty`, `RefExpr`) should not be
  visible to backends. TIR is the decoupling layer.

---

## Decision

### 1. Introduce `src/mvl/ir.rs` — the Typed IR type layer

A new module `ir` defines the post-checker, post-monomorphization representation:

```
src/mvl/ir.rs         — TIR type definitions (TirExpr, TirFn, TirProgram, …)
src/mvl/ir/lower.rs   — lowering pass: MonoProgram + expr_types → TirProgram
```

The core invariant: **every `TirExpr` node carries its fully-resolved `Ty` inline**.

```rust
pub struct TirExpr {
    pub kind: TirExprKind,
    pub ty: Ty,      // resolved — no lookup needed
    pub span: Span,
}
```

`TirExprKind` mirrors `ast::Expr` variants but without embedded `TypeExpr` annotations:
type information is in `TirExpr::ty`, not spread across variant fields.

```rust
pub enum TirExprKind {
    Literal(Literal),
    Var(String),
    MethodCall { receiver: Box<TirExpr>, method: String, args: Vec<TirExpr> },
    FnCall { name: String, args: Vec<TirExpr> },
    Binary { op: BinaryOp, left: Box<TirExpr>, right: Box<TirExpr> },
    // … all other expression forms
}
```

Similarly, `TirStmt` mirrors `ast::Stmt`, `TirBlock` mirrors `ast::Block`, and `TirFn`
mirrors `FnDecl` — but with resolved `Ty` rather than syntactic `TypeExpr` at every
type position.

`TirFn` records the mangled name and original name from `MonoFn`:

```rust
pub struct TirFn {
    pub name: String,           // mangled (e.g. "map_Int_String")
    pub original_name: String,  // unmangled (e.g. "map")
    pub totality: Option<Totality>,
    pub params: Vec<TirParam>,  // each param carries Ty, not TypeExpr
    pub ret_ty: Ty,
    pub effects: Vec<Effect>,
    pub body: TirBlock,
    pub span: Span,
}
```

`TirProgram` is a flat collection of concrete functions — no generics:

```rust
pub struct TirProgram {
    pub fns: Vec<TirFn>,
}
```

### 2. Lowering pass: `MonoProgram + expr_types → TirProgram`

The lowering function in `ir/lower.rs` accepts the existing `MonoProgram` (ADR-0034)
and the checker's type oracle:

```rust
pub fn lower(mono: &MonoProgram, expr_types: &HashMap<Span, Ty>) -> TirProgram
```

For each `MonoFn` in the program:

1. **Build Ty-level substitution** from the function's `type_subs: TypeSubst`
   (`HashMap<String, TypeExpr>`), converting each `TypeExpr` to `Ty` via `typeexpr_to_ty()`.
2. **Lower parameters and return type** by converting their `TypeExpr` annotations to `Ty`
   and applying the substitution.
3. **Walk the body AST**, and for each `Expr` node:
   - Look up `expr_types[expr.span()]` → raw `Ty` (may contain type-parameter placeholders
     such as `Ty::Named("T", [])` for bodies of generic functions).
   - Apply `substitute_ty(raw_ty, ty_subs)` to resolve any remaining type parameters.
   - Embed the concrete `Ty` into the `TirExpr` wrapper.

This two-step resolution — checker lookup then substitution — is necessary because the
checker operates on the generic AST and assigns type-parameter `Ty`s to body expressions
in generic functions.  The monomorphization substitution makes them concrete.

### 3. Type utilities: `typeexpr_to_ty` and `substitute_ty`

Two internal helpers support the lowering:

**`typeexpr_to_ty(te: &TypeExpr) -> Ty`** — converts a syntactic `TypeExpr` to a semantic
`Ty`.  Known primitive names (`"Int"`, `"Bool"`, `"List"`, `"Map"`, etc.) are mapped
directly.  Unknown names become `Ty::Named(name, args)`, which includes unresolved type
parameters like `"T"` — these are handled by substitution.  Session types fall back to
`Ty::Unknown` (session type resolution happens through `expr_types`, not this path).

**`substitute_ty(ty: &Ty, subs: &HashMap<String, Ty>) -> Ty`** — recursively replaces
`Ty::Named(name, [])` entries that appear in `subs`.  Structural types (`List`, `Map`,
`Option`, `Fn`, `Tuple`, etc.) are recursed into.  Primitive types and `Ty::Session`
are returned unchanged.  The empty-subs fast-path avoids unnecessary cloning for
non-generic functions.

### 4. Pipeline position

```text
parser → resolver → checker → mono → TIR lower → backends
                               ↑         ↑
                          MonoProgram  expr_types
                          (ADR-0034)  (CheckResult)
```

TIR lowering is an explicit, named pass between monomorphization and backends.
It is not embedded in either.

### 5. Backend migration is incremental

Existing backends were **not** changed in this ADR. At the time this ADR landed, both
the Rust and LLVM backends still consumed `ast::Program + CheckResult` directly. The
migration to TIR consumption was tracked in #1096, then finished by the follow-up work
in #1594 (ADR-0050) across two phases:

- **Phase 3b (#1648):** LLVM backend rewritten to walk TIR; AST fallback (`type_of_expr`,
  `expr_types.get()` call sites) deleted.
- **Phase 3c (#1649, #1650):** Rust backend switched to TIR entry point; all
  `use parser::ast` imports and inline qualified paths removed. `TranspileConfig` now
  holds `Vec<TirProgram>` for the prelude.

The incremental strategy let backends be migrated one at a time without breaking the
existing pipeline. **Migration complete** as of 2026-07-03 — see ADR-0050 for the
completion audit.

---

## Consequences

**Positive:**

- **Checker decoupling (goal):** Once backends are migrated, `checker::types::Ty` will no
  longer be imported by backends.  Checker internals are hidden behind the TIR boundary.
- **No per-expression lookups at codegen:** Backends call `expr.ty()` instead of
  `expr_types.get(&expr.span())` — direct field access, no map lookup, no `Option` handling.
- **Single self-contained input:** A `TirProgram` carries all information needed for
  codegen.  New backends receive one input, not two.
- **Type substitution is done once:** Generic type parameters are resolved during lowering,
  not at each codegen call site.  Backends never see `Ty::Named("T", [])`.
- **Stable optimization target:** Future optimization passes (constant folding, inlining)
  can operate on `TirProgram` without needing `expr_types`.

**Negative / trade-offs:**

- **Parallel maintenance during migration (resolved 2026-07-03):** Until backends were
  migrated, the compiler maintained both `ast::Program + CheckResult` and `TirProgram`
  as parallel representations. This trade-off ended with the completion of ADR-0050
  (Phases 3b + 3c). One structural mirror remains as a static invariant: any new
  `ast::Expr` variant must be reflected in `TirExprKind` (the lowering pass must map it).
- **Session type fallback:** `typeexpr_to_ty` returns `Ty::Unknown` for `TypeExpr::Session`
  because the checker's `SessionTy` and the parser's `SessionOp` are structurally different.
  Session-typed expressions are resolved via `expr_types` lookups (the normal path), so the
  fallback is only hit for session-typed parameter annotations — rare in practice.
- **`expr_types` still required:** The lowering pass takes `expr_types` as input; it does
  not eliminate `CheckResult.expr_types`.  The field remains until backends no longer need
  it independently.  Long-term, once all backends consume TIR, `expr_types` may be removed
  from `CheckResult`.

---

## Rejected Alternatives

**Extend `MonoProgram` to embed resolved types.**
`MonoFn.decl` is an `FnDecl` — a parser AST node with `TypeExpr` annotations.
Embedding `Ty` into `FnDecl` would require modifying the parser AST, which must
remain source-faithful.  A separate representation (TIR) is cleaner.

**Pass `expr_types` as an additional argument to backends.**
This is the current approach and the source of the problem. It does not decouple checker
from backends — it just makes the coupling explicit in function signatures. Every new
backend would still import `Ty` and implement the lookup-then-match pattern.

**Per-backend type resolution (each backend converts TypeExpr → Ty independently).**
Each backend would implement its own `TypeExpr → Ty` conversion, duplicating the mapping
logic. This was already happening implicitly (the LLVM backend contains JIT monomorphization
logic that partially duplicates what the mono pass does). TIR centralizes the conversion.

**Wait for Phase 9.**
The type definitions and lowering pass are low-risk infrastructure. Defining TIR now:
(a) validates the design against the real AST, (b) enables incremental backend migration
at any point, and (c) unblocks optimization pass work. The cost of delay is that each
new backend added before TIR adoption must implement the `expr_types` lookup pattern.

---

## Relation to language definition

This ADR defines compiler infrastructure and does not change MVL surface language
semantics. No user-visible behavior changes.

### Eleven Requirements (ADR-0001)

| Req | Requirement | Effect |
|-----|-------------|--------|
| 1 | Type safety | **consistent with** — TIR embeds the same types the checker proved; it cannot introduce new type errors |
| 2–9 | Memory, effects, linearity, deadlock, ownership, purity, termination, data-race | **consistent with** — TIR is a representation change; all semantic information is preserved from the checker |
| 10 | Refinements | **consistent with** — `Ty::Refined` is preserved through TIR; refinement predicates (`RefExpr`) are preserved in `TirExprKind::Quantifier` |
| 11 | IFC | **consistent with** — `Ty::Labeled` is preserved through TIR; `TirExprKind::Relabel` maps directly from `Expr::Relabel` |

### Design Principles (README)

- **Explicit over implicit** — **strengthens**: type information is explicitly embedded in
  every TIR node rather than looked up implicitly via a `Span → Ty` map at codegen time.
- **No hidden costs** — **strengthens**: the lowering pass is a named, inspectable step; the
  cost of type resolution is made visible in the pipeline rather than distributed across
  backend call sites.
- **One way to do each thing** — **strengthens**: there is now one canonical way for a backend
  to get the type of an expression — `expr.ty()` — not two (`expr_types.get(span)` or
  `check_result.type_env.lookup(name)`).
- All other principles — **consistent with**.

### Specifications

No existing specs are affected. TIR is a compiler-internal concern; no MVL surface language
semantics change. The compilation model diagram in `docs/compilation-model.md` shows the
TIR lowering step as an explicit stage between the checker/passes and code emission.
