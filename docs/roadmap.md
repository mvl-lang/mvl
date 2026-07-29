# MVL Roadmap

**Status (July 2026):** Foundation complete (phases 1–4). Phase 5 shipped (LLVM backend, v0.60–v0.68). Phase 6 shipped (stdlib complete, packaging live). Phase 7 (self-hosting) and Phase 8 (proves) in progress.

See [spec 012](specs/012-phases.md) for the full pillar/phase model and per-phase acceptance criteria.

---

## Eight Pillars

A language is "complete" along eight independent pillars. Each phase delivers one or more pillars.

| # | Pillar | What it covers |
|---|--------|---------------|
| 1 | **Requirements** | The 11 compile-time guarantees (ADR-0001) |
| 2 | **Language constructs** | Grammar, semantics, type system (~25 constructs) |
| 3 | **Stdlib** | Core types, standard library, extern bridges |
| 4 | **Testing** | Unit, mutation, property, MC/DC, integration |
| 5 | **Packaging** | Registry, dependencies, signing, SBOM, supply chain |
| 6 | **Backends** | Rust transpiler, LLVM compiler, future WASM/interpreter |
| 7 | **Toolchain** | Linter, formatter, LSP, assurance pipeline |
| 8 | **Verification** | Model checker, actors, session types, formal proofs |

---

## Phases

```
Phase 1–4  Foundation     MVL verifies its 11 requirements at compile time  ✅ Done
Phase 5    Compiles       MVL owns the full compilation chain (LLVM, no     ✅ Done (May 2026)
                          host compiler dependency)
Phase 6    Works          Real programs run — stdlib complete, testing      ✅ Done
                          matures
Phase 7    Self-hosting   The compiler compiles itself — MVL is its own     🔄 In progress
                          first customer
Phase 8    Proves         Concurrent programs verified — actors, session    🔄 In progress
                          types, model checking
Phase 9    Proven         Language formally verified — Lean/Coq metatheory  Future
                          + package supply chain trust
```

### Phase 5 — Compiles ✅

LLVM backend shipped across five sub-phases (v0.60–v0.68):

| Sub-phase | What | Status |
|-----------|------|--------|
| A | LLVM IR codegen: primitives, arithmetic, control flow | ✅ Done |
| B | LLVM IR codegen: functions, calls, modules | ✅ Done |
| C | LLVM IR codegen: structs, enums, pattern matching | ✅ Done |
| D | LLVM memory runtime (`mvl_memory` cdylib): String, Array, Map | ✅ Done |
| E | Ownership-based drop — `HeapKind` tracking, drop at exit | ✅ Done |

Both backends compile the same MVL source. The test suite differentially fuzzes them against each other (`make fuzz-diff`).

### Phase 6 — Works ✅

**Goal:** Real programs run without stubs. Stdlib modules have real implementations. Packaging pipeline delivers trustworthy distribution.

30 stdlib modules ship real implementations (actors, args, audit, collections, config, core, crypto, csv, db, effects, env, error, ifc, io, json, kv, lists, log, math, net, pbt, process, random, regex, runtime, strings, testing, text, time, toml). Packaging module (`src/mvl/packages/`) ships SBOM (CycloneDX + SPDX), audit, manifest, dependency resolution, and lock-file verification.

Open follow-ups tracked separately (do not block phase completion):

| Component | Issues | Status |
|-----------|--------|--------|
| Iterator trait + lazy ops | [#219](https://github.com/mvl-lang/mvl/issues/219) | Done (2026-04-17) |
| Generics constraint enforcement | [#225](https://github.com/mvl-lang/mvl/issues/225) | Done (2026-04-17) |
| MC/DC coverage in CI | — | Open |
| Mutation testing score ≥ 0.85 | [#210](https://github.com/mvl-lang/mvl/issues/210) | Done (2026-04-18) |
| Borrow lifetimes (full Req 2) | [#234](https://github.com/mvl-lang/mvl/issues/234) | Done (2026-05-01) |

See [stdlib](stdlib.md) for full module implementation status.

### Phase 7 — Self-hosting 🔄

**Goal:** The MVL compiler compiles itself. Validates the toolchain end-to-end and proves the language is expressive enough for a real, non-trivial program (the compiler).

The MVL-in-MVL compiler lives under `compiler/` (56 files, ~26,300 lines of MVL) and passes `mvl check` and `mvl lint`. Lexer, parser, TIR, and both LLVM and Rust emitters are ported. Resolver, monomorphizer, and type-checker passes are in progress.

**Completion criterion:** Three-stage bootstrap verify — Rust `mvl₀` compiles `compiler/*.mvl` to produce `mvl₁`; `mvl₁` recompiles the same source to produce `mvl₂`; `mvl₁` and `mvl₂` are byte-identical.

[#187](https://github.com/mvl-lang/mvl/issues/187) (MVL frontend in MVL) and [#1117](https://github.com/mvl-lang/mvl/issues/1117) (self-hosted checker) are closed. Remaining: [#1118](https://github.com/mvl-lang/mvl/issues/1118) — self-hosted Rust and LLVM emitters.

### Phase 8 — Proves 🔄

**Goal:** Concurrent programs verified — actors, session types, model checker.

Foundations exist: `std.actors` with Tokio runtime (spec 015), data-race freedom checker foundation (`src/mvl/checker/data_race.rs`), session types spec drafted (spec 016). Model-checking and full protocol verification tracked as ongoing work.

The original Phase 8 scope is **closed**. Epic
[#134](https://github.com/mvl-lang/mvl/issues/134) ("Actors, Concurrency, Model
Checker — 11/11") completed on 2026-05-16, along with actor syntax
([#63](https://github.com/mvl-lang/mvl/issues/63)), the model checker
([#37](https://github.com/mvl-lang/mvl/issues/37)), session types
([#260](https://github.com/mvl-lang/mvl/issues/260)), spatial composition
([#295](https://github.com/mvl-lang/mvl/issues/295)) and mutable borrows with
alias checking ([#306](https://github.com/mvl-lang/mvl/issues/306),
[#362](https://github.com/mvl-lang/mvl/issues/362)). Temporal effect properties
([#262](https://github.com/mvl-lang/mvl/issues/262)) were closed as not planned.

Implementation landed: `src/mvl/checker/data_race.rs` (845 lines),
`runtime/llvm/src/actors.rs` (969 lines), spec 015 (actors) and spec 016 (session
types) both on disk.

**A second wave is open**, which is why this phase is not yet marked done:
[#1621](https://github.com/mvl-lang/mvl/issues/1621) (epic: Actors v2 — freeze
actor semantics and runtime for Phase 9 formalization),
[#1740](https://github.com/mvl-lang/mvl/issues/1740) (`select {}` arms are not
yet lowered to real codegen on all backends),
[#1495](https://github.com/mvl-lang/mvl/issues/1495) (bounded mailbox),
[#1552](https://github.com/mvl-lang/mvl/issues/1552) (per-sender causal FIFO),
[#1741](https://github.com/mvl-lang/mvl/issues/1741) (dead-letter routing),
[#2008](https://github.com/mvl-lang/mvl/issues/2008) (behavioural
substitutability) and [#1613](https://github.com/mvl-lang/mvl/issues/1613)
(message throughput 25-30x slower than Go channels).

### Phase 9 — Proven

Two pillars: package supply chain trust (registry, signing, publish workflow) and formal metatheory (Lean 4 / Coq soundness theorem). Post-1.0.

Supply-chain groundwork is already done: [#56](https://github.com/mvl-lang/mvl/issues/56) (package manager, SBOM, licensing), [#151](https://github.com/mvl-lang/mvl/issues/151) (CVE-aware dependency auditing) and [#185](https://github.com/mvl-lang/mvl/issues/185) (unified `mvl check` / `audit` / `sbom` trust pipeline) all closed between May and June 2026.

Still open: [#246](https://github.com/mvl-lang/mvl/issues/246) (Lean/Coq metatheory epic), [#252](https://github.com/mvl-lang/mvl/issues/252) (package signing — Sigstore + SLSA), [#251](https://github.com/mvl-lang/mvl/issues/251) (organisational package proxy).

---

## Requirement enforcement status

| # | Requirement | Checker | Rust backend | LLVM backend |
|---|------------|---------|--------------|--------------|
| 1 | Type safety (ADTs) | ✅ enforced | native (rustc) | native (LLVM types) |
| 2 | Memory safety | ✅ use-after-move + borrow lifetimes (#234, done 2026-05-01) | native (rustc borrow checker) | noalias / nonnull metadata |
| 3 | Totality (exhaustive match) | ✅ enforced | native (rustc) | native |
| 4 | Null elimination (Option) | ✅ enforced | native (rustc) | native |
| 5 | Error visibility (Result) | ✅ enforced | native (rustc) | native |
| 6 | Ownership (linearity) | ✅ move tracking | native (rustc) | HeapKind drop |
| 7 | Effect tracking | ✅ enforced | doc comment | IR-generation error (planned) |
| 8 | Termination | ✅ while rejected; structural recursion planned | doc comment | IR-generation error (planned) |
| 9 | Data race freedom | ✅ capabilities parsed; actor-boundary Phase 8 | capability comment | planned Phase 8 |
| 10 | Refinement types | ✅ static + debug_assert! fallback | debug_assert! | SMT (Z3) planned Phase 6 |
| 11 | IFC | ✅ labels + `relabel` transitions enforced | newtypes + relabel | taint pass planned Phase 6 |

---

## Architecture decisions

| ADR | Decision |
|-----|----------|
| [ADR-0001](adr/0001-eleven-requirements.md) | Eleven compiler-verified requirements |
| [ADR-0002](adr/0002-language-contraction.md) | Language contraction — what to drop and why |
| [ADR-0003](adr/0003-compilation-strategy.md) | Compilation strategy — prototype Rust, production LLVM |
| [ADR-0004](adr/0004-language-size.md) | Language size — deliberately the smallest |
| [ADR-0005](adr/0005-recursive-descent-parser.md) | Hand-written recursive descent parser |
| [ADR-0006](adr/0006-ffi-extern-rust-bridge.md) | FFI via extern "rust" and the bridge.rs convention |
| [ADR-0007](adr/0007-stdlib-import-model.md) | Standard library import model |
| [ADR-0009](adr/0009-toolchain-layout.md) | Toolchain layout — XDG, versioning, linking, caches |
| [ADR-0010](adr/0010-corpus-test-structure.md) | Corpus test structure — progressive complexity ramp |
| [ADR-0012](adr/0012-extended-package-model.md) | Extended package model |
| [ADR-0013](adr/0013-transpiler-mediated-codegen.md) | Transpiler-mediated type-directed code generation |
| [ADR-0014](adr/0014-mutation-testing-execution-model.md) | Mutation testing execution model |
| [ADR-0015](adr/0015-mcdc-coverage-execution-model.md) | MC/DC coverage execution model |
| [ADR-0016](adr/0016-llvm-memory-runtime.md) | LLVM memory runtime (mvl_memory cdylib) |
| [ADR-0017](adr/0017-linter-hint-severity-explicit-ifc-annotations.md) | Linter hint severity — explicit IFC annotations |
| [ADR-0018](adr/0018-five-stage-pipeline-passes-module.md) | Five-stage pipeline — passes module |
| [ADR-0019](adr/0019-llvm-stdlib-two-path.md) | Two-path stdlib architecture (LLVM vs Rust) |
| [ADR-0020](adr/0020-bdd-library-naming-convention.md) | BDD as library naming convention |
| [ADR-0021](adr/0021-primitives-runtime-redesign.md) | Primitives and runtime architecture redesign |
| [ADR-0022](adr/0022-operator-intrinsic-mapping.md) | Operator → intrinsic mapping |
| [ADR-0023](adr/0023-stdlib-profiles.md) | Stdlib profiles — trusted vs proven |
| [ADR-0024](adr/0024-label-transparent-functions.md) | Label-transparent functions |
| [ADR-0025](adr/0025-function-contracts.md) | Function contracts |
| [ADR-0026](adr/0026-input-validation-philosophy.md) | Input validation philosophy |
| [ADR-0027](adr/0027-multi-backend-architecture.md) | Multi-backend architecture |

---

## Design principles

1. **Verification density:** Every feature exists to increase properties proven per token
2. **Contraction:** Remove features that resist verification — the language shrinks by policy
3. **One way:** One way to branch, one way to loop, one way to handle errors
4. **Stdlib grows, language doesn't:** New functionality via library, not language extensions
5. **Two backends, one proof gate:** The MVL compiler verifies all 11 requirements; the backend is a delivery mechanism
