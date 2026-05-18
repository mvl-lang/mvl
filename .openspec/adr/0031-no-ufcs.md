# ADR-0031: No Uniform Function Call Syntax (UFCS)

**Status:** Amended
**Date:** 2026-05-16 (amended 2026-05-18)
**Issues:** #868

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

However, MVL **does** support type-attached methods via `fn TypeName::method(self, ...)` syntax (#868). These are explicitly distinct from UFCS — see "Amendment" below.

1. **No UFCS for standalone functions.** The expression `x.f()` is not valid for arbitrary standalone functions. Only actor behaviors and type-attached methods use dot syntax.

2. **Type-attached methods use `fn TypeName::method(self, ...)`**:
   ```mvl
   fn Logger::info(self, msg: String) -> Unit ! Console { … }

   let log = Logger { min_level: Level::Info }
   log.info("started")   // ✓ method call
   info(log)             // ✗ not a valid standalone call
   ```

3. **Chaining uses explicit intermediates.** The idiomatic MVL pattern for standalone functions remains:
   ```mvl
   let parsed = parse(input)
   let transformed = transform(parsed)
   let validated = validate(transformed)
   let result = save(validated)
   ```

4. **Autocomplete is a tooling concern.** LSP implementations can provide "functions accepting this type as first parameter" without language-level UFCS.

5. **Stdlib naming is explicit.** Free functions are named to be clear without method context: `map(list, f)` not `list.map(f)`. Type signatures disambiguate overloads.

---

## Consequences

### Positive

1. **Unambiguous resolution.** `f(x)` always calls the function named `f`. No scope analysis needed.

2. **LLM-friendly generation.** Code generators don't need to track imports to determine which function `x.f()` resolves to. For type-attached methods, `x.method()` resolves only if `fn Type::method(self)` is declared — still deterministic.

3. **One way to call each thing.** Standalone functions: `f(x)`. Type-attached methods: `x.method()`. Actor behaviors: `actor.behavior()`. No ambiguity.

4. **Type-attached methods are lightweight.** No impl blocks, no traits, no separate declaration context. `fn Logger::info(self, ...)` is a top-level declaration that is clearly attached to `Logger`.

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

**Originally rejected, now accepted as `fn TypeName::method(self, ...)` (#868).**

The original concerns are addressed by the chosen syntax:
- "Two kinds of functions" is intentional: methods and standalone functions are explicitly different, not ambiguous
- No `impl` blocks needed: `fn Logger::info(self, ...)` is a top-level declaration
- Actors remain the pattern for stateful/async behavior; type-attached methods serve stateless data-holder types
- Net benefit: enables fluent APIs on structs without actor overhead (Logger, Builder, Money, etc.)

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

---

## Amendment: Type-attached methods (#868, 2026-05-18)

Alternative B was accepted. The syntax `fn TypeName::method(self, ...)` is now part of MVL.

### Why this is NOT UFCS

| UFCS (rejected) | Type-attached methods (accepted) |
|-----------------|----------------------------------|
| `x.f()` resolves to ANY `f(x, ...)` in scope | `x.method()` only resolves if `fn Type::method(self)` declared |
| Scope-dependent resolution | Explicit type attachment |
| Import changes can change resolution | Method is part of type definition |
| Two ways to call same function | Methods and functions are distinct |

```mvl
fn Logger::info(self, msg: String) -> Unit ! Console { }  // Method
fn log_info(l: Logger, msg: String) -> Unit ! Console { } // Function

logger.info("msg")   // ✓ method call
logger.log_info()    // ✗ no such method
log_info(logger)     // ✓ function call
info(logger)         // ✗ no such function
```

### Implementation

| Component | Change |
|-----------|--------|
| AST | `FnDecl.receiver_type: Option<String>` |
| Parser | Parses `fn TypeName::method(self, ...)` |
| Checker | Method table per type; resolves `x.method()` via receiver type |
| Rust backend | Emits `impl Type { fn method(&self, ...) }` |
| LLVM backend | Emits `Type_method(self, ...)` with mangled name |

---

## References

- Go receiver syntax (explicit, not UFCS): https://go.dev/tour/methods/1
- Rust impl blocks (explicit, not UFCS): https://doc.rust-lang.org/book/ch05-03-method-syntax.html
- Issue #868: type-attached methods proposal
