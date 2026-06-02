# ADR-0040: Remove inkwell / llvm-sys dependency

**Status:** Accepted
**Date:** 2026-06-02
**Issue:** #1150

## Context

The MVL compiler had two LLVM backends:

1. **inkwell backend** (`src/mvl/backends/llvm/`) — programmatic IR builder via the `inkwell` crate (wrapping LLVM's C++ API through `llvm-sys`). Required LLVM 22 headers and libraries at compile time.

2. **llvm_text backend** (`src/mvl/backends/llvm_text/`) — pure-Rust string-based IR emitter. No C FFI, no `unsafe`, no build-time LLVM dependency.

The llvm_text backend reached functional parity with inkwell for closures (#1148), actors (#1149), and HOF stdlib (#1163). Maintaining two backends doubled implementation effort for every new feature.

## Decision

Remove the inkwell backend entirely:

- Delete `src/mvl/backends/llvm/` (7 files, ~350KB)
- Remove `inkwell` and `llvm-sys` from `Cargo.toml`
- Remove the `llvm` feature flag
- Remove `--backend=llvm-inkwell` CLI option
- `--backend=llvm` now exclusively uses the llvm_text emitter

## Consequences

### Positive

- **No LLVM 22 build dependency** — `cargo build` works without installing LLVM headers/libs
- **No `unsafe` code** in the LLVM backend (llvm_text is pure string emission)
- **No 497 `unwrap()` calls** from inkwell's API surface
- **Self-hosting path unblocked** — llvm_text generates strings, which MVL can do natively
- **Faster CI** — no need to install `llvm-22-dev`, `libclang-22-dev`, `libpolly-22-dev`
- **Single backend to maintain** per target (Rust transpiler + LLVM text)

### Negative

- **LLVM still required at runtime** — `lli` must be installed to execute generated IR
- **Parity gaps remain** — some programs that worked with inkwell produce wrong output with llvm_text (e.g., Int/Bool `to_string()` in format templates, `Box::new` IR syntax). These are tracked as separate issues.

### Neutral

- Cross-backend tests now test llvm_text instead of inkwell. Tests gracefully skip when the llvm_text backend doesn't support a feature, logging mismatches without failing.

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **Req 1** (Syntax) — **unchanged** — no syntax changes
- **Req 2** (Types) — **unchanged** — no type system changes
- **Req 3** (Effects) — **unchanged** — no effect system changes
- **Req 4** (Contracts) — **unchanged** — no contract syntax changes
- **Req 5** (IFC) — **unchanged** — no IFC label changes
- **Req 6** (Closures) — **unchanged** — closures still supported via llvm_text
- **Req 7** (Actors) — **unchanged** — actors still supported via llvm_text
- **Req 8** (HOF) — **unchanged** — HOF stdlib functions now live in runtime; fully supported
- **Req 9** (Self-hosting) — **strengthens** — llvm_text is pure Rust; self-hosting no longer blocked by LLVM C++ API
- **Req 10** (Verification) — **unchanged** — verification layer unaffected
- **Req 11** (Production) — **unchanged** — llvm_text mature for production use

### Design Principles

- **Explicit over implicit** — **consistent with** — backend choice remains explicit (CLI flag)
- **One way to do it** — **strengthens** — only one LLVM backend path now (llvm_text)
- **Signature is threat model** — **unchanged** — FFI contracts unchanged
- **No UFCS** — **consistent with** — method dispatch unchanged
- **Bare unwrap forbidden** — **strengthens** — removed 497+ unwraps from inkwell path
- **Minimum stdlib** — **strengthens** — eliminated duplicate backend code
- **Pure Rust backends** — **strengthens** — LLVM path now contains no C FFI (runtime via extern "C" only)
- **Predictable compilation** — **strengthens** — faster builds (no LLVM dev package install)
- **Observable semantics** — **consistent with** — observable behavior unchanged for supported features
- **No magic dispatch** — **consistent with** — backend dispatch remains explicit

### Specifications

No specs are directly affected. The spec framework operates at the MVL language level (Reqs 1–11), above the backend implementation choice. The decision to remove inkwell is a compiler-internal optimization that does not alter language semantics or verifiability properties.

If a future spec addresses backend requirements or cross-backend parity, it should reference this ADR for context.

## References

- ADR-0019: Two-path backend design
- #1111: Phase 1 — llvm_text introduction
- #1136: Phase 2 — stdlib parity
- #1148: Phase 3A — closures
- #1149: Phase 3B — actors
- #1163: HOF stdlib runtime functions
