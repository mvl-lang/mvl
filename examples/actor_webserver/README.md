# actor_webserver

HTTP server with actor-per-request pattern — demonstrates `iso` capabilities, `pkg.http` types, and layered configuration.

**Phase 8 example** — requires actor runtime ([#695](https://github.com/LAB271/mvl_language/issues/695)).
Syntax is complete; codegen lands in #695 / #696.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| `pkg.http` types | `Request`, `Response`, `HttpError` | Structured HTTP protocol handling via package |
| `iso` ownership transfer | `pub fn handle(iso stream: TcpStream)` | Stream moves from accept loop to handler — no copying, no sharing |
| Actor-per-request | One `RequestHandler` per incoming connection | Each request has its own actor, no shared mutable state |
| Layered config | defaults → TOML → env → CLI | Composable configuration via `std.config` |

---

## Architecture

```
main()
  → load_config()               (defaults → config.toml → env → CLI)
  → tcp_listen(host, port)
  → serve(listener)
      while true:
        stream = tcp_accept(listener)
        h = actor RequestHandler {}
        h.handle(consume(stream))   ← iso transfer, accept loop no longer owns stream

RequestHandler.handle(iso stream):
  raw = tcp_read_request(stream)       ← Tainted[String]
  req = parse_request(raw)             ← pkg.http: detaint + parse
  resp = route(req)                    ← pure routing: Request → Response
  tcp_write(stream, serialize_response(resp))
  tcp_close_stream(stream)
```

---

## Why `iso` for TcpStream?

A TCP connection is a uniquely owned resource: exactly one handler should read from and write to it. Using `iso` enforces this at compile time — the compiler rejects any attempt to share or copy the stream across actors.

```
accept loop creates   iso stream
                          │
                          │  ownership transfers via consume()
                          ▼
handler.handle(       iso stream    )   ← accept loop can no longer access stream
                          │
                          │  handler owns it exclusively while processing
                          ▼
                tcp_close_stream(stream)
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
2026-05-22 10:00:00 INFO  server starting host=127.0.0.1 port=8000
```

---

## Related

- HTTP package: `pkg/http/` ([#783](https://github.com/LAB271/mvl_language/issues/783))
- Issue: [#581 actor webserver example](https://github.com/LAB271/mvl_language/issues/581)
- Epic: [#579 Phase 8 actor examples](https://github.com/LAB271/mvl_language/issues/579)
- Basic actor example: [actor_pingpong](../actor_pingpong/)
- Actor runtime: [#695](https://github.com/LAB271/mvl_language/issues/695), [#696](https://github.com/LAB271/mvl_language/issues/696)
- Spec (actors): `.openspec/specs/015-actors/spec.md`
- Spec (capabilities): `.openspec/specs/014-data-race-freedom/spec.md`
