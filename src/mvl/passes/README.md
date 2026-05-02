# MVL Passes

`src/mvl/passes/` contains backend-agnostic AST transformation passes. Passes sit
between the checker and the backends (transpiler / LLVM codegen) in the
five-stage pipeline:

```
parser → resolver → checker → passes → backends
```

## What is a pass?

A pass takes a typed AST produced by the checker and produces:

- **Analysis output** — metadata tables consumed by CLI commands (e.g. MC/DC
  obligation tables, mutation point counts).
- **Instrumented AST / metadata** — markers and maps consumed by backends to
  emit target-specific code (e.g. branch hit counters for coverage, clause
  arrays for MC/DC, mutation dispatch wrappers).

Passes are **target-neutral**: they contain no Rust syntax strings and no LLVM
IR. Target-specific emission lives in the backend (`transpiler/<pass>_emit.rs`,
`codegen/<pass>_emit.rs`).

## Directory layout

```
passes/
├── coverage/
│   ├── mod.rs          re-exports transform::*
│   └── transform.rs    BranchKind, BranchInfo, CoverageMap; preamble/report helpers
├── mcdc/
│   ├── mod.rs          declares analysis + transform submodules
│   ├── analysis.rs     analyze_mcdc(), DecisionInfo, collect_clauses — static analysis
│   └── transform.rs    MCDCDecision, MCDCMap, detect_coupled_pairs — runtime types
└── mutation/
    ├── mod.rs          re-exports transform::*
    └── transform.rs    mutations_for_binary_op/int_literal, MutantInfo, MutationMap
```

## Analysis vs transform split

| Module              | Has analysis? | Has transform? | Emits Rust? |
|---------------------|:---:|:---:|:---:|
| `coverage/transform`  | no  | yes | no (emit helpers in `transpiler/coverage_emit.rs`) |
| `mcdc/analysis`       | yes | no  | no |
| `mcdc/transform`      | no  | yes | no (emit helpers in `transpiler/mcdc_emit.rs`) |
| `mutation/transform`  | yes | yes | no (ADR-0014 env-var dispatch is target-neutral) |

## How to add a new pass

1. Create `passes/<name>/mod.rs`, `analysis.rs`, `transform.rs` as needed.
2. Declare it in `passes/mod.rs`.
3. Add target-specific emission helpers in `transpiler/<name>_emit.rs` (and
   `codegen/<name>_emit.rs` when the LLVM backend needs them).
4. Wire up the pass in the relevant transpile variant in `transpiler/mod.rs`.
5. Add an ADR if the pass introduces new architectural decisions.
