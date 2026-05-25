# ADR-0037: Runtime Abstraction Layer

**Status:** Accepted
**Date:** 2026-05-25
**Issues:** #1014, #1033

---

## Context

The MVL compiler has two backends that both hardcode actor runtime primitives:

- **Rust backend** (`emit_actors.rs`): Inlines `std::thread::spawn` + `std::sync::mpsc::sync_channel(256)` directly into generated Rust code.
- **LLVM backend** (`runtime/llvm/src/actors.rs`): Exposes C-ABI functions (`mvl_actor_spawn/send/drop`) but their implementation also uses `std::thread` + `mpsc`.

This works for host-platform server software but prevents:
- **Embedded targets** (ESP32): needs FreeRTOS tasks (`xTaskCreate` + `xQueueSend`)
- **High-concurrency servers**: needs async runtime (tokio/smol)
- **Bare metal**: needs cooperative scheduler (no OS)
- **WebAssembly**: needs single-threaded event loop

The compiler should not know which scheduling primitive backs an actor. It emits calls to a runtime interface; the linker decides what those calls mean.

## Decision

### Three independent axes

Backend, runtime, and target are orthogonal choices:

```
mvl build --backend=rust  --target=host        → mvl_runtime (std::thread)
mvl build --backend=rust  --target=esp32-s3    → mvl_runtime (freertos feature)
mvl build --backend=llvm  --target=host        → mvl_runtime_c (std::thread)
mvl build --backend=llvm  --target=wasm32      → mvl_runtime_c (bare feature)
```

- **Backend** = code generation strategy (Rust source vs LLVM IR) — unchanged from ADR-0027
- **Target** = platform triple — determines which runtime implementation is linked
- **Runtime** = actor scheduler + channel + supervision implementation — selected by target

### Runtime interface

The compiler emits calls to these functions. Each runtime provides concrete implementations.

#### Rust backend — module `mvl_runtime::actors`

```rust
pub fn mvl_actor_spawn<S, M>(
    state: S,
    dispatch: fn(&mut S, M),
) -> MvlActorHandle<M>;

pub fn mvl_actor_send<M>(handle: &MvlActorHandle<M>, msg: M);

pub fn mvl_register_actor(join: MvlJoinHandle);
pub fn mvl_join_actors();
```

The Rust backend remains generic over state `S` and message `M` types. `MvlActorHandle<M>` wraps whatever send mechanism the runtime uses (channel sender, async sender, queue handle). `MvlJoinHandle` wraps the join mechanism (thread handle, task handle, etc.).

#### LLVM backend — C-ABI (unchanged)

```c
void *mvl_actor_spawn(dispatch_fn, state_ptr, state_size);
void  mvl_actor_send(handle, disc, argc, args);
void *mvl_actor_self();
void  mvl_actor_drop(handle);
```

The LLVM backend already calls these C-ABI functions. The abstraction is which `.dylib`/`.so` is loaded at link time.

#### Future: channels, supervision, scheduling (Stage 2)

```rust
// Channels (Phase 9)
mvl_channel_create(capacity) -> (Sender, Receiver)
mvl_channel_send(sender, message)
mvl_channel_recv(receiver, timeout) -> Option<Message>

// Supervision (Phase 9)
mvl_supervisor_register(child, strategy)
mvl_watchdog_feed()

// Scheduling (Phase 9)
mvl_timeout_set(duration)
mvl_sleep(duration)
mvl_yield()
```

These are not implemented in Stage 1. The interface is documented here for completeness.

### Default mapping

| Target | Rust backend runtime | LLVM backend runtime |
|--------|---------------------|---------------------|
| `host` (default) | `mvl_runtime::actors` (std::thread + mpsc) | `libmvl_runtime_c` (std::thread + mpsc) |
| `esp32-s3` (Stage 2) | `mvl_runtime` with `freertos` feature | `libmvl_runtime_freertos` |
| `wasm32` (Stage 2) | `mvl_runtime` with `bare` feature | `libmvl_runtime_bare` |
| `tokio` (Stage 2) | `mvl_runtime` with `tokio` feature | N/A (LLVM backend doesn't use Rust async) |

### Selection mechanism

1. `src/cli/args.rs` parses `--target=<triple>` (default: `host`)
2. Target is passed through the build pipeline to the backend
3. **Rust backend**: target selects Cargo feature flags on the `mvl_runtime` crate
4. **LLVM backend**: target selects which `libmvl_runtime_*.{dylib,so}` to load

### What changes in emit_actors.rs

Before (hardcoded):
```rust
cg.line("let (tx, rx) = std::sync::mpsc::sync_channel(256);");
cg.line("let __handle = std::thread::spawn(move || {");
```

After (runtime calls):
```rust
cg.line("let (__handle, __tx, __rx) = mvl_runtime::actors::mvl_actor_spawn_raw(256);");
cg.line("mvl_runtime::actors::mvl_spawn_loop(__rx, state, |actor, msg| {");
```

The generated code calls `mvl_runtime::actors::*` functions instead of directly using `std::thread` and `std::sync::mpsc`. The runtime module provides the concrete implementation based on compile-time feature selection.

## Consequences

- **Stage 1 is behavior-preserving.** The default `host` target produces identical output to today. All existing tests pass.
- **Adding a new target runtime:** Implement the `mvl_runtime::actors` functions behind a feature flag (Rust) or provide a new `libmvl_runtime_*` shared library (LLVM). No compiler changes needed.
- **No language changes.** Actor syntax, capability checking, and effect system are unaffected. This is purely compiler/runtime infrastructure.
- **LLVM backend needs minimal changes.** It already emits C-ABI calls — only the library selection logic changes.

## References

- ADR-0027: Multi-Backend Architecture
- ADR-0029: Pony Reference Capability Adaptation
- Spec 015: Actor Model (Phase 8)
- #1014: Epic — Runtime Abstraction Layer
- #751: Actor runtime optimization
- #854: Actor model design
