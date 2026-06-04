# ADR-0041: Stdlib Method Dispatch — Eliminate Emitter Special-Casing

**Status:** Accepted
**Date:** 2026-06-04
**Issues:** #1217, #992, #1214
**Related:** ADR-0013 (transpiler-mediated codegen), ADR-0019 (two-path stdlib), ADR-0021 (primitives redesign), ADR-0022 (three-category operator/stdlib model), ADR-0027 (multi-backend), ADR-0031 (no UFCS), ADR-0038 (TIR)

---

## Context

Stdlib method dispatch currently requires **three synchronized layers**:

1. **Stdlib declaration** (`std/lists.mvl`) — `builtin fn` or MVL body
2. **Runtime implementation** (`mvl_runtime/`) — actual Rust code
3. **Emitter special-casing** (`backends/rust/emit_exprs.rs`, `backends/llvm_text/emitter.rs`) — match arms that intercept method calls and emit custom backend-specific code

The emitter layer creates a **4-way sync requirement** (#992): to add or change any stdlib method, one must touch the declaration, the runtime, the Rust emitter, and the LLVM emitter independently. This leads to:

- Stub methods with wrong or broken MVL bodies (#1214), because the body is never reached — the emitter intercepts before it runs
- Duplicated dispatch logic across both backends (Rust emitter: ~200 lines of method-specific match arms; LLVM emitter: ~300 lines)
- Backend parity gaps: category D stubs work in the Rust backend via inline Rust code but are completely broken on LLVM

### Current Dispatch Categories

| Category | Methods | Rust Emitter | LLVM Emitter | MVL Body |
|----------|---------|-------------|-------------|----------|
| **A. Kernel builtins** | `len`, `push`, `get`, `slice`, `concat`, `contains` | Calls runtime fn | C-ABI dispatch | `builtin fn` (no body) |
| **B. Pure MVL → UFCS redirect** | `trim`, `take`, `skip`, `reverse`, `first`, `last`, `flatten` | `method(receiver)` free fn call | Strips body, hardcodes C-ABI | Has real MVL body |
| **C. HOF intercepts** | `map`, `filter`, `fold`, `any`, `all`, `take_while`, `skip_while` | Inline Rust iterator chains | C-ABI to `List_filter` etc. | Has real MVL body (never runs via transpiler) |
| **D. Stub intercepts** | `sort`, `partition`, `group_by`, `windows`, `chunks` | Inline Rust code | **Nothing** (broken on LLVM) | Placeholder/infinite recursion |
| **E. Transpiler-only** | `min`, `max`, `join` | Inline Rust (fixed in 9f61798) | MVL body runs | Has real MVL body as LLVM fallback |

---

## Decision

**The emitter should only know about primitives. Everything else compiles from the MVL body or routes through `builtin fn` → runtime.**

### Target State

```
Emitter knows:
  builtin fn → call C-ABI symbol (name derived from declaration)
  operators (+, -, <, >, ==) → LLVM instructions / Rust operators

Emitter does NOT know:
  sort, filter, map, fold, trim, take, skip, ...
  (these compile from their MVL bodies like any user function)
```

### Migration Path

Migration proceeds in three phases, each independently shippable:

**Phase 1: Promote stubs to explicit builtins** (unblocked, relates to #1214)

- `sort`, `partition`, `windows`, `chunks`, `group_by` → `builtin fn` in `std/lists.mvl`
- Add C-ABI runtime implementations in `mvl_runtime/`
- Remove emitter match arms for these methods in both backends

**Phase 2: Compile pure MVL methods normally** (blocked on #992 for LLVM)

- Fix LLVM backend SSA dominance issue for MVL bodies with loops (#992)
- Remove `STDLIB_REPLACED_BY_DISPATCH` stripping from LLVM backend
- Remove `STDLIB_UFCS_METHODS` list from Rust backend
- Methods like `trim`, `take`, `skip`, `reverse` simply compile from their MVL source

**Phase 3: Compile HOF methods from MVL bodies** (blocked on closure lowering)

- Fix closure lowering in both backends so `for x in self { f(x) }` compiles
- Remove inline iterator chain generation from Rust emitter
- Remove C-ABI `List_filter`/`List_map` trampolines from LLVM backend
- `map`, `filter`, `fold`, `any`, `all` compile from their MVL source

### What Stays in the Emitter Permanently

- **Kernel builtins** (category A): `len`, `push`, `get`, `slice`, `concat`, `contains` — irreducible memory operations with no expressible MVL body
- **Operator intrinsics**: arithmetic, comparison, logical, bitwise (ADR-0022)
- **`builtin fn` dispatch table**: maps MVL declaration name → C-ABI symbol

---

## Consequences

**Positive:**
- **Eliminates 4-way sync** (#992): adding a stdlib method = MVL source + runtime impl (2 places, not 4)
- **Eliminates stub methods**: no more MVL bodies that lie about their behaviour (#1214)
- **Backend parity by construction**: both backends compile the same MVL source; divergence becomes impossible for non-builtin methods
- **Simpler emitters**: ~500 fewer lines of method-specific code across both backends
- **Testability**: stdlib methods compiled from MVL source are tested by the corpus test suite without special-case coverage

**Negative / mitigations:**
- Phase 2 and 3 are blocked by prerequisites (#992, closure lowering) — mitigated by Phase 1 being fully unblocked and independently shippable
- HOF performance may differ between compiled-from-MVL and inline-Rust paths until the Rust compiler optimises the indirection — acceptable for correctness-first Phase 1

**Deferred:**
- Phase 2: gated on #992 (LLVM SSA dominance fix)
- Phase 3: gated on closure lowering in both backends

---

## Rejected Alternatives

### Keep the Rust emitter as-is, fix only the LLVM emitter

Rejected because it does not eliminate the 4-way sync. The Rust emitter would still require updates when stdlib methods change. Backend divergence would remain a source of bugs.

### Replace emitter special-casing with a code-generation macro system

Rejected per ADR-0013 (no macros, no reflection). The transpiler is a code generator; stdlib methods should be expressible in MVL and compiled like user code. Macros would add a new language layer without eliminating the sync problem.

### Emit all stdlib methods as C-ABI builtins

Rejected because HOF methods (`map`, `filter`, `fold`) require closure arguments. C-ABI trampolines for closures introduce complexity that defeats the goal of simplicity. Phase 3 achieves correct closure compilation from MVL source instead.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

This decision does not weaken any of the eleven compiler-verified requirements. Relevant interactions:

- **Req 3 (Effect tracking):** Unchanged. Stdlib methods compiled from MVL source carry their declared effects through the normal effect checker.
- **Req 6 (Supply chain / no hidden behaviour):** **Strengthens.** Removing emitter special-casing means stdlib method behaviour is fully visible in the MVL source — no invisible Rust shim overrides the declared body.
- **Req 11 (Backend parity):** **Strengthens.** Both backends compile the same MVL source; divergence is structurally eliminated for non-builtin methods.

### Design Principles (README)

- **Explicit over implicit** — **strengthens**: stdlib method bodies are visible MVL source, not hidden emitter match arms.
- **One way to do it** — **strengthens**: one dispatch path (`builtin fn` → C-ABI or MVL body → compiler) replaces three ad-hoc paths.
- **The signature is the threat model** — **consistent with**: effect annotations on stdlib methods survive to callers regardless of whether the body is MVL or `builtin fn`.
- **No hidden behaviour** — **strengthens**: emitter interception is hidden behaviour; removing it means what the compiler emits matches what the MVL source declares.
- All other principles: **consistent with**.

### Specifications

- **Spec 009** (transpiler codegen) — Requirement 4 (Stdlib Function Mapping) documents the current emitter dispatch table. Phase 1–3 completion will require updating that requirement to reflect table-driven `builtin fn` dispatch and the removal of method-specific match arms. A cross-reference to this ADR is added to spec 009.
- **Spec 005** (modules) — consistent; stdlib module structure (`std/lists.mvl`, `std/strings.mvl`) is unchanged.
- **ADR-0019** (two-path stdlib) — this ADR narrows the Rust path: direct method dispatch via emitter match arms is eliminated; the Rust path becomes `builtin fn` → C-ABI only, consistent with ADR-0019.
- **ADR-0022** (three-category model) — **strengthens**: this ADR enforces the category boundary more strictly. Category A (primitives) stays in the emitter; categories B–E move to MVL source or `builtin fn`.
