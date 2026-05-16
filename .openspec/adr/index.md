# ADR Index

| ADR | Title | Status |
|-----|-------|--------|
| [0001](0001-eleven-requirements.md) | Eleven compiler-verified requirements | Accepted |
| [0002](0002-language-contraction.md) | Language contraction — what to drop and why | Accepted |
| [0003](0003-compilation-strategy.md) | Compilation strategy — prototype Rust, production LLVM | Accepted |
| [0004](0004-language-size.md) | Language size — deliberately the smallest | Accepted |
| [0005](0005-recursive-descent-parser.md) | Hand-written recursive descent parser | Accepted |
| [0006](0006-ffi-extern-rust-bridge.md) | FFI via extern "rust" and the bridge.rs convention | Accepted |
| [0007](0007-stdlib-import-model.md) | Standard library import model — prelude, explicit, and trust boundaries | Accepted |
| ~~0008~~ | ~~Compilation units and linking~~ | Merged into 0009 |
| [0009](0009-toolchain-layout.md) | Toolchain layout — XDG, versioning, linking, caches | Accepted |
| [0010](0010-corpus-test-structure.md) | Corpus test structure — progressive complexity ramp | Accepted |
| ~~0011~~ | ~~Generational toolchain~~ | Merged into 0009 |
| [0012](0012-extended-package-model.md) | Extended package model — extern inside, verified API outside | Accepted |
| [0013](0013-transpiler-mediated-codegen.md) | Transpiler-mediated type-directed code generation — no macros, no reflection | Accepted |
| [0014](0014-mutation-testing-execution-model.md) | Mutation testing execution model — single compile, parallel runs | Accepted |
| [0015](0015-mcdc-coverage-execution-model.md) | MC/DC coverage execution model — eager evaluation, Unique-Cause, u32 encoding | Accepted |
| [0016](0016-llvm-memory-runtime.md) | LLVM memory runtime — Rust cdylib with reference counting for String, Array, Map | Accepted |
| [0017](0017-linter-hint-severity-explicit-ifc-annotations.md) | Linter Hint severity — explicit IFC annotations as the preferred style | Accepted |
| [0018](0018-five-stage-pipeline-passes-module.md) | Five-stage pipeline — introduce `src/mvl/passes/` | Accepted |
| [0019](0019-llvm-stdlib-two-path.md) | Two-Path Stdlib Architecture — Rust Crate + C-ABI cdylib | Accepted |
| [0020](0020-bdd-library-naming-convention.md) | BDD as library naming convention, not language syntax | Accepted |
| [0021](0021-primitives-runtime-redesign.md) | Primitives and runtime architecture redesign — unsigned types, bit ops, prelude slim, mvl_memory scope | Accepted |
| [0022](0022-operator-intrinsic-mapping.md) | Operator → intrinsic mapping and stdlib category model (three-category model) | Accepted |
| [0023](0023-stdlib-profiles.md) | Stdlib profiles — trusted vs proven | Accepted |
| [0024](0024-label-transparent-functions.md) | Label-transparent functions (`transparent fn`) — ADR-0024 | Accepted |
| [0025](0025-function-contracts.md) | Function contracts — `requires`/`ensures`, `ghost`, `invariant`, `decreases`, `forall`/`exists` (Phases 1–5) | Accepted |
| [0026](0026-input-validation-philosophy.md) | Input validation philosophy — post-Postel strictness | Accepted |
| [0027](0027-multi-backend-architecture.md) | Multi-backend architecture — `backends/` namespace, `Backend` trait, merged runtime | Accepted |
| [0029](0029-pony-reference-capability-adaptation.md) | Pony reference capability adaptation — iso/val/ref/tag for MVL actors | Accepted |
| [0030](0030-rust-coding-conventions.md) | Rust coding conventions — edition 2021, module layout, fmt, clippy, error handling | Accepted |
