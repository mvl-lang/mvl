# ADR-0034: Monomorphization as an Explicit Pre-Analysis Pass

**Status:** Partially implemented — superseded for analysis passes by TIR (#1096)
**Date:** 2026-05-17
**Issues:** #837, #838, #1096
**Related:** ADR-0003 (compilation strategy — Phase 5C), ADR-0018 (five-stage pipeline), ADR-0024 (label-transparent functions), ADR-0025 (function contracts)

> **Supersession note (2026-05-27):** Decision 3 ("Analysis passes accept MonoProgram") is
> superseded by the Typed IR (TIR) work in #1096. `MonoProgram` still contains AST-level
> `TypeExpr` nodes and would still require `CheckResult.expr_types` alongside it — no cleaner
> abstraction than the current `Program + CheckResult`. TIR embeds resolved `Ty` into every
> expression node, delivering the same per-instantiation precision with a single self-contained
> input type. The `passes/mono.rs` pass (Decisions 1–2) remains valid and is the input to TIR
> lowering. Decision 4 (backends simplify via MonoProgram) is also superseded: backends will
> consume TIR instead.

---

## Context

MVL supports generic functions with type parameters (`fn map[T, U](...)`). Today, monomorphization — replacing type parameters with concrete types — is handled in two different ways depending on the backend:

**Rust backend (Phase 1–4):** Generic functions are emitted as polymorphic Rust functions. `rustc` performs monomorphization during Rust compilation. From MVL's perspective this is fully implicit: the MVL compiler never sees concrete instantiations.

**LLVM backend (Phase 5A+):** Monomorphization is explicit but just-in-time (JIT). At each call-site emission, `ensure_monomorphized()` infers concrete types from call arguments, mangles a unique name (`map__Int__String`), and emits a concrete copy on demand. This lives entirely inside `backends/llvm.rs` and is invisible to analysis passes.

In both cases, all analysis passes — IFC (`ifc.rs`), refinements (`refinements.rs`), data-race checking (`data_race.rs`), contracts (`contracts.rs`), and termination checking (`termination.rs`) — operate on the **generic AST**. Generic function names appear as single nodes regardless of how many concrete instantiations exist:

```
Call graph (today):        Call graph (after mono):
map ──► list_get           map_Int_String ──► list_get_Int
map ──► list_set           map_Bool_Int   ──► list_get_Bool
```

This is **correct and sufficient** for all current analysis use cases. IFC label propagation does not care whether `fn map[T]` was called with `T=Int` or `T=String`; refinement contracts apply uniformly to all instantiations; data-race checking operates on borrow structure, not concrete types.

The limitation surfaces when **context-sensitive analysis** is needed: different label flows or refinement contracts per instantiation. A single generic call edge `map` cannot carry per-instantiation information. This matters for:

- Precise termination proofs with measure functions that depend on the concrete type size
- Per-instantiation refinement contracts (e.g., bounds that vary by element type)
- Context-sensitive IFC: the label assigned to `map`'s return depends on which concrete `T` flows through it
- Interprocedural call graphs for mutual-recursion detection (#829, #142)

ADR-0003 Phase 5C explicitly names a "monomorphisation pass — instantiate generic functions at call sites" as a planned Phase 5 deliverable.

---

## Decision

### 1. Monomorphization becomes an explicit compiler pass

A dedicated **Monomorphize** pass is introduced between the type checker and analysis passes. The pipeline becomes:

```
Parse → Resolve → TypeCheck → Monomorphize → [CallGraph, IFC, Refinements, DataRace, Contracts] → Emit
```

vs. current:

```
Parse → Resolve → TypeCheck → [CallGraph, IFC, Refinements, DataRace, Contracts] → Emit (implicit mono)
```

### 2. Monomorphized program representation

The pass produces a `MonoProgram` — a concrete, non-generic view of the program reachable from entry points (`main` and all `pub fn` declarations):

```
MonoProgram {
    fns: Vec<MonoFn>,     // concrete function copies, no type_params
    types: Vec<MonoType>, // concrete struct/enum copies
}

MonoFn {
    mangled_name: String,         // e.g. "map__Int__String"
    original_name: String,        // "map"
    type_subs: HashMap<String, TypeExpr>,  // { "T" -> Int, "U" -> String }
    decl: FnDecl,                 // FnDecl with type_params substituted
}
```

Generic functions not reachable from any entry point are excluded. Non-generic functions appear once with an identity substitution.

### 3. Analysis passes accept MonoProgram

After the mono pass, analysis passes receive `&MonoProgram` instead of `&Program`. Each concrete `MonoFn` is analysed independently:

- **Call graph:** edges between concrete `MonoFn` entries — fully precise
- **IFC:** label flow tracked per instantiation; enables future per-instantiation label assignments
- **Refinements:** contracts checked per concrete copy; bounds parameterised by concrete types
- **DataRace / Contracts / Termination:** same semantics, richer precision

### 4. Backends simplify

Both backends receive `MonoProgram` and emit only concrete functions. The LLVM JIT monomorphization logic (`ensure_monomorphized`, `infer_type_subs`, `mangle_fn_name`) moves into the compiler pass, not the backend. The Rust backend emits concrete, non-generic Rust functions instead of polymorphic generics.

### 5. This decision is deferred — implementation tracked in #838

Writing this ADR is the deliverable of #837. The actual implementation (new `passes/mono.rs`, `MonoProgram` types, pipeline wiring, backend updates) is tracked separately in #838.

The deferral is intentional: current analysis passes are correct and complete for their current use cases. The mono pass is the prerequisite for **context-sensitive** interprocedural analysis, which is a Phase 5/6 concern. The correct ordering is:

1. ✅ Call graph construction (#829) — works without mono for current precision needs
2. ✅ IFC forward propagation (#830–#833) — works without mono for label propagation
3. ⬜ Monomorphization pass (#838) — unlocks context-sensitive precision
4. ⬜ Context-sensitive IFC / refinements — depends on #838

---

## Consequences

**Positive:**

- Call graph becomes fully precise: each edge maps to exactly one concrete callee, enabling mutual-recursion detection and WCET analysis.
- Per-instantiation label tracking enables context-sensitive IFC — future use case but impossible without this pass.
- Refinement contracts can reference the concrete type's properties (e.g., bounds that vary by `Int` vs `Float`).
- Both backends simplify: monomorphization is done once in one place, not duplicated across LLVM JIT emission and rustc.
- Analysis passes work on a smaller, concrete graph — potentially faster and easier to reason about.

**Negative / trade-offs:**

- New `MonoProgram` representation must be maintained alongside `Program`; any change to `FnDecl` or `TypeDecl` requires updates in both.
- Generic functions that are defined but never called (e.g., stdlib functions unused in a given program) are excluded from `MonoProgram`. Analysis of unused code requires a separate path.
- Reachability-based monomorphization can miss dynamic dispatch patterns if MVL ever adds trait objects. (Not planned — ADR-0031 rejects UFCS; ADR-0004 keeps the language small.)
- The Rust backend currently benefits from `rustc`'s mature monomorphization and optimization. Emitting pre-monomorphized concrete Rust functions loses polymorphic optimization opportunities (e.g., LLVM SROA across instantiation boundaries). This is acceptable: Phase 5 LLVM backend is the production path.

**Follow-up work created:**

- #838: Implement the monomorphization pass (`passes/mono.rs`, `MonoProgram`, pipeline wiring, backend updates)
- Update `docs/compilation-model.md` with the new pipeline stage when #838 ships
- Revisit `termination.rs` mutual-recursion detection (#142) — the call graph from `MonoProgram` provides the prerequisite data structure

---

## Rejected Alternatives

**Keep JIT monomorphization inside each backend:** The current approach duplicates logic (already exists in LLVM, would need to be added to any future backend). Analysis passes remain blind to concrete instantiations. Rejected because the mono pass is a **language concern**, not a **backend concern**.

**Monomorphize lazily during analysis (per-pass):** Each analysis pass could carry its own type-substitution environment and expand generics on demand. This avoids the `MonoProgram` representation but means every pass must independently implement substitution, mangling, and deduplication. Rejected: too much duplication, harder to keep consistent.

**Wait for Phase 6 (formal provers):** Formal provers need concrete, fully-resolved programs. Deferring to Phase 6 couples monomorphization to the prover work and makes the Phase 5 LLVM backend carry JIT logic forever. Rejected: the pass belongs in Phase 5C as planned in ADR-0003.

**Monomorphize only for the LLVM backend, keep Rust as-is:** Analysis passes run before backend selection. Backend-specific monomorphization means analysis results differ by backend. Rejected: the compiler must give consistent analysis results regardless of emission target.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

| Req | Requirement | Effect |
|-----|-------------|--------|
| 1 | Type safety | **strengthens** — type substitution is fully resolved before analysis; no latent type mismatch between generic declaration and concrete usage |
| 2 | Memory safety | **consistent with** — monomorphization does not affect borrow tracking |
| 3 | Effect safety | **consistent with** — effect signatures are unchanged; concrete copies inherit the generic's effects |
| 4 | Resource linearity | **consistent with** — `iso`/`val` resource tracking is orthogonal to type-parameter substitution |
| 5 | Deadlock freedom | **consistent with** — session-type checking is unaffected |
| 6 | Ownership | **consistent with** — ownership rules apply equally to generic and concrete forms |
| 7 | Purity | **consistent with** — `pure fn` attribute is preserved through substitution |
| 8 | Termination | **strengthens** — a precise concrete call graph enables mutual-recursion detection; measure functions can be verified per instantiation |
| 9 | Data-race freedom | **consistent with** — iso aliasing and ref-escape checks are structural, not type-parameterised |
| 10 | Refinements | **strengthens** — per-instantiation contract checking allows bounds that depend on the concrete type; currently refinements apply uniformly to the generic form |
| 11 | IFC | **strengthens** — per-instantiation label tracking is the prerequisite for context-sensitive IFC; currently label flow is uniform across all uses of a generic function |

### Design Principles (README)

- **Explicit over implicit** — **strengthens**: monomorphization moves from an invisible rustc or JIT backend step to a named, inspectable compiler pass with a documented data structure.
- **One way to do each thing** — **strengthens**: a single canonical `MonoProgram` replaces two divergent mechanisms (rustc generics + LLVM JIT expansion).
- **Verified at compile time** — **strengthens**: analysis passes see concrete programs; proofs over `MonoFn` are strictly stronger than proofs over generic `FnDecl`.
- **Small and complete** — **consistent with**: the pass adds necessary machinery without extending the surface language.
- **No hidden costs** — **strengthens**: the mono pass makes the cost of generics visible in the compiler pipeline; previously the cost was hidden inside rustc or deferred to LLVM IR emission.
- All other principles — **consistent with**.

### Specifications

- `013-termination/spec.md` — Req 8 mutual-recursion detection (#142) requires a concrete call graph. The mono pass is the prerequisite; the spec will need a new requirement and scenario when #838 ships.
- `003-information-flow/spec.md` — Context-sensitive IFC label tracking (future) will reference this ADR as its structural foundation.
- All other specs — unaffected. The mono pass is a compiler-internal concern; no surface language semantics change.
