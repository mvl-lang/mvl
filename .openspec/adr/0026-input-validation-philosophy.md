# ADR-0026: Input Validation Philosophy — Post-Postel Strictness

**Status:** Accepted
**Date:** 2026-05-11
**Issues:** #614

---

## Context

MVL uses refinement types (Req 10) to prove properties at compile time. Inside the system, the solver can verify that `x: Int where x > 0` holds — because the value's origin is known. At input boundaries (user input, files, network), values are *unknown*: the solver cannot prove properties statically without an explicit validation step.

Two historical philosophies exist for handling this gap:

- **Postel's Law (RFC 761, 1980):** "Be liberal in what you accept, conservative in what you send." Tolerate malformed or unexpected input; try to make it work.
- **RFC 9413 (2023):** "Robustness Principle Reconsidered." Postel's liberal acceptance created decades of security vulnerabilities — parsers that accept garbage enable exploits. Be strict everywhere.

MVL must document its position so that stdlib authors, compiler implementors, and language users apply the same policy at every input boundary.

---

## Decision

**MVL is post-Postel.** The policy has three parts:

1. **Syntactic tolerance permitted** — a parser MAY accept multiple equivalent formats for the same value (e.g., `"05/09/2026"` and `"2026-05-09"` for a date). This is a representation concern, not a correctness concern.

2. **Semantic strictness required** — every value entering the system MUST satisfy its refinement predicate before it is used. The solver enforces this; there is no bypass.

3. **Invalid input rejected, not coerced** — a value that fails its refinement is rejected. Silent conversion (e.g., clamping `-5` to `0` for an `age: Int where age >= 0`) is not permitted. The caller must handle the rejection explicitly.

The boundary pattern for all input-receiving code:

```
External world (untrusted)
        ↓
    [ Parser ]     <- syntactic tolerance: multiple formats OK
        ↓
    [ Validator ]  <- semantic strictness: prove refinement or reject
        ↓
Internal world (proven)
```

Once a value crosses the validator, its refinement is proven and the solver treats it as trusted. Before the validator, its type carries the `Tainted` IFC label (Req 11).

---

## Consequences

**Easier:**
- Security guarantees are mechanical: no tainted value can enter the verified core without an explicit validation step.
- Composability with IFC (Req 11): the `Tainted` label enforces the boundary at the type level.
- Auditing: validation is always explicit and grep-able — no hidden coercion to hunt down.
- Stdlib contracts are simpler: functions in the proven world can assert their preconditions without defensive fallbacks.

**Harder:**
- More explicit code at boundaries: callers must handle `Result`/`Option` returns from validators — there is no silent success.
- Stdlib needs boundary validation primitives (e.g., `validate`, `parse_or_reject`) that return `Result<T where P, ValidationError>`.

**Follow-up work:**
- Stdlib boundary module with typed validators (spec update to `docs/stdlib.md`).
- Corpus tests demonstrating tainted-to-proven transition at boundaries.

---

## Rejected Alternatives

**Full Postel's Law (accept everything, coerce silently)**
Rejected. Silent coercion (`-5 → 0`, `"foo" → 0`) hides bugs and creates unpredictable behaviour. RFC 9413 documents the historical damage. MVL's design principle "Explicit over implicit" (DP 1) prohibits hidden conversions.

**Full strictness on syntax too (reject all non-canonical forms)**
Rejected. Syntax variations for equivalent values (date formats, whitespace in CSV) are a representation concern, not a correctness concern. Rejecting them would make MVL hostile to real-world data without improving safety.

**Coerce with a warning / lint**
Rejected. A warning can be silenced; a rejected value cannot. "Strictness enables proof" (RFC 9413's framing) requires hard rejection, not advisory rejection.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

| Req | Effect | Rationale |
|-----|--------|-----------|
| 10 — Refinement types | **Strengthens** | Refinements are enforced at all input boundaries, not just internally. The solver's guarantees extend to the system edge. |
| 11 — Information flow control | **Strengthens** | The `Tainted` label on unvalidated input makes this boundary mechanically enforced at the type level. Tainted values cannot enter the proven core without an explicit validator. |
| 1–9 | Leaves unchanged | This decision is about boundary policy, not type structure, ownership, effects, or termination. |

### Design Principles (README)

- **DP 1 — Explicit over implicit:** **Strengthens** — validation is explicit at every boundary; no silent coercion is possible.
- **DP 7 — Security labels on all data:** **Strengthens** — unvalidated input carries `Tainted`; the post-Postel policy gives this label teeth at the boundary.
- **DP 10 — Refinement types inline:** **Strengthens** — `x: Int where x > 0` at a boundary is the validation expression, not a post-hoc assertion.
- **DP 2 — One way to do each thing:** consistent with — there is exactly one pattern: parse then refine.
- **DP 5 — Immutable by default:** consistent with — validated values enter the proven world as immutable, unchanged.
- **DP 6 — Effects in signatures:** consistent with — validators that do I/O carry an effect annotation; the policy does not change this.

### Specifications

- **Spec 001 (type system):** The validation boundary interacts with refinement type checking. The spec should note that refinements on function parameters are enforced at call sites, and at input boundaries via explicit validators.
- **Spec 003 (IFC):** The `Tainted` label is the mechanical enforcement of this policy. No spec update required; this ADR strengthens the existing rationale.
- No other specs are directly affected.
