---
domain: language
version: 0.1.0
status: draft
date: 2026-05-14
---

# 015 — Actor Model (Phase 8)

The MVL actor model is the primary concurrency mechanism introduced in Phase 8.  It extends
the data race freedom guarantee proven in Phase 3 (Spec 014) from capability-level checks to
a full architectural proof: no shared mutable state can exist between communicating actors
because message passing is the only communication mechanism and all messages are
capability-safe transfers.

## Philosophy

Actors eliminate shared mutable state by construction, not by convention.  Each actor owns
its private state exclusively; other actors can only send messages to it.  Messages carry
values with sendable capabilities (`iso`, `val`, `tag`) — the compiler enforces this at
call sites, making a data race a compile error rather than a runtime observation.

**Origin:** The Pony actor model (Clebsch et al., 2015), adapted for MVL's simplified
capability set (`iso`, `val`, `ref`, `tag`) as documented in ADR-0029.  Unlike Pony's
runtime scheduler, MVL's Rust backend emits `async fn` / Tokio actors, and the LLVM
backend uses a mailbox runtime.  Semantics are backend-independent.

## Scheduling Model

MVL actors use **cooperative, reduction-budgeted scheduling** — matching Erlang's approach
where reduction counting is transparent to the programmer.

- Actors yield only at **message boundaries** — no OS-level preemption mid-behavior
- A fixed reduction budget prevents starvation: after processing a configurable number of
  messages, the scheduler yields to other actors
- **Phase 8 default:** budget is fixed per actor; per-class configuration is deferred to Phase 9
- **Rust backend:** Tokio task-per-actor with `SyncSender` mailbox
- **LLVM backend:** C-ABI mailbox runtime (`mvl_actor_spawn`/`send`/`drop`)
- Scheduling semantics are **backend-independent** — programs must not rely on execution order
  across actors

No fairness guarantee is made for Phase 8 (see also L3 in Known Limitations).

## Failure Philosophy

MVL uses a **three-layer failure model**:

| Layer | Handles |
|-------|---------|
| Compiler | Type errors, data races, missing effects, contract violations |
| `Result[T, E]` | Expected operational failures (network, DB, external APIs) |
| Actor isolation | Residual unexpected failures — actor terminates; future supervision restarts |

"Let it crash" applies only to what the compiler cannot verify.  Crashing for something
the compiler should have caught is a design failure, not runtime recovery work.

**Mailbox overflow (Phase 8):** When a mailbox is full, `try_send` **drops the message
silently**.  This is fire-and-forget semantics — callers MUST NOT rely on message delivery
under load.  Mailbox capacity is fixed at **256 messages** per actor in both backends.
Configurable capacity and backpressure are deferred to Phase 9.

**Actor failure handling (Phase 8):** If a behavior panics, the actor thread terminates.
No automatic restart occurs.  Supervision tree support via `std.actors.Supervisor` is
planned for Phase 9 with `one_for_one`, `one_for_all`, and `rest_for_one` strategies.

## Supervision (Phase 9 Preview)

No dedicated language construct exists for supervision in Phase 8.  The design decision
(issue #854) is:

- `std.actors.Supervisor` will provide standard restart strategies in Phase 9
- Erlang-style **bidirectional links** are the Phase 8 primitive — pass `tag ActorRef`
  references between actors to build callback / notification patterns
- One-way monitors (observe failure without coupling fate) are a Phase 9 addition

## Capability Sendability Recap

| Capability | Sendable across actor boundary |
|------------|-------------------------------|
| `iso`      | Yes — ownership transferred via `consume()` |
| `val`      | Yes — deeply immutable, freely shared |
| `tag`      | Yes — opaque identity only, no data access |
| `ref`      | No — mutable, confined to local scope |

The full capability model is specified in Spec 014 and ADR-0029.  This spec covers actor
lifecycle and message-passing semantics; it does not duplicate the capability table.

## Syntax Overview

```mvl
// Actor type declaration
actor Counter {
    count: Int

    // Private helper — sync, internal only
    fn validate(delta: Int) -> Bool {
        delta >= 0
    }

    // Behavior (async message handler) — pub fn inside actor
    pub fn increment(iso delta: Int) {
        if self.validate(delta) {
            self.count = self.count + delta
        }
    }

    pub fn reset() {
        self.count = 0
    }

    pub fn get_count(tag reply: ActorRef) {
        reply.receive(self.count)
    }
}

// Create an actor, receive an ActorRef (tag capability)
let tag counter: ActorRef = actor Counter { count: 0 }

// Send a message (behavior call)
counter.increment(consume(delta))

// fn main() is implicitly an actor — spawned actors are drained at exit
fn main() -> Unit {
    let tag a: ActorRef = actor Worker {}
    a.run()
}   // main() returns after all spawned actors drain (ADR-0037)
```

**Syntax design decisions:**
- `pub fn` inside actor = behavior (async message handler). No `be` keyword needed.
- `fn` inside actor = private helper (sync, internal only).
- `actor Type { ... }` for creation (not `spawn` — avoids conflict with `ProcessSpawn` effect).
- Behaviors implicitly return `Unit` — no return type annotation required.

## Requirements

### Requirement 1: Actor Declaration Syntax [MUST]

The compiler MUST parse `actor TypeName { fields* functions* }` as a top-level declaration.
An actor type consists of:
- Named fields with types (the actor's private mutable state)
- Zero or more functions: `pub fn` = behavior (async), `fn` = private helper (sync)

The `actor` keyword is a hard-reserved keyword and is used for both declaration and instantiation.

**Implementation:** `src/mvl/parser/ast.rs::Decl::Actor`,
`src/mvl/parser/functions.rs::parse_actor_decl`

**Tests:** `tests/corpus/12_actors/basic_actor.mvl`,
`tests/corpus/negative/req09_data_race/actor_syntax_errors.mvl`

**Corpus:** `tests/corpus/09_concurrency/actors.mvl`, `examples/programs/actor_spawn.mvl`

#### Scenario: Actor type parses correctly

- GIVEN an `actor` declaration with fields and behaviors
- WHEN the parser processes the source
- THEN the AST MUST contain a `Decl::Actor` node with the correct field list and behavior list

**Tests:** `tests/corpus/12_actors/basic_actor.mvl`

#### Scenario: Actor keyword is reserved

- GIVEN source that uses `actor` as a variable name: `let actor = 1`
- WHEN the parser processes the source
- THEN the compiler MUST emit a parse error: "`actor` is a reserved keyword"

**Tests:** `tests/corpus/negative/req09_data_race/actor_syntax_errors.mvl`

---

### Requirement 2: Behavior Semantics [MUST]

A behavior (`pub fn` inside an actor) is an asynchronous message handler.  The compiler MUST enforce:

1. Behaviors implicitly return `Unit` — no return type annotation, no synchronous return value
2. All parameters of a behavior MUST have sendable capabilities (`iso`, `val`, or `tag`)
3. Behaviors have access to `self` for reading and writing the actor's private fields
4. Behaviors MUST NOT call other behaviors synchronously — they enqueue messages
5. Private helpers (`fn` without `pub`) are synchronous and may return any type

**Implementation:** `src/mvl/checker.rs::TypeChecker::check_behavior`,
`src/mvl/backends/rust/emit_functions.rs::emit_behavior`,
`src/mvl/backends/llvm.rs::emit_behavior`

**Tests:** `tests/corpus/12_actors/behaviors.mvl`,
`tests/corpus/negative/req09_data_race/behavior_ref_param.mvl`

**Corpus:** `tests/corpus/09_concurrency/actors.mvl`, `examples/programs/actor_send.mvl`

#### Scenario: Behavior with ref parameter rejected

- GIVEN `pub fn update(ref data: Buffer) { self.buf = data }`
- WHEN the checker processes the behavior
- THEN the compiler MUST emit `CheckError::CapabilityViolation`:
  "`ref` capability of `data` cannot be used in a behavior parameter"

**Tests:** `tests/corpus/negative/req09_data_race/behavior_ref_param.mvl`

#### Scenario: Behavior with iso parameter accepted

- GIVEN `pub fn enqueue(iso msg: Message) { self.queue.push(consume(msg)) }`
- WHEN the checker processes the behavior
- THEN the compiler MUST accept the declaration

**Tests:** `tests/corpus/12_actors/behaviors.mvl`

#### Scenario: Behavior with return type rejected

- GIVEN `pub fn get() -> Int { self.count }`
- WHEN the checker processes the behavior
- THEN the compiler MUST emit a type error: "behavior cannot have explicit return type"

**Tests:** `tests/corpus/negative/req09_data_race/behavior_return_type.mvl`

#### Scenario: Private helper with return type accepted

- GIVEN `fn validate(x: Int) -> Bool { x >= 0 }` (no `pub`)
- WHEN the checker processes the function
- THEN the compiler MUST accept — private helpers are sync and may return values

**Tests:** `tests/corpus/12_actors/behaviors.mvl`

---

### Requirement 3: Actor Creation and Lifecycle [MUST]

The compiler MUST support `actor ActorType { field: value, ... }` as an expression that:

1. Allocates and initialises the actor's private state
2. Starts the actor's message-processing loop
3. Returns an `ActorRef` with `tag` capability (identity only — no field access)

The `actor` keyword is reused for both declaration and instantiation (no separate `spawn` keyword —
this avoids conflict with `ProcessSpawn` effect for OS process creation).

An actor terminates when:
- All `ActorRef` handles referring to it are dropped, AND
- Its message queue is empty

**Implementation:** `src/mvl/parser/expressions.rs::parse_actor_expr`,
`src/mvl/checker.rs::TypeChecker::check_actor_creation`,
`src/mvl/backends/rust/emit_exprs.rs::emit_actor_creation`,
`src/mvl/backends/llvm/exprs.rs::emit_actor_creation`

**Tests:** `tests/corpus/12_actors/lifecycle.mvl`,
`tests/corpus/negative/req09_data_race/actor_field_mismatch.mvl`

**Corpus:** `examples/programs/actor_spawn.mvl`, `examples/programs/actor_send.mvl`

#### Scenario: Actor creation returns ActorRef with tag capability

- GIVEN `let tag counter: ActorRef = actor Counter { count: 0 }`
- WHEN the checker processes the actor expression
- THEN the type of `counter` MUST be `ActorRef` with `tag` capability

**Tests:** `tests/corpus/12_actors/lifecycle.mvl`

#### Scenario: Actor creation with wrong field types rejected

- GIVEN `actor Counter { count: "hello" }` where `count: Int`
- WHEN the checker processes the actor expression
- THEN the compiler MUST emit a type error

**Tests:** `tests/corpus/negative/req09_data_race/actor_field_mismatch.mvl`

---

### Requirement 4: Message Send — Ownership Transfer via iso [MUST]

Sending a message to an actor is a behavior call on an `ActorRef`: `actor_ref.behavior(args)`.

For `iso` arguments, the call MUST consume the sender's binding via `consume()`.
The compiler MUST reject a behavior call that passes an `iso` value without consuming it
(this would alias the isolated reference across the actor boundary).

**Implementation:** `src/mvl/checker/capabilities.rs::check_send_capability`,
`src/mvl/checker.rs::TypeChecker::check_behavior_call`

**Tests:** `tests/corpus/12_actors/message_send.mvl`,
`tests/corpus/negative/req09_data_race/iso_send_without_consume.mvl`

**Corpus:** `tests/corpus/09_concurrency/actors.mvl`, `examples/programs/actor_send.mvl`

#### Scenario: iso message send requires consume

- GIVEN `iso packet: Packet = make_packet()` and `worker.process(packet)` without consume
- WHEN the checker processes the behavior call
- THEN the compiler MUST emit `CheckError::IsoAliasingViolation`:
  "`iso` value `packet` must be transferred with `consume()` across actor boundary"

**Tests:** `tests/corpus/negative/req09_data_race/iso_send_without_consume.mvl`

#### Scenario: Correct iso message send accepted

- GIVEN `worker.process(consume(packet))`
- WHEN the checker processes the behavior call
- THEN the compiler MUST accept — ownership transfers to the receiving actor

**Tests:** `tests/corpus/12_actors/message_send.mvl`

#### Scenario: val message send requires no consume

- GIVEN `val config: Config = load_config()` and `worker.configure(config)`
- WHEN the checker processes the behavior call
- THEN the compiler MUST accept — `val` is freely shareable, no consume needed

**Tests:** `tests/corpus/12_actors/message_send.mvl`

#### Scenario: Mailbox full — message is dropped silently

- GIVEN an actor with a full mailbox (256 pending messages)
- WHEN a behavior call is made via `try_send`
- THEN the message IS dropped — no error is raised, no blocking occurs

**Tests:** `tests/corpus/12_actors/mailbox_overflow.mvl`

---

### Requirement 5: Sendability Rules [MUST]

The compiler MUST enforce at every behavior call site:

- `iso` parameters: argument MUST be transferred via `consume()`
- `val` parameters: argument MAY be passed directly or via consume (both accepted)
- `tag` parameters: argument MUST have `tag` capability (identity reference only)
- `ref` parameters: FORBIDDEN in behaviors — compiler MUST emit `CheckError::CapabilityViolation`

These rules extend the channel-send rules from Spec 014 (Req 1) to actor behavior calls.

**Implementation:** `src/mvl/checker/capabilities.rs::check_send_capability`,
`src/mvl/checker.rs::TypeChecker::check_behavior_call`

**Tests:** `tests/type_checker.rs::sending_ref_param_rejected`,
`tests/corpus/negative/req09_data_race/behavior_ref_param.mvl`

**Corpus:** `tests/negative/req09/send_ref_across_actor.mvl`

#### Scenario: ref capability rejected at behavior call

- GIVEN `ref buf: Buffer` and `actor_ref.write(buf)`
- WHEN the checker processes the behavior call
- THEN the compiler MUST emit `CheckError::CapabilityViolation`:
  "`ref` capability cannot cross actor boundary"

**Tests:** `tests/corpus/negative/req09_data_race/behavior_ref_param.mvl`

---

### Requirement 6: Actor Isolation — No Shared Mutable State [MUST]

An actor's fields MUST NOT be readable or writable from outside the actor.  The compiler
MUST reject any expression that attempts to read or write an actor field through an `ActorRef`.

The only permitted interaction with an actor from the outside is sending a message
(behavior call).  Field access on an `ActorRef` MUST produce a compile error.

**Implementation:** `src/mvl/checker.rs::TypeChecker::check_field_access`

**Tests:** `tests/corpus/negative/req09_data_race/actor_field_access.mvl`

**Corpus:** `tests/negative/req09/ref_escapes_to_actor.mvl`

#### Scenario: Direct field access on ActorRef rejected

- GIVEN `let tag a: ActorRef = actor Counter { count: 0 }` and `let x = a.count`
- WHEN the checker processes the field access
- THEN the compiler MUST emit a type error:
  "actor field `count` is not accessible through `ActorRef` — send a message instead"

**Tests:** `tests/corpus/negative/req09_data_race/actor_field_access.mvl`

#### Scenario: Behaviors read and write own fields freely

- GIVEN `pub fn reset() { self.count = 0 }`
- WHEN the checker processes the behavior
- THEN `self.count` field access MUST be accepted (within actor, no isolation violation)

**Tests:** `tests/corpus/12_actors/basic_actor.mvl`

---

### Requirement 7: ActorRef Semantics — tag Capability [MUST]

`actor Type { ... }` returns an `ActorRef` with `tag` capability.  An `ActorRef`:

- Carries the actor's identity (used to send messages)
- Does NOT provide read or write access to any actor field
- MAY be shared freely (tag is sendable)
- MAY be compared for identity (`==` on two `ActorRef` values checks same actor)
- MUST NOT be used as an `iso` or `ref` value

**Implementation:** `src/mvl/checker.rs::TypeChecker::check_actor_creation`,
`src/mvl/parser/ast.rs::Type::ActorRef`

**Tests:** `tests/corpus/12_actors/actor_ref.mvl`

**Corpus:** `tests/corpus/09_concurrency/actor_ref.mvl`

#### Scenario: ActorRef is tag-sendable

- GIVEN `tag counter: ActorRef = actor Counter { count: 0 }` and `worker.set_target(counter)`
  where `set_target` expects `tag target: ActorRef`
- WHEN the checker processes the call
- THEN the compiler MUST accept — `tag` is sendable

**Tests:** `tests/corpus/12_actors/actor_ref.mvl`

#### Scenario: ActorRef identity comparison accepted

- GIVEN two `ActorRef` values `a` and `b`
- WHEN `a == b` is evaluated
- THEN the result MUST be `Bool` — true iff both refer to the same actor

**Tests:** `tests/corpus/12_actors/actor_ref.mvl`

#### Scenario: Reply pattern — passing ActorRef for callbacks

- GIVEN a `Requester` actor that calls `worker.compute(consume(data), self_ref)`
  where `self_ref: tag ActorRef` is a reference to the requester
- WHEN the worker completes processing
- THEN the worker MAY call `self_ref.on_result(consume(result))` to deliver the reply
- AND the compiler MUST accept — `tag ActorRef` is sendable and carries only identity

**Tests:** `tests/corpus/12_actors/actor_ref.mvl`, `examples/programs/actor_pingpong.mvl`

---

### Requirement 8: Actor Lifetime — Main Drain [SHOULD]

All actors spawned within `fn main()` are drained (joined) before the process exits.
The runtime MUST process all pending messages in each actor's mailbox and wait for
each actor thread to terminate before `main()` returns.

This ensures that concurrent work is bounded by the program's execution lifetime and
that the process does not exit with pending messages or incomplete message handlers.

**Implementation:** `src/mvl/backends/rust/emit_functions.rs::emit_fn_body` — emits `_mvl_join_actors()`
at the end of main. ADR-0037 documents the design: `fn main()` is implicitly an actor with an
implicit join. `src/mvl/backends/llvm_text/emitter.rs` uses equivalent `mvl_actor_join_all`.

**Tests:** `tests/corpus/12_actors/actor_spawn.mvl`, `tests/corpus/12_actors/actor_send.mvl`,
`tests/stdlib/net_basic.mvl`

#### Scenario: Spawned actors are drained before main exits

- GIVEN a `fn main()` that spawns one or more actors and sends them messages
- WHEN `main()` returns
- THEN the runtime MUST process all pending messages in each actor's mailbox
  before the process exits
- AND this drain is **guaranteed**, not best-effort

**Tests:** `tests/corpus/12_actors/actor_spawn.mvl` (minimal spawn), `examples/programs/actor_spawn.mvl`

**Note:** If an actor panics, the actor thread terminates and remaining messages are
dropped. Supervision and restart are tracked for Phase 9 via `std.actors.Supervisor`.

---

### Requirement 9: Select on Behaviors with Timeout [SHOULD]

MVL SHOULD support a `select` expression that waits for the first of several pending
actor messages or a timeout to fire:

```mvl
select {
    result = worker.get_result() => { process(result) }
    timeout(Duration::ms(100)) => { handle_timeout() }
}
```

The `select` expression:
- Evaluates to `Unit`
- Fires the first branch whose condition becomes ready
- MUST use `timeout` as an explicit branch, not an implicit behaviour

**Implementation:** `src/mvl/parser/expressions.rs::parse_select_expr`,
`src/mvl/checker.rs::TypeChecker::check_select`,
`src/mvl/backends/rust/emit_exprs.rs::emit_select`,
`src/mvl/backends/llvm/exprs.rs::emit_select`

**Tests:** `tests/corpus/12_actors/select.mvl`,
`tests/corpus/negative/req09_data_race/select_no_timeout.mvl`

**Corpus:** `tests/corpus/09_concurrency/select.mvl`

#### Scenario: Select fires first ready branch

- GIVEN a `select` with two branches pointing to two actors
- WHEN one actor delivers a message before the other
- THEN the first branch's handler MUST execute; the other branch is discarded

**Tests:** `tests/corpus/12_actors/select.mvl`

#### Scenario: Select with timeout fires when no branch ready

- GIVEN a `select` with a single actor branch and `timeout(Duration::ms(50))`
- WHEN the actor does not deliver a message within 50 ms
- THEN the timeout branch MUST execute

**Tests:** `tests/corpus/12_actors/select.mvl`

---

## Cross-Backend Parity

Actor semantics MUST be identical across the Rust and LLVM backends (#698):

- Actor isolation (no shared state) is a checker guarantee — both backends inherit it
- Message ordering within a single actor MUST be FIFO
- Creation/terminate lifecycle MUST behave identically
- `select` timeout precision is backend-defined but MUST be best-effort

**Tests:** `tests/corpus/12_actors/` — all files in this directory are run against both backends
as part of cross-backend parity validation.
`tests/cross_backend.rs::cross_backend_actor_corpus_actors`,
`tests/cross_backend.rs::cross_backend_actor_corpus_capabilities`,
`tests/cross_backend.rs::cross_backend_actor_corpus_session_types`,
`tests/cross_backend.rs::cross_backend_actor_corpus_supervisor`,
`tests/cross_backend.rs::cross_backend_actor_corpus_dead_letter`,
`tests/cross_backend.rs::cross_backend_actor_corpus_process_links`,
`tests/cross_backend.rs::cross_backend_actor_corpus_select`

## Known Limitations (Phase 8)

These limitations are accepted for Phase 8 and tracked as follow-up work:

- **L1**: Post-consume ownership tracking — the checker does not detect re-aliasing of the
  value received from `consume()` within the same function. Tracked as Phase 9 work.
- **L2**: Actor field type restrictions — complex generic field types in actor declarations
  may require additional type-checking passes (deferred).
- **L3**: select fairness — when multiple branches are simultaneously ready, branch selection
  is implementation-defined (Rust/LLVM schedulers differ). No fairness guarantee is made.
- **L4**: failure handling — if a behavior panics, the actor thread terminates with no
  automatic restart. No panic recovery or supervision tree exists in Phase 8.  Supervision
  via `std.actors.Supervisor` is a Phase 9 feature.
- **L5**: one-way monitors — Phase 8 supports only bidirectional links (passing `tag ActorRef`
  references for reply/notification patterns). Erlang-style one-way monitors (observe failure
  without coupling fate) are deferred to Phase 9.
- **L6**: mailbox capacity — fixed at 256 messages per actor; overflow silently drops
  messages. Configurable capacity, blocking send, and backpressure are Phase 9 features.
- **L7**: graceful shutdown ordering — main drain is guaranteed on normal exit but not on
  actor panic or external stop. Ordered shutdown via `Supervisor.stop()` is deferred to Phase 9.
