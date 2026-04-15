# ADR-0010: Corpus Test Structure — Progressive Complexity Ramp

**Status:** Accepted
**Date:** 2026-04-15
**Context:** The test corpus in `tests/corpus/` serves two purposes: (1) compiler test cases ensuring each requirement is enforced, and (2) a training/validation corpus for LLM generation quality (corpus hypothesis). The structure should reflect progressive complexity — each tier tests harder requirement combinations.

## Decision

### Structure

```
tests/corpus/
├── 01_basics/          Language syntax: literals, expressions, statements, functions, keywords
├── 02_types/           Type system: ADTs, enums, structs, exhaustive match, Option, Result,
│                       immutability, refinement declarations (Req 1, 3, 4, 5)
├── 03_stdlib/          Stdlib usage: collections, map/set literals, string ops, format
│                       (#42, #43, #64, #67 — library functions, not type system)
├── 04_ownership/       Ownership and borrowing (Req 6)
├── 05_effects/         Effect declarations, propagation, pure vs effectful (Req 7)
├── 06_ifc/             Information flow control: labels, lattice, propagation,
│                       declassification, implicit flow (Req 11)
├── 07_refinements/     Refinement types: valid programs and violations (Req 10)
├── 08_termination/     Total vs partial, structural recursion (Req 8)
├── 09_concurrency/     Reference capabilities, data race freedom (Req 9)
├── 10_verification/    Cross-requirement interaction: effect+IFC together,
│                       refinement+totality, ownership+effects. Adversarial cases.
├── 11_programs/        Full programs: progressive complexity from hello_world to
│                       auth_handler. Each uses more requirements than the last.

examples/               Multi-file real projects (access_control, log_analyzer).
                        Not compiler tests — showcases.
```

### Progressive complexity ramp

| Tier | Requirements tested | Purpose |
|---|---|---|
| 01_basics | None (syntax only) | Parser correctness |
| 02_types | R1, R3, R4, R5 | Type system fundamentals |
| 03_stdlib | R1, R3, R4, R5 | Library function verification |
| 04_ownership | R6 | Ownership in isolation |
| 05_effects | R7 | Effects in isolation |
| 06_ifc | R11 | Information flow in isolation |
| 07_refinements | R10 | Refinement types in isolation |
| 08_termination | R8 | Termination in isolation |
| 09_concurrency | R9 | Data race freedom in isolation |
| 10_verification | R1-R11 combined | Cross-requirement interaction |
| 11_programs | All applicable | Realistic programs, progressive |

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

- Stdlib tests separated from type system tests (was mixed in 02_types)
- New 10_verification tier for adversarial cross-requirement cases
- Full programs renumbered to 11 (was 09) to leave room for verification tier
- All `include_str!` paths in Rust tests updated
- The ramp doubles as the LLM generation difficulty ramp (Phase 4 #130)

## Connected to

- ADR-0004: Language size (corpus reflects the minimal language surface)
- Phase 4 (#130): Stdlib generation uses this corpus as validation
- Corpus hypothesis (Paper 3): generation quality measured per tier
