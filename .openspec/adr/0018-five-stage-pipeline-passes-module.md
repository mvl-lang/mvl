# ADR-0018: Five-Stage Pipeline — Introduce `src/mvl/passes/`

**Status:** Accepted
**Date:** 2026-05-02
**Issues:** #443, #444

## Context

MVL's compiler pipeline grew organically with coverage, MC/DC, and mutation
instrumentation living inside `transpiler/` and `checker/`. This conflated two
concerns:

1. **AST-level transformations** (instrument conditions, inject mutation points,
   mark branches) — target-agnostic work that should be reusable by any backend.
2. **Rust code generation** — target-specific emission consumed only by the
   transpiler backend.

Additionally, MC/DC was split: `checker/mcdc.rs` held the static analysis and
`transpiler/mcdc_instr.rs` held the runtime types and preamble generation — two
halves of one feature in different directories.

## Decision

Introduce `src/mvl/passes/` as a top-level peer to `parser/`, `resolver/`,
`checker/`, `transpiler/`, and `codegen/`. Relocate coverage, MC/DC, and
mutation logic into it.

### Directory layout

```
src/mvl/passes/
├── mod.rs
├── coverage/
│   ├── mod.rs
│   └── transform.rs   (was transpiler/coverage.rs)
├── mcdc/
│   ├── mod.rs
│   ├── analysis.rs    (was checker/mcdc.rs)
│   └── transform.rs   (was transpiler/mcdc_instr.rs)
└── mutation/
    ├── mod.rs
    └── transform.rs   (was transpiler/mutation.rs)
```

### Five-stage pipeline

```
parser    → per-file:        text → AST                    (parallelizable)
resolver  → whole-project:   ASTs → module graph           (sequential)
checker   → per-program:     graph → CheckResult           (semantic checking)
             └─ CheckResult exposes: errors, type_env, call_graph, expr_types
passes    → per-program:     CheckResult → Verdict[]       (verification, optional)
lower     → per-program:     AST + expr_types → TirProgram (monomorphize + type resolution)
backends  → per-program:     TIR → Rust source / LLVM IR   (codegen)
```

**`CheckResult` (checker output) now includes (#829, ADR-0034):**
- `type_env: TypeEnv` — full type environment (function signatures, declared types,
  `From` impl registry). Exposed so downstream passes and tools have proper access
  to the type system's output without coupling to checker internals.
- `call_graph: CallGraph` — whole-program function call topology.  Built as a simple
  AST walk over `Expr::FnCall` after type checking.  Precise for MVL because there
  is no virtual dispatch, no function pointers, and closures are statically typed.
  `MethodCall` edges are deferred until the explicit monomorphization pass (#838).
- `expr_types: HashMap<Span, Ty>` — resolved type of every expression (unchanged).

**Interim pipeline (until ADR-0034 monomorphization pass, #838):**
```
parser → resolver → checker(TypeCheck + CallGraph) → passes → backends
```

**Current pipeline (post ADR-0038 TIR + ADR-0050 backend migration):**
```
parser → resolver → TypeCheck → Monomorphize → [CallGraph, passes] → TIR lower → backends
```

The `lower` stage (`src/mvl/ir/lower.rs`) is now an explicit named pass. Backends
consume `TirProgram` only — see ADR-0038 and ADR-0050.

The `transpiler/` and `codegen/` modules are now both backends in this model.
`passes/` is the backend-agnostic instrumentation layer between them and the
checker.

### Rust-emission decoupling (issue #444)

Coverage and MC/DC preamble/report helpers currently emit Rust syntax strings.
These have been extracted to `transpiler/coverage_emit.rs` and
`transpiler/mcdc_emit.rs`. The pass modules themselves contain zero Rust syntax
strings and are fully target-neutral. Future LLVM-side instrumentation will add
`codegen/coverage_emit.rs` and `codegen/mcdc_emit.rs` without touching
`passes/`.

## Consequences

- Each pass lives in one directory regardless of which pipeline stage produces
  its inputs. Checker and transpiler no longer own instrumentation logic.
- The LLVM backend can reuse `passes/coverage`, `passes/mcdc`, and
  `passes/mutation` without depending on `transpiler/`.
- `checker/` retains only semantic analysis (types, effects, IFC, termination,
  data-race, refinements). `mcdc.rs` is gone from `checker/`.
- ADR-0014 (mutation) and ADR-0015 (MC/DC) reference `transpiler/`; they remain
  valid — the execution model is unchanged. Source paths in those ADRs are
  superseded by this ADR.
