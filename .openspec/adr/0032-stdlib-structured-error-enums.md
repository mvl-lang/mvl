# ADR-0031: Stdlib Structured Error Enums

**Status:** Accepted
**Date:** 2026-05-16
**Issues:** #782
**Related:** ADR-0001 (eleven requirements), ADR-0019 (two-path stdlib), ADR-0016 (LLVM memory runtime)

---

## Context

All stdlib functions currently return `Result[T, String]` for errors:

```mvl
pub builtin fn tcp_connect(host: String, port: Int) -> Result[TcpStream, String] ! Net
pub builtin fn read_to_string(p: Path) -> Result[Tainted[String], String] ! FileRead
```

This violates two of the eleven compiler-verified requirements:

- **Req 3 (Totality):** MVL enforces exhaustive match, but `String` has no variants — callers cannot branch on error kind at compile time.
- **Req 5 (Error visibility):** `String` tells the type system nothing about *what* can fail; all error paths look identical.

Callers must inspect the string at runtime (fragile) or ignore it entirely.

---

## Decision

### 1. Define domain-specific error enums in `std/*.mvl`

Each stdlib module gains a public error enum. All enums follow the same shape: named unit variants for well-known error kinds plus a catch-all `Other(String)` for unclassified OS errors.

```mvl
// std/net.mvl
pub type NetError = enum {
    ConnectionRefused,
    ConnectionReset,
    Timeout,
    AddressInUse,
    HostUnreachable,
    Other(String),
}
```

Modules and their error types:

| Module      | Error type      | Variants                                                      |
|-------------|-----------------|---------------------------------------------------------------|
| `std.net`   | `NetError`      | ConnectionRefused, ConnectionReset, Timeout, AddressInUse, HostUnreachable, Other(String) |
| `std.io`    | `IoError`       | NotFound, PermissionDenied, AlreadyExists, IsADirectory, InvalidPath, Other(String) |
| `std.regex` | `RegexError`    | InvalidPattern(String), Other(String)                         |
| `std.json`  | `JsonError`     | SyntaxError, UnexpectedToken, MissingField, TypeMismatch, Other(String) |
| `std.process` | `ProcessError` | NotFound, PermissionDenied, SpawnFailed, Other(String)       |

`std.crypto` and `std.time` have no fallible operations and are unaffected.

### 2. Mirror enums in the Rust runtime (established pattern)

Each error enum is replicated verbatim in `runtime/rust/src/stdlib/*.rs` with a comment "Mirrors the `XxxError` enum declared in `std/xxx.mvl`." This is the same pattern already used by `ExitStatus` and `ProcessOutput`. The sanitize functions return enum variants instead of strings.

```rust
/// Mirrors the `NetError` enum declared in `std/net.mvl`.
#[derive(Debug, Clone, PartialEq)]
pub enum NetError {
    ConnectionRefused,
    ConnectionReset,
    Timeout,
    AddressInUse,
    HostUnreachable,
    Other(String),
}
```

### 3. Extend the LLVM ABI with `LlvmEnumError`

The LLVM backend uses `LlvmResult { tag: u8, payload: *mut c_void }`. Currently, error payloads are `*mut MvlString` (a string). For enum errors we introduce a new heap-allocated struct:

```rust
/// Heap-allocated enum error value matching MVL payload-enum LLVM layout.
/// Layout: `{ i8, [8 x i8] }` — matches what the LLVM codegen generates for
/// payload enums where the largest variant payload is one pointer (8 bytes).
#[repr(C)]
pub struct LlvmEnumError {
    pub disc: u8,        // variant discriminant (0-based, matches MVL enum order)
    pub payload: [u8; 8], // pointer-sized payload bytes (zero for unit variants)
}
```

This layout exactly matches the LLVM IR type `{ i8, [8 x i8] }` that the codegen generates for payload enums. The discriminant at offset 0 is the `i8` field; the payload bytes at offset 1 are the `[8 x i8]` field. Since both fields have alignment 1, there is no padding.

Helper methods:
- `LlvmEnumError::unit(disc)` — unit variant, payload zeroed
- `LlvmEnumError::with_str(disc, msg)` — variant with one `String` payload (stores `*mut MvlString` bytes)

The LLVM codegen already handles payload enum pattern matching generically:
1. Receive `LlvmResult { tag=1, payload=*mut LlvmEnumError }`
2. `wrap_c_result_with_slot` stores the payload pointer in a stack alloca
3. `bind_pattern_vars` for `Err(e)` loads the alloca → `*mut LlvmEnumError`
4. Match on `e: XxxError`: load i8 discriminant from field 0, switch, extract payload

No codegen changes are required; the existing enum matching infrastructure is fully generic.

### 4. Process module: acknowledged gap

`std.process` LLVM functions (`_mvl_process_spawn`, `_mvl_process_wait`, `_mvl_process_kill`) use `MvlResult` (the 3-field C-ABI path with `err: *mut c_char`). This is a different ABI from the `LlvmResult` path used by net/io/regex. Converting process to `LlvmResult` requires additional LLVM codegen work and is deferred to a follow-up issue.

For this release: the Rust runtime path for `std.process` is fully updated (returns `ProcessError` enum). The LLVM path retains string errors and is flagged with a TODO.

### 5. `std.json` — pure MVL, no runtime changes

`decode` is a `pub transparent partial fn` implemented entirely in MVL. No runtime or LLVM changes are needed. Only the MVL source and the single error conversion point (`VPErr(e) => Err(JsonError::Other(e))`) are updated.

---

## Consequences

**Positive:**
- Callers can now exhaustively match on error kinds — the compiler enforces this (Req 3).
- Error kinds are visible in the type signature — no runtime string parsing (Req 5).
- The `Other(String)` catch-all preserves backward compatibility for unclassified errors.
- The mirror pattern is low-friction: no generated code, no macro magic.

**Negative / trade-offs:**
- **Breaking change**: existing code using `Err(e)` where `e: String` will not compile. Migration: match on `XxxError::Other(msg)` or the specific variant.
- The mirror pattern requires keeping two definitions in sync (MVL enum + Rust enum). A future type-generation step could automate this.
- The `LlvmEnumError` payload is fixed at 8 bytes (one pointer). Error enums with multi-field variants would require a larger payload. Current enums only use `Other(String)` — one pointer.
- The process LLVM gap means programs compiled with the LLVM backend cannot yet pattern-match on `ProcessError` variants.

---

## Rejected Alternatives

**Keep `Result[T, String]` everywhere:** Maintains the status quo but permanently blocks Req 3 and Req 5 for error paths. Rejected.

**Single shared `StdError` enum:** A single uber-enum across all stdlib modules would conflate unrelated error domains and make exhaustive matching unwieldy. Rejected in favour of per-module enums.

**`builtin type XxxError` (opaque runtime types):** Would require the transpiler to know about every error variant. Using normal `pub type XxxError = enum` lets the type checker handle it uniformly. Rejected.

**Struct variant syntax `Other { msg: String }`:** The LLVM codegen does not yet support struct variant field extraction by name. Tuple variant `Other(String)` uses `Pattern::TupleStruct` which is fully supported. Rejected in favour of tuple variant.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

| Req | Effect |
|-----|--------|
| 3 (Totality) | **Strengthened** — callers can now write exhaustive matches over error kinds; the compiler rejects non-exhaustive patterns |
| 5 (Error visibility) | **Strengthened** — error types are now domain-specific enums; `String` no longer hides what can fail |
| All others | Unchanged |

### Design Principles (README)

- **Req 3 (Totality) / exhaustive match** — strengthens
- **Req 5 (Error visibility) / explicit Result** — strengthens
- **Simplicity** — consistent with; enums are idiomatic MVL, no new mechanisms
- **Two-path stdlib (ADR-0019)** — consistent with; the mirror pattern is already in use

### Specifications

- `019-stdlib-errors/spec.md` (new) — this ADR is the architectural basis for that spec.
- `002-effect-system/spec.md` — unaffected; effect annotations on stdlib functions unchanged.
