---
domain: language
version: 0.1.0
status: draft
date: 2026-04-14
---

# 014 — Data Race Freedom (Partial — Phase 3)

The MVL data race freedom checker covers Requirement 9 (Data Race Freedom) from ADR-0001.
This spec describes the Phase 3 partial proof.  The full architectural proof requires the
actor model (Phase 6).

## Philosophy

Data races are impossible to diagnose at runtime in a deterministic way — they manifest as
intermittent corruption under concurrent load.  The capability system makes race freedom a
compile-time property: the compiler rejects programs where shared mutable state can be observed
concurrently, without requiring programmer-written locks, atomics, or annotations beyond the
capability qualifier on parameters.

**Origin:** Pony reference capabilities (Clebsch et al., 2015) — `iso`, `val`, `ref`, `tag`.
MVL uses a simplified subset (`iso`, `val`, `ref`, `tag`) with the same isolation guarantee.
Unlike Rust's borrow checker, MVL capabilities are per-parameter rather than per-borrow-scope,
which reduces annotation burden for LLM-generated code.

## Reference Capability Model

| Capability | Isolated | Readable | Writable | Sendable |
|------------|----------|----------|----------|----------|
| `iso`      | Yes      | Yes      | Yes      | Yes      |
| `val`      | No       | Yes      | No       | Yes      |
| `ref`      | No       | Yes      | Yes      | No       |
| `tag`      | No       | No       | No       | Yes      |

- **`iso`** (isolated): only one live reference exists at any time.  Must be transferred via
  `consume()`.  Can cross actor boundaries.
- **`val`** (value): deeply immutable.  May be freely shared.  Can cross actor boundaries.
- **`ref`** (reference): locally mutable.  Confined to the creating scope.  Cannot be sent.
- **`tag`** (tag): opaque identity.  No read or write access.

## Scope and Defaults

Phase 3 proves race freedom at the capability level:
- Sendability across actor boundaries (checked by the type checker, Phase 1).
- `iso` isolation: no two live references to the same isolated object.
- Function-level classification: functions with no `ref` parameters are provably race-free.

Phase 6 (actor model) will extend this to:
- Structured concurrency lifetimes bounding task lifetimes.
- Message-passing semantics replacing shared-state access.
- Full architectural proof that no shared mutable state exists across actor boundaries.

## Requirements

### Requirement 1: Sendability [MUST]

Only `iso` and `val` values MAY cross actor boundaries.  The compiler MUST emit
`CheckError::CapabilityViolation` when a `ref` or `tag` value is passed to `channel.send()`.

**Implementation:** `src/mvl/checker.rs::TypeChecker::check_send_capability`

**Tests:** `tests/type_checker.rs::sending_ref_param_rejected`,
`tests/type_checker.rs::sending_iso_param_accepted`,
`tests/type_checker.rs::sending_val_param_accepted`

#### Scenario: ref param rejected at send boundary

- GIVEN `fn send_ref(channel: Channel, ref data: Payload) -> Unit { channel.send(data) }`
- THEN the compiler MUST reject: "`ref` capability of `data` cannot be sent across actor boundary"

**Tests:** `tests/type_checker.rs::sending_ref_param_rejected`

#### Scenario: iso and val params accepted at send boundary

- GIVEN `fn send_iso(channel: Channel, iso data: Payload) -> Unit { channel.send(data) }`
- THEN the compiler MUST accept (`iso` is sendable)

**Tests:** `tests/type_checker.rs::sending_iso_param_accepted`

---

### Requirement 2: iso Isolation — No Aliasing [MUST]

An `iso` value MUST NOT be bound to a new variable without `consume()`.  Writing `let y = iso_x`
creates two simultaneous references to the same isolated object, violating the single-reference
invariant.  The compiler MUST emit `CheckError::IsoAliasingViolation` for any such aliasing.

The canonical ownership-transfer idiom is `consume(iso_x)`, which consumes the original binding.

**Implementation:** `src/mvl/checker/data_race.rs::check_iso_aliasing`

**Tests:** `tests/type_checker.rs::iso_aliasing_without_consume_rejected`,
`tests/type_checker.rs::iso_with_consume_accepted`,
`tests/type_checker.rs::iso_direct_send_accepted`

#### Scenario: iso aliasing rejected

- GIVEN `fn alias(channel: Channel, iso x: Payload) -> Unit { let y = x; channel.send(consume(y)) }`
- THEN the compiler MUST reject: "`iso` value `x` aliased without `consume()`"

**Tests:** `tests/type_checker.rs::iso_aliasing_without_consume_rejected`

#### Scenario: consume() is not aliasing

- GIVEN `fn transfer(channel: Channel, iso item: Payload) -> Unit { channel.send(consume(item)) }`
- THEN the compiler MUST accept (`consume()` transfers ownership without aliasing)

**Tests:** `tests/type_checker.rs::iso_with_consume_accepted`

#### Scenario: val aliasing is allowed

- GIVEN `fn copy_val(val config: Config) -> Unit { let copy = config; consume(copy) }`
- THEN the compiler MUST accept (`val` is immutable — aliasing cannot cause data races)

**Tests:** `tests/type_checker.rs::val_param_aliasing_not_checked`

---

### Requirement 3: Function Race-Freedom Classification [SHOULD]

The assurance pass SHOULD classify each function as provably race-free or requiring actor-model
analysis.  A function is provably race-free if **none** of its parameters carry `ref` capability
(which allows shared mutable access).  Functions with only `iso`, `val`, or unannotated parameters
cannot participate in data races at the capability level.

When ALL top-level functions are provably race-free, the Req 9 verdict MUST be `Proven` with
evidence noting that the full actor model proof is pending Phase 6.  When some functions carry
`ref` parameters, the verdict is `Unchecked` with a count of the proven vs. total functions.

**Implementation:** `src/mvl/checker/data_race.rs::count_race_free_fns`,
`src/mvl/checker/passes.rs::DataRaceFreedomPass`

**Tests:** `src/mvl/checker/passes.rs::tests::req9_proven_for_no_ref_params`,
`src/mvl/checker/passes.rs::tests::req9_unchecked_for_ref_params`

#### Scenario: All functions race-free yields Proven

- GIVEN a program where all functions have only `iso`/`val`/unannotated params
- THEN Req 9 verdict is `Proven` with evidence string referencing Phase 6

**Tests:** `src/mvl/checker/passes.rs::tests::req9_proven_for_no_ref_params`

#### Scenario: ref params yield Unchecked

- GIVEN a program containing `fn local(ref x: Buffer) -> Int { 42 }`
- THEN Req 9 verdict is `Unchecked` (ref param requires actor-model analysis)

**Tests:** `src/mvl/checker/passes.rs::tests::req9_unchecked_for_ref_params`

---

## Known Limitations

### L1: Function-Call iso Transfer

Passing an `iso` variable to a non-`send` function call without `consume()` is not yet detected
as aliasing.  This requires interprocedural capability analysis (Phase 6).

### L2: Closure iso Capture

The MVL parser does not yet support lambda surface syntax.  The aliasing checker
recurses into `Expr::Lambda` bodies (verified via AST-level unit tests), so the
logic is correct, but it cannot be exercised from source strings until lambda
parsing is implemented (Phase 6 / future sprint).

### L3: Struct Field iso Tracking

An `iso` value stored in a struct field and later accessed via field access is not tracked.
Full field-capability propagation requires a dependent type system extension (Phase 6).

### L4: Multiple Alias Sites

Each `let y = iso_x` statement generates a separate `IsoAliasingViolation`.
Multiple alias sites in the same block are each reported independently — this is
the intended behaviour.

### L5: iso Rebinding After consume() [RESOLVED]

After `let y = consume(x)`, the variable `y` becomes the new iso owner and is
added to the tracked iso-vars set.  The original `x` is removed from tracking.
Subsequent aliasing of `y` (e.g., `let z = y`) is now detected as an
`IsoAliasingViolation`.  Branch-scoped iso tracking uses snapshot semantics
(conservative: ownership changes inside branches don't leak to outer scope).

---

*Part of Phase 3 (#129), closes #138.*
