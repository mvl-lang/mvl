# ADR-0056: Bounded quantifiers in refinement predicates via L3 expansion

**Status:** Accepted
**Date:** 2026-07-19
**Issues:** #1915, #1910 (motivating case)

---

## Context

The refinement checker's predicate language (`RefExpr`) previously supported
unbounded quantifiers (`forall x: Int, p`, `exists x: Int, p`). These forms
were parsed and emitted to Z3 at L5, but never discharged deterministically —
they either sat in a runtime-only checker escape hatch or timed out at Z3.

Several near-term case studies (the CBTC train-presence table in #1910, plus
sibling refinement extensions #1916 / #1919 / #1921) need to state properties
of the form "for every index in this range, some pointwise property holds."
Without a decidable form, these can't be expressed as refinements at all —
the reason #1910 was originally scoped to skip its forall invariant.

Two options were considered:

1. **Tighten Z3 quantifier support at L5** — trigger-set tuning, matching-loop
   heuristics. Preserves the general `forall x: Int` syntax but keeps proof
   inside SMT territory (non-deterministic timings, no per-layer breakdown).
2. **Bounded expansion at L3** — restrict the source syntax to a new form
   `forall x in [lo..hi]. p` with literal integer bounds. Unroll into a
   conjunction/disjunction of instantiated bodies, each dispatched through
   the full L1..L5 cascade.

---

## Decision

Add a new bounded-quantifier form to the refinement predicate grammar and
discharge it by expansion at L3. Reject the old unbounded form at parse time.

### 1. Grammar

```
ref_atom := ...
         |  'forall' IDENT 'in' '[' int_lit '..' int_lit ']' '.' ref_expr
         |  'exists' IDENT 'in' '[' int_lit '..' int_lit ']' '.' ref_expr
```

- Endpoints are integer literals (optionally negated). Refined-expression
  endpoints — e.g. `[0..self.len()-1]` — are **deferred to a follow-up**;
  they require the L2 interval solver to project the endpoint to a concrete
  bound, which is orthogonal to the expansion machinery.
- Both endpoints inclusive: `[a..b]` matches `{a, a+1, ..., b}`.
- The bound variable is scoped to the body; it does NOT leak into the
  surrounding refinement.

The legacy form `forall x: Int, p` is rejected with a targeted parser
diagnostic pointing users to the new syntax. Unbounded quantifiers are
out of scope for MVL's decidable core; if we ever need them, they will
land as a separate feature with SMT-only discharge.

### 2. AST

Two new `RefExpr` variants (parser/ast.rs):

```rust
BoundedForall { var: String, lo: i64, hi: i64, body: Box<RefExpr>, span: Span }
BoundedExists { var: String, lo: i64, hi: i64, body: Box<RefExpr>, span: Span }
```

The existing unbounded `Forall`/`Exists` variants remain in the enum but
are no longer constructed by the parser. Downstream sites that walked the
old variants (contracts substitution, checker patterns, printer, backend
runtime-checkability) were extended to also walk the new variants.

### 3. Discharge (L3 expansion)

New pre-pass at the top of `check_arg_against_pred_counted`
(`checker/refinements.rs`): if the predicate is a bounded quantifier, expand
it before the L1..L5 cascade.

For `forall x in [lo..hi]. body`:
1. For each `k` in `lo..=hi`, substitute `x := Integer(k)` in `body`.
2. Dispatch each instantiated body through the full layered cascade.
3. Aggregate:
   - Any instance `Failed` ⇒ whole quantifier `Failed` (short-circuit, with
     counterexample `x = k`).
   - All instances `Proven` ⇒ whole quantifier `Proven`.
   - Otherwise ⇒ `RuntimeCheck`.

Existential is dual: any instance `Proven` ⇒ `Proven` (short-circuit); all
instances `Failed` ⇒ `Failed`; otherwise `RuntimeCheck`.

Each expanded instance is credited to `counts.by_layer[3]` (per issue AC)
regardless of which inner layer actually discharged it — the expansion IS
the L3 activity.

### 4. Expansion cap

Module constant `MAX_BOUNDED_EXPANSION: usize = 1000` in
`checker/refinements.rs`. Ranges wider than the cap fall back to
`RuntimeCheck` without attempting expansion, preventing pathological blow-up
(same pattern as L3's `MAX_PATHS`).

A configurable CLI knob is deliberately out of scope for this ADR — the
1000-obligation ceiling is well above the target application scale
(bounded state machines, small tables) and can be lifted later without an
API break.

### 5. L1 closed-form evaluation

Substitution produces instances like `Integer(0) < Integer(10)` — closed
comparisons with no free variables. Neither `is_tautology` nor
`eval_pred_int(self, pred)` recognized these because they gate on `self`.

A new helper `try_eval_closed` in `layer1.rs` evaluates any predicate whose
tree contains only integer literals, arithmetic ops, comparisons, and
logical connectives. Runs before the argument-level analysis in
`try_trivial`; returns `None` for anything referring to an identifier,
`len`, `old`, quantifier, or field access.

### 6. Closed-predicate call-site dispatch

`check_requires_at_call` previously silently dropped any `requires` clause
that referenced zero parameters (`single_param_ref` returned `None`, the
multi-param fallback required ≥2 params). Bounded quantifiers over closed
bodies fall exactly into this gap. A new `check_closed_requires` helper
dispatches these with a dummy `Unit` argument, funneling them through the
same layered solver as parameter-referencing predicates.

---

## Consequences

**Positive**

- Bounded quantifiers land inside the decidable L3 layer, preserving the
  paper's Design Space claim that programs staying out of L5 are fully
  decidable.
- Composes with the sibling array-index ticket (#1916): once
  `list.get(i)` is parseable in refinements, `forall i in [0..N-1].
  p(list.get(i))` expands and discharges without any Z3 involvement for
  the pointwise cases.
- `mvl prove` per-layer breakdown attributes each expanded obligation to
  L3, matching the AC.

**Negative**

- The legacy unbounded form is a hard parse error. Two corpus fixtures
  (`tests/fixtures/01_syntax/keywords.mvl`,
  `tests/fixtures/11_contracts/loop_verification.mvl`) needed migration.
  External sources using the old syntax will need the same one-line change.
- Non-literal range endpoints are not supported yet. Cases like
  `forall i in [0..self.len()-1]. p` — the shape #1910 will actually
  need — must wait for a follow-up that hooks the range parser into
  L2 interval extraction.
- Nested-quantifier expansion is O(N^d) in depth `d`. Depth-2 nesting
  (`forall i. forall j. p`) is feasible up to ≈32×32; deeper nesting must
  either stay under the cap or fall back to `RuntimeCheck`. A dedicated
  diagnostic for depth > 2 is a possible follow-up.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **R7 — Refinement Discharge:** *strengthens*. Bounded quantifiers `forall i
  in [lo..hi]. p(i)` become a first-class refinement construct that expands
  into a finite conjunction and discharges inside the decidable L1–L3 tiers,
  extending the "programs that stay out of L5 are fully decidable" claim to
  fixed-arity pointwise properties over arrays.
- **R2 — Explicit Effects:** unchanged. Quantifier bodies are pure refinement
  predicates and carry no effect.
- **R1, R3–R6, R8–R11:** unchanged.

### Design Principles (README)

- **Explicit over implicit** — *strengthens*: the unbounded form is a hard
  parse error, so every quantifier states the exact range it iterates.
- **One way to do it** — *strengthens*: `forall x in [lo..hi]. p` is the
  single admitted quantifier form; the ambient-scope legacy form is removed.
- **The signature IS the threat model** — *strengthens*: pointwise array
  invariants (bounds, non-empty, sorted, etc.) surface in the type signature
  rather than deferring to runtime assertions.
- Remaining principles: *consistent with*.

### Specifications

- No specs in `.openspec/specs/` currently pin the quantifier syntax; when
  the refinement-language spec is written it must reference the bounded
  form and cite this ADR. Two corpus fixtures were migrated in-place
  (`tests/fixtures/01_syntax/keywords.mvl`,
  `tests/fixtures/11_contracts/loop_verification.mvl`).

---

## References

- Issue #1915 — this ticket.
- Issue #1910 — CBTC train-presence, the motivating case that needs bounded
  quantifiers together with #1916 array-index refinements.
- Issue #1916, #1919, #1921 — sibling checker enhancements that build on
  the same parser-atom groundwork.
- ADR-0055 — atom normalization at the solver dispatch boundary. Bounded
  expansion runs before atom normalization; the expansion instances are
  then normalized like any other pred.
- DML (Xi/Pfenning 1998–1999) — prior art for bounded-integer index
  refinements.
