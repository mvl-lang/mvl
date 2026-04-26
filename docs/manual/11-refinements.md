# 11. Refinement Types

Refinement types constrain values beyond their base type ([Req 10](../requirements.md#req-10)). The compiler verifies predicates at compile time using SMT solving.

## 11.1 Syntax

```mvl
Type where predicate
```

The predicate is a boolean expression over the value and its fields.

## 11.2 Examples

```mvl
// Non-zero divisor
fn divide(a: Int, b: Int where b != 0) -> Int {
    a / b
}

// Valid port number
fn listen(port: UInt16 where port >= 1 && port <= 65535) -> () ! Net {
    // ...
}

// Non-empty collection
fn first[T](items: Array[T] where len(items) > 0) -> T {
    items[0]     // safe — compiler knows items is non-empty
}

// Bounded array index
fn get_safe(arr: Array[Int], i: UInt where i < len(arr)) -> Int {
    arr[i]       // safe — compiler knows i is in bounds
}
```

## 11.3 Predicate Language

Refinement predicates support:

| Form | Example |
|------|---------|
| Comparison | `x > 0`, `x != 0`, `x <= 100` |
| Logical | `x > 0 && x < 100`, `x == 0 || y > 0` |
| Negation | `!is_empty` |
| Arithmetic | `x + y < 100`, `x % 2 == 0` |
| Length | `len(items) > 0` |
| Field access | `self.age >= 18` |

## 11.4 Verification Strategy

1. **Static verification (preferred):** The compiler uses an SMT solver (Z3) to prove the predicate holds at every call site. If provable, no runtime check.

2. **Runtime check (fallback):** If the compiler cannot statically verify, it inserts a runtime check. The function then returns `Result` instead of the base type.

3. **Propagation:** Refinements propagate through the program. If `x: Int where x > 0`, then `x + 1` is known to be `> 1`.

## 11.5 Named Refinement Types

```mvl
type NonZero = Int where self != 0
type Percentage = Float64 where self >= 0.0 && self <= 100.0
type NonEmpty[T] = Array[T] where len(self) > 0
type ValidPort = UInt16 where self >= 1 && self <= 65535
```

## 11.6 Checked Arithmetic

Default arithmetic on fixed-width integers is checked:

```mvl
let a: Int32 = Int32.MAX;
let b = a + 1;                       // COMPILE ERROR: potential overflow
let b = a.checked_add(1);            // Option<Int32> — None on overflow
let b = a.wrapping_add(1);           // wraps to Int32.MIN (explicit)
let b = a.saturating_add(1);         // stays at Int32.MAX (explicit)
```

Overflow is not a runtime surprise — it's a compile-time decision.

## 11.7 Property Testing Connection

Refinement types make property testing a library, not a language feature. `Int where x > 0` tells the property testing framework exactly what values to generate. No `forall` keyword needed.

```mvl
// The test framework reads the refinement and generates valid inputs
#[property]
fn divide_nonzero(a: Int, b: Int where b != 0) -> Bool {
    divide(a, b) * b <= a     // property holds for all valid inputs
}
```
