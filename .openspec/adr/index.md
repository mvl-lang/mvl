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
| [0015](0015-mcdc-coverage-execution-model.md) | MC/DC coverage execution model — eager evaluation, Unique-Cause, u16 encoding | Accepted |
| [0016](0016-llvm-memory-runtime.md) | LLVM memory runtime — Rust cdylib with reference counting for String, Array, Map | Accepted |
| [0017](0017-linter-hint-severity-explicit-ifc-annotations.md) | Linter Hint severity — explicit IFC annotations as the preferred style | Accepted |
| [0018](0018-llvm-runtime-c-abi.md) | Two-path stdlib architecture — Rust API vs C-ABI cdylib for LLVM backend | Accepted |
