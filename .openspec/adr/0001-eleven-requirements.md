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
- Capability security → fold into Req 11 (IFC labels as capability tokens, #931) + std/audit (runtime policy)
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

## Implementation Status (v0.5.2)

| # | Requirement | Parsed | Checked | Transpiled | Notes |
|---|------------|--------|---------|------------|-------|
| 1 | Type safety (ADTs) | ✓ | ✓ | — | Structs, enums, field validation, type inference |
| 2 | Memory safety | ✓ | ✓ partial | — | Use-after-move detected. No borrow lifetime analysis yet. |
| 3 | Totality (exhaustive match) | ✓ | ✓ | — | Non-exhaustive match rejected at compile time |
| 4 | Null elimination (Option) | ✓ | ✓ | — | Direct field access on Option rejected |
| 5 | Error visibility (Result) | ✓ | ✓ | — | Unused Result rejected, `?` propagation parsed |
| 6 | Ownership (linearity) | ✓ | ✓ | — | Immutability enforced. Linear resource consumption (LinearTypeBareBind) enforced — bare assignment of linear type without consume() rejected. |
| 7 | Effect tracking | ✓ | ✓ | — | Undeclared effects rejected, propagation enforced |
| 8 | Termination | ✓ | ✓ partial | — | `while` in total rejected. No structural recursion proof yet. |
| 9 | Data race freedom | ✓ | ✓ partial | — | ref/tag capabilities parsed. Full actor-boundary checking Phase 2. |
| 10 | Refinement types | ✓ | ✓ | — | Static call-site check (RefinementViolated). Phase 2 adds SMT solver. |
| 11 | Information flow control | ✓ | ✓ | — | Lattice enforced, declassify/sanitize required. Phase 2 adds full flow analysis. |

**Summary:** All 11 requirements are fully represented in the grammar and enforced in the type checker. Reqs 2, 8, 9 are partial — core violations caught, deeper analysis deferred to Phase 2. Req 6 is fully proven at Phase 1.

### Readiness targets

- **Phase 1 complete (transpiler):** All 11 enforced at compile time. Transpiler maps MVL constructs to Rust for execution.
- **Phase 2 (LLVM):** Deeper enforcement — Req 10 via SMT solver, Req 11 via full flow analysis, Req 2/6/8/9 completing their partial implementations
- **Phase 3 (ecosystem):** Self-hosting, with assurance reports documenting per-module satisfaction

### Rust as transpilation target

Rust covers 6 of the 11 requirements natively (Reqs 1–6: type safety, memory safety, totality, null elimination, error visibility, ownership). The MVL compiler adds the remaining 5 (Reqs 7–11: effect tracking, termination, data race freedom, refinements, IFC) as a verification layer before emitting Rust.

## Consequences

- The MVL scores 11/11 by construction. No existing language exceeds 9.5/11 (F*).
- The annotation burden is real but borne by the LLM, not the human.
- The compiler is the trust boundary — one compiler, one proof chain.
