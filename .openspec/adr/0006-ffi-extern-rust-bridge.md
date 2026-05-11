# ADR-0006: FFI via extern "rust" and the bridge.rs Convention

**Status:** Accepted
**Date:** 2026-04-12
**Context:** MVL programs need to call existing Rust libraries (argument parsing, filesystem, cryptography, networking) without requiring the entire ecosystem to be rewritten in MVL. How should the FFI mechanism be designed so that the trust boundary is explicit, auditable, and enforced at compile time?

## Decision

MVL SHALL use `extern "rust"` blocks to declare trust boundaries, and the `bridge.rs` convention to provide implementations. The build tool (`mvl build`) SHALL discover and link `bridge.rs` automatically.

```mvl
// In foo.mvl — declaration side (verified by MVL)
extern "rust" {
    fn read_file(path: Clean[String]) -> Tainted[String];
}
```

```rust
// In bridge.rs — implementation side (trusted but unverified)
#[no_mangle]
pub extern "Rust" fn read_file(path: Clean[String]) -> Tainted[String] {
    Tainted(std::fs::read_to_string(&**path).unwrap())
}
```

## Academic Context

FFI design splits into two fundamentally different philosophies.

**Rust's approach** is zero-cost and direct: calling C is a raw ABI call with no runtime overhead, but safety guarantees end at the `unsafe` block boundary. The language gives you the knife — you're responsible for not cutting yourself.

**Go's CGo** imposes a runtime cost (~171 ns/call) because Go's goroutine stack model requires a context switch to C's fixed-size stack. CockroachDB famously documents this overhead causing production pprof blindspots and broken cross-compilation. This is the core engineering tradeoff: Rust's approach is faster but more dangerous; CGo is slower but better encapsulated.

**The research trajectory:**

The academic literature has largely moved past "is FFI safe?" (answer: no) toward quantifying and bounding the damage:

- **FFIChecker** (Song/Li 2022, ESORICS) — scanned 987 crates, found 34 memory bugs at FFI boundaries
- **McCormack 2025** (ICSE, CMU, doi:10.48550/arXiv.2404.11671) — characterised undefined behaviour patterns specifically at language boundaries: lifetime violations, aliasing rule breaks, misaligned access — bugs the Rust type checker cannot catch because they originate from C's side
- **SafeFFI** (Braunsdorf 2025, doi:10.48550/arXiv.2510.20688) — closes the gap at runtime with boundary-hoisted sanitizer checks, achieving low overhead by bundling checks at entry/exit rather than per-operation
- **Sandcrust 2017 / Gülmez 2023** — the isolation thread: sandboxes unsafe components so a memory bug in an FFI call cannot corrupt safe Rust's heap (defence-in-depth: you can't make C safe, but you can contain it)
- **Miri** (Jung 2026, POPL) — de facto UB detector for Rust including FFI calls; uses a virtual machine interpreting MIR that detects undefined behaviour before it reaches hardware
- **Cai 2026** (TOSEM) — examined 320 bugs across bindgen, cbindgen, and cxx; the main obstacle is not tool crashes but incorrect code generation, primarily around opaque types, lifetimes, and struct layout mismatches

**MVL's position in this landscape:**

The `extern "rust"` design is architecturally ahead of the dominant practice. The literature converges on exactly what this architecture already provides — typed boundaries, effect tracking, IFC labels — as the right structural answer.

**The gap McCormack + SafeFFI define is precisely MVL's Phase 3 target:**

McCormack 2025 identifies the UB patterns that escape the type system at language boundaries (lifetime violations, aliasing, misaligned access). SafeFFI 2025 shows runtime boundary checks can close this gap, but at a cost. MVL's path is more direct: the IFC labels and effect types on `extern "rust"` function signatures provide enough structure to discharge boundary conditions as SMT lemmas — **compile-time proofs rather than runtime checks**.

```
Phase 2 (done):  runtime IFC/effect enforcement — labels enforced via Rust newtypes,
                 debug_assert! guards at trust boundaries
Phase 3 (target): SMT-proven boundary contracts — extern "rust" fn signatures become
                 formal proof obligations discharged by an SMT solver at compile time
```

This path is validated by the research: McCormack defines the problem, SafeFFI proves runtime enforcement works but has overhead, and MVL's type-theoretic approach is the compile-time solution the field hasn't yet built.

Because MVL uses Rust-to-Rust FFI (not Rust-to-C), the entire class of C ABI bugs (layout mismatches, lifetime violations across C, unaligned access) is eliminated. The bridge code is still unverified Rust, but it operates entirely within Rust's memory model — Miri can check it, the borrow checker can partially check it, and the IFC type system labels every value that crosses the boundary.

## Rationale

### Why FFI at all?

MVL is a verification language. Its value is what it can prove. But real programs need real libraries: command-line argument parsing, file I/O, TLS, database drivers. Requiring all of these to be rewritten in MVL would make MVL unusable.

The alternative is to accept that some code is unverified Rust, but make the choice **explicit and greppable**. The `extern "rust"` block is the declaration: "this is where verification ends."

### Why `extern "rust"` syntax (not `extern "C"`)?

| ABI | Considered | Decision |
|-----|------------|----------|
| `extern "C"` | Standard for FFI in most languages | Rejected for Rust-to-Rust |
| `extern "Rust"` | Native Rust ABI, no marshalling overhead | Accepted |

The MVL compiles to Rust. Rust-to-Rust FFI via `extern "Rust"` is zero-cost — no marshalling, no C ABI conversion, no `unsafe` boilerplate beyond what the generated code already emits. The bridge and the MVL-generated code live in the same binary; the Rust ABI is the right tool.

`extern "C"` would be the right choice if the bridge called into a C library or if the MVL compiler ever targets a non-Rust backend. That is deferred.

### Why a sibling `bridge.rs` file (not a separate crate)?

**Alternatives considered:**

| Approach | Considered | Rejected because |
|----------|------------|-----------------|
| Separate crate in `bridge/` | Clean separation | Overkill for Phase 2; would require a second Cargo.toml and workspace configuration |
| Inline Rust blocks in `.mvl` source | Like Swift's `#if canImport` or Kotlin's `actual` | Mixes languages in one file; the MVL file is supposed to be fully verifiable MVL |
| Named bridge file (e.g., `foo_bridge.rs` matching `foo.mvl`) | Pairs each MVL file with its bridge | More flexible but also more to discover; a single `bridge.rs` per directory is simpler |
| `bridge/` directory with multiple files | Better for large trust surfaces | Deferred — `bridge.rs` handles the Phase 2 case; multi-file bridges can be added later |

A **sibling `bridge.rs`** is:
- Zero-configuration (no Cargo.toml edits)
- Co-located with the MVL source (easy to find)
- Unambiguous (one file, fixed name)
- Greppable (`grep -r bridge.rs` finds all trust boundaries in a repo)

### Why `#[no_mangle]` on bridge functions?

The generated `main.rs` contains `extern "Rust" { fn foo(); }` — a declaration that `foo` is an external symbol. The bridge.rs is compiled into the same crate via `mod bridge;`. For the `extern "Rust"` declaration to resolve to the bridge function, the symbol must be available under its plain name. `#[no_mangle]` ensures this.

Without `#[no_mangle]`, the Rust compiler would mangle the function name (e.g., `_ZN6bridge3fooE`) and the extern declaration would fail to link.

### Why inject `mod bridge;` rather than `use bridge::*;`?

`mod bridge;` includes the bridge module in the crate's compilation unit. `use bridge::*;` would re-export all bridge symbols into scope, creating potential name collisions with MVL-generated types. `mod bridge;` keeps bridge functions in the `bridge::` namespace and lets the `extern "Rust"` declarations resolve via `#[no_mangle]` without polluting the top-level namespace.

### Why fail-fast on missing bridge?

If `extern "rust"` is declared and `bridge.rs` is absent, `mvl build` exits with an error **before** calling cargo. This is deliberate:

1. **No silent linker errors.** Without fail-fast, the user sees a cargo linker error pointing at generated code — confusing because the generated code is not the source of the problem.
2. **The error points at the right place.** The MVL tool knows the source file and the expected bridge location; cargo does not.
3. **Enforces the contract.** `extern "rust"` is a declaration of intent: "I will provide implementations." Not providing them is an error, not a warning.

### Why detect `extern "rust"` specifically (not `extern_count > 0`)?

`extern_count` counts all ABI blocks including `extern "c"`. A C library bridge would be linked differently (typically via a build.rs script or a pre-existing Rust crate). The bridge.rs convention is specific to `extern "rust"`. A dedicated `has_extern_rust` flag in `TranspileOutput` makes the intent clear and prevents false positives when `extern "c"` blocks are introduced.

### The assurance angle

The `extern_count` metric in `mvl assurance` counts trust boundaries. The bridge.rs convention makes each boundary auditable:

```
extern "rust" block in foo.mvl  ←→  implementation in bridge.rs
```

A future version of `mvl assurance` can validate that every declared extern fn has a corresponding `#[no_mangle]` implementation in `bridge.rs`, and flag missing or extra implementations.

## Implementation

```
src/main.rs
  build_project()          — bridge discovery, copy, error if missing
  inject_mod_bridge()      — inserts mod bridge; after use mvl_runtime::prelude::*;

src/mvl/backends/rust/mod.rs
  TranspileOutput.has_extern_rust  — flag set by transpiler
  has_extern_rust_decls()          — predicate on Program AST

examples/log_analyzer/
  main.mvl    — reference example using the convention
  bridge.rs   — reference bridge implementation
```

## Current limitations (to address in future phases)

| Limitation | Mitigation | Future path |
|------------|------------|-------------|
| One `bridge.rs` per directory | Sufficient for Phase 2 single-file programs | `bridge/` directory with multiple files |
| No signature validation | Type mismatch caught by `cargo build` (Rust type system) | `mvl assurance` cross-check in Phase 3 |
| `extern "c"` not linked via bridge.rs | C libraries linked via separate `build.rs` (standard Rust approach) | Document `build.rs` + `extern "c"` pattern |
| Bridge functions not tracked in assurance output | `extern_count` counts blocks, not functions | Add per-function trust surface metrics |

## Consequences

- Every trust boundary is declared in MVL source and has an implementation in a co-located, greppable file
- `mvl build` enforces the contract: no bridge → no build
- The verified fraction of a program is `1 - (bridge lines / total lines)` — visible and measurable
- Rust ecosystem libraries are accessible without rewriting them in MVL
- Bridge code is unverified by MVL — the author accepts responsibility for its correctness
