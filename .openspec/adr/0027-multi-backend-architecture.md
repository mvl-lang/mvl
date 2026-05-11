# ADR-0027: Multi-Backend Architecture

**Status:** Accepted  
**Date:** 2026-05-11  
**Issue:** #646 (first step of epic #615)

## Context

The MVL compiler had two code-generation paths implemented as top-level siblings:

- `src/mvl/transpiler/` — Rust source emission (~7,500 lines)
- `src/mvl/codegen/` — LLVM IR emission via inkwell (~9,000 lines)

Supporting additional backends (WebAssembly, C, Python, etc.) would require adding more top-level modules, making the boundary between "compiler frontend" and "backend" invisible in the module tree.  The runtime crates also lacked a clear grouping: `mvl_runtime`, `mvl_memory`, and `mvl_runtime_c` were three separate workspace roots with no organisational relationship expressed in the directory layout.

## Decision

### Module reorganisation

Move backend code under a shared `backends/` namespace:

```
src/mvl/backends/
├── mod.rs         # Backend trait
├── rust/          # Rust source emission (was transpiler/)
└── llvm/          # LLVM IR emission (was codegen/)
```

### Backend trait

A minimal `Backend` trait in `src/mvl/backends/mod.rs` provides the common interface:

```rust
pub trait Backend {
    fn name(&self) -> &'static str;
    fn file_extension(&self) -> &'static str;
    fn emit_program(&self, prog: &Program, crate_name: &str) -> String;
}
```

Specialised functionality (coverage, MC/DC, mutation) remains on the concrete Rust backend type and is called directly from `src/main.rs`.

### Runtime consolidation

Move runtime crates under a shared `runtime/` workspace directory:

```
runtime/
├── rust/          # Rust stdlib (was mvl_runtime/)
└── llvm/          # C-ABI stdlib (merged from mvl_runtime_c/ + mvl_memory/)
```

`mvl_memory` (heap types + lifecycle) is merged into `runtime/llvm/` as the `memory` module.  The merged crate keeps the package name `mvl_runtime_c` so the `libmvl_runtime_c` binary name is unchanged.  lli now loads a single library instead of two.

## Consequences

- **Adding backends:** Create `src/mvl/backends/<name>/`, implement `Backend`, wire up in `parse_backend()`.
- **Calling convention:** `libmvl_runtime_c.{dylib,so}` filename is unchanged; CI and tooling continue to work without changes.
- **No functional change:** All existing tests pass; `--backend=rust` and `--backend=llvm` produce identical output.
- **Import paths:** `mvl::mvl::transpiler` → `mvl::mvl::backends::rust`, `mvl::mvl::codegen` → `mvl::mvl::backends::llvm`.

## References

- ADR-0003: Compilation strategy
- ADR-0016: LLVM memory layout
- ADR-0019: Two-path stdlib architecture
