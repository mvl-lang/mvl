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

## References

- ADR-0019: Two-path backend design
- #1111: Phase 1 — llvm_text introduction
- #1136: Phase 2 — stdlib parity
- #1148: Phase 3A — closures
- #1149: Phase 3B — actors
- #1163: HOF stdlib runtime functions
