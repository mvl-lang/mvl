# ADR-0025: Function Contracts — requires / ensures (Phase 1)

**Status:** Accepted
**Date:** 2026-05-09
**Issues:** #621

---

## Context

MVL's refinement types constrain values in isolation (`Int where self > 0`). They describe a single value's invariant but cannot express *relational* properties between arguments and return values.

Dafny, F*, and Lean demonstrate that function contracts (`requires`/`ensures`) are the practical lightweight way to add pre/postcondition verification without dependent types. The existing 5-layer solver already has the machinery to evaluate predicates; contracts simply specify *where* those predicates apply.

Phase 1 is intentionally conservative: it handles the most common patterns (single-parameter preconditions, result-only postconditions) using the existing solver infrastructure.

---

## Decision

1. **Two new keywords**: `requires` and `ensures`, parsed after the `where`-constraints clause and before the body block.

2. **Grammar rule**:
   ```ebnf
   fn_contract = ("requires" | "ensures") refinement ;
   fn_decl     = … { fn_contract } (block | (* builtin *)) ;
   ```

3. **AST extension**: `FnDecl` gains `requires: Vec<RefExpr>` and `ensures: Vec<RefExpr>`.

4. **`requires` checking** at call sites:
   - If the predicate references exactly one parameter name, normalise it to `self` and run the solver on the corresponding argument.
   - If the predicate references zero or multiple parameters: `RuntimeCheck` (deferred, no compile error).
   - `RefResult::Failed` → `CheckError::PreconditionViolated`.

5. **`ensures` checking** at return points (both explicit `return e` and implicit tail expressions):
   - Normalise `result` → `self` in the predicate.
   - If the normalised predicate still references any parameter name: `RuntimeCheck` (Phase 2 will add parameter-value tracking).
   - `RefResult::Failed` → `CheckError::PostconditionViolated`.

6. **Error assignment**: both new errors belong to Req 10 (Refinement Types & Contracts).

7. **Deferred to Phase 2+**: `ghost`, `old(e)`, `invariant`, `decreases`, `forall`/`exists` — all now implemented in Phases 3–5 (#628).

8. **All contract keywords are hard-reserved** (Phase 5 decision): `ghost`, `invariant`, `decreases`, `forall`, `exists` are unconditional keywords in the lexer, not contextual keywords. This keeps the grammar LL(1) without disambiguation hacks and is consistent with every verification language (Dafny, F*, Lean). User code that previously used any of these as identifiers must rename. The concrete conflict resolved by Phase 5 was `exists` in the file-I/O corpus, renamed to `path_exists`.

---

## Consequences

**Easier:**
- Common precondition patterns (`requires b != 0`, `requires n >= 0`) are now enforced statically at call sites.
- Simple postcondition patterns (`ensures result >= 0`) are verified at return points.
- The predicate language (`RefExpr`) and 5-layer solver are reused without modification.

**Harder / follow-up:**
- `requires` predicates referencing multiple parameters require multi-variable substitution (Phase 2).
- `ensures` predicates that mention both `result` and parameter names need parameter-value tracking through the function body (Phase 2).
- Ghost bindings (`ghost let x = …`) are deferred; they require a separate erasure pass before transpilation.
- `invariant` on `while` loops requires checking at loop entry, each iteration exit, and loop exit (Phase 3).

---

## Rejected Alternatives

**Runtime-only contracts**: Emitting `debug_assert!()` without static verification would mean any simple `requires b != 0; divide(10, 0)` is only caught at runtime. The solver already handles these; compile-time catching is strictly better.

**Integrate into param `where` clauses**: `requires n >= 0` is equivalent to `n: Int where self >= 0` for single-parameter cases. However, `requires` belongs to the function contract, not the parameter type, and can cross-reference multiple params — a distinction that matters for Phase 2.

**Extend `RefExpr` with `result` and `old` immediately**: Deferred. Adding `old(e)` requires capturing entry-time values, which needs a new analysis pass. Phase 1 treats `result` as a plain identifier that is normalised to `self` before checking; `old(e)` is not yet parsed.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

| Requirement | Effect |
|-------------|--------|
| **Req 1 — Type Safety** | Consistent — contracts add verification on top of types |
| **Req 2 — Memory Safety** | Consistent |
| **Req 10 — Refinement Types** | **Strengthens** — contracts are relational refinements; both `PreconditionViolated` and `PostconditionViolated` are counted under Req 10 |
| Req 3–9, 11 | Consistent |

### Design Principles (README)

- **Correctness by Construction** — strengthens: pre/postconditions are verified at compile time where the solver can prove them.
- **Minimality** — consistent: only `requires`/`ensures` in Phase 1; ghost/invariant/decreases deferred.
- **Reuse over Reinvention** — strengthens: the 5-layer solver and `RefExpr` predicate language are reused directly.
- **Explicit over Implicit** — consistent: contracts are explicit annotations, not inferred.
- Remaining principles — consistent.

### Specifications

- `.openspec/specs/001-type-system/spec.md` — Req 10 (Refinement Types) is extended to include function contracts; consider adding a contract-specific scenario when the spec is next updated.
- No other spec files are directly affected by Phase 1.
