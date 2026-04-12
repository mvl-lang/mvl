# 3. Declarations

## 3.1 Function Declarations

```mvl
fn name(params) -> ReturnType { body }
```

### Pure functions

```mvl
fn add(a: Int, b: Int) -> Int {
    a + b
}
```

Pure functions have no effects. The compiler rejects any side-effecting operation in a pure function.

### Effectful functions

```mvl
fn read_config(path: Path) -> Result<Config, IOError> ! FileRead {
    let content = read_to_string(path)?;
    parse_config(content)
}
```

Effects are declared after `!`. Multiple effects: `! FileRead, Console, Net`.

### Total functions ([Req 8](../requirements.md#req-8))

```mvl
total fn factorial(n: UInt where n <= 20) -> UInt {
    match n {
        0 => 1,
        n => n * factorial(n - 1),   // structural recursion — compiler verifies termination
    }
}
```

`total` functions MUST provably terminate. The compiler checks structural recursion (argument decreases on recursive call). If termination cannot be proven, the compiler rejects.

### Partial functions

```mvl
partial fn server_loop() -> Never ! Net, Console {
    while true {
        let conn = accept()?;
        handle(conn)?;
    }
}
```

`partial` functions may not terminate. `while` loops are only permitted in `partial` functions. Functions are `total` by default — `partial` is the opt-in escape hatch.

### Program entry point

A file with `fn main() -> ()` is a **binary** — it compiles to an executable. A file without `fn main` is a **library** — it compiles to a reusable module.

```mvl
// Binary: this file produces an executable
fn main() -> () ! Console {
    println("hello");
}
```

There is no `#[binary]` attribute, no `package main`, no `__name__` guard. The presence of `fn main` is the only signal. The compiler infers binary vs library automatically.

`fn main` is typically `partial` (servers loop forever) or effectful (most programs do I/O). A pure, total `fn main` is valid but unusual.

### Methods

Methods are functions inside `impl` blocks:

```mvl
impl Point {
    fn distance(self, other: &Point) -> Float64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}
```

## 3.2 Type Declarations

See [Chapter 2: Types](02-types.md).

```mvl
type Name = struct { fields }       // product type
type Name = enum { variants }       // sum type
type Name = ExistingType             // alias
type Name = trait { methods }        // interface
```

## 3.3 Constant Declarations

```mvl
const MAX_CONNECTIONS: UInt32 = 1024;
const PI: Float64 = 3.14159265358979;
```

Constants are compile-time evaluated. The right-hand side MUST be a constant expression.

## 3.4 Module Declarations

```mvl
module http {
    type Request = struct { ... }
    type Response = struct { ... }

    fn get(url: Clean<Url>) -> Result<Tainted<Response>, NetError> ! Net {
        // ...
    }
}
```

See [Chapter 13: Module System](13-modules.md).

## 3.5 Extern Declarations (FFI)

```mvl
extern "rust" {
    fn crypto_random_bytes(n: UInt) -> Array<Byte>;
}
```

Extern functions are trust boundaries — the MVL compiler does not verify their implementation. They are greppable (`extern`), trackable in assurance reports, and counted separately in coverage metrics.

See [Chapter 14: Foreign Function Interface](14-ffi.md).

## 3.6 Impl Blocks

```mvl
impl Display for HttpError {
    fn to_string(self) -> String {
        format("HTTP {}: {}", self.status, self.message)
    }
}
```

A type can have multiple `impl` blocks. Trait implementations must satisfy all trait methods.
