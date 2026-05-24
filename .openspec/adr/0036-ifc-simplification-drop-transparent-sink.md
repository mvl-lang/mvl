# ADR-0036: IFC Simplification — Drop `transparent` and `sink` Keywords

**Status:** Accepted
**Date:** 2026-05-24
**Issues:** #1007
**Supersedes:** ADR-0024

---

## Context

The IFC system had three mechanisms beyond the type system:

1. **`transparent` keyword** — declared that a function propagates security labels from arguments to return type. After ADR-0024, all functions were already label-transparent by default — the keyword was a no-op.

2. **`sink` keyword** — declared that a function is a public output sink. The checker rejected labeled arguments at call sites. But the type system already handles this: `println(value: String)` rejects `Secret[String]` via type mismatch.

3. **`PUBLIC_SINKS` constant** — hardcoded list of sink function names for implicit flow detection. Fragile, incomplete (missed `write_file` under a secret branch), and redundant with the effect system.

The effect system provides the same observability information: any function with `! Console`, `! Log`, `! FileWrite`, etc. is observable. Calling an effectful function under a high-PC (labeled) branch is an implicit flow.

---

## Decision

**Drop `transparent` and `sink` keywords entirely.** The IFC model uses only:

| Protection | Mechanism |
|---|---|
| Label declaration | `label Tainted`, `label Secret` |
| Direct label mismatch | Type system: `Secret[String]` ≠ `String` |
| Label propagation | Automatic: all functions propagate unconditionally |
| Implicit flow detection | Effect system: effectful call under high-PC branch = violation |
| Explicit trust crossing | `relabel` with audit tag |

**Two IFC keywords remain:** `label` (declares a label type) and `relabel` (crosses a label boundary).

### Implicit flow: effects replace sinks

The implicit flow checker now uses **effect declarations** instead of sink names:

- Any function with declared effects (`! Console`, `! Log`, etc.) is observable
- `build_effect_reachability` replaces `build_sink_reachability`
- Transitively, `a→b→println` is flagged if `println` has effects

This is stricter than the old `PUBLIC_SINKS` list (which only covered a hardcoded subset). It is also correct: ANY observable effect under a high-PC branch leaks information through control flow.

---

## Consequences

**Positive:**
- Two fewer keywords to learn, document, and maintain
- No hardcoded sink lists — the effect system handles observability
- Design Principle #2 satisfied: one way to do each thing (type system + relabel)
- Stricter implicit flow detection: covers `write_file`, `append`, custom effectful functions

**Negative:**
- Slightly stricter: pure helper functions that call effectful functions are now in the effect-reach set even if they weren't previously in the sink-reach set. This is correct behavior (they *are* observable) but may surface new implicit flow errors in existing code.

**Removed:**
- `TokenKind::Transparent`, `TokenKind::Sink` from lexer
- `is_label_transparent`, `is_sink` from `FnDecl` and `FnInfo`
- `TransparentFnNoParams`, `TransparentFnLabeledReturn`, `TransparentFnGeneric` error variants
- `LoggingLabelViolation` error variant
- `collect_sink_names`, `PUBLIC_SINKS` constant
- `transparent`/`sink` from EBNF grammar and tree-sitter grammar

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **Requirement 7 (Format/Transform):** Strengthened — all functions propagate labels unconditionally, not just those marked `transparent`.
- **Requirement 11 (IFC):** Strengthened — implicit flow detection uses the effect system (complete) instead of a hardcoded sink list (incomplete).
- All other requirements: unchanged.

### Design Principles (README)

- **#2 One way to do each thing:** The "one way" is: type system + `relabel`. No `transparent`, no `sink`.
- **#7 Security labels:** Updated from `Public, Tainted, Secret` to `Tainted, Secret, user-defined`. `Public` was never a real label — bare types are implicitly public.
