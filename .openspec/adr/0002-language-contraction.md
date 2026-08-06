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
| Variadic arguments | — | `List[T]` for N args |
| Macros | — | Stdlib functions (vocabulary over syntax) |
| Ternary operator | — | `if expr { a } else { b }` |
| String interpolation | C sprintf (1972), Go fmt (2009), Perl taint (1989) | Explicit `format()` with IFC-typed args |
| Inheritance | Rust traits (2015), Haskell typeclasses (1989), GoF (1994) | Composition + traits only |
| Exceptions | Rust Result (2015), Haskell Either (1990s) | `Result[T,E]` only |
| Null | SML option (1990), Hoare recanted (2009) | `Option[T]` only |
| Mutable by default | Haskell (1990), Rust (2015) | Immutable default, `mut` opts in |
| Global state | E language (1997), Pony (2015), Koka (2014) | All state passed explicitly |
| `while` in total functions | Idris 2 (2021), Lean 4 (2021) | `for` with bounded iterators; `while` only in `partial` fns |
| `break` | — | Extract loop body into function with early `return`. `break` undermines `decreases` termination proofs — the checker can't prove the bound is reached if `break` exits early. |
| `continue` | — | `if` condition inverted: `if cond { skip }` → `if !cond { process }`. `continue` is a goto in disguise — non-local control flow inside a loop body. |
| Trait objects / dynamic dispatch | Rust `dyn Trait` (2015), Java interfaces (1995) | Static dispatch only. All generics are monomorphized (ADR-0034). No `dyn`, no vtables, no type erasure. |
| Per-field visibility | Rust `pub(crate)` fields (2015) | Struct fields are always public. Access control is at the module boundary (`pub fn`), not the field level. Simplifies codegen and verification. |
| Anonymous tuples (`(Int, String)`, `.0`, `.1`) | ML (1973), Python (1991), Rust (2010), Go (multiple returns, 2009) | Named structs. `Indexed[T]`, `Pair[A,B]`, `Partitioned[T]`, `Entry[K,V]` for stdlib pairs; user code declares its own (#1380, 2026-06). |

### Design note: tuples removed

Anonymous tuples were briefly available (#1366) and then removed (#1380, 2026-06) before adoption spread. Five reasons:

1. **Implicit meaning violates "Explicit over Implicit"** — `(Int, Int)` could be `(x, y)`, `(width, height)`, or `(min, max)`. `.0` says nothing; `.x` does.
2. **Safety hazard** — Swapped destructuring (`let (min, max) = bounds()` vs the function returning `(max, min)`) is a latent defect the compiler can't catch.
3. **LLM-unfriendly** — Field names are self-documenting context for token interpolation. Positional access is not.
4. **Auditability** — `result.error_code` traces back to a spec line; `result.0` does not.
5. **Refactoring hazard** — Adding a field to a tuple silently breaks every positional access.

If your data has meaning, give it a name. Stdlib pairs (enumerate, zip, partition, entries, env.all) return named records; user code does the same. The `for (a, b) in xs` shorthand is replaced by `for item in xs { let a = item.field; … }` — slightly more verbose, which is the point.

### Design note: `while` and termination

MVL has both `while` and `for` (see "What survives" below and Spec 001 Req 11/13) — `for` iterates a finite `Iterator`, `while` is the general-purpose loop. A bare `while` (no measure) is restricted to `partial fn` because an unbounded loop cannot, in general, be proven to terminate. In `total fn`, a loop must either be a `for` over a finite iterator, use structurally-decreasing recursion, or carry an explicit `decreases` clause on the `while` that the checker can verify. See Spec 013 (Termination) for the enforcement details, and its Amendment below (#2218) for the correction to this note.

## What survives

~10 statement forms, ~5 expression forms, ~3 declaration forms:

`fn`, `let`/`let mut`, `if`/`else`, `match`, `for`, `return`, `.method()`, `?`, `|x| expr` (immutable-capture lambda), `type` (struct/enum), `use` / `pub use` (imports and re-exports). Note: there are no inline `module Foo { }` blocks — one file equals one module (see ADR-0002 context and Spec 005).

Compare: Python ~30 statement forms, Rust ~20, Go ~15.

## The paradox

Dropping features makes the language more powerful, not less. Every dropped feature is a dropped ambiguity. Every dropped ambiguity is a property the compiler can now verify. Smaller language, stronger verification.

## Compression model

Two kinds of compression:
- **Syntax compression (dropped):** Lambdas, comprehensions, sugar. Hides semantics from the compiler. Bad compression.
- **Vocabulary compression (stdlib):** `Map.get()` → `Option[T]`, `format()` with IFC labels. Compresses through named, typed, verifiable functions. Good compression.

Compress through vocabulary (library functions the compiler understands), not through syntax (sugar the compiler can't see through).

## Consequences

- The MVL has the smallest surface area of any general-purpose language.
- LLM interpolation improves (fewer patterns to learn).
- Verification density increases (fewer constructs to check).
- Human developers will find it verbose — that's the point. The LLM writes it.

## Amendment: `for` shipped; `while` in `total fn` is conditionally allowed (#2218, 2026-08-06)

The original "Design note: `while` and termination" above claimed `while` was
the only loop form and was unconditionally restricted to `partial fn`. Both
claims went stale without the ADR being updated: `for` shipped as a
first-class construct (Spec 001 Req 11/13, `grammar.ebnf`) — the "What
survives" list two sections below this ADR's own decision already listed it
— and `while … decreases m` became legal in `total fn` once the checker
gained a per-iteration measure-verification pass (#628; enforced in
`src/mvl/checker/stmts.rs` and `src/mvl/checker/contracts/loop_and_field.rs`,
specified in `.openspec/specs/013-termination/spec.md`). The design note
above has been corrected in place; this amendment exists so the drift is
traceable.
