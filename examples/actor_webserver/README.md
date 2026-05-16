# actor_webserver

HTTP server with actor-per-request pattern — demonstrates `iso` and `tag` capabilities in a real-world context.

**Phase 8 example** — requires actor runtime ([#695](https://github.com/LAB271/mvl_language/issues/695)).
Syntax is complete; codegen lands in #695 / #696.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| `iso` ownership transfer | `pub fn handle(iso req: Request)` | Request moves from Listener to Handler — no copying, no sharing |
| `tag` shared identity | `db: DbPool` field + `tag caller: RequestHandler` param | Callers can query the pool but cannot read its state |
| `val` immutable result | `pub fn query_done(val result: QueryResult, ...)` | Query result is deeply immutable — safe to share freely |
| Actor-per-request | One `RequestHandler` per incoming connection | Erlang/Akka style: each request has its own actor, no shared mutable state |
| Structured concurrency | `concurrently { ... }` | Scope blocks until all actor mailboxes drain |

---

## Architecture

```
main()
  concurrently {
    db       = actor DbPool { connections: 10 }
    listener = actor Listener { port: 8080, db: db }
    h1       = actor RequestHandler { db: db }   ─┐
    h2       = actor RequestHandler { db: db }    ├─ all hold tag DbPool
    h3       = actor RequestHandler { db: db }   ─┘

    listener.accept(Request #1, h1)  ──► h1.handle(iso req)
    listener.accept(Request #2, h2)  ──► h2.handle(iso req)
    listener.accept(Request #3, h3)  ──► h3.handle(iso req)
  }
  ↓ blocks until all mailboxes empty
```

### Ownership flow for a `/users` request

```
main()
  → listener.accept(iso req #1, tag h1)
      Listener logs "accepted GET /users"
      → h1.handle(iso req)             ← req ownership moves here
          Handler logs "routing Users"
          → db.query("SELECT ...", req_id=1, tag self)
              DbPool logs query
              → h1.query_done(val result, req_id=1)
                  Handler logs "200 OK — [{name: Alice}, ...]"
```

---

## Why `iso` for Request and Response?

An HTTP request is a uniquely owned value: exactly one handler should process it. Using `iso` enforces this at compile time — the compiler rejects any attempt to share or copy the request across actors.

```
Listener creates   iso Request { id: 1, ... }
                       │
                       │  ownership transfers on send
                       ▼
handler.handle(    iso req    )   ← Listener can no longer access req
                       │
                       │  handler owns it exclusively while processing
                       ▼
             processes and discards
```

If you try to pass the same `iso` value twice — the compiler rejects it:

```
// Error: req consumed by handle(); cannot use again
listener.accept(req, h1)
listener.accept(req, h2)  // ← compile error: req already consumed
```

---

## Why `tag` for DbPool?

Multiple handlers share one database pool, but no handler should be able to read or modify the pool's internal state (connection count, connection objects, etc.).

`tag` gives you identity without access:

| Capability | Read pool state | Write pool state | Send queries | Used for |
|------------|----------------|-----------------|--------------|----------|
| `ref`      | yes            | yes             | yes          | local mutable access (cannot cross actor boundary) |
| `iso`      | yes            | yes             | yes          | exclusive ownership — only one holder |
| `val`      | yes            | no              | yes          | deeply immutable view |
| **`tag`**  | **no**         | **no**          | **yes**      | **actor identity — send messages only** |

```mvl
actor RequestHandler {
    db: DbPool   // tag — can call db.query(...), cannot read db.connections
    ...
}
```

The compiler rejects any attempt to read `db.connections` inside `RequestHandler` — the `tag` capability has no read access.

---

## Why `val` for QueryResult?

A query result from the database is immutable — it reflects a point-in-time snapshot. Using `val` declares this explicitly:

- Multiple handlers can receive the same `QueryResult` without copying.
- No handler can mutate the result (accidental modification would affect all holders).
- The compiler verifies that `val` values are never written through.

```mvl
let result: QueryResult = QueryResult { data: "..." }
caller.query_done(result, req_id)
// DbPool and caller both see the same immutable result — safe, no race.
```

---

## Running

```bash
# From the repo root:
make build
cd examples/actor_webserver
make run
```

Expected output:

```
Listener: accepted GET /users (req #1)
Handler: routing Users (req #1)
DbPool: query 'SELECT * FROM users' for req #1
Handler: 200 OK (req #1) — [{name: Alice}, {name: Bob}]
Listener: accepted GET /health (req #2)
Handler: routing Health (req #2)
Handler: 200 OK (req #2) — ok
Listener: accepted DELETE /unknown (req #3)
Handler: routing Unknown (req #3)
Handler: 404 Not Found (req #3)
Server: all requests processed.
```

---

## Related

- Issue: [#581 actor webserver example](https://github.com/LAB271/mvl_language/issues/581)
- Epic:  [#579 Phase 8 actor examples](https://github.com/LAB271/mvl_language/issues/579)
- Basic actor example: [actor_pingpong](../actor_pingpong/)
- Actor runtime (Rust backend): [#695](https://github.com/LAB271/mvl_language/issues/695)
- Actor runtime (LLVM backend): [#696](https://github.com/LAB271/mvl_language/issues/696)
- Spec (actors): `.openspec/specs/015-actors/spec.md`
- Spec (capabilities): `.openspec/specs/014-data-race-freedom/spec.md`
