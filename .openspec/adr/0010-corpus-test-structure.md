# ADR-0010: Corpus Test Structure — Progressive Complexity Ramp

**Status:** Accepted
**Date:** 2026-04-15 (revised 2026-06-04)
**Context:** The test corpus in `tests/corpus/` serves two purposes: (1) compiler test cases ensuring each requirement is enforced, and (2) a training/validation corpus for LLM generation quality (corpus hypothesis). The structure should reflect progressive complexity — each tier tests harder requirement combinations.

## Decision

### Structure

```
tests/corpus/
├── 01_syntax/          Language syntax: literals, expressions, statements, keywords (4)
├── 02_functions/       Functions, generics, pattern matching, closures (4)
├── 03_types/           Type system: ADTs, enums, structs, exhaustive match, Option, Result,
│                       immutability, refinement declarations (Req 1, 3, 4, 5) (14)
├── 04_primitives/      Primitive type operations: int, float, string, bool, char,
│                       bitwise, overflow, unsigned, LLVM codegen for primitives (11)
├── 05_collections/     Collections: List, Map, Set operations, HOF (3)
├── 06_ownership/       Ownership, borrowing, clone heap independence (Req 6) (2)
├── 07_effects/         Effect declarations, propagation, pure vs effectful (Req 7) (8)
├── 08_ifc/             Information flow control: labels, lattice, propagation,
│                       declassification, implicit flow (Req 11) (14)
├── 09_refinements/     Refinement types: valid programs and violations (Req 10) (3)
├── 10_termination/     Total vs partial, structural recursion (Req 8) (1)
├── 11_contracts/       Pre/post conditions, ghost/old, loop invariants (3)
├── 12_actors/          Actor model, capabilities, session types, supervisor,
│                       structured concurrency, data race freedom (Req 9) (11)
├── 13_stdlib/          Stdlib functions: IO, env, crypto, time, random, logging,
│                       file I/O, audit trail, eprint (22)
├── 14_linting/         Linter rules: complexity, antipatterns (2)
├── 15_verification/    Cross-requirement interaction: effect+IFC together,
│                       refinement+totality, ownership+effects. Adversarial cases (3)
├── 16_programs/        Full programs: progressive complexity (reserved)
├── 17_bdd/             BDD naming convention examples (ADR-0020, spec 004 Req 5) (1)

examples/               Multi-file real projects (access_control, log_analyzer).
                        Not compiler tests — showcases.
```

### Progressive complexity ramp

| Tier | Requirements tested | Purpose | Files |
|---|---|---|---|
| 01_syntax | None (syntax only) | Parser correctness | 4 |
| 02_functions | None (syntax only) | Functions, generics, patterns | 4 |
| 03_types | R1, R3, R4, R5 | Type system fundamentals | 14 |
| 04_primitives | R1 | Primitive type operations + LLVM codegen | 11 |
| 05_collections | R1, R3 | Collection types and HOF | 3 |
| 06_ownership | R6 | Ownership in isolation | 2 |
| 07_effects | R7 | Effects in isolation | 8 |
| 08_ifc | R11 | Information flow in isolation | 14 |
| 09_refinements | R10 | Refinement types in isolation | 3 |
| 10_termination | R8 | Termination in isolation | 1 |
| 11_contracts | R1, R10 | Pre/post conditions, invariants | 3 |
| 12_actors | R9 | Actor model, concurrency, race freedom | 11 |
| 13_stdlib | R7 (effects) | Stdlib function testing | 22 |
| 14_linting | None | Linter rule testing | 2 |
| 15_verification | R1-R11 combined | Cross-requirement interaction | 3 |
| 16_programs | All applicable | Realistic programs, progressive | 0 |
| 17_bdd | Spec 004 Req 5 | BDD naming convention (_test.mvl, `mvl test`) | 1 |

### Key splits from previous structure

| Old | New | Rationale |
|---|---|---|
| 01_basics (16 files) | 01_syntax, 02_functions, 04_primitives, 05_collections, 06_ownership, 13_stdlib | Basics was a grab bag — separated by concept |
| 02_types (25 files) | 03_types, 04_primitives, 05_collections | Primitives and collections are not type system |
| 05_effects (26 files) | 07_effects, 13_stdlib | Effect mechanism vs stdlib functions using effects |
| 09_concurrency (11 files) | 12_actors | Renamed: actors are the abstraction, not raw concurrency |

### Distinction from examples/

| | `tests/corpus/` | `examples/` |
|---|---|---|
| Files | Single-file | Multi-file projects |
| Purpose | Compiler test cases | Showcase / documentation |
| Tested by | `cargo test` (include_str!) | `mvl build` / `mvl run` |
| Complexity | One concept per file | Full applications |
| Audience | Compiler developer | MVL user |

### Naming convention

- Files named after the concept they test, not the requirement number
- Comments at top of each file state which requirements are exercised
- Corpus files are VALID programs unless filename contains `_violations` or `_invalid`

## Consequences

- 17-tier structure replaces original 12-tier (issue #1239)
- Primitives, collections, stdlib separated from type system and effects
- Actors renamed from concurrency (concept clarity)
- Functions and syntax separated from the old basics grab bag
- All `include_str!` paths in Rust tests, benchmarks, and Makefile updated
- The ramp doubles as the LLM generation difficulty ramp (Phase 4 #130)

## Connected to

- ADR-0004: Language size (corpus reflects the minimal language surface)
- Phase 4 (#130): Stdlib generation uses this corpus as validation
- Corpus hypothesis (Paper 3): generation quality measured per tier
- Issue #1239: Reorganize test corpus
- Issue #1247: Backend test balance — cross-backend parity tests added for closures (#1250),
  monomorphization (#1251), actors (#1253), and C runtime (#1254) across tiers 02, 12, 13
