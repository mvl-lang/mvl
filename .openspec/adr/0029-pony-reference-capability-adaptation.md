# ADR-0029: Pony Reference Capability Adaptation for MVL

**Status:** Accepted
**Date:** 2026-05-14
**Issues:** #700, #506

---

## Context

MVL's data race freedom guarantee (Req 9) requires a compile-time capability system that
prevents shared mutable state from crossing actor boundaries. Pony's reference capabilities
(Clebsch et al., 2015 — "Deny Capabilities for Safe, Fast Actors") provide a proven,
production-tested foundation: four capabilities (iso, val, ref, tag) cover the common
patterns of isolated ownership, deep immutability, local mutability, and opaque identity.

Without an ADR, the rationale for MVL's specific adaptation — what is taken from Pony, what
is dropped, and what is deliberately changed — is undocumented. This creates risk of silent
drift across PRs that touch the type system, actor model, or either backend.

This ADR formalises the adaptation and locks down the five decisions that have architectural
scope: the capability set, iso recovery, the Capability/TypeExpr split, cross-backend
applicability, and the Phase 3 vs. Phase 8 boundary.

Spec 014 (`014-data-race-freedom/spec.md`) documents the capability model table, sendability
matrix, and Phase 3 requirements in detail. This ADR documents the *why* and the *trade-offs*;
it does not duplicate the spec.

---

## Decision

### 1. Adopt Pony's four capabilities: iso, val, ref, tag

MVL uses exactly the four Pony capabilities with the same core semantics:

| Capability | Isolated | Readable | Writable | Sendable |
|------------|----------|----------|----------|----------|
| `iso`      | Yes      | Yes      | Yes      | Yes      |
| `val`      | No       | Yes      | No       | Yes      |
| `ref`      | No       | Yes      | Yes      | No       |
| `tag`      | No       | No       | No       | Yes      |

Pony's additional capabilities (`box`, `trn`) are not adopted. `box` (read-only view of a
mutable object) and `trn` (transition from mutable to immutable) add complexity without
covering any Phase 3–8 use case that `ref` or `val` cannot handle. Per ADR-0002 (language
contraction), capabilities that serve no current requirement are excluded.

### 2. No iso recovery via recover expressions

Pony supports `recover` blocks that can promote a `ref` graph to `iso` by proving no external
aliases exist. MVL does not support this mechanism.

Rationale: iso recovery requires alias-graph analysis across the whole sub-expression, which
conflicts with MVL's goal of local, per-parameter annotation (rather than scope-scoped borrow
analysis). Iso is only created at declaration: a parameter declared `iso` is isolated at the
call boundary. Ownership transfer uses `consume(x)`, which moves the iso binding and makes the
original name unavailable. Post-consume aliasing tracking (i.e., detecting that the receiver of
`consume()` is not subsequently re-aliased) is deferred to Phase 8 and documented as limitation
L1/L5 in Spec 014.

### 3. Capabilities are per-parameter, not per-borrow-scope

MVL capabilities annotate function parameters (`iso data: Payload`), not individual borrows.
This is a deliberate divergence from Rust's borrow checker, which tracks capabilities per
reference lifetime within a scope.

Per-parameter annotation:
- Reduces annotation burden for generated and LLM-written code (one annotation at the boundary)
- Is sufficient for actor isolation (the actor boundary is a function call)
- Is consistent with Pony's design, where capabilities describe what a reference *is*,
  not what you can *do* with it in a given scope

Local memory safety (stack borrows, `&T` vs `&mut T`) uses `TypeExpr::Ref { mutable }` in the
AST and is resolved separately during Rust-backend emission. These are distinct concerns:
- `Capability` (iso/val/ref/tag) — concurrency safety, enforced by the checker
- `TypeExpr::Ref` — local borrow semantics, resolved by the Rust emitter

### 4. Capability verification is pre-backend and applies uniformly to all backends

The capability checker runs as part of the `CheckerPass` before any backend-specific code
generation. Both the Rust transpiler and LLVM backends inherit the same capability proofs:
no duplication of safety logic in either backend is required or permitted.

LLVM IR has no native capability model; the checker's static proof IS the safety guarantee
for LLVM-compiled programs.

### 5. Phase 3 / Phase 8 boundary

Phase 3 (complete) proves:
- Sendability at channel/actor boundaries (`iso`, `val`, `tag` sendable; `ref` rejected)
- Direct `iso` aliasing (`let y = iso_x` without consume)
- Function-level race-freedom classification (functions with no `ref` parameters)

Phase 8 (Phase 8, this epic) extends to:
- Full actor spawn/terminate lifecycle with capability-correct message passing
- Post-consume ownership tracking (L1, L5 in Spec 014)
- Structured concurrency scope lifetimes
- Model checker integration for pre/post-condition reasoning across actor boundaries

The Phase 3 limitations (L1–L6 in Spec 014) are accepted as known gaps, not bugs, and are
tracked against Phase 8 work.

---

## Consequences

**Easier:**
- Actor model (Phase 8) has a stable, documented capability foundation to build on
- Sendability matrix is the single source of truth (Spec 014 + this ADR) — no ambiguity
  in backend PRs about what is or is not safe to cross an actor boundary
- The Capability/TypeExpr split is documented; reviewers can distinguish concurrency bugs
  from local borrow bugs without re-deriving the distinction

**Harder / follow-up:**
- Ownership transfer tracking post-`consume()` remains a Phase 8 deliverable (L1, L5)
- Struct field iso tracking (L3) and function-argument iso transfer (L1) are not Phase 3
- Any future capability extension (e.g., a `box`-like read-only view) must update this ADR

---

## Rejected Alternatives

**Adopt Rust's borrow checker:** Rust's borrow system is per-scope and lifetime-based. It is
more expressive but requires lifetime annotations on all references. MVL targets LLM-generated
code; lifetime annotations introduce a class of errors that are hard to recover from
automatically. Per-parameter capabilities are sufficient for actor isolation.

**Adopt all six Pony capabilities (box, trn):** `box` and `trn` serve transition and read-view
patterns not required in Phase 3–8. Adding them would complicate the sendability matrix and
checker without enabling any planned feature. Revisit if a concrete use case arises.

**Infer capabilities from usage:** Capability inference (like Pony's `ref by default`) would
reduce annotations but would make capability violations harder to explain (inferred origin vs.
explicit declaration). Explicit annotation is consistent with MVL's principle of making
correctness obligations visible.

**Ownership-track post-consume in Phase 3:** Full ownership rebinding tracking requires
dataflow analysis across the function body, which is a substantial checker extension. The
Phase 3 benefit (catching L1/L5) does not justify the complexity ahead of the actor model
that motivates the feature.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

| Requirement | Effect |
|-------------|--------|
| **Req 9 — Data Race Freedom** | **Strengthens** — this ADR formalises the capability model that proves Req 9 at compile time; the sendability matrix makes the guarantee precise |
| **Req 1 — Type Safety** | Consistent — capabilities extend the type system but do not change type safety rules |
| **Req 6 — Ownership / Move Semantics** | Consistent — `consume()` and iso transfer align with the ownership model; no conflict |
| Req 2–5, 7–8, 10–11 | Consistent |

### Design Principles (README)

- **Reuse over Reinvention** — strengthens: Pony's capability system is proven in production
  (the Pony language has used it since 2015); MVL adopts it rather than inventing a new model.
- **Correctness by Construction** — strengthens: capability violations are compile-time errors,
  not runtime faults; race freedom is a theorem, not a test result.
- **Explicit over Implicit** — strengthens: capabilities are explicit per-parameter annotations;
  no inference, no defaults that hide safety assumptions.
- **Minimality** — consistent: exactly four capabilities, matching the four Pony capabilities
  needed; `box` and `trn` excluded per ADR-0002.
- **Simplicity** — consistent: per-parameter annotation is simpler than per-scope lifetime
  tracking for the actor-boundary use case MVL targets.
- Remaining principles — consistent.

### Specifications

- `.openspec/specs/014-data-race-freedom/spec.md` — canonical capability model, sendability
  matrix, Phase 3 requirements, and known limitations. This ADR does not duplicate that content;
  it provides the architectural rationale.
- `.openspec/specs/013-actors/` (Phase 8, forthcoming via #701) — will extend Spec 014 with
  full actor lifecycle semantics grounded in this capability model.
- No other spec files require immediate update.
