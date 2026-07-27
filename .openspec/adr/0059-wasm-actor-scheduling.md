# ADR-0059: WASM Actor Model — Run-to-Completion Scheduling on WASI Preview 1

**Status:** Accepted
**Date:** 2026-07-27
**Issues:** #2012, epic #1817

---

## Context

`mvl build --backend=wasm` accepted all three files in `tests/corpus/12_actors/`
and emitted assembleable WAT in which every function body was `unreachable`.
Actors looked supported because the build step did not fail. Epic #1817 had
deferred them explicitly ("Actors deferred. Single-threaded WASM has no thread
story yet — that's an ADR of its own"), and the spike left the question open
(`tests/spikes/006-wasm-backend/README.md`, open question #3).

Three constraints shape the answer.

**1. WASM has no threads on this target.** The backend targets
`wasi_snapshot_preview1`, which has no concurrency primitives at all. WASI 0.2's
`wasi:io/poll` is the cooperative-scheduling story, and reaching it means moving
to the component model.

**2. The runtime cannot call back into the emitted module.** This is the binding
constraint, and it is specific to WASM. `wasmtime run --preload runtime=X main.wasm`
instantiates the preload first and resolves *main's* imports against *its*
exports. There is no reverse edge: no shared function table, no host callback
registration, and Rust-generated code in `runtime/wasm` cannot `call_indirect`
into the emitted module's table. The LLVM contract — `_mvl_actor_spawn(dispatch_fn_ptr, …)`
handing the runtime a function pointer it later invokes (ADR-0027,
"Actor runtime interface") — therefore **cannot be ported to WASM at all**. Any
design where the scheduler owns dispatch is impossible here.

**3. The corpus needs message-passing semantics, not parallelism.** All 14 test
fns are single-threaded and deterministic: spawn, send some behaviours, then read
state through a `pub test fn` and assert on it. The observable requirement is
that a read sees every message sent before it (per-actor FIFO). Spec 015 already
states that "scheduling semantics are backend-independent — programs must not
rely on execution order across actors", so cross-actor interleaving is latitude
we are entitled to use.

---

## Decision

### 1. WASI preview 1, run-to-completion. Not WASI 0.2.

Actors on WASM execute run-to-completion on the single available thread. We do
**not** move to the component model or `wasi:io/poll` for this.

This is not a stopgap that a later "real" implementation replaces. Preview 1 with
run-to-completion delivers the full *semantics* MVL's actor model guarantees —
per-actor FIFO, state isolation, message-passing mutation. What it does not
deliver is *parallelism*, and single-threaded WASM cannot deliver parallelism by
any mechanism, including `wasi:io/poll`. Revisiting this becomes worthwhile when
the target gains real concurrency (threads proposal, or a host that schedules
multiple instances), not when the ABI version changes.

### 2. The scheduler lives in emitted WAT, with static dispatch.

Because constraint 2 forbids the runtime from driving dispatch, the mailbox
drain loop is emitted into the module itself.

Dispatch is resolved **at compile time**, not through a function table. The
emitter knows every actor type in the program, so a handle carries a small
integer type tag and the drain loop switches on it to a direct
`call $<actor>_dispatch`. No `(table …)`, no `(elem …)`, no `call_indirect`, and
no new `(type …)` declarations — none of which the emitter currently produces.

Rejected alternative: export the emitted module's table and import it into
`runtime/wasm` so the runtime could `call_indirect`. This adds a cross-module
table contract, forces the emitter to grow table/elem/type emission, and buys
nothing — the set of dispatch targets is statically known, so indirection is
pure overhead.

### 3. Mailbox in linear memory, built from existing runtime primitives.

Actor state is allocated with the existing `_mvl_struct_alloc`, laid out with the
same field-offset rules as structs. The mailbox is a fixed-slot message queue in
linear memory. No new `_mvl_*` runtime symbols are required, so
`runtime/wasm/src/lib.rs` and `RUNTIME_IMPORTS` are untouched by actor support.

### 4. Drain at the outermost send; sync reads are direct calls.

`send` appends to the queue and then, **if no drain is already in progress**,
runs the drain loop to exhaustion. The re-entrancy guard is what makes this
correct rather than merely convenient: a behaviour that sends a message must not
recurse into dispatch, or a self-send would grow the WASM stack until it traps
where a real mailbox would simply queue. With the guard, nested sends enqueue and
are drained by the outermost loop — true FIFO, bounded stack.

Because the queue is therefore always empty between statements, a `pub test fn`
read compiles to a **direct call on the state pointer**. No reply cell, no
blocking. This is a case where the single-threaded target is genuinely simpler
than LLVM, which needs `_mvl_actor_sync_call` and a reply slot (#2012).

The end-of-`main` drain required by spec 015 Requirement 8 falls out for free:
the queue is empty at every statement boundary, so there is nothing left pending
when `main` returns.

### 5. Mailbox bounds

`mailbox(N)` / `unbounded` configuration is accepted and recorded but does not
change behaviour: with drain-at-send the queue depth never exceeds the number of
messages one behaviour chain enqueues. Overflow of the fixed slot region traps
via `unreachable` rather than silently dropping, because a silent drop on a
single-threaded target would be a compiler bug, not backpressure.

---

## Consequences

**Good**

- No new runtime ABI surface; `runtime/wasm` is unchanged.
- No tables or indirect calls — the emitted WAT stays within the constructs the
  emitter already produces, and stays readable.
- Sync reads are cheaper and simpler than on LLVM.
- Translatable to self-hosted MVL later (#1815) with no host-specific machinery.

**Bad**

- No parallelism. Actors on WASM are concurrent in semantics only. A program that
  depends on actors making progress independently (a blocking behaviour, a
  supervisor polling a worker) will deadlock or serialise where the LLVM backend
  would not.
- Cross-actor interleaving differs observably from LLVM's work-stealing
  scheduler. Permitted by spec 015, but it means the WASM backend is not a
  drop-in oracle for actor scheduling behaviour.
- `link`/`monitor`/`on_exit`/`on_down`, `select`, and actor panics-as-exit are out
  of scope here; they need the supervision registry, which this ADR does not
  address.

**Neutral**

- Deviates from ADR-0027's "the emitter calls only the named symbols; swapping
  the target replaces the crate" property for actors specifically. Constraint 2
  makes that property unachievable on WASM, so this ADR records the exception
  rather than pretending the contract holds.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **R9 — Data race freedom:** **Consistent with, trivially satisfied.** A single
  thread cannot race. Sendability of behaviour parameters (`val`/`iso`/value
  types) is still enforced by the checker, unchanged, so a program that
  type-checks for WASM also type-checks for the threaded backends — the
  guarantee does not weaken when the same source is retargeted.
- **R7 — Effect tracking:** **Unchanged.** `Spawn` and `Send` are required at the
  same sites; the backend does not relax effect checking.
- **R2 — Memory safety:** **Unchanged in kind.** Actor state is heap-allocated
  via `_mvl_struct_alloc` and, like all WASM struct allocation today, is never
  freed. Consistent with the existing (documented, intentional) leak in the WASM
  memory model, not a new exception.
- All other requirements: unchanged.

### Design Principles (README)

- **Explicit over implicit:** consistent with — dispatch targets are named
  directly in the emitted WAT; there is no hidden indirection layer.
- **One way to do it:** consistent with — one scheduling model per backend, and
  the surface language is identical.
- **The signature is the threat model:** consistent with — effects and
  sendability remain in the signature and are checked identically.
- **No UFCS (ADR-0031):** consistent with — behaviour calls dispatch on a
  declared actor type, resolved through the actor registry.
- All other principles: consistent with.

### Specifications

- `spec/015-actors`: "Scheduling Model" gains a WASM row — cooperative,
  run-to-completion, drain-at-send, single-threaded. Requirement 8 (Actor
  Lifetime — Main Drain) is satisfied structurally rather than by an explicit
  join call. The corpus-parity note gains `make test-rust-wasm`. Known
  limitations gain an entry recording that WASM provides no actor parallelism.
- `spec/027-backends` (if present) — additive note only.

### Superseded / amended ADRs

- **ADR-0027** (Multi-Backend Architecture): amended, not superseded. Its
  "Actor runtime interface" section describes a dispatch-function-pointer
  contract that the WASM `--preload` linkage model cannot express. WASM
  implements the same *semantics* through an in-module scheduler instead.
