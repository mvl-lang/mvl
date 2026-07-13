# Spike 006 — WASM Backend

End-to-end validation of the WASM backend story from the [#1571 epic][epic].
Two small MVL programs go through the real emitter (`mvl build --backend=wasm`),
are assembled with `wasm-tools`, and executed with `wasmtime`.

Hand-written `*_reference.wat` files are checked in alongside the sources as
a spec of the target shape the emitter should produce — useful for
bootstrapping and as documentation, but no longer the primary test path.

## Variants

| Files | What it shows | Run shape |
|-------|---------------|-----------|
| `add.mvl` + `add_reference.wat`   | Pure compute. Two exported functions (`main`, `add`). No host imports. | `wasmtime run --invoke <fn> add.wasm [args…]` |
| `hello.mvl` + `hello_reference.wat` | Host imports via WASI preview 1 (`fd_write`). Real `Int→String` + `println → fd_write` lowering. | `wasmtime run hello.wasm` |

## Running

```bash
# The whole thing: fresh debug mvl → emit → assemble → run, for both programs.
make -C tests/spikes/006-wasm-backend test

# Individual emit-path targets
make -C tests/spikes/006-wasm-backend check   # type-check the MVL sources
make -C tests/spikes/006-wasm-backend add     # emit + run add.mvl
make -C tests/spikes/006-wasm-backend hello   # emit + run hello.mvl

# Reference pipeline against hand-written *_reference.wat
make -C tests/spikes/006-wasm-backend test-reference
```

`make test` treats `cargo build` (debug) as a prerequisite, so the emitter
under test is always current with source.

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

## Reference WAT vs. emitter output — where they differ

The hand-written `*_reference.wat` files are a *minimal* target shape. The
emitter's output for the same program is intentionally *more complete* in
one place:

- `hello_reference.wat` hard-codes the literal bytes `"5\n"` — it does not
  actually compute `add(2, 3).to_string()`. It's a shape spec, not a real
  lowering.
- `hello.wat` (emitted by `mvl build --backend=wasm`) does the real work: a
  bump allocator, an inline `i64 → decimal ASCII` helper (`$mvl_int_to_string`),
  and a `println → fd_write` lowering with a two-entry iovec (string + newline).

Both produce the same stdout (`5\n`), by design.

## What the spike deliberately *does not* do

- **No `MvlString` layout**. `runtime/llvm/src/memory.rs` defines the LLVM
  layout (`{ptr, len, cap, rc}`). WASM needs the same fields but in linear
  memory — the ADR call-out in the epic. Today the emitter passes strings
  as bare `(ptr, len)` pairs on the WASM stack, which works only because
  nothing is ever dropped.
- **No drop / refcount emission**. The bump allocator never frees. Fine for
  a "print one line and exit" program; broken for anything longer-running.
- **No effects-to-imports table**. The emitter has a single hard-coded
  mapping: `Console → wasi_snapshot_preview1/fd_write`. `Net`, `FileRead`,
  `Log`, etc. all fall through today.
- **No string literals in source**. `println("hello")` would fail because
  the emitter has no `String::Literal → data section` path yet — only
  `Int.to_string()` output is supported.
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
