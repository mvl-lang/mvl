# 1. Lexical Structure

## 1.1 Source Encoding

MVL source files MUST be UTF-8 encoded. No BOM. File extension: `.mvl`.

## 1.2 Keywords

The MVL has approximately 25 keywords. Each serves a specific verification purpose.

| Keyword | Purpose | Requirement |
|---------|---------|-------------|
| `fn` | Function declaration | — |
| `let` | Immutable binding | [Req 6](../requirements.md#req-6) (default immutable) |
| `mut` | Mutable qualifier | [Req 6](../requirements.md#req-6) (explicit mutability) |
| `type` | Type declaration | [Req 1](../requirements.md#req-1) (ADTs) |
| `struct` | Product type | [Req 1](../requirements.md#req-1) |
| `enum` | Sum type | [Req 1](../requirements.md#req-1) |
| `trait` | Interface/contract | [Req 1](../requirements.md#req-1) |
| `impl` | Implementation block | [Req 1](../requirements.md#req-1) |
| `if` / `else` | Conditional | — |
| `match` | Pattern matching | [Req 3](../requirements.md#req-3) (exhaustive) |
| `for` | Bounded iteration | [Req 8](../requirements.md#req-8) (termination) |
| `while` | Unbounded iteration | [Req 8](../requirements.md#req-8) (partial only) |
| `return` | Early return | — |
| `module` | Namespace | — |
| `const` | Compile-time constant | — |
| `extern` | Foreign function interface | — |
| `total` | Provably terminating | [Req 8](../requirements.md#req-8) |
| `partial` | May not terminate | [Req 8](../requirements.md#req-8) |
| `move` | Transfer ownership | [Req 6](../requirements.md#req-6) |
| `consume` | Transfer isolated cap | [Req 9](../requirements.md#req-9) |
| `where` | Refinement predicate / constraint | [Req 10](../requirements.md#req-10) |
| `true` / `false` | Boolean literals | — |
| `Some` / `None` | Option constructors | [Req 4](../requirements.md#req-4) |
| `Ok` / `Err` | Result constructors | [Req 5](../requirements.md#req-5) |

Reserved for future use: `async`, `await`, `yield`, `macro`, `unsafe`.

## 1.3 Identifiers

```
IDENT = ALPHA { ALPHA | DIGIT | "_" }
ALPHA = "a"..."z" | "A"..."Z" | "_"
DIGIT = "0"..."9"
```

Naming conventions (enforced by compiler warning):

- Types: `PascalCase` — `UserProfile`, `HttpError`
- Functions and variables: `snake_case` — `find_user`, `max_retries`
- Constants: `SCREAMING_SNAKE` — `MAX_CONNECTIONS`, `PI`
- Modules: `snake_case` — `module http_client`
- Type parameters: single uppercase — `T`, `E`, `K`, `V`

## 1.4 Literals

### Integer literals

```
42                  // decimal
0xFF                // hexadecimal
0b1010              // binary
0o77                // octal
1_000_000           // underscores for readability
```

Integer literals have type `Int` (arbitrary precision) by default. Assign to a fixed-width type to narrow: `let x: Int32 = 42`.

### Float literals

```
3.14                // Float64 by default
1.0e10              // scientific notation
```

### String literals

```
"hello world"       // basic string (UTF-8)
"line 1\nline 2"    // escape sequences: \n \t \r \\ \" \0
```

No string interpolation. Use `format()` from stdlib. Rationale: string interpolation hides effects and IFC label mixing ([Req 7](../requirements.md#req-7), [Req 11](../requirements.md#req-11)).

### Character literals

```
'a'                 // single Unicode scalar value
'\n'                // escape sequences
```

### Collection literals

```
[1, 2, 3]           // Array<Int>
{1, 2, 3}           // Set<Int>
{"a": 1, "b": 2}    // Map<String, Int>
(1, "hello")        // Tuple (Int, String)
()                  // Unit
```

Note: empty `{}` is ambiguous between empty set and empty map. Requires type annotation: `let m: Map<K,V> = {}`.

## 1.5 Comments

```
// line comment — the only comment form
/// doc comment — convention, recognized by doc tool
```

No block comments (`/* */`). Rationale: LLMs generate all code — no need to "comment out sections." Block comments nest badly and add parser complexity for zero verification value.

## 1.6 Operators

See [Chapter 19: Operators and Precedence](19-operators.md) for the complete table.

## 1.7 Semicolons

Statements end with `;`. Expressions used as statements end with `;`. The last expression in a block is the block's value (no `;`).

```
fn max(a: Int, b: Int) -> Int {
    if a > b { a } else { b }   // no semicolon — this is the return value
}
```

## 1.8 Whitespace

Whitespace (spaces, tabs, newlines) is insignificant except as token separator. No significant indentation.
