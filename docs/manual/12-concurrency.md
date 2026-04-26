# 12. Concurrency

MVL provides safe concurrency through actors, reference capabilities, and structured concurrency ([Req 9](../requirements.md#req-9)).

## 12.1 Actors

Actors are the primary concurrency unit. Each actor has:
- Its own state (not shared)
- A message queue (mailbox)
- Reference capabilities that control what can be sent between actors

```mvl
type Counter = actor {
    mut count: Int,

    fn increment(self) -> () {
        self.count = self.count + 1;
    }

    fn get(self) -> Int {
        self.count
    }
}
```

**Design origin:** Pony (Clebsch et al., 2015). Simplified for MVL.

## 12.2 Reference Capabilities

| Capability | Meaning | Sendable? | Readable? | Writable? |
|-----------|---------|-----------|-----------|-----------|
| `iso` | Isolated — sole reference | Yes | Yes | Yes |
| `val` | Deeply immutable | Yes (shared) | Yes | No |
| `ref` | Local mutable | No | Yes | Yes |
| `tag` | Opaque identity | Yes | No | No |

**Rules:**
- Only `iso` and `val` can be sent between actors
- `ref` is confined to the creating actor
- `tag` can be sent but cannot be read — used for identity comparison

These rules guarantee data race freedom at compile time. No locks needed.

## 12.3 Channels

```mvl
let (tx, rx) = Channel.new[Message]();

// Sender (in one actor)
tx.send(iso message);                // must send iso or val

// Receiver (in another actor)
match rx.recv() {
    Some(msg) => process(msg),
    None => break,
}
```

## 12.4 Structured Concurrency

No orphan tasks. Every spawned task is tied to a scope:

```mvl
fn parallel_fetch(urls: Array[Url]) -> Array<Result[Response, Error]> ! Net, Async {
    scope(|s| {
        let handles = urls.map(|url| s.spawn(|| fetch(url)));
        handles.map(|h| h.join())
    })
}
```

When the scope exits, all spawned tasks are joined. No fire-and-forget.

## 12.5 WCET Refinements

For real-time systems, functions can declare worst-case execution time:

```mvl
fn process_sample(data: Array<Float64> where len(data) <= 1024)
    -> Float64
    wcet 100us
{
    // compiler verifies this terminates within 100 microseconds
    // (requires total + bounded iteration + no allocation)
}
```

This is an advanced feature for safety-critical domains (DO-178C, IEC 61508).

## 12.6 What's NOT in the Concurrency Model

- No shared mutable state between actors
- No raw threads
- No locks/mutexes exposed to user code (actors handle serialization)
- No async/await at language level (effect system handles it)
- No `unsafe` escape hatch for data races
