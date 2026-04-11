# 6. Pattern Matching

Pattern matching is the primary mechanism for working with sum types, Option, and Result. All matches MUST be exhaustive (Req 3).

## 6.1 Pattern Forms

```mvl
_                                   // wildcard — matches anything
x                                   // binding — matches and binds
42                                  // literal — matches exact value
"hello"                             // string literal
true                                // boolean literal
Circle(r)                           // variant destructuring
Point { x, y }                      // struct destructuring
(a, b)                              // tuple destructuring
Some(value)                         // Option::Some
None                                // Option::None
Ok(value)                           // Result::Ok
Err(error)                          // Result::Err
```

## 6.2 Exhaustiveness

The compiler MUST reject non-exhaustive matches:

```mvl
type Color = enum { Red, Green, Blue }

match color {
    Red => "red",
    Green => "green",
    // COMPILE ERROR: non-exhaustive match — Blue not handled
}
```

Adding a variant to an enum forces all match expressions to be updated. This is how the compiler turns type changes into compile errors at every usage site.

## 6.3 Guards

```mvl
match value {
    x where x > 0 => "positive",
    x where x < 0 => "negative",
    _ => "zero",
}
```

Guards add conditions to match arms. The compiler checks that guards + patterns together cover all cases.

## 6.4 Nested Patterns

```mvl
match result {
    Ok(Some(user)) => process(user),
    Ok(None) => default_user(),
    Err(e) => handle_error(e),
}
```

## 6.5 Let-Else (Refutable Patterns)

Refutable patterns in `let` are not permitted. Use `match` or `if let`:

```mvl
// COMPILE ERROR:
let Some(user) = find_user(id);     // what if None?

// Correct:
match find_user(id) {
    Some(user) => process(user),
    None => handle_missing(),
}
```
