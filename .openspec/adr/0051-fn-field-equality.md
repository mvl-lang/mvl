# ADR-0051: Equality on Structs Containing Function-Typed Fields

**Status:** Accepted (2026-07-04)
**Date:** 2026-07-04
**Issues:** #1660 (this decision), #1656 (crate-level allow-list audit)
**Related:** ADR-0026 (Post-Postel strictness), ADR-0044 (self-hosting translatability)

---

## Context

MVL's Rust emitter currently applies `#[derive(Debug, Clone, PartialEq)]`
unconditionally to every emitted struct (`src/mvl/backends/rust/emit_types.rs:361`).
When a struct contains a function-typed field (`fn(A) -> B`), the derived
`PartialEq` implementation compares those fields by function-pointer identity —
which Rust flags as `unpredictable_function_pointer_comparisons` because the
comparison is implementation-defined (the linker may deduplicate identical
functions, or vary addresses across builds).

The emitter currently silences this warning via a crate-level
`#![allow(unpredictable_function_pointer_comparisons)]` header.  That allow is
one of the entries the umbrella audit in #1656 is trying to eliminate — but
this specific case isn't a lint-noise problem, it's an **unresolved semantic
question about MVL**.

## The Question

Should MVL support `==` between struct values whose type contains a function
field?  If yes, with what semantics?  If no, how is the restriction surfaced?

### Options considered

| Option | Where enforced | Error surface |
|---|---|---|
| **(a)** Emitter skips `PartialEq` derive when a struct has an fn field | Rust codegen | Cryptic `trait bound PartialEq not satisfied` at Rust build |
| **(a')** Type checker rejects `==` on structs with fn fields | MVL checker | Clean MVL error at check time |
| **(b)** Emitter emits a manual `PartialEq` that panics on fn-field compare | Runtime | Unpredictable runtime crash |
| **(c)** Emitter emits a manual `PartialEq` that ignores fn fields | Runtime | Silent — equal-by-`==` values may behave differently |
| **(d)** Forbid function-typed struct fields entirely | MVL checker | Clean error at declaration site |

### Facts from the codebase (2026-07-04)

- **Zero** MVL structs in `tests/corpus/`, `std/`, or `examples/` currently declare a function-typed field.
- **Zero** existing tests rely on `==` semantics for fn-field structs.
- The type checker (`src/mvl/checker/infer.rs:649–698`) already validates the
  `Eq` constraint on type parameters — the concept "some types support `==`,
  others do not" is present, just not applied to concrete types beyond
  parametric bounds.
- The emitter's unconditional derive is the only barrier keeping the language
  compilable with fn-field structs today.

## Decision

Adopt **Option (a')** — the type checker rejects `==` (and `!=`) on struct
values whose type transitively contains a function-typed field, with a clear
MVL-layer diagnostic.  The emitter continues to derive `PartialEq` on all
structs; the checker guarantees no such comparison ever reaches Rust.

### Rationale

1. **Explicit over implicit** (CLAUDE.md, design principle 1).  MVL's core
   promise is that ambiguity gets surfaced at compile time, in source terms.
   Function-pointer equality is implementation-defined in Rust; that
   ambiguity is exactly what MVL's checker exists to reject.

2. **The signature IS the threat model** (CLAUDE.md, design principle 3).  A
   struct containing a callback is a value whose identity depends on that
   callback.  Declaring that MVL can't reason about the callback's identity
   is honest — and rejecting `==` on such values follows from the same
   principle that rejects `Secret[T]` flowing to `Console`: the compiler is
   the arbiter of what comparisons are meaningful.

3. **Preserves the design space.**  Function-typed struct fields have real
   use cases MVL should not close off (strategy pattern, callback-holding
   handles, plugin registries, actor-adjacent code).  Option (d) removes
   the *field*; (a') removes only the *equality operation on values
   containing that field* — a narrower cut.

4. **Zero blast radius.**  With 0 existing sites, this decision affects no
   current MVL program.  It only sets the rule for future code.

5. **Cleanest error UX.**  MVL-layer diagnostics reference the source struct
   and field by name; Rust's `trait bound PartialEq not satisfied` message
   would be delivered through the transpiled `lib.rs` with line numbers the
   user never wrote.

6. **Existing checker infrastructure.**  `infer.rs`'s `Eq`-constraint machinery
   is the natural home for this rule — narrowing an existing concept, not
   introducing a new pipeline stage.

### Rejected alternatives

- **(a) Emitter-only:** works but produces the wrong error surface.  The user
  sees a Rust compiler complaint about generated code, not an MVL diagnostic.
- **(b) Panic:** violates MVL's total-function / signature-is-threat-model
  principles.  Runtime surprises are never the answer.
- **(c) Silent ignore:** produces two values that compare equal but behave
  differently — a category of bug MVL's design specifically exists to prevent.
- **(d) Forbid fn-typed struct fields:** closes off callback-holding structs
  as a design pattern.  Too restrictive given no evidence the pattern is
  problematic beyond the equality case.

## Consequences

### Positive

- The `unpredictable_function_pointer_comparisons` entry in the crate-level
  `#![allow(...)]` can be removed (closes the umbrella #1656 sub-issue).
- Users who add a callback field to an existing struct and later try to
  `==` it get a diagnostic that tells them exactly why, in MVL terms.
- Sets precedent for future "type is not comparable" rules
  (e.g. actors — see also #1570 unsafe escape hatch).

### Negative

- Two conceptually similar constructs — bare `fn` values and `fn`-field
  structs — get slightly different treatment: bare `fn` values inside actor
  messages or higher-order code aren't affected by this ADR; only the
  struct-field equality case is.  This is honest but requires documentation.
- Adds a new checker rule (small); the fn-transitive-containment check is
  simple but must handle nested structs and generic instantiation.

### Neutral

- The emitter's `#[derive(PartialEq)]` remains unconditional.  Alternative
  implementations that switched to a conditional derive would achieve the
  same runtime outcome but at the cost of harder-to-read emitted Rust and
  a worse error surface.

## Implementation Sketch

1. `src/mvl/checker/infer.rs` — extend `BinaryOp::Eq`/`Ne` handling to
   check the operand type; if it is a struct type that transitively
   contains a `Ty::Fn(...)` field, emit a diagnostic pointing at the
   field.  Transitive means: struct-A field of type struct-B where
   struct-B has an fn field also rejects `A == A`.
2. Add corpus test at `tests/corpus/03_types/fn_field_struct_no_eq.mvl`
   (marked `corpus:expect-fail`) demonstrating the rejection.
3. Remove `unpredictable_function_pointer_comparisons` from the
   `#![allow(...)]` header in `src/mvl/backends/rust/emitter.rs`.

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **Req 1 (Type Safety) — strengthens.**  Adds a new type-checker rule
  that rejects a class of comparison whose result would be
  implementation-defined at runtime.  The rule extends the existing
  "operator requires trait bound" machinery (`Eq` constraint on type
  parameters) to concrete struct types that transitively hold fn
  values.
- **Req 3 (Totality) — consistent with.**  Total functions were
  already forbidden from depending on non-total operations; adding
  a "cannot compare" case for fn-field structs sits naturally
  alongside that.
- All other requirements — unchanged.

### Design Principles (README / CLAUDE.md)

- **Explicit over implicit — strengthens.**  Function-pointer equality
  is implementation-defined in Rust; MVL now surfaces that ambiguity
  at check time with a source-level diagnostic rather than hiding it
  behind a crate-level allow.
- **One way to do it — consistent with.**  There is no MVL syntax
  that will silently produce a fn-pointer comparison after this
  decision.  Users who want value-identity semantics for
  callback-holding structs must build them explicitly (e.g. by
  storing an identifier alongside the callback).
- **The signature IS the threat model — strengthens.**  A struct
  containing a callback advertises "identity depends on the
  callback" — the checker now agrees.
- Other principles — consistent with.

### Specifications

No spec files under `.openspec/specs/` are affected.  The behaviour
change is confined to the type checker's binary-operator rules
(001-type-system Req 9 machinery) and does not add or modify any
external requirement.

---

## References

- Umbrella audit: #1656
- Design principles: `CLAUDE.md`
- Related implementation-defined-behavior stance: #1570 (unsafe escape hatch)
- Self-hosting readability goal: ADR-0044
- Post-Postel strictness precedent: ADR-0026
