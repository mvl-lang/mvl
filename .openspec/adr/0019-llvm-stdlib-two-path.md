# ADR-0019: Two-Path Stdlib Architecture — Rust Crate + C-ABI cdylib

**Status:** Accepted
**Date:** 2026-05-02
**Issues:** #431, #432
**Supersedes:** —
**Related:** ADR-0003 (transpilation strategy), ADR-0016 (LLVM memory runtime)

---

## Context

MVL has two code-generation backends:

1. **Rust transpiler** — emits Rust source, compiled by `cargo`/`rustc`.
2. **LLVM backend** — emits LLVM IR, interpreted by `lli`.

The stdlib (`std.env`, `std.process`, etc.) is implemented in `mvl_runtime` as idiomatic Rust.  The Rust transpiler accesses it natively via `use mvl_runtime::prelude::*`.  The LLVM backend previously had no access to stdlib — it was limited to built-ins (println, arithmetic, heap collections via `mvl_memory`).

Epic #314 requires full stdlib parity across both backends.

---

## Decision

Introduce a second crate, `mvl_runtime_c`, that wraps `mvl_runtime` Rust APIs with `#[no_mangle] extern "C"` symbols.  This crate is compiled as a `cdylib` and loaded by `lli` at runtime alongside `mvl_memory`.

```
Path 1 (Rust transpiler):  MVL → Rust source → cargo/rustc
                            stdlib via `use mvl_runtime::prelude::*` (Rust APIs)
                            native Rust types throughout

Path 2 (LLVM backend):     MVL → LLVM IR → lli
                            stdlib via libmvl_runtime_c.{so,dylib} (C-ABI exports)
                            primitive types + opaque pointers across the boundary
```

Cross-backend behavioral parity is enforced by `tests/cross_backend.rs` — identical output from both backends for the same MVL program.

---

## Symbol naming convention

All C-ABI exports use the prefix `_mvl_` followed by the module and function name:

| MVL stdlib call | C-ABI symbol          |
|-----------------|-----------------------|
| `env.getuid()`  | `_mvl_env_getuid`     |
| `env.getgid()`  | `_mvl_env_getgid`     |
| `env.get(name)` | `_mvl_env_get`        |
| `env.exit(n)`   | `_mvl_env_exit`       |
| `process.spawn` | `_mvl_process_spawn`  |
| version (pilot) | `_mvl_runtime_version`|

---

## ABI marshalling types

Values that cross the C boundary use these types (defined in `mvl_runtime_c/src/abi.rs`):

| MVL type              | C-ABI representation                                        |
|-----------------------|-------------------------------------------------------------|
| `Int`                 | `i64`                                                       |
| `Bool`                | `i8` (0=false, 1=true)                                      |
| `String` (input)      | `*const c_char` (caller owns, not freed by callee)          |
| `String` (output)     | `*mut c_char` (callee allocates, caller frees with `free`) |
| `Option[T]`           | `MvlOption { tag: u8, payload: *mut c_void }`               |
| `Result[T, E]`        | `MvlResult { tag: u8, payload: *mut c_void, err: *mut c_char }` |
| Process handles       | `*mut c_void` (opaque Box pointer)                          |

---

## LLVM backend integration

The LLVM backend auto-discovers the library at startup:

```rust
// src/mvl/codegen/mod.rs
pub fn find_mvl_runtime_c_lib() -> Option<PathBuf> { ... }
```

Search order:
1. `MVL_RUNTIME_C_LIB` environment variable
2. Sibling of the current executable in `target/{debug,release}/` and `deps/`

The `lli` invocation in `run_project_llvm` and `cmd_test_llvm` adds `--load=libmvl_runtime_c.{so,dylib}` when the library is found.

`use std.env.{getuid, getgid}` imports in MVL source are recognized by `collect_stdlib_imports()` in the LLVM backend, which maps them to their `_mvl_*` C-ABI counterparts for dispatch in `emit_fn_call`.

---

## Build

```bash
make build-llvm-runtime   # builds mvl_memory + mvl_runtime_c
```

Both cdylibs are required for programs that use stdlib + heap collections via LLVM.

---

## Consequences

### Positive
- Stdlib becomes available to LLVM-compiled MVL programs without changing the transpiler path.
- Adding a new stdlib function to the LLVM path is ~10 lines of Rust in `mvl_runtime_c`.
- Cross-backend parity is testable via `tests/cross_backend.rs`.

### Negative
- Two crates to maintain for stdlib functions (Rust API + C-ABI wrapper).
- Complex marshalling (strings, arrays, maps) requires care to avoid leaks.
- The LLVM path has no garbage collection — callers must free returned strings.

### Neutral
- `mvl_runtime_c` does **not** depend on `mvl_memory` at the Rust crate level.  String outputs use `*mut c_char` (allocated via `CString::into_raw`), not `MvlString*`.  This avoids symbol conflicts when both libraries are loaded by `lli`.
