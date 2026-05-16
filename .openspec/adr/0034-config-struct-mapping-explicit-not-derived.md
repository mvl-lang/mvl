# ADR-0034: Config struct mapping — explicit construction, not derived

**Status:** Accepted
**Date:** 2026-05-16
**Issues:** #804

---

## Context

When implementing `std.config`, the question arose whether to provide a generic
`load_config[T: FromConfig](path, prefix) -> Result[T, ConfigError]` that
automatically deserializes a config file into a user-defined struct `T`, or to
return an untyped `ConfigValue` tree that callers map to their struct manually.

The `load_config[T]` approach requires either:

1. **Derive macros / codegen** — the compiler generates field-by-field extraction
   code for every struct annotated with `derive(FromConfig)`.
2. **A `FromConfig` protocol** — callers implement a conversion trait; the compiler
   enforces the bound but cannot auto-generate the impl.

MVL currently has neither mechanism (see ADR-0013: no macros, no reflection).
Adding either would be a significant language extension, not a stdlib addition.

More critically: the safety argument for `load_config[T]` does not hold.

---

## Decision

1. `load_config` returns `ConfigValue` (an untyped value tree, same pattern as
   `std.json`'s `Value`). No generic parameter. No `FromConfig` protocol.

2. Callers construct their target struct **explicitly** from the returned
   `ConfigValue`, using field accessors:

   ```mvl
   let val = load_config(None, "MYAPP")?
   let cfg = Config {
       port: val.get("port")?.as_int()?,
       host: val.get("host")?.as_string()?,
   }
   ```

3. `load_config[T]` syntax is **aspirational only** — noted in the issue and
   stdlib docstring as a future ergonomics improvement, not a safety requirement.

4. Struct construction is the validation point. Refinements (`where self > 0`)
   are checked there regardless of how the struct is populated.

---

## Consequences

**Positive:**
- Ships immediately. No language changes required.
- Explicit field access makes the mapping visible and auditable.
- Type safety is not compromised: the struct constructor enforces refinements
  whether the value comes from `ConfigValue` or a derived impl.
- Consistent with `std.json` — users already know the `Value` access pattern.

**Negative:**
- Boilerplate: callers repeat field names as string literals (`"port"`, `"host"`).
  Typos are runtime errors, not compile errors.
- No exhaustiveness check: a forgotten required field is caught at construction
  time (missing field error), not statically.

These are ergonomic gaps, not safety gaps. They are acceptable at this stage of
the language.

---

## Rejected Alternatives

### `load_config[T: FromConfig]` with derive

Requires the compiler to generate `impl FromConfig for T` by introspecting T's
fields at compile time — field names and types. This needs either macro expansion
or a `derive` mechanism, neither of which exists in MVL (ADR-0013). Even if it
did, the safety gain over explicit construction is zero: refinements are validated
at struct construction in both cases. Rejected as premature language complexity.

### `load_config[T: FromConfig]` with manual impl

Users implement `FromConfig for Config` manually. This is identical boilerplate
to explicit construction, just wrapped in a protocol impl. No ergonomic gain.
Adds a protocol to the stdlib with no present benefit. Rejected.

### Protocol-based structural introspection

Require T to expose field names/types via a reflection protocol. This conflicts
with MVL's "no runtime reflection" stance (ADR-0001, requirement: predictable
resource usage). Rejected.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **Predictable resource usage** — strengthened: no runtime reflection, no
  generated dispatch tables. Explicit field access has constant overhead.
- **Type safety** — unchanged: struct construction validates refinements either
  way. This decision neither adds nor removes a safety guarantee.
- All other requirements: consistent with.

### Design Principles (README)

- **Minimum viable** — strengthened: ships the smallest thing that works.
  Derive/protocol can follow when boilerplate becomes painful at scale.
- **Explicit over implicit** — strengthened: field mapping is written out; the
  programmer sees exactly what keys map to what fields.
- **No macros, no reflection** (ADR-0013) — consistent with: no codegen,
  no runtime reflection used.
- **Consistency** — consistent with: mirrors `std.json`'s `Value` access pattern.
- All other principles: consistent with.

### Specifications

No specs in `.openspec/specs/` currently cover `std.config`. The `std.config`
module and its `load_config` signature are documented in `std/config.mvl`.
No spec updates required.
