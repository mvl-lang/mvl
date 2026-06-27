# Spike 006 — WASM Backend (hand-translation)

A first end-to-end validation of the WASM backend story from the [#1571 epic][epic].
Two small MVL programs are translated **by hand** to WebAssembly Text (WAT),
assembled with `wasm-tools`, and executed with `wasmtime`.

Goal: prove the path works and surface the concrete questions a real
`TIR → WASM` emitter would have to answer. No code is generated yet.

## Variants

| File | What it shows | Run shape |
|------|---------------|-----------|
| `add.{mvl,wat}`   | Pure compute. Two exported functions (`main`, `add`). No host imports. | `wasmtime run --invoke <fn> add.wasm [args…]` |
| `hello.{mvl,wat}` | Host imports via WASI preview 1 (`fd_write`). Models the eventual `extern "wasm"` ABI from the epic. | `wasmtime run hello.wasm` |

## Running

```bash
# from repo root
make -C tests/spikes/006-wasm-backend test

# individual targets
make -C tests/spikes/006-wasm-backend check   # type-check the MVL sources
make -C tests/spikes/006-wasm-backend add     # pure compute variant
make -C tests/spikes/006-wasm-backend hello   # WASI variant
```

Prerequisites: `wasm-tools` (`cargo install wasm-tools`) and `wasmtime`.

## Expected output

```
add.wasm     main()      → 5
add.wasm     add(7, 35)  → 42
hello.wasm   _start      → prints "5\n" to stdout
```

## What the spike answers

### Pure compute (`add`)

- **`Int` → `i64`**.  Matches `runtime/llvm/src/lib.rs` (e.g. `_mvl_int_pow`
  takes `i64`).
- **Trailing-expression return** in MVL maps cleanly to "leave the value on
  the WASM stack at end of function".  No explicit `return` opcode needed.
- **Function calls**: arguments pushed in order, `call $name` pops them and
  pushes the result. The MVL `add(2, 3)` is literally three instructions:
  `i64.const 2; i64.const 3; call $add`.
- **Host invocation works** via `wasmtime --invoke`, which is convenient for
  cross-backend test parity (Rust ≡ LLVM ≡ WASM) — the same shape we already
  use for the LLVM backend tests.

### Host imports (`hello`)

- **WASI preview 1 is the lowest-friction host import**: a single `import`
  declaration, an exported `_start`, and an exported `memory`. Wasmtime
  links it automatically (`wasi_snapshot_preview1`).
- **`extern "wasm" "<module>" { ... }`** from the epic maps directly to WAT
  `(import "<module>" "<name>" (func ...))`. The module/name pair is the
  ABI surface.
- **Linear memory is mandatory** for any host import that takes string or
  buffer arguments — `fd_write` reads `iovec`s from guest memory. The
  emitter must allocate a memory and export it.

## What the spike deliberately *does not* do

- **No `i64 → String`**. The `hello.wat` variant hard-codes `"5\n"`. A real
  emitter needs `mvl_int_to_string` (already in `runtime/llvm/`) ported to
  WASM, plus a small bump allocator or `wee_alloc`-style runtime.
- **No `MvlString` layout**. `runtime/llvm/src/memory.rs` defines the LLVM
  layout (`{ptr, len, cap, rc}`). WASM needs the same fields but in linear
  memory — the ADR call-out in the epic.
- **No drop / refcount emission**. The compute path has no allocations so
  this didn't come up. It will the moment a `String` or `List[T]` shows up
  in a function body.
- **No effects-to-imports mapping**. `hello.mvl` declares `! Console`, but
  the WAT manually picks `wasi_snapshot_preview1/fd_write`. The emitter
  needs a table: `Console → wasi:cli/stdout`, `Net → wasi:sockets`, etc.
- **No component model**. We target the older `wasi_snapshot_preview1`
  ABI, not WASI 0.2 components. The ADR in the epic should decide whether
  the emitter targets preview1, preview2/component, or both.

## Open questions surfaced

1. **Calling convention for non-scalar returns.** WASM multi-value returns
   exist, but the conventional approach (also used by Rust's WASM target)
   is to pass a pointer to caller-allocated space as a hidden first
   parameter. The emitter needs to pick one.
2. **Drop emission across WASM call boundaries.** Linear memory has no
   GC; the runtime must refcount. The LLVM backend already does this
   (`mvl_string_drop` etc.) — the WASM port can reuse the algorithm but
   needs to decide where the refcount field lives (currently after the
   data in `MvlString`).
3. **Actor scheduling on a single-threaded target.** Browser WASM has no
   threads; `wasi:io/poll` (WASI 0.2) is the cooperative-scheduling story.
   Out of scope here but worth flagging.
4. **`--invoke` is marked experimental** by wasmtime. Cross-backend tests
   probably want a tiny generated `_start` wrapper that calls the function
   and prints its result — more portable than relying on `--invoke`.

## Where the real emitter lives (per the epic)

> "WASM emitter source lives in `compiler/backends/wasm/` (MVL source),
>  not Rust — assumes Phase A of #1113."

This spike stays in Rust-adjacent territory (just `.wat` files) to unblock
the design conversation without committing to either an MVL-side or
Rust-side implementation. The text-WAT approach mirrors the
text-LLVM-IR approach from #1111.

[epic]: https://github.com/mvl-lang/mvl/issues/1571
