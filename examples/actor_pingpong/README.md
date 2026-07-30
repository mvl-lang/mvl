# actor_pingpong

Two actors exchanging messages — the simplest possible actor communication pattern.

---

## Files

| File | Role |
|------|------|
| `pingpong.mvl` | `Ping`/`Pong` actors and message types — the module under test |
| `main.mvl` | CLI entry point: parses `--rounds`, spawns both actors, kicks off the exchange |
| `pingpong_test.mvl` | Deterministic behavior tests (`use pingpong.{...}`, no redeclaration) |

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Actor definition | `actor Pong { received: Int ... }` | Private mutable state + behaviors |
| Behavior | `pub fn ping(val msg: PingMsg, tag sender: Ping)` | Async message handler |
| Private helper | `fn log(seq: Int) -> Unit ! Console` | Synchronous, internal only |
| Actor creation | `let pong: Pong = actor Pong { received: 0 }` | Spawn actor, get a handle |
| `val` capability | `val msg: PingMsg` | Immutable message — shareable, no ownership transfer |
| `tag` capability | `tag sender: Ping` | Identity-only reference — sendable, no read/write |
| Message send | `sender.pong(PongMsg { seq: msg.seq })` | Fire-and-forget async dispatch |
| `pub test fn` | `pub test fn get_received() -> Int { self.received }` | Synchronous state accessor, usable directly from test fns |
| Implicit lifecycle | — | `fn main()` drains all spawned actors' mailboxes before the process exits (#1048) — no explicit `concurrently` keyword needed |

---

## How it works

```
main()
  spawn Pong                          actor Pong { received: 0 }
  spawn Ping (partner = pong)         actor Ping { rounds: N, sent: 0, partner: pong }
  ping.start()
  ↓ runtime blocks until all mailboxes drain before main() returns
```

Message flow for 5 rounds:

```
Ping.start()
  → Pong.ping(seq=0, sender=ping)
      ← Ping.pong(seq=0)           logs "Ping --> pong #0"
          → Pong.ping(seq=1, ...)
              ← Ping.pong(seq=1)   logs "Ping --> pong #1"
                  ...
                      ← Ping.pong(seq=4)   logs "Ping --> pong #4"  — stops (sent == rounds)
```

Expected output (`--rounds=5`, the default):

```
Pong  <-- ping #0
Ping  --> pong #0
Pong  <-- ping #1
Ping  --> pong #1
Pong  <-- ping #2
Ping  --> pong #2
Pong  <-- ping #3
Ping  --> pong #3
Pong  <-- ping #4
Ping  --> pong #4
```

---

## Capability rules (why val and tag?)

| Capability | Read | Write | Sendable | Used for |
|------------|------|-------|----------|----------|
| `val` | yes | no | yes | Immutable messages (`PingMsg`, `PongMsg`) |
| `tag` | no | no | yes | Actor identity / reply address (`Ping`/`Pong` handles) |
| `ref` | yes | yes | **no** | Local mutable state (actor fields, local vars) |
| `iso` | yes | yes | yes | Owned heap values transferred across boundaries |

`ref` values cannot cross actor boundaries — the compiler rejects attempts to send them.

## Why the actors live in a sibling module, not main.mvl

`main.mvl` is the CLI entry point, not a module — `pingpong_test.mvl` needs to
`use pingpong.{Ping, Pong, PingMsg, PongMsg}` rather than redeclaring the
actors under test (see `.openspec/patterns/006-no-test-shadows.md`).

## Testing an async round-trip deterministically

`pingpong_test.mvl` does **not** test the full multi-round `ping.start()`
cascade end-to-end: once started, the round-trip replies are enqueued by
Pong's own thread/turn, not the test thread, so there's no way to observe
"all N rounds have happened" without a wait/drain primitive — that's only
exposed to `fn main` (via `mvl_join_actors()`, #1048), not to test fns.

Instead, each test drives one actor's behavior directly from the test
thread and asserts on that **same** actor's own state via a `pub test fn`
accessor. Both calls are FIFO-ordered within that actor's own mailbox, so
the result is deterministic regardless of scheduling — as long as `rounds`
is chosen so any further reply the exchange might trigger on its own is a
no-op (see the comments in `pingpong_test.mvl` for the exact reasoning).

---

## Running

```bash
# From the repo root:
make build
cd examples/actor_pingpong
make run              # full end-to-end demo (Rust backend)
make test             # deterministic behavior tests (Rust backend)
make test-llvm        # same tests, LLVM backend
make test-wasm        # same tests, WASM backend
```

---

## Related

- Spec: `.openspec/specs/014-data-race-freedom/spec.md`
- ADR-0059: WASM actor scheduling (single-threaded run-to-completion — relevant to why the full cascade isn't tested end-to-end)
