# ADR-0031: No Uniform Function Call Syntax (UFCS)

**Status:** Accepted
**Date:** 2026-05-16
**Issues:** N/A (design clarification)

---

## Context

Uniform Function Call Syntax (UFCS) allows `x.f(y)` to be equivalent to `f(x, y)`. Languages like Nim and D support this, enabling method-like chaining without explicit method declarations.

The question arose whether MVL should support UFCS to enable fluent APIs:

```mvl
// Without UFCS (current)
let result = save(validate(transform(parse(input))))

// With UFCS
let result = input.parse().transform().validate().save()
```

Arguments for UFCS:
- Left-to-right reading matches data flow direction
- Editor autocomplete works after `.` (type-aware suggestions)
- Familiar to developers from OOP languages
- Enables fluent APIs without `impl` blocks or traits

Arguments against UFCS:
- Resolution depends on what functions are in scope (implicit)
- Creates two syntaxes for the same operation (violates one-way principle)
- LLM code generation must track import scope to resolve calls
- Scope-dependent resolution is "spooky action at a distance"

---

## Decision

**MVL does not support UFCS.** All function calls use explicit syntax: `f(x, y)`.

1. **No method syntax for standalone functions.** The expression `x.f()` is not valid for standalone functions. Only actor behaviors use dot syntax: `actor_ref.behavior()`.

2. **Chaining uses explicit intermediates.** The idiomatic MVL pattern is:
   ```mvl
   let parsed = parse(input)
   let transformed = transform(parsed)
   let validated = validate(transformed)
   let result = save(validated)
   ```

3. **Autocomplete is a tooling concern.** LSP implementations can provide "functions accepting this type as first parameter" without language-level UFCS.

4. **Stdlib naming is explicit.** Functions are named to be clear without method context: `map(list, f)` not `list.map(f)`. Type signatures disambiguate overloads.

---

## Consequences

### Positive

1. **Unambiguous resolution.** `f(x)` always calls the function named `f`. No scope analysis needed.

2. **LLM-friendly generation.** Code generators don't need to track imports to determine which function `x.f()` resolves to.

3. **One way to call functions.** No stylistic debates about `f(x)` vs `x.f()`.

4. **Simpler mental model.** Functions are functions. Methods exist only on actors.

### Negative

1. **Verbose chaining.** Deep pipelines require intermediates or nested calls.

2. **No dot-autocomplete for functions.** Developers can't type `x.` and see available functions (tooling can mitigate).

3. **Unfamiliar to OOP developers.** `map(list, f)` instead of `list.map(f)`.

### Follow-up work

- Document the intermediate-variable pattern in style guide
- Ensure LSP provides function suggestions based on first parameter type
- Consider pipe operator `|>` as separate future decision (currently also rejected)

---

## Rejected Alternatives

### Alternative A: Full UFCS (Nim/D style)

`x.f(y)` resolves to `f(x, y)` for any function `f` in scope.

**Rejected because:**
- Violates "explicit over implicit" — resolution depends on imports
- Violates "one way to do each thing" — two syntaxes for same call
- Scope-dependent resolution is LLM-hostile

### Alternative B: Type-directed method syntax

Add `x.method()` only for type-attached functions, but not UFCS for free functions.

**Rejected because:**
- Creates two kinds of functions (standalone vs type-attached)
- Requires `impl` blocks or similar mechanism
- MVL already has actor behaviors for stateful method syntax
- Adds complexity without sufficient benefit

### Alternative C: Pipe operator only

Add `|>` for chaining without UFCS: `x |> f |> g |> h`.

**Rejected because:**
- Still creates two syntaxes: `f(x)` and `x |> f`
- Intermediate variables serve the same purpose with better debuggability
- Each intermediate is named, making data flow explicit

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

This decision does not directly affect any of the eleven compiler-verified requirements. UFCS is purely syntactic sugar — it changes how functions are called, not what guarantees the type system provides.

All eleven requirements: **unchanged**.

### Design Principles (README)

| Principle | Relation |
|-----------|----------|
| 1. Explicit over implicit | **strengthens** — `f(x)` is unambiguous, no scope-dependent resolution |
| 2. One way to do each thing | **strengthens** — only one syntax for function calls |
| 3. Vocabulary over syntax | **consistent with** — no new syntax added |
| 4. Total by default | consistent with |
| 5. Immutable by default | consistent with |
| 6. Effects in signatures | consistent with |
| 7. Security labels on all data | consistent with |
| 8. Actors, not threads | consistent with |
| 9. Ownership, not GC | consistent with |
| 10. Refinement types inline | consistent with |

### Specifications

No specs in `.openspec/specs/` are affected. This ADR documents a design constraint, not a feature implementation.

The modularization documentation (`work/projects/mvl/modularization.md` in the knowledge base) already lists UFCS under "What MVL Does NOT Have" — this ADR provides the formal rationale.

---

## References

- Nim UFCS: https://nim-lang.org/docs/manual.html#procedures-method-call-syntax
- D UFCS: https://dlang.org/spec/function.html#pseudo-member
- Go receiver syntax (explicit, not UFCS): https://go.dev/tour/methods/1
- Rust impl blocks (explicit, not UFCS): https://doc.rust-lang.org/book/ch05-03-method-syntax.html
