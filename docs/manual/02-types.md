# 2. Types

The type system is the foundation of MVL. It covers Requirements 1 (type safety), 3 (totality), 4 (null elimination), 5 (error visibility), 6 (ownership), 9 (data race freedom), 10 (refinement types), 11 (information flow control), and the Iterator protocol (lazy, sequential element access).

**Principle:** Types are proofs. A program that compiles has proven structural properties about itself.

## 2.1 Primitive Types

| Type | Size | Description |
|------|------|-------------|
| `Bool` | 1 byte | `true` or `false` |
| `Int` | arbitrary | Arbitrary precision integer (default) |
| `Int8`..`Int64` | 1-8 bytes | Fixed-width signed integers |
| `UInt8`..`UInt64` | 1-8 bytes | Fixed-width unsigned integers |
| `Float32` | 4 bytes | IEEE 754 single precision |
| `Float64` | 8 bytes | IEEE 754 double precision |
| `Byte` | 1 byte | Raw byte (alias for `UInt8`) |
| `Char` | 4 bytes | Unicode scalar value |
| `String` | variable | UTF-8, immutable |

`Int` is the default integer type. Arithmetic on `Int` never overflows. Fixed-width integers (`Int32`, `UInt64`, etc.) use checked arithmetic by default — overflow is a compile error unless explicitly handled with `checked_add`, `wrapping_add`, or `saturating_add`.

## 2.2 Algebraic Data Types

### Product types (structs)

```mvl
type Point = struct {
    x: Float64,
    y: Float64,
}
```

### Sum types (enums)

```mvl
type Shape = enum {
    Circle(Float64),
    Rect(Float64, Float64),
    Triangle(Float64, Float64, Float64),
}
```

Every `match` on a sum type MUST be exhaustive — the compiler rejects incomplete matches ([Req 3](../requirements.md#req-3)). Adding a variant forces all match expressions to be updated.

### Type aliases

```mvl
type UserId = Int64
type Coordinates = (Float64, Float64)
```

## 2.3 Built-in Parameterized Types

### Option\<T\> — absence ([Req 4](../requirements.md#req-4))

```mvl
type Option[T] = enum {
    Some(T),
    None,
}
```

Replaces `null`. Accessing the inner value requires pattern matching or `?` propagation. There is no `.unwrap()` — use `match` or combinators (`.map()`, `.unwrap_or()`, `.and_then()`).

### Result\<T, E\> — fallibility ([Req 5](../requirements.md#req-5))

```mvl
type Result[T, E] = enum {
    Ok(T),
    Err(E),
}
```

Functions that can fail MUST return `Result`. Error types are visible in the signature. The `?` operator propagates errors to the caller.

### Collections

```mvl
Array[T]            // ordered, indexed, growable
Map[K, V]           // key-value, ordered by insertion
Set[T]              // unique elements
(T, U)              // tuple (fixed size, heterogeneous)
```

`Array.get(index)` returns `Option[T]` — never panics, never returns null.

## 2.4 Generics

```mvl
fn first[T](items: Array[T]) -> Option[T] {
    items.get(0)
}
```

Type parameters use square brackets in all positions. Constraints via `where`:

```mvl
fn sort[T](items: Array[T]) -> Array[T]
    where T: Ord
{
    // ...
}
```

## 2.5 Traits

```mvl
type Display = trait {
    fn to_string(self) -> String
}

type Error = trait {
    fn message(self) -> String
    fn source(self) -> Option[&Error]
}
```

No inheritance. Composition through traits. A type can implement multiple traits.

```mvl
impl Display for Point {
    fn to_string(self) -> String {
        format("{}, {}", self.x, self.y)
    }
}
```

## 2.6 Iterator Trait

The `Iterator[T]` trait is the protocol for lazy, sequential element access:

```mvl
type Iterator[T] = trait {
    fn next(mut self) -> Option[T]
}
```

`next` advances the iterator and returns the next element, or `None` when exhausted. `Array[T]`, `Range`, `Map[K,V]`, and `Set[T]` all implement `Iterator` out of the box.

Custom types implement it with `impl`:

```mvl
type Counter = struct { mut current: Int, limit: Int }

impl Iterator[Int] for Counter {
    fn next(mut self) -> Option[Int] {
        if self.current >= self.limit {
            None
        } else {
            let value = self.current;
            self.current = self.current + 1;
            Some(value)
        }
    }
}
```

Any type implementing `Iterator[T]` can be used in a `for...in` loop. See [§4.5 For Loop](04-statements.md#45-for-loop).

## 2.7 Refinement Types ([Req 10](../requirements.md#req-10))

Refinement types constrain values beyond their base type:

```mvl
fn divide(a: Int, b: Int where b != 0) -> Int {
    a / b
}

fn create_port(n: UInt16 where n >= 1 && n <= 65535) -> Port {
    Port { number: n }
}

type NonEmpty[T] = Array[T] where len(self) > 0
```

The compiler verifies refinement predicates at compile time using SMT solving. If the predicate cannot be statically verified, the compiler requires a runtime check at the call site.

## 2.8 Security Labels ([Req 11](../requirements.md#req-11) — global IFC requirement)

Every type can carry a security label:

```mvl
Public[T]           // safe for any output
Clean[T]            // sanitized external data
Tainted[T]          // from external sources (user input, network, files)
Secret[T]           // cryptographic material, passwords, keys
```

Data flows up the lattice freely (`Public` → `Secret`). Flowing down requires explicit action:

- `Tainted` → `Clean`: `sanitize(value)` (validates and cleans)
- `Secret` → `Public`: `declassify(value)` (auditable, greppable)

See [Chapter 10: Information Flow Control](10-ifc.md).

## 2.9 Reference Types ([Req 2](../requirements.md#req-2), 6)

```mvl
&T                  // shared (immutable) borrow
&mut T              // exclusive (mutable) borrow
```

Ownership rules:
- Values have exactly one owner
- Ownership transfers via `move`
- Shared borrows (`&T`) allow multiple readers
- Exclusive borrows (`&mut T`) allow one writer and no readers
- Borrows cannot outlive the owner

## 2.10 Reference Capabilities ([Req 9](../requirements.md#req-9))

For concurrency safety, values carry capabilities:

```mvl
iso T               // isolated — only one reference exists (sendable)
val T               // deeply immutable (sharable)
ref T               // local mutable reference (not sendable)
tag T               // opaque identity — can compare but not read
```

See [Chapter 12: Concurrency](12-concurrency.md).

## 2.11 Function Types

```mvl
fn(Int, Int) -> Int                 // pure function
fn(String) -> Result[Int, ParseError] ! FileRead  // effectful function
```

Function types include their effects. A `fn(A) -> B` is pure; a `fn(A) -> B ! E` has effect `E`.
