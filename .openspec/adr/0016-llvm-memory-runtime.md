# ADR-0016: LLVM Memory Runtime — Rust cdylib with Reference Counting

**Status:** Accepted
**Date:** 2026-05-01
**Context:** Phase C of the LLVM backend (#391) introduces heap-allocated types: `String`, `Array`, and `Map`. These require a runtime memory library that the emitted LLVM IR can call at execution time.

## Decision

Ship a dedicated Rust crate (`mvl_memory`) compiled as a `cdylib`, providing C-ABI functions for heap allocation, reference counting, and deallocation of MVL's collection types. The `lli` interpreter loads it at runtime via the `--load` flag.

## Memory Layout

All heap-allocated MVL collection types share a common header followed by type-specific data. The header is always at offset 0.

### Common RC Header

```
struct MvlRcHeader {
    refcount: u64,   // reference count; starts at 1
}
```

### MvlString — `{ *const u8, usize len, usize cap, MvlRcHeader }`

```
offset 0:  ptr  — pointer to UTF-8 bytes (null-terminated for printf compat)
offset 8:  len  — byte length (excluding null terminator)
offset 16: cap  — allocated capacity in bytes
offset 24: rc   — reference count (u64)
```

LLVM IR type: `%MvlString = type { ptr, i64, i64, i64 }`

### MvlArray — `{ *mut u8, usize len, usize cap, usize elem_size, MvlRcHeader }`

```
offset 0:  ptr        — pointer to element data
offset 8:  len        — number of elements
offset 16: cap        — capacity in elements
offset 24: elem_size  — size of each element in bytes
offset 32: rc         — reference count (u64)
```

LLVM IR type: `%MvlArray = type { ptr, i64, i64, i64, i64 }`

### MvlMap — `{ ptr, usize len, usize cap, MvlRcHeader }` (open-addressing hash map)

```
offset 0:  slots  — pointer to (key_ptr, value_ptr) pairs
offset 8:  len    — number of live entries
offset 16: cap    — slot count (power of two)
offset 24: rc     — reference count (u64)
```

LLVM IR type: `%MvlMap = type { ptr, i64, i64, i64 }`

All collection values in LLVM IR are represented as `ptr` (a pointer to one of the above structs). This is a uniform representation: every `String`, `Array`, or `Map` local variable is an alloca'd pointer slot holding the address of a heap struct.

## C-ABI Functions (exported from `mvl_memory` cdylib)

```c
// Allocation / deallocation
MvlString* mvl_string_new(const char* bytes, size_t len);
MvlString* mvl_string_clone(MvlString* s);
void       mvl_string_drop(MvlString* s);
size_t     mvl_string_len(MvlString* s);
MvlString* mvl_string_concat(MvlString* a, MvlString* b);
int        mvl_string_eq(MvlString* a, MvlString* b);
const char* mvl_string_ptr(MvlString* s);   // for printf

MvlArray*  mvl_array_new(size_t elem_size, size_t initial_cap);
MvlArray*  mvl_array_clone(MvlArray* a);
void       mvl_array_drop(MvlArray* a);
void       mvl_array_push(MvlArray* a, const void* elem);
void*      mvl_array_get(MvlArray* a, size_t idx);
size_t     mvl_array_len(MvlArray* a);

MvlMap*    mvl_map_new(size_t initial_cap);
MvlMap*    mvl_map_clone(MvlMap* m);
void       mvl_map_drop(MvlMap* m);
void       mvl_map_insert(MvlMap* m, const void* key, size_t key_len, const void* val);
void*      mvl_map_get(MvlMap* m, const void* key, size_t key_len);
size_t     mvl_map_len(MvlMap* m);

// Allocator primitives
void*      mvl_alloc(size_t size);
void       mvl_free(void* ptr);
void       mvl_panic(const char* msg);   // prints msg to stderr, aborts
```

### Reference counting rules

- `_new` returns RC=1.
- `_clone` increments RC and returns the same pointer.
- `_drop` decrements RC; frees when RC reaches 0.
- Move semantics (L5-15, future): no `_clone` is emitted at the last use of a value — the caller transfers ownership and the callee calls `_drop`. Until L5-15 is implemented, call sites emit `_clone` defensively on every non-last use.

### Known limitation: cycle leaks

Reference counting cannot collect cycles (e.g. `Array[Array[T]]` with a self-referential element). This is accepted as a known limitation of Phase C. Cycle collection is deferred to Phase C+ or ownership-based drop (L5-15, ADR TBD).

## Integration with lli

### Build step

`mvl_memory` is a sibling Cargo workspace crate:

```toml
# mvl_memory/Cargo.toml
[package]
name = "mvl_memory"
crate-type = ["cdylib"]

[dependencies]
# none — no_std-compatible; only libc for malloc/free
libc = "0.2"
```

Cargo builds it as `target/debug/libmvl_memory.dylib` (macOS) or `libmvl_memory.so` (Linux). A `build.rs` in the main crate triggers a rebuild when `mvl_memory` sources change.

### lli invocation

```rust
// src/main.rs  (run_project_llvm and cmd_test_llvm)
let runtime_lib = find_mvl_memory_lib();
let status = process::Command::new(&lli)
    .arg(format!("--load={}", runtime_lib.display()))
    .arg(tmp.path())
    .status()?;
```

`find_mvl_memory_lib()` checks (in order):
1. `MVL_MEMORY_LIB` environment variable
2. `target/{debug,release}/libmvl_memory.{dylib,so}` relative to the compiler binary
3. A system path (for installed distributions)

### LLVM IR declarations

The LLVM backend declares each runtime function with `Linkage::External` — the same pattern used for `printf` and `rand`. No inline IR bodies are needed.

```llvm
declare ptr @mvl_string_new(ptr, i64)
declare ptr @mvl_string_drop(ptr)
declare i64 @mvl_string_len(ptr)
declare ptr @mvl_string_concat(ptr, ptr)
```

## Type mapping update (L5-13 prerequisite)

`mvl_type_to_llvm` must map collection types to `ptr` before L5-14 can emit runtime calls:

| MVL type | Phase B mapping | Phase C mapping |
|----------|----------------|-----------------|
| `String` | `ptr` (static global) | `ptr` (MvlString heap ptr) |
| `List[T]` / `Array[T]` | `i64` (stub) | `ptr` (MvlArray heap ptr) |
| `Map[K,V]` | `i64` (stub) | `ptr` (MvlMap heap ptr) |
| `Set[T]` | `i64` (stub) | `ptr` (MvlArray heap ptr, same layout) |

## Alternatives considered

| Approach | Rejected because |
|----------|-----------------|
| Inline LLVM IR bodies | malloc+free+pointer arithmetic in IR is verbose and hard to test in isolation |
| C runtime (`mvl_runtime.c`) | Rust gives memory safety for the runtime itself; catches bugs in the unsafe foundation early |
| Rust `staticlib` + `llvm-link` | Requires `llvm-link` tool and an extra link step; `cdylib` + `lli --load` is simpler |
| Reuse existing `mvl_runtime` crate | That crate is `#![forbid(unsafe_code)]` and designed for the Rust transpiler. Memory management requires `unsafe` and a different compilation model. Keep them separate. |

## Consequences

- `mvl_memory` is the single unsafe boundary in the MVL compiler pipeline. It must have its own unit test suite (Rust `#[test]` + Miri for UB detection).
- The `lli --load` flag adds a build artifact dependency. CI must build `mvl_memory` before running LLVM backend tests.
- String literals that were previously static globals (`build_global_string_ptr`) must be wrapped in `mvl_string_new` calls in Phase C. Phase B tests are unaffected (they use the transpiler backend).
- Cycle leaks are accepted and documented. Valgrind will show them in `valgrind --leak-check=full` output. The done-condition for Phase C accepts this.
