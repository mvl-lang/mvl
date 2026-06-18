---
domain: checker
version: 0.1.0
status: active
date: 2026-05-15
epic: phase-8-refinement-solver
---

# 018 — Layered Refinement Solver

The MVL refinement solver proves `where` predicates statically, reducing runtime
overhead and strengthening the trust boundary.  The architecture is deliberately
layered: fast, MVL-native techniques handle the vast majority of proofs, with Z3
as a final escape hatch for the rare complex cases.

## Philosophy

Most refinement predicates in real MVL programs are simple — a literal check, a
range constraint, or a structural path argument.  Routing all proofs through Z3
is correct but wasteful: Z3 carries a ~1 ms per-call overhead and is implemented
in C++, outside MVL's own trust boundary.

The layered solver solves this in two ways:

1. **Speed:** 90%+ of proofs resolve in microseconds at Layers 1–2.
2. **Trust:** Layers 1–4 are pure MVL/Rust, verifiable by MVL's own toolchain.
   Only Layer 5 requires trusting an external solver.

The long-term goal is for Layers 1–4 to be rewritten in MVL and self-verified.

**Implementation:** `src/mvl/checker/solver/`

**Epic:** #545

## Architecture

```
MVL `where` predicate
        ↓
┌───────────────────────┐
│ Layer 1: Trivial      │  O(1)    ~40% — #589 (DONE)
└──────────┬────────────┘
           ↓ None
┌───────────────────────┐
│ Layer 2: Intervals    │  O(n)    ~35% — #590 (DONE)
└──────────┬────────────┘
           ↓ None
┌───────────────────────┐
│ Layer 3: Symbolic     │  O(n²)   ~15% — #592 (DONE)
└──────────┬────────────┘
           ↓ None
┌───────────────────────┐
│ Layer 4: Cooper's QE  │  O(exp)  ~5%  — #593 (DONE)
└──────────┬────────────┘
           ↓ None
┌───────────────────────┐
│ Layer 5: Z3 SMT       │  O(exp)  ~5%  — #543 (DONE)
└──────────┬────────────┘
           ↓ None
    RuntimeCheck emitted
```

Each layer returns `Option<RefResult>`.  `None` means "I cannot decide — try
the next layer."  The solver falls through to `RuntimeCheck` only when all
layers are exhausted.

---

## Requirements

### Requirement 1: Layer 1 — Trivial Pattern Matching [MUST]

The solver MUST resolve literal-argument, same-refinement subsumption,
tautology, and contradiction cases in O(1) without deeper analysis.

**Implementation:** `src/mvl/checker/solver/layer1.rs`

**Issue:** #589

**Tests:** `tests/type_checker.rs::refinement_literal_zero_to_nonzero_param_rejected`, `tests/corpus/09_refinements/`

#### Scenario: Literal argument

- GIVEN `positive(42)` where `positive` is `x > 0`
- WHEN Layer 1 evaluates the refinement
- THEN `42 > 0` is evaluated by constant folding → `Proven`

#### Scenario: Subsumption

- GIVEN `fn foo(x: Int where x > 0)` calling `bar(x: Int where x > 0)`
- WHEN Layer 1 checks the call site
- THEN caller's refinement implies callee's refinement → `Proven`

#### Scenario: Contradiction

- GIVEN predicate `x > 0 && x < 0`
- WHEN Layer 1 evaluates the predicate
- THEN contradiction detected → `Failed`

#### Scenario: Non-trivial case passed through

- GIVEN `fn foo(x: Int where x > y)` with symbolic `y`
- WHEN Layer 1 evaluates the refinement
- THEN `None` returned, Layer 2 is tried

---

### Requirement 2: Layer 2 — Interval Arithmetic [MUST]

The solver MUST track value ranges through control flow and prove refinements
via interval containment.

**Implementation:** `src/mvl/checker/solver/layer2.rs`

**Issue:** #590

**Tests:** `tests/type_checker.rs::refinement_positive_literal_proven_accepted`

#### Scenario: Branch narrowing

- GIVEN `if x > 0 { needs_positive(x) }` where `needs_positive` requires `x > 0`
- WHEN Layer 2 evaluates the branch body
- THEN `x ∈ (0, ∞)` from the branch condition → `Proven`

#### Scenario: Interval propagation through arithmetic

- GIVEN `x ∈ [0, 10]` and `y = x + 1`
- WHEN Layer 2 evaluates `y`
- THEN `y ∈ [1, 11]` is derived

#### Scenario: Soundness over completeness

- GIVEN a predicate that interval analysis cannot confirm
- WHEN Layer 2 cannot prove the refinement
- THEN `None` is returned (no false positives)

---

### Requirement 3: Layer 3 — Symbolic Path Analysis [MUST]

The solver MUST analyze multi-path pure functions, tracking symbolic expressions
and path constraints, to prove return refinements.

**Implementation:** `src/mvl/checker/solver/layer3.rs`

**Issue:** #592

**Tests:** `tests/type_checker.rs::refinements_corpus_parses`

#### Scenario: Clamp-style function

- GIVEN `fn clamp(x, min, max) -> Int where result >= min && result <= max`
  with three return branches
- WHEN Layer 3 analyzes each path
- THEN all three paths satisfy the return refinement → `Proven`

#### Scenario: Path explosion bound

- GIVEN a function with more than 32 distinct paths
- WHEN Layer 3 encounters the path limit
- THEN `None` is returned to avoid exponential blowup

#### Scenario: Non-pure function fallthrough

- GIVEN an argument that calls a function with side effects
- WHEN Layer 3 checks applicability
- THEN `None` is returned immediately (Layer 3 is pure-function only)

---

### Requirement 4: Layer 4 — Cooper's Presburger QE [MUST]

The solver MUST implement Cooper's quantifier-elimination algorithm to decide
linear integer arithmetic constraints that symbolic path analysis cannot handle.

**Implementation:** `src/mvl/checker/solver/layer4.rs`

**Issue:** #593

**Tests:** `tests/requirements.rs::req10_refinements_proven`

#### Scenario: Linear arithmetic

- GIVEN `fn always_nonzero(x: Int) -> Int where result != 0 { 2 * x + 1 }`
- WHEN Layer 4 applies Cooper's algorithm
- THEN `2x + 1 ≠ 0 ∀x` is proven by quantifier elimination → `Proven`

#### Scenario: Complexity bound

- GIVEN a predicate with more than 5 variables
- WHEN Layer 4 estimates complexity
- THEN `None` is returned to avoid exponential worst-case

#### Scenario: Non-linear fallthrough

- GIVEN predicate `x * y > z` (non-linear)
- WHEN Layer 4 inspects the predicate
- THEN `None` is returned immediately (Layer 5/Z3 handles this)

---

### Requirement 5: Layer 5 — Z3 SMT Solver [MUST]

The solver MUST delegate to Z3 when all MVL-native layers are exhausted.
The Z3 layer MUST be feature-gated and MUST fall through gracefully when
unavailable or when Z3 cannot decide within the timeout.

**Implementation:** `src/mvl/checker/solver/layer5.rs`

**Issue:** #543

**Tests:** `tests/requirements.rs::req10_refinements_proven`

#### Scenario: Z3 proves via unsatisfiability

- GIVEN a predicate that Layers 1–4 cannot decide
- WHEN Layer 5 queries Z3 with `¬pred(arg)` under hypotheses
- THEN Z3 returns `unsat` → `Proven`

#### Scenario: Z3 timeout

- GIVEN a query that Z3 cannot resolve in 1 second
- WHEN the timeout fires
- THEN `None` is returned → `RuntimeCheck` is emitted

#### Scenario: Feature disabled

- GIVEN the `z3` cargo feature is not compiled in
- WHEN `try_z3` is called
- THEN `None` is returned immediately without panicking

---

### Requirement 6: Layered Dispatch Integration [MUST]

The solver layers MUST be wired into the checker as a unified dispatch that
tries layers in order and emits `RuntimeCheck` only when all layers are
exhausted.

**Implementation:** `src/mvl/checker/refinements.rs`, `src/mvl/checker/solver.rs`

**Issue:** #594

#### Scenario: End-to-end dispatch

- GIVEN a refinement predicate at a call site
- WHEN the checker invokes `RefinementSolver`
- THEN layers 1–5 are tried in order, stopping at the first `Some(result)`
- AND `RuntimeCheck` is emitted only if all layers return `None`

#### Scenario: CLI solver control

- GIVEN `mvl check --refinement-solver=fast-only`
- WHEN the checker runs
- THEN only Layers 1–2 are used; unsolved cases emit `RuntimeCheck`

#### Scenario: Refinement stats

- GIVEN `mvl check --refinement-stats`
- WHEN checking completes
- THEN per-layer hit counts and percentages are printed

---

### Requirement 7: Builtin Rewrite Rules (Layer 3 Extension) [SHOULD]

Layer 3 SHOULD include rewrite rules for builtin functions, enabling proofs
involving `len`, `concat`, `push`, and other standard operations.

**Implementation:** `src/mvl/checker/solver/layer3.rs` (rewrite sub-module)

**Issue:** #596

#### Scenario: String length rewrite

- GIVEN predicate `len(concat(a, b)) == len(a) + len(b)`
- WHEN Layer 3 applies rewrite rules
- THEN `len(concat(a, b))` is rewritten to `len(a) + len(b)` → provable

#### Scenario: Option rewrite

- GIVEN predicate `is_some(Some(x))`
- WHEN Layer 3 applies rewrite rules
- THEN `is_some(Some(x))` rewrites to `true` → `Proven`

#### Scenario: Rewrite confluence

- GIVEN a predicate with multiple applicable rules
- WHEN rewrites are applied in any order
- THEN the result is identical (rules are confluent and terminating)

---

### Requirement 8: SMT Axioms for Z3 (Layer 5 Extension) [SHOULD]

Layer 5 SHOULD load SMT-LIB2 axioms for builtin operations when initializing
the Z3 context, enabling proofs that require universally quantified properties.

**Implementation:** `src/mvl/checker/solver/layer5.rs` (axiom loading)

**Issue:** #597

#### Scenario: String length axiom

- GIVEN Z3 context with string axioms loaded
- WHEN Z3 checks `len(concat(a, b)) >= len(a)` for arbitrary strings
- THEN the universally-quantified axiom enables the proof → `Proven`

#### Scenario: List containment axiom

- GIVEN Z3 context with list axioms loaded
- WHEN Z3 checks `contains(push(l, x), x)` for any list `l` and value `x`
- THEN the axiom `∀l x. contains(push(l, x), x)` enables the proof → `Proven`

---

### Requirement 9: Performance Benchmarks [SHOULD]

The layered solver SHOULD be benchmarked against Z3-only to validate the 10×
performance hypothesis.

**Implementation:** `benches/refinement_solver.rs`

**Issue:** #595

#### Scenario: Corpus benchmark

- GIVEN the full test corpus with hundreds of refinement checks
- WHEN run with `--refinement-solver=layered` vs `--refinement-solver=z3-only`
- THEN layered solver is at least 10× faster in wall-clock time

#### Scenario: Per-layer micro-benchmark

- GIVEN individual benchmark cases for each layer
- WHEN Criterion runs the benchmarks
- THEN Layer 1 resolves in < 1 µs, Layer 2 in < 10 µs, Layer 3 in < 100 µs

---

## Work Breakdown

| # | Title | Layer | Status |
|---|-------|-------|--------|
| **#589** | Layer 1 — trivial pattern matching | 1 | DONE |
| **#590** | Layer 2 — interval arithmetic | 2 | DONE |
| **#592** | Layer 3 — symbolic path analysis | 3 | DONE |
| **#593** | Cooper's algorithm (Layer 4) | 4 | DONE |
| **#543** | Z3 integration (Layer 5) | 5 | DONE |
| **#594** | Layered dispatch + Z3 fallback | integration | OPEN |
| **#596** | Builtin rewrite rules (Layer 3 ext.) | 3 | OPEN |
| **#597** | SMT axioms for Z3 (Layer 5 ext.) | 5 | OPEN |
| **#595** | Benchmark layered vs Z3-only | perf | OPEN |

### Dependency Graph

```
#589 L1 ──→ #590 L2 ──→ #592 L3 ──→ #593 L4 ──→ #543 L5
                                │                    │
                                └──→ #596 rewrites   └──→ #597 axioms
                                
All layers ──→ #594 dispatch ──→ #595 benchmarks
```

---

## Success Criteria

- [ ] 90%+ of refinements proven without Z3 (Layers 1–4)
- [ ] 10× faster average check time vs Z3-only (benchmark #595)
- [ ] Zero false positives (soundness invariant — no incorrect `Proven`)
- [ ] Z3 disabled (`--no-z3`) still proves 85%+ at Layers 1–4
- [ ] Layers 1–4 written in Rust, eventually rewritable in MVL (stretch: Phase 9)

---

## References

- ADR-0019: Spec format
- Spec 001: Type system (refinement types)
- Liquid Haskell: https://ucsd-progsys.github.io/liquidhaskell/
- Cooper (1972): "Theorem Proving in Arithmetic without Multiplication"
- Presburger arithmetic: https://en.wikipedia.org/wiki/Presburger_arithmetic
