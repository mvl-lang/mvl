---
domain: roadmap
version: 0.1.0
status: draft
date: 2026-05-02
---

# 012 — Language Completeness Phases

This spec defines the **completeness model** of the MVL language: the eight pillars
that together describe what makes a language "done", and the phases (5–9) that
deliver each pillar to a verifiable state. It supersedes the README's earlier
three-phase narrative (Prototype / Production / Ecosystem) with a higher-fidelity
model that better matches the project as it has actually been built.

## Philosophy

A language is not "complete" because the parser works. It is complete when:
the type checker proves the requirements, the stdlib is real, the testing
discipline catches regressions, the package supply chain is trustworthy, the
backend is independent of any host compiler, the toolchain supports day-to-day
work, and the verification reaches into concurrency. Each of these is a
*pillar* — independently buildable, independently shippable, independently
testable.

The **phases** are an ordering of pillars: a sequence in which to deliver them
so that the project at every step has a coherent story to tell ("MVL compiles",
"MVL works", "MVL ships", "MVL proves"). Phases do not parallelize freely —
they have a natural dependency order, but pillars within a phase often can.

**Origin:** April 2026 reorganization — the 1/2/3 phases in the README had
collapsed (Phase 1 Rust transpiler done, Phase 2 LLVM ~80% done, Phase 3 vague
and overloaded). The pillar/phase split makes the remaining work visible.

---

## Requirements

### Requirement 1: Eight Pillars [MUST]

The language completeness model MUST consist of exactly eight pillars,
covering every aspect of language readiness from grammar through formal
verification. Each pillar is independently scoped, independently testable, and
mapped to a phase that delivers it.

| # | Pillar | What it covers |
|---|--------|---------------|
| 1 | **Requirements** | The 11 compile-time guarantees (ADR-0001) |
| 2 | **Language constructs** | Grammar, semantics, type system (~25 constructs) |
| 3 | **Stdlib** | Core types, standard library, extern bridges |
| 4 | **Testing** | Unit, mutation, property, MC/DC, integration |
| 5 | **Packaging** | Registry, dependencies, signing, SBOM, supply chain |
| 6 | **Backends** | Rust transpiler, LLVM compiler, future WASM/interpreter |
| 7 | **Toolchain** | Linter, formatter, LSP, architecture tools, assurance pipeline |
| 8 | **Verification** | Model checker, actors, session types, formal proofs |

**Implementation:** `README.md`, label set `phase-N` (issue tracker)

#### Scenario: Pillar coverage check

- GIVEN any open MVL issue
- WHEN it is triaged
- THEN it MUST map cleanly onto exactly one of the eight pillars (or be marked `meta`)

### Requirement 2: Phase Sequence [MUST]

The completeness phases MUST be numbered 1 through 9, with phases 1–4
delivering the foundation (already complete), and phases 5–9 delivering the
remaining pillars in dependency order. Each phase carries a one-word identity
so its purpose is unambiguous.

| Phase | Identity | What it proves | Pillars delivered |
|-------|----------|----------------|-------------------|
| 1–4 | **Foundation** | MVL verifies its 11 requirements at compile time | 1 (Requirements), 2 (Constructs partial) |
| 5 | **Compiles** | MVL owns the full compilation chain (LLVM, no host compiler dependency) | 6 (Backends) |
| 6 | **Works** | Real programs run — stdlib complete, testing matures | 3 (Stdlib), 4 (Testing), 7 (Toolchain partial) |
| 7 | **Self-hosting** | The compiler verifies its own source — MVL is its own first customer | 2 (Constructs complete), 7 (Toolchain) |
| 8 | **Proves** | Concurrent programs verified — actors and model checking | 8 (Verification, applied) |
| 9 | **Proven** | Language formally verified — Lean/Coq metatheory + supply chain | 5 (Packaging), 8 (Verification, formal) |

**Implementation:** `docs/PHASES.md` — issue labels `phase-1` through `phase-9` (LAB271/mvl_language)

#### Scenario: Every issue carries a phase label

- GIVEN an open issue in the `enhancement` or `feat` category
- WHEN it is triaged
- THEN it MUST carry exactly one `phase-N` label corresponding to the pillar it advances

#### Scenario: Phase 5 is closed when its pillar is delivered

- GIVEN all `phase-5` issues are closed
- AND the LLVM backend compiles the corpus end-to-end without rustc dependency
- THEN Phase 5 is considered complete and the label is retained for historical reference only

### Requirement 3: Phase 5 — Compiles [MUST]

Phase 5 MUST deliver the **Backends** pillar to the point where MVL has its
own compilation chain: LLVM IR codegen, runtime, ownership-based drop, and
cross-backend regression testing.

**Implementation:** `src/mvl/backends/llvm/` (LLVM backend), `runtime/llvm/`, `runtime/rust/`

#### Scenario: Phase 5 completion criteria

- GIVEN the LLVM backend module
- WHEN the corpus is compiled end-to-end via `--backend=llvm`
- THEN every program in `tests/corpus/` produces stdout identical to the Rust transpiler backend
- AND `tests/cross_backend.rs` passes

**Status (2026-05-02):** All 43 phase-5 issues closed. Released as v0.60–v0.65 on May 1, 2026. Cross-backend regression coverage exists for hello_world, calculator, shapes; full-corpus parity is the remaining tail (tracked separately, see follow-up to #406).

### Requirement 4: Phase 6 — Works [MUST]

Phase 6 MUST deliver the **Stdlib** and **Testing** pillars to the point where
real programs (not just toy corpus) run reliably and have meaningful test
coverage including mutation, property-based, and MC/DC discipline.

**Implementation:** `std/`, `tests/corpus/11_programs/`, `tools/mcdc/`

#### Scenario: Stdlib completeness

- GIVEN a fresh MVL project depending on `std`
- WHEN it imports `pkg.collections`, `pkg.io`, `pkg.string`, `pkg.math`, `pkg.crypto`
- THEN every public function MUST be a real implementation (no `extern stub` placeholder)

#### Scenario: Testing discipline

- GIVEN any pull request that adds or modifies a function
- WHEN CI runs
- THEN unit tests MUST exist, MC/DC coverage SHOULD be reported, and mutation testing SHOULD pass with score ≥ 0.85

**Tracked issues:** #314 (epic: stdlib real implementations), #180, #179, #40, #39, #206, #326, #175, #171, #170

### Requirement 5: Phase 7 — Self-hosting [MUST]

Phase 7 MUST deliver **self-hosting**: the MVL compiler compiles itself. This
proves the language is complete enough to express a real, non-trivial program
(the compiler) and validates the toolchain end-to-end. MVL becomes its own
first customer.

**Implementation:** `compiler/` (MVL sources: parser, checker, linter in MVL)

#### Scenario: Self-hosted parser

- GIVEN `compiler/parser.mvl`
- WHEN compiled via the LLVM backend
- THEN it MUST parse all files in `tests/corpus/` identically to the Rust parser

#### Scenario: Self-hosted checker

- GIVEN `compiler/checker.mvl`
- WHEN it type-checks itself
- THEN it MUST produce the same typed AST as the Rust checker (modulo representation)

#### Scenario: Bootstrapping

- GIVEN the Rust-based MVL compiler (stage 0)
- WHEN it compiles `compiler/*.mvl` (stage 1)
- AND stage 1 compiles `compiler/*.mvl` again (stage 2)
- THEN stage 1 and stage 2 binaries MUST be identical (reproducible bootstrap)

**Tracked issues:** #187 (milestone: MVL frontend in MVL)

### Requirement 6: Phase 8 — Proves [MUST]

Phase 8 MUST deliver the **Verification** pillar applied to concurrency: actor
runtime, session types, and model checking for protocol correctness. This is
where MVL achieves its 11/11 requirement coverage at runtime.

**Implementation:** `src/mvl/checker/data_race.rs` (foundation), planned actor/model-checker modules

#### Scenario: Actor isolation

- GIVEN two MVL actors communicating via typed channels
- WHEN one actor attempts to access the other's mutable state directly
- THEN the type checker MUST reject it at compile time

#### Scenario: Model-checked protocol

- GIVEN a protocol expressed as a session type
- WHEN the model checker analyzes it
- THEN deadlock and unreachable-state conditions MUST be reported as errors

**Tracked issues:** #134, #63, #69, #260, #37, #262, #295, #306, #362

### Requirement 7: Phase 9 — Proven [SHOULD]

Phase 9 SHOULD deliver two pillars:

1. **Packaging** — registry, signing, SBOM, supply chain trust
2. **Verification (formal)** — metatheory in Lean 4 or Coq

This phase is post-1.0 and represents the full maturity of MVL as a production
ecosystem with formally verified foundations.

**Implementation:** `src/mvl/packages/`, `etc/registry/` (planned), `mvl_metatheory` companion repo

#### Scenario: Package distribution

- GIVEN a package authored in MVL
- WHEN it is published to the registry
- THEN the registry MUST verify the signature, attach an SBOM, and reject any package whose declared API does not match its compiled surface

#### Scenario: Soundness theorem

- GIVEN the MVL type system formalized in Lean 4
- WHEN the soundness theorem is proven
- THEN every well-typed MVL program is shown to satisfy each of the eleven requirements

**Tracked issues:** #56, #246, #252, #251, #185, #561, #615, #633, #635, #636, #637

**Status:** Not yet started. Target: post-1.0 release.

### Requirement 8: README Alignment [MUST]

The `README.md` "How — Three Phases" section MUST be replaced with the 5–9
phase model defined in this spec. The README MUST link to this spec for the
detailed pillar mapping.

**Implementation:** `README.md` (top-level)

#### Scenario: README reflects the model

- GIVEN a fresh visitor reading `README.md`
- WHEN they reach the roadmap section
- THEN they MUST see the 1–9 phase structure, current phase status, and a link to spec 012

---

## Compiler Architecture — Five-Stage Pipeline

The pillars in Requirement 1 describe *what is delivered*. The pipeline below describes *how the compiler is organized* to deliver them. Each stage has a distinct character (granularity, optionality, output shape) and corresponds to a top-level directory under `src/mvl/`.

```
parser     ─►  per-file:        text → AST                     (parallelizable)
resolver   ─►  whole-project:   ASTs → module graph            (sequential — Spec 005)
checker    ─►  per-program:     graph → typed AST              (Specs 001–003, 011, ADR-0001 Reqs 1–11)
passes     ─►  per-program:     typed AST → instrumented AST   (optional, composable)
backends   ─►  per-program:     AST → Rust source / LLVM IR    (transpiler / codegen)
```

| Stage | Directory | Granularity | Optional? | Examples |
|-------|-----------|-------------|-----------|----------|
| Parser | `src/mvl/parser/` | Per-file | No | Lexer + recursive-descent parser (ADR-0005) |
| Resolver | `src/mvl/resolver/` | Whole-project | No | Visibility, imports, cycle detection (Spec 005) |
| Checker | `src/mvl/checker/` | Per-program | No | Type, effect, IFC, termination, refinement, data-race |
| Passes | `src/mvl/passes/` | Per-program | Yes (each pass) | Coverage, MC/DC, mutation (ADR-0014, ADR-0015) |
| Backends | `src/mvl/backends/rust/`, `src/mvl/backends/llvm/` | Per-program | One required | Rust source emission, LLVM IR emission |

Sibling concerns that are not pipeline stages:

- `src/mvl/linter/` — Spec 011. Lint-flavored analysis over the typed AST. Produces diagnostics, does not transform. Distinct from passes.
- `src/mvl/stdlib/`, `src/mvl/packages/`, `src/mvl/toolchain/` — supporting infrastructure, not pipeline stages.

### Requirement 9: Pipeline Stage Discipline [MUST]

Each pipeline stage MUST live in its own top-level directory under `src/mvl/`. AST-level instrumentation transformations (coverage, MC/DC instrumentation, mutation injection) MUST live under `src/mvl/passes/`, not under `src/mvl/backends/rust/` or `src/mvl/backends/llvm/`. The transpiler and LLVM codegen MUST consume the same instrumented AST produced by the passes — instrumentation is written once per concern, not per backend.

**Implementation:** `src/mvl/{parser,resolver,checker,passes,backends,linter}/`

#### Scenario: New AST instrumentation lands in passes/

- GIVEN a new instrumentation concern (e.g., complexity counting, fuzzer harness injection)
- WHEN the implementation lands
- THEN it MUST live under `src/mvl/passes/<name>/`, not under any backend directory

#### Scenario: Both backends consume the same instrumented AST

- GIVEN a typed AST instrumented for coverage (or MC/DC, or mutation)
- WHEN compiled through both `--backend=rust` and `--backend=llvm`
- THEN both backends MUST honor the instrumentation, producing executables that emit equivalent coverage / MC/DC / mutation evidence

**Tracked refactor:** #443 (introduce `src/mvl/passes/`), #444 (decouple passes from Rust-specific emission)

---

## Out of Scope

- The eleven requirements themselves are specified in ADR-0001 and per-feature specs (000–011). This spec governs *delivery*, not *requirements*.
- Per-pillar epic decomposition lives in individual issues, not here. This spec is the index, not the implementation.
- The cross-backend test gap noted in #406 (4 failing `mvl_runtime` link tests) is a separate fix, not part of this spec.

## Changelog

- **2026-05-02** — initial draft (closes #406)
- **2026-05-02** — added "Compiler Architecture — Five-Stage Pipeline" section + Requirement 9 (pipeline stage discipline) to formalize parser/resolver/checker/passes/backends layout. Tracks #443, #444.
- **2026-05-14** — Phase 7 changed from "Ships" to "Self-hosting" per README alignment. Packaging moved to Phase 9. Self-hosting scenarios added (bootstrap, parser parity, checker parity).
