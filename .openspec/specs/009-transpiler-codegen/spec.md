---
domain: compiler
version: 0.1.0
status: draft
date: 2026-04-15
---

# 009 — Transpiler & Code Generation

The transpiler translates a checked MVL AST into Rust source code. This spec defines the emission rules that preserve MVL's eleven requirements through the translation. The transpiler is not an optimiser — it produces correct, readable Rust at the cost of verbosity.

> **Origin:** Issues #193, #197, #198, #175. The snake game example (PR #190) exposed multiple codegen gaps: missing clone-on-pass for value semantics, broken else-if formatting, and invalid println expansion. These were ad-hoc patches. This spec formalises the rules so fixes are principled.

## Philosophy

MVL is a verification language that transpiles to Rust (Phase 1). The transpiler's job is **semantic preservation**, not optimisation. Every MVL guarantee proven by the checker MUST survive in the emitted Rust. Where MVL and Rust semantics diverge, the transpiler bridges the gap explicitly:

- **Value semantics → Clone:** MVL values are conceptually copied on every use. Rust has move semantics. The transpiler inserts `.clone()` at move points.
- **Totality → exhaustive match:** The checker proves exhaustiveness; the transpiler emits all arms.
- **Effects → doc comments:** Effect annotations are erased at runtime but preserved as doc comments for auditability.
- **IFC labels → newtypes:** Security labels become zero-cost newtype wrappers.

**Design principle:** When in doubt, emit more code. A redundant `.clone()` is correct; a missing one is a compiler bug. Rust's optimiser (LLVM) removes redundant clones. The transpiler does not need to be clever.

**ADR:** [0003 — Compilation Strategy](../adr/0003-compilation-strategy.md)

## Requirements

### Requirement 1: Type Derive Attributes [MUST]

All transpiled struct and enum declarations MUST include `#[derive(Debug, Clone, PartialEq)]`. Refined type aliases over Copy primitives MUST additionally derive `Copy` and `PartialOrd`.

**Implementation:** `src/mvl/backends/rust/emit_types.rs::emit_struct`, `emit_enum`, `emit_alias`

| MVL type | Derives |
|----------|---------|
| `struct` | `Debug, Clone, PartialEq` |
| `enum` | `Debug, Clone, PartialEq` |
| Refined alias (Copy inner) | `Debug, Clone, Copy, PartialEq, PartialOrd` |
| Refined alias (non-Copy inner) | `Debug, Clone, PartialEq, PartialOrd` |
| Security label newtype | `Debug, Clone, PartialEq` |

#### Scenario: Struct derives Clone

- GIVEN `struct Point { x: Int, y: Int }`
- WHEN transpiled
- THEN the emitted Rust contains `#[derive(Debug, Clone, PartialEq)]` immediately before `pub struct Point`

#### Scenario: Enum derives Clone

- GIVEN `enum Direction { Up, Down, Left, Right }`
- WHEN transpiled
- THEN the emitted Rust contains `#[derive(Debug, Clone, PartialEq)]` immediately before `pub enum Direction`

**Tests:** `tests/transpiler.rs::struct_derives_debug`, `tests/transpiler.rs::enum_derives_debug`

### Requirement 2: Value Semantics via Clone-on-Pass [MUST]

MVL has value semantics: passing a value to a function conceptually copies it. The transpiler MUST insert `.clone()` on every value-typed argument at function call sites, EXCEPT:

- Copy types (`Int`, `Float`, `Bool`, `Char`, refined aliases over Copy primitives)
- The last use of a value in its scope (move is sufficient — Phase A)

**Implementation:** `src/mvl/backends/rust/emit_exprs.rs::emit_expr_as_arg`

#### Phase A: Last-use move elision (implemented, issue #234)

The transpiler performs a single-pass last-use analysis over each function body
before emission (`src/mvl/backends/rust/last_use.rs::compute_last_uses`).  Variables
used exactly once, or whose last occurrence is outside any loop, are moved instead
of cloned.  This eliminates unnecessary copies for the common case: a value passed
to one function and never used again.

**Conservative rules:**
- Variables used inside `for`/`while` bodies are always cloned (loop-bound variables
  may be accessed on each iteration; even their outside-loop use is excluded).
- Lambda bodies are not analysed (captures may be called multiple times).
- `FieldAccess` expressions are always cloned (partial struct moves are complex).

**Tests:** `tests/borrow.rs` — 12 targeted Phase A tests.

#### Phase B: Borrow parameter inference (implemented, issue #660)

The transpiler analyses each function's parameters to determine whether they can be
passed by borrow (`&T` or `&mut T`) instead of by value. This is a cross-function
analysis performed by `src/mvl/backends/rust/capability_params.rs::build_capability_params_map`.

**Algorithm:**
1. For each function, examine every parameter.
2. A parameter qualifies for borrow if it has NO disqualifying uses in the function body.
3. Disqualifying uses: returned from function, assigned to a variable, destructured,
   passed to a free function (which may store it), used in a lambda capture, used as
   a field access base in assignment position.
4. Conservative rules: loop variables and lambda captures are always disqualified.
5. Qualified parameters are emitted as `&T` (shared borrow) or `&mut T` (mutable borrow)
   depending on whether the parameter is used in mutable position.

**Implementation:** `src/mvl/backends/rust/capability_params.rs`

**Tests:** `tests/parser/borrow.rs` — 40+ targeted Phase B tests.

#### Scenario: Read-only parameter borrowed (Phase B)

- GIVEN `fn show(p: Point) -> String { format("{}", p.x) }` where `p` is only read
- WHEN transpiled with Phase B
- THEN the emitted Rust signature contains `p: &Point`
- AND call sites pass `&p` instead of `p.clone()`

#### Scenario: Mutated parameter remains owned (Phase B)

- GIVEN `fn consume(s: String) -> String { s }` where `s` is returned
- WHEN transpiled with Phase B
- THEN the emitted Rust signature keeps `s: String` (owned)
- AND the parameter is not borrowed (disqualifying use: returned)

#### Scenario: Single-use variable is moved (Phase A)

- GIVEN `fn f(p: Point) -> Int { show(p) }` where `p` is used exactly once
- WHEN transpiled
- THEN the emitted Rust contains `show(p)` without `.clone()`

#### Scenario: Multi-use variable clones all but last (Phase A)

- GIVEN:
  ```mvl
  fn show(p: Point) -> String { format("{}, {}", p.x, p.y) }
  fn main() -> Unit ! Console {
      let p = Point { x: 1, y: 2 };
      println(show(p));
      println(show(p));
  }
  ```
- WHEN transpiled
- THEN the emitted Rust contains `show(p.clone())` for at least the first call
- AND the emitted Rust compiles without move errors

#### Scenario: Loop variable always cloned (Phase A conservative)

- GIVEN `for _ in range(0, n) { f(b); () }` where `b` is defined outside the loop
- WHEN transpiled
- THEN every call to `f(b)` inside the loop emits `f(b.clone())`
- AND the outside-loop use of `b` also clones if `b` appears anywhere in the loop

#### Scenario: Copy types not cloned (Phase C target)

> **Note:** Phase B (borrow parameter inference) is implemented and eliminates many
> unnecessary clones by passing parameters by reference. However, Copy type inference
> (`Int`, `Bool`, `Char` auto-derive Copy on structs) remains deferred to Phase C.
> Copy types still emit `.clone()` when used in non-last-use positions, but `.clone()`
> on a Copy type is a no-op optimised away by the Rust compiler.
>
> **Tests:** `tests/transpiler.rs::copy_type_ident_clone_is_emitted_but_harmless`

- GIVEN `fn add(a: Int, b: Int) -> Int { a + b }`
- AND `let x = 1; add(x, x);`
- WHEN transpiled (Phase C)
- THEN the emitted Rust contains `add(x, x)` without `.clone()`

#### Scenario: Collection iterable cloned before for-in

- GIVEN `for item in items { ... }` where `items` is a `List[T]`
- WHEN transpiled
- THEN the emitted Rust iterates over `(items).clone()` or equivalent
- AND `items` remains usable after the loop

**Tests:** `tests/compile_and_run.rs::struct_value_semantics` (#193)

**Corpus:** `tests/corpus/11_programs/struct_value_semantics.mvl` (#193)

### Requirement 3: Control Flow Emission [MUST]

#### 3a: else-if Formatting [MUST]

`else if` chains MUST be emitted as `} else if cond {` on a single line. The transpiler MUST NOT delegate `ElseBranch::If` to the top-level `emit_stmt` path, which prepends indentation.

**Implementation:** `src/mvl/backends/rust/emit_stmts.rs::emit_if`

#### Scenario: else-if on one line

- GIVEN `if a { x } else if b { y } else { z }`
- WHEN transpiled
- THEN the emitted Rust contains `} else if b {` with no extra whitespace between `else` and `if`

**Tests:** `tests/transpiler.rs::else_if_formatting` (#197)

#### 3b: match/if as Tail Expression [SHOULD]

When `match` or `if/else` is the last expression in a block, the transpiler SHOULD emit it as a Rust expression (no trailing semicolon) so the block returns the arm values. This requires the parser to recognise tail-position control flow as expressions, not statements.

**Implementation:** `src/mvl/backends/rust/emit_stmts.rs::emit_block` (tail detection)

> **Note:** This is tracked as #189. Until resolved, users must use `let` bindings as a workaround.

#### Scenario: match returns value

- GIVEN:
  ```mvl
  fn describe(d: Direction) -> String {
      match d {
          Direction::Up => "up",
          Direction::Down => "down",
          Direction::Left => "left",
          Direction::Right => "right",
      }
  }
  ```
- WHEN transpiled
- THEN the `match` expression is emitted without a trailing semicolon
- AND the function body compiles as returning `String`

**Tests:** `tests/compile_and_run.rs::match_tail_expression` (#189)

### Requirement 4: Stdlib Function Mapping [MUST]

Built-in functions from `std/core.mvl` MUST be translated to valid Rust macro invocations or function calls.

**Implementation:** `src/mvl/backends/rust/emit_exprs.rs::emit_call`

| MVL function | Rust emission | Notes |
|-------------|---------------|-------|
| `println(s)` | `println!("{}", s)` | Single non-string arg |
| `println(fmt, args...)` | `println!(fmt, args...)` | String literal first arg |
| `eprintln(s)` | `eprintln!("{}", s)` | Same as println |
| `format(fmt, args...)` | `format!(fmt, args...)` | String literal first arg |
| `assert(cond)` | `assert!(cond)` | |
| `assert_eq(a, b)` | `assert_eq!(a, b)` | |
| `panic(msg)` | `panic!("{}", msg)` | |

#### Scenario: println with single non-string argument

- GIVEN `println(x)` where `x: Int`
- WHEN transpiled
- THEN the emitted Rust is `println!("{}", x);`

#### Scenario: println with string format and arguments

- GIVEN `println("x = {}, y = {}", x, y)`
- WHEN transpiled
- THEN the emitted Rust is `println!("x = {}, y = {}", x, y);`

#### Scenario: println with non-string first arg and additional args [MUST NOT]

- GIVEN `println(msg, x)` where `msg` is not a string literal
- WHEN transpiled
- THEN the transpiler MUST NOT emit `println!("{}", msg, x)` (invalid: one placeholder, two args)
- THEN the transpiler MUST either:
  - (a) Emit `println!("{} {}", msg, x)` — one placeholder per arg, OR
  - (b) Reject at the checker level with a clear error

**Tests:** `tests/transpiler.rs::println_non_string_args` (#198)

> **ADR:** [ADR-0041 — Stdlib method dispatch: eliminate emitter special-casing](../adr/0041-stdlib-method-dispatch.md)
> The current dispatch table (above) reflects the transitional state. Phases 1–3 of ADR-0041 will migrate
> category B–E methods out of the emitter. When complete, this requirement will describe only category A
> (kernel builtins: `len`, `push`, `get`, `slice`, `concat`, `contains`) and the `builtin fn` dispatch
> rule; all other stdlib methods will compile from their MVL bodies like user functions.

### Requirement 5: Expression Context vs Statement Context [MUST]

The transpiler MUST track whether an expression is emitted in statement context (result discarded, semicolon appended) or expression context (result used, no semicolon). Function calls, `match`, and `if/else` can appear in both contexts.

**Implementation:** `src/mvl/backends/rust/emit_stmts.rs`, `src/mvl/backends/rust/emit_exprs.rs`

**Tests:** `tests/transpiler.rs` (implicit in all emit tests)

#### Scenario: Function call as statement

- GIVEN `println("hello");` (result is Unit, discarded)
- WHEN transpiled
- THEN the emitted Rust has a trailing semicolon: `println!("hello");`

#### Scenario: Function call as expression

- GIVEN `let x = compute(a, b);`
- WHEN transpiled
- THEN the right-hand side has no trailing semicolon: `let x = compute(a.clone(), b.clone());`

### Requirement 6: Effect Annotations [SHOULD]

Effect annotations declared on functions (`fn foo() -> T ! E1, E2`) SHOULD be preserved as Rust doc comments on the emitted function, for auditability.

**Implementation:** `src/mvl/backends/rust/emit_functions.rs`

**Tests:** `tests/transpiler.rs::extern_rust_fn_effects_emitted_as_comment`

#### Scenario: Effect preserved as doc comment

- GIVEN `fn read_config(path: String) -> String ! FileRead`
- WHEN transpiled
- THEN the emitted Rust function has a doc comment containing `Effects: FileRead`

### Requirement 7: For-Loop Desugaring [MUST]

`for` loops MUST be desugared to Rust `for` loops with correct iterator handling. The iterable expression MUST be cloned if it is a non-Copy type (see Requirement 2).

**Implementation:** `src/mvl/backends/rust/emit_stmts.rs::emit_for`

The clone MUST wrap the entire iterable expression, not be appended after emit:

```rust
// CORRECT: wraps the expression
for item in (get_items()).clone() { ... }

// WRONG: appends after emit (fragile, misses method chains)
for item in get_items().clone() { ... }  // .clone() chains on return value, not collection
```

> **Note:** For simple identifiers, `items.clone()` and `(items).clone()` are equivalent. The parenthesised form is the general-purpose pattern that works for all expressions.

#### Scenario: For-loop over function return value

- GIVEN `for x in get_list() { ... }`
- WHEN transpiled
- THEN the iterable is cloned: `for x in (get_list()).clone() { ... }` or equivalent
- AND the emitted Rust compiles without move errors

**Tests:** `tests/transpiler.rs::for_loop_clone_expression`, `tests/transpiler.rs::for_loop_clone_fn_call_expression`

## Non-Goals (Phase 1)

These are explicitly out of scope for the current transpiler:

- **Clone elision beyond Phase B:** Phase A (last-use move) and Phase B (borrow parameter inference) are implemented. Phase C (Copy inference, cross-scope borrow) is deferred.
- **Copy inference:** Don't auto-derive Copy on structs. Derive Clone uniformly. (Phase C target.)
- **Expression-level type tracking in emitter:** The transpiler operates on AST nodes, not typed IR. Type-aware emission is Phase 3.
- **Rust formatting:** The emitted Rust does not need to pass `rustfmt`. Correctness over style.
- **Lifetime annotations:** All cloned values are owned. No borrows in emitted Rust (Phase 1).

## Open Issues

### Stdlib method dispatch simplification (#1217)

The emitter currently special-cases ~25 stdlib method calls (sort, map, filter, fold, etc.) with inline Rust/LLVM code generation instead of compiling the MVL body. This creates a 4-way sync requirement and leads to stub methods with incorrect MVL bodies. Proposed direction: eliminate emitter special-casing, keep emitter builtins only for kernel primitives. See #1217; to be documented as ADR-0041.
