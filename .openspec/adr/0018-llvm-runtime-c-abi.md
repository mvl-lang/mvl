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

### Generic dispatch (mandatory pattern)

Adding a new stdlib function to the LLVM backend requires **no changes** to the codegen layer. The dispatch is fully generic:

1. `main.rs::build_stdlib_fn_map()` scans the program's `use std.X.{...}` declarations,
   parses the corresponding stdlib module from `STDLIB_FILES`, and extracts every `FnDecl`
   signature into a `HashMap<String, StdlibFnInfo>`.
2. `compile_to_ir` receives the map and stores it in `LlvmBackend::stdlib_fns`.
3. In `emit_fn_call`, after user-defined functions are checked, unresolved names fall through
   to `emit_stdlib_call`, which:
   - Derives the C symbol as `_mvl_{module}_{fn_name}`
   - Lowers MVL parameter/return types via `stdlib_type_to_llvm_meta` / `stdlib_return_type`
   - Declares and calls the external symbol via `get_or_declare_fn`
   - Emits `unreachable` + sets `terminated` for `Never`-return functions (e.g. `exit`)

**Do NOT add per-function `get_mvl_*()` getter methods to `runtime_c.rs`** — the old pattern
from issue #408 was explicitly rejected here. The `stdlib_type_to_llvm_meta` helper covers
all scalar types; add new type mappings there as needed.

`main.rs` passes `--load=libmvl_runtime_c.{so,dylib}` to lli when the library is found (optional — programs not using the wrapped stdlib functions run without it).

## Pilot Function

`_mvl_runtime_version() -> *const c_char` — returns the crate version as a static C string.
Used as a smoke test that the cdylib builds, loads, and resolves under lli.

## Consequences

- The Rust transpiler path is unaffected; `mvl_runtime` Rust APIs stay idiomatic.
- Each new stdlib function requires two implementations: Rust API in `mvl_runtime`, C-ABI wrapper in `mvl_runtime_c`. The macro minimises the wrapper boilerplate to ~5 lines.
- `make build-llvm-runtime` builds both LLVM cdylibs in one step.
- Loading `libmvl_runtime_c` is optional — programs not using the wrapped stdlib functions run without it.
