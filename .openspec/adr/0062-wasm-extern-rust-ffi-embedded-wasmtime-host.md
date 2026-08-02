# ADR-0062: WASM `extern "rust"` FFI via an embedded-wasmtime host

**Status:** Accepted
**Date:** 2026-08-02
**Issues:** #2049

---

## Context

ADR-0006 established `extern "rust"` + a sibling `bridge.rs` as MVL's FFI
trust boundary, specifically justified as *Rust-to-Rust* FFI — the
generated code and the bridge share one Rust ABI, one binary, one compiler
pass. That justification is exactly why `--backend=wasm` (and, before it,
LLVM) couldn't just reuse the mechanism as-is: neither backend puts a Rust
compiler in the loop at emission time, so "same Rust ABI" isn't available
to lean on. This ADR is about extending ADR-0006's contract — same
`bridge.rs`, same signatures, same fail-fast-if-missing discovery — to a
backend that has no Rust compiler of its own, by giving it one at run time
instead of at emission time.

`--backend=wasm` never read `TirProgram::externs` at all: a call to an
`extern "rust"` function emitted a bare `call $foo` with no `(import ...)`
declaration, so `wasm-tools parse` rejected the module with "unknown func"
(#2049). Every example with an extern trust boundary — `config_server`
foremost among them — had no path to `--backend=wasm` at all.

The LLVM backend hit the identical wall earlier and chose a different
answer: `compute_extern_rust_exclusion_set` in
`llvm_text/emit_program_tir.rs` walks the call graph and *excludes* any
function that transitively calls an extern-rust fn from LLVM emission
entirely. That is the right choice for LLVM — there is no Rust compiler in
that emission path, so genuinely linking against a `bridge.rs` written in
idiomatic Rust (`Result<Config, ConfigError>`, `Secret<String>`, ordinary
struct literals) is not on the table. The same is true for `wasm_text.rs`:
it hand-emits WAT text directly, with no Rust compiler anywhere in the
pipeline.

Two paths were open:

1. Mirror LLVM's exclusion approach — turn the silent invalid-module bug
   into a clear, non-silent diagnostic (a real fix for the *bug*, but not
   for the *capability gap*).
2. Make `extern "rust"` genuinely runnable under `--backend=wasm` — real
   FFI, not exclusion.

This ADR is about (2), studied against `~/wc/lab271/wasm/experiments/
016_ffi_assemblyscript`: an AssemblyScript WASM guest calling Rust host
functions registered via `wasmtime::Linker::func_wrap`, passing `(ptr, len)`
into linear memory and reading results back via `Memory::write`/`data`.

`bridge.rs` cannot become a low-level `extern "C"` shim to make this work,
though — it is written once and shared across backends
(`examples/config_server/bridge.rs`'s own header: "same MVL-visible
signatures" for Rust and, now, WASM), using idiomatic Rust types the Rust
backend's transpiled code already produces (`Config`, `Secret<String>`,
`Result<Clean<String>, HandlerError>`). Forcing bridge authors to hand-write
a pointer-marshalling ABI layer instead would violate that "same file, same
signatures" contract and duplicate logic no compiler-generated code should
duplicate by hand.

---

## Decision

1. **`wasm_text.rs` declares real imports.** For every `extern "rust"` fn,
   emit `(import "extern" "<name>" (func $<name> <sig>))`, using the exact
   same `Ty → WASM type` lowering already used for ordinary calls (String →
   `(ptr, len)` i32 pair; struct/enum/Option/Result → boxed i32 pointer;
   `Ty::Labeled` peeled to its inner type — labels are zero runtime
   representation at this boundary, confirmed true everywhere else in this
   file already). This alone is the direct #2049 fix, matching what the
   file's own doc comment already anticipated at the `extern "wasm"` ABI
   note (now removed — see below, this reuses `extern "rust"`, it is not a
   new ABI keyword).

2. **A generated wasmtime-embedding host satisfies those imports at run
   time**, not a second WASM module. `wasm_host_glue::generate_host_main`
   generates a small Rust `main.rs` that:
   - Embeds `wasmtime` + `wasmtime-wasi`, loads `mvl_runtime_wasm.wasm` (the
     same runtime module `wasmtime run --preload runtime=...` already uses)
     and the compiled guest `.wasm`.
   - For every `extern "rust"` fn, registers one
     `Linker::func_wrap("extern", "<name>", ...)` whose body marshals the
     WASM-shaped arguments into native Rust values, calls the *unmodified*
     `bridge::<name>` (linked in via `mod bridge;`, exactly mirroring
     `cli/build.rs`'s existing Rust-backend bridge injection), and marshals
     the result back.
   - Instantiates the guest module and calls the requested export — `_start`
     by default, or a `test fn` name for the WASM test harness's per-test
     invocation (`wasmtime run --invoke`/`lli <ir> <test_name>`'s
     equivalent).

3. **Marshalling reuses layout knowledge, not a new stable ABI.**
   `StructLayout`/`collect_structs`/`collect_type_aliases` are exposed
   `pub(crate)` from `wasm_text.rs` and used directly by the host-glue
   generator — the two can never independently drift, because there is
   only one implementation. Struct/enum Rust *type declarations* are
   generated via `rust::emitter::RustEmitter::emit_tir_type_decl` — the
   exact same codegen the Rust backend itself uses — so `bridge.rs`'s
   `Config { port: Port(v), .. }`-style construction sees precisely the
   type it already targets.

4. **Supported shapes:** `Unit`, `Int`, `Bool`, `String`, user structs,
   unit-only enums, refinement-newtype aliases (`type Port = Int where
   ...`), `Secret[T]`/`Tainted[T]`/`Clean[T]`/`Public[T]`, and `Option[T]`/
   `Result[T, E]` nested arbitrarily over the above. Payload-carrying enums,
   `List`/`Map`/`Set`, and function values are explicitly unsupported for
   now — an extern fn whose signature needs one is reported via
   `UnsupportedExternFn`, never silently mis-marshalled.

5. **Label handling is two-tiered**, because label transparency differs by
   which side of the boundary is looking:
   - WASM-side layout: fully transparent, as already true everywhere else.
   - Native Rust side: *not* transparent — bridge.rs's real Rust signatures
     use `Secret<T>`/`Tainted<T>`/etc. directly. Where the MVL type system
     itself exposes the label, the generator constructs/destructures it
     directly (`Label(v)` / `v.0` — label newtypes are `#[repr(transparent)]
     pub struct Label<T>(pub T);`, robust regardless of type inference).
     Where a bridge author uses a label with no MVL-level counterpart at all
     (`Clean<T>` — MVL has no `Clean[T]`, "clean" is a Rust-only
     already-sanitized convention), every extern call argument additionally
     gets `.into()` at the call site, mirroring the Rust backend's own
     `emit_expr_as_value_arg(coerce: true)`, which relies on the identical
     blanket `impl<T> From<T> for Label<T>` in `mvl_runtime::ifc`.

6. **The WASM test harness (`mvl test --backend=wasm`) gained `test fn`
   discovery**, matching what `cmd_test_llvm_text` already did (LLVM
   supported both `fn main` + `// expect:` corpus files and `test fn`
   declarations; WASM only supported the former). No emitter changes were
   needed — `test fn`s were already exported like any other function; only
   CLI-side discovery + `wasmtime run --invoke <name>` (or the host-glue
   binary, passed the same name) were missing.

---

## Consequences

**Easier:**
- `extern "rust"` genuinely runs under `--backend=wasm` for the shapes
  listed above — not stubbed, not excluded, the real `bridge.rs`.
- `examples/config_server`'s `handler_test.mvl` (18 test fns) and
  `storage.mvl` (4 test fns) now run under `--backend=wasm`, exercising the
  real bridge (`verify_request_auth`, `get_config_value`, etc.) — previously
  zero WASM test cases were even discovered in that directory.
- Any future example with an `extern "rust"` boundary gets this for free,
  no per-example wiring beyond a `bridge.rs`.

**Harder / follow-up work:**
- Payload-carrying enums, `List[T]`/`Map[K,V]`/`Set[T]`, and `Fn` values
  crossing the extern boundary are not supported yet. `UnsupportedExternFn`
  makes this a clear build-time failure, not a silent one, but the
  capability gap remains real.
- `std.log`'s formatting builtins (`format_json_line`, `format_logfmt_line`,
  `format_fields_kv`, `format_datetime`) are unsupported under
  `--backend=wasm` — a separate, pre-existing gap, unrelated to this ADR,
  that blocks `config_server/main.mvl` itself (as opposed to its test
  files) from running under wasm. Tracked as a follow-up, not fixed here.
- The wasmtime-embedding host is a **new Rust crate compiled per test
  run** (temp dir, `cargo build`), not a lighter-weight mechanism — slower
  than the plain `wasmtime run` subprocess path for extern-free programs
  (which is why that path is preserved unchanged and only extern-using
  files pay this cost).
- Three unrelated, pre-existing `wasm_text.rs` bugs were found and fixed
  while building this (each in its own commit): struct fields whose
  declared type is an alias to `Int`/`Float`/`UInt` were under-allocated by
  4 bytes, silently corrupting a following field; a `match`/`if let` arm
  binding a value to a name that shadows an outer String-typed parameter
  produced a "duplicate local identifier" WAT assembly error. Both were
  real, live bugs independent of FFI, only surfaced because this was the
  first time `wasm_text.rs`'s output for these patterns was actually
  executed rather than just assembled.

---

## Rejected Alternatives

**Mirror LLVM's exclusion approach instead.** Turns the silent
invalid-module bug into a clear diagnostic with a much smaller diff, and
was explicitly offered as the "small" option. Rejected once the user
weighed in: the whole point of a trust-boundary example like
`config_server` is to demonstrate the extern boundary working, not to
demonstrate that it's excluded from one backend.

**A new `extern "wasm"` ABI keyword**, with a bridge author hand-writing a
separate low-level, pointer-passing implementation per backend. Rejected:
duplicates `bridge.rs` per backend, breaks the "one bridge, same
signatures, every backend" contract the file's own header comment already
promises, and pushes MVL's internal (and deliberately unstable) struct/enum
memory layout into a hand-maintained, cross-backend-visible surface.

**Compile `bridge.rs` itself to `wasm32-wasip1` and `--preload` it as a
second WASM module** (the direct, naive generalization of
`016_ffi_assemblyscript`'s pattern, and MVL's own `mvl_runtime_wasm.wasm`
preload precedent). Rejected: would require `bridge.rs` to be written
against a low-level pointer/struct-layout ABI instead of idiomatic Rust
types (`Result<Config, ConfigError>`), the same objection as above, and
loses real native capabilities (hardware crypto, arbitrary OS syscalls) —
exactly the class of workload `016_ffi_assemblyscript`'s own hypothesis
says FFI should be *for*.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **Req 11 (Information flow control) — leaves unchanged.** Labels
  (`Secret`/`Tainted`/`Clean`/`Public`) already had zero runtime
  representation at the WASM boundary before this decision; this decision
  makes the *native Rust* side of that boundary construct the exact label
  type bridge.rs expects, preserving — not weakening or strengthening — the
  existing IFC guarantee across a backend that previously couldn't reach
  the boundary at all.
- **Req 10 (Refinement types) — leaves unchanged, with one caveat worth
  stating plainly.** A refinement-newtype value crossing the boundary is
  reconstructed via direct tuple-struct construction (`Port(v)`), not the
  validating `Port::new(v)`. This does not re-open a hole: the refinement
  was already discharged (by the checker/solver) at the point the value was
  originally constructed inside the guest module, before it ever reached
  the extern boundary — this decision does not introduce a new place where
  an unvalidated value could appear.
- Requirements 1–9 are untouched by this decision.

### Design Principles (README)

- **Honest over silent — strengthens.** An extern fn signature needing an
  unsupported shape (payload enum, `List`/`Map`/`Set`, `Fn`) is reported via
  `UnsupportedExternFn` at build time, not silently mis-marshalled — this is
  the same discipline `wasm_text.rs`'s own `stubbed_fns()` reporting already
  established for unsupported function bodies, applied to the new FFI
  surface.
- **The signature IS the threat model — consistent with.** The extern
  signature's declared types (including labels) fully determine the
  generated marshalling code; nothing about the FFI boundary's behavior is
  implicit or inferred from bridge.rs's implementation.
- **Vocabulary over syntax — consistent with.** No new MVL syntax. `extern
  "rust"` is unchanged; the entire new surface is compiler-internal codegen
  (`wasm_host_glue.rs`) plus CLI plumbing.
- Other principles are not directly affected.

### Specifications

No `.openspec/specs/` files describe the WASM backend's extern-boundary
behavior specifically; none require updating for this decision.
