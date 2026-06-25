# 6. Pattern Matching

Pattern matching is the primary mechanism for working with sum types, Option, and Result. All matches MUST be exhaustive ([Req 3](../requirements.md#req-3)).

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

Guards attach a boolean condition to a match arm using `if` (not `where` — `where` is reserved for refinement types):

```mvl
match value {
    x if x > 0 => "positive",
    x if x < 0 => "negative",
    _ => "zero",
}
```

A guarded arm fires only when the pattern matches AND the condition holds. A guarded arm does NOT contribute to exhaustiveness, because the compiler cannot prove the guard is true — you still need a fallback arm:

```mvl
match opt {
    Some(v) if v > 0 => v,
    Some(_)          => 0,     // fallback for Some(v) when guard fails
    None             => 0,
}
```

Use guards instead of nesting an `if` inside an arm body. See [§6.8 Avoiding Deep Nesting](#68-avoiding-deep-nesting).

## 6.4 OR Patterns

Multiple patterns can share one arm using `|`:

```mvl
type Direction = enum { North, South, East, West }

fn is_horizontal(d: Direction) -> Bool {
    match d {
        East | West   => true,
        North | South => false,
    }
}

fn is_weekend(day: Int) -> Bool {
    match day {
        0 | 6 => true,
        _     => false,
    }
}
```

OR patterns collapse parallel cases without duplicating arm bodies. Prefer them over chains of `==` comparisons.

## 6.5 Nested Patterns

```mvl
match result {
    Ok(Some(user)) => process(user),
    Ok(None)       => default_user(),
    Err(e)         => handle_error(e),
}
```

Nested patterns are powerful but cost readability past two levels. For deeper structures, see [§6.8 Avoiding Deep Nesting](#68-avoiding-deep-nesting).

## 6.6 Let-Else (Refutable Patterns)

Refutable patterns in `let` are not permitted. Use `match` or `if let`:

```mvl
// COMPILE ERROR:
let Some(user) = find_user(id);     // what if None?

// Correct:
match find_user(id) {
    Some(user) => process(user),
    None       => handle_missing(),
}
```

## 6.7 `if let` and `while let`

When you only care about one variant, a full `match` is noise. Use `if let` to bind and run:

```mvl
// verbose
match config.timeout {
    Some(t) => apply_timeout(t),
    None    => {},
}

// idiomatic
if let Some(t) = config.timeout {
    apply_timeout(t);
}
```

`if let` also has an `else` form that desugars to an exhaustive `match` expression:

```mvl
let n: Int = if let Ok(v) = parse(s) { v } else { 0 };
```

`while let` drains an iterator-like value until the pattern stops matching:

```mvl
let iter: ref Iterator[T] = items.iter();
while let Some(x) = iter.next() {
    process(x);
}
```

This is what `for x in items { ... }` desugars to internally — no index variable, no off-by-one risk.

**When NOT to use `if let`:**

- When you care about both branches with different logic — use `match`, so the compiler tells you about future variants.
- On `Result` when the error branch needs handling — `if let Ok(v) = res { ... }` silently drops `Err`. The linter warns on this pattern (see corpus `14_linting/silent_result_discard.mvl`).

## 6.8 Avoiding Deep Nesting

Two levels of `match` nesting is the soft limit. Past that, readability collapses fast. Four strategies, from cheapest to heaviest:

### 6.8.1 Tuple-match instead of nested match

If you're dispatching on two values at once, build a tuple and match on it. One match, flat arms:

```mvl
// NESTED — hard to see the full decision table
fn describe(a: Option[Int], b: Option[Int]) -> String {
    match a {
        Some(x) => match b {
            Some(y) => "both",
            None    => "only a",
        },
        None => match b {
            Some(_) => "only b",
            None    => "neither",
        },
    }
}

// FLAT — the decision table is visible in one match
fn describe(a: Option[Int], b: Option[Int]) -> String {
    match (a, b) {
        (Some(_), Some(_)) => "both",
        (Some(_), None)    => "only a",
        (None,    Some(_)) => "only b",
        (None,    None)    => "neither",
    }
}
```

### 6.8.2 Guards instead of nested `if`

If you find yourself writing `if` inside a match arm body, hoist the condition into a guard:

```mvl
// NESTED
match msg {
    Request(r) => {
        if r.priority > 5 {
            handle_urgent(r)
        } else {
            handle_normal(r)
        }
    },
    Other(o) => handle_other(o),
}

// FLAT
match msg {
    Request(r) if r.priority > 5 => handle_urgent(r),
    Request(r)                   => handle_normal(r),
    Other(o)                     => handle_other(o),
}
```

### 6.8.3 Early return / `if let` to flatten Result/Option chains

A pyramid of `match` on successive `Result` calls flattens with early-return:

```mvl
// PYRAMID
fn load(id: UserId) -> Result[Profile, Error] {
    match find_user(id) {
        Ok(user) => match fetch_settings(user) {
            Ok(settings) => match build_profile(user, settings) {
                Ok(p)  => Ok(p),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    }
}

// FLAT — each step bails on Err, success path stays at the top level
fn load(id: UserId) -> Result[Profile, Error] {
    let user: User = match find_user(id) {
        Ok(u)  => u,
        Err(e) => return Err(e),
    };
    let settings: Settings = match fetch_settings(user) {
        Ok(s)  => s,
        Err(e) => return Err(e),
    };
    build_profile(user, settings)
}
```

### 6.8.4 Extract a helper per outer arm

When an inner match is large enough to be its own concept, give it a name:

```mvl
fn route(req: Request) -> Response {
    match req.kind {
        Get(path)  => handle_get(path, req),
        Post(path) => handle_post(path, req),
        Delete(id) => handle_delete(id, req),
    }
}

fn handle_get(path: String, req: Request) -> Response { ... }
fn handle_post(path: String, req: Request) -> Response { ... }
```

The outer match becomes a dispatch table. Each handler matches on its own narrower domain. The signatures document the threat model — effects, labels, and ownership all stay visible per branch.

## 6.9 Why Match Over `if`/`else` Chains

The point of `match` is not terseness — it's that the compiler tracks variants for you. When a new variant is added to a sum type, every `match` that doesn't handle it fails to compile, pointing at the exact location. An `if`/`else if` chain on the same data degrades silently: the `else` branch swallows the new case, and the bug surfaces in production.

Rules of thumb:

- **Enum or sum type → `match`.** Never `if x == Foo::A { ... } else if x == Foo::B { ... }`.
- **`Option` / `Result` → `match`, `if let`, or the dedicated methods (`unwrap_or`, `map`, `?`-style propagation via `match … return Err(e)`).** Never a bare `unwrap()` — it doesn't exist.
- **Boolean → `if`.** A two-arm match on `Bool` is noise; `if cond { ... } else { ... }` reads better.
- **Integer or string dispatch on a closed set → `match` with OR patterns.** Open set (parsing arbitrary input) → `match` with a `_` fallback that surfaces the error explicitly, never silently.

The feedback loop you want is: adding a variant → compile error at every dispatch site → fix → ship. Not: adding a variant → silent fallthrough → production incident.
