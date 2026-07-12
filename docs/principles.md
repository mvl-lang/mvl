# Design Principles

MVL has nine design principles across two tiers.

- **Tier 1 — Meta principles** are the *why*. Cross-cutting philosophy that guides every decision. Every ADR in the corpus checks its proposal against these.
- **Tier 2 — Structural decisions** are the *what*. Concrete structural choices those principles produced. Each has its own anchor ADR.

Language-level choices like *total by default*, *immutable by default*, *effects in signatures*, *security labels*, *refinement types*, and *actors instead of threads* are **not** listed here as separate principles. They follow from Tier 1 combined with the eleven requirements (#6) — restating them would be redundant.

---

## Tier 1 — Meta principles

### 1. Explicit over implicit

No hidden control flow, no implicit conversions, no silent coercion. Every relevant property lives in the signature or the source.

Consequences: no default arguments, no operator overloading, no implicit numeric conversion, no ambient effects, no inference on `let` bindings, no bare `unwrap()`.

**Cited as:** "DP 1" / "Principle 1" across ADR-0017, ADR-0022, ADR-0024, ADR-0025, ADR-0026.

### 2. One syntax per concept

Each concept has exactly one expression. One loop, one branch, one error mechanism, one form of mutation. Regularity beats variety — the LLM benefits from a single pattern, the compiler benefits from fewer cases.

Consequences: no ternary (only `if/else`), no `switch` (only `match`), no `for` (only `while`) in `partial fn`, one loop form, one comment form.

**Anchors:** ADR-0002, ADR-0004, ADR-0005.

### 3. Vocabulary over syntax

Grow the stdlib, not the grammar. Compression comes from named, typed, verifiable functions the compiler understands — never from sugar it can't see through. The boundary between language and stdlib moves in one direction only: **stdlib grows, language doesn't.**

Consequences: no macros, no string interpolation, no list comprehensions. `format()` and `Map.get()` exist instead.

**Anchors:** ADR-0002 (compression model), `.openspec/language.md`.

### 4. The signature IS the threat model

Effects, IFC labels, ownership, refinements, termination, errors — all visible in the type signature. Reading a signature reveals the whole contract; nothing is ambient.

Consequences: effects follow `!`, labels wrap types (`Tainted[T]`, `Secret[T]`), reference capabilities are type-level (`val`, `ref`, `iso`, `tag`), errors surface via `Result[T, E]`, termination via `total` / `partial`.

**Anchors:** ADR-0001, ADR-0004.

### 5. Honest over silent

The compiler must either verify a property or reject the program. Never silently drop, accept, or defer. Post-Postel: parsers may accept multiple syntactic forms; validators must reject invalid input, never coerce it.

Consequences: no lossy default conversions, no silent label drops at stdlib boundaries, no fallback error handling that hides failure. Explicit `declassify` / `sanitize` for label transitions.

**Anchors:** ADR-0024 (label-transparent functions), ADR-0026 (input validation philosophy), the project `silent-drop-audit` skill.

---

## Tier 2 — Structural decisions

### 6. Eleven requirements — no more, no less

The compiler verifies exactly eleven properties:

1. Type safety (ADTs)
2. Memory safety
3. Totality (exhaustive match)
4. Null elimination (Option)
5. Error visibility (Result)
6. Ownership (linearity)
7. Effect tracking
8. Termination checking
9. Data race freedom
10. Refinement types
11. Information flow control

A twelfth is added only if it catches bugs no combination of the other eleven catches.

**Instantiates:** #1 (each requirement makes a property explicit in the type), #4 (the requirements *are* the threat model), #5 (each requirement is verified or the program is rejected).

**Anchor ADR:** [ADR-0001](adr/0001-eleven-requirements.md).

### 7. Language contraction

Features are dropped whenever they prioritise writability over verifiability. The MVL removes: mutable closures, list comprehensions, decorators, operator overloading, implicit conversions, default arguments, variadic arguments, macros, ternary operator, string interpolation, inheritance, exceptions, null, mutable-by-default bindings, global state, `break`, `continue`, trait objects / dynamic dispatch, and anonymous tuples.

Result: ~10 statement forms, ~5 expression forms, ~3 declaration forms.

**Instantiates:** #2 (removes multiple ways to express one concept), #3 (moves capabilities into the stdlib rather than the grammar).

**Anchor ADR:** [ADR-0002](adr/0002-language-contraction.md).

### 8. LL(1) grammar, hand-written recursive descent

Grammar fits in ~100 EBNF productions, LL(1), no lookahead beyond one token. Hand-written parser — no parser generator, no macros, no PEG.

**Instantiates:** #2 at the grammar level — LL(1) means every construct has one unambiguous parse.

**Anchor ADR:** [ADR-0005](adr/0005-recursive-descent-parser.md).

### 9. Ownership, not GC

Memory safety via linearity and reference capabilities (`val`, `ref`, `iso`, `tag`) adapted from Pony. No garbage collector, no tracing runtime. Deterministic deallocation, suitable for real-time and safety-critical use.

**Instantiates:** Req 6 in #6 (ownership is one of the eleven), #4 (lifetime is in the type, not hidden in a runtime).

**Anchor ADR:** [ADR-0029](adr/0029-pony-reference-capability-adaptation.md).

---

## At a glance

| # | Principle                                    | Tier       | Anchor ADR |
|---|----------------------------------------------|------------|------------|
| 1 | Explicit over implicit                       | Meta       | 0017 / 0026 |
| 2 | One syntax per concept                       | Meta       | 0002 / 0004 |
| 3 | Vocabulary over syntax                       | Meta       | 0002       |
| 4 | The signature IS the threat model            | Meta       | 0001 / 0004 |
| 5 | Honest over silent                           | Meta       | 0024 / 0026 |
| 6 | Eleven requirements — no more, no less       | Structural | 0001       |
| 7 | Language contraction                         | Structural | 0002       |
| 8 | LL(1) grammar, hand-written recursive descent | Structural | 0005       |
| 9 | Ownership, not GC                            | Structural | 0029       |

## What's not on this list

The following are **outputs** of the principles above, not principles themselves. They are captured in specs and ADRs:

| Choice                          | Captured in                                 |
|---------------------------------|---------------------------------------------|
| Total by default                | Spec 013, Req 8                              |
| Immutable by default (`ref`)    | Spec 001, Req 6                              |
| Effects in signatures           | Spec 002, Req 7, ADR-0035                    |
| Security labels on all data     | Spec 003, Req 11, ADR-0017 / 0024 / 0036     |
| Refinement types inline         | Spec 018, Req 10, ADR-0025                   |
| Actors, not threads             | Spec 015, Req 9, ADR-0029 / 0037             |
| No bare `unwrap()`              | Stdlib policy, follows from #1               |

If you find yourself proposing a new "principle" that is really one of these, capture it in the relevant spec/ADR instead. The principles page stays small on purpose.
