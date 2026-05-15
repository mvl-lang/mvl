---
domain: compiler
version: 0.1.0
status: draft
date: 2026-05-15
epic: phase-8-compiler-refactor
---

# 017 — Compiler Architecture Refactoring

This spec defines the **Phase 8 Epic** for restructuring the MVL compiler codebase.
The compiler has grown organically to ~60k lines across 86 files. While functional,
several architectural issues impede maintainability and extensibility.

## Motivation

The current codebase exhibits:

1. **Monolithic main.rs** — All CLI commands and file loading inline (~2k lines visible)
2. **Transpile function explosion** — 20+ near-identical `transpile_*` variants in `backends/rust/mod.rs`
3. **Scattered file loading** — 7 functions for loading files, stdlib, packages
4. **No pipeline abstraction** — Each command reconstructs parse→check→emit manually
5. **Inconsistent monomorphization** — Rust backend defers to rustc, LLVM does JIT

**Goal:** Establish a clean, layered architecture that supports future growth
(IDE integration, incremental compilation, new backends) without accumulating
further technical debt.

---

## Requirements

### Requirement 1: Unified File Loader [MUST]

A single `Loader` module MUST handle all file discovery and parsing, replacing
the 7 scattered functions currently in `main.rs`.

**Implementation:** `src/mvl/loader.rs` (new)

#### Scenario: Load single file with dependencies

- GIVEN a path to a `.mvl` file
- WHEN `Loader::load_file(path)` is called
- THEN the file is parsed AND its `use` declarations are analyzed
- AND stdlib modules are loaded for `use std.*` imports
- AND sibling modules are loaded for `use module::*` imports
- AND pkg modules are loaded for `use pkg.*` imports

#### Scenario: Load project directory

- GIVEN a directory path
- WHEN `Loader::load_dir(path, test_only=false)` is called
- THEN all `*.mvl` files (excluding `*_test.mvl`) are discovered recursively
- AND each file is parsed with its dependencies resolved

#### Scenario: Implicit prelude always loaded

- GIVEN any load operation
- WHEN the loader initializes
- THEN `core.mvl`, `strings.mvl`, `lists.mvl` are loaded as implicit prelude
- AND they are available to all subsequent programs

---

### Requirement 2: Pipeline Abstraction [MUST]

A `Pipeline` struct MUST orchestrate the compilation phases, providing a single
entry point for check, build, test, and analysis commands.

**Implementation:** `src/mvl/pipeline.rs` (new)

#### Scenario: Check pipeline

- GIVEN a loaded set of programs
- WHEN `pipeline.check()` is called
- THEN all programs are type-checked with prelude
- AND checker passes (termination, IFC, refinements, contracts) run
- AND results are aggregated per-file

#### Scenario: Build pipeline

- GIVEN a checked set of programs
- WHEN `pipeline.build(config)` is called
- THEN the transpiler emits Rust/LLVM output
- AND cargo/clang compiles the output
- AND the binary is produced

#### Scenario: Pipeline with instrumentation

- GIVEN a pipeline instance
- WHEN `.with_coverage()` or `.with_mcdc()` is called
- THEN the transpile phase includes instrumentation
- AND metadata (branch info, decisions) is collected

---

### Requirement 3: TranspileConfig Builder [MUST]

The 20+ `transpile_*` function variants MUST be consolidated into a single
`transpile(prog, config)` function with a builder-pattern configuration.

**Implementation:** `src/mvl/backends/rust/config.rs` (new)

#### Scenario: Basic transpilation

- GIVEN a program and crate name
- WHEN `TranspileConfig::new("crate").build()` is passed to `transpile()`
- THEN standard Rust output is produced

#### Scenario: Transpilation with prelude

- GIVEN a program and prelude programs
- WHEN `TranspileConfig::new("crate").with_prelude(progs).build()` is used
- THEN prelude declarations are visible during emission

#### Scenario: Transpilation with coverage

- GIVEN a program and coverage start ID
- WHEN `TranspileConfig::new("crate").with_coverage(start_id).build()` is used
- THEN branch instrumentation is emitted
- AND branch metadata is returned in the result

#### Scenario: Transpilation for test crate

- GIVEN a source program (not `_test.mvl`)
- WHEN `TranspileConfig::new("crate").for_test_crate().build()` is used
- THEN `extern "rust"` blocks become `todo!()` stubs
- AND the output compiles without external dependencies

---

### Requirement 4: CLI Module Extraction [SHOULD]

CLI command implementations SHOULD be extracted from `main.rs` into a
dedicated `src/cli/` module hierarchy.

**Implementation:** `src/cli/` (new directory)

#### Scenario: Command dispatch

- GIVEN a CLI invocation `mvl check path`
- WHEN main.rs parses arguments
- THEN `cli::check::run(args)` is invoked
- AND main.rs contains only dispatch logic (~50 lines)

#### Scenario: Command-specific logic isolated

- GIVEN the `cmd_mcdc` function (currently ~150 lines in main.rs)
- WHEN extracted to `src/cli/mcdc.rs`
- THEN it uses `Loader` and `Pipeline` abstractions
- AND it contains only MC/DC-specific logic

---

### Requirement 5: Visitor-Based Emission [MAY]

The emit functions MAY be refactored to use a visitor pattern, enabling
cleaner composition of instrumentation passes.

**Implementation:** `src/mvl/backends/rust/visitor.rs` (future)

#### Scenario: Base emission

- GIVEN an AST and a `BaseEmitVisitor`
- WHEN the visitor traverses the AST
- THEN standard Rust code is emitted

#### Scenario: Instrumented emission

- GIVEN a `CoverageVisitor` wrapping a `BaseEmitVisitor`
- WHEN the visitor traverses an `if` expression
- THEN coverage tracking code is injected around the base emission

---

### Requirement 6: Explicit Monomorphization Pass [MAY]

A dedicated monomorphization pass MAY be added to unify the handling of
generic functions across backends.

**Implementation:** `src/mvl/passes/monomorphize.rs` (future)

#### Scenario: Generic function specialization

- GIVEN a generic function `fn identity[T](x: T) -> T`
- AND call sites `identity(42)` and `identity("hello")`
- WHEN the monomorphization pass runs
- THEN `identity_Int` and `identity_String` functions are created
- AND call sites are rewritten to target specialized versions

---

## Work Breakdown

### Epic: Phase 8 — Compiler Architecture Refactor

**8 tickets, 4 weeks.**

| # | Title | Scope | Size | Depends |
|---|-------|-------|------|---------|
| **#766** | Create `Loader` module | Extract all 7 file-loading functions from main.rs into `src/mvl/loader.rs`. Single `Loader` struct with `load_file()`, `load_dir()`, `resolve_imports()`. Handles implicit prelude, stdlib, pkg modules, sibling resolution. | L | — |
| **#767** | Create `Pipeline` abstraction | New `src/mvl/pipeline.rs` with `Pipeline::check()`, `Pipeline::build()`. Modifiers `.with_coverage()`, `.with_mcdc()`, `.with_mutation()`. Orchestrates Loader → checker → transpiler flow. | L | #766 |
| **#768** | Introduce `TranspileConfig` builder | Create `backends/rust/config.rs`. Builder pattern: `.with_prelude()`, `.with_coverage(start_id)`, `.with_mcdc(start_id)`, `.with_mutation()`, `.for_test_crate()`. | M | — |
| **#769** | Consolidate transpile functions | Replace 20+ `transpile_*` variants with single `transpile(prog, config) -> TranspileResult`. Delete all deprecated functions. Update all call sites. | L | #768 |
| **#770** | Extract CLI commands | Move `cmd_check`, `cmd_build`, `cmd_test`, `cmd_mcdc`, `cmd_mutate`, `cmd_assurance`, `cmd_lint`, `cmd_complexity` to `src/cli/` modules. Each command uses Loader + Pipeline. | L | #766, #767 |
| **#771** | Slim main.rs to dispatch | Reduce main.rs to arg parsing + command dispatch (~50 lines). All logic delegated to `cli::` modules. | M | #770 |
| **#772** | Documentation + cleanup | Update ARCHITECTURE.md with new module structure. Add doc comments to Loader, Pipeline, TranspileConfig public APIs. Run clippy, fix warnings, verify all tests pass. | M | #771 |
| **#773** | (Future) Visitor-based emission | Tracking issue for emit visitor pattern refactor. Design doc only — not blocking Phase 8 completion. | S | — |

### Dependency Graph

```
#768 TranspileConfig ──→ #769 Consolidate transpile
                                    │
#766 Loader ────┐                   │
                ├──→ #770 CLI ──→ #771 Slim main.rs ──→ #772 Cleanup
#767 Pipeline ──┘
```

### Timeline

| Week | Focus | Tickets |
|------|-------|---------|
| 1 | Foundation | #766 Loader, #768 TranspileConfig |
| 2 | Abstraction | #767 Pipeline, #769 Consolidate |
| 3 | Extraction | #770 CLI commands, #771 Slim main.rs |
| 4 | Polish | #772 Documentation + cleanup |

---

## Architecture After Refactoring

```
src/
├── main.rs                    50 lines — arg parse + dispatch only
├── cli/
│   ├── mod.rs                 Command enum, shared helpers
│   ├── check.rs               mvl check
│   ├── build.rs               mvl build
│   ├── test.rs                mvl test
│   ├── mcdc.rs                mvl mcdc
│   ├── mutate.rs              mvl mutate
│   ├── assurance.rs           mvl assurance
│   ├── lint.rs                mvl lint
│   └── complexity.rs          mvl complexity
└── mvl/
    ├── loader.rs              NEW: Unified file loading
    ├── pipeline.rs            NEW: Compilation orchestration
    ├── parser/                (unchanged)
    ├── checker/               (unchanged)
    ├── resolver/              (unchanged)
    ├── passes/                (unchanged)
    ├── backends/
    │   ├── mod.rs             Backend trait
    │   ├── rust/
    │   │   ├── mod.rs         SLIMMED: ~200 lines
    │   │   ├── config.rs      NEW: TranspileConfig builder
    │   │   ├── emitter.rs     (unchanged)
    │   │   ├── emit_*.rs      (unchanged)
    │   │   └── ...
    │   └── llvm/              (unchanged)
    ├── stdlib/                (unchanged)
    ├── packages/              (unchanged)
    └── toolchain/             (unchanged)
```

---

## Metrics

### Before

| File | Lines | Concern |
|------|-------|---------|
| main.rs | ~4000 | Everything |
| backends/rust/mod.rs | 1255 | 20+ transpile variants |
| (file loading) | ~400 | Scattered across main.rs |

### After

| File | Lines | Concern |
|------|-------|---------|
| main.rs | ~50 | Dispatch only |
| cli/*.rs | ~1500 | Command implementations |
| loader.rs | ~400 | File discovery & parsing |
| pipeline.rs | ~300 | Compilation orchestration |
| backends/rust/mod.rs | ~200 | Re-exports only |
| backends/rust/config.rs | ~150 | TranspileConfig builder |

**Total reduction in main.rs:** 4000 → 50 lines (98% reduction)
**Consolidation:** 20+ transpile functions → 1 function + config

---

## References

- ADR-0027: Multi-backend architecture
- Spec 009: Transpiler codegen
- Spec 012: Language completeness phases
