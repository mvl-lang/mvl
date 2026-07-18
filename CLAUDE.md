# MVL Language — Claude Code Instructions

## Verify After Writing

**After writing or modifying any `.mvl` file, run:**

```bash
cargo run -- check <file.mvl>
```

If it fails, fix the errors before moving on. The compiler is the oracle — don't guess syntax, verify it.

For full test suite: `make test-corpus` (parse+check all corpus files) or `make test-rust-rust` (run corpus through Rust backend).

---

## MVL Syntax Cheat Sheet

MVL looks like Rust but isn't. These are the mistakes Claude makes most often.

### Statements end with semicolons, last expression does NOT

```mvl
fn example() -> Int {
    let x: Int = 42;       // semicolon — it's a statement
    let y: Int = x + 1;    // semicolon
    y                       // NO semicolon — this is the return expression
}
```

A trailing semicolon on the last line makes it return `Unit`, not the value.

### All `let` bindings require explicit types

```mvl
// CORRECT
let x: Int = 42;
let name: String = "hello";
let items: List[Int] = [1, 2, 3];

// WRONG — no type inference on let
let x = 42;
let name = "hello";
```

### Mutable bindings use `ref`

```mvl
let x: Int = 42;           // immutable (default)
let count: ref Int = 0;    // mutable — can be reassigned
count = count + 1;
```

There is no `mut` keyword. Use `ref`.

### Generics use `[T]` not `<T>`

```mvl
// CORRECT
fn first[T](items: List[T]) -> Option[T] { ... }
let xs: List[Int] = [1, 2, 3];
let m: Map[String, Int] = Map::new();

// WRONG — angle brackets
fn first<T>(items: List<T>) -> Option<T> { ... }
let xs: List<Int> = [1, 2, 3];
```

### Empty collections — no `{}` for maps

```mvl
// CORRECT
let empty_list: List[Int] = [];
let empty_map: Map[String, Int] = Map::new();         // let annotation drives inference
let m = Map[String, Int]::new();                      // explicit type params — no annotation needed
process(Map[String, Int]::new())                      // inline use, no let required

// WRONG — {} is an empty block, not an empty map
let empty_map: Map[String, Int] = {};
```

Map literals use `{"key": value}` syntax only when non-empty:
```mvl
let m: Map[String, Int] = {"a": 1, "b": 2};
```

### Effects are declared with `!` after the return type

```mvl
// Pure function (default)
fn add(a: Int, b: Int) -> Int { a + b }

// Function with effects
fn greet(name: String) -> Unit ! Console {
    println("Hello, " + name)
}

// Multiple effects
fn process() -> Unit ! Console + FileRead + Net { ... }
```

Effects are NOT generic parameters. They go after the return type.

### Match uses `=>` and trailing commas

```mvl
match opt {
    Some(v) => v,       // comma after each arm
    None => 0,          // including the last one
}
```

No `switch`, no `case`, no `:`. Always `pattern => expr,`.

### IFC labels — `Tainted[T]`, `Secret[T]`, `relabel`

```mvl
// Labels are opaque wrappers
fn handle(input: Tainted[String]) -> String {
    relabel trust(input, "XSS-001")     // explicit unwrap with audit tag
}

fn protect(data: String) -> Secret[String] {
    relabel classify(data, "PII-001")   // explicit wrap with audit tag
}
```

You cannot pass `Tainted[String]` where `String` is expected — compile error.

### Refinement types use `where`

```mvl
type PositiveInt = Int where self > 0
type NonEmpty = String where len(self) > 0

type Person = struct {
    name: String where len(self) > 0,
    age: Int where self >= 0,
}

type Range = struct {
    lo: Int,
    hi: Int,
} with invariant self.lo <= self.hi
```

**`where` in MVL means one thing only: a solver-discharged predicate.**
The trailing `where T: Trait` clause on fn signatures is **NOT MVL syntax**
(ADR-0053) — MVL has no trait system.  If you find yourself writing
`fn foo[T]() where T: Clone` you are leaking Rust vocabulary into MVL
source; the parser will reject it.  Specialize on a concrete type instead:

```mvl
// WRONG — parse error, ADR-0053
fn compare[T](a: T, b: T) -> Bool where T: Eq { a == b }

// CORRECT — specialize
fn compare_ints(a: Int, b: Int) -> Bool { a == b }
```

### Contracts: `requires` / `ensures`

```mvl
fn safe_divide(a: Float, b: Float) -> Float
    requires b != 0.0
    ensures result >= 0.0
{
    a / b
}
```

### Effects are defined in `std/effects.mvl`, not hardcoded

```mvl
effect Clock
effect Console
effect FileRead
effect Net
effect Log > Clock                  // subsumption
effect IO > Console + FileRead + Net  // composite
```

User-defined effects are supported.

### Actors

```mvl
actor Counter {
    count: Int

    pub fn increment(val n: Int) { }   // async behavior, sendable params
    pub fn reset() { }
    fn get_count() -> Int { 0 }         // private sync helper
}
```

`pub fn` on actors = async behaviors. Parameters must be sendable (`val`, `iso`, or value types).

### Termination

```mvl
total fn factorial(n: Int) -> Int {
    if n <= 1 { 1 } else { n * factorial(n - 1) }
}

fn count_down(n: Int) -> Unit {
    let i: ref Int = n;
    while i > 0 decreases i {
        i = i - 1;
    }
}
```

`total fn` = compiler proves termination. `decreases` = loop variant.

### Extension methods

```mvl
pub fn String::len(self) -> Int { ... }
pub fn List[T]::is_empty(self) -> Bool { self.len() == 0 }
pub fn Map[K, V]::get(self, key: K) -> Option[V] { ... }
```

Methods are called with dot syntax: `"hello".len()`, `xs.is_empty()`.

### `use` imports

```mvl
use std.log.{log_info, log_warn}
use std.env.{get, get_secret}
```

Dot-separated module paths, not `::`.

### No bare `unwrap()`

```mvl
// CORRECT
let val: Int = opt.unwrap_or(0);        // provide default
match opt { Some(v) => v, None => 0 }   // explicit match
if let Some(v) = opt { ... }            // if-let binding

// WRONG — unwrap() does not exist
let val: Int = opt.unwrap();
```

### Comments

```mvl
// Line comment
/// Doc comment (on pub items)
//! Module-level doc comment (first lines of file)
```

No `/* */` block comments.

---

## Test files must import, not redeclare

**Never declare a `type`, `fn`, `total fn`, or `partial fn` inside a
`*_test.mvl` file.** Test code exists to exercise production code — every
symbol a test refers to must be reachable via `use module::Item;` from a
sibling production `.mvl` file or the standard library.

### Why

Redeclarations in test files silently shadow production. Every drift bug
this project has caught started as a shadow that made the tests keep
passing while production changed underneath. Concrete cases (all on
branch `chore/exterminate-96-workaround`):

- `flight_clearance` — 19 test sites bound to a ghost `MaintenanceStatus::Cleared` variant while production had `Airworthy`
- `log_analyzer` / `task_pipeline` — dead `RunError::MissingArg` tests exercised a variant no longer in production
- `access_control` — effect-stripped `log_access` shim with `assert_eq(1, 1)` "tests" that verified nothing about production
- `csv_transactions` — phantom `Transaction` / `encode_transaction` / `decode_transaction` that never existed in production

### If a test wants to import something that isn't importable

- **Item lives in `main.mvl`?** `main.mvl` is the entry point, not a
  module. Move the item into a sibling module (e.g., `errors.mvl`,
  `security.mvl`, `paths.mvl`). `main.mvl` re-imports it; the test
  imports it directly.
- **Item is not `pub`?** Add `pub` to production. If the item genuinely
  must be private, don't test it in isolation — test through the public
  surface, or via integration (`make run`).
- **Item carries an effect the test can't declare?** Do NOT write an
  effect-stripped shim. Either extract the pure logic into a separate
  `pub` helper and test that, or accept that the function is
  integration-only.

### Rule summary

- Zero `type` declarations in `*_test.mvl` files.
- Zero standalone `fn`/`total fn`/`partial fn` declarations in `*_test.mvl` files. (Test-local closures and lambdas are fine; test-fn helpers that are only used by other test fns in the same file are fine, but prefer moving them to production or accepting the duplication as tacit debt to be repaid.)
- If a fossil-scan grep for the above returns any hits, the sweep is not done.

See `.openspec/patterns/006-no-test-shadows.md` for the full pattern
catalogue with before/after examples and historical drift cases.

## Project Layout

```
src/mvl/          — compiler source (Rust)
std/              — stdlib declarations (.mvl)
tests/corpus/     — test programs by category (00_smoke..13_stdlib), test fn blocks
tests/stdlib/     — stdlib integration tests, test fn blocks
tests/bdd/        — Gherkin BDD scenarios (rust/rust only)
mvl_runtime/      — Rust runtime backing stdlib builtins
docs/             — mkdocs site, language manual (grammar EBNF lives in mvl-lang/mvl-spec)
.openspec/        — specs and ADRs
```

## Build & Test

```bash
cargo build                    # build compiler
cargo run -- check file.mvl    # type-check a file
cargo run -- build file.mvl    # compile via Rust backend
make test-corpus               # parse+check all corpus files
make test-rust-rust            # run corpus through Rust backend (matrix anchor)
make test-rust-llvm            # run corpus through LLVM backend
make test-stdlib               # run stdlib tests (Rust backend)
make test                      # all tests
```

## Loop Style: `while true` over recursive tail-calls

**Prefer `while true` + `return` over recursive tail-call loops.**

MVL supports `return` for early exit from `while true` loops. Use this instead of
recursive function calls for server loops, accept loops, and receive loops.

```mvl
// PREFERRED — while true + return
partial fn accept_loop(listener: TcpListener) -> Result[Unit, ZmqError] ! Net {
    while true {
        match tcp_accept(listener) {
            Err(e) => {
                if !is_transient_accept_error(e) {
                    tcp_close_listener(listener);
                    return Ok(())
                }
            },
            Ok(stream) => {
                let _: Result[Unit, ZmqError] = handle(stream);
            },
        }
    }
}

// AVOID — recursive tail-call
partial fn accept_loop(listener: TcpListener) -> Result[Unit, ZmqError] ! Net {
    match tcp_accept(listener) {
        Err(e) => {
            if is_transient_accept_error(e) {
                accept_loop(listener)       // recursive call = implicit loop
            } else {
                tcp_close_listener(listener);
                Ok(())
            }
        },
        Ok(stream) => {
            let _: Result[Unit, ZmqError] = handle(stream);
            accept_loop(listener)           // recursive call = implicit loop
        },
    }
}
```

Note: the linter does NOT currently detect recursive tail-calls that could be `while true`.
This is a manual style preference.

## LLVM Backend: C-ABI Naming Convention

When emitting LLVM IR for C-ABI calls (e.g., runtime builtins), use the **unprefixed** form in IR:

```llvm
// CORRECT — LLVM IR C-ABI function calls
call void @mvl_yield_check()
call void @mvl_actor_spawn(...)
call void @mvl_string_drop(ptr %s)
call ptr @mvl_array_slice(...)
declare void @mvl_yield_check()
```

The C compiler (Clang/GCC) automatically adds platform-specific prefixes when generating symbols:
- **macOS/Darwin**: `_mvl_yield_check` (one underscore prefix)
- **Linux**: `mvl_yield_check` (no prefix)

Never hardcode the underscore in LLVM IR — the platform convention handles it transparently.
This applies to all C-ABI runtime functions in `runtime/llvm/` and `runtime/rust/`.

---

## Key Design Principles

1. **Explicit over implicit** — no hidden behavior, no implicit conversions
2. **One way to do it** — one syntax for each concept
3. **The signature IS the threat model** — effects, labels, ownership all in the signature
4. **No UFCS** (ADR-0031) — `x.method()` only for declared methods, not free functions
5. **No bare unwrap** — always handle the None/Err case
