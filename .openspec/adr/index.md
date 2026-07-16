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
| [0012](0012-extended-package-model.md) | Extended package model — extern inside, verified API outside | Superseded by ADR-0047 |
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
| [0028](0028-c4-len-field-access-refexpr.md) | C4 Context: Field-access support in len() RefExpr for decreases clauses | Accepted |
| [0029](0029-pony-reference-capability-adaptation.md) | Pony reference capability adaptation — iso/val/ref/tag for MVL actors | Accepted |
| [0030](0030-rust-coding-conventions.md) | Rust coding conventions — edition 2021, module layout, fmt, clippy, error handling | Accepted |
| [0031](0031-no-ufcs.md) | No Uniform Function Call Syntax (UFCS) — explicit `f(x)` over implicit `x.f()` | Accepted |
| [0032](0032-stdlib-structured-error-enums.md) | Stdlib structured error enums — domain-specific Result[T, XxxError] replacing Result[T, String] | Accepted |
| [0033](0033-rust-2018-sibling-file-module-style.md) | Rust 2018 sibling-file module style — `foo.mvl` preferred over `foo/mod.mvl` | Superseded by ADR-0030 |
| [0034](0034-monomorphization-pass.md) | Monomorphization as an explicit pre-analysis pass — `MonoProgram` between TypeCheck and analysis | Accepted |
| [0035](0035-effect-system-upgrade.md) | Effect system upgrade — named user effects, subsumption, composite IO | Accepted |
| [0036](0036-ifc-simplification-drop-transparent-sink.md) | IFC simplification — drop transparent/sink labels, unify around Tainted/Secret | Accepted |
| [0037](0037-main-as-actor.md) | Main-as-actor — drop `concurrently` keyword, implicit actor lifecycle | Accepted |
| [0038](0038-typed-ir.md) | Typed Intermediate Representation (TIR) — post-checker typed expression layer | Accepted |
| [0039](0039-package-distribution-sbom.md) | Repository-less package distribution and supply chain security (SBOM, Phase A) | Superseded by ADR-0047 |
| [0040](0040-remove-inkwell.md) | Remove inkwell / llvm-sys dependency | Accepted |
| [0041](0041-stdlib-method-dispatch.md) | Stdlib method dispatch — eliminate emitter special-casing | Accepted |
| [0042](0042-pkg-llvm-backend-convention.md) | Per-package LLVM backend convention — `llvm.rs` + `extern "c"` ABI | Accepted |
| [0043](0043-no-broken-crypto-in-stdlib.md) | No broken crypto in stdlib — algorithm allow-list and deprecation path | Accepted |
| [0044](0044-self-hosting-tir-first-strategy.md) | Self-hosting strategy — TIR-first phase plan and `compiler/tir.mvl` unified types | Accepted |
| [0045](0045-self-hosting-phase3-parser-type-resolution.md) | Self-hosting Phase 3 — parser recursive type resolution (`List[T]` indirection, struct literal disambiguation) | Accepted |
| [0046](0046-transitive-dependency-resolution.md) | Transitive dependency resolution — BFS over package manifests in `mvl update` | Superseded by ADR-0047 |
| [0047](0047-package-management-system.md) | Package management system — format, identity, lock file, transitive resolution, supply chain | Accepted |
| [0048](0048-ast-visit-trait-exhaustive-walkers.md) | AST Visit trait and exhaustive walkers — single canonical traversal, no `..` in walker bodies, contracts in scope | Accepted |
| [0049](0049-llvm-runtime-ifc-refine-audit.md) | IFC, refine, and audit runtime parity — codegen-only on LLVM | Accepted |
| [0050](0050-backend-ast-import-audit.md) | Backend AST import audit + CLI-side prelude assembly contract | Accepted (extended 2026-07-16) |
| [0051](0051-fn-field-equality.md) | Function field equality | Accepted |
| [0052](0052-qualified-module-paths.md) | Qualified module paths for nested files — dot-separated names derived from relative paths | Accepted |
| [0053](0053-no-rustc-concepts-in-mvl.md) | No rustc concepts in MVL grammar — reject trailing `where T: Trait` fn-signature bounds | Accepted |
| [0054](0054-method-receiver-clone-semantics.md) | Method receiver clone semantics — per-dispatch-path clone insertion for user vs stdlib methods | Accepted |
