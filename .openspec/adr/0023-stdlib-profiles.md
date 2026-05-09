# ADR-0023: Stdlib Profiles — Trusted vs Proven

**Status:** Accepted
**Date:** 2026-05-09
**Issues:** #533, #539, #541, #542
**Related:** ADR-0007 (import model), ADR-0019 (two-path stdlib), ADR-0022 (three-category model)

---

## Context

The MVL stdlib is implemented in two layers (ADR-0019):

1. The `mvl_runtime` / `mvl_runtime_c` Rust crates — real implementations that
   are memory-safe and well-tested but not expressed in MVL.
2. `pub builtin fn` declarations in `.mvl` files — type-checked signatures that
   bind to the Rust implementations at link time.

This architecture means the compiler can verify *your* code against all 11
requirements, but the stdlib *implementations* are trusted without proof.

For most programs this is acceptable.  For safety-critical systems (DO-178C,
IEC 61508, ISO 26262) the certification authority may require proof coverage that
extends into the standard library.

The question this ADR addresses: **how do we expose this distinction to users and
toolchains without breaking existing programs?**

---

## Decision

### 1. Two named profiles

| Name | CLI flag | Behaviour |
|------|----------|-----------|
| `trusted` | *(default, or `--stdlib=trusted`)* | `pub builtin fn` backed by Rust runtime; 11 requirements verified on user code only |
| `proven` | `--stdlib=proven` | stdlib is MVL source; 11 requirements verified on user code *and* stdlib |

The default is `trusted` so existing programs are unaffected.

### 2. Profile flag syntax

`--stdlib=<profile>` on all compiler sub-commands (`check`, `build`, `run`, `test`).
Unknown profile names are fatal errors with a suggestion.

### 3. Irreducible builtins remain trusted in `proven` mode

A small set of OS-level and hardware-level operations cannot be expressed in MVL
(syscalls, hardware intrinsics, atomic operations at the VM level).  These are
documented per-module in `docs/trust-boundary.md` (pending #538) and minimised
by design.  They remain `pub builtin fn` even in `proven` mode.

### 4. Incremental rollout

`proven` mode is accepted by the CLI today (#539) but falls back to `trusted`
until the MVL stdlib implementations are written (#538).  A note is printed on
stderr.  This allows toolchains and CI pipelines to adopt the flag now.

---

## Consequences

### Positive

- Existing programs require no changes (trusted is the default).
- Safety-critical users have a clear upgrade path.
- The distinction between "verified" and "trusted" is explicit in the source
  tree and in compiler output.
- CI can pin `--stdlib=proven` today and start failing when #538 ships if
  something regresses.

### Negative

- Two code paths to maintain once #538 lands.
- The exact boundary of "irreducible builtins" will require careful documentation
  and review.

### Follow-up work

- #538 — Write MVL implementations for the non-OS stdlib functions.
- Trust-boundary doc update once #538 finalises the numbers.

---

## Rejected Alternatives

**Single profile with an annotation per builtin.**  Considered marking individual
`pub builtin fn` declarations with `#[trusted]` vs `#[proven]`.  Rejected because
it pushes the profile concept into every stdlib file and makes the global switch
harder to implement.

**No profiles — always verify everything.**  Would require #538 to be done before
the flag could exist, and would be a breaking change for current users.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

This decision does not weaken any requirement.

- **Req 1–11 (user code):** unchanged — all profiles apply all 11 requirements to
  user-written MVL programs.
- **Req 1–11 (stdlib):** the `proven` profile *strengthens* coverage by extending
  verification into stdlib source.  The `trusted` profile leaves it unchanged
  relative to the pre-ADR state.

### Design Principles (README)

- **Safety** — strengthened: `proven` mode provides a path to full formal
  evidence for safety-critical users.
- **Simplicity** — consistent with: the flag is optional; trusted is the default.
- **Correctness** — consistent with: both profiles apply all 11 requirements to
  user code; the difference is only in stdlib coverage.
- **Transparency** — strengthened: the trust boundary is made explicit and
  documented rather than implicit.

### Specifications

No spec files in `.openspec/specs/` are directly affected.  The stdlib module
specs in `docs/stdlib.md` should be updated once #538 finalises which functions
move to proven status.
