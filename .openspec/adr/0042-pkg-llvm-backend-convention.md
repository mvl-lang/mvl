# ADR-0042: Per-Package LLVM Backend Convention — `llvm.rs` + `extern "c"` ABI

**Status:** Accepted
**Date:** 2026-06-04
**Context:** Packages using `extern "rust"` in their `ffi.mvl` only work on the Rust backend. The LLVM backend silently produces no-op stubs for unrecognised extern functions, giving package authors no path to LLVM support. Discovered while designing `pkg.sqlite` (#785). See issue #811.

---

## Decision

### Option A: Per-Package `llvm.rs` (Selected)

Each package that needs LLVM support ships an `llvm.rs` alongside its `bridge.rs`:

```
pkg/sqlite/
├── bridge.rs           <- Rust backend  (extern "Rust" ABI)
├── llvm.rs             <- LLVM backend  (extern "C" ABI, #[no_mangle])
├── mvl.toml
└── src/
    └── internal/
        └── ffi.mvl     <- extern "c" declarations for LLVM path
```

### Why This Option

| Criterion | Option A (selected) | Option B (unified C-ABI) | Option C (registry) |
|-----------|-------|-------|-------|
| Consistent with ADR-0006 | Yes — extends bridge.rs pattern | Partially — changes Rust ABI | No — new mechanism |
| Consistent with ADR-0019 | Yes — separate Rust/LLVM runtimes | No — unifies | Partially |
| Build system complexity | Medium — compile + load | Low — reuse bridge.rs | High — compiler changes |
| Rust backend overhead | None | C-ABI overhead | None |
| Package author effort | Two files | One file | None |

Option A preserves the zero-cost Rust-ABI path for the Rust backend (ADR-0006) while adding an opt-in LLVM path using the same convention as the stdlib C runtime (ADR-0019).

### Rejected Alternatives

**Option B (Single `extern "c"`):** Would change `bridge.rs` from `extern "Rust"` to `extern "C"`, adding ABI conversion overhead on the Rust path. Both backends would link the same library — simpler but slower.

**Option C (Compiler-driven registry):** Would extend `mvl.toml` with symbol mappings so the compiler auto-generates dispatch. Elegant but requires significant compiler architecture changes for marginal benefit.

---

## Convention

### `llvm.rs` Format

```rust
// pkg/sqlite/llvm.rs — LLVM backend implementations
// Compiled to cdylib, loaded via lli --load=

#[no_mangle]
pub extern "C" fn sqlite_open(path: *const u8, path_len: usize) -> i64 {
    // C-ABI implementation
    todo!()
}

#[no_mangle]
pub extern "C" fn sqlite_query(
    db: i64,
    sql: *const u8,
    sql_len: usize,
) -> i64 {
    todo!()
}
```

### `ffi.mvl` Dual-ABI Declarations

The package's `ffi.mvl` uses `extern "c"` for functions that the LLVM backend needs:

```mvl
// Rust backend path — used when --backend=rust (default)
extern "rust" {
    fn sqlite_open(path: String) -> Result[Db, SqliteError]
    fn sqlite_query(db: Db, sql: String) -> Result[List[Row], SqliteError]
}

// LLVM backend path — used when --backend=llvm
extern "c" {
    fn sqlite_open(path: String) -> Result[Db, SqliteError]
    fn sqlite_query(db: Db, sql: String) -> Result[List[Row], SqliteError]
}
```

The compiler selects the appropriate block based on the active backend:
- Rust backend: processes `extern "rust"`, ignores `extern "c"`
- LLVM backend: processes `extern "c"`, ignores `extern "rust"`

### Build Flow

```
mvl run --backend=llvm app.mvl
  │
  ├─ Discover pkg/sqlite/llvm.rs
  ├─ Compile: rustc --crate-type=cdylib llvm.rs -o libpkg_llvm_bridge.{dylib,so}
  ├─ Emit LLVM IR with `declare` for extern "c" functions
  └─ Execute: lli --load=libmvl_runtime_c.so --load=libpkg_llvm_bridge.so app.ll
```

### Discovery Rules

1. `find_pkg_llvm_bridge()` scans `use pkg.*` imports
2. Looks for `llvm.rs` in the resolved package directory
3. If found: compile and load alongside `libmvl_runtime_c`
4. If absent: LLVM backend proceeds without — `extern "c"` calls fail at runtime with unresolved symbols (clear error)

---

## Implementation

### Compiler Changes

1. **LLVM emitter** (`src/mvl/backends/llvm_text/emitter.rs`):
   - Handle `Decl::Extern` for `extern "c"` ABI
   - Emit `declare` instructions for each extern C function
   - Register return/param types for call emission

2. **Loader** (`src/mvl/loader.rs`):
   - `find_pkg_llvm_bridge()`: discover `llvm.rs` from pkg directories

3. **CLI** (`src/cli/llvm_text.rs`):
   - `compile_llvm_bridge()`: compile `llvm.rs` → cdylib
   - `run_project_llvm_text()`: load compiled library via `lli --load=`

### File Changes

| File | Change |
|------|--------|
| `src/mvl/backends/llvm_text/emitter.rs` | `Decl::Extern` handling for `extern "c"` |
| `src/mvl/loader.rs` | `find_pkg_llvm_bridge()` |
| `src/cli/llvm_text.rs` | `compile_llvm_bridge()`, `--load` integration |

---

## Current Limitations

| Limitation | Mitigation | Future Path |
|------------|------------|-------------|
| `llvm.rs` cannot depend on external crates | Document in pkg authoring guide | Build via Cargo instead of bare rustc |
| Dual `extern "rust"` / `extern "c"` blocks in ffi.mvl | Both are valid MVL | Compiler flag to select ABI at parse time |
| No signature validation across bridge.rs ↔ llvm.rs | Type mismatches caught at link time | `mvl assurance` cross-check |
| `pkg.tui` has no LLVM path | Stays Rust-only (documented) | Add `llvm.rs` if LLVM support needed |

## `pkg.tui` Status

`pkg.tui` is a Rust-only package (no `llvm.rs`). This is documented and intentional — terminal UI libraries rely on Rust ecosystem crates (crossterm, ratatui) that have no C-ABI equivalent. Programs using `pkg.tui` cannot run on `--backend=llvm`.

---

## Consequences

- Package authors can opt into LLVM support by adding `llvm.rs`
- Rust backend performance is unchanged (zero-cost `extern "Rust"` ABI preserved)
- LLVM backend can resolve package extern functions via `--load`
- Build system discovers and compiles `llvm.rs` automatically
- The trust boundary remains explicit: `extern "c"` in `ffi.mvl` + `llvm.rs` is greppable and auditable

---

## Related

- ADR-0006 — FFI via `extern "rust"` and the `bridge.rs` convention
- ADR-0012 — Extended package model
- ADR-0019 — LLVM stdlib two-path architecture
- #785 — `pkg.sqlite` (first package to use this convention)
- #811 — This issue
