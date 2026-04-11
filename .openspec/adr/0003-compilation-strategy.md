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

## Milestones

- **Phase 1:** Compile the two reference examples (auth handler, safe division). Demonstrate all 11 requirements via Rust transpilation.
- **Phase 2:** Self-hosting — the MVL compiler compiles itself to LLVM IR.
- **Phase 3:** A real project built end-to-end in MVL with AAE-4 evidence generated automatically.

## Consequences

- Phase 1 accepts two-compiler friction as temporary cost
- Phase 2 requires building LLVM codegen — significant investment
- WASM target unlocks sandboxed execution for The Cog and edge deployment
- The transpilation corpus from Phase 1 seeds LLM training for Phase 3
