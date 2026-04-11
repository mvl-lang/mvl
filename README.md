# MVL — Minimum Verification Language

What if we turn things around? Code generation just became frictionless. LLMs write code in any language, at any verbosity, with any annotation burden — for free. So why are we still designing languages for humans to write? What if we designed a language that maximized everything a compiler can verify — type safety, memory safety, termination, data race freedom, information flow — and let the LLM handle the fact that it's verbose, heavily annotated, and ugly to write by hand?

That's the MVL. The smallest language where the compiler verifies the most. Not for human ergonomics — for maximum verification density per generated token.

## Why

Two forces converge:

- **Cybersecurity.** AI-speed attacks (Mythos: autonomous zero-day discovery for $50 in compute) need compile-time defenses. The MVL makes entire vulnerability classes — injection, secret leakage, buffer overflow, privilege escalation — structurally impossible. Code that Mythos would exploit doesn't compile.

- **Safety.** Mission-critical systems (avionics, industrial, automotive) require formal evidence. The MVL compiler generates that evidence automatically: every property proven at compile time is an audit artifact. The path from AAE-3 (spec-driven) to AAE-5 (externally certified).

## What

Eleven compiler-verified requirements. No existing language enforces all of them:

| # | Requirement | What the compiler proves |
|---|---|---|
| 1 | Type safety (ADTs) | No impossible states |
| 2 | Memory safety | No use-after-free, no buffer overflow |
| 3 | Totality (exhaustive match) | All cases handled |
| 4 | Null elimination (Option) | No null pointer dereference |
| 5 | Error visibility (Result) | All errors in the type signature |
| 6 | Ownership (linearity) | No double-free, no leaks |
| 7 | Effect tracking | Side effects visible in types |
| 8 | Termination checking | Functions provably halt (total by default) |
| 9 | Data race freedom | No concurrent access on shared mutable state |
| 10 | Refinement types | Values within valid ranges at compile time |
| 11 | Information flow control | Secret/tainted data tracked through types |

Code that compiles is **well-formed** (internal quality proven). Tests handle **validation** (external quality — does it do the right thing).

## How — Three Phases

### Phase 1: Prototype (MVL → Rust)

Transpile MVL to Rust. Fast iteration, proof of concept. Rust scores 7/11 — the highest of any mainstream language. The transpilation adds the remaining 4 requirements (termination, race freedom, refinements, IFC) as a layer on top of Rust's existing guarantees.

- Define the EBNF grammar
- Build a parser and type checker in Rust
- Transpile to Rust source, compile with `rustc`
- Borrow Rust's ecosystem (crates.io, cargo)
- Accept the two-compiler friction (MVL checker + Rust compiler)

**Milestone:** Compile the two reference examples (authentication handler, safe division with audit trail) and demonstrate all 11 requirements.

### Phase 2: Production (MVL → LLVM IR)

One compiler, one trust boundary, one proof chain. The MVL compiler verifies all 11 requirements and emits LLVM IR directly. No intermediate language with its own opinions.

- Build LLVM IR codegen (replaces Rust transpilation)
- All LLVM targets: ARM, x86, WASM, RISC-V
- Build the MVL stdlib natively (core: ~30 types, standard: ~200 functions)
- WASM target enables sandboxed execution (The Cog, edge, browser)

**Milestone:** Self-hosting — the MVL compiler compiles itself.

### Phase 3: Ecosystem

- MVL package manager
- Extended packages: HTTP, TLS, databases, YAML, advanced crypto
- Transpilation corpus for LLM training (MVL ↔ Rust/Haskell/Koka)
- IDE/CLI tooling (language server, formatter, test runner)
- Integration with OpenSpec for spec-driven development

**Milestone:** A real project built end-to-end in MVL with AAE-4 evidence generated automatically.

## Design Principles

1. **Explicit over implicit.** No hidden control flow, no implicit conversions.
2. **One way to do each thing.** One loop, one branch, one error mechanism.
3. **Vocabulary over syntax.** Stdlib functions, not macros or sugar.
4. **Total by default.** Functions must terminate. `partial` opts out.
5. **Immutable by default.** `mut` opts in.
6. **Effects in signatures.** Pure is the default.
7. **Security labels on all data.** `Public`, `Tainted`, `Secret`.
8. **Actors, not threads.** No shared mutable state, no locks, no deadlocks.
9. **Ownership, not GC.** Deterministic deallocation for real-time.
10. **Refinement types inline.** `x: Int where x > 0` is a first-class type.

## What the MVL Drops

No anonymous lambdas, no list comprehensions, no decorators, no operator overloading, no implicit conversions, no default arguments, no variadic arguments, no macros, no ternary operator, no string interpolation, no inheritance, no exceptions, no null, no global state, no `while` in total functions.

~10 statement forms. ~5 expression forms. ~3 declaration forms. The smallest general-purpose language.

## Research

Full research, EBNF grammar, code examples, language scorecard, OWASP mapping, and references: see `my-brain/study/mvl_research.md` and `my-brain/study/mvl_references.bib`.

## License

MIT
