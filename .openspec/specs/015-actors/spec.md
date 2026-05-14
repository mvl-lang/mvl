---
domain: language
version: 0.1.0
status: draft
date: 2026-05-14
---

# 015 — Actor Model (Phase 8)

The MVL actor model is the primary concurrency mechanism introduced in Phase 8.  It extends
the data race freedom guarantee proven in Phase 3 (Spec 014) from capability-level checks to
a full architectural proof: no shared mutable state can exist between concurrently executing
actors because message passing is the only communication mechanism and all messages are
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

    // Behavior (async message handler)
    be increment(iso delta: Int) -> Unit {
        self.count = self.count + delta
    }

    be reset() -> Unit {
        self.count = 0
    }

    be get_count(tag reply: ActorRef) -> Unit {
        reply.receive(self.count)
    }
}

// Spawn an actor, receive an ActorRef (tag capability)
let tag counter: ActorRef = spawn Counter { count: 0 }

// Send a message (behavior call)
counter.increment(consume(delta))

// Structured concurrency — scope lifetime
concurrently {
    let tag a: ActorRef = spawn Worker {}
    a.run()
}   // a is dropped here; runtime waits for pending messages to drain
```

## Requirements

### Requirement 1: Actor Declaration Syntax [MUST]

The compiler MUST parse `actor TypeName { fields* behaviors* }` as a top-level declaration.
An actor type consists of:
- Named fields with types (the actor's private mutable state)
- Zero or more behaviors declared with the `be` keyword

The `actor` keyword is a hard-reserved keyword.

**Implementation:** `src/mvl/parser/ast.rs::Decl::Actor`,
`src/mvl/parser/functions.rs::parse_actor_decl`

**Tests:** `tests/corpus/actors/basic_actor.mvl`,
`tests/corpus/negative/req09_data_race/actor_syntax_errors.mvl`

#### Scenario: Actor type parses correctly

- GIVEN an `actor` declaration with fields and behaviors
- WHEN the parser processes the source
- THEN the AST MUST contain a `Decl::Actor` node with the correct field list and behavior list

**Tests:** `tests/corpus/actors/basic_actor.mvl`

#### Scenario: Actor keyword is reserved

- GIVEN source that uses `actor` as a variable name: `let actor = 1`
- WHEN the parser processes the source
- THEN the compiler MUST emit a parse error: "`actor` is a reserved keyword"

**Tests:** `tests/corpus/negative/req09_data_race/actor_syntax_errors.mvl`

---

### Requirement 2: Behavior Semantics [MUST]

A behavior (`be`) is an asynchronous message handler.  The compiler MUST enforce:

1. Behaviors return `Unit` — no synchronous return value (message passing is one-way)
2. All parameters of a behavior MUST have sendable capabilities (`iso`, `val`, or `tag`)
3. Behaviors have access to `self` for reading and writing the actor's private fields
4. Behaviors MUST NOT call other behaviors synchronously — they enqueue messages

**Implementation:** `src/mvl/checker/mod.rs::TypeChecker::check_behavior`,
`src/mvl/backends/rust/emit_functions.rs::emit_behavior`,
`src/mvl/backends/llvm/mod.rs::emit_behavior`

**Tests:** `tests/corpus/actors/behaviors.mvl`,
`tests/corpus/negative/req09_data_race/behavior_ref_param.mvl`

#### Scenario: Behavior with ref parameter rejected

- GIVEN `be update(ref data: Buffer) -> Unit { self.buf = data }`
- WHEN the checker processes the behavior
- THEN the compiler MUST emit `CheckError::CapabilityViolation`:
  "`ref` capability of `data` cannot be used in a behavior parameter"

**Tests:** `tests/corpus/negative/req09_data_race/behavior_ref_param.mvl`

#### Scenario: Behavior with iso parameter accepted

- GIVEN `be enqueue(iso msg: Message) -> Unit { self.queue.push(consume(msg)) }`
- WHEN the checker processes the behavior
- THEN the compiler MUST accept the declaration

**Tests:** `tests/corpus/actors/behaviors.mvl`

#### Scenario: Behavior with non-Unit return rejected

- GIVEN `be get() -> Int { self.count }`
- WHEN the checker processes the behavior
- THEN the compiler MUST emit a type error: "behavior return type must be `Unit`"

**Tests:** `tests/corpus/negative/req09_data_race/behavior_return_type.mvl`

---

### Requirement 3: Actor Spawn and Lifecycle [MUST]

The compiler MUST support `spawn ActorType { field: value, ... }` as an expression that:

1. Allocates and initialises the actor's private state
2. Starts the actor's message-processing loop
3. Returns an `ActorRef` with `tag` capability (identity only — no field access)

An actor terminates when:
- All `ActorRef` handles referring to it are dropped, AND
- Its message queue is empty

**Implementation:** `src/mvl/parser/expressions.rs::parse_spawn_expr`,
`src/mvl/checker/mod.rs::TypeChecker::check_spawn`,
`src/mvl/backends/rust/emit_exprs.rs::emit_spawn`,
`src/mvl/backends/llvm/exprs.rs::emit_spawn`

**Tests:** `tests/corpus/actors/lifecycle.mvl`,
`tests/corpus/negative/req09_data_race/spawn_field_mismatch.mvl`

#### Scenario: Spawn returns ActorRef with tag capability

- GIVEN `let tag counter: ActorRef = spawn Counter { count: 0 }`
- WHEN the checker processes the spawn expression
- THEN the type of `counter` MUST be `ActorRef` with `tag` capability

**Tests:** `tests/corpus/actors/lifecycle.mvl`

#### Scenario: Spawn with wrong field types rejected

- GIVEN `spawn Counter { count: "hello" }` where `count: Int`
- WHEN the checker processes the spawn expression
- THEN the compiler MUST emit a type error

**Tests:** `tests/corpus/negative/req09_data_race/spawn_field_mismatch.mvl`

---

### Requirement 4: Message Send — Ownership Transfer via iso [MUST]

Sending a message to an actor is a behavior call on an `ActorRef`: `actor_ref.behavior(args)`.

For `iso` arguments, the call MUST consume the sender's binding via `consume()`.
The compiler MUST reject a behavior call that passes an `iso` value without consuming it
(this would alias the isolated reference across the actor boundary).

**Implementation:** `src/mvl/checker/capabilities.rs::check_send_capability`,
`src/mvl/checker/mod.rs::TypeChecker::check_behavior_call`

**Tests:** `tests/corpus/actors/message_send.mvl`,
`tests/corpus/negative/req09_data_race/iso_send_without_consume.mvl`

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

**Tests:** `tests/corpus/actors/message_send.mvl`

#### Scenario: val message send requires no consume

- GIVEN `val config: Config = load_config()` and `worker.configure(config)`
- WHEN the checker processes the behavior call
- THEN the compiler MUST accept — `val` is freely shareable, no consume needed

**Tests:** `tests/corpus/actors/message_send.mvl`

---

### Requirement 5: Sendability Rules [MUST]

The compiler MUST enforce at every behavior call site:

- `iso` parameters: argument MUST be transferred via `consume()`
- `val` parameters: argument MAY be passed directly or via consume (both accepted)
- `tag` parameters: argument MUST have `tag` capability (identity reference only)
- `ref` parameters: FORBIDDEN in behaviors — compiler MUST emit `CheckError::CapabilityViolation`

These rules extend the channel-send rules from Spec 014 (Req 1) to actor behavior calls.

**Implementation:** `src/mvl/checker/capabilities.rs::check_send_capability`,
`src/mvl/checker/mod.rs::TypeChecker::check_behavior_call`

**Tests:** `tests/type_checker.rs::sending_ref_param_rejected`,
`tests/corpus/negative/req09_data_race/behavior_ref_param.mvl`

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

**Implementation:** `src/mvl/checker/mod.rs::TypeChecker::check_field_access`

**Tests:** `tests/corpus/negative/req09_data_race/actor_field_access.mvl`

#### Scenario: Direct field access on ActorRef rejected

- GIVEN `let tag a: ActorRef = spawn Counter { count: 0 }` and `let x = a.count`
- WHEN the checker processes the field access
- THEN the compiler MUST emit a type error:
  "actor field `count` is not accessible through `ActorRef` — send a message instead"

**Tests:** `tests/corpus/negative/req09_data_race/actor_field_access.mvl`

#### Scenario: Behaviors read and write own fields freely

- GIVEN `be reset() -> Unit { self.count = 0 }`
- WHEN the checker processes the behavior
- THEN `self.count` field access MUST be accepted (within actor, no isolation violation)

**Tests:** `tests/corpus/actors/basic_actor.mvl`

---

### Requirement 7: ActorRef Semantics — tag Capability [MUST]

`spawn` returns an `ActorRef` with `tag` capability.  An `ActorRef`:

- Carries the actor's identity (used to send messages)
- Does NOT provide read or write access to any actor field
- MAY be shared freely (tag is sendable)
- MAY be compared for identity (`==` on two `ActorRef` values checks same actor)
- MUST NOT be used as an `iso` or `ref` value

**Implementation:** `src/mvl/checker/mod.rs::TypeChecker::check_spawn`,
`src/mvl/parser/ast.rs::Type::ActorRef`

**Tests:** `tests/corpus/actors/actor_ref.mvl`

#### Scenario: ActorRef is tag-sendable

- GIVEN `tag counter: ActorRef = spawn Counter { count: 0 }` and `worker.set_target(counter)`
  where `set_target` expects `tag target: ActorRef`
- WHEN the checker processes the call
- THEN the compiler MUST accept — `tag` is sendable

**Tests:** `tests/corpus/actors/actor_ref.mvl`

#### Scenario: ActorRef identity comparison accepted

- GIVEN two `ActorRef` values `a` and `b`
- WHEN `a == b` is evaluated
- THEN the result MUST be `Bool` — true iff both refer to the same actor

**Tests:** `tests/corpus/actors/actor_ref.mvl`

---

### Requirement 8: Structured Concurrency — Scope Lifetime [SHOULD]

Actors spawned inside a `concurrently` block MUST NOT outlive that block's scope.
When the `concurrently` block exits, the runtime MUST drain all pending messages for
actors spawned within the block before returning control to the enclosing scope.

This prevents dangling actor references and ensures that concurrent work is bounded
by the scope in which it was created.

**Implementation:** `src/mvl/parser/expressions.rs::parse_concurrently_expr`,
`src/mvl/checker/mod.rs::TypeChecker::check_concurrently`,
`src/mvl/backends/rust/emit_exprs.rs::emit_concurrently`,
`src/mvl/backends/llvm/exprs.rs::emit_concurrently`

**Tests:** `tests/corpus/actors/structured_concurrency.mvl`,
`tests/corpus/negative/req09_data_race/actor_escape_scope.mvl`

#### Scenario: Actor does not escape concurrently block

- GIVEN a `concurrently { let tag a = spawn Worker {}; a.run() }` block
- WHEN the outer scope attempts to use `a` after the block
- THEN the compiler MUST emit an error: "actor `a` does not outlive its `concurrently` block"

**Tests:** `tests/corpus/negative/req09_data_race/actor_escape_scope.mvl`

#### Scenario: Concurrently block drains before returning

- GIVEN a `concurrently` block containing `spawn Worker {}` that processes messages
- WHEN the block exits normally
- THEN all pending messages for actors spawned within the block MUST be processed
  before the enclosing scope continues

**Tests:** `tests/corpus/actors/structured_concurrency.mvl`

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
`src/mvl/checker/mod.rs::TypeChecker::check_select`,
`src/mvl/backends/rust/emit_exprs.rs::emit_select`,
`src/mvl/backends/llvm/exprs.rs::emit_select`

**Tests:** `tests/corpus/actors/select.mvl`,
`tests/corpus/negative/req09_data_race/select_no_timeout.mvl`

#### Scenario: Select fires first ready branch

- GIVEN a `select` with two branches pointing to two actors
- WHEN one actor delivers a message before the other
- THEN the first branch's handler MUST execute; the other branch is discarded

**Tests:** `tests/corpus/actors/select.mvl`

#### Scenario: Select with timeout fires when no branch ready

- GIVEN a `select` with a single actor branch and `timeout(Duration::ms(50))`
- WHEN the actor does not deliver a message within 50 ms
- THEN the timeout branch MUST execute

**Tests:** `tests/corpus/actors/select.mvl`

---

## Cross-Backend Parity

Actor semantics MUST be identical across the Rust and LLVM backends (#698):

- Actor isolation (no shared state) is a checker guarantee — both backends inherit it
- Message ordering within a single actor MUST be FIFO
- Spawn/terminate lifecycle MUST behave identically
- `select` timeout precision is backend-defined but MUST be best-effort

**Tests:** `tests/corpus/actors/` — all files in this directory are run against both backends
as part of cross-backend parity validation.

## Known Limitations (Phase 8)

These limitations are accepted for Phase 8 and tracked as follow-up work:

- **L1**: Post-consume ownership tracking — the checker does not detect re-aliasing of the
  value received from `consume()` within the same function. Tracked as Phase 9 work.
- **L2**: Actor field type restrictions — complex generic field types in actor declarations
  may require additional type-checking passes (deferred).
- **L3**: select fairness — when multiple branches are simultaneously ready, branch selection
  is implementation-defined (Rust/LLVM schedulers differ). No fairness guarantee is made.
