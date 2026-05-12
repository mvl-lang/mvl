---
domain: language
version: 0.1.0
status: draft
date: 2026-04-11
---

# 001 — Type System

The MVL type system covers Requirements 1 (type safety), 3 (totality), 4 (null elimination), 5 (error visibility), 6 (ownership), 9 (generics), and 10 (refinement types). It is the foundation — every other system builds on it.

> **Note:** Requirement numbers in this spec are local to this file and do not map 1:1 to the eleven requirements in ADR-0001.

## Philosophy

Types are proofs. A program that compiles has proven structural properties about itself. The type system exists to maximize the number of properties the compiler can verify per token of source code (verification density).

## Requirements

### Requirement 1: Algebraic Data Types [MUST]

The type system MUST support sum types (enums) and product types (structs) as first-class constructs. All domain states MUST be representable. Impossible states MUST be unrepresentable.

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::adt_corpus_parses_and_checks`, `tests/type_checker.rs::struct_extra_field_rejected`, `tests/type_checker.rs::field_access_on_struct_valid`, `tests/compile_and_run.rs::linked_list_check_passes`, `tests/compile_and_run.rs::linked_list_runs_and_produces_expected_output` (#194)

#### Scenario: Sum type with exhaustive matching

- GIVEN a type `enum Shape { Circle(Float64), Rect(Float64, Float64), Triangle(Float64, Float64) }`
- WHEN a `match` expression handles only `Circle` and `Rect`
- THEN the compiler MUST reject with "non-exhaustive match: Triangle not handled"

#### Scenario: Adding a variant forces updates

- GIVEN a `match` on `Shape` that handles all three variants
- WHEN a fourth variant `Polygon(Array[Point])` is added to the enum
- THEN every `match` on `Shape` MUST fail compilation until the new variant is handled

### Requirement 2: Null Elimination [MUST]

The type system MUST NOT have a null, nil, or undefined value. Absence MUST be represented by `Option[T]` (either `Some(value)` or `None`). Accessing the value inside an `Option` MUST require pattern matching or explicit unwrapping.

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::option_field_access_rejected`, `tests/type_checker.rs::option_result_corpus_parses_and_checks`

#### Scenario: Option forces handling

- GIVEN `fn find_user(id: UserId) -> Option[User]`
- WHEN the caller writes `find_user(42).name`
- THEN the compiler MUST reject: "cannot access field `name` on `Option[User]`"

#### Scenario: Pattern matching on Option

- GIVEN `let user: Option[User] = find_user(42)`
- WHEN the caller writes `match user { Some(u) => u.name, None => "unknown" }`
- THEN the compiler MUST accept

### Requirement 3: Error Visibility [MUST]

Functions that can fail MUST return `Result[T, E]`. Error types MUST be visible in the function signature. The caller MUST handle the error (via `match`, `?` propagation, or combinators).

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::result_in_stmt_without_use_rejected`, `tests/type_checker.rs::result_match_missing_ok_rejected`, `tests/compile_and_run.rs::safe_division_check_passes`, `tests/compile_and_run.rs::safe_division_runs_and_produces_expected_output` (#191)

#### Scenario: Result forces error handling

- GIVEN `fn parse_int(s: String) -> Result[Int, ParseError]`
- WHEN the caller writes `let n: Int = parse_int(input)`
- THEN the compiler MUST reject: "expected `Int`, got `Result[Int, ParseError]`"

#### Scenario: Propagation with ?

- GIVEN a function that returns `Result[T, E]`
- WHEN the caller uses `parse_int(input)?` inside a function that also returns `Result<_, ParseError>`
- THEN the compiler MUST accept and propagate the error

### Requirement 4: Ownership and Linearity [MUST]

Every value MUST have exactly one owner. Transfer of ownership MUST be explicit (`move` semantics). Borrowing MUST be either read-only (`val T`) or mutable (`ref T`), never both simultaneously.

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::use_after_explicit_move_rejected`, `tests/type_checker.rs::ownership_corpus_parses`

#### Scenario: Use after move

- GIVEN `let a = create_resource()`
- WHEN `let b = a` (ownership transferred) followed by `use(a)`
- THEN the compiler MUST reject: "value used after move"

#### Scenario: Shared and mutable borrow conflict

- GIVEN `let mut v = vec![1, 2, 3]`
- WHEN `let r = &v[0]` followed by `v.push(4)`
- THEN the compiler MUST reject: "cannot borrow `v` as mutable while shared borrow exists"

### Requirement 5: Refinement Types [MUST]

The type system MUST support refinement predicates on types: `T where predicate`. The compiler MUST verify refinements statically where possible and insert runtime checks where necessary.

This requirement also covers **function contracts** — the relational extension of refinement types:
- `requires pred` / `ensures pred` — pre/postconditions on function calls (Phases 1–2, #621)
- `ghost let x: T = e` — specification-only bindings erased before codegen (Phase 4, #627)
- `invariant pred` on `while` loops — checked at entry and after each iteration (Phase 3, #621)
- `decreases expr` on `while` loops — termination measure (Phase 5, #628)
- `forall x: T, pred` / `exists x: T, pred` — quantifiers in contract positions (Phase 5, #628)

See ADR-0025 for the full contract system design and phased implementation.

**Implementation:** `src/mvl/checker/mod.rs`, `src/mvl/checker/contracts.rs`, `src/mvl/checker/refinements.rs`

**Tests:** `tests/type_checker.rs::refinements_corpus_parses`, `tests/compile_and_run.rs::safe_division_check_passes`, `tests/compile_and_run.rs::safe_division_runs_and_produces_expected_output` (#191); `tests/type_checker.rs::loop_verification_corpus_parses_and_checks` (#628)

#### Scenario: Division by zero prevention

- GIVEN `fn divide(a: Int, b: Int where b != 0) -> Int`
- WHEN the caller writes `divide(x, 0)`
- THEN the compiler MUST reject: "refinement violated: `0 != 0` is false"

#### Scenario: Refinement proved by guard

- GIVEN `fn divide(a: Int, b: Int where b != 0) -> Int`
- WHEN the caller writes `if y != 0 { divide(x, y) }`
- THEN the compiler MUST accept: guard proves the refinement

#### Scenario: Array bounds

- GIVEN `fn get(arr: Array[T], i: Int where i >= 0 && i < len(arr)) -> T`
- WHEN the caller writes `get(arr, arr.len())`
- THEN the compiler MUST reject: "refinement violated: `len(arr) < len(arr)` is false"

### Requirement 6: Immutable by Default [MUST]

All bindings and struct fields MUST be immutable unless explicitly marked `mut`. Mutation MUST be visible at the declaration site.

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::immutable_binding_assignment_rejected`, `tests/type_checker.rs::immutable_field_mutation_rejected`, `tests/type_checker.rs::immutability_corpus_parses_and_checks`

#### Scenario: Immutable binding

- GIVEN `let x = 5`
- WHEN the caller writes `x = 6`
- THEN the compiler MUST reject: "cannot assign to immutable binding `x`"

#### Scenario: Mutable opt-in

- GIVEN `let mut x = 5`
- WHEN the caller writes `x = 6`
- THEN the compiler MUST accept

### Requirement 7: Totality — Exhaustive Matching [MUST]

Every `match` expression MUST cover all variants of the matched type. The compiler MUST reject non-exhaustive matches.

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::enum_match_missing_variant_rejected`, `tests/type_checker.rs::exhaustive_match_corpus_parses_and_checks`

#### Scenario: Missing variant

- GIVEN `match result { Ok(v) => v }`
- THEN the compiler MUST reject: "non-exhaustive: `Err(_)` not handled"

### Requirement 8: No Null, No Exceptions, No Global State [MUST]

The language MUST NOT contain: null/nil/undefined values, throw/catch/try exception mechanism, global mutable variables, or implicit type conversions.

**Implementation:** `src/mvl/parser/functions.rs::parse_decl`

**Tests:** `src/mvl/parser/functions.rs::static_mut_is_rejected`, `src/mvl/parser/functions.rs::static_decl_is_rejected`, `src/mvl/parser/functions.rs::global_keyword_is_rejected`, `src/mvl/parser/statements.rs::throw_is_rejected`, `src/mvl/parser/statements.rs::try_is_rejected`, `src/mvl/parser/statements.rs::catch_is_rejected` (#289)

#### Scenario: No global state

- GIVEN `static mut COUNTER: Int = 0`
- THEN the parser MUST reject: `static mut` is not valid MVL syntax

### Requirement 9: Generics [MUST]

The type system MUST support parametric polymorphism via type parameters (generics). Generics MUST be monomorphized at compile time (Rust-style), producing one concrete instantiation per unique type argument set. There MUST be no runtime type dispatch overhead. Higher-kinded types (HKT) are NOT supported in Phase 1.

#### Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Strategy | Monomorphization | Aligns with ownership model; zero-cost abstraction; Rust transpilation is direct |
| Constraint syntax | `where T: Trait` (Rust-style) | Separate clause keeps type signatures readable; consistent with effect annotations |
| Inline syntax | NOT supported | `fn foo<T: Ord>` inline constraints are rejected in Phase 1 (deferred) |
| Higher-kinded types | NOT supported in Phase 1 | Keep type inference tractable; add HKT in Phase 2 if needed for Effect handlers |
| Type inference | Per-expression, not Hindley-Milner | Explicit type annotations at function boundaries; local inference inside bodies |
| Variance | Invariant (default) | Safe default; covariant/contravariant annotations deferred to Phase 2 |

#### Type parameter declarations

Type parameters MUST be declared in angle brackets after the item name:

```mvl
// Generic type declaration
type Container[T] = struct { value: T }

// Generic function
total fn identity[T](x: T) -> T {
    return x;
}

// Multiple type parameters
type Pair[A, B] = struct { first: A, second: B }

// Generic with constraint
total fn sort[T](items: List[T]) -> List[T]
where T: Ord
{
    // …
}
```

#### Constraint syntax

Constraints MUST appear in a `where` clause after the function signature (before the body). Multiple constraints MUST be separated by commas:

```mvl
total fn merge[T, E](a: Result[T, E], b: Result[T, E]) -> Result[T, E]
where T: Eq, E: Display
{
    // …
}
```

**Supported trait bounds** (Phase 1):
- `Eq` — structural equality (`==`, `!=`)
- `Ord` — total ordering (`<`, `>`, `<=`, `>=`)
- `Display` — human-readable formatting
- `Clone` — explicit value duplication
- `Default` — zero-value construction
- `Iterator[T]` — lazy iteration protocol (see Requirement 11)
- User-defined traits (declared in the module system)

**Implementation:** `src/mvl/parser/ast.rs::GenericParam`, `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::generic_identity_parses`, `tests/type_checker.rs::generic_type_decl_parses`, `tests/type_checker.rs::generic_pair_type_parses`, `tests/type_checker.rs::generic_with_constraint_parses`, `tests/type_checker.rs::generic_multiple_constraints_parse`, `tests/compile_and_run.rs::linked_list_check_passes`, `tests/compile_and_run.rs::linked_list_runs_and_produces_expected_output` (#194)

> **Phase 1 scope note:** The parser accepts and stores generics AST nodes. Constraint *enforcement* (rejecting unconstrained `T` in operator expressions, rejecting HKT notation with a clear diagnostic, rejecting inline `<T: Ord>` syntax) is tracked for Phase 2 implementation. See rejection scenarios below for the intended semantics.

#### Scenario: Generic identity function

- GIVEN `total fn identity[T](x: T) -> T { return x; }`
- WHEN the checker processes the declaration
- THEN it MUST accept: the type parameter is consistent

#### Scenario: Constraint bounds checked

- GIVEN `total fn max[T](a: T, b: T) -> T where T: Ord { … }`
- WHEN called with `max(1, 2)` where `Int: Ord`
- THEN the compiler MUST accept

#### Scenario: Missing constraint rejected

- GIVEN `total fn max[T](a: T, b: T) -> T { if a > b { a } else { b } }`
- WHEN the checker sees `a > b` with unconstrained `T`
- THEN the compiler MUST reject: "type parameter `T` does not implement `Ord`"

#### Scenario: No higher-kinded types

- GIVEN `type Functor<F<_>> = …` (HKT notation)
- WHEN the parser processes the declaration (Phase 1: grammar rejects nested angle brackets in type params)
- THEN it MUST reject at the parser level with an error referencing the unexpected token

#### Scenario: Inline constraint syntax rejected

- GIVEN `total fn max<T: Ord>(a: T, b: T) -> T { return a; }`
- WHEN the parser processes the declaration
- THEN it MUST reject: inline type constraints in `<T: Trait>` form are not valid in Phase 1; use a `where` clause

#### Monomorphization and Rust emission

MVL generics transpile to Rust generics with the same monomorphization semantics. Each instantiation is a concrete Rust function. The transpiler MUST emit:

```rust
// MVL: total fn identity[T](x: T) -> T { return x; }
fn identity<T>(x: T) -> T { x }

// MVL: total fn sort[T](items: Vec<T>) -> Vec<T> where T: Ord
fn sort<T>(mut items: Vec<T>) -> Vec<T>
where T: std::cmp::Ord
{
    items.sort();
    items
}
```

Constraints in `where` clauses MUST map to Rust trait bounds (fully-qualified paths):
- `where T: Ord` → `T: std::cmp::Ord`
- `where T: Eq` → `T: std::cmp::Eq`
- `where T: Display` → `T: std::fmt::Display`
- `where T: Clone` → `T: std::clone::Clone`
- `where T: Default` → `T: std::default::Default`

---

### Requirement 10: Debug and Display Traits [MUST]

Every struct and enum MUST automatically derive `Debug` (auto-derivable via Rust's `#[derive(Debug)]`).
Users MAY implement `Display` for custom string representation using `impl Display for T`.

#### Debug trait

All struct and enum declarations MUST emit `#[derive(Debug, Clone, PartialEq)]` so any value can be debug-printed.

**Implementation:** `src/mvl/backends/rust/emit_types.rs::emit_struct`, `src/mvl/backends/rust/emit_types.rs::emit_enum`

**Tests:** `tests/transpiler.rs::struct_derives_debug`, `tests/transpiler.rs::enum_derives_debug`

#### Display trait syntax

```mvl
impl Display for Point {
    fn fmt(self: Point) -> String {
        format("({}, {})", self.x, self.y)
    }
}
```

Transpiles to:

```rust
impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format!("({}, {})", self.x, self.y))
    }
}
```

**Implementation:** `src/mvl/backends/rust/emit_impls.rs::emit_display_impl`

**Tests:** `tests/transpiler.rs::impl_display_emits_display_trait`

#### format() function

The `format()` built-in accepts a Rust-style format string (using `{}`, `{:?}`, `{:08x}`, etc.) and variadic arguments, returning a `String`. The IFC label of the result SHOULD be the join of all argument labels (enforcement deferred to Phase 2 type checker).

```mvl
let msg: String = format("Hello, {}!", name)
let hex: String = format("{:08x}", value)
```

**Implementation:** `src/mvl/backends/rust/emit_exprs.rs` (mapped to Rust `format!` macro)

**Tests:** `tests/transpiler.rs::format_call_emits_format_macro`

#### Number literal formats

Integer literals MUST support hex (`0xFF`), binary (`0b1010`), and octal (`0o77`) prefixes.
Float literals MUST support scientific notation (`1.5e10`, `2.0e-3`).

**Implementation:** `src/mvl/parser/lexer.rs::lex_number`, `src/mvl/parser/lexer.rs::lex_integer_base`

**Tests:** `src/mvl/parser/lexer.rs::tokenize_hex_literal`, `tokenize_binary_literal`, `tokenize_octal_literal`, `tokenize_scientific_notation`

#### Scenario: Display impl emits Rust fmt::Display

- GIVEN `impl Display for Point { fn fmt(self: Point) -> String { format("({}, {})", self.x, self.y) } }`
- WHEN the transpiler processes the declaration
- THEN the output MUST contain `impl std::fmt::Display for Point`
- AND the output MUST contain `fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result`
- AND the output MUST contain `write!(f, "{}"` wrapping the body expression

---

### Requirement 11: Iterator Trait [MUST]

The type system MUST define the `Iterator[T]` trait as the protocol for lazy, sequential element access. Every type used in a `for...in` loop MUST implement `Iterator[T]`. Collection operations that transform sequences (`map`, `filter`, `flat_map`) MUST return `Iterator[U]` rather than a concrete collection — evaluation is deferred until elements are consumed.

**Implementation:** `src/mvl/checker/mod.rs`, `src/mvl/backends/rust/emit_impls.rs`, `src/mvl/backends/rust/emit_stmts.rs`

#### Iterator trait definition

```mvl
type Iterator[T] = trait {
    fn next(mut self) -> Option[T]
}
```

`next` takes `mut self` — it advances the iterator in place and returns the next element, or `None` when exhausted. All iterators MUST be fused: once `None` is returned, every subsequent call to `next` MUST also return `None`.

#### Built-in Iterator implementations

The following core types MUST implement `Iterator[T]`:

| Type | Element type | Notes |
|------|-------------|-------|
| `Array[T]` | `T` | via `.iter()` method |
| `Range` | `Int` | `0..10` produces `0, 1, …, 9` |
| `Map[K, V]` | `(K, V)` | insertion order |
| `Set[T]` | `T` | unspecified order |

#### For loop desugaring

`for x in expr { body }` desugars to repeated `next()` calls:

```mvl
// Source
for item in collection {
    process(item);
}

// Desugars to (conceptually)
let mut iter: Iterator[T] = collection.iter();
while let Some(item) = iter.next() {
    process(item);
}
```

The type checker MUST verify that the expression after `in` implements `Iterator[T]` or has an `.iter()` method that returns `Iterator[T]`. The for loop MUST only appear in `total` functions — bounded iteration is guaranteed by the fused, finite iterator contract. Infinite iterators (types whose `next` never returns `None`) are only permitted in `partial` functions using `while`.

#### Lazy collection operations

`map`, `filter`, and `flat_map` MUST return `Iterator`, not a concrete collection. No elements are computed until consumed:

```mvl
fn map[T, U](self: Iterator[T], f: fn(T) -> U) -> Iterator[U]
fn filter[T](self: Iterator[T], pred: fn(val T) -> Bool) -> Iterator[T]
fn flat_map[T, U](self: Iterator[T], f: fn(T) -> Iterator[U]) -> Iterator[U]
fn enumerate[T](self: Iterator[T]) -> Iterator[(UInt, T)]
fn zip[T, U](self: Iterator[T], other: Iterator[U]) -> Iterator[(T, U)]
```

Terminal operations that force evaluation:

```mvl
fn fold[T, U](self: Iterator[T], init: U, f: fn(U, T) -> U) -> U
fn collect[T](self: Iterator[T]) -> Array[T]
fn any[T](self: Iterator[T], pred: fn(val T) -> Bool) -> Bool
fn all[T](self: Iterator[T], pred: fn(val T) -> Bool) -> Bool
fn find[T](self: Iterator[T], pred: fn(val T) -> Bool) -> Option[T]
fn sum[T](self: Iterator[T]) -> T  where T: Add, T: Default
fn min[T](self: Iterator[T]) -> Option[T]  where T: Ord
fn max[T](self: Iterator[T]) -> Option[T]  where T: Ord
```

#### Custom Iterator implementations

Any user-defined type MAY implement `Iterator[T]`:

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

Once implemented, the type can be used directly in `for...in`:

```mvl
for n in Counter { current: 0, limit: 5 } {
    println(n.to_string());
}
// prints: 0, 1, 2, 3, 4
```

#### Constraint syntax

Functions accepting any iterable use `where T: Iterator[E]`:

```mvl
fn sum_all[T, E](items: T) -> E
where T: Iterator[E], E: Add, E: Default
{
    items.fold(E.default(), |acc, x| acc + x)
}
```

#### Transpilation

`Iterator[T]` transpiles to Rust's `std::iter::Iterator<Item = T>`:

```rust
// MVL: type Iterator[T] = trait { fn next(mut self) -> Option[T] }
// → Rust built-in: std::iter::Iterator

// MVL: impl Iterator[Int] for Counter
impl std::iter::Iterator for Counter {
    type Item = i64;
    fn next(&mut self) -> Option<i64> { … }
}

// MVL: for item in collection
for item in collection.iter() { … }
```

**Tests:** `tests/type_checker.rs::iterator_trait_for_loop_accepted`, `tests/type_checker.rs::non_iterator_for_loop_rejected`, `tests/type_checker.rs::custom_iterator_impl_accepted`, `tests/type_checker.rs::for_loop_rejected_in_partial_fn`, `tests/transpiler.rs::iterator_impl_emits_rust_iterator`

#### Scenario: For loop over array accepted

- GIVEN `let items: Array[Int] = [1, 2, 3]`
- WHEN `for x in items { println(x.to_string()); }`
- THEN the type checker MUST accept: `Array[Int]` implements `Iterator[Int]`

#### Scenario: For loop over non-iterator rejected

- GIVEN `let n: Int = 42`
- WHEN `for x in n { … }`
- THEN the type checker MUST reject: "`Int` does not implement `Iterator`"

#### Scenario: Custom type implements Iterator

- GIVEN `type Counter = struct { mut current: Int, limit: Int }` with `impl Iterator[Int] for Counter`
- WHEN `for n in Counter { current: 0, limit: 3 } { … }`
- THEN the type checker MUST accept

#### Scenario: Lazy map does not allocate intermediate collection

- GIVEN `let result = items.iter().map(|x| x + 1).filter(|x| x > 2).collect()`
- WHEN the expression is type-checked
- THEN the type of `.map(…)` MUST be `Iterator[Int]`, not `Array[Int]`
- AND no intermediate array MUST be allocated between `.map()` and `.filter()`

#### Scenario: Fold terminates the chain

- GIVEN `let sum = items.iter().map(|x| x * 2).fold(0, |acc, x| acc + x)`
- THEN `fold` MUST consume the iterator and return `Int`
- AND the result MUST equal the sum of doubled elements

#### Scenario: For loop rejected inside partial function

- GIVEN `partial fn f(items: Array[Int]) { for x in items { println(x.to_string()); } }`
- WHEN the function is type-checked
- THEN the type checker MUST reject with: "`for` is not permitted in `partial` functions; use `while` instead"

---

### Requirement 12: Explicit Type Annotations [MUST]

Every `let` and `let mut` binding MUST declare its type explicitly using the `: T` annotation. The parser MUST reject any `let` binding that omits the type annotation. Type inference from the initializer expression MUST NOT substitute for an explicit annotation.

This is the enforcement of Design Principle 1 ("Explicit over implicit") at the binding level. It is also the reason Spec 011 Requirement 4 (`unnecessary-annotation` rule) was removed (#408): with annotations mandatory, no annotation can be "unnecessary".

**Implementation:** `src/mvl/parser/statements.rs::parse_let_stmt`

**Tests:** `src/mvl/parser/statements.rs::parse_let_without_type`, `tests/type_checker.rs::let_without_annotation_rejected`, `tests/type_checker.rs::let_mut_without_annotation_rejected`, `tests/type_checker.rs::let_with_annotation_accepted`

#### Scenario: Unannotated let binding rejected

- GIVEN `let y = 99`
- WHEN the parser processes the statement
- THEN the parser MUST reject with a parse error (missing type annotation)

#### Scenario: Unannotated let mut binding rejected

- GIVEN `let mut count = 0`
- WHEN the parser processes the statement
- THEN the parser MUST reject with a parse error (missing type annotation)

#### Scenario: Annotated binding accepted

- GIVEN `let x: Int = 42`
- WHEN the parser processes the statement
- THEN the parser MUST accept: the type annotation is present

---

### Requirement 13: Minimal Control-Flow Surface [MUST]

The language MUST provide exactly one construct for each control-flow category:

- **Bounded iteration:** `for x in expr { }` — available in `total` functions only
- **Unbounded iteration:** `while condition { }` — available in `partial` functions only
- **Conditional:** `if`/`else if`/`else`
- **Pattern dispatch:** `match`
- **Error propagation:** `Result[T, E]` with `?` propagation (see Requirement 3)

No other iteration, conditional, or error-propagation constructs are permitted. The parser MUST reject `throw`, `try`, `catch`, and `goto`. The type checker MUST reject `while` in `total` functions.

This is Design Principle 2 ("One way to do each thing"). Cross-references: Requirement 3 (Result), Requirement 8 (no throw/catch/try), Spec 002 Requirement 5 (total/partial distinction).

**Implementation:** `src/mvl/parser/statements.rs`, `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::while_loop_in_total_function_rejected`, `tests/type_checker.rs::while_loop_in_implicit_total_function_rejected`, `src/mvl/parser/statements.rs::throw_is_rejected`, `src/mvl/parser/statements.rs::try_is_rejected`, `src/mvl/parser/statements.rs::catch_is_rejected`

#### Scenario: while in total function rejected

- GIVEN `total fn f() -> Int { let mut i: Int = 0; while i < 10 { i = i + 1; } return i; }`
- WHEN the type checker processes the function
- THEN it MUST reject: "`while` is not permitted in `total` functions; use `for` instead"

#### Scenario: throw rejected

- GIVEN `throw SomeError`
- WHEN the parser processes the statement
- THEN it MUST reject: `throw` is not valid MVL syntax

#### Scenario: try/catch rejected

- GIVEN `try { risky() } catch (e) { handle(e) }`
- WHEN the parser processes the statement
- THEN it MUST reject: `try` and `catch` are not valid MVL syntax

---

### Requirement 14: Vocabulary over Syntax [MUST]

The language MUST NOT provide a macro facility accessible in user source code. Common language-level operations (string formatting, I/O, collection construction) SHALL be expressed as stdlib function calls, not as special syntax or macros.

The transpiler MAY internally map specific stdlib functions (e.g., `println`, `format`) to Rust macros for implementation efficiency, but from the MVL source perspective these are ordinary function calls. No MVL source file ever contains macro invocation syntax.

This is Design Principle 3 ("Vocabulary over syntax").

**Implementation:** `src/mvl/backends/rust/emit_exprs.rs` (MACRO_HANDLED list), `src/mvl/stdlib/`

**Tests:** `tests/transpiler.rs::format_call_emits_format_macro`, `tests/transpiler.rs::macro_handled_names_are_excluded_from_prelude`

#### Scenario: format() is a function call in MVL source

- GIVEN `fn greeting(name: String) -> String { format("{} world", name) }`
- WHEN the transpiler processes the function
- THEN the Rust output MUST contain `format!(` — the macro invocation is an internal transpiler detail
- AND the MVL source contains a plain function call `format(...)`, not macro syntax `format!(...)`

#### Scenario: println is a stdlib function, not a macro

- GIVEN `pub fn println(value: String) -> Unit { ... }` defined in the stdlib prelude
- WHEN the transpiler processes a program that calls `println("hello")`
- THEN `println` MUST NOT appear as a regular Rust function definition in the output (it is macro-handled)
- AND the call site emits `println!("hello")` in Rust, transparently to the MVL programmer
