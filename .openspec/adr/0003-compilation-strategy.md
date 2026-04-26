# ADR-0003: Compilation Strategy — Prototype Rust, Production LLVM

**Status:** Accepted
**Date:** 2026-04-11
**Context:** How should the MVL compiler emit executable code? Transpile to an existing language or target LLVM IR directly?

## Decision

Four-phase approach:
1. **Phase 1 — It compiles:** MVL → Rust transpilation
2. **Phase 2 — It's useful:** Rust FFI ecosystem, real programs
3. **Phase 3 — It's trustworthy:** LLVM backend, all 11 proven at compile time
4. **Phase 4 — It's self-sufficient:** Self-hosting, package ecosystem, certification

## Rationale

### The ISPE argument

The I→S→P→E model requires P→E to be deterministic and proof-preserving. Transpiling to another language (MVL → Rust → binary) puts TWO compilers in the trust chain:
- The MVL compiler verifies the 11 requirements
- The Rust compiler re-verifies ownership, types, lifetimes

They might disagree. Rust may reject valid MVL programs (borrow checker friction). Rust may accept code that loses MVL's IFC labels (erased in transpilation). One compiler, one trust boundary is the correct architecture.

CompCert (Leroy, INRIA, 2006) proved this: the compiler IS the theorem prover, the backend is a verified code generator.

### Why Rust for Phase 1

Rust scores 7/11 — highest of any mainstream language. The transpilation only adds 4 requirements (termination, races, refinements, IFC). Go would require adding 10.5. Zig 6.5. Rust is the closest starting point.

### Why LLVM for Phase 3

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

## Phase 1 Completion Criteria

Phase 1 is **done** when:

1. **`mvl build` produces a native binary** — transpile to Rust, generate `Cargo.toml`, invoke `cargo build`
2. **Both reference examples compile and run** — `auth_handler.mvl` and `safe_division.mvl` produce working binaries
3. **All 11 requirements are enforced** — 9 at MVL compile time, Req 10 as Rust runtime asserts, Req 11 as Rust newtypes
4. **`mvl test` runs tests** — transpile `_test.mvl` files to Rust `#[test]`, invoke `cargo test`
5. **Module system works** — multi-file programs with `module` and `use`
6. **Generics emit correctly** — `Array[T]`, `Option[T]`, `Result[T,E]` map to Rust generics
7. **Core stdlib bridge exists** — core types and operations map to Rust std equivalents

### Phase 1 Critical Path

```
Step 1: Transpiler basics (#29, #30)     → types + functions emit Rust
Step 2: Stdlib bridge (#42, #43)         → core types map to Rust std
Step 3: End-to-end (#33)                 → corpus compiles to binary
Step 4: Cargo integration (#34)          → `mvl build` = transpile + cargo
Step 5: IFC as newtypes (#31)            → Req 11 enforcement via Rust
Step 6: Refinements as asserts (#32)     → Req 10 enforcement via Rust
Step 7: Module system (#47)              → multi-file programs
Step 8: Generics (#48)                   → Array[T], Option[T] emit correctly
```

Steps 1-4 achieve "hello world to binary." Steps 5-6 complete all 11 requirements. Steps 7-8 make it usable for real programs.

## Phase 2 — It's useful

**Done when:** Real programs written in MVL, calling Rust ecosystem via FFI.

- **`extern "rust"` blocks** (#52, #91) — call any Rust crate through typed, effect-tracked, IFC-labeled boundaries
- **Module system** (#47) — multi-file programs with `module` and `use`
- **Generics** (#48) — `Array[T]`, `Option[T]`, `Result[T,E]` emit correctly
- **Test transpilation** (#38) — `_test.mvl` → Rust `#[test]`
- **Assurance reports** (#73) — compiler tracks verified vs trusted (extern) ratio
- **Zero MVL stdlib** — Rust is the stdlib, accessed through extern. Stdlib grows later as verified MVL wrappers.

## Phase 3 — It's trustworthy

**Done when:** All 11 requirements proven at compile time. One compiler, one trust chain.

- **LLVM IR backend** — replace Rust transpilation with direct LLVM codegen
- **SMT solver integration** — Req 10 moves from runtime asserts to compile-time proofs
- **Native IFC analysis** — Req 11 moves from Rust newtypes to compiler-native flow checking
- **Borrow lifetimes** — full Req 2 enforcement (beyond use-after-move)
- **Linear resources** — full Req 6 enforcement (must-consume semantics)
- **Structural recursion proofs** — full Req 8 enforcement
- **Model checker** (#37) — invariants, deadlock/livelock detection as compiler pass
- **WASM target** — sandboxed execution for The Cog and edge deployment

## Phase 4 — It's self-sufficient

**Done when:** MVL compiler compiles itself. Full ecosystem.

- **Self-hosting** — MVL compiler rewritten in MVL, compiled by Phase 3 compiler
- **Package manager** (#56) — dependency resolution, SBOM generation, trust scoring
- **Verified MVL stdlib** — replaces extern wrappers, pushes assurance ratio toward 90%+
- **Concurrency model** — actors, reference capabilities, WCET refinements
- **Transpilation corpus** — seed for LLM training on MVL generation quality
- **AAE-5 certification pipeline** — automated evidence for IEC 61508, DO-178C

## Consequences

- Phase 1 accepts two-compiler friction as temporary cost
- Phase 2 makes the language immediately useful without building a stdlib
- Phase 3 requires building LLVM codegen — significant investment
- Phase 4 is the long game — self-hosting proves the language is general-purpose
- WASM target (Phase 3) unlocks sandboxed execution for The Cog and edge deployment
