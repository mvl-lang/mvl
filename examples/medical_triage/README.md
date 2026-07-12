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
| `make prove` | Refinement contracts discharged by solver | 9 obligations, L1:5 L2:1 L3:1 L4:2 |

The reassessment helpers deliberately span the refinement-solver layers:

| Function | Layer | What it exercises |
|----------|-------|-------------------|
| `reassess_interval` (5 arms) | **L1 trivial** | Each match arm returns a literal fitting `[0, 240]` |
| `cap_stable_reassess` | **L2 interval** | Parameter's `[0, 60]` range is a subset of the postcondition's `[0, 240]` |
| `sanitize_reassess` | **L3 symbolic** | Enumerates the three paths of `clamp_to_mts_range` and proves each independently fits `[0, 240]` |
| `buffered_reassess_min` | **L4 Presburger** | From `mins >= 0`, derives `mins + 5 >= 5` via Fourier-Motzkin |
| `buffered_reassess_max` | **L4 Presburger** | From `mins <= 240`, derives `mins + 5 <= 245` via Fourier-Motzkin |

All nine obligations discharge without falling through to Z3 (L5) or runtime.

---

## Related

- Assurance note: 0 extern blocks (pure logic demo)
- Spec: `.openspec/specs/000-parser/spec.md` (ADT syntax)
