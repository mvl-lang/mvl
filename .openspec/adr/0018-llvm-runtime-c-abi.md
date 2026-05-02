# ADR-0018: Two-Path Stdlib Architecture — Rust API vs C-ABI cdylib

**Status:** Accepted
**Date:** 2026-05-02
**Context:** The LLVM backend (#431) needs access to stdlib functions (env, process, io, …) at runtime. These are already implemented in `mvl_runtime` as idiomatic Rust APIs. The LLVM backend cannot call Rust APIs directly — it needs C-ABI symbols resolvable at LLVM IR level.

## Decision

Introduce a second runtime crate, `mvl_runtime_c`, compiled as a `cdylib`. It wraps `mvl_runtime` Rust APIs behind `#[no_mangle] extern "C"` symbols. The LLVM backend loads it at runtime via `lli --load=libmvl_runtime_c.{so,dylib}`.

## Two-Path Architecture

```
Path 1 — Rust transpiler:
  MVL source → Rust source → cargo/rustc
  stdlib via `use mvl_runtime::prelude::*`   (Rust API, zero FFI overhead)
  native Rust types throughout

Path 2 — LLVM backend:
  MVL source → LLVM IR → lli
  stdlib via libmvl_runtime_c.so (C-ABI exports, this ADR)
  mvl_memory cdylib for heap collections (ADR-0016)
```

Cross-backend behavioral parity is enforced by `tests/cross_backend.rs`.

## Crate Responsibilities

| Crate | Kind | Used by | Purpose |
|-------|------|---------|---------|
| `mvl_runtime` | rlib | Rust transpiler | Idiomatic Rust stdlib APIs, IFC types, prelude |
| `mvl_memory` | cdylib+rlib | LLVM backend | Heap allocation, reference counting (ADR-0016) |
| `mvl_runtime_c` | cdylib+rlib | LLVM backend | C-ABI wrappers for stdlib functions |

## C-ABI Marshalling Types

Defined in `mvl_runtime_c/src/abi.rs`:

- `MvlOption` — `{ tag: u8, payload: *mut c_void }` — None = tag 0, Some = tag 1
- `MvlResult` — `{ tag: u8, ok_payload: *mut c_void, err_payload: *mut c_void }` — Ok = tag 0, Err = tag 1
- `MvlString*`, `MvlArray*`, `MvlMap*` reused from `mvl_memory`

## Export Convention

All C-ABI exports use the `_mvl_` prefix and follow `mvl_memory`'s pattern:

```rust
#[no_mangle]
pub extern "C" fn _mvl_runtime_version() -> *const libc::c_char { ... }
```

The `mvl_c_export!` macro generates the boilerplate for mechanical wrappers.

## LLVM Backend Integration

`src/mvl/codegen/runtime_c.rs` provides lazy-declaration helpers (same pattern as `memory.rs`).
`main.rs` passes `--load=libmvl_runtime_c.{so,dylib}` to lli when the library is found (optional — programs not using C-ABI stdlib still run without it).

## Pilot Function

`_mvl_runtime_version() -> *const c_char` — returns the crate version as a static C string.
Used as a smoke test that the cdylib builds, loads, and resolves under lli.

## Consequences

- The Rust transpiler path is unaffected; `mvl_runtime` Rust APIs stay idiomatic.
- Each new stdlib function requires two implementations: Rust API in `mvl_runtime`, C-ABI wrapper in `mvl_runtime_c`. The macro minimises the wrapper boilerplate to ~5 lines.
- `make build-llvm-runtime` builds both LLVM cdylibs in one step.
- Loading `libmvl_runtime_c` is optional — programs not using the wrapped stdlib functions run without it.
