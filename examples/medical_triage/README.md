# medical_triage

Emergency department triage logic — demonstrates **pure domain modeling** and **exhaustive enum matching**.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| ADT modeling | `type Priority = enum { Red, Orange, Yellow, Green, Blue }` | Clinical priority levels |
| Nested structs | `Patient { vitals: Vitals, ... }` | Compound domain types |
| Pure assessment | `fn assess(p: Patient) -> Assessment` | No effects, fully testable |
| Exhaustive match | `match p.vitals.consciousness { ... }` | All variants handled |

---

## Domain model

| Type | Fields | Purpose |
|------|--------|---------|
| `Vitals` | heart_rate, systolic_bp, oxygen_sat, temperature, consciousness, breathing, pulse | Patient measurements |
| `Patient` | age_group, complaint, vitals, allergies, pregnant | Complete patient state |
| `Priority` | Red, Orange, Yellow, Green, Blue | ESI-like triage levels |
| `Assessment` | priority, notes | Triage outcome |

---

## Clinical scenarios

| Scenario | Input | Expected Priority |
|----------|-------|-------------------|
| Stable adult | Normal vitals, minor complaint | GREEN |
| Unresponsive | consciousness=Unresponsive | RED |
| Elderly chest pain | age=Elderly, complaint=ChestPain | ORANGE |
| Pediatric fever | age=Child, high temp | ORANGE |
| Pregnant trauma | pregnant=true, complaint=Trauma | ORANGE |
| Low oxygen | oxygen_sat < 90 | RED |

---

## Running

```bash
make build                       # build the compiler once, from repo root
cd examples/medical_triage

make check          # type-check all sources
make test           # run unit tests (79 tests)
make coverage       # branch coverage report
make mcdc           # MC/DC coverage (DO-178C DAL-A)
make prove          # per-call-site refinement proof breakdown
make assurance      # full assurance report (spec + MC/DC + mutation)
```

---

## Assurance surface

| Layer | What it verifies | Current result |
|-------|------------------|----------------|
| `make check` | Type-check + totality | 10/11 requirements proven per file |
| `make test` | Behaviour on scenarios | 79/79 tests pass |
| `make coverage` | Branch coverage of test suite | 100% (33/33 branches) |
| `make mcdc` | MC/DC clause independence | 100% (57/57 pure obligations) |
| `make prove` | Refinement contracts discharged by solver | 5 obligations, all Layer 1 |

The `reassess_interval` function carries an `ensures result >= 0 && result <= 240`
postcondition — the solver proves it once per match arm, giving `mvl prove` real
work to do.  Add further `ensures` clauses to the boolean helpers to expand the
refinement-proof surface.

---

## Related

- Assurance note: 0 extern blocks (pure logic demo)
- Spec: `.openspec/specs/000-parser/spec.md` (ADT syntax)
