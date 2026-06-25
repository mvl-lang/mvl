# ADR-0048: AST Visit trait and exhaustive walkers

**Status:** Accepted
**Date:** 2026-06-25
**Related:** ADR-0018 (five-stage pipeline — passes module), ADR-0030 (Rust coding conventions)
**Issues:** #1090 (initial walker consolidation), #1443 (Visit trait introduction), #1522 (checker walker consolidation), #1527 (walk_stmt silent skipping fix)

---

## Context

The compiler traverses the AST in many places: ~6 linter rules, mcdc analysis and transform, complexity analysis, checker passes (memory-safety counts, handling counts, contract checks, IFC flow-site counts), and forthcoming passes. Before #1090 and #1443, each of these wrote its own recursive `match expr { … }` triple over `Block`/`Stmt`/`Expr` — ~17 variants of `Expr` and ~8 of `Stmt`, each enumerated by hand.

Three failure modes followed from this duplication:

1. **Drift on AST growth.** When a new `Expr` variant landed, every hand-written walker needed an update. Rust's exhaustiveness check caught the *variants*, but not the *recursion behavior* — easy to forget to descend into a sub-expression and silently under-count.
2. **Inconsistent traversal scope.** Different walkers chose different sub-expressions to visit. Three walkers in `passes.rs` and `contracts.rs` had subtly different policies on whether to descend into `while`/`for` invariants, `decreases` clauses, and patterns — with no documentation explaining why.
3. **Hidden contract-clause skipping.** PR #1443 introduced a shared `Visit` trait whose default `walk_stmt` used `..` patterns and silently dropped `Stmt::While.invariants`, `Stmt::While.decreases`, and `Stmt::For.invariants`. Every consumer of the default — including the totality linter's `CallsFn` predicate — under-detected calls inside contract clauses. The bug surfaced during #1522 and was fixed in #1527.

PR #1522 consolidated the major hand-written walkers in `src/mvl/checker/` onto `Visit`, removing ~300 lines of duplicated traversal logic. #1527 then fixed the underlying silent-skip bug and removed the workaround `visit_stmt` overrides that #1522 had added to compensate.

This ADR captures the resulting design and locks in the convention so it survives future readers and refactors.

---

## Decision

### 1. `parser::visit::Visit` is the canonical AST traversal API.

Any code that walks `Block` / `Stmt` / `Expr` MUST do so by implementing `Visit` and calling `walk_block` / `walk_stmt` / `walk_expr` for recursion, rather than writing its own match-on-variants triple.

The trait has the shape:

```rust
pub trait Visit<'a> {
    fn visit_block(&mut self, b: &'a Block)   { walk_block(self, b); }
    fn visit_stmt(&mut self, s: &'a Stmt)     { walk_stmt(self, s); }
    fn visit_expr(&mut self, e: &'a Expr)     { walk_expr(self, e); }
}
```

Visitors override the methods they care about; the default delegates to the shared walker. Short-circuiting (e.g. "stop on first match") is achieved by *not* calling the walker. Pre-order vs post-order is the visitor's choice — pre-order is the natural default; post-order is achieved by calling `walk_*` before the visitor's own logic.

Hand-written `match expr { Expr::FnCall { args, .. } => … }` recursion is acceptable only when the traversal is genuinely narrow and bounded (e.g. inspecting a single `RefExpr` for parameter references). Default to `Visit` and justify any exception inline.

### 2. Walkers MUST be exhaustive: no `..` in struct-variant patterns.

Every struct-variant pattern in `walk_block` / `walk_stmt` / `walk_expr` binds every field explicitly. Fields the walker does not descend into are bound as `field: _`. The `..` shorthand is forbidden in walker bodies.

Example:

```rust
// CORRECT — explicit
Stmt::While { cond, invariants, decreases, body, span: _ } => {
    v.visit_expr(cond);
    for inv in invariants { v.visit_expr(inv); }
    if let Some(dec) = decreases { v.visit_expr(dec); }
    v.visit_block(body);
}

// FORBIDDEN — `..` silently drops sub-expressions when AST grows
Stmt::While { cond, body, .. } => {
    v.visit_expr(cond);
    v.visit_block(body);
}
```

Rationale: when the AST gains a new sub-expression field, the match must stop compiling. "Should the generic walker descend into this?" becomes a forced compile-time question rather than a silent default-to-no.

This applies only to the central walkers in `parser::visit` and `ir::visit`. Visitor implementations that override `visit_stmt`/`visit_expr` may use `..` freely — they are not bound by the convention because their narrowness is explicit at the call site.

### 3. Traversal scope includes contract sub-expressions.

`walk_stmt` traverses every `Expr` slot reachable from a `Stmt`, including those that are part of MVL contract syntax:

- `Stmt::While.invariants: Vec<Expr>`
- `Stmt::While.decreases: Option<Box<Expr>>`
- `Stmt::For.invariants: Vec<Expr>`

These are real sub-expressions of the function body, semantically. They reference local variables, can contain function calls, and contribute to the function's overall decision surface. A generic walker that wants to find every call site or every reference to a variable must see them. ADR-0025 (function contracts) defines these clauses as part of the function's specification; this ADR confirms they are also part of the AST traversal surface.

The same principle applies to any future `Stmt` or `Expr` variant that grows contract-position sub-expressions: they are walked by default unless there is a documented reason not to.

### 4. The Visit trait does NOT cross into other AST types.

`walk_expr` does not descend into:

- `Pattern` — patterns are a separate AST type; visitors that care write `count_pattern`-style helpers
- `TypeExpr` — types are separate; visitors that care write helpers
- `RefExpr` (the contract-DSL expression type used by `requires`/`ensures`/match guards) — also separate
- `Span` — leaf metadata, no traversal

A consequence: `MatchArm.guard: Option<RefExpr>` is *not* walked by `walk_expr`. Visitors that need to traverse guard expressions (e.g. the contract checker) do so via the `RefExpr` traversal in `checker::refinements`, not via `Visit`.

If a future refactor extends `Visit` to cover patterns or types, it should be a separate ADR — the trait's current scope (AST `Expr`/`Stmt`/`Block` only) is deliberate.

---

## Consequences

**Easier:**
- A new AST variant requires updating one walker (in `parser::visit`), not seven hand-written copies. The shared walker is the single source of truth.
- New checker passes pick up correct contract-clause traversal automatically; no per-pass workaround needed.
- A new sub-expression slot on an existing variant *cannot* be silently missed — the match fails to compile until the walker is updated.
- Code review of a new `Visit` impl is local: reviewers can assume `walk_*` is correct and focus on the visitor's logic.

**Harder:**
- Walker bodies are slightly more verbose: every struct field is named, even when ignored. Acceptable cost for the compile-time forcing function.
- Visitors that need *narrower* traversal than `walk_stmt` provides (e.g. `ifc::FlowSiteVisitor` which intentionally skips most variants) must override `visit_stmt`/`visit_expr` and re-implement the narrow logic. This is a known pattern; the override pays the cost of opting out.

**Follow-up work:**
- `ifc_propagation::collect_violations_*` (#1526) still uses hand-written walkers because of Lambda-scope env-cloning. A future PR will port these to `Visit` with inline save/restore in `visit_expr`.
- The IR-level `ir::visit::Visit` mirrors this design for `TirBlock`/`TirStmt`/`TirExpr`. It should be audited for the same `..` issue when TIR grows new variants; the convention here applies to both.

---

## Rejected Alternatives

**Keep hand-written walkers per pass.** Rejected. The duplication-induced bug class (drift on AST growth, silent contract skipping) is exactly what #1090, #1443, #1522, and #1527 spent effort to remove. Maintaining N parallel walkers means every AST change costs N edits with no compile-time link between them.

**Use `..` in walkers, accept silent skipping.** Rejected. The cost of explicit `field: _` is one extra word per field. The cost of `..` is invisible incorrectness — exactly the failure mode #1527 documents. The asymmetry is decisive.

**Add a sibling walker (`walk_stmt_full`) for contract-aware passes; keep the narrow default.** Rejected as the primary design but supported as an opt-out via visitor override. Two parallel defaults invites the same drift the consolidation tried to remove. A single canonical walker with the wider default, plus per-visitor override for genuinely-narrow cases (e.g. `ifc::FlowSiteVisitor`), is simpler and harder to misuse.

**Use `#[non_exhaustive]` on `Stmt`/`Expr` themselves.** Rejected. `#[non_exhaustive]` is a public-API tool for crate-external consumers; it would force ad-hoc match arms in every internal site, not just walkers, and break exhaustiveness inference compiler-wide. The targeted fix — explicit field bindings in the walker — does the same job in one file.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

This decision is internal compiler infrastructure and does not directly change the semantics of any of the eleven requirements. It indirectly **strengthens** the requirements that depend on traversal-based checker passes:

- **Req 2 (Memory Safety)** — `count_memory_safety_sites` now sees `consume` calls inside loop contracts. **Strengthened.**
- **Req 4, 5, 6 (Null elimination, Error visibility, Ownership)** — `count_handling_sites` now sees `?` propagate sites and match patterns inside loop contracts. **Strengthened.**
- **Req 7 (Termination)** — the totality linter's `CallsFn` / `HasCalls` predicates now detect recursive calls inside `decreases` clauses; functions with such recursion are correctly flagged. **Strengthened.**
- **All other requirements** — unchanged.

### Design Principles (README)

- **Principle 1 — Explicit over implicit:** **strengthens.** `..` in walkers was an implicit-skip pattern; explicit `field: _` makes the skip visible at the source.
- **Principle 11 — Honest over silent:** **strengthens.** The pre-#1527 walker silently dropped sub-expressions; the new convention guarantees the compiler either visits a field or fails to compile.
- **Principle 4 — Total by default:** **consistent with.** No change to the totality rule itself; the totality linter that *enforces* the rule is now more reliable (see Req 7 above).
- All other principles **consistent with.**

### Specifications

No spec in `.openspec/specs/` is directly affected. The `Visit` trait is internal compiler scaffolding, not language semantics. The contract-clause syntax referenced here (`invariant`, `decreases`) is defined by ADR-0025 (function contracts) and the relevant requirement specs; this ADR only affects how the compiler walks those AST nodes internally.
