# MVL Language Cheatsheet

*One-page reference for keywords, operators, types, and actors.*

---

## Keywords (Clustered)

### Declarations
| Keyword | Usage | Example |
|---------|-------|---------|
| `fn` | Function | `fn add(a: Int, b: Int) -> Int { a + b }` |
| `struct` | Product type | `struct Point { x: Float64, y: Float64 }` |
| `enum` | Sum type | `enum Option<T> { Some(T), None }` |
| `type` | Alias | `type Meters = Float64` |
| `const` | Constant | `const MAX: Int = 100` |
| `actor` | Actor type | `actor Counter { count: Int; pub fn inc() { } }` |

> **Note:** `trait` and `impl` are **NOT SUPPORTED** in MVL.
> MVL uses composition over inheritance. Use standalone functions with generic bounds
> (`where T: cloneable`) instead of user-defined traits. See #753 for design rationale.

### Modifiers
| Keyword | Usage | Example |
|---------|-------|---------|
| `pub` | Public visibility | `pub fn api() -> Unit { }` |
| `total` | Must terminate | `total fn fib(n: Int) -> Int { ... }` |
| `partial` | May not terminate | `partial fn serve() -> Unit ! Net { }` |
| `test` | Test function | `test fn test_add() { assert_eq(1+1, 2) }` |
| `extern` | FFI boundary | `extern "rust" { fn low_level() -> T }` |

### Control Flow
| Keyword | Usage | Example |
|---------|-------|---------|
| `if` / `else` | Conditional | `if x > 0 { x } else { -x }` |
| `match` | Pattern match | `match opt { Some(v) => v, None => 0 }` |
| `for` | Iteration | `for item in items { process(item) }` |
| `while` | Loop (partial only) | `while cond { body }` |
| `return` | Early return | `return Err(e)` |
| `break` / `continue` | Loop control | `break` |

### Bindings
| Keyword | Usage | Example |
|---------|-------|---------|
| `let` | Variable binding | `let x: Int = 42` |
| `use` | Import | `use std.io.{File, read}` |

### Capabilities (on types)
| Keyword | Meaning | Sendable? |
|---------|---------|-----------|
| `val` | Immutable, shareable | Yes |
| `ref` | Mutable, local only | No |
| `iso` | Isolated, transferable | Yes (consumed) |
| `tag` | Identity only | Yes |

### Effects
| Keyword | Usage |
|---------|-------|
| `!` | Effect annotation | `fn read() -> String ! FileRead` |

---

## Operators

### Arithmetic
| Op | Meaning | Example |
|----|---------|---------|
| `+` `-` `*` `/` | Basic math | `a + b` |
| `%` | Modulo | `a % b` |
| `-` (unary) | Negation | `-x` |

### Comparison
| Op | Meaning | Example |
|----|---------|---------|
| `==` `!=` | Equality | `a == b` |
| `<` `<=` `>` `>=` | Ordering | `a < b` |

### Logical
| Op | Meaning | Example |
|----|---------|---------|
| `&&` | And | `a && b` |
| `||` | Or | `a || b` |
| `!` (unary) | Not | `!flag` |

### Bitwise
| Op | Meaning | Example |
|----|---------|---------|
| `&` `|` `^` | And, Or, Xor | `a & b` |
| `<<` `>>` | Shift | `a << 2` |
| `~` | Complement | `~a` |

### Other
| Op | Meaning | Example |
|----|---------|---------|
| `?` | Try/propagate error | `file.read()?` |
| `..` | Range | `0..10` |
| `::` | Path separator | `std::io::File` |
| `.` | Field/method access | `point.x` |
| `|>` | Pipe | `x |> f |> g` |

---

## Types

### Primitives
| Type | Size | Description |
|------|------|-------------|
| `Bool` | 1 bit | `true` / `false` |
| `Int` | 64 bit | Default signed integer |
| `Int8`..`Int64` | 8-64 bit | Sized signed integers |
| `UInt8`..`UInt64` | 8-64 bit | Sized unsigned integers |
| `Float32` | 32 bit | Single precision |
| `Float64` | 64 bit | Double precision |
| `Char` | 32 bit | Unicode scalar |
| `Byte` | 8 bit | Raw byte |
| `Unit` | 0 bit | No value (like `void`) |

### Built-in Generics
| Type | Description | Example |
|------|-------------|---------|
| `String` | UTF-8 text | `"hello"` |
| `Option<T>` | Maybe value | `Some(42)`, `None` |
| `Result<T, E>` | Success or error | `Ok(v)`, `Err(e)` |
| `Array<T>` | Fixed-size array | `[1, 2, 3]` |
| `List<T>` | Dynamic list | `List::new()` |
| `Map<K, V>` | Hash map | `Map::new()` |
| `Set<T>` | Hash set | `Set::new()` |
| `(T, U, ...)` | Tuple | `(1, "a", true)` |

### Generics Syntax

MVL uses different brackets for declaration vs instantiation:

| Context | Syntax | Example |
|---------|--------|---------|
| Declaration | `<T>` | `fn identity<T>(x: T) -> T` |
| Call site | `[T]` | `identity[Int](42)` |

This avoids parsing ambiguity with comparison operators (`<`, `>`).

### Generic Bounds (Built-in Only)

MVL provides built-in bounds for common operations. These are **not user-extensible**:

| Bound | Meaning | Example |
|-------|---------|---------|
| `cloneable` | Can be deep-copied | `fn dup<T: cloneable>(x: T) -> T` |
| `comparable` | Supports `==`, `<`, etc. | `fn max<T: comparable>(a: T, b: T) -> T` |
| `hashable` | Can be used as map key | `fn index<K: hashable, V>(m: Map<K,V>, k: K)` |

Call site with bounds: `max[Int](a, b)` — bounds checked at compile time.

> **No user-defined traits.** Unlike Rust, you cannot define your own traits.
> Use standalone functions and composition instead of inheritance hierarchies.

### Refinement Types
```mvl
type NonZero = Int where self != 0
type Positive = Int where self > 0
type Bounded = Int where self >= 0 && self <= 100

fn divide(a: Int, b: NonZero) -> Int { a / b }
```

### Security Labels (IFC)
| Type | Flows to | Use case |
|------|----------|----------|
| `Tainted<T>` | Must sanitize | External input |
| `Secret<T>` | Cannot output | Passwords, keys |
| `Public<T>` | Anywhere | Safe data |
| `Clean<T>` | DB, output | Sanitized input |

---

## Effects

| Effect | Description |
|--------|-------------|
| `Console` | stdout/stderr |
| `FileRead` | Read files |
| `FileWrite` | Write files |
| `Net` | Network I/O |
| `DB` | Database access |
| `Random` | Non-determinism |
| `Time` | Clock access |
| `Log` | Logging |
| `Env` | Environment vars |
| `ProcessSpawn` | OS process creation |

```mvl
fn read_config(path: String) -> Config ! FileRead {
    let content = File::read(path)?;
    parse(content)
}
```

---

## Reference Capabilities

### The Four Capabilities

| Capability | Read | Write | Alias | Sendable | Use case |
|------------|------|-------|-------|----------|----------|
| `val T` | Yes | No | Yes | Yes | Immutable shared data |
| `ref T` | Yes | Yes | No | No | Local mutable access |
| `iso T` | Yes | Yes | No | Yes (consumed) | Transfer to actor |
| `tag T` | No | No | Yes | Yes | Identity/opaque handle |

### Capability Lattice (Deny Capabilities)

Capabilities describe what you **deny others**, not what you gain:

```
          iso        <- deny global read AND write (isolated)
         /   \
       val   ref     <- deny write (val) or deny alias (ref)
         \   /
          tag        <- deny read/write (identity only)
```

### Usage in Signatures

```mvl
fn process(
    val config: Config,    // Read-only, can be shared
    ref buffer: Buffer,    // Mutable, cannot escape
    iso payload: Data,     // Ownership transferred in
    tag handle: ActorRef   // Identity only
) -> Result<Output, Error> { }
```

### Ownership Transfer

```mvl
let iso data: Data = create_data()

// Transfer ownership - original binding consumed
send_to_actor(consume(data))

// data is no longer accessible here - compile error if used
```

### Why Not Rust Lifetimes?

| Aspect | Rust | MVL |
|--------|------|-----|
| Syntax | `&'a T`, `&'a mut T` | `val T`, `ref T` |
| Scope tracking | NLL dataflow | Simple depth comparison |
| Annotations | Sometimes required (`'a`) | Never required |
| Concurrency | `Send`/`Sync` traits | `iso`/`val` capabilities |
| LLM-friendliness | Hard (lifetime puzzles) | Easy (pick capability) |

---

## Actor Semantics

### Declaration
```mvl
actor Counter {
    // Private state
    count: Int

    // Private helper (sync, internal)
    fn validate(x: Int) -> Bool {
        x >= 0
    }

    // Behavior (async message handler)
    pub fn increment(iso delta: Int) {
        if self.validate(delta) {
            self.count = self.count + delta
        }
    }

    pub fn reset() {
        self.count = 0
    }

    pub fn get(tag reply: ActorRef) {
        reply.receive(self.count)
    }
}
```

### Creation
```mvl
// Returns ActorRef with tag capability
let tag counter: ActorRef = actor Counter { count: 0 }
```

### Message Send
```mvl
// val/tag args - pass directly
counter.reset()

// iso args - must consume (ownership transfer)
counter.increment(consume(delta))
```

### Sendability Rules
| Capability | Sendable? | At call site |
|------------|-----------|--------------|
| `iso` | Yes | `consume(x)` required |
| `val` | Yes | Pass directly |
| `tag` | Yes | Pass directly |
| `ref` | **No** | Compile error |

### Structured Concurrency
```mvl
concurrently {
    let tag a = actor Worker { }
    let tag b = actor Worker { }
    a.process(data1)
    b.process(data2)
}  // Waits for all messages to drain
```

### Select with Timeout
```mvl
select {
    result = worker.get_result() => { handle(result) }
    timeout(Duration::ms(100)) => { handle_timeout() }
}
```

### Actor Isolation Rules
1. **No shared mutable state** - actor fields are private
2. **No field access via ActorRef** - only message send
3. **Behaviors return Unit** - no synchronous return
4. **FIFO ordering** - messages to same actor processed in order

---

## Quick Reference

### Function Signature
```mvl
pub total fn process(
    val config: Config,      // immutable, shareable
    ref buffer: Buffer,      // mutable, local only
    iso data: Payload        // transferable ownership
) -> Result<Output, Error> ! FileRead + Net {
    // ...
}
```

### Pattern Matching
```mvl
match value {
    Some(x) if x > 0 => handle_positive(x),
    Some(x) => handle_other(x),
    None => handle_none(),
}
```

### Error Handling
```mvl
fn fallible() -> Result<Int, Error> {
    let x = may_fail()?;   // Propagate error
    Ok(x + 1)
}
```

---

*See `docs/language.md` for full language reference.*
