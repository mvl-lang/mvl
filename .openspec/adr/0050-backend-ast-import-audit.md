# ADR-0050: Backend AST Import Audit — TIR-First Migration Gap Analysis

**Status:** Accepted
**Date:** 2026-06-27
**Issues:** #1594 (audit), #1118 (backends Phase A), #1113 (self-hosting epic)
**Related:** ADR-0044 (TIR-first strategy), ADR-0038 (Typed IR), ADR-0027 (multi-backend)

---

## Context

ADR-0044 established the TIR-first architecture: both backends are structurally
`TirProgram → String` generators.  The migration **landed structurally but not
completely** — both emitters still import and walk AST node types alongside TIR.

This ADR documents the Phase 1 audit: every `use crate::mvl::parser::ast` import
in `src/mvl/backends/`, categorised by migration strategy, so that Phases 2–3
have a clear, agreed-upon plan.

---

## Inventory

### Baseline (2026-06-27)

```
grep -rn 'parser::ast' src/mvl/backends/ | grep 'use ' | wc -l
→ 18 use-statement lines (40 total references across 17 files)
```

A CI guard (`make audit-backend-ast`) records this baseline and will fail when
the count rises above it.  The target is **0**.

### File-by-file breakdown

#### LLVM text backend (11 files)

| File | AST types imported | Category |
|------|--------------------|----------|
| `llvm_text.rs` | `TypeExpr` | (a) re-exported via `crate::mvl::ir` |
| `llvm_text/context.rs` | `ActorDecl, FnDecl, TypeExpr` | (a/b) metadata already in TIR |
| `llvm_text/emit_actors.rs` | `ActorDecl, Expr, MailboxConfig, MailboxPolicy, TypeExpr` | (a/b) see below |
| `llvm_text/emit_closures.rs` | `Block, ElseBranch, Expr, MatchBody, Stmt, TypeExpr` | (a) TIR equivalents exist |
| `llvm_text/emit_construct.rs` | `Expr, MatchArm, MatchBody, Pattern, TypeExpr` | (a) TIR equivalents exist |
| `llvm_text/emit_exprs.rs` | `BinaryOp, Block, Expr, Literal, MatchArm, MatchBody, Pattern, TypeExpr, UnaryOp` | (a) all available in TIR |
| `llvm_text/emit_method_call.rs` | `Expr, TypeExpr` | (a) TIR equivalents exist |
| `llvm_text/emit_mono.rs` | `Expr, FnDecl, Literal, MatchArm, MatchBody, Pattern, TypeExpr` | (a/b) `FnDecl` metadata-only |
| `llvm_text/emit_stmts.rs` | `Block, ElseBranch, Expr, LValue, LetKind, MatchArm, Pattern, Stmt, TypeExpr` | (a) all in TIR or re-exported |
| `llvm_text/emit_types.rs` | `BinaryOp, Expr, Literal, Stmt, TypeExpr, UnaryOp` | (a) all in TIR |
| `llvm_text/emitter.rs` | `Decl, FnDecl, Program, Stmt, TypeBody, TypeExpr, VariantFields` | (b/c) entry point still takes `&Program` |

#### Rust backend (6 files)

| File | AST types imported | Category |
|------|--------------------|----------|
| `rust.rs` | `Decl, Program` | (c) legacy entry points; TIR path already exists |
| `rust/capability_params.rs` | `Capability, Param, TypeExpr` | (a) all re-exported via `crate::mvl::ir` |
| `rust/config.rs` | `Program` | (c) stored in `TranspileConfig.prelude_progs`; removable |
| `rust/emit_types.rs` | `FieldDecl, GenericParam, RefExpr, TypeBody, TypeDecl, TypeExpr, Variant, VariantFields` | (a/b) TIR equivalents: `TirFieldDecl`, `TirTypeBody`, `TirTypeDecl`, `TirVariant`, `TirVariantFields` |
| `rust/emitter.rs` | `BinaryOp, Decl` | (a/c) `BinaryOp` re-exported; `Decl` replaced by TIR iteration |
| `rust/last_use.rs` | `Block, ElseBranch, Expr, MatchBody, Stmt` (aliased as `Ast*`) | (c) AST fallback path; TIR path already present |

---

## Categories

### (a) Import path only — switch `parser::ast::X` → `crate::mvl::ir::X`

These types are already re-exported from `crate::mvl::ir` (line 40–44 of `src/mvl/ir.rs`)
or have a direct TIR equivalent.  No semantic change; only the `use` path changes.

| AST type | TIR form | Re-exported? |
|----------|----------|--------------|
| `BinaryOp` | same | ✅ |
| `UnaryOp` | same | ✅ |
| `Literal` | same | ✅ |
| `Pattern` | same | ✅ |
| `LValue` | same | ✅ |
| `LetKind` | same | ✅ |
| `Capability` | same | ✅ |
| `TypeExpr` | same | ✅ |
| `GenericParam` | same | ✅ |
| `MailboxConfig` | same | ✅ |
| `MailboxPolicy` | same | ✅ |
| `Expr` | `TirExpr` / `TirExprKind` | — switch pattern match |
| `Stmt` | `TirStmt` | — switch pattern match |
| `Block` | `TirBlock` | — switch pattern match |
| `MatchArm` | `TirMatchArm` | — switch pattern match |
| `MatchBody` | `TirMatchBody` | — switch pattern match |
| `ElseBranch` | `TirElseBranch` | — switch pattern match |

For the re-exported types, the change is mechanical: `use crate::mvl::parser::ast::X`
becomes `use crate::mvl::ir::X`.  No call-site changes.

For `Expr → TirExpr` and `Stmt → TirStmt`, each `match expr { … }` arm must be
updated to the TIR variant names, and type information switches from a separate
`Span → Ty` lookup to `tir_expr.ty` (already embedded).

### (b) Metadata-only — switch to TIR struct fields

These types are imported for static metadata (name, type parameters, receiver type)
that is already present in the corresponding TIR struct.  No semantic gap; the
emitter just needs to read from `TirFn` / `TirActorDecl` instead of the raw AST decl.

| AST type | Used for | TIR equivalent |
|----------|----------|----------------|
| `FnDecl` | `name`, `type_params`, `receiver_type` | `TirFn.name`, `TirFn.type_params`, `TirFn.receiver_ty` |
| `ActorDecl` | field names, method dispatch | `TirActorDecl.fields`, `TirActorDecl.methods` |
| `Param` | parameter names/types | `TirParam` |
| `FieldDecl` | field names/types | `TirFieldDecl` |
| `TypeBody` | struct/enum body shape | `TirTypeBody` |
| `TypeDecl` | type name, generics | `TirTypeDecl` |
| `Variant` | enum variant names/fields | `TirVariant` |
| `VariantFields` | unit/tuple/struct fields | `TirVariantFields` |
| `RefExpr` | refinement predicates (doc/spec) | kept in `TirFn.requires/ensures`, `TirMatchArm.guard` — do not iterate |

### (c) Erase — remove the AST path entirely

These types reflect old entry points or fallback paths that pre-date the TIR
migration.  The TIR-first path already exists alongside them; the AST path just
needs to be removed.

| AST type | Location | Why removable |
|----------|----------|---------------|
| `Program` | `rust.rs`, `rust/config.rs`, `llvm_text/emitter.rs` | TIR entry points exist; `TranspileConfig.prelude_progs` can hold `Vec<TirProgram>` |
| `Decl` | `rust.rs`, `rust/emitter.rs`, `llvm_text/emitter.rs` | Iteration over `Decl` variants replaced by `tir.fns`, `tir.types`, `tir.actors`, etc. |
| `Block/Expr/Stmt/MatchBody/ElseBranch` (aliased) | `rust/last_use.rs` | The TIR path `compute_last_uses(body: &TirBlock)` already exists; remove the `compute_last_uses_ast()` fallback |

---

## Emitter entry-point signatures (current vs target)

### LLVM text backend

```rust
// Current (AST-coupled)
pub fn compile_to_ir(&self, prog: &Program, module_name: &str, ...) -> Result<String, String>

// Target (TIR-first)
pub fn compile_to_ir(&self, prog: &TirProgram, module_name: &str, ...) -> Result<String, String>
```

### Rust backend

```rust
// Current (both paths exist)
pub fn transpile(tir: &TirProgram, config: TranspileConfig) -> TranspileResult  // ✅ TIR-first
pub fn transpile_project(prog: &Program, config: TranspileConfig) -> ...         // ❌ AST — remove

// Target: transpile() only, transpile_project() deleted
```

---

## Phase plan

| Phase | Scope | Estimated effort |
|-------|-------|-----------------|
| **1 (this ADR)** | Audit — categorise all imports, set CI baseline | ✅ Done |
| **2** | Extend TIR and lowering if any gaps found | S (none found — all TIR equivalents exist) |
| **3a** | LLVM backend: switch to TIR entry point, replace functional AST types (`Expr/Stmt/Block/…`) | M (~1 week) |
| **3b** | LLVM backend: remove metadata-only AST imports (`FnDecl`, `ActorDecl`, `Program`, `Decl`) | S |
| **3c** | Rust backend: delete legacy entry points, remove AST fallback in `last_use.rs` | S |
| **CI gate** | `grep -r 'parser::ast' src/mvl/backends/ | wc -l` returns 0 | on merge of 3c |

### Key insight from audit

**Phase 2 is a no-op.**  All TIR equivalents already exist in `src/mvl/ir.rs`.
The audit found no gaps that require new TIR node types or lowering extensions.
The work is entirely in the backends (Phases 3a–3c).

The self-hosting effort estimate is revised downward: the backend port surface is
the TIR-consuming code only (~6 K LOC), not the 9–10 K LOC figure that assumed
residual AST coupling.

---

## Acceptance criteria (from #1594)

- [ ] `src/mvl/backends/` imports nothing from `crate::mvl::parser::ast` (CI grep guard: count = 0)
- [ ] Both emitters' entry points accept `&TirProgram` — no AST threaded through
- [ ] Cross-backend test matrix (Rust ↔ LLVM, 110 tests) still green
- [ ] Self-hosting LOC estimate re-baselined in ADR-0044 (TIR-only surface: ~6 K LOC)

---

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

All requirements are **unaffected** by this ADR — it documents an audit and adds a
CI guard.  No language semantics, no IR structure, no runtime behaviour changes.
The migration in Phases 3a–3c will be `semantics-preserving` by construction: the
cross-backend test suite (110 tests) is the oracle that any refactor must keep green.

### Design Principles

- **Explicit over implicit** — the CI guard makes the AST coupling explicit and
  trackable; removing it removes hidden coupling, not explicit behaviour.
- **One way to do it** — Phase 3 eliminates the parallel AST-walk path, leaving
  one canonical code path through TIR.

### Specifications

No spec changes.  This ADR amends the Phase A status entry in ADR-0044 and
documents the ground truth for the #1118 self-hosting backend port scope.

---

## Consequences

**Positive:**

- **Self-hosting estimate corrected:** Phase A (#1118) surface is ~6 K LOC, not ~10 K.
- **No hidden TIR gaps:** All functional AST types have TIR equivalents; no new IR
  nodes required before Phase 3 can begin.
- **CI baseline prevents regression:** `make audit-backend-ast` will fail if a new
  AST import is added to backends, making the migration monotonically forward.

**Negative / trade-offs:**

- Phase 3a (LLVM backend functional migration) is the largest single chunk (~4 K LOC
  of pattern-match rewrites).  It must be done atomically on the LLVM emitter to
  avoid a partially-broken intermediate state; plan for a feature branch.
