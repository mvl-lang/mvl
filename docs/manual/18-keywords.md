# 18. Keywords Reference

Complete list of MVL keywords with definitions and the requirement they serve.

## Active Keywords (~25)

| Keyword | Category | Definition | Req |
|---------|----------|-----------|-----|
| `fn` | Declaration | Define a function | — |
| `let` | Statement | Immutable variable binding | 6 |
| `mut` | Modifier | Mark binding or borrow as mutable | 6 |
| `type` | Declaration | Define a type (struct, enum, alias, trait) | 1 |
| `struct` | Type | Product type (record with named fields) | 1 |
| `enum` | Type | Sum type (tagged union of variants) | 1 |
| `trait` | Type | Interface contract (method signatures) | 1 |
| `impl` | Declaration | Implement methods or traits for a type | 1 |
| `if` | Control | Conditional branch (also expression) | — |
| `else` | Control | Alternative branch | — |
| `match` | Control | Exhaustive pattern matching (also expression) | 3 |
| `for` | Control | Bounded iteration over iterator | 8 |
| `while` | Control | Unbounded loop (partial functions only) | 8 |
| `return` | Control | Early return from function | — |
| `const` | Declaration | Compile-time constant | — |
| `module` | Declaration | Namespace declaration | — |
| `use` | Import | Bring names into scope | — |
| `extern` | Declaration | Foreign function interface | — |
| `total` | Modifier | Function provably terminates | 8 |
| `partial` | Modifier | Function may not terminate | 8 |
| `move` | Expression | Transfer ownership | 6 |
| `consume` | Expression | Transfer isolated capability | 9 |
| `where` | Constraint | Refinement predicate or generic constraint | 10 |
| `true` | Literal | Boolean true | — |
| `false` | Literal | Boolean false | — |

## Built-in Type Names

| Name | Definition | Req |
|------|-----------|-----|
| `Some` | Option variant — value present | 4 |
| `None` | Option variant — value absent | 4 |
| `Ok` | Result variant — success | 5 |
| `Err` | Result variant — failure | 5 |
| `Option` | Absence type (replaces null) | 4 |
| `Result` | Fallibility type (replaces exceptions) | 5 |
| `Public` | IFC label — safe for any output | 11 |
| `Clean` | IFC label — sanitized data | 11 |
| `Tainted` | IFC label — external data | 11 |
| `Secret` | IFC label — cryptographic material | 11 |
| `iso` | Reference capability — isolated | 9 |
| `val` | Reference capability — deeply immutable | 9 |
| `ref` | Reference capability — local mutable | 9 |
| `tag` | Reference capability — opaque identity | 9 |

## Built-in Functions

| Name | Definition | Req |
|------|-----------|-----|
| `declassify()` | Lower security label (Secret → Public) | 11 |
| `sanitize()` | Clean external data (Tainted → Clean) | 11 |
| `panic()` | Unrecoverable error — terminate program | — |

## Reserved Keywords

Reserved for future use, not currently valid in programs:

`async`, `await`, `yield`, `macro`, `unsafe`
