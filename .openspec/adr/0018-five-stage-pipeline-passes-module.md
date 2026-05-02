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
checker   → per-program:     graph → typed AST             (semantic checking)
passes    → per-program:     typed AST → instrumented AST  (optional, composable)
backends  → per-program:     AST → Rust source / LLVM IR   (codegen)
```

The `transpiler/` and `codegen/` modules are now both backends in this model.
`passes/` is the backend-agnostic instrumentation layer between them and the
checker.

### Rust-emission decoupling (ADR-0018b, issue #444)

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
