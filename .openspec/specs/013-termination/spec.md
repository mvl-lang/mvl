---
domain: language
version: 0.1.0
status: draft
date: 2026-04-14
---

# 013 — Termination Checker

The MVL termination checker covers Requirement 8 (Termination) from ADR-0001. Every `total fn`
MUST be proven to terminate by the compiler. The checker is a post-type-check pass that analyses
the typed AST; it does not require programmer annotations beyond the `total`/`partial` keyword on
function declarations.

## Philosophy

Termination is not a property that can be checked lazily. An LLM-generated function may look
locally correct while diverging for a large class of inputs. The compiler is the only agent in
the pipeline that can close this gap without manual review. Because LLMs write all the code, the
annotation burden that made termination checking impractical for human developers drops to zero.

**Origin:** Martin-Löf (1972) for structural recursion. Idris 2 (Brady, 2021) for the `total`
keyword and the checker architecture. The integer-decrement measure extends the structural measure
to primitive-recursive functions over integers without requiring dependent types.

## Scope and Defaults

Functions MUST be annotated as `total` or `partial`. A function with no annotation is implicitly
`total`. This default ensures that newly generated code is checked by default; opting out requires
explicit acknowledgment via `partial fn`.

```
total fn factorial(n: Int) -> Int { … }  // explicit
fn double(n: Int) -> Int { n * 2 }       // implicitly total — checked
partial fn stream(n: Int) -> Int { … }   // explicitly partial — exempt
```

## Requirements

### Requirement 1: Total Functions MUST Terminate [MUST]

Every `total fn` (explicit or implicit) MUST either be non-recursive or pass at least one of the
recognised decrease measures at every self-recursive call site. The compiler MUST emit
`CheckError::UnprovenRecursion` for any recursive call that cannot be proven terminating.

**Implementation:** `src/mvl/checker/termination.rs::check_structural_recursion`

**Tests:** `tests/type_checker.rs::unbounded_recursion_in_total_fn_rejected`,
`tests/type_checker.rs::non_recursive_total_fn_accepted`,
`tests/type_checker.rs::recursion_in_partial_fn_not_checked`

#### Scenario: Unbounded self-recursion rejected

- GIVEN `fn spin(n: Int) -> Int { spin(n) }`
- THEN the compiler MUST reject: "`recursive call in total function \`spin\` cannot be proven terminating`"

**Tests:** `tests/type_checker.rs::unbounded_recursion_in_total_fn_rejected`

#### Scenario: Non-recursive total fn accepted

- GIVEN `fn add(a: Int, b: Int) -> Int { a + b }`
- THEN the compiler MUST accept (trivially terminating)

**Tests:** `tests/type_checker.rs::non_recursive_total_fn_accepted`

#### Scenario: Partial fn is exempt

- GIVEN `partial fn loop_forever(n: Int) -> Int { loop_forever(n) }`
- THEN the compiler MUST accept (no termination check for partial fns)

**Tests:** `tests/type_checker.rs::recursion_in_partial_fn_not_checked`

---

### Requirement 2: Integer Decrement Measure [MUST]

A recursive call is accepted as terminating if at least one argument is of the form `param - N`
where `param` is any function parameter named in the argument expression and `N` is a positive
integer literal (`N > 0`). The checker matches the subtraction syntactically against any parameter
name — not positionally — so `f(b - 1, a)` is accepted when `b` is a parameter of `f`.

**Soundness note:** The integer-decrement measure is syntactic. The checker does not verify that
the parameter holds a non-negative value at the call site. A total function that calls itself with
`n - 1` where `n` may be negative will diverge for negative inputs. Callers are expected to
supply a non-negative value or the function must include an explicit base-case guard.

**Implementation:** `src/mvl/checker/termination.rs::arg_decreases`

**Tests:** `tests/type_checker.rs::integer_decrement_recursion_accepted`,
`tests/type_checker.rs::increasing_recursion_in_total_fn_rejected`

#### Scenario: Integer decrement accepted

- GIVEN `fn fact(n: Int) -> Int { if n == 0 { 1 } else { n * fact(n - 1) } }`
- THEN the compiler MUST accept (`n - 1` is a syntactic decrement of parameter `n`)

**Tests:** `tests/type_checker.rs::integer_decrement_recursion_accepted`

#### Scenario: Increasing argument rejected

- GIVEN `fn bad(n: Int) -> Int { bad(n + 1) }`
- THEN the compiler MUST reject (`n + 1` does not match the decrement pattern)

**Tests:** `tests/type_checker.rs::increasing_recursion_in_total_fn_rejected`

#### Scenario: Decrement by zero rejected

- GIVEN `fn f(n: Int) -> Int { f(n - 0) }`
- THEN the compiler MUST reject (`N == 0` is not a decrease)

**Tests:** `tests/type_checker.rs::decrement_by_zero_in_total_fn_rejected`

#### Scenario: Integer division by constant accepted

A recursive call is ALSO accepted when at least one argument is of the form `param / N` where
`param` is any function parameter and `N` is an integer literal greater than 1. This catches
binary search, merge sort, and other logarithmic algorithms without requiring `partial`.

**Soundness note:** The integer-division measure is syntactic. The checker does not verify that
`param` holds a non-negative value at the call site. See Known Limitation §L5.

- GIVEN `fn halve(n: Int) -> Int { if n == 0 { 0 } else { halve(n / 2) } }`
- THEN the compiler MUST accept (`n / 2` is a syntactic division of parameter `n` by a constant > 1)

**Tests:** `tests/type_checker.rs::division_by_constant_recursion_accepted`,
`tests/type_checker.rs::division_by_large_constant_recursion_accepted`

#### Scenario: Division by one rejected

- GIVEN `fn f(n: Int) -> Int { f(n / 1) }`
- THEN the compiler MUST reject (`N == 1` is not a decrease — the value is unchanged)

**Tests:** `tests/type_checker.rs::division_by_one_in_total_fn_rejected`

---

### Requirement 3: Structural Subterm Measure [MUST]

A recursive call is accepted as terminating if at least one argument is a variable that was
pattern-bound from an *immediate sub-pattern* of a direct function parameter in a surrounding
`match` expression. Only variables bound at depth 1 (one level below the matched value) qualify;
binding the whole value (`Pattern::Ident` matching the scrutinee) does not.

The structural subterm relation is established only when the `match` scrutinee is a bare function
parameter identifier. Matching on a local variable, field access, or expression result does NOT
establish the relation — those bindings are not in the `smaller` set.

**Implementation:** `src/mvl/checker/termination.rs::subterm_vars`,
`src/mvl/checker/termination.rs::as_param`

**Tests:** `tests/type_checker.rs::structural_recursion_on_adt_subterm_accepted`

#### Scenario: Structural recursion on ADT subterm accepted

- GIVEN `enum List { Nil, Cons(Int, List) }` and
  ```
  fn len(list: List) -> Int {
      match list {
          List::Nil => 0
          List::Cons(_, tail) => 1 + len(tail)
      }
  }
  ```
- THEN the compiler MUST accept (`tail` is bound from `Cons(_, tail)` where `list` is a parameter)

**Tests:** `tests/type_checker.rs::structural_recursion_on_adt_subterm_accepted`

#### Scenario: Match on non-parameter does not grant subterm status

- GIVEN `fn f(list: List) -> Int { let local = list; match local { List::Cons(_, tail) => f(tail) … } }`
- THEN the compiler MUST reject (`local` is not a bare parameter; `tail` is not in `smaller`)

**Tests:** `tests/type_checker.rs::structural_recursion_via_non_param_match_rejected`

#### Scenario: Option subterm accepted

- GIVEN a function matching on `Some(inner)` where the parameter is the scrutinee and recursing with `inner`
- THEN the compiler MUST accept (`inner` is a structural subterm of the `Option` parameter)

**Tests:** `tests/type_checker.rs::structural_recursion_on_adt_single_field_accepted`

#### Scenario: Method accessor on parameter accepted [MUST]

A recursive call is ALSO accepted when an argument is `param.tail()` or `param.rest()` (called
with zero arguments) where `param` is any function parameter. These zero-argument accessor methods
are treated as yielding a strict structural subterm of their receiver. The same applies when the
receiver is a variable already in the `smaller` set (i.e. a known structural subterm).

- GIVEN `fn f(xs: List[Int]) -> Int { if xs == [] { 0 } else { f(xs.tail()) } }`
- THEN the compiler MUST accept (`xs.tail()` is a structural subterm of parameter `xs`)

**Tests:** `tests/type_checker.rs::tail_accessor_recursion_accepted`,
`tests/type_checker.rs::rest_accessor_recursion_accepted`

#### Scenario: Method accessor on non-parameter rejected

- GIVEN `fn f(xs: List[Int]) -> Int { let local = xs; f(local.tail()) }`
- THEN the compiler MUST reject (`local` is not a function parameter; `local.tail()` is not a proven structural decrease)

**Tests:** `tests/type_checker.rs::tail_on_local_variable_rejected`

#### Scenario: Subterm length accepted [MUST]

A recursive call is ALSO accepted when an argument is `subterm.len()` (called with zero arguments)
where `subterm` is a variable in the `smaller` set (i.e. a known structural subterm bound via
pattern match). The length of a structural subterm is provably smaller than the length of the
original parameter. Bare function parameters do NOT qualify — only pattern-bound subterms.

- GIVEN `fn f(xs: List) -> Int { match xs { List::Nil => 0, List::Cons(_, tail) => f(tail, tail.len()) } }`
- THEN the compiler MUST accept (`tail` is a structural subterm; `tail.len()` is its length, which is smaller)

**Tests:** `tests/type_checker.rs::subterm_len_recursion_accepted`

#### Scenario: Length of bare parameter not accepted

- GIVEN `fn f(xs: List[Int]) -> Int { if xs == [] { 0 } else { f(xs.len()) } }`
- THEN the compiler MUST reject (`xs` is a direct parameter, not a structural subterm; `xs.len()` is not a proven decrease)

**Tests:** `tests/type_checker.rs::len_on_param_directly_rejected`

---

### Requirement 4: Lambdas Are Out of Scope [MUST]

The termination checker MUST NOT descend into lambda expressions. Lambdas have their own
lexical scope and are not self-recursive with respect to the enclosing function. A call to a
function named the same as the enclosing `total fn` inside a lambda MUST NOT produce
`UnprovenRecursion`.

**Implementation:** `src/mvl/checker/termination.rs::check_expr` (`Expr::Lambda` arm)

**Tests:** `tests/type_checker.rs::recursion_inside_lambda_not_flagged`

#### Scenario: Lambda referencing enclosing function name not flagged

- GIVEN `fn outer(n: Int) -> Int { let f = |x| outer(x); n + 1 }`
- THEN the compiler MUST accept (the call to `outer` is inside a lambda, not a direct recursion)

**Tests:** `tests/type_checker.rs::recursion_inside_lambda_not_flagged`

---

### Requirement 5: For Loops Are Trivially Terminating [MUST]

`for` loops iterate over finite iterators. The checker MUST accept `for` loops in total functions
without additional proof. Recursive calls inside a `for` loop body are subject to the same
decrease-measure rules as recursive calls anywhere else in the function.

`while` loops in total functions are already rejected by the type checker as
`CheckError::UnboundedLoopInTotal` before the termination pass runs. The termination pass
therefore treats `while` as a no-op (the error has already been emitted).

**Precondition:** `TypeChecker::check_program` MUST have run before `check_structural_recursion`
is called, so that any `while` loop in a total function is already flagged.

**Implementation:** `src/mvl/checker/termination.rs::check_stmt` (`Stmt::For` and `Stmt::While` arms)

**Tests:** `tests/type_checker.rs::for_loop_in_total_function_accepted`,
`tests/type_checker.rs::while_loop_in_total_function_rejected`

---

## Known Limitations (Phase 1)

The following are recognised limitations of the Phase 1 termination checker. They do not
constitute spec violations — they are design boundaries, tracked for Phase 2.

### L1: Mutual recursion is not checked

Functions that are not self-recursive but form a call-graph cycle (e.g. `f` calls `g`, `g` calls
`f`) are not detected. The compiler will accept them even if the cycle has no decreasing measure.
Tracked in #142.

### L2: While-loop decreasing measures are not supported

`while` loops with an explicit decreasing measure annotation (as in Lean 4 or Dafny) are not yet
supported. `while` in total functions is unconditionally rejected in Phase 1. Tracked in #142.

### L3: Integer-decrement measure assumes non-negative input

The syntactic `param - N` check does not verify that `param` is non-negative at the call site.
A total function may diverge for negative inputs and still be accepted by the checker.

### L5: Integer-division measure assumes non-negative dividend

The syntactic `param / N` check does not verify that `param` is non-negative at the call site.
A total function may diverge for negative inputs and still be accepted by the checker. This is
the same class of limitation as §L3 (integer-decrement measure) — both measures rely on the
caller supplying non-negative values or the function including an explicit base-case guard.

### L4: Subterm variable shadowing inside match arm bodies is not tracked

If a `let` binding inside a match arm re-binds a name that is in the `smaller` set (e.g.
`List::Cons(_, tail) => { let tail = 99; f(tail) }`), the checker will incorrectly accept
`f(tail)` as terminating. This is a known soundness gap; its real-world impact is low because
shadowing a subterm variable in an arm body is unusual style.

---

## Deferred to Phase 2

| Item | Tracking |
|------|----------|
| Mutual recursion (call-graph cycle detection) | #142 |
| `while` loop with decreasing measure annotation | #142 |
| Non-negative precondition check for integer-decrement and integer-division measures | future |
| Subterm shadowing tracking in match arm bodies | future |
| Per-function Req 8 status in assurance report | future |
