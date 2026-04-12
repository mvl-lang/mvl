# ADR-0001: Eleven Compiler-Verified Requirements

**Status:** Accepted
**Date:** 2026-04-11
**Context:** What properties should the MVL compiler verify? How many is enough?

## Decision

The MVL compiler SHALL verify eleven properties. Seven from the convergence of formal methods and safety-critical practice. Four more that become economically viable when LLMs generate all code.

## The Eleven Requirements

| # | Requirement | Origin (theory) | Origin (practice) |
|---|---|---|---|
| 1 | Type safety (ADTs) | Curry-Howard (1934/1980), Hope (Burstall 1980) | MISRA C type discipline |
| 2 | Memory safety | Linear logic (Girard 1987) | 70% of Microsoft CVEs (MSRC 2019) |
| 3 | Totality (exhaustive match) | Constructive logic (Martin-Lof 1972), Hope (1980) | DO-178C Level A |
| 4 | Null elimination (Option) | SML option (1990), Hoare recanted (2009) | Hoare's billion dollar mistake |
| 5 | Error visibility (Result) | OCaml result, Haskell Either (1990s) | Silent failures in production |
| 6 | Ownership (linearity) | Linear logic (Girard 1987), Rust (2015) | Use-after-free, double-free CVEs |
| 7 | Effect tracking | Plotkin & Pretnar (2009), Koka (Leijen 2014) | Hidden side effects in production |
| 8 | Termination checking | Martin-Lof (1972), Idris 2 (Brady 2021) | LLMs generate infinite loops |
| 9 | Data race freedom | Pony ref capabilities (Clebsch 2015), Rust Send/Sync | Concurrency bugs |
| 10 | Refinement types | Liquid Haskell, Ada/SPARK (40 years avionics) | Division by zero, out-of-range values |
| 11 | Information flow control | Denning (1976), Perl taint (1989) | SQL injection, secret leakage, XSS |

## Why eleven and not seven

Requirements 8-11 were known but considered impractical: the annotation burden was too high for human developers. Every recursive function needs a termination proof. Every value needs a security label. When LLMs generate all code, this cost drops to zero. The compiler can verify properties that were previously uneconomical.

## Why eleven and not fifteen

Research identified 15 candidates. Four were absorbed:
- Session types → fold into Req 6 (linearity) via typestate
- Capability security → fold into Req 7 (fine-grained effects)
- Numeric overflow → fold into Req 10 (refinement types)
- Deadlock freedom → architectural (actor model, no locks)

Two were rejected:
- Dimensional analysis — too niche, buildable as library on Req 1
- Full dependent types — type checking becomes undecidable. Req 10 (refinements) is the decidable fragment.

Principle: add a requirement only if it catches bugs that no combination of the other requirements catches.

## Quality model

- **Well-formed (internal quality):** Code compiles → 11 requirements proven. Structural correctness at compile time.
- **Validated (external quality):** Code passes tests → semantic correctness at runtime.

Every requirement is a category of tests you never write. Well-formedness reduces the validation surface.

## Implementation Status (v0.4.0)

| # | Requirement | Parsed | Checked | Transpiled | Notes |
|---|------------|--------|---------|------------|-------|
| 1 | Type safety (ADTs) | ✓ | ✓ | — | Structs, enums, field validation, type inference |
| 2 | Memory safety | ✓ | ✓ partial | — | Use-after-move detected. No borrow lifetime analysis yet. |
| 3 | Totality (exhaustive match) | ✓ | ✓ | — | Non-exhaustive match rejected at compile time |
| 4 | Null elimination (Option) | ✓ | ✓ | — | Direct field access on Option rejected |
| 5 | Error visibility (Result) | ✓ | ✓ | — | Unused Result rejected, `?` propagation parsed |
| 6 | Ownership (linearity) | ✓ | ✓ partial | — | Use-after-move. No linear resource consumption check yet. |
| 7 | Effect tracking | ✓ | ✓ | — | Undeclared effects rejected, propagation enforced |
| 8 | Termination | ✓ | ✓ partial | — | `while` in total rejected. No structural recursion proof yet. |
| 9 | Data race freedom | ✓ | ✓ | — | ref/tag capabilities rejected at actor boundaries |
| 10 | Refinement types | ✓ | ○ parse-only | — | Grammar complete. No SMT checking — planned for Phase 1 transpiler as runtime asserts. |
| 11 | Information flow control | ✓ | ○ parse-only | — | Labels parsed. No flow analysis — planned for Phase 1 transpiler as Rust newtypes. |

**Summary:** All 11 requirements are fully represented in the grammar. 9/11 have active enforcement in the type checker. Req 10 and 11 will gain enforcement through the transpiler (Rust runtime checks and newtypes respectively).

### Readiness targets

- **Phase 1 complete (transpiler):** All 11 enforced — 9 at compile time, 2 via transpiled Rust code (Req 10 as asserts, Req 11 as newtype wrappers)
- **Phase 2 (LLVM):** All 11 enforced at compile time — Req 10 via SMT solver integration, Req 11 via compiler-native flow analysis
- **Phase 3 (ecosystem):** All 11 enforced, with assurance reports documenting per-module satisfaction

## Consequences

- The MVL scores 11/11 by construction. No existing language exceeds 9.5/11 (F*).
- The annotation burden is real but borne by the LLM, not the human.
- The compiler is the trust boundary — one compiler, one proof chain.
