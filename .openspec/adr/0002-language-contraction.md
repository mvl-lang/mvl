# ADR-0002: Language Contraction — What to Drop and Why

**Status:** Accepted
**Date:** 2026-04-11
**Context:** The MVL is designed for LLM generation and compiler verification, not human ergonomics. Which language features should be deliberately excluded?

## Decision

The MVL drops every feature that exists for writability over readability. One way to do each thing. The LLM doesn't benefit from syntactic variety — it benefits from regularity. The compiler benefits from explicitness.

## What is dropped

| Dropped | Origin of the decision | MVL alternative |
|---------|----------------------|-----------------|
| Mutable closures | — | Lambdas with immutable captures only (`\|x\| expr`). Mutable captures violate Req 7 (hidden state). |
| List comprehensions | — | `list.map(f).filter(g)` chains |
| Decorators | — | Explicit wrapper functions |
| Operator overloading | Go (2009) | Named methods: `matrix.add(other)` |
| Implicit conversions | — | Explicit: `to_float(x)` |
| Default arguments | — | Overloaded names or `Option` params |
| Variadic arguments | — | `List<T>` for N args |
| Macros | — | Stdlib functions (vocabulary over syntax) |
| Ternary operator | — | `if expr { a } else { b }` |
| String interpolation | C sprintf (1972), Go fmt (2009), Perl taint (1989) | Explicit `format()` with IFC-typed args |
| Inheritance | Rust traits (2015), Haskell typeclasses (1989), GoF (1994) | Composition + traits only |
| Exceptions | Rust Result (2015), Haskell Either (1990s) | `Result<T,E>` only |
| Null | SML option (1990), Hoare recanted (2009) | `Option<T>` only |
| Mutable by default | Haskell (1990), Rust (2015) | Immutable default, `mut` opts in |
| Global state | E language (1997), Pony (2015), Koka (2014) | All state passed explicitly |
| `while` in total functions | Idris 2 (2021), Lean 4 (2021) | `for` with bounded iterators; `while` only in `partial` fns |

## What survives

~10 statement forms, ~5 expression forms, ~3 declaration forms:

`fn`, `let`/`let mut`, `if`/`else`, `match`, `for`, `return`, `.method()`, `?`, `|x| expr` (immutable-capture lambda), `type` (struct/enum), `module`.

Compare: Python ~30 statement forms, Rust ~20, Go ~15.

## The paradox

Dropping features makes the language more powerful, not less. Every dropped feature is a dropped ambiguity. Every dropped ambiguity is a property the compiler can now verify. Smaller language, stronger verification.

## Compression model

Two kinds of compression:
- **Syntax compression (dropped):** Lambdas, comprehensions, sugar. Hides semantics from the compiler. Bad compression.
- **Vocabulary compression (stdlib):** `Map.get()` → `Option<T>`, `format()` with IFC labels. Compresses through named, typed, verifiable functions. Good compression.

Compress through vocabulary (library functions the compiler understands), not through syntax (sugar the compiler can't see through).

## Consequences

- The MVL has the smallest surface area of any general-purpose language.
- LLM interpolation improves (fewer patterns to learn).
- Verification density increases (fewer constructs to check).
- Human developers will find it verbose — that's the point. The LLM writes it.
