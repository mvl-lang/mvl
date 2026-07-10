# Pattern 005: Flat Control Flow

## Summary

MVL's `match` expression is powerful but produces deep nesting when used naively.
Five techniques — `if let`, let-match extraction, `if let` guards, `and_then`
chaining, and extract-to-helper — keep control flow at one level of indentation
and make the happy path obvious.

The guiding principle: **the error or empty case exits early; the happy path
never gets nested.**

## When to use

- Any function with two or more consecutive `match` expressions
- Any `match` arm whose body contains another `match`
- Any `match { None => {}, Some(v) => body }` (no-op arm)
- Parsers and loops that accumulate state via error flags

## When NOT to use

- A single `match` with equally-weighted arms (no clear "happy path") — exhaustive
  match is the right tool there
- State transition tables — see Pattern 004

---

## Technique 1: `if let` for no-op arms

**Trigger:** One match arm is an empty block `{}`.

```mvl
// BEFORE — None => {} is dead weight
match chars.get(cur) {
    None => {},
    Some(c) => {
        if c == "-" { negative = true; cur = cur + 1 } else {}
    }
}

// AFTER
if let Some(c) = chars.get(cur) {
    if c == "-" { negative = true; cur = cur + 1 } else {}
}
```

Also applies when iterating with optional lookups:

```mvl
// BEFORE
while i < len {
    match keys.get(i) {
        Some(k) => {
            match overlay.get(k) {
                Some(ov) => { out = out.put(k, merged(k, ov)) },
                None => {},
            }
        },
        None => {},
    }
    i = i + 1
}

// AFTER
while i < len {
    if let Some(k) = keys.get(i) {
        if let Some(ov) = overlay.get(k) {
            out = out.put(k, merged(k, ov))
        }
    }
    i = i + 1
}
```

---

## Technique 2: let-match extraction (the `?` equivalent)

**Trigger:** One match arm returns or exits; the success path needs a single
extracted value. This is MVL's substitute for the `?` operator.

```mvl
// BEFORE — happy path is nested inside the arm
match chars.get(cur) {
    None    => { return FloatPos::FPErr("empty number") },
    Some(c) => {
        if c == "-" { negative = true; cur = cur + 1 } else {}
        // ...continues deeply indented
    }
}

// AFTER — extract the value, continue flat
let c: String = match chars.get(cur) {
    None    => return FloatPos::FPErr("empty number"),
    Some(c) => c,
};
if c == "-" { negative = true; cur = cur + 1 } else {}
// everything below drops one indentation level
```

The guard arm uses `return` to terminate; the success arm yields the bare value
into the `let` binding. Works for any single-value extraction from `Option` or
a custom result type.

---

## Technique 3: `if let` guards for two-product matches

**Trigger:** Two nested matches both destructure a different value; the default
case is the same for all other combinations.

```mvl
// BEFORE — nested match just to reach the one special case
match base {
    ConfigValue::Map(b) => {
        match overlay {
            ConfigValue::Map(o) => ConfigValue::Map(merge_maps(b, o)),
            _                   => overlay,
        }
    },
    _ => overlay,
}

// AFTER — guard the special case, fall through to the default
if let ConfigValue::Map(b) = base {
    if let ConfigValue::Map(o) = overlay {
        return ConfigValue::Map(merge_maps(b, o))
    }
}
overlay
```

When both values are known up-front and the combination space is small, a
tuple match is also valid (see Pattern 004):

```mvl
match (base, overlay) {
    (ConfigValue::Map(b), ConfigValue::Map(o)) => ConfigValue::Map(merge_maps(b, o)),
    (_,                   _)                   => overlay,
}
```

---

## Technique 4: `.and_then()` for Option pipelines

**Trigger:** Two or more sequential `match opt { None => None, Some(v) => next(v) }`
— each propagating `None` and passing the value into the next step.

```mvl
// BEFORE — triple-nested, all doing the same None → None propagation
match cfg {
    ConfigValue::Map(m) => {
        match parts.get(idx) {
            Some(key) => {
                match m.get(key) {
                    Some(v) => get_path_parts(v, parts, idx + 1),
                    None    => None,
                }
            },
            None => None,
        }
    },
    _ => None,
}

// AFTER — one match, then a flat chain
match cfg {
    ConfigValue::Map(m) =>
        parts.get(idx)
            .and_then(|key: String| m.get(key))
            .and_then(|v: ConfigValue| get_path_parts(v, parts, idx + 1)),
    _ => None,
}
```

`and_then` signature: `Option[T]::and_then[U](self, f: fn(T) -> Option[U]) -> Option[U]`

---

## Technique 5: Extract inner match to a helper

**Trigger:** A match arm has a non-trivial body that itself contains another match.
The arm body becomes its own function.

```mvl
// BEFORE — three levels: parse_string → chars.get → parse_value
match parse_string(chars, pos) {
    StrPos::SPErr(e)    => KVPos::KVPErr(e),
    StrPos::SP(key, np) => {
        let cur: Int = skip_ws(chars, np);
        match chars.get(cur) {
            None        => KVPos::KVPErr("expected ':' after key"),
            Some(colon) => {
                if colon != ":" {
                    KVPos::KVPErr("expected ':', got: ".concat(colon))
                } else {
                    match parse_value(chars, cur + 1) {
                        ValuePos::VPErr(e)    => KVPos::KVPErr(e),
                        ValuePos::VP(jv, np2) => KVPos::KVP(key, jv, np2),
                    }
                }
            }
        }
    }
}

// AFTER — outer match is one level; helper owns the rest
match parse_string(chars, pos) {
    StrPos::SPErr(e)    => KVPos::KVPErr(e),
    StrPos::SP(key, np) => parse_kv_after_key(chars, key, skip_ws(chars, np)),
}

partial fn parse_kv_after_key(chars: List[String], key: String, cur: Int) -> KVPos {
    let colon: String = match chars.get(cur) {
        None    => return KVPos::KVPErr("expected ':' after key"),
        Some(c) => c,
    };
    if colon != ":" { return KVPos::KVPErr("expected ':', got: ".concat(colon)) }
    match parse_value(chars, cur + 1) {
        ValuePos::VPErr(e)    => KVPos::KVPErr(e),
        ValuePos::VP(jv, np2) => KVPos::KVP(key, jv, np2),
    }
}
```

The helper uses Technique 2 (let-match extraction) internally, so it stays flat too.

---

## Quick reference

| Situation | Pattern |
|---|---|
| `match x { None => {}, Some(v) => body }` | Technique 1: `if let Some(v) = x { body }` |
| Guard + single extracted value | Technique 2: `let v = match x { Err(e) => return Err(e), Ok(v) => v };` |
| Two nested matches, shared default | Technique 3: nested `if let` + early return, or tuple match |
| Option chain where `None` propagates | Technique 4: `.and_then(\|v\| ...)` |
| Match arm body contains another match | Technique 5: extract arm body to a helper function |

## Related

- Pattern 004 (State Machines) — exhaustive tuple match for transition tables
- `std/core.mvl` — `Option::and_then`, `Option::unwrap_or`
- `std/json.mvl` — real-world parser using Techniques 1, 2, and 5
- `std/config.mvl` — `merge` and `get_path_parts` as before/after examples
