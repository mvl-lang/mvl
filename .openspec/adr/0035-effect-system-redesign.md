# ADR-0035: Effect System Redesign — User-Defined Effects and Aliases

**Status:** Proposed
**Date:** 2026-05-17
**Issues:** #846, #839

---

## Context

The current effect system (Spec 002, implemented in `src/mvl/checker.rs`) has three
structural limitations that become blockers as the stdlib grows.

### Problem 1: Hardcoded effect names

Effects are validated against a compile-time constant:

```rust
const VALID_EFFECT_NAMES: &[&str] = &[
    "Console", "FileRead", "FileWrite", "FileDelete",
    "Net", "DB", "ProcessSpawn", "Random", "CryptoRandom",
    "Clock", "Env", "Log", "Async", "Terminal",
];
```

Users cannot declare domain-specific effects (e.g., `! PaymentGateway`, `! AuditTrail`).
Every new stdlib effect requires a compiler change. This violates the extensibility goal
implied by Spec 002's origin in the Koka and E language traditions.

### Problem 2: Effect proliferation

Effects propagate virally upward with no aliasing mechanism. A typical application
entry point accumulates:

```mvl
fn handle_request(req: Request) -> Response ! Net + DB + Log + Clock + Env { ... }
```

Every internal implementation detail leaks into every public signature in the call chain.
This makes signatures noisy and couples callers to implementation choices of callees.

### Problem 3: No discharge mechanism

The checker enforces that callee effects ⊆ caller effects (propagation-only).
There is no way to "handle" or "absorb" an effect at a boundary. This forces all
effects to be declared all the way to `main`, even when a module author intends an
effect to be an internal implementation detail.

Concrete case: issue #839 proposed moving `std/log.mvl` formatting from Rust to pure
MVL, which requires calling `std.time.now()` (requires `! Clock`). This would force
every `log_info` call site to also declare `! Clock` — a breaking change caused by an
internal formatting decision. The Rust runtime already has an explicit Phase-A
exemption for this (documented in `runtime/rust/src/stdlib/log.rs:23–27`), but that
exemption cannot be expressed in MVL source.

### What already exists

- **Parametrized effects** (`! FileRead("/etc/app/")`) — partial capability scoping,
  implemented with prefix-match subsumption in `effect_satisfies()`.
- **Pony reference capabilities** (ADR-0029) — handle data race freedom orthogonally;
  not in scope here.
- **Effect row polymorphism for HOFs** — partially implemented in `calls.rs:245–267`
  (Phase 1 gap #711); not in scope here.

### Performance constraint

Effect checking is purely compile-time today — zero runtime cost. Any design that
introduces runtime dispatch for effect handling (e.g., Koka-style delimited
continuations, ZIO-style effect layers) trades static guarantees for dynamism and adds
measurable overhead on hot paths. This is unacceptable for Phase A–4.

---

## Decision

### 1. User-defined effect declarations

Remove `VALID_EFFECT_NAMES`. Allow effects to be declared in source with an `effect`
keyword:

```mvl
// In std/effects.mvl (stdlib canonical effects)
pub effect Console
pub effect FileRead
pub effect FileWrite
pub effect Net
// ... etc.

// In user application code
pub effect PaymentGateway
pub effect AuditTrail
```

Effect names remain simple identifiers. Parametrized effects (`! FileRead("/path")`)
continue to work — the parameter is validated against the declared effect's name at
the call site.

**Compiler change:** Replace `VALID_EFFECT_NAMES` constant in `src/mvl/checker.rs`
with a registration pass that collects `effect` declarations from imported modules.
Undeclared effect names remain a compile error (`CheckError::UnknownEffect`).

Builtin effects (those registered from `context.rs` for stdlib functions) are
pre-registered by the checker from `std.effects` before user code is analyzed.

### 2. Effect aliases

Allow a name to stand for a set of effects:

```mvl
// In an application's domain module
effect App = Log + Clock + DB + Net

fn handle_request(req: Request) -> Response ! App { ... }
```

An alias expands at the call site — it is syntactic sugar, not a new effect kind.
The expanded set is what the checker validates against. Effect aliases do not provide
encapsulation (callers still see the full expanded set on error messages).

**Compiler change:** During the declaration pass, resolve alias names to their
constituent effect sets. Store expanded sets in `FnInfo.effects`. Emit an aliased
name in error messages for readability.

### 3. Effect masking (module boundary sealing) — Phase 5

For the `std/log.mvl` problem specifically: a `pub` function in a stdlib module may
declare a **masked** effect set — effects used in the implementation that are not
part of the public contract. This is valid only for `pub` functions in `std.*`
modules, not arbitrary user code.

```mvl
// std/log.mvl
pub fn log_info(msg: String, fields: Map[String, String]) -> Unit ! Log
    masks Clock {
    let ts = now();   // ! Clock — masked, not propagated to callers
    log_write(format_entry(Level::Info, msg, fields, ts));
}
```

The `masks` clause is a trusted annotation: the compiler verifies that the masked
effects are actually used in the body (otherwise the clause is dead), but it does NOT
verify that the masking is semantically safe (i.e., that callers can observe the
effect). Trust is bounded to `std.*` modules, consistent with the existing trust
boundary in ADR-0006.

**Compiler change:** Parse `masks EffectList` as an optional clause on `pub fn`.
During body checking, add masked effects to `current_fn_effects` without adding them
to `fn_info.effects`. Reject `masks` on non-`pub` and non-`std.*` functions.

This unblocks #839 and gives a principled expression for the Phase-A Rust exemption
that currently lives only in a comment.

### 4. Full algebraic handlers — deferred to Phase 8

Koka-style `with Handler in { }` blocks with delimited continuations are explicitly
deferred. Rationale:

- Requires CPS transform or a segmented-stack / fiber model at runtime — not zero-cost.
- Koka's row-polymorphic type inference is a substantial checker investment.
- The problems that motivate handlers (proliferation and sealing) are adequately
  addressed by aliases and `masks` for Phase A–5.
- Phase 8 (actor model, structured concurrency) is the natural home: handlers and
  actor message dispatch are the same mechanism at different granularities.

The syntax sketched in issue #846 (`with ConsoleLogger in { }`) is reserved and
must not be used for any other purpose before Phase 8.

---

## Migration path

| Step | What changes | Breaking? |
|------|-------------|-----------|
| 1 | Add `effect` declarations to `std/effects.mvl` | No — additive |
| 2 | Checker reads declared effects instead of hardcoded list | No — same names |
| 3 | Add effect alias syntax | No — additive |
| 4 | Existing `! Log + Clock + DB` signatures continue to work | No |
| 5 | Stdlib modules adopt aliases (`effect StdObservability = Log + Clock`) | No |
| 6 | `masks` annotation added to stdlib boundary functions | No — tightens contract |
| 7 | User code replaces verbose unions with aliases (opt-in) | No — opt-in |

No existing MVL source files require immediate changes. The migration is purely
additive until a module author opts into aliases or `masks`.

---

## Consequences

**Easier:**
- Domain-specific effects without compiler changes — applications define their own
  `effect` vocabulary.
- Public API signatures stay clean; aliases hide internal effect composition.
- `std/log.mvl` can be rewritten in pure MVL (#839) without leaking `! Clock`.
- Error messages remain precise: alias expansion shown in full on type errors.

**Harder / follow-up:**
- `effect` declaration pass must run before call-site checking — new compiler pass order.
- Alias cycles must be detected (`effect A = B`, `effect B = A`).
- `masks` creates a trust surface: must audit stdlib uses for correctness.
- Full handlers remain future work; the `with` syntax is reserved.

---

## Rejected Alternatives

**Full Koka-style algebraic effect handlers now:** Would fully solve effect discharge
but requires delimited continuations (runtime cost) and row-polymorphic type inference
(significant checker complexity). The Phase-A–5 problems are solvable with less.

**Effect row polymorphism only:** Allows `fn foo[|e]() ! e` but does not reduce
proliferation at call sites — callers still see the full effect set. Deferred to
Phase 8 alongside handlers.

**Effect sealing for all modules (not just std.*):** Generalizing `masks` to user
code removes the trust boundary and makes it easy to accidentally hide effects from
callers. Restricting to `std.*` keeps the trusted surface bounded and auditable.

**Effect inference (implicit, no declarations):** Removes the "effects in signatures"
design principle. Rejected outright — visible effect declarations are a first-class
correctness obligation in MVL (Design Principle 6).

**Keep hardcoded list, just add more names:** Doesn't address extensibility or
proliferation. Rejected as kicking the can.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

| Requirement | Effect |
|-------------|--------|
| **Req 7 — Effect Tracking** | **Strengthens** — user-defined effects extend tracking to domain operations; aliases reduce friction without weakening tracking |
| **Req 3 — Least Privilege** | **Strengthens** — domain effects give fine-grained capability names; `masks` keeps stdlib contracts minimal |
| **Req 9 — Data Race Freedom** | Consistent — effect changes are orthogonal to capabilities (ADR-0029) |
| Req 1–2, 4–6, 8, 10–11 | Consistent |

### Design Principles (README)

- **Effects in signatures** — strengthens: user-defined effects extend the vocabulary without weakening the guarantee; every side effect still declared
- **Pure by default** — consistent: no change to purity rules
- **Correctness by construction** — strengthens: `masks` makes the Phase-A Rust exemption a compiler-checked annotation rather than a comment
- **Reuse over reinvention** — consistent: building on Koka/E language tradition from Spec 002, not replacing it
- **Minimality** (ADR-0002) — tension — explained: three new syntactic forms (`effect`, alias, `masks`) add surface area; accepted because each solves a concrete blocker

### Specifications

- **002-effect-system/spec.md** — add Requirements 7 (user-defined effects), 8 (effect
  aliases), 9 (effect masking). Update Requirement 2 to reference the new declaration
  mechanism instead of the hardcoded list.
- **014-data-race-freedom/spec.md** — no change; capabilities remain orthogonal.
- No other specs require immediate update.
