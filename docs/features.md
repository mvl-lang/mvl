# MVL Language Features

25 features across 7 categories. The language is deliberately small (ADR-0004) — every feature exists to increase verification density per token.

---

## Type System

### 1. Algebraic Data Types (Req 1)

Sum types (enums) and product types (structs) with exhaustive pattern matching. Invalid states are unrepresentable — adding an enum variant forces every match site to update. Foundation for all other requirements.

### 2. Generics with Const Parameters

Type parameters (`List[T]`) and const-generic fixed-size arrays (`Array[T, N]`). Monomorphized at transpile time — each concrete instantiation gets its own Rust code. Constraint enforcement on generics is Phase 4 (#225).

### 3. Refinement Types (Req 10)

Value-level predicates on types: `Int where x > 0`, `Array[T] where len(self) > 0`. Currently symbolic proof for literals, runtime checks otherwise. SMT integration deferred. Gets you "right type AND right value."

### 4. Security Labels / IFC (Req 11)

Four-level lattice: `Secret > Tainted > Clean > Public`. Data flows up freely; flowing down requires explicit `declassify()` or `sanitize()`. Implicit flow analysis detects leakage through control flow. No mainstream language has this at compile time.

---

## Safety

### 5. Memory Safety (Req 2)

Ownership tracking with use-after-move detection. Passing a value transfers ownership; using it after is a compile error. Currently implemented via clone-on-pass to Rust — correct but not zero-copy. Full borrow lifetimes in Phase 5 (#234).

### 6. Null Elimination (Req 4)

No null. Absence is `Option[T]` — `Some(value)` or `None`. Accessing the inner value requires pattern matching. The billion-dollar mistake is a compile error.

### 7. Error Path Visibility (Req 5)

Functions that can fail return `Result[T, E]`. Error type is in the signature. Ignoring a Result is a compile error. `?` propagates errors explicitly. No hidden exception paths.

### 8. Resource Linearity (Req 6)

Values have exactly one owner. `move` transfers ownership. Linear resources (files, connections) must be explicitly consumed. Absorbs session types (Req 12) via typestate pattern — no separate feature needed.

### 9. Totality / Exhaustive Matching (Req 3)

Every `match` must cover all cases. The compiler rejects incomplete logic. Adding an enum variant breaks every match that doesn't handle it. Types become guarantees, not documentation.

---

## Verification

### 10. Effect Tracking (Req 7)

Side effects declared in function signatures: `! Console`, `! FileRead`, `! Net`, `! DB`, etc. 13 fine-grained effects. Pure functions (no `!`) are provably pure — the compiler rejects any I/O in them. Effects tell you the *class* of action; capability labels (Req 11, IFC) tell you *which* resource (#931).

### 11. Termination Checking (Req 8)

Functions are total by default. The compiler verifies termination via structural recursion and integer decrement patterns. `partial fn` opts out for servers and event loops. `while` only in `partial` functions. LLMs generate the structural form; humans couldn't be bothered.

### 12. Data Race Freedom (Req 9)

Reference capabilities: `iso` (isolated, sendable), `val` (deeply immutable), `ref` (shared mutable), `tag` (type-state). Only `iso` and `val` cross actor boundaries. Alias checking on `iso` values. Full proof requires actors (Phase 6).

---

## Compilation

### 13. Rust Transpiler Backend

MVL source → Rust source → `cargo build` → native binary. The transpiler emits readable, idiomatic Rust. Rust's ecosystem is the runtime: anything Rust can do, MVL can call via `extern "rust"`. Rust's borrow checker provides a second verification pass on requirements 1–6 (types, memory, exhaustiveness, Option, Result, ownership). The transpiler is the production backend today.

### 14. LLVM Backend

MVL source → LLVM IR → native binary. Direct IR generation with no Rust in the chain. This is the strategic backend: one compiler, one trust chain, full control over all eleven requirements in the emitted IR. Effects, totality, and refinements that are doc-comments in the Rust backend become static IR-generation errors here. SMT-verified refinements (Z3/CVC5) replace `debug_assert!`. The two backends run in parallel — the same MVL source compiles to both, and the compiler's test suite differentially fuzzes them against each other to catch divergences. See [How MVL Compiles](compilation-model.md) for the full breakdown.

### 15. Five-Stage Compiler Pipeline

The MVL compiler runs exactly five stages in order:

1. **Parse** — source → AST (recursive descent, LL(1), no backtracking)
2. **Resolve** — imports, modules, stdlib linking; produces scoped AST
3. **Check** — type checking + all eleven compile-time guarantees; every requirement has its own pass
4. **Passes** — analysis passes over the checked AST: coverage instrumentation, MC/DC encoding, mutation testing, linting
5. **Emit** — target-specific emission: Rust source (backend 1) or LLVM IR (backend 2)

Every stage is independent. `mvl check` stops after stage 3. `mvl lint` runs through stage 4. `mvl build` runs all five. The separation means passes and backends can evolve independently.

### 16. FFI via `extern "rust"` (ADR-0006)

Typed trust boundary. Extern functions declare their effects and IFC labels. The assurance report counts verified vs trusted code. The boundary is greppable, auditable, and visible in every function signature.

### 17. Module System

Multi-file programs with `module` and `use` imports. Resolver validates imports; transpiler emits Rust modules. Stdlib uses tiered imports: core (implicit prelude), standard (`use std.*`), extended (`use pkg.*`).

---

## Tooling

### 18. Assurance Reports

`mvl assurance` generates a per-module verdict matrix: 11 requirements × modules. Each requirement gets Proven / Failed / Unchecked / Timeout. The verified-vs-trusted ratio is the metric — how much of your program is proven, how much is extern.

### 19. Linter (3 phases)

Phase 1: style rules. Phase 2: semantic rules. Phase 3: LLM corpus quality + complexity rules (cyclomatic complexity, match depth, effect width, trait impl count, module fanout, extern ratio). The linter enforces regenerability.

### 20. Test Transpilation

`test fn` in MVL source transpiles to `#[cfg(test)]` + `#[test]` in Rust. Internal tests (private API) are P-layer; external tests (`_test.mvl`) are E-layer evidence that survives regeneration. ISPE separation by design.

---

## Verification Philosophy

### 21. No Proof Language — The Compiler Proves or You Mark `partial`

The programmer never writes proofs, tactics, or verification annotations beyond types and refinements. The compiler runs 11 verification passes automatically on every function. If a pass can't prove a property, it reports the gap — not a compile error demanding a proof. For termination (Req 8), the escape hatch is `partial fn`. For refinements (Req 10), unprovable predicates fall back to runtime checks. This is the opposite of Lean/Coq/Dafny where the programmer assists the prover. In MVL, the prover is on its own.

---

## Design Principles

### 22. Language Contraction (ADR-0002)


No macros, no exceptions, no inheritance, no null, no while (in total functions), no operator overloading, no implicit conversions. The language shrinks by policy. Every removed feature is a verification obstacle eliminated.

### 23. Transpiler-Mediated Codegen (ADR-0013)

No macros, no reflection. When a feature requires compile-time struct iteration (derives, `parse[T]()`, serialization), the transpiler generates it from type definitions. The type IS the spec; the transpiler IS the generator; the checker IS the verifier. Third path between macros and reflection.

### 24. One Way (ADR-0004)

One error type (`Result`), one absence type (`Option`), one loop form (`for`), one branching form (`match`/`if`). Stdlib provides vocabulary, not syntax. The smallest language that enforces all 11 requirements.

---

## Contracts

### 25. Struct Invariants — `with invariant` (Req 10, ADR-0025)

Structs may carry a cross-field predicate verified at construction and mutation:

```
type DateRange = struct {
    start: Date,
    end: Date,
} with invariant start <= end
```

The compiler injects a check at every construction site and every mutation of `ref`-bound struct fields. Invalid construction is a compile error for literal values; a `CheckError::InvariantViolation` at runtime for dynamic inputs. This is SPARK-style cross-field precondition checking — no proof language required, no annotation burden beyond the `with invariant` clause. Shipped in v0.97.0 (#654).
