# etcs_movement_authority — Rail supervision case study

A refinement-typed ETCS Level 2 movement-authority supervision kernel. A Radio Block Centre (RBC) issues a Movement Authority (MA) to an on-board unit over a radio link; the on-board unit computes a brake supervision curve and decides accept / warn / service brake / emergency brake / MA-revoked.

**Standard.** EN 50128 (Railway Software) SIL 4 — the highest safety-critical software category, EU-mandated for ETCS.
**Ticket.** `mvl-lang/mvl#1906`.
**Domain distinctiveness.** First rail-signalling case study in the corpus. Nonlinear obligations come from physics (v² = u² − 2as), not from clinical dosing — broadens the paper's L5 claim beyond pharmacology.

## Files

- `model.mvl` — types (`TrainState`, `MovementAuthority`, `BrakeCurve`, `SupervisionResult`).
- `braking.mvl` — kinematics: three L5-forcing predicates + brake-curve construction + structural checks + inline unit tests.
- `supervision.mvl` — IFC boundary (`RBC-MA-001`), compound safety decisions (`MCDC-ETCS-001`, `MCDC-ETCS-002`), supervision kernel.
- `main.mvl` — six scenarios walking through NORMAL / WARNING / SERVICE / EMERGENCY / MA-REVOKED.
- `supervision_test.mvl` — end-to-end scenario tests + IFC round-trip.
- `Makefile` — standard targets plus `make test-50128` (SIL 4 assurance envelope).

## What is proven

`make prove` reports:

```
Summary: 29 proven (L1:23 L2:0 L3:0 L4:0 L5:6), 0 runtime, 0 failed
```

- **L1 (23)** — trivial literal subsumption on inline test-fn call sites.
- **L5 (6)** — Z3 QF-NIA discharges:
  - `braking_distance_squared_diff` bounds — squared-velocity difference (two independent squarings)
  - `kinetic_energy_index` bounds — mass × speed² (three-stage product)
  - `safety_margin_kinematic` bounds — mass × brake_ratio × time (three-variable product; same shape as `dose_scheduling::total_infusion_dose`)

Every L5 obligation stays inside QF-NIA's tractable envelope: two-variable squarings and three-variable products with bounded positive factors, propagated bounds. Zero runtime obligations — the entire kinematic layer is compile-time certified.

## IFC boundary (new axis for this case)

Single audit anchor: **`RBC-MA-001`** — sole `relabel trust` from `Tainted[MovementAuthority]` to a plain `MovementAuthority` the supervision kernel may consume.

The MA arrives from the RBC over the radio channel. Radio-borne payloads must be assumed adversarially controlled until an explicit audited crossing (SIL-4 principle: no trust without audit). All bounds validation from `model.mvl` continues to hold post-relabel — the type system guarantees that.

Reproduce the audit:

```bash
grep -n "RBC-MA-001" supervision.mvl
```

Returns exactly one line.

**Contrast with `data_integrity`.** `data_integrity` traces the *output* direction of the IFC axis (`Secret[T]` → `relabel release`). `etcs_movement_authority` traces the *input* direction (`Tainted[T]` → `relabel trust`). Together the two cases close the paper's IFC-completeness claim.

## Compound decisions for MC/DC

Two audit anchors:

- **`MCDC-ETCS-001`** — `needs_emergency_brake` — five atomic conditions with `!is_shunting && !override_active` as the coupled sub-clause. The two atoms cannot be factored into single-atom clauses without changing the semantics (an operator override only applies while in shunting mode). EN 50128 / DO-178C §6.4.4.2 masking exemption covers the coupling.
- **`MCDC-ETCS-002`** — `is_route_permitted` — four atomic conditions with `level_crossing_locked || !tunnel_active` as the interlock-coupled clause.

**Current status:** `make mcdc` returns "No compound boolean conditions found" — the known `#1888` gap. The compound decisions are structured to activate MC/DC discovery once #1888 lands.

## Standard mapping (EN 50128 SIL 4)

`make test-50128` composes the assurance envelope:

- Static refinement proof (compile-time, all inputs) — 29 proven / 0 runtime / 0 failed
- Behavioural unit tests — 25 passed
- Branch coverage — 89% (35/39 branches on production)
- MC/DC coverage — blocked by #1888; recovers on that fix landing
- Audit anchors — `MCDC-ETCS-001`, `MCDC-ETCS-002`, `RBC-MA-001` visible via grep

## Running the demo

```bash
make run
```

Produces:
```
1. normal running:                    NORMAL
2. warning zone:                      WARNING
3. service zone (shunting hold):      SERVICE BRAKE
4. emergency (above emergency line):  EMERGENCY BRAKE
5. service zone (no override):        EMERGENCY BRAKE
6. MA revoked:                        MA REVOKED
```

## Design decisions worth naming

**Nonlinear obligations come from physics, not from domain magic.** `v² = u² − 2as` forces QF-NIA at every stopping-distance calculation. This is the paper's key claim: L5's role is domain-agnostic — pharma dosing (`dose_scheduling`) and rail kinematics (`etcs_movement_authority`) both discharge the same shape of nonlinear obligation.

**Movement authority arrives over an untrusted channel.** The radio link between RBC and on-board unit is subject to interference, spoofing, and stale-message attacks. Enforcing `Tainted[MovementAuthority]` at the ingress makes the trust boundary a single grep-able anchor rather than an implicit assumption scattered through the code.

**Coupled clauses in `needs_emergency_brake` are deliberate.** The `!is_shunting && !override_active` sub-clause represents the physical semantics of dispatcher intervention: an override is only meaningful while shunting. Factoring it into single-atom clauses would mis-model the interlock. This is the pattern DO-178C §6.4.4.2 permits explicitly (masking exemption) and EN 50128 accepts by reference.

**Priority ordering in `supervise`.** MA revocation first (structural), then emergency brake (kinematic), then service, then warning, then normal running. The ordering doesn't affect safety guarantees but makes operational log analysis unambiguous.
