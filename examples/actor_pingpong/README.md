# actor_pingpong

Two actors exchanging messages — the simplest possible actor communication pattern.

**Phase 8 example** — requires actor runtime ([#695](https://github.com/LAB271/mvl_language/issues/695)).
Syntax is complete; codegen lands in #695 / #696.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Actor definition | `actor Pong { ... }` | Private mutable state + behaviors |
| Behavior | `pub fn ping(val msg: PingMsg, tag sender: ActorRef)` | Async message handler |
| Private helper | `fn log(seq: Int) -> Unit ! Console` | Synchronous, internal only |
| Actor creation | `let tag pong: ActorRef = actor Pong { received: 0 }` | Spawn actor, get tag handle |
| `val` capability | `val msg: PingMsg` | Immutable message — shareable, no ownership transfer |
| `tag` capability | `tag sender: ActorRef` | Identity-only reference — sendable, no read/write |
| Message send | `sender.pong(PongMsg { seq: msg.seq })` | Fire-and-forget async dispatch |
| Structured concurrency | `concurrently { ... }` | Scope drains all mailboxes before returning |

---

## How it works

```
main()
  concurrently {
    spawn Pong                          actor Pong { received: 0 }
    spawn Ping (partner = pong)         actor Ping { rounds: 5, sent: 0, partner: pong }
    ping.start()
  }
  ↓ blocks until all mailboxes empty
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

Expected output:

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
Done -- 5 ping-pong rounds complete.
```

---

## Capability rules (why val and tag?)

| Capability | Read | Write | Sendable | Used for |
|------------|------|-------|----------|----------|
| `val` | yes | no | yes | Immutable messages (`PingMsg`, `PongMsg`) |
| `tag` | no | no | yes | Actor identity / reply address (`ActorRef`) |
| `ref` | yes | yes | **no** | Local mutable state (actor fields, local vars) |
| `iso` | yes | yes | yes | Owned heap values transferred across boundaries |

`ref` values cannot cross actor boundaries — the compiler rejects attempts to send them.

---

## Running

```bash
# From the repo root:
make build
cd examples/actor_pingpong
make run
```

---

## Related

- Issue: [#580 actor pingpong example](https://github.com/LAB271/mvl_language/issues/580)
- Epic:  [#579 Phase 8 actor examples](https://github.com/LAB271/mvl_language/issues/579)
- Actor runtime (Rust backend): [#695](https://github.com/LAB271/mvl_language/issues/695)
- Actor runtime (LLVM backend): [#696](https://github.com/LAB271/mvl_language/issues/696)
- Spec: `.openspec/specs/014-data-race-freedom/spec.md`
