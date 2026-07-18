# flight_fuel_planning — Aviation dispatch case study

A refinement-typed airline dispatch fuel-calculation kernel. Given a flight plan (distance, cruise speed, alternate distance, contingency percentage, reserve requirements) and a fuel-burn model, compute the minimum uplift fuel and emit a dispatchability verdict (dispatchable / requires review / requires second alternate / undispatchable).

**Standard.** DO-178C Class C (major criticality for FMS-adjacent software) + EU-OPS 1.255 + FAA 14 CFR §91.167.
**Ticket.** `mvl-lang/mvl#1907`.
**Domain distinctiveness.** First aviation case study in the corpus. Completes the medical/rail/aviation triangulation that supports the paper's L5-is-domain-agnostic claim.

## Files

- `model.mvl` — types (`FlightPlan`, `FuelBurn`, `FuelUplift`, `DispatchResult`, `UndispatchReason`).
- `calc.mvl` — fuel calculations: three L5-forcing predicates + structural checks + inline unit tests.
- `dispatch.mvl` — IFC boundary (`WX-MINIMA-001`), compound safety decisions (`MCDC-FUEL-001`, `MCDC-FUEL-002`), dispatch kernel.
- `main.mvl` — six scenarios walking through the dispatch outcomes.
- `dispatch_test.mvl` — end-to-end scenario tests + IFC round-trip + ETOPS-specific coverage.
- `Makefile` — standard targets plus `make test-178c` (DO-178C Class C assurance envelope).

## What is proven

`make prove` reports:

```
Summary: 36 proven (L1:24 L2:6 L3:0 L4:0 L5:6), 0 runtime, 0 failed
```

- **L1 (24)** — trivial literal subsumption on inline test-fn call sites.
- **L2 (6)** — interval discharges from struct-field bound propagation in `compute_uplift`. Struct-typed inputs carry their refinements to call sites; MVL discharges the resulting obligations at the interval layer.
- **L5 (6)** — Z3 QF-NIA discharges:
  - `trip_fuel_kg` bounds (distance × burn / speed — two-variable product with bounded quotient)
  - `alternate_fuel_kg` bounds (same shape, tighter envelope)
  - `total_uplift_ceiling` bounds (three-variable product — same shape as `dose_scheduling::total_infusion_dose` and `etcs_movement_authority::safety_margin_kinematic`)

Zero runtime obligations — the entire fuel-arithmetic layer is compile-time certified. Bounds propagate cleanly from input envelopes to result envelopes.

## IFC boundary (weather-minima ingest)

Single audit anchor: **`WX-MINIMA-001`** — sole `relabel trust` from `Tainted[Bool]` to plain `Bool` for weather-minima bits arriving from the Aeronautical Information Service (AIS) feed.

Reproduce the audit:

```bash
grep -n "WX-MINIMA-001" dispatch.mvl
```

Returns exactly one line. Weather NOTAMs and TAFs are advisory external data; treating them as `Tainted` until an audited crossing forces every consuming code path to pass through the single anchor.

## Compound decisions for MC/DC

Two audit anchors:

- **`MCDC-FUEL-001`** — `requires_dispatch_review` — five atomic conditions with `weather_alt_marginal && !is_etops` as the coupled sub-clause. Marginal alternate weather only forces review when the flight is NOT an ETOPS operation (ETOPS has more stringent alternate rules that supersede the marginal check). DO-178C §6.4.4.2 masking exemption covers the coupling.
- **`MCDC-FUEL-002`** — `requires_second_alternate` — four atomic conditions with `!dest_wx_ok && !alt_wx_ok` as the coupled sub-clause. A second alternate is only required when BOTH destination and first alternate have degraded weather.

**Current status:** `make mcdc` returns "No compound boolean conditions found" — the known `#1888` gap. The compound decisions are structured to activate MC/DC discovery once #1888 lands.

## Standard mapping (DO-178C Class C)

`make test-178c` composes the assurance envelope:

- Static refinement proof (compile-time, all inputs) — 36 proven / 0 runtime / 0 failed
- Behavioural unit tests — 27 passed
- Branch coverage — 93% (28/30 branches on production)
- MC/DC coverage — blocked by #1888; recovers on that fix landing
- Audit anchors — `MCDC-FUEL-001`, `MCDC-FUEL-002`, `WX-MINIMA-001` visible via grep

## Running the demo

```bash
make run
```

Produces:
```
1. short haul, standard:              DISPATCHABLE
2. high load (>95% capacity):         REQUIRES REVIEW
3. marginal alt weather (non-ETOPS):  REQUIRES REVIEW
4. both dest+alt weather degraded:    REQUIRES SECOND ALTERNATE
5. long haul over capacity:           UNDISPATCHABLE (over capacity)
6. medical diversion needs:           REQUIRES REVIEW
```

## Design decisions worth naming

**Fuel physics is the L5 driver.** distance × burn / speed is a two-variable product with a bounded quotient — Z3 QF-NIA discharges the bounds cleanly. Just like ETCS's brake curves and dose_scheduling's infusion products, the nonlinearity comes from physics/pharmacokinetics, not from domain magic. This is the paper's L5 claim.

**Envelope-cap saturation is deliberate.** `cap()` in `dispatch.mvl` saturates each fuel component at its declared struct-field envelope. This means the `FuelUplift` struct-invariant holds by construction regardless of input pathology — the type system enforces the ceiling, and downstream consumers can rely on it without re-validation.

**ETOPS as a suppressor, not an amplifier.** The `!is_etops` conjunct in `requires_dispatch_review` deliberately suppresses the marginal-weather review for ETOPS operations, because ETOPS has its own more stringent alternate rules. Modelling this as a coupled clause (rather than a separate branch) makes the DO-178C §6.4.4.2 masking exemption explicit.

**Priority ordering in `dispatch`.** Capacity check first (structural), then second-alternate weather check (regulatory), then review triggers (advisory), then dispatchable. The ordering matches operational dispatch practice — hard failures dominate soft ones.
