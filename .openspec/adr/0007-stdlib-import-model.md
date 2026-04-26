# ADR-0007: Standard Library Import Model — Prelude, Explicit, and Trust Boundaries

**Status:** Accepted
**Date:** 2026-04-14
**Context:** How does MVL code access the standard library? The design must balance convenience (no boilerplate on every file) with the MVL's commitment to explicitness and effect visibility.

## Decision

Three-tier import model. The boundary rule: **if the compiler needs it to verify, it's prelude. If it adds effects, it's explicit import.**

| Tier | Import mechanism | Rationale |
|---|---|---|
| Core (~30 types) | Implicit prelude — always available | Types the compiler needs for verification: Option, Result, Array, String, basic ops. Requiring `use std.core.Option` on every file is noise, not safety. |
| Standard (~200 functions) | Explicit `use std.*` | Adds effects (FileRead, Net, Clock, CryptoRandom). The import is where the effect enters your module — it shows up in every function signature downstream. |
| Extended (packages) | Explicit `use pkg.*` | Third-party code. Crosses a trust boundary. Effect and IFC labels enforced at the package API surface. |

## Core prelude contents

Types: `Bool`, `Int`, `Int8`..`Int64`, `UInt8`..`UInt64`, `Float32`, `Float64`, `Byte`, `Char`, `String`, `Array[T]`, `Map[K,V]`, `Set[T]`, `Option[T]`, `Result[T,E]`, `Tuple`, `Range`.

Traits: `Eq`, `Ord`, `Hash`, `Display`, `Debug`, `Iterator`.

Functions: `println`, `eprintln`, basic arithmetic, `?` propagation.

No effects in the prelude. `println` requires `! Console` — but the *type* is prelude, the *effect* is visible in your signature.

## Why not everything explicit (Go model)?

The MVL already dropped every implicit feature that hurts verification (implicit conversions, default args, macros, inheritance, null). The remaining prelude items are safe to keep implicit because they ARE the type system — `Option[T]` and `Result[T,E]` aren't library conveniences, they're how the compiler reasons about absence and errors. Forcing explicit imports for these adds verbosity without adding safety.

## Why not everything implicit (Python model)?

Python imports everything and you can't tell from a file what it depends on. The MVL's standard tier adds effects — `use std.fs` is where `! FileRead` enters your module. This is the real access control: not the import statement, but the effect system. The explicit import is documentation; the effect annotation is enforcement.

## Examples

Core — no import needed:
```mvl
fn main() -> Result[(), Error] ! Console {
    let names = ["alice", "bob", "charlie"]
    let upper = names.map(String.to_upper)
    println(upper.join(", "))
}
```

Standard — explicit import, effects visible:
```mvl
use std.fs.{File, read_to_string}
use std.json

fn load_config(path: Path) -> Result[Config, Error] ! FileRead {
    let text = read_to_string(path)?
    json.decode[Config](text)
}
```

Extended — explicit import, trust boundary:
```mvl
use pkg.http.{Server, Request, Response}

fn handle(req: Request) -> Response ! Net, Log {
    // pkg.http is third-party — MVL compiler enforces effect/IFC at the boundary
}
```

## Consequences

- Core types available everywhere — consistent, no boilerplate
- Effect introduction is always visible via `use std.*` — greppable, auditable
- Third-party code is always behind `use pkg.*` — trust boundary is explicit
- `mvl audit` can report: which modules introduce which effects, which packages cross trust boundaries
- SBOM generation knows the full dependency tree from import analysis

## Connected to

- ADR-0001: Eleven requirements (effects = Req 7, IFC = Req 11)
- ADR-0002: Language contraction (no implicit conversions, but implicit prelude is safe)
- ADR-0006: FFI extern "rust" bridge (trust boundary between MVL and Rust)
- Phase 4 (#130): Verified standard library
- Epic 6 (#41): Stdlib stories
