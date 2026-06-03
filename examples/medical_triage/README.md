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
make build
cd examples/medical_triage
make test
```

---

## Related

- Assurance note: 0 extern blocks (pure logic demo)
- Spec: `.openspec/specs/000-parser/spec.md` (ADT syntax)
