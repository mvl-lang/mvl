# ADR-0055: Atom normalization lives at the solver dispatch boundary

**Status:** Accepted
**Date:** 2026-07-14
**Issues:** #1805

---

## Context

The layered refinement solver runs L1 (trivial) → L2 (interval) → L3
(symbolic) → L4 (Cooper QE) → L5 (Z3) for every call-site argument that
carries a `where` refinement.  Each layer is designed to reason about
integer variables — its input atom is expected to be `Expr::Ident`.

Real MVL programs pass compound atoms:

- `ball.vx` — `Expr::FieldAccess`
- `xs.len()` — `Expr::MethodCall`
- `f(x)` where `f` is not a pure fold target — `Expr::FnCall`

L2 (`layer2.rs::try_interval`), L4 (`layer4.rs::linterm_from_expr`), and
L5's encoding all hard-filter their argument to `Expr::Ident` and return
`None` for anything else.  As a result, arguments carrying a compound atom
never reach the arithmetic layers; they either succeed at L1's structural
shape-equality path or fall through to `RuntimeCheck`.

L1's shape-equality is powerful for "field-is-untouched" patterns (e.g.
`Ball { x: b.x, .. }` matching `result.x == b.x`) but blind to arithmetic
reasoning: `field.height - 1 < field.height`, universally true for any
integer `field.height`, cannot be discharged because L4 refuses to build
a linear term from `FieldAccess`.

Two implementation shapes were considered:

1. **Per-layer accommodation** — extend each of L2/L3/L4/L5 to accept
   `FieldAccess` / `MethodCall` and treat those subtrees as opaque
   variables.  Five modules gain identical code, drift risk is high.
2. **Dispatch-boundary normalization** — a single pass rewrites compound
   atoms to fresh `Ident("__atom_N")` before layer dispatch.  L2/L3/L4/L5
   are untouched; they still only need to reason about integer variables.

---

## Decision

Two coordinated changes make compound-atom arguments discharge through
the arithmetic layers.

### 1. Atom normalization at the dispatch boundary

`src/mvl/checker/refinements.rs::check_arg_against_pred_counted` runs a
single normalizer (`src/mvl/checker/solver/atom_norm.rs`) between L1 and
L2.  The normalizer:

- Walks the argument `Expr`, the predicate `RefExpr`, and every value in
  `var_refs`.
- Rewrites each maximal *non-arithmetic* subtree to
  `Ident("__atom_N")`:
  - `Expr::FieldAccess`, `Expr::MethodCall`
  - `RefExpr::FieldAccess`, `RefExpr::Len`
- Uses a canonical string form (`canon_expr` / `canon_refexpr`) as the
  deduplication key, so two occurrences of the same subtree — whether
  in the goal or in a hypothesis, whether as `Expr` or as `RefExpr` —
  collapse to the same atom name.
- Preserves arithmetic operators (`Unary`, `Binary`, `ArithOp`,
  `Compare`, `LogicOp`, `Not`, `Grouped`) by recursing into their
  children, so leaves inside a formula still get normalized.

L1 continues to run first with the original inputs — structural shape
equality remains the workhorse for identity-of-field cases.  L3
(`try_symbolic`) also receives the original inputs, since its dispatch
key is `Expr::FnCall` / `Expr::If` / `Expr::Block` and its internal
substitution logic operates on user AST directly.

**Atoms live at the dispatch layer.**  Layers below the boundary see
only integer variables.  A future contributor tempted to add a
per-layer `FieldAccess` handler should extend the normalizer instead.

### 2. Struct-field hypothesis projection

Normalization is inert without a hypothesis to feed the arithmetic
layers.  A second change in `params_to_var_refs` projects a
struct-typed parameter's per-field refinements into synthetic
hypothesis keys of the form `"param.field"`:

```
type Box = struct { size: Int where self > 5 }
fn caller(b: Box) -> Int { positive(b.size) }
```

For the `caller` binding, `var_refs["b.size"]` is now
`Some(self > 5)` — matching the canonical key
`AtomNormalizer::canon_expr` synthesizes for `b.size`.
`AtomNormalizer::rewrite_var_refs` then bridges that entry onto the
atom name (`__atom_0`), so L2/L4/L5 find the hypothesis via the atom
they see in the goal.

Only fields with an explicit refinement are projected.  Unrefined
fields cost nothing and add no keys.

### 3. Let-binding unfolding in contract checking

The ticket flagged a third gap: when a function body ends with a tail
return that references a let-bound name whose init is an `if`-expression,
the contract checker (`check_ensures_in_block` in `contracts/mod.rs`)
skipped the let conservatively.  The tail reached the solver as an
opaque identifier and fell to runtime, even when the branches of the
init would trivially discharge with the branch condition as a
hypothesis.

The fix extends `check_ensures_in_block` to substitute the let-bound
name into every subsequent statement's return-carrying position:

```
fn f(g: Game) -> Int ensures result >= 0 {
    let s: Int = if g.right_score < 11 { 5 } else { 3 };
    s
}
```

becomes, for the ensures check:

```
if g.right_score < 11 { 5 } else { 3 }
```

which `check_ensures_for_return_expr_recur` already decomposes into two
branches with the branch condition as a hypothesis, both discharging at
L1 trivially.

Substitution reuses the `substitute_expr` and `substitute_stmt`
functions in `layer3.rs` (promoted to `pub(crate)`); those in turn
gained `Construct` and `FieldAccess` recursion so that the substituted
value can flow through struct literals and dotted references.

`layer3.rs::collect_block_paths` gains a parallel unfolding path so
that when the solver is invoked with a `Block` argument (rather than
via the contract-check boundary), the same let-fork logic applies.
Path explosion is capped by the existing `MAX_PATHS` guard.

### 4. MethodCall / FnCall postcondition projection

Method calls carry postconditions the same way, but through the
callee's `ensures` clause rather than a struct declaration.  A new
registry `build_fn_ensures_combined(progs)` lowers every function's
`ensures` clauses (via `expr_to_ref_expr_ext`) to `RefExpr`s
normalized with `result` → `self`.  The registry is threaded through
`RefinementAnalyzer::fn_ensures`.

At each `FnCall` / `MethodCall` call site, the analyzer walks all
argument expressions with `collect_call_hypotheses`, inserting a
per-callee hypothesis under the canonical key
`"receiver.method(args)"` or `"name(args)"`.  The atom normalizer
then bridges that key onto the atom, exactly like struct fields.

The stdlib collections (`List`, `Map`, `Set`, `String`, `Span`)
declare `ensures result >= 0` on `len()` as part of this change so
that `xs.len()` carries a usable hypothesis at every call site.
Unknown-shape postconditions that `expr_to_ref_expr_ext` cannot
lower are silently dropped from the registry (they still get
verified elsewhere by `contracts::check_contracts`; here they
simply cannot be leveraged as static hypotheses).

---

## Consequences

**Positive**

- L2/L4/L5 gain vocabulary support for compound atoms without any
  changes to their solver logic.
- Cross-representation identity: `Expr::FieldAccess { x, y }` in the
  goal and `RefExpr::FieldAccess { x, y }` in a hypothesis map to the
  same atom, so hypothesis-driven proofs can align.
- Single point of extension — adding new atom shapes (e.g. `Expr::Index`
  if MVL grows one) touches one module.

**Negative / follow-up**

- `Expr::FnCall` is deliberately *not* normalized in this pass — L3
  requires the FnCall shape to unfold pure function bodies.
  "Unknown FnCall" atoms (functions L3 cannot unfold) remain a
  separate consideration; deferring keeps L3's semantics intact.
- `requires b.field > N` clauses on the caller side are still not
  projected — only struct-declaration field refinements are.  Extending
  the projection to `requires` clauses is a straightforward follow-up.
- Adds one clone of the `var_refs` map per non-trivial dispatch call
  and one extra key per refined field per struct-typed param.  In
  practice both are bounded by O(params × fields); the cost is minor.

**Observable impact.**  Three constructed tests move from runtime to a
statically-proven layer:

- `positive(b.size)` where `Box.size: Int where self > 5` — struct
  field projection lands at L2.
- `non_negative(xs.len())` on `List[Int]` — MethodCall postcondition
  projection lands at L2.
- `fn f(g: Game) -> Int ensures result >= 0 { let s = if ... { 5 }
  else { 3 }; s }` — let-unfolding + existing per-branch descent lands
  at L1 (once per branch).

The existing example corpus (`medical_triage`, `access_control`, etc.)
uses plain-Int parameters everywhere, so its layer distribution is
unchanged.  Programs that model domain entities as structs with refined
fields, that guard arithmetic with `xs.len() >= 0`, or that shape their
return via `let x = if …` — the patterns envisioned by the ticket —
now discharge to the arithmetic / trivial layers instead of falling to
runtime.

---

## Rejected Alternatives

- **Extend L2/L4/L5 in-place.**  Would work, but every layer grows the
  same FieldAccess / MethodCall handling.  5× surface area, 5× drift.
  Rejected on maintainability grounds.
- **Normalize inside each layer entry.**  Same problem plus double
  work — the pass would run once per layer instead of once per
  dispatch.
- **Close #1805 as premature.**  The payback numbers in the ticket
  (Pong 5%→40% L2+) turned out to be speculative — `examples/pong` does
  not exist in this repo.  However, the plumbing is genuinely useful:
  it unblocks the eventual hypothesis-threading work by giving the
  arithmetic layers a name to hang hypotheses on.  Landing the
  vocabulary fix now costs little and removes a foreseeable roadblock.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **Requirement 10 (Refinement types).**  Strengthens.  This change
  widens the set of refinement obligations the layered solver can even
  *consider* — compound-atom arguments now enter the arithmetic layers
  instead of falling straight through to runtime.  It does not weaken
  any existing check.
- All other requirements: unchanged.

### Design Principles (README)

- **One way to do it** — strengthens: one normalization pass at one
  location, not per-layer branching.
- **Explicit over implicit** — consistent with: the synthesized atom
  prefix `__atom_` is deterministic and inspectable; nothing hides.
- **Signature is the threat model** — consistent with: no change to
  what appears in function signatures.

Other principles: consistent with.

### Specifications

No specs in `.openspec/specs/` describe the internal solver layering,
so no spec updates are required.  If a "solver architecture" spec is
ever authored, it should reference this ADR when describing the
dispatch boundary.
