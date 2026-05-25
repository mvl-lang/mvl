# 19. Operators and Precedence

No operator overloading. Every operator has exactly one meaning.

## 19.1 Operator Table

Listed from highest to lowest precedence:

| Prec | Operator | Associativity | Description |
|------|----------|--------------|-------------|
| 1 | `.` | Left | Field access, method call |
| 1 | `()` | — | Function call |
| 1 | `[]` | Left | Index access (returns `Option[T]`) |
| 1 | `?` | Postfix | Result/Option propagation |
| 2 | `!` `~` | Prefix | Logical NOT, bitwise NOT |
| 2 | `-` | Prefix | Numeric negation |
| 3 | `*` `/` `%` | Left | Multiplication, division, modulo |
| 4 | `+` `-` | Left | Addition, subtraction |
| 5 | `<<` `>>` | Left | Bitwise shift left / right |
| 6 | `==` `!=` | Left | Equality comparison |
| 6 | `<` `>` `<=` `>=` | Left | Ordering comparison |
| 7 | `&` | Left | Bitwise AND |
| 8 | `^` | Left | Bitwise XOR |
| 9 | `\|` | Left | Bitwise OR |
| 10 | `&&` | Left | Logical AND (short-circuit) |
| 11 | `\|\|` | Left | Logical OR (short-circuit) |
| 12 | `=` | Right | Assignment (mut bindings only) |

## 19.2 Arithmetic

All arithmetic operators work on numeric types only. No operator overloading — `+` cannot mean "string concatenation" or "matrix addition." Use named methods instead:

```mvl
// Numbers:
let sum = a + b;

// Strings:
let greeting = "hello".concat(" world");

// Collections:
let merged = list_a.concat(list_b);
```

## 19.3 Checked Arithmetic

On fixed-width integers (`Int32`, `UInt64`, etc.):

| Method | Behavior on overflow |
|--------|---------------------|
| `a + b` | Compile error if overflow possible |
| `a.checked_add(b)` | Returns `Option[T]` |
| `a.wrapping_add(b)` | Wraps around |
| `a.saturating_add(b)` | Clamps to min/max |

On `Int` (arbitrary precision): no overflow possible.

## 19.4 Comparison

All comparison operators return `Bool`. Types must implement `Eq` (for `==`, `!=`) or `Ord` (for `<`, `>`, `<=`, `>=`).

## 19.5 Bitwise Operators

Bitwise operations are first-class operators on integer types (see precedence in §19.1):

```mvl
let mask  = flags & 0xFF;
let shifted = value << 3;
let flipped = flags ^ mask;
let inv   = ~flags;
```

Note: bitwise operators bind more tightly than `&&`/`||` but more loosely than `==`/`!=`. Shift operators (`<<`/`>>`) bind tighter than comparisons. Right shift (`>>`) uses arithmetic (sign-extending) semantics.

Method aliases (`bit_and`, `bit_or`, `shift_left`, etc.) are also supported for clarity.
