# ADR-0053: No Rustc Concepts in MVL Grammar

**Status:** Accepted
**Date:** 2026-07-09
**Issues:** #1707

---

## Context

While working through the corpus test-crate errors (#1707), we found that
`std/lists.mvl` and `std/collections.mvl` were annotated with Rust-style
trailing trait bounds:

```mvl
pub fn List[T]::map[U](self, f: fn(T) -> U) -> List[U] where T: Clone, U: Clone { ... }
pub fn Set[T]::filter(self, f: fn(T) -> Bool) -> Set[T] where T: Clone { ... }
```

None of `Clone`, `Ord`, or `Eq` are MVL concepts.  MVL's ownership model
is Pony's four capabilities (`iso`, `val`, `ref`, `tag`, per ADR-0029) —
not Rust's `Clone` / `Copy` axis.  MVL has no trait system, no user
`impl Trait for Type` mechanism, and no way to declare that a user type
satisfies `Clone`.

Yet the parser accepted the syntax, threaded it through TIR, and the Rust
backend re-emitted it verbatim into the generated crate.  It "worked" only
because rustc happened to interpret the identifiers the way the stdlib
author intended.

Verification: a bogus bound compiles cleanly:

```mvl
fn foo[T](x: T) -> T where T: BananaBread { x }
// mvl check: OK  (9/11 requirements proven)
```

MVL check never validated `BananaBread` because MVL has no concept of it.

The `where T: Trait` grammar was Rust vocabulary bolted onto MVL syntax
with no semantic backing on the MVL side.  The one exception — the
checker's `MissingConstraint` for `T: Ord` on comparison operators
(`src/mvl/checker/infer.rs:670`) — is itself a leak: it only exists
because rustc happens to enforce Ord and MVL had no other way to declare
"generic T must be comparable".

## Decision

1. **`where` in MVL means exactly one thing: a predicate the solver must
   discharge.**  Refinement predicates on parameters (`n: Int where self >= 1`),
   on return types (`-> Int where self > 0`), on struct fields
   (`x: Int where self > 0`), and on type aliases
   (`type PositiveInt = Int where self > 0`).  All feed the Z3-backed
   solver via the RefExpr grammar.

2. **The trailing trait-bound `where` clause on fn signatures is deleted
   from the grammar.**  `parse_where_constraints` is removed.  The
   `constraints: Vec<Constraint>` field on `FnDecl` is removed.
   `where T: Trait` on a fn is now a parse error.

3. **Comparisons on unbounded generic type parameters become a compile-time
   error.**  Without a trait system, MVL cannot express "T is comparable".
   `fn cmp[T](a: T, b: T) -> Bool { a < b }` is rejected with a
   `ComparisonOnGenericType` error.  Users comparing must either specialise
   to a concrete comparable primitive or express the operation via a
   builtin method (`List::sort`, etc.) whose implementation handles the
   comparability internally.

4. **Stdlib is cleaned up**: `where T: Clone, U: Clone` etc. on
   `List::map`, `Set::filter`, etc. are stripped.  They were never
   enforced by MVL and never load-bearing for correctness (the Rust emit
   pattern that required them is a separate bug tracked in the same
   ticket, addressed independently).

## Rationale

The design principle from ADR-0002 ("Language Contraction") applies:
**one way to do each thing**.  Having `where` mean two different things
in the same production violates this.  Having it silently accept Rust
vocabulary and pass it through to rustc is worse: it lets rustc concepts
leak back into MVL, contradicting MVL's independence from any particular
backend.

MVL is meant to be a source of truth for both a Rust backend and an LLVM
backend, and it is meant to be self-hostable (ADR-0044).  Every concept
in the language must have a definition inside MVL.  Rust's `Clone` is
not defined in MVL; therefore it may not appear in MVL source.

## Consequences

**Positive**

- `where` has one meaning throughout MVL: solver-discharged predicate.
- New backends can be added without maintaining a Rust-trait-to-target
  mapping (there is nothing to map — the concepts don't exist in MVL).
- The stdlib documents only what MVL enforces; nothing decorative.
- LLM code generation is less confused — the grammar no longer suggests
  a trait system that isn't there.

**Negative**

- Any prior corpus / user code with `fn foo[T]() where T: Clone { … }`
  must be rewritten.  A grep of the current tree finds ~20 sites in
  stdlib and 2 in corpus (`02_functions/functions.mvl:114,163`).
  All are removed as part of landing this ADR.
- Comparisons on generic T become a hard error.  The existing wrapper
  `fn sort[T](xs: List[T]) -> List[T] where T: Ord { xs.sort() }`
  cannot be written this way.  Options for users needing sort on generic
  data: call `List::sort` directly on a concrete instantiation, or
  wait for a proper user-facing trait system (out of scope for MVL 1.0).

## Follow-up

- HOF inlining in the Rust backend still relies on an implicit
  `T: Clone` requirement (the `xs.into_iter().map(|x| f(x.clone())).collect()`
  pattern in `emit_method_call.rs`).  That is a separate bug — the emit
  should not require Clone MVL never declared.  Tracked under #1707
  as a distinct phase (post-cleanup).

- `capability_params_for_tir_fn` inference borrows params whose only use
  is a method call, misinterpreting consuming methods as reads.  Also a
  distinct emit-side fix.

## Rejected Alternatives

- **Keep `where T: Trait` but validate the bound name.**  MVL still
  wouldn't have a semantic meaning for `T: Clone` — validation would just
  produce more centralised bookkeeping of Rust concepts.  Doesn't remove
  the leak, just formalises it.
- **Introduce an MVL trait system.**  Out of scope; MVL is a small
  language (ADR-0004) and traits are the largest single feature Rust
  adds to a language.  If needed later, do it deliberately with its own
  ADR.

## Enforcement

The parser change is the enforcement.  A CI grep of stdlib and corpus
for `where [A-Z][a-zA-Z]*:` catches regressions before they reach a
review.  Added as a `make lint-no-rustc-leaks` target.
