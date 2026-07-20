# ADR-0058: Z3 Real Theory for Float Refinements

**Status:** Accepted
**Date:** 2026-07-20
**Issues:** #1957

---

## Context

MVL's Layer 5 (Z3) refinement solver proved predicates over integer and boolean
parameters using `z3::ast::Int` (QF-NIA) and over bit-vector parameters using
`z3::ast::BV` (QF-BV). Float-typed parameters were silently skipped: any
call-site argument that was not a concrete float literal fell through all layers
and was emitted as `RuntimeCheck`.

This created a visible asymmetry: `type Probability = Float where self >= 0.0 &&
self <= 1.0` was enforced at runtime but never *proved* statically, even for
arguments whose refinement was obviously provable (e.g., the result of a
function whose `ensures` clause implies it). The same gap affected `mvl harden`
axes 2 (tightening), 3 (boundary witnesses), and 4 (MC/DC synthesis for Float
clauses).

---

## Decision

1. **Z3 Real domain path.** Add `impl_z3_real` — a parallel implementation to
   `impl_z3` that uses `z3::ast::Real` for all variables and the self-term.
   Dispatch via `has_float_ops(pred)` detection (analogous to `has_bitwise_ops`
   and `has_string_ops`) before the NIA fallback inside `try_z3`.

2. **Translation functions.** Add `ref_to_real` (parallel to `ref_to_int`) and
   `ref_to_bool_real` (parallel to `ref_to_bool`) for the Real domain.  Add
   `expr_to_real` (parallel to `expr_to_int`) for call-site argument
   translation.

3. **Witness synthesis.** Extend `impl_z3_witness` to create `z3::ast::Real`
   variables for `Float` / `Float32` / `Float64` parameters, extract model
   values as `f64`, and return them as a new `WitnessValue::Float(f64)` variant.

4. **Tightening.** Change `TightenResult.tighter_bound` from `i64` to `f64`
   (lossless for the ±1 000 000 Int range). Add
   `extract_simple_self_bound_float`, `make_self_cmp_float`, and
   `impl_z3_tighten_real` that binary-search f64 bounds analogously to the
   integer path.

5. **NaN / rounding caveat.** Z3 Real theory is `ℚ` (rationals / algebraic
   reals), not IEEE-754 f64. The Real path is gated on `has_float_ops(pred)`,
   which requires at least one `RefExpr::Float` node in the predicate. Predicates
   that contain `RefExpr::Float` but could be NaN-sensitive are not yet expressible
   in the current `RefExpr` grammar (`is_nan`, `is_finite`, etc. are not
   `RefExpr` variants), so there is no unsound path today. If such variants are
   added in future, they MUST be excluded from `has_float_ops` (i.e. the function
   must return `false` for predicates involving them) so they stay at
   `RuntimeCheck`.

---

## Consequences

**Positive:**
- `mvl prove` now reports Float refinements as `proven` for non-literal symbolic
  arguments, not just concrete float literals.
- `mvl harden` axes 2, 3, and 4 gain Float support — tighter bounds, boundary
  witnesses, and MC/DC pairs can be synthesized for Float parameters.
- The symmetry that Int/Bool users already enjoyed is now available to Float users.

**Negative / trade-offs:**
- Real arithmetic is an approximation of f64. Proofs are sound for the finite
  ordered arithmetic subset but cannot detect rounding-dependent failures (e.g.,
  `self + 0.1 == 0.3` is `true` in Real but `false` in f64). This is acceptable
  because MVL refinement predicates in practice express bounds, not rounding-exact
  equalities.
- `TighterBound` is now `f64`. Existing callers comparing tighter bounds
  (`c.tighter_bound < prev.tighter_bound`) continue to work since `f64` supports
  `PartialOrd`. The display format now uses `{:.6}` for float bounds.

---

## Rejected Alternatives

**Z3 QF-FP (floating-point theory).** Would model IEEE-754 exactly including NaN,
subnormals, rounding. Rejected because: (a) Z3 QF-FP is dramatically slower than
QF-LRA for the bound-style predicates MVL uses; (b) most MVL refinements express
ordered ranges, not rounding behaviour. Reserved as a future opt-in for
NaN-sensitive predicates.

**`ArithAst` enum in shared translation.** The issue suggested an
`ArithAst::Int | ArithAst::Real` enum to unify `ref_to_int` / `ref_to_real`.
Rejected in favour of parallel standalone functions to preserve readability and
avoid lifetime-polymorphism complications.  Self-hosting (spec 018) favours
clear, independently translatable functions over clever abstractions.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **R3 — Refinement types:** **Strengthened.** Float-typed refinement parameters
  are now statically discharged by Z3 when provable, not always deferred to
  runtime.
- **R5 — Function contracts (`ensures`):** **Strengthened.** Tightening (axis 2)
  and witness synthesis (axis 3) now cover Float `ensures` clauses.
- All other requirements: unchanged.

### Design Principles (README)

- **Explicit over implicit:** consistent with — no hidden Float promotion; the
  Real path is explicitly dispatched when `has_float_ops` detects Float nodes.
- **The signature is the threat model:** strengthens — Float refinements in
  signatures are now statically verified, not silently runtime-checked.
- **No UFCS (ADR-0031):** consistent with — no new method dispatch.
- All other principles: consistent with.

### Specifications

- `spec/018-refinement-solver`: R1 (Layer 1 concrete) is unchanged. R5 (Layer 5
  Z3) is extended to cover the Real domain. No spec text changes required; the
  extension is additive.
- `spec/026-harden`: R5 (axis 4 / MC/DC) now covers Float clauses. Update note
  only — no normative text change.
