# ADR-0021: Primitives and Runtime Architecture Redesign

**Status:** Accepted
**Date:** 2026-05-05
**Issues:** #480, #481, #482, #483, #484, #485, #488, #489, #490
**Supersedes:** —
**Related:** ADR-0016 (LLVM memory runtime), ADR-0019 (two-path stdlib architecture)

---

## Context

Epic #480 addressed several gaps in the MVL type system and runtime architecture:

1. Unsigned numeric types (`UByte`, `UInt`) were absent despite being needed for index types and C-ABI interoperability.
2. `Map<K,V>` and `Set<T>` were represented as `Ty::Named` strings in the checker, making structural compatibility impossible.
3. Bitwise operators (`&`, `|`, `^`, `~`, `<<`, `>>`) were missing from the grammar, parser, and all backends.
4. Overflow-safe arithmetic (`checked_*`, `wrapping_*`) required safe integer handling.
5. `mvl_runtime::prelude` re-exported all OS stdlib modules unconditionally, even for programs that never used them.
6. `mvl_memory` conflated type definitions + lifecycle with operations, creating an architecturally muddled boundary.

---

## Decisions

### 1. Unsigned types: `UByte` and `UInt` (#481)

Add `Ty::UByte` and `Ty::UInt` to the checker type system.

- `UByte` maps to Rust `u8` (transpiler) and `i8` LLVM type (LLVM backend; sign is irrelevant for bit patterns).
- `UInt` maps to Rust `u64` (transpiler) and `i64` LLVM type.
- `is_unsigned_int()` helper added for dispatch on unsigned-specific operations.
- `is_integer()` covers all integer types (Int, Byte, UByte, UInt) for operator dispatch.

**Evidence:** `tests/corpus/02_types/unsigned_types.mvl`, `src/mvl/checker/types.rs`

### 2. First-class Map and Set types (#482)

Replace `Ty::Named("Map", args)` and `Ty::Named("Set", args)` with dedicated `Ty::Map(Box<Ty>, Box<Ty>)` and `Ty::Set(Box<Ty>)` variants.

- Structural compatibility (`types_compatible`) now compares key/value/element types recursively.
- Map and Set method dispatch (`map_method_ty`, `set_method_ty`) operates on concrete type parameters.
- Map and Set literals infer `Ty::Map`/`Ty::Set` directly (no string round-trip).

**Evidence:** `src/mvl/checker/types.rs`, `src/mvl/checker/mod.rs`

### 3. Bitwise operators (#483, #484)

Add six bitwise operators to the full language stack:

| MVL | Rust (transpiler) | LLVM IR |
|-----|-------------------|---------|
| `a & b` | `a & b` | `and` |
| `a \| b` | `a \| b` | `or` |
| `a ^ b` | `a ^ b` | `xor` |
| `~a` | `!a` | `xor` with `const_all_ones()` |
| `a << n` | `a << n` | `shl` |
| `a >> n` | `a >> n` | `ashr` (arithmetic shift, sign-extended) |

Pratt table renumbered to 10–90 scale (matching C-style precedences): BITOR=30, BITXOR=40, BITAND=50, shift=70, add=80, mul=90. This matches ISO C operator precedence.

Grammar (`docs/grammar.ebnf`), tree-sitter (`etc/tree-sitter-mvl/grammar.js`, `highlights.scm`), and nvim highlights updated.

**Evidence:** `tests/corpus/02_types/bit_operators.mvl`, `src/mvl/parser/ast.rs`, `src/mvl/backends/rust/emit_exprs.rs`, `src/mvl/backends/llvm/exprs.rs`

### 4. Overflow-checking arithmetic (#485)

Method-based API (`.checked_add(b)`) rather than standalone functions:
- Avoids polluting the global namespace.
- Consistent with Rust's own API.
- Applicable to `Int`, `Byte`, `UByte`, `UInt`.

Methods: `checked_add/sub/mul/div` return `Option[T]`; `wrapping_add/sub/mul` return `T`.

**Evidence:** `tests/corpus/02_types/overflow_checking.mvl`, `src/mvl/checker/mod.rs` (`int_method_ty`, `byte_method_ty`, etc.)

### 5. Slim prelude + stdlib-gated imports (#488, #489)

`mvl_runtime::prelude` now exports ONLY language fundamentals:
- `effects`, `ifc`, `mvl_refine` — core language machinery
- `str_*`, `list_*` kernel primitives — always needed by generated code
- `MvlMap`, `MvlContains`, `MvlPow` traits — method dispatch across types
- `ParseFromArgs`, `get_arg`, `parse` — struct-parsing infrastructure (generated for all concrete structs)

OS-specific modules (`io`, `env`, `log`, `time`, `random`, `process`, `crypto`) are NOT in the prelude. Instead, the transpiler emits an explicit `use mvl_runtime::stdlib::X::*;` line for each `use std.X.*` declaration in the MVL source.

New helper: `collect_stdlib_modules(prog: &Program) -> Vec<String>` in `src/mvl/backends/rust/mod.rs`.

**Evidence:** `runtime/rust/src/prelude.rs`, `src/mvl/backends/rust/emitter.rs`

### 6. mvl_memory scope clarification (#490)

`mvl_memory` now contains **only**:
- Type definitions: `MvlString`, `MvlArray`, `MvlMap`, `MvlMapSlot`
- Allocation primitives: `mvl_alloc`, `mvl_free`, `mvl_panic`
- Lifecycle: `mvl_{string,array,map}_{new,clone,drop}`

All **operations** move to `mvl_runtime_c::memory_ops`:
- String: `mvl_string_{len,ptr,concat,eq}`
- Array: `mvl_array_{push,get,len}`
- Map: `mvl_map_{insert,get,len}` + internal helpers `fnv1a`, `map_find_slot`

`mvl_memory` tests are now lifecycle-only (Miri-safe: raw struct field access, no operation calls). Operation tests live in `mvl_runtime_c::memory_ops::tests`.

**Evidence:** `runtime/llvm/src/memory.rs`, `runtime/llvm/src/memory_ops.rs`

---

## Consequences

**Positive:**
- Unsigned types enable safe bit manipulation and C-ABI index types.
- First-class Map/Set enables structural type checking.
- Bitwise operators complete the integer operation set.
- Slimmed prelude reduces Rust compile times for programs that don't use OS modules.
- Explicit stdlib imports make dependencies visible and auditable.
- `mvl_memory` is now a clean boundary (types + lifecycle), testable with Miri.

**Neutral:**
- `ParseFromArgs` / `get_arg` / `parse` remain in the prelude because the transpiler generates `impl ParseFromArgs` for all concrete structs unconditionally. A future phase can gate this on `use std.args.*`.
- LLVM right-shift uses arithmetic shift (ashr) by default; a future phase can add unsigned shift operators.

**Follow-up issues:**
- `as` type-cast syntax (UByte → Int, etc.) not yet implemented — corpus note added.
- Unsigned right-shift (`>>>`) for `UByte`/`UInt` may be added in a future phase.
- `ParseFromArgs` emission could be gated on `use std.args.*` to further slim generated code.
