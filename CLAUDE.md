# MVL Language — Claude Code Instructions

## Source of Truth Hierarchy

When code, tests, specs, or docs conflict, follow this priority:

1. **Language definition** — `.openspec/language.md` + `docs/grammar.ebnf`
2. **Specs** — `.openspec/specs/NNN-*/spec.md`
3. **ADRs** — `.openspec/adr/NNNN-*.md`
4. **Implementation** — `src/mvl/` (parser, checker, transpiler, codegen)
5. **Tests** — `tests/` (must match implementation AND language definition)
6. **Docs/manual** — `docs/` (may lag behind; update when touching related code)

If you find a conflict between layers, **flag it** before proceeding. Do not silently "fix" tests to match stale implementation or reintroduce removed concepts to make old tests pass.

## Drift Detection: Stop and Ask

**Before implementing any change**, check whether the change is consistent with the current language design. Specifically:

- If a test references syntax or concepts that don't exist in `docs/grammar.ebnf` or `.openspec/language.md`, the **test is wrong**, not the implementation.
- If implementation code still uses old terminology in comments but the runtime behavior is correct, the **comments need updating**, not the code.
- If a spec references removed features, the **spec needs updating**.

**Red flags that indicate drift:**
- Tests using `&T` / `&mut T` syntax in MVL source strings (should be `val T` / `ref T`)
- Comments or docs describing MVL-level semantics using Rust borrow terminology
- Code reintroducing `&`-style parsing or checking that was deliberately removed
- New features built on top of concepts that were simplified away

**When you detect drift, do NOT:**
- Silently make tests pass by reintroducing removed concepts
- "Fix" the implementation to match outdated tests
- Add compatibility shims for old syntax

**Instead:**
- Flag the specific drift to the user
- Identify which layer is wrong (test vs impl vs spec vs grammar)
- Propose updating the wrong layer to match the source of truth

## MVL Syntax: Capability-Based References (NOT Rust Borrows)

MVL uses **Pony-inspired capability keywords**, not Rust `&`/`&mut` syntax:

| MVL syntax | Meaning | Rust equivalent (emitted) |
|------------|---------|---------------------------|
| `val T` | Shared immutable reference | `&T` |
| `ref T` | Exclusive mutable reference | `&mut T` |
| `val expr` | Take shared borrow | `&expr` |
| `ref expr` | Take mutable borrow | `&mut expr` |
| `iso T` | Isolated (future, needs actors) | — |
| `tag T` | Opaque/non-capability (future, needs actors) | — |

**Key distinction:** MVL *source* uses `val`/`ref`. The *transpiler* correctly emits Rust `&`/`&mut`. The *LLVM codegen* may reference `&T` in comments about emitted IR. This is fine — the confusion is when MVL-level descriptions use Rust syntax.

## Known Drift Inventory (as of 2026-05-06)

These are known areas where old `&T`/`&mut T` Rust syntax persists in MVL-level descriptions:

- `src/mvl/transpiler/borrow_params.rs` — Module docs (lines 1-31) describe MVL semantics using `&T`/`&mut T`
- `.openspec/language.md:178` — Mock example uses `&DbConn` instead of `val DbConn`
- `tests/parser/borrow.rs` — Doc comments say "inferred as &T" (test inputs are correct)
- `tests/transpiler.rs:689,735` — Comments reference "&T" for MVL params
- `tests/type_checker.rs:4206-4248` — Comments mix `&T` with `val`/`ref`
- `src/mvl/codegen/mod.rs:1689`, `exprs.rs:1045` — LLVM comments say "Borrow params (`&T`)"
- `.openspec/specs/009-transpiler-codegen/spec.md` — Phase B references use `&T` framing

These are **comment/doc issues only** — the runtime behavior is correct.

## Function Implementation Trinity

Every function in MVL belongs to exactly one of three tiers. When adding or designing a function, identify its tier first — it determines where the implementation lives and what work is required.

### Tier 1 — Primitives (compiler-lowered)

The compiler translates these directly to IR/Rust without any runtime call. No stdlib file, no dylib.

- Arithmetic / comparison operators (`+`, `-`, `*`, `/`, `==`, `<`, …)
- Boolean operators (`&&`, `||`, `!`)
- `panic(msg)`, string interpolation, integer/float literals
- Struct/enum construction and pattern matching
- Basic I/O builtins (`print`, `println`)

**Transpiler:** emitted inline as Rust expressions.
**LLVM:** emitted inline as LLVM IR instructions.
**Rule:** if it can be a primitive, it should be. No function call overhead, no dylib dependency.

### Tier 2 — Stdlib builtins (dual implementation required)

Functions that need OS/hardware access or non-trivial Rust internals. Each one must be implemented **twice**:

| Side | How | Where |
|------|-----|--------|
| Transpiler | `pub builtin fn` in `.mvl` + Rust fn in `mvl_runtime::stdlib::X` | `mvl_runtime/src/stdlib/` |
| LLVM | C-ABI export in `mvl_runtime_c` + declaration + emit case in `src/mvl/codegen/` | `mvl_runtime_c/src/` + `codegen/` |

Examples: `std.random.*`, `std.env.*`, `std.process.*`, `std.crypto.*`, `std.time.*`, `std.io.*`, `std.args.*`, all memory ops (`mvl_string_*`, `mvl_array_*`, `mvl_map_*`).

**Cost:** 3 files touched per new function. Use sparingly.

### Tier 3 — Pure MVL stdlib (zero backend work)

Functions implemented entirely in `.mvl` source, calling only Tier 1 primitives and already-wired Tier 2 builtins. Both backends handle them automatically.

Two sub-tiers:

- **Core stdlib** — loaded as part of the language prelude; always available without an explicit `use`. Examples: basic collection helpers, `Option`/`Result` methods.
- **Regular stdlib** — explicitly imported (`use std.pbt.*`, `use std.json.*`, etc.); higher-level libraries built on top of Tier 1 and Tier 2.

**Rule:** default to Tier 3. Only escalate to Tier 2 when the feature genuinely requires OS/hardware access that cannot be expressed in MVL.

---

## Dual-Backend Rule

Every language feature, stdlib addition, and architectural decision must work with **both** backends:

| Backend | How code runs | Stdlib access |
|---------|--------------|---------------|
| **Transpiler** | MVL → Rust source → `rustc` | `use mvl_runtime::stdlib::X::*` (Rust crate linked at compile time) |
| **LLVM** | MVL → LLVM IR → `lli` | C-ABI dylibs loaded at runtime (`libmvl_memory`, `libmvl_runtime_c`) |

**Implications for new stdlib functions:**

- `pub builtin fn` (Rust-backed) costs **3× LLVM work** per function: a C-ABI export in `mvl_runtime_c`, a declaration in `src/mvl/codegen/`, and an emit case in `codegen/exprs.rs` or `mod.rs`.
- **Pure MVL** stdlib functions that only call already-wired primitives cost **zero LLVM work** — the IR is emitted automatically.
- Before reaching for a Rust builtin, check whether the feature can be built in pure MVL on top of existing primitives. Example: `std.random.int` and `std.random.float` are already wired in the LLVM backend; any pure MVL code calling them works in both backends for free.

**Already wired in the LLVM backend** (safe to call from pure MVL stdlib):
- `std.random.int(min, max)` → `_mvl_random_int`
- `std.random.float()` → `_mvl_random_float`
- `std.env.*`, `std.process.*`, `std.crypto.*`, `std.time.*`, `std.args.*`
- String, Array, Map memory ops (`mvl_string_*`, `mvl_array_*`, `mvl_map_*`)

**Red flag:** If a design says "add to `RUST_BACKED_STDLIB`" without first checking whether pure MVL suffices, push back and verify.

## Build and Test

```bash
cargo build                  # build
cargo test                   # full test suite (includes pre-commit checks)
cargo test --test type_checker  # just checker tests
cargo test --test transpiler    # just transpiler tests
cargo test --test compile_and_run  # corpus end-to-end tests
make test-llvm               # LLVM backend tests (builds mvl_runtime_c)
```

## Project Structure

- `src/mvl/parser/` — Lexer, parser, AST definitions
- `src/mvl/checker/` — Type checker, effects, IFC, borrow state
- `src/mvl/transpiler/` — Rust code emitter (Phase A last-use, Phase B borrow params)
- `src/mvl/codegen/` — LLVM backend
- `src/mvl/stdlib/` — Embedded stdlib `.mvl` files
- `tests/corpus/` — End-to-end test programs
- `.openspec/` — Specs, ADRs, language definition
- `docs/grammar.ebnf` — Authoritative grammar (LL(1), ~100 productions)
