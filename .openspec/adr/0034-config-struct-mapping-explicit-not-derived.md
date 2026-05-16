# ADR-0034: Struct mapping — explicit construction, not derived

**Status:** Accepted
**Date:** 2026-05-16
**Issues:** #804

---

## Context

Several stdlib functions return an untyped value tree (`ConfigValue`, `JsonValue`,
a database `Row`, etc.) that the caller wants to map into a typed struct:

```mvl
// config
let cfg = load_config(None, "MYAPP")?

// database
let row = db.query_one("SELECT port, host FROM settings")?

// json
let val = json.parse(raw)?
```

A recurring temptation is to make these generic over the target type:

```mvl
let cfg  = load_config[Config](None, "MYAPP")?
let row  = db.query_one[Settings]("SELECT ...")?
let val  = json.parse[Config](raw)?
```

This pattern requires the compiler or runtime to **derive** the field-by-field
extraction code for `T` — mapping string keys to struct fields, coercing value
types, and surfacing errors for missing or mistyped fields.

The question: is derived extraction a **type safety** feature, or a **convenience** feature?

---

## Decision

1. MVL does not provide derived struct construction hidden behind a generic
   parameter. There is no `FromConfig`, `FromRow`, `FromJson`, or equivalent
   auto-derive protocol.

2. Callers construct their target struct **explicitly** from the returned value tree:

   ```mvl
   let val = load_config(None, "MYAPP")?
   let cfg = Config {
       port: val.get("port")?.as_int()?,
       host: val.get("host")?.as_string()?,
   }
   ```

3. The struct constructor is the validation point. Refinements (`where self > 0`)
   are enforced there regardless of how the struct is populated. Derive does not
   add a new safety boundary — it only moves the same check behind generated code.

4. Generic deserialization syntax (e.g. `load_config[T]`) is **aspirational** —
   a future ergonomics improvement contingent on MVL gaining a `derive` or
   structural typeclass mechanism. It is not a safety requirement and must not
   drive language or stdlib design before that mechanism exists.

---

## Consequences

**Positive:**
- No language extension required. Ships with what MVL has today.
- Explicit field access is visible and auditable — the mapping is in the source,
  not in generated code.
- Type safety is not compromised: refinements are checked at construction in
  both approaches.
- Consistent across domains: config, JSON, database all use the same pattern.

**Negative:**
- Boilerplate: field names appear as string literals (`"port"`, `"host"`).
  Typos are runtime errors, not compile-time errors.
- No static exhaustiveness check for required fields — a forgotten field is
  caught at construction time, not earlier.

These are ergonomic gaps, not safety gaps.

---

## Rejected Alternatives

### `fn load_x[T: FromX]` with compiler-generated impls (derive)

Requires the compiler to introspect T's fields (names and types) at compile time
and emit extraction code. MVL has no `derive` mechanism (ADR-0013: no macros,
no reflection). Even when this exists, the type safety gain is zero: refinements
are validated at struct construction either way. Rejected as premature language
complexity — add when boilerplate is actually painful at scale.

### `fn load_x[T: FromX]` with manual protocol impls

Users implement `FromX for Config` by hand. Identical boilerplate to explicit
construction, just indirected through a protocol. Adds a protocol with no present
benefit. Rejected.

### Runtime reflection / structural introspection

Inspect T's field layout at runtime to drive extraction. Conflicts with MVL's
predictable resource usage requirement (ADR-0001) and the no-reflection stance
(ADR-0013). Rejected.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **Predictable resource usage** — strengthened: no runtime reflection, no
  generated dispatch tables. Explicit construction has constant, visible overhead.
- **Type safety** — unchanged: struct constructors validate refinements in both
  approaches. This decision neither adds nor removes a safety guarantee.
- All other requirements: consistent with.

### Design Principles (README)

- **Minimum viable** — strengthened: ships the smallest thing that works.
  Derive follows when boilerplate becomes genuinely painful.
- **Explicit over implicit** — strengthened: field mapping is written out;
  the programmer sees exactly what keys map to what fields.
- **No macros, no reflection** (ADR-0013) — consistent with.
- **Consistency** — strengthened: one pattern across config, JSON, database,
  and any future source of untyped value trees.
- All other principles: consistent with.

### Specifications

No existing specs are affected. This decision applies to all stdlib modules that
return untyped value trees (`std.config`, `std.json`, future `std.db`).
