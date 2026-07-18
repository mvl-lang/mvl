# grid_protection — Substation protection relay case study

Power-grid protection relay for an 11 kV feeder. Monitors current and voltage, applies configured protection thresholds, decides trip/hold/block based on fault detection, SCADA state, and breaker position.

**Standard.** IEC 61508 SIL 2 (industrial functional safety) + IEC 61850 (substation communication).
**Ticket.** `mvl-lang/mvl#1909`.
**Sibling cases at different domains.** `dose_scheduling` (medical, pharma nonlinear), `etcs_movement_authority` (rail, kinematics nonlinear). Grid is the same solver theory (QF-NIA) applied to a third distinct physics.

## Files

- `model.mvl` — types (`CurrentMeasurement`, `ProtectionSetting`, `FeederState`, `SCADACommand`, `BreakerState`, `TripDecision`, `FaultType`).
- `protection.mvl` — the kernel: IFC boundary, three L5-forcing nonlinear predicates, compound safety decision, trip-decision kernel, inline unit tests.
- `main.mvl` — six fault scenarios walking through normal / overcurrent / SCADA-blocked / critical / differential / manual override.
- `protection_test.mvl` — end-to-end scenario tests plus IFC smoke and arithmetic sanity.
- `Makefile` — standard targets plus `make test-61508` (SIL-2 assurance envelope).

## What is proven

`make prove` on the production file reports:

```
Summary: 20 proven (L1:17 L2:0 L3:0 L4:0 L5:3), 3 runtime, 0 failed
```

- **L1 (17)** — trivial literal subsumption on the inline test-fn call sites (per-parameter bounds).
- **L5 (3)** — Z3 QF-NIA discharges:
  - `overcurrent_margin: result >= -500000` (product-of-bounded-variables lower bound)
  - `thermal_stress: result >= 0` (positivity of two-variable product)
  - `thermal_stress: result <= 300000000` (upper bound of two-variable product)
- **runtime (3)** — obligations Z3 could not derive within timeout, deferred to runtime assertion:
  - `overcurrent_margin: result <= 1000000` (upper bound; requires deriving `actual*safety - pickup*100 <= 1_000_000` from the individual factor bounds — same shape as dose_scheduling's three-variable product upper bound)
  - `differential_deviation_pct: result >= 0` and `result <= 200` (nonlinear division with a variable divisor — genuinely harder for Z3 QF-NIA than pure multiplication)

## The nonlinear predicates

Three predicates exercise QF-NIA:

- **`overcurrent_margin(actual, pickup, safety_pct)`** — computes `actual × safety_pct − pickup × 100`. Two products of bounded variables. Same solver-theory shape as `dose_scheduling::total_infusion_dose`.
- **`thermal_stress(current_amps, duration_ms)`** — computes `current × duration`. Two-variable product with lower-bound positivity from bounded positive factors.
- **`differential_deviation_pct(i_in, i_out)`** — computes `|i_in − i_out| × 200 / (i_in + i_out)`. Combines abs-difference with **variable-denominator division**. This is where Z3 QF-NIA's heuristics reach their practical ceiling — same phenomenon dose_scheduling exposed for three-variable product upper bounds, now surfaced in a different arithmetic shape.

The runtime obligations are the honest reporting of Z3's ceiling. In the compiled binary they become assertions evaluated on concrete values at every execution; the tests exercise them incidentally.

## IFC boundary

SCADA commands from the control-centre RTU arrive as `Tainted[SCADACommand]`. Single audit anchor:

- `SCADA-CMD-001` — declassification via `admit_scada_command`.

Reproduce:
```bash
grep -n "SCADA-CMD-001" protection.mvl
```

Returns exactly one line. Every trust-boundary crossing on SCADA commands is grep-able. Same pattern as ETCS movement authority (`RBC-MA-001`) and CBTC train presence (`DISPATCHER-CMD-001`, `DISPATCHER-CMD-002`).

## Compound decision for MC/DC

`should_trip(fault_detected, block_command_active, is_critical_fault, manual_override)` — three-clause compound predicate with `fault_detected` appearing in two clauses. Under unique-cause MC/DC this is a structural coupling; under DO-178C masking MC/DC the coupled clause is exempt.

**Clinical rationale for the coupling** (documented in the code): a critical fault must trip even when SCADA has blocked non-critical trips — a planned maintenance-window block must not defeat life-safety protection.

Audit anchor: `MCDC-GRID-001`. Grep-reproducible.

**Current status:** `make mcdc` returns "No compound boolean conditions found" — this is the known `#1888` gap where the MC/DC tool doesn't scan production files reachable via test-file imports. The compound decisions are structured to activate MC/DC discovery once that fix lands.

## Explicitly out of scope for this example

Three properties inherent to substation protection are documented as future work, not developed here:

- **Coordination timing.** `t_upstream > t_downstream + coord_margin` across every fault scenario. Inductive-invariant approach handles the pointwise projection; full temporal correctness needs a model checker (UPPAAL, NuSMV).
- **Real-time trip latency.** Trip command must be emitted within 50 ms of fault detection. Timed-automata territory.
- **Liveness.** Any genuine fault eventually results in a trip. LTL/CTL model checking.

These are cited as the boundary where MVL's pointwise refinements meet their limit. See the refinement paper's Design Space §Failure Modes for the wider discussion; this example is a concrete demonstration of that boundary.

## Standard mapping (IEC 61508 SIL 2)

`make test-61508` composes the SIL-2 assurance envelope:

- Static refinement proof (compile-time, all inputs) — 20 proven / 3 runtime
- Behavioural unit tests — 12 passed
- Branch coverage — 92% (13/14 branches on production)
- MC/DC coverage — blocked by #1888; recovers on that landing

## Running the demo

```bash
make run
```

Produces:
```
1. normal operation:              hold  (no action)  (margin=-45000)
2. overcurrent fault:             TRIP  (fault confirmed)  (margin=65000)
3. overcurrent + SCADA block:     block (SCADA suppressed non-critical trip)
4. critical fault defeats block:  TRIP  (fault confirmed)
5. differential fault:            TRIP  (fault confirmed)  (deviation=50%)
6. manual override, no fault:     TRIP  (fault confirmed)
```

## Design decisions worth naming

**Priority ordering in `should_trip`.** Manual override is a separate clause independent of fault detection. Critical fault defeats SCADA block via the compound OR. These aren't cosmetic — they encode the safety priority the field operator expects.

**Runtime obligations are documented, not hidden.** Three obligations fall to runtime; the README calls each out with the reason. This is the pattern the refinement paper argues for: gradient of compile-time / runtime coverage, honestly reported, never silent.

**No division-by-zero in the physical shape.** `differential_deviation_pct` requires both `i_in` and `i_out` bounded below by 100 A, so the sum is always positive; division-by-zero at the type level is impossible. This is enforced by the struct field refinements on `CurrentMeasurement`.
