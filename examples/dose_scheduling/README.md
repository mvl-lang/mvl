# dose_scheduling — L5 case study

Companion example to `../medical_triage/`. Same clinical domain (weight-based drug dosing), different refinement pressure: this example intentionally forces the solver into **L5 (Z3 QF-NIA)** via multiplications of bounded program variables.

## What it proves (`make prove`)

```
Summary: 24 proven (L1:15 L2:0 L3:0 L4:2 L5:7), 8 runtime, 0 failed
```

| Layer | Count | What it discharges |
|---|---:|---|
| L1 (trivial)  | 15 | `clamp_*` return-bound ensures — literal subsumption |
| L2 (interval) | 0  | (no bare interval-subsumption obligations in this example) |
| L3 (symbolic) | 0  | (no path-enumeration obligations) |
| L4 (Cooper)   | 2  | `combined_dose` — linear sum of bounded terms |
| **L5 (Z3)**   | **7** | **Nonlinear products — see below** |
| runtime       | 8  | Two documented gaps — see below |

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
