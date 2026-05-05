# ADR-0003: Compilation Strategy — Prototype Rust, Production LLVM

**Status:** Accepted
**Date:** 2026-04-11
**Revised:** 2026-04-30 — expanded to five phases; Phases 1–4 completed; Phase 5 (LLVM) active
**Context:** How should the MVL compiler emit executable code? Transpile to an existing language or target LLVM IR directly?

## Decision

Five-phase approach (originally four; Phase 3 split as the project matured):

| Phase | Name | Status |
|-------|------|--------|
| 1 | **It compiles** — MVL → Rust transpilation | ✅ Complete |
| 2 | **It's useful** — Rust FFI ecosystem, real programs | ✅ Complete |
| 3 | **It's verified** — All 11 requirements enforced at compile time | ✅ Complete |
| 4 | **It's complete** — Full stdlib in pure MVL | ✅ Complete |
| 5 | **It's native** — Direct LLVM IR backend | 🔄 Active (Phase A done) |
| 6 | **It's trustworthy** — Formal proofs, SMT solver, model checker | ⬜ Planned |
| 7 | **It's self-sufficient** — Self-hosting, certification pipeline | ⬜ Planned |

> **Why the split?** The original Phase 3 bundled LLVM codegen and formal provers together as a single milestone. In practice, the type checker achieved full 11-requirement enforcement via the Rust transpiler (completing "trustworthy" semantics without LLVM), and the stdlib matured enough to warrant its own phase. LLVM IR codegen is now a distinct engineering phase (Phase 5) tracked under the `phase-5` GitHub label.

## Rationale

### The ISPE argument

The I→S→P→E model requires P→E to be deterministic and proof-preserving. Transpiling to another language (MVL → Rust → binary) puts TWO compilers in the trust chain:
- The MVL compiler verifies the 11 requirements
- The Rust compiler re-verifies ownership, types, lifetimes

They might disagree. Rust may reject valid MVL programs (borrow checker friction). Rust may accept code that loses MVL's IFC labels (erased in transpilation). One compiler, one trust boundary is the correct architecture.

CompCert (Leroy, INRIA, 2006) proved this: the compiler IS the theorem prover, the backend is a verified code generator.

### Why Rust for Phase 1

Rust scores 7/11 — highest of any mainstream language. The transpilation only adds 4 requirements (termination, races, refinements, IFC). Go would require adding 10.5. Zig 6.5. Rust is the closest starting point.

### Why LLVM for Phase 5

- One compiler, one proof chain
- All targets: ARM, x86, WASM, RISC-V
- WASM enables sandboxed execution (The Cog, edge, browser)
- No fighting another language's type system
- This is what Rust, Zig, Swift, and Lean 4 all chose

## Alternatives considered

| Approach | Score | Rejected because |
|----------|-------|-----------------|
| MVL → Go | Go is 0.5/11 | Must implement nearly everything ourselves |
| MVL → Zig | Zig is 4.5/11 | Better than Go but still 6.5 requirements to add |
| MVL → C | C is 0/11 | No safety properties at all |
| MVL → JVM bytecode | JVM has GC | Violates Req 6 (ownership), no deterministic deallocation |
| Direct machine code | — | Reimplements optimization. LLVM does this better. |

## Phase 1 — It compiles ✅

**Done when:** `mvl build` produces a native binary.

- `mvl build` = transpile to Rust + invoke `cargo build` (#29, #30, #33, #34)
- Both reference examples compile and run (`auth_handler.mvl`, `safe_division.mvl`)
- IFC as Rust newtypes (#31) — Req 11 enforcement
- Refinements as Rust runtime asserts (#32) — Req 10 enforcement
- Module system (#47) — multi-file programs with `module` and `use`
- Generics (#48) — `Array[T]`, `Option[T]`, `Result[T,E]` emit correctly

## Phase 2 — It's useful ✅

**Done when:** Real programs written in MVL calling the Rust ecosystem via FFI.

- `extern "rust"` blocks (#52, #91) — typed, effect-tracked, IFC-labeled FFI
- Test transpilation (#38) — `_test.mvl` → Rust `#[test]`; `mvl test` runs corpus
- Assurance reports (#73) — compiler tracks verified vs trusted (extern) ratio
- `mvl mutate`, `mvl coverage` — mutation testing and MC/DC tooling

## Phase 3 — It's verified ✅

**Done when:** All 11 requirements enforced at MVL compile time (not only at Rust level).

- Full type checker — 200+ integration tests, all 11 requirements assigned to error variants
- Borrow tracking — `BorrowState` (Owned/SharedBorrowed/MutablyBorrowed), lifetime scope depth
- Effect system — `IO`, `Net`, `FS`, `Terminal` capabilities tracked; `pure` enforcement
- IFC label propagation — Req 11 checked in checker, not only as Rust newtypes
- Mutation testing engine (#331) — boundary value analysis, surviving mutant reports
- MC/DC coverage (#315) — condition/decision coverage reporting

## Phase 4 — It's complete ✅

**Done when:** A genuine MVL stdlib exists, written in MVL, not just Rust wrappers.

- Pure MVL higher-order list methods (#307) — `filter`, `fold`, `take_while`, `skip_while`, `any`, `all`
- `std/crypto` with real SHA-256/512 and CSPRNG (#349) — backed by Rust runtime
- Stdlib scope defined: `std/lists`, `std/strings`, `std/math`, `std/crypto`, `std/io`, `std/args`, `std/log`
- Assurance ratio enforced — `mvl test` harness with `// expect:` annotation support

## Phase 5 — It's native 🔄

**Done when:** Direct LLVM IR backend replaces Rust transpilation as the primary compilation path.

The LLVM backend is gated on the `llvm` Cargo feature (default-on). `mvl build/run/test --backend=llvm` invokes it.

### Phase 5A — Hello World ✅ (v0.55.0, closes #352)

| Story | Description | Status |
|-------|-------------|--------|
| L5-01 | `inkwell` optional dep, `llvm` Cargo feature gate | ✅ |
| L5-02 | LLVM module setup: target triple, data layout, `main() → i32 0` | ✅ |
| L5-03 | `mvl test --backend=llvm` dual-backend harness with `// expect:` | ✅ |
| L5-04 | Primitive type codegen: `Int→i64`, `Float→f64`, `Bool→i1`, `Byte→i8`, `Char→i32`, `Unit→void`, `String→ptr` | ✅ |
| L5-07 | Functions: two-pass emit, parameter alloca pattern, if-expressions with phi nodes | ✅ |
| L5-10 | Arithmetic with checked overflow (`llvm.sadd/ssub/smul.with.overflow` + `llvm.trap`), comparisons, float ops | ✅ |
| L5-17 | `print`/`println` → libc `printf`; typed format dispatch (`%lld`/`%f`/`%s`) | ✅ |

### Phase 5B — Structs & Enums (planned)

- Struct type codegen → LLVM `{field...}` aggregate types
- Enum codegen → tagged union representation
- Pattern matching → `switch` + phi

### Phase 5C — Generics & Polymorphism (planned)

- Monomorphisation pass — instantiate generic functions at call sites
- `Array[T]` → heap-allocated with bounds-checked indexing

### Phase 5D — Borrow Analysis (planned)

- Move elision and borrow tracking in LLVM backend (currently only in Rust transpiler)
- `val T` / `ref T` as LLVM pointer types with compiler-enforced aliasing rules

### Phase 5E — WASM & Targets (planned)

- `--target=wasm32` — WASM output for The Cog and edge deployment
- `--target=aarch64` — ARM support
- One compiler, all targets — the core promise of Phase 5

## Phase 6 — It's trustworthy ⬜

**Done when:** Formal proofs replace runtime asserts and extern trust.

- SMT solver integration — Req 10 (refinements) proven at compile time
- Native IFC flow analysis — Req 11 proven without Rust newtypes
- Full Non-Lexical Lifetimes — Req 2 (memory safety) with complete borrow analysis
- Linear resources — Req 6 (must-consume) enforced in compiler, not just runtime
- Structural recursion proofs — Req 8 termination
- Model checker (#37) — invariants, deadlock/livelock detection

## Phase 7 — It's self-sufficient ⬜

**Done when:** MVL compiler compiles itself. Full ecosystem for certified software.

- Self-hosting — MVL compiler rewritten in MVL, compiled by Phase 6 compiler
- Package manager (#56) — dependency resolution, SBOM generation, trust scoring
- Verified MVL stdlib — replaces Rust runtime wrappers, assurance ratio → 90%+
- Concurrency model — actors, reference capabilities, WCET refinements
- AAE-5 certification pipeline — automated evidence for IEC 61508, DO-178C

## Consequences

- Phase 1 accepted two-compiler friction as a temporary cost — now complete
- Phases 1-4 on the Rust transpiler proved the language design before building LLVM
- Phase 5 is the production path: one compiler, one proof chain, all targets
- The `llvm` Cargo feature is default-on from v0.55.0; `--no-default-features` keeps the Rust backend
- WASM target (Phase 5E) unlocks sandboxed execution for The Cog and edge deployment
- Phases 6-7 are the long game — formal verification and self-hosting prove the language is general-purpose
