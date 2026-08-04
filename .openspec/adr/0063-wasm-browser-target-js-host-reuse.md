# ADR-0063: `wasm-browser` target — maximize host reuse, minimize bespoke bridge code

**Status:** Accepted
**Date:** 2026-08-04
**Issues:** #2093 (Phase 2)

---

## Context

#2093 Phase 1 wired `std.env`/`std.io`/`std.time`/`std.random` to the
`--backend=wasm` emitter against `runtime/wasm/src/lib.rs`, a `cdylib`
compiled to `wasm32-wasip1` and loaded by `wasmtime --preload
runtime=<path>`. Phase 2 is the `--target=wasm-browser` flag itself
(already stubbed in #2098, rejecting with "not yet implemented") —
deploying an MVL program somewhere with no WASI host at all: a browser tab,
or any `wasm32-unknown-unknown` consumer.

The explicit direction for this phase, from the person who owns the
project: do as little bespoke bridge work as possible and lean on
whatever the host (a browser, or `wasm-bindgen`-adjacent tooling) already
offers, rather than inventing new protocol. Two implementation paths were
evaluated against that goal before writing any browser-side code.

### Path 1 (rejected): `wasm-bindgen` + `js-sys`/`web-sys`

The obvious "reuse the host" move is `wasm-bindgen` — it's the standard
Rust↔JS bridge, and `js-sys`/`web-sys` are the standard bindings to
`Math.random`/`Date.now`/`console.log`. A minimal spike (two functions,
`js_sys::Math::random()` + `js_sys::Date::now()`, compiled to
`wasm32-unknown-unknown`) showed why this doesn't fit here:

- The compiled module imports from `__wbindgen_placeholder__` and
  `__wbindgen_externref_xform__` — placeholders `wasm-bindgen-cli`'s own
  code-generation pass resolves into real JS glue plus an externref table.
  A plain `cargo build` artifact isn't instantiable by
  `WebAssembly.instantiate` on its own.
- It exports ~150 `__wbindgen_describe_*` shims for the *entire* `js-sys`
  surface pulled in transitively, for two functions actually used.
- `runtime/wasm`'s existing preload architecture (one plain cdylib,
  wasmtime resolves its exports against the user module's imports) has no
  slot for a paired `.wasm` + generated `.js` artifact with its own ABI.

`wasm-bindgen` is "the host's tooling" in the sense that it's widely used,
but adopting it here would mean building a second, incompatible bridge
mechanism alongside the one `runtime/wasm` already has — the opposite of
minimal.

### Path 2 (accepted): cfg-gate the three real OS touchpoints

`runtime/wasm/src/lib.rs` is ~3,900 lines, but only three functions
actually touch the OS: `SystemTime::now()` (twice — `_mvl_time_now` and
the random-seed path) and `std::thread::sleep()` (once). Everything
else — every `_mvl_array_*`/`_mvl_option_*`/`_mvl_string_*`/`_mvl_result_*`
helper, and the `xorshift64` PRNG itself — is pure heap/computation with
zero OS dependency, and already compiles cleanly to `wasm32-unknown-unknown`
unmodified.

`wasm32-unknown-unknown` is Rust's own "the host provides everything, I
provide nothing" contract: an `extern "C"` function with no body is
compiled as a wasm *import*, resolved by whatever instantiates the
module — no macro, no code generation, no paired artifact. That is a
smaller, more literal version of "take what the host already offers" than
`wasm-bindgen`'s.

## Decision

**`runtime/wasm` is one crate, compiled to two targets.** No new Rust
crate, no duplicated array/option/string logic.

1. **`wasm32-wasip1`** (unchanged): OS-touching functions use
   `std::time`/`std::thread`/`std::env`/`std::fs` exactly as before.
2. **`wasm32-unknown-unknown`** (new, `make build-runtime-wasm-browser`):
   - `_mvl_time_now`/`ensure_seeded` (the random PRNG's seed source) read
     wall-clock time through a shared `epoch_nanos_now()` helper,
     `#[cfg]`-gated to either `SystemTime::now()` (wasi) or a one-line
     `extern "C" { fn _mvl_js_now_ms() -> f64; }` import (browser),
     resolved by `runtime/wasm-browser/runtime.mjs`'s `Date.now()` shim.
   - `_mvl_time_thread_sleep` is a documented no-op under the browser
     target — a real blocking sleep needs `SharedArrayBuffer` +
     `Atomics.wait`, which only works cross-origin-isolated; genuinely out
     of scope for "as little as possible."
   - `std.env`/`std.io` (17 functions) get a `#[cfg(not(target_os =
     "wasi"))]` sibling body each, returning the same `Err`/`None`/empty
     shape their WASI counterpart would return on failure — **not**
     omitted. See "the fixed import table" below for why omission doesn't
     work.
   - `.cargo/config.toml` gains `[target.wasm32-unknown-unknown]
     rustflags = ["-C", "link-args=--import-undefined"]` so wasm-ld treats
     the one genuinely undefined symbol (`_mvl_js_now_ms`) as an import
     instead of a link error — the same thing `wasm-bindgen-cli` would
     otherwise be doing, done with a linker flag instead of a
     code-generation pass.

**The one thing that must be hand-written in JS**: `runtime/wasm-browser/
runtime.mjs`. Not because the `runtime` module needs it — `_mvl_js_now_ms`
is its only host dependency — but because **every** MVL WASM module
unconditionally imports `wasi_snapshot_preview1.fd_write`/`clock_time_get`
at the emitter level (`wasm_text.rs`'s WASI runtime blob backs `println`
and the allocator's heap-init read), regardless of whether the program
uses `std.random`/`std.time` at all. A browser has no WASI host, so
*something* has to answer those two imports. `runtime.mjs` supplies them —
`fd_write` decodes a WASI iovec and calls `console.log`/`console.error`,
`clock_time_get` writes `Date.now()` widened to nanoseconds — using
nothing but what a JS engine already provides. It also exposes
`instantiateMvlProgram(runtimeBytes, programBytes)`, which passes the
runtime module's own `.exports` object directly as the user module's
`"runtime"` import namespace — no per-function re-listing needed on the JS
side at all.

### The fixed import table (a constraint discovered mid-implementation)

The emitter's `(import "runtime" ...)` declarations are a **fixed table
covering every stdlib builtin**, independent of what a given program
actually calls (confirmed against `wasm_text.rs`'s import-signature
table). A program with zero `std.env`/`std.io` usage still imports
`_mvl_env_args`, `_mvl_io_write_file`, etc. — and WASM requires *every*
declared import to resolve for `WebAssembly.instantiate` to succeed at
all, whether or not it's ever called. The first design considered here
(`std.env`/`std.io` simply absent — not compiled — on the browser target,
so a program using them fails to link with a clear error) was wrong: it
would fail to link *every* program, including ones that never touch
`std.env`/`std.io`. Verified by diffing `wasm-tools print`'s export list
between the two target builds: 111 exports each, identical names.

## Consequences

- `runtime/wasm/src/lib.rs`'s top-of-file doc comment now states both
  targets explicitly; each cfg-gated function pair sits next to its
  sibling rather than in a separate file, so a future reader sees both
  implementations of "how does this touch the outside world" together.
- `mvl build --backend=wasm --target=wasm-browser` now succeeds (was: hard
  error). The emitted WAT is byte-for-byte the same as the default/`wasi`
  target — the target only changes which runtime module and JS shim get
  linked in at instantiation time, entirely outside the compiler. `mvl
  test --backend=wasm --target=wasm-browser` still rejects: `cmd_test_wasm`
  is a wasmtime-based harness with no browser/JS host to run against; that
  remains a follow-up, verified instead by `make test-runtime-wasm-browser`
  (a Node-based end-to-end smoke test — Node's `WebAssembly` is the same
  implementation a browser uses, so this is a faithful stand-in without
  needing an actual browser).
- A program calling `std.env`/`std.io` under `wasm-browser` gets an
  ordinary `Err`/`None`/empty result at the MVL level, not an instantiation
  failure or a trap — consistent with every other capability gap in this
  codebase (#2014's stub-function philosophy: fail at the call site with a
  typed result, not silently or catastrophically).
- No new Cargo dependency. No generated JS glue file to keep in sync with
  a Rust build. `runtime/wasm-browser/runtime.mjs` is the entire
  browser-side surface, and it never has to change unless the WASI runtime
  blob's import list itself changes.

## Rejected Alternatives

1. **`wasm-bindgen` + `js-sys`/`web-sys`** — see Context. Wrong shape for
   `runtime/wasm`'s existing preload-based composition; large unused
   surface area from one dependency.
2. **A separate `runtime/wasm-browser/` Rust crate reimplementing
   `std.random`/`std.time` from scratch** — would duplicate the array/
   option/string heap layout `runtime/wasm/src/lib.rs` already implements
   and tests, with no shared source of truth. The cfg-gate approach gets
   the same two targets from one crate instead.
3. **Hand-written JS reimplementing the full `runtime` namespace** (array/
   option construction, Fisher-Yates shuffle over refcounted heap
   structures) — considered and rejected once the actual OS-touching
   surface turned out to be 3 functions, not ~100. Reimplementing
   undocumented Rust struct layouts in JS would have been *more* bespoke
   bridge code, not less.
4. **Compiling `std.env`/`std.io` out entirely under the browser target**
   — the initial plan, invalidated by the fixed-import-table discovery
   above: it breaks every program, not just ones using those modules.

## Relation to language definition

No MVL-visible syntax or semantics change. `std.env`/`std.io`'s documented
effects (`Env`, `FileRead`, `FileWrite`, `FileDelete`) are unchanged; a
program using them under `wasm-browser` still type-checks identically to
every other target, and only differs in the runtime `Err`/`None` it gets
back — the same "signature is the threat model" contract already governs
what the effect declares, not what a given deployment target can actually
satisfy.
