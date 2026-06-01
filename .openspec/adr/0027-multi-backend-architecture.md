# ADR-0027: Multi-Backend Architecture

**Status:** Accepted
**Date:** 2026-05-11 (updated 2026-05-30 — actor runtime interface, #1014)
**Issue:** #646 (first step of epic #615), #1014 (actor runtime abstraction)

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

### Actor runtime interface

The emitter (compiler) and the runtime crate are separated by a **named interface**. The emitter
MUST NOT reference `std::thread`, `tokio`, `std::sync::mpsc`, or any other scheduler primitive
directly. It emits calls to named symbols; the linked runtime crate provides the implementation.
Swapping `--target` changes the runtime crate — the emitter is unchanged.

#### LLVM backend (C-ABI)

The LLVM emitter declares and calls these external C symbols:

```c
// Actor lifecycle
void* mvl_actor_spawn(void* dispatch_fn, void* state, int64_t size,
                      int64_t capacity, int64_t policy);
void  mvl_actor_send(void* handle, int64_t disc, int64_t argc, int64_t* args);
void  mvl_actor_drop(void* handle);
void* mvl_actor_self();
void  mvl_actor_join_all();
```

`capacity`: `> 0` = bounded mailbox of that size; `<= 0` = unbounded.
`policy`: `0` = DropNewest (`try_send`); `1` = Block (sender blocks when full).

These are implemented in `runtime/llvm/src/actors.rs` and compiled into `libmvl_runtime_c`.

#### Rust backend (typed interface)

The Rust emitter generates code that calls only these symbols from `mvl_runtime::actors`:

```rust
// Opaque wrappers — implementation is runtime-internal
pub struct MvlSender<M: Send + 'static>  // actor handle field type
pub struct MvlReceiver<M: Send + 'static>
pub struct MvlJoinHandle

// Called from _start_<actor> functions:
pub fn mvl_channel<M: Send + 'static>(capacity: i64, policy: i64)
    -> (MvlSender<M>, MvlReceiver<M>);
pub fn mvl_actor_run<S, M>(rx: MvlReceiver<M>, state: S, dispatch: fn(&mut S, M))
    -> MvlJoinHandle
    where S: Send + 'static, M: Send + 'static;
pub fn mvl_register_actor(h: MvlJoinHandle);

// Called at end of fn main():
pub fn mvl_join_actors();

// Called from behavior dispatch wrappers:
impl<M: Send + 'static> MvlSender<M> {
    pub fn send(&self, msg: M);  // respects policy (drop or block)
    pub fn clone(&self) -> Self;
}

// Runtime-internal (not called from generated code):
// mvl_spawn — used internally by mvl_actor_run
// MvlReceiver::recv — used internally by mvl_actor_run
```

The actor handle struct field type is always `mvl_runtime::MvlSender<XMailbox>` — never
`std::sync::mpsc::SyncSender` or any other concrete type.

#### --target selects the runtime

`mvl build --target=<name>` selects which runtime crate is linked. The emitter output is
identical for all targets.

| Target | Rust runtime | LLVM runtime |
|--------|-------------|--------------|
| `default` | `runtime/rust` — `std::thread` + `mpsc` | `runtime/llvm` — `std::thread` + `mpsc` |
| `tokio` | `runtime/rust-tokio` — tokio tasks | (same LLVM runtime) |
| `freertos` | `runtime/rust-freertos` | `runtime/llvm-freertos` |

`--target` is not yet implemented (Phase 9). The `default` runtime is always used until then.
The emitter interface defined above MUST be stable before `--target` is introduced.

## Consequences

- **Adding backends:** Create `src/mvl/backends/<name>/`, implement `Backend`, wire up in `parse_backend()`.
- **Calling convention:** `libmvl_runtime_c.{dylib,so}` filename is unchanged; CI and tooling continue to work without changes.
- **No functional change:** All existing tests pass; `--backend=rust` and `--backend=llvm` produce identical output.
- **Import paths:** `mvl::mvl::transpiler` → `mvl::mvl::backends::rust`, `mvl::mvl::codegen` → `mvl::mvl::backends::llvm`.

## Relation to language definition

This ADR defines the compiler infrastructure architecture and does not affect the MVL language semantics. It organizes existing functionality (Rust transpilation and LLVM code generation) under a unified `Backend` trait to enable future extensions (WebAssembly, C, Python, etc.) without language changes.

The Backend trait is internal compiler plumbing — users see no difference: `mvl build --backend=rust` and `mvl build --backend=llvm` continue to work identically.

## References

- ADR-0003: Compilation strategy
- ADR-0016: LLVM memory layout
- ADR-0019: Two-path stdlib architecture
