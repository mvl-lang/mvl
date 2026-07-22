<!-- Canonical AGENTS.md template for MVL projects.
     Lives in mvl-lang/mvl/etc/. Copy this file into the root of any MVL
     project (eventually written by `mvl new`). Keep it tool-agnostic:
     every coding agent reads AGENTS.md first. -->

# AGENTS.md — Working in an MVL Project

## The one rule: verify after writing

**After writing or modifying any `.mvl` file, run:**

```bash
mvl check <file.mvl>
```

If it fails, fix the errors before moving on. **The compiler is the oracle —
don't guess syntax, verify it.** MVL looks like Rust but isn't; your instincts
will produce plausible-looking wrong code, and only the checker catches it.

## Commands

```bash
mvl check <file.mvl>   # parse + type + effect + refinement check (fast — run constantly)
mvl test [path]        # run `test fn` blocks
mvl build <file.mvl>   # compile to a native binary
mvl lint [path]        # style checks
mvl fmt [path]         # formatter
```

A package's `mvl.toml` pins the toolchain: `requires-mvl = ">=1.7.0"`.
Everyone — humans, agents, CI — compiles with the same oracle.

## The 11 Compiler-Verified Requirements

MVL verifies 11 properties at compile time. Every annotation you write maps to one of these.
**Removing an annotation doesn't simplify code — it removes a proof.** The compiler rejects
the program instead.

Requirements 1–7 come from formal methods and safety-critical practice. Requirements 8–11
were known but historically impractical; LLM-generated code makes the annotation burden
zero, so the compiler enforces them too. (ADR-0001)

| # | Requirement | What you write | Compiler rejects |
|---|---|---|---|
| 1 | **Type safety** | `type T = struct { … }` / `enum { … }` | Field access on wrong type; missing struct fields |
| 2 | **Memory safety** | `move expr`, `consume expr` | Use after move; bare reassign of linear value |
| 3 | **Totality** | All `match` arms present | Non-exhaustive `match` |
| 4 | **Null elimination** | `Option[T]`; `.unwrap_or()` or `match` | `.unwrap()`; direct access without matching |
| 5 | **Error visibility** | `Result[T,E]`; `?` or `match` | Unused `Result` |
| 6 | **Ownership** | `let x: T` (immutable), `let x: ref T` (mutable) | Mutating immutable binding; use-after-move |
| 7 | **Effect tracking** | `-> ReturnType ! Effect` on every effectful fn | Undeclared effect; calling effectful fn from pure fn |
| 8 | **Termination** | `total fn` + `decreases expr` on loops; `partial fn` if non-terminating | `while` without `decreases` in `total fn`; unproven recursion |
| 9 | **Data race freedom** | `val` / `ref` / `iso` / `tag` on params; actor `pub fn` params must be sendable | Sending `ref` across actor boundary |
| 10 | **Refinement types** | `type T = Base where predicate`; `requires`/`ensures` | Value violating predicate at call site |
| 11 | **IFC** | `Tainted[T]`, `Secret[T]`, `relabel` with audit tag | `Tainted` used as clean type; `Secret` leaked without `relabel` |

### Req 1 — Type safety: all types are named ADTs

No anonymous records or structural subtyping. Every type is declared explicitly.

```mvl
type User = struct {
    name: String,
    age: Int,
}

type Status = enum {
    Active,
    Suspended(String),   // variant with payload
}
```

The compiler rejects field access on a wrong type and unknown field names.

### Req 2 — Memory safety: `move` and `consume`

No raw pointers, no garbage collector. Ownership is explicit.

```mvl
let payload: iso Payload = Payload { data: "blob" };
let other = consume payload;   // transfers exclusive ownership; 'payload' is now gone
// payload.data                // compile error: value was consumed
```

`consume` transfers an `iso` (isolated, exclusively owned) value — typically when sending
to an actor. `move` transfers any owned value. Writing `let y = x` for a linear type without
`consume` → `LinearTypeBareBind` compile error.

### Req 3 — Totality: exhaustive match

Every `match` must cover every variant. The compiler will not let you forget a case.

```mvl
match status {
    Status::Active => "ok",
    Status::Suspended(reason) => reason,   // must handle the payload variant too
    // missing arm → compile error
}
```

### Req 4 — Null elimination: `Option[T]`

There is no `null`. Absence is always `Option[T]`. There is no `.unwrap()` method.

```mvl
let val: Int = opt.unwrap_or(0);           // provide a default
match opt { Some(v) => v, None => 0, }    // explicit match
if let Some(v) = opt { use(v) }           // if-let binding
```

### Req 5 — Error visibility: `Result[T, E]` must be handled

Ignoring a `Result` is a compile error. Every error must be acknowledged.

```mvl
// WRONG — unused Result
write(log_path);

// CORRECT
write(log_path)?;                                    // propagate to caller
match write(log_path) { Ok(_) => …, Err(e) => … }  // handle inline
let _: Result[Unit, IoError] = write(log_path);     // explicitly discard
```

### Req 6 — Ownership: `ref` for mutable, `iso` for exclusive

Bindings are immutable by default. `ref` makes a binding locally mutable. `iso` marks
exclusive ownership that can cross boundaries (e.g., sent to an actor).

```mvl
let x: Int = 42;            // immutable — cannot be reassigned
let count: ref Int = 0;     // mutable — can be reassigned
count = count + 1;          // OK
// x = 99;                  // compile error: immutable binding
```

There is no `mut` keyword. Use `ref`.

### Req 7 — Effect tracking: `! Effect` on every effectful fn

Every side effect is declared in the function signature. A pure function cannot call an
effectful one. Effects propagate up the call chain until handled or declared.

```mvl
fn greet(name: String) -> Unit ! Console {   // declared — Console I/O required
    println("Hello, " + name)
}

fn pure_add(a: Int, b: Int) -> Int {         // no ! — truly pure
    a + b
    // println("debug")                      // compile error: undeclared Console effect
}
```

### Req 8 — Termination: `total fn` and `decreases`

`total fn` promises the compiler the function terminates. The compiler verifies this:
recursive calls need a structural argument that decreases, loops need a `decreases` variant.
Use `partial fn` to explicitly opt out of the termination guarantee.

```mvl
total fn factorial(n: Int) -> Int {
    if n <= 1 { 1 } else { n * factorial(n - 1) }   // structural recursion on n
}

fn drain(items: List[Int]) -> Unit {
    let i: ref Int = items.len();
    while i > 0 decreases i {     // variant: i strictly decreases each iteration
        i = i - 1;
    }
}
```

### Req 9 — Data race freedom: reference capabilities

Four capabilities govern what a value can do across concurrency boundaries
(adapted from Pony, ADR-0029):

| Capability | Mutable | Sendable | Use |
|---|---|---|---|
| `val T` | No | Yes | Shared immutable — safe to send anywhere |
| `ref T` | Yes | **No** | Local mutable — stays in one thread |
| `iso T` | Yes | Yes | Exclusive — only one reference exists |
| `tag T` | No (opaque) | Yes | Identity only — no read/write access |

Actor `pub fn` parameters must be sendable (`val`, `iso`, or plain value types).

```mvl
actor Worker {
    pub fn process(val data: Payload) { … }     // val — sendable immutable
    pub fn take(iso resource: Handle) { … }     // iso — exclusive ownership transfer
    // pub fn borrow(ref buf: Buffer) { … }     // compile error: ref not sendable
}
```

### Req 10 — Refinement types: `where` and `requires`/`ensures`

Named invariants checked by the compiler (Phase 1: static call-site; Phase 2: SMT solver).

```mvl
type PositiveInt = Int where self > 0
type NonEmpty = String where len(self) > 0

type Range = struct {
    lo: Int,
    hi: Int,
} with invariant self.lo <= self.hi   // struct-level invariant

fn safe_divide(a: Float, b: Float) -> Float
    requires b != 0.0                 // precondition — caller must satisfy
    ensures result * b == a           // postcondition — compiler checks body
{
    a / b
}
```

`where` in MVL means only one thing: a solver-discharged predicate. The `where T: Trait`
clause from Rust is **not valid MVL syntax** — the compiler will reject it.

### Req 11 — IFC: information flow labels and `relabel`

Four labels form a security lattice. Labels don't flow upward without an explicit `relabel`
with an audit tag that appears in the assurance report.

| Label | Meaning |
|---|---|
| `Public[T]` | Explicitly public (default when unannotated; prefer explicit in IFC code) |
| `Tainted[T]` | From external/untrusted source — cannot be used as clean without sanitize |
| `Clean[T]` | Tainted value that has been sanitized |
| `Secret[T]` | Confidential — cannot be logged, returned, or displayed without declassify |

```mvl
fn handle_input(input: Tainted[String]) -> String {
    relabel trust(input, "XSS-001")    // explicit unwrap — audit tag required
}

fn store_key(key: String) -> Secret[String] {
    relabel classify(key, "PII-001")   // explicit wrap — audit tag required
}

// Prefer explicit Public[T] in IFC-focused functions (ADR-0017)
fn declassify_token(secret: Secret[Token]) -> Public[Token] {
    relabel release(secret, "AUDIT-002")
}
```

Passing `Tainted[String]` where `String` is expected → compile error.
`Public[T]` is the implicit default but should be written explicitly in IFC-focused code
(the linter emits a `hint:` for redundant labels, not a warning — explicit is preferred).

---

## MVL Syntax Cheat Sheet

MVL looks like Rust but isn't. These are the mistakes agents make most often.

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

// WRONG — angle brackets
fn first<T>(items: List<T>) -> Option<T> { ... }
```

### Empty collections — no `{}` for maps

```mvl
// CORRECT
let empty_list: List[Int] = [];
let empty_map: Map[String, Int] = Map::new();

// WRONG — {} is an empty block, not an empty map
let empty_map: Map[String, Int] = {};
```

Map literals use `{"key": value}` syntax only when non-empty.

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
    Some(v) => v,       // comma after each arm,
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

type Range = struct {
    lo: Int,
    hi: Int,
} with invariant self.lo <= self.hi
```

**`where` in MVL means one thing only: a solver-discharged predicate.**
The trailing `where T: Trait` clause on fn signatures is **NOT MVL syntax** —
MVL has no trait system. `fn foo[T]() where T: Clone` is a parse error.
Specialize on a concrete type instead.

### Contracts: `requires` / `ensures`

```mvl
fn safe_divide(a: Float, b: Float) -> Float
    requires b != 0.0
    ensures result >= 0.0
{
    a / b
}
```

### Actors

```mvl
actor Counter {
    count: Int

    pub fn increment(val n: Int) { }   // async behavior, sendable params
    fn get_count() -> Int { 0 }        // private sync helper
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
```

Methods are called with dot syntax: `"hello".len()`, `xs.is_empty()`.

### `use` imports

```mvl
use std.log.{log_info, log_warn}
use std.env.{get_secret}
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

## Stdlib Map

The implicit prelude (no `use` needed) loads: **core, strings, lists,
collections, effects, io**. Everything else needs `use std.<module>.{item}`.

| Module | What you get |
|---|---|
| `std.core` | `println`/`print`/`eprintln` (`! Console`), `range`, `Option`/`Result` methods (`unwrap_or`, `is_some`, `and_then`, …) |
| `std.strings` | `String::len` (builtin), `contains`, `starts_with`, `trim`, `replace`, `is_empty` |
| `std.lists` | `List[T]::len` (builtin), `is_empty`, `first`/`last` → `Option[T]`, `take`, `skip`, `reverse`, `flatten` |
| `std.collections` | `Map[K,V]` + `Set[T]` — `get` → `Option[V]`, `insert`, `contains_key`, `keys`, `values`. `Map::new()` requires `use std.collections.{Map}` |
| `std.effects` | `Console`, `FileRead`, `FileWrite`, `Net`, `DB`, `Env`, `Random`, `ProcessSpawn`, `Terminal`, `Spawn`, `Clock`; `Log > Clock`; `IO > Console + FileRead + Net` |
| `std.io` | `Path` + `path()`/`join()`/`to_string()`, `IoError` |
| `std.env` | `env_var` → `Option[Tainted[String]]`, `get_secret` → `Option[Secret[String]]` (`! Env`) |
| `std.log` | `default_logger()`, `Logger::info/warn/debug(msg, fields)` (`! Log`) |
| `std.json` | `Value`, `encode`, `decode` → `Result[_, JsonError]`, jsonl variants |
| `std.ifc` | labels `Tainted`/`Secret`; `relabel classify`/`taint`/`trust`/`release` |
| `std.error` | convention: every error type has `user_message() -> String` (safe) and `debug_message() -> Secret[String]` |
| `std.math` | `int_abs`, `int_pow`, `int_clamp`, `int_min`, `int_max` |
| `std.time` | `Instant`, `DateTime`, `Duration`, `seconds()`, `millis()` |
| `std.regex` | `Regex`, `find_all`, `replace` |
| `std.testing` | `assert_contains`, `assert_len`, `assert_empty`, `assert_some`, `assert_none` |

More modules (`args`, `audit`, `config`, `crypto`, `csv`, `db`, `net`,
`random`, `text`, `toml`, …) live in `std/` — read the source, it's `.mvl`.

## Project Conventions

### Test files import, never redeclare

**Never declare a `type`, `fn`, `total fn`, or `partial fn` inside a
`*_test.mvl` file.** Redeclarations silently shadow production code — the
tests keep passing while production changes underneath. Every symbol a test
uses must come via `use module::Item` from a sibling production file. If the
item isn't importable (lives in `main.mvl`, isn't `pub`), move it into a
module or make it `pub`.

### Loop style: `while true` over recursive tail-calls

Prefer `while true` + `return` for server/accept/receive loops instead of
recursive tail-calls.

### Layout

```
mvl.toml          # package manifest (requires-mvl pins the toolchain)
main.mvl          # entry point (fn main)
<module>.mvl      # production modules
<module>_test.mvl # test fn blocks — import from production, never redeclare
```

## References

- Language grammar (EBNF): [mvl-lang/mvl-spec](https://github.com/mvl-lang/mvl-spec)
- LSP server (compiler-backed diagnostics): `pip install mvl-lsp`
