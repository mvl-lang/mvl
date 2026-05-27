# ADR-0037: Main-as-Actor — Drop `concurrently` Keyword, Implicit Actor Lifecycle

**Status:** Accepted
**Date:** 2026-05-25
**Issue:** #1048
**Author:** Claude Sonnet 4.6

---

## Context

Phase 8 introduced `concurrently { }` as a structured concurrency scope: actors spawned inside
the block were scoped to it; when the block exited, the runtime drained all pending messages
before returning control to the enclosing scope.

This was a keyword that solved a problem the runtime should handle. Its existence implied that
`fn main()` is a plain function that doesn't participate in the actor system — and therefore
something explicit had to mark "wait here for actors". That premise is wrong.

Additionally, `concurrently { }` blocks couldn't be composed inside helper functions without
giving those functions special semantics, leading to awkward code (see `actor_trading` with
three sequential `concurrently` blocks in main).

---

## Decision

**Remove the `concurrently` keyword from the language entirely.**

`fn main()` is implicitly a one-shot actor. The Rust backend injects `_mvl_join_actors()` as
the implicit return expression of the emitted `fn main()`, ensuring all spawned child actors
drain their mailboxes before the process exits.

This aligns with ADR-0002 (language contraction): remove syntax, expand vocabulary.

---

## Consequences

### Language changes

- `concurrently` is no longer a keyword. Programs using it fail to parse.
- `fn main()` semantics: runtime drains all spawned actors before process exit.
- No new keywords introduced. `match`, `while`, and `self.receive()` cover the remaining
  use cases (addressed in full by the `actor Main { }` form, deferred to a follow-up).

### Implementation

- `TokenKind::Concurrently` removed from lexer.
- `Expr::Concurrently` removed from AST and all 21 match sites across checker, linter, passes,
  and backends.
- `RustEmitter::has_actors` / `inject_actor_join` fields added; `emit_fn_body` injects
  `_mvl_join_actors()` at the end of `fn main()` when actors are present.
- LLVM backend: stub arm removed (it was already sequential fallback only).

### Example migration

Before:
```mvl
fn main() -> Unit ! Console {
    concurrently {
        let w: Worker = actor Worker { id: 1 };
        w.process("task")
    }
    println("done")
}
```

After:
```mvl
fn main() -> Unit ! Console {
    let w: Worker = actor Worker { id: 1 };
    w.process("task")
    // runtime drains w before process exits
}
```

Note: statements after the last actor send (like `println("done")`) now execute *before* actors
complete, since `_mvl_join_actors()` is called at process exit, not mid-function. Programs that
require sequential actor-then-continuation patterns should use the `actor Main { }` form
(ADR-0038, follow-up).

### Deferred

- `actor Main { }` as explicit entry point with `self.receive()` — tracked in #1048 as Step 2.
- Signal integration (`std/signal.mvl`, `Process` effect) — Phase 9 supervision work.

---

## Rejected Alternatives

**Explicit `join()` call on actor references** — `w.join()` at the point where the program needs
to wait. Rejected because it requires the programmer to track every actor reference and call join
in the right order, creating a manual resource-management problem that the runtime can solve.

**`async fn main()`** — adopt an async executor model (like Rust tokio). Rejected: MVL has
an explicit actor model, not a generic async abstraction. Mixing would double the concurrency
concepts and violate ADR-0004 (deliberately smallest language).

**Keep `concurrently` as syntactic sugar** — desugar `concurrently { }` to block-scoped join.
Rejected: syntactic sugar that doesn't compose (can't be called from helper functions without
special semantics) is not sugar, it's a limitation. ADR-0002 says contract, not expand.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

| Requirement | Impact |
|-------------|--------|
| Req 3 (Actor isolation) | **Strengthens** — actor lifetime is always managed by the runtime; no manual scoping required |
| Req 9 (Data race freedom) | **Consistent with** — actor mailbox discipline unchanged; join point moves but race freedom is unaffected |
| Req 11 (IFC) | **Consistent with** — main-as-actor does not change information flow control semantics; actor isolation strengthens composability of `fn main()` by draining actors spawned anywhere, unlike `concurrently {}` which could not be composed inside helpers |

All other requirements are not directly affected by this decision.

### Design Principles (README)

- **Principle 8 (Actors, not threads):** **Strengthens** — `fn main()` is implicitly an actor participant; no programmer-visible join primitive leaks the thread model
- **Principle 1 (Explicit over implicit):** **Tension — explained below** — the join is now implicit. Trade-off is acceptable because explicit join required tracking every actor reference manually; the implicit join is documented in the language spec and visible in the emitted Rust (`_mvl_join_actors()`).
- **Principle 6 (Effects in signatures):** **Consistent with** — actor lifecycle does not require a new effect; actors are already covered by the `Spawn`/`Send`/`Recv` effects from ADR-0035.

### Specifications

- **015-actors:** The actor lifecycle section should be updated to reflect that `fn main()` drains actor mailboxes implicitly via `_mvl_join_actors()`.
- No other specs are directly affected.

---

## References

- ADR-0002: Language contraction
- ADR-0029: Pony reference capability adaptation (actor model)
- Issue #1048: feat(lang): Main-as-actor
- Issue #581: actor pingpong example
