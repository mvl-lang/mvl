# 16. Standard Library

The stdlib has three tiers. The language grows through the stdlib, not through language changes.

See [stdlib.md](../stdlib.md) for the complete specification.

## 16.1 Core (~30 types/functions)

Every program needs these. Verified to the same standard as the compiler.

**Types:** `Bool`, `Int`, `Int8`..`Int64`, `UInt8`..`UInt64`, `Float32`, `Float64`, `Byte`, `Char`, `String`, `Array<T>`, `Map<K,V>`, `Set<T>`, `Option<T>`, `Result<T,E>`, `Tuple`, `Range`.

**Key operations:** String manipulation, collection ops (`map`, `filter`, `fold`, etc.), error combinators, basic I/O (`print`, `println`), OS basics (`env`, `args`, `exit`).

## 16.2 Standard (~200 functions)

Most programs need these. File I/O, path handling, filesystem operations, regex, math, random, time/datetime, concurrency primitives, JSON, TOML, basic crypto, process management, testing, logging.

## 16.3 Extended (packages)

Third-party ecosystem. Networking (TCP, HTTP, TLS), serialization extras (YAML, XML, CSV, protobuf), crypto extras (AES, RSA, ECDSA), database drivers, CLI parsing, compression, advanced data structures.

## 16.4 How the Type System Changes the Stdlib

Every stdlib function respects the eleven requirements:

- `Map.get()` returns `Option<T>` (never panics) — Req 4
- `File.read()` returns `Result<String, IOError> ! FileRead` — Req 5, 7
- `http.get()` returns `Tainted<Response>` — Req 11
- `divide(a, b where b != 0)` — Req 10
- `a + b` on `Int32` is checked — Req 10

The stdlib isn't just functions — it's contracts the compiler verifies.

## 16.5 Test Helpers

| Helper | Stubs |
|--------|-------|
| `StubFS { files }` | Filesystem (in-memory) |
| `in_memory_db(rows)` | Database (no connection) |
| `mock_channel()` | Channel (records sent messages) |
| `fixed_clock(timestamp)` | Clock (deterministic) |
| `seeded_random(seed)` | Random (reproducible) |
| `capture_log()` | Logging (captures entries for assertion) |

No mock framework needed — effects + traits make test doubles trivial.
