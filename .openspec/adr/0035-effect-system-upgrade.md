# ADR-0035: Effect System Upgrade — Subsumption and std/effects.mvl

**Status:** Proposed
**Date:** 2026-05-17
**Issues:** #846

---

## Context

The current effect system (002-effect-system) was designed for security and functional safety tracking. The implementation has drifted from this intent:

1. **Hardcoded effects** — `VALID_EFFECT_NAMES` in `src/mvl/checker.rs` is a Rust constant. Effects should be defined in MVL.

2. **FFI absorption** — Some effects (Clock in Log) are hidden in Rust runtime implementations. This violates "full truth in signature."

3. **Coarse concurrency** — Single `Async` effect covers spawning, sending, and receiving. These are distinct security concerns.

4. **Totality conflation** — Spec says "`partial` is semantically an effect" but it's implemented as a function prefix. Totality is orthogonal to effects.

5. **No subsumption** — Every effect must be listed explicitly. `Log` uses `Clock` internally but callers must declare both, or Clock is hidden via FFI.

The original design intent was correct: effects track what code CAN do, for security audit and functional safety. The implementation needs to catch up.

---

## Decision

### 1. Effects Declared in MVL Source

Effects are declared in MVL source, not hardcoded in the compiler. Base effects live in `std/effects.mvl`. User code can declare domain-specific effects.

```mvl
// std/effects.mvl — base effects
effect Clock
effect Console
effect FileRead
effect FileWrite
effect FileDelete
effect Net
effect DB
effect ProcessSpawn
effect Env
effect Random
effect Spawn
effect Send
effect Recv

// user code — domain effects
effect Billing > DB + Log
effect Notification > Net + Log
```

The compiler uses dual-pass compilation:
1. **Parse pass:** Parse all files, collect `EffectDecl` nodes (no validation)
2. **Resolve pass:** Build hierarchy, validate parents exist, detect cycles
3. **Check pass:** Type-check with complete hierarchy

No special ordering. `std/effects.mvl` is just another file.

### 2. Subsumption Hierarchy

Effects can subsume other effects. If `A > B`, declaring `! A` satisfies `! B` requirements:

```mvl
// std/effects.mvl

// Log uses Clock internally for timestamps
effect Log > Clock

// CryptoRandom is stronger than Random
effect CryptoRandom > Random

// IO is a convenience parent for all I/O effects
effect IO > Console
effect IO > FileRead
effect IO > FileWrite
effect IO > FileDelete
effect IO > Net
effect IO > DB
effect IO > ProcessSpawn
effect IO > Env
effect IO > Log

// Actor is a convenience parent for concurrency effects
effect Actor > Spawn
effect Actor > Send
effect Actor > Recv
```

Subsumption is transitive: `IO > Log > Clock` means `! IO` satisfies `! Clock`.

### 3. Syntax

```
effect_decl = "effect" IDENT [ ">" IDENT ( "+" IDENT )* ] ;
```

Examples:
- `effect Clock` — base effect
- `effect Log > Clock` — subsumes Clock
- `effect Billing > DB + Log + Clock` — subsumes multiple effects

### 4. No Aliases

Subsumption replaces aliases. Instead of `effect IO = Console + FileRead + ...`, we use multiple subsumption declarations. One mechanism, not two.

### 5. Totality is Separate

Remove "partial is semantically an effect" from spec. Totality (`partial` prefix) is orthogonal to effects. A function can be:
- `partial fn server() ! Net` — non-terminating, does network
- `fn pure_calc() -> Int` — total, pure
- `fn read_config() ! FileRead` — total, effectful

### 6. Replace Async with Fine-Grained Concurrency

Remove `Async`. Add:
- `Spawn` — can create actors
- `Send` — can send on channels
- `Recv` — can receive on channels (blocking)

These are distinct security concerns (resource exhaustion, data exfiltration, blocking).

### 7. Drop FFI Absorption

All effects must be explicit. If `log_debug` uses `now()` internally, either:
- `Log > Clock` subsumption handles it (preferred)
- Or `log_debug` declares `! Log + Clock`

No hiding effects in Rust FFI implementations.

---

## Consequences

### Positive

- **MVL defines itself** — Effects in std/effects.mvl, not Rust constants
- **One mechanism** — Subsumption covers both "A uses B" and "A groups B, C, D"
- **Explicit security tracking** — No hidden effects in FFI
- **Fine-grained concurrency** — Spawn/Send/Recv auditable separately
- **Cleaner separation** — Totality is its own concern, not conflated with effects

### Negative

- **Migration** — Existing `! Async` code needs updating to `! Actor` or specific effects
- **std bootstrap** — Compiler must parse std/effects.mvl before checking user code
- **Subsumption cycles** — Need to detect and reject `A > B > A`

### Follow-up Work

- Update spec 002-effect-system
- Implement `effect` declaration parsing
- Implement subsumption checking in type checker
- Create std/effects.mvl
- Remove `VALID_EFFECT_NAMES` constant
- Update all `! Async` in stdlib to `! Spawn`, `! Send`, or `! Recv`
- Audit runtime FFI for hidden effect usage

---

## Rejected Alternatives

### Aliases + Subsumption

Two mechanisms: `effect IO = A + B` (alias) and `effect Log > Clock` (subsumption). Rejected because:
- Subsumption alone covers both use cases
- Simpler mental model
- One mechanism to learn

### Algebraic Effects with Handlers (Koka-style)

Handlers discharge effects at boundaries. Rejected because:
- MVL's goal is tracking for security audit, not abstraction
- Handlers make signatures interfaces, not facts
- "Full truth in signature" requires effects to propagate, not discharge
- Closest language is Austral (capability tracking), not Koka (effect abstraction)

### Ambient Effects

Some effects (Log, Clock) don't propagate. Rejected because:
- Violates "full truth in signature"
- Security audit needs to see all effects
- Subsumption handles the verbosity concern without hiding

### Keep Totality as Effect

`! Partial` instead of `partial fn`. Rejected because:
- Totality is about the function's termination behavior, not operations it performs
- Similar to `pub` or `async` — metadata about the function, not a side effect
- Already implemented as prefix, spec was aspirational

### Hardcoded std/effects.mvl Bootstrap

Load std/effects.mvl specially before user code. Rejected because:
- Dual-pass (parse first, resolve later) handles forward references naturally
- No special ordering needed
- std/effects.mvl is just another file
- User-defined effects work the same way

---

## Relation to Language Definition

### Eleven Requirements (ADR-0001)

| Requirement | Impact |
|-------------|--------|
| Req 7 (Effect tracking) | **Strengthens** — explicit subsumption, no FFI hiding, fine-grained concurrency |
| Req 8 (Termination) | **Unchanged** — explicitly decoupled from effects |
| Req 9 (Data race freedom) | **Strengthens** — Spawn/Send/Recv are auditable separately |

### Design Principles (README)

- **Principle 6 (Effects in signatures):** **Strengthens** — no FFI absorption, full truth restored
- **Principle 4 (Total by default):** **Consistent with** — totality remains separate from effects
- **Principle 8 (Actors, not threads):** **Strengthens** — fine-grained Spawn/Send/Recv tracking

### Specifications

- **002-effect-system:** Requires update — subsumption syntax, remove totality-as-effect, replace Async
- **015-actors:** May need update — effect requirements for actor operations
