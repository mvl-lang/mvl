# dose_scheduling — L5 case study

Companion example to `../medical_triage/`. Same clinical domain (weight-based drug dosing), different refinement pressure: this example intentionally forces the solver into **L5 (Z3 QF-NIA)** via multiplications of bounded program variables.

## What it proves (`make prove`)

```
Summary: 31 proven (L1:21 L2:0 L3:0 L4:2 L5:8), 8 runtime, 0 failed
```

| Layer | Count | What it discharges |
|---|---:|---|
| L1 (trivial)  | 21 | `clamp_*` return-bound ensures — literal subsumption |
| L2 (interval) | 0  | (no bare interval-subsumption obligations in this example) |
| L3 (symbolic) | 0  | (no path-enumeration obligations) |
| L4 (Cooper)   | 2  | `combined_dose` — linear sum of bounded terms |
| **L5 (Z3)**   | **8** | **Nonlinear products — see below** |
| runtime       | 8  | Two documented gaps — see below |

## Testing coverage

```
make test        →  35 passed, 0 failed
make coverage    →  29/29 branches (100%)
make mcdc        →  8/10 obligations (80%) — 2 structurally coupled
make mcdc + --masking →  PASS (DO-178C masking rules exempt coupled clauses)
```

## MC/DC coupling exemptions (audit anchors)

The two "missed" MC/DC obligations are structural couplings via shared
variables. Each exemption is documented **inline in the source** as an
audit anchor — a stable string a reviewer can grep for:

| Anchor | Function | Shared term | Location |
|---|---|---|---|
| `MCDC-DOSE-001` | `contraindicated` | `has_allergy` in two clauses | `dosing.mvl:125` |
| `MCDC-DOSE-002` | `requires_pharmacy_review` | `total_mg` in two clauses | `dosing.mvl:153` |

Reproduce the audit:

```bash
grep -n "MCDC-DOSE-" dosing.mvl
```

Both exemptions cite **DO-178C Appendix A §6.4.4.2 masking MC/DC** as the
accepting rule. The doc comments explain the clinical rationale for
keeping the shared term (refactoring into a helper would hide the review
criterion, not eliminate the structural coupling).

`make all` runs the full pipeline with `--masking` enabled by default.
Running `make mcdc` without arguments deliberately fails to surface the
two coupled clauses — this is the intended behaviour for a reviewer who
wants to see the unique-cause result before applying the exemption.

## IEC 62304 assurance envelope (`make test-62304`)

For Class C (life-supporting) medical software, IEC 62304 requires
evidence along three independent axes:

1. **Static proof** — verification that the code holds for **all**
   inputs of the declared types (compile-time, universal quantification).
2. **Unit tests** — verification that the code produces the **specific**
   expected output on representative inputs (dynamic, value-level).
3. **MC/DC coverage** — verification that every atomic condition in
   every compound decision independently affects the outcome (coverage
   property of the test suite, not of the code).

No single axis subsumes the others. Static proofs cannot check that the
formula being computed is the intended one (`weight + rate*hours` would
satisfy `result > 0` and be clinically wrong). Tests cannot generalise
beyond the inputs they explore. MC/DC certifies test-suite quality but
says nothing about static correctness.

`make test-62304` composes all four checks and emits a one-screen
assurance envelope:

```
── (1) Static refinement proof (compile-time, all inputs) ─────
Total: 31 proven (L1:21 L2:0 L3:0 L4:2 L5:8), 8 runtime, 0 failed

── (2) Behavioural unit tests (dynamic, specific inputs) ──────
test result: ok. 35 passed; 0 failed
All tests passed.

── (3) Branch coverage (decision points reached) ──────────────
Total: 29/29 branches  (100%)

── (4) MC/DC coverage (compound decisions, --masking) ─────────
MC/DC coverage: 8/10 pure obligations met (80%)
PASS

── Audit anchors (grep -n MCDC-DOSE- dosing.mvl) ──────────────
  125:/// MC/DC AUDIT ANCHOR (`MCDC-DOSE-001`): ...
  153:/// MC/DC AUDIT ANCHOR (`MCDC-DOSE-002`): ...
```

The eight runtime obligations from axis (1) are exercised by axis (2):
every test that invokes `plan_infusion` executes through the seven
inter-procedural precondition checks; `total_dose_max_envelope` exercises
the runtime upper-bound assertion that L5 could not statically discharge.
This yields a three-layer defensive structure — compile-time proofs,
runtime assertions, dynamic tests — with the caveat that runtime checks
only fire on inputs the tests actually cover. Refinements matter
precisely because they discharge the property for **all** admissible
inputs, not just the sample the tests explored.

## The seven L5 obligations

Each of these requires reasoning about a **product of bounded program variables**, which sits outside Cooper's Presburger fragment at L4:

| Location | Obligation | Why L5 |
|---|---|---|
| `dosing.mvl:35` | `total_infusion_dose: result > 0` | `weight × rate × hours > 0` from positivity of each factor |
| `dosing.mvl:52` | `escalate_dose: result >= base` | `base × factor >= base` monotonicity |
| `dosing.mvl:52` | `escalate_dose: result <= 5000` | Product upper bound |
| `dosing.mvl:69` | `max_daily_dose: result > 0` | Two-variable product positivity |
| `dosing.mvl:69` | `max_daily_dose: result <= 20000` | Product upper bound |
| `dosing.mvl:85` | `per_bolus_mg: result >= 0` | Integer division of positives is non-negative |
| `dosing.mvl:98` | `concentration_mg_per_ml: result >= 0` | Same shape |

## The eight runtime fallthroughs

These are **honest reporting** of solver limits, not example bugs:

- **1× nonlinear upper bound Z3 could not derive.** `total_infusion_dose: result <= 240000` — deriving `weight × rate × hours <= 200 × 50 × 24` requires QF-NIA reasoning that Z3 does not currently discharge within the timeout, even though the bound is structurally true.
- **7× inter-procedural precondition propagation.** The seven parameter-refinement checks at `plan_infusion`'s call sites — the callee refinements are known and the caller flows come from `clamp_*` helpers whose ensures constrain the outputs, but the solver does not currently propagate the callee ensures forward across the call boundary at the caller. See related work in mvl-lang/mvl#1895 and #1896.

Both categories become MVL runtime asserts. The program stays safe; the compile-time proof coverage is documented.

## Files

- `model.mvl` — patient / order / plan types
- `dosing.mvl` — the refinement-heavy calculation module
- `main.mvl` — four demo scenarios (neonate → elderly)
- `Makefile` — standard example targets (`check`, `prove`, `assurance`, `test-solver`, `run`, `all`)

## How this compares to medical_triage

- `medical_triage`: 9 obligations, L1:5 L2:1 L3:1 L4:2 L5:0. All bounded ordinal reasoning. Sweet spot for Liquid-Types-class refinements (QF-EUFLIA).
- `dose_scheduling`: 32 call sites, 24 proven, 8 runtime. L5 handles 7 nonlinear obligations that L1–L4 cannot touch. Demonstrates the layered escape hatch and its practical ceiling.

Together the two examples characterise the full L1–L5 dispatch surface: what discharges cheaply, what escapes to Z3, and where Z3 itself hits its heuristic limits.

## Related tickets

- **#1897** — this example (add case study exercising L5)
- **#1895** — warn when L5/Z3 is compiled out
- **#1896** — surface Z3 counter-examples in diagnostics
