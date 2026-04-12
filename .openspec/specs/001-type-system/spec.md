---
domain: language
version: 0.1.0
status: draft
date: 2026-04-11
---

# 001 — Type System

The MVL type system covers Requirements 1 (type safety), 3 (totality), 4 (null elimination), 5 (error visibility), 6 (ownership), and 10 (refinement types). It is the foundation — every other system builds on it.

## Philosophy

Types are proofs. A program that compiles has proven structural properties about itself. The type system exists to maximize the number of properties the compiler can verify per token of source code (verification density).

## Requirements

### Requirement 1: Algebraic Data Types [MUST]

The type system MUST support sum types (enums) and product types (structs) as first-class constructs. All domain states MUST be representable. Impossible states MUST be unrepresentable.

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::adt_corpus_parses_and_checks`, `tests/type_checker.rs::struct_extra_field_rejected`, `tests/type_checker.rs::field_access_on_struct_valid`

#### Scenario: Sum type with exhaustive matching

- GIVEN a type `enum Shape { Circle(Float64), Rect(Float64, Float64), Triangle(Float64, Float64) }`
- WHEN a `match` expression handles only `Circle` and `Rect`
- THEN the compiler MUST reject with "non-exhaustive match: Triangle not handled"

#### Scenario: Adding a variant forces updates

- GIVEN a `match` on `Shape` that handles all three variants
- WHEN a fourth variant `Polygon(Array<Point>)` is added to the enum
- THEN every `match` on `Shape` MUST fail compilation until the new variant is handled

### Requirement 2: Null Elimination [MUST]

The type system MUST NOT have a null, nil, or undefined value. Absence MUST be represented by `Option<T>` (either `Some(value)` or `None`). Accessing the value inside an `Option` MUST require pattern matching or explicit unwrapping.

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::option_field_access_rejected`, `tests/type_checker.rs::option_result_corpus_parses_and_checks`

#### Scenario: Option forces handling

- GIVEN `fn find_user(id: UserId) -> Option<User>`
- WHEN the caller writes `find_user(42).name`
- THEN the compiler MUST reject: "cannot access field `name` on `Option<User>`"

#### Scenario: Pattern matching on Option

- GIVEN `let user: Option<User> = find_user(42)`
- WHEN the caller writes `match user { Some(u) => u.name, None => "unknown" }`
- THEN the compiler MUST accept

### Requirement 3: Error Visibility [MUST]

Functions that can fail MUST return `Result<T, E>`. Error types MUST be visible in the function signature. The caller MUST handle the error (via `match`, `?` propagation, or combinators).

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::result_in_stmt_without_use_rejected`, `tests/type_checker.rs::result_match_missing_ok_rejected`

#### Scenario: Result forces error handling

- GIVEN `fn parse_int(s: String) -> Result<Int, ParseError>`
- WHEN the caller writes `let n: Int = parse_int(input)`
- THEN the compiler MUST reject: "expected `Int`, got `Result<Int, ParseError>`"

#### Scenario: Propagation with ?

- GIVEN a function that returns `Result<T, E>`
- WHEN the caller uses `parse_int(input)?` inside a function that also returns `Result<_, ParseError>`
- THEN the compiler MUST accept and propagate the error

### Requirement 4: Ownership and Linearity [MUST]

Every value MUST have exactly one owner. Transfer of ownership MUST be explicit (`move` semantics). Borrowing MUST be either shared-immutable (`&T`) or exclusive-mutable (`&mut T`), never both simultaneously.

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

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::refinements_corpus_parses`

#### Scenario: Division by zero prevention

- GIVEN `fn divide(a: Int, b: Int where b != 0) -> Int`
- WHEN the caller writes `divide(x, 0)`
- THEN the compiler MUST reject: "refinement violated: `0 != 0` is false"

#### Scenario: Refinement proved by guard

- GIVEN `fn divide(a: Int, b: Int where b != 0) -> Int`
- WHEN the caller writes `if y != 0 { divide(x, y) }`
- THEN the compiler MUST accept: guard proves the refinement

#### Scenario: Array bounds

- GIVEN `fn get(arr: Array<T>, i: Int where i >= 0 && i < len(arr)) -> T`
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
type Container<T> = struct { value: T }

// Generic function
total fn identity<T>(x: T) -> T {
    return x;
}

// Multiple type parameters
type Pair<A, B> = struct { first: A, second: B }

// Generic with constraint
total fn sort<T>(items: List<T>) -> List<T>
where T: Ord
{
    // …
}
```

#### Constraint syntax

Constraints MUST appear in a `where` clause after the function signature (before the body). Multiple constraints MUST be separated by commas:

```mvl
total fn merge<T, E>(a: Result<T, E>, b: Result<T, E>) -> Result<T, E>
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
- User-defined traits (declared in the module system)

**Implementation:** `src/mvl/parser/ast.rs::Constraint`, `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::generic_identity_parses`, `tests/type_checker.rs::generic_with_constraint_parses`

#### Scenario: Generic identity function

- GIVEN `total fn identity<T>(x: T) -> T { return x; }`
- WHEN the checker processes the declaration
- THEN it MUST accept: the type parameter is consistent

#### Scenario: Constraint bounds checked

- GIVEN `total fn max<T>(a: T, b: T) -> T where T: Ord { … }`
- WHEN called with `max(1, 2)` where `Int: Ord`
- THEN the compiler MUST accept

#### Scenario: Missing constraint rejected

- GIVEN `total fn max<T>(a: T, b: T) -> T { if a > b { a } else { b } }`
- WHEN the checker sees `a > b` with unconstrained `T`
- THEN the compiler MUST reject: "type parameter `T` does not implement `Ord`"

#### Scenario: No higher-kinded types

- GIVEN `type Functor<F<_>> = …` (HKT notation)
- WHEN the parser processes the declaration
- THEN it MUST reject: "higher-kinded type parameters are not supported in Phase 1"

#### Monomorphization and Rust emission

MVL generics transpile to Rust generics with the same monomorphization semantics. Each instantiation is a concrete Rust function. The transpiler MUST emit:

```rust
// MVL: total fn identity<T>(x: T) -> T { return x; }
fn identity<T>(x: T) -> T { x }

// MVL: total fn sort<T>(items: Vec<T>) -> Vec<T> where T: Ord
fn sort<T: Ord>(mut items: Vec<T>) -> Vec<T> {
    items.sort();
    items
}
```

Constraints in `where` clauses MUST map to Rust trait bounds:
- `where T: Ord` → `<T: std::cmp::Ord>`
- `where T: Eq` → `<T: std::cmp::Eq>`
- `where T: Display` → `<T: std::fmt::Display>`
