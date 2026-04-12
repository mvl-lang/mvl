# ADR-0003: Compilation Strategy — Prototype Rust, Production LLVM

**Status:** Accepted
**Date:** 2026-04-11
**Context:** How should the MVL compiler emit executable code? Transpile to an existing language or target LLVM IR directly?

## Decision

Three-phase approach:
1. **Phase 1 (prototype):** MVL → Rust transpilation
2. **Phase 2 (production):** MVL → LLVM IR
3. **Phase 3 (ecosystem):** Package manager, transpilation corpus, tooling

## Rationale

### The ISPE argument

The I→S→P→E model requires P→E to be deterministic and proof-preserving. Transpiling to another language (MVL → Rust → binary) puts TWO compilers in the trust chain:
- The MVL compiler verifies the 11 requirements
- The Rust compiler re-verifies ownership, types, lifetimes

They might disagree. Rust may reject valid MVL programs (borrow checker friction). Rust may accept code that loses MVL's IFC labels (erased in transpilation). One compiler, one trust boundary is the correct architecture.

CompCert (Leroy, INRIA, 2006) proved this: the compiler IS the theorem prover, the backend is a verified code generator.

### Why Rust for Phase 1

Rust scores 7/11 — highest of any mainstream language. The transpilation only adds 4 requirements (termination, races, refinements, IFC). Go would require adding 10.5. Zig 6.5. Rust is the closest starting point.

### Why LLVM for Phase 2

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
6. **Generics emit correctly** — `Array<T>`, `Option<T>`, `Result<T,E>` map to Rust generics
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
Step 8: Generics (#48)                   → Array<T>, Option<T> emit correctly
```

Steps 1-4 achieve "hello world to binary." Steps 5-6 complete all 11 requirements. Steps 7-8 make it usable for real programs.

## Phase 2 Scope

- **LLVM IR backend** — one compiler, one proof chain
- **SMT solver integration** — Req 10 moves from runtime asserts to compile-time proofs
- **Native IFC analysis** — Req 11 moves from Rust newtypes to compiler-native flow checking
- **Self-hosting** — the MVL compiler rewritten in MVL
- **Model checker** (#37) — invariants, deadlock/livelock detection as compiler pass
- **WASM target** — sandboxed execution for The Cog and edge deployment

## Phase 3 Scope

- **Package manager** (#56) — dependency resolution, SBOM generation, trust decay
- **Extended stdlib** — networking, HTTP, crypto, database drivers
- **Assurance reports** (#73) — compiler emits per-module requirement satisfaction
- **Transpilation corpus** — seed for LLM training on MVL generation quality
- **AAE-4/5 integration** — automated evidence for certification frameworks

## Consequences

- Phase 1 accepts two-compiler friction as temporary cost
- Phase 2 requires building LLVM codegen — significant investment
- WASM target unlocks sandboxed execution for The Cog and edge deployment
- The transpilation corpus from Phase 1 seeds LLM training for Phase 3
