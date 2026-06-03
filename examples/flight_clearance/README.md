# flight_clearance

Flight dispatch decision logic — demonstrates **MC/DC coverage** and **pure domain modeling**.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Pure functions | No `!` annotations | Fully testable business logic |
| ADT modeling | `type Clearance = enum { Cleared, Rejected }` | Domain concepts as types |
| Compound decisions | `wx.icing && !ac.etops_cert` | 9 boolean expressions exercised |
| Exhaustive scenarios | `s1_domestic_clear()` ... `s9_...` | All decision paths covered |

---

## Domain model

| Type | Variants/Fields | Purpose |
|------|-----------------|---------|
| `Weather` | visibility, wind, icing, thunderstorm | Meteorological conditions |
| `Aircraft` | maintenance, fuel, crew_valid, etops_cert | Aircraft readiness |
| `RouteType` | Domestic, International, Oceanic | Flight category |
| `Clearance` | Cleared, Rejected | Decision outcome |

---

## Decision functions

| Function | Decision logic |
|----------|---------------|
| `departure_gate()` | Main clearance decision — combines weather, aircraft, route |
| `check_needs_diversion()` | Severe weather forces diversion |
| `check_fuel_planning_ok()` | Fuel margin vs route type |
| `check_route_permitted()` | ETOPS certification for oceanic |

---

## MC/DC coverage

Each compound boolean is tested with scenarios that toggle individual conditions:

```
thunderstorm && !wx.icing           → s2, s7
fuel == Marginal && route == Intl   → s3, s4
visibility == Poor || wind == High  → s5, s6
```

---

## Running

```bash
make build
cd examples/flight_clearance
make test
```

---

## Related

- Spec: `.openspec/specs/010-mcdc/spec.md`
- Assurance note: 0 extern blocks (pure logic demo)
