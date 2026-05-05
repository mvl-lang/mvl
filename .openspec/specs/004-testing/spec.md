---
domain: language
version: 0.1.0
status: draft
date: 2026-04-15
---

# 004 — Testing

MVL testing approach — language-level unit tests that survive regeneration and
integrate with the ISPE assurance model.

---

### Requirement 1: Internal Test Functions [MUST]

MVL functions prefixed with `test` are unit tests internal to the module.
They have access to private API and are part of the Program (P) layer.
The transpiler MUST emit them inside a `#[cfg(test)] mod tests { … }` block
with the `#[test]` attribute so `cargo test` picks them up.

```mvl
test fn check_add() -> Unit {
    assert_eq(add(1, 2), 3)
}
```

**Syntax:** `test fn <name>(<params>) -> <return_type> { <body> }`

- `test` is a keyword prefix (like `total` / `partial`).
- Test functions MUST NOT be `pub` — they are development artefacts.
- Test functions MAY be combined with `total` / `partial`: `test total fn …`.
- Regeneration (Article 4): internal tests are regenerated with the module.
  External tests (`_test.mvl` files) are permanent evidence (E layer) and
  survive regeneration.

**Implementation:** `src/mvl/transpiler/emit_functions.rs`

#### Scenario: test fn is emitted under #[cfg(test)]

- GIVEN a module with `test fn check_add() -> Unit { }`
- WHEN transpiled to Rust
- THEN the output contains `#[cfg(test)]`, `mod tests {`, and `#[test]`

**Tests:** `tests/transpiler.rs::test_fn_emits_cfg_test_block`

#### Scenario: normal fn is not marked #[test]

- GIVEN a module with `fn add(a: Int, b: Int) -> Int { a + b }`
- WHEN transpiled to Rust
- THEN the output does NOT contain `#[cfg(test)]`

**Tests:** `tests/transpiler.rs::no_test_fns_no_cfg_test_block`

---

### Requirement 2: External Test Files [MUST]

External tests live in `*_test.mvl` files alongside the module under test.
They MUST only access the public API (Evidence layer — E).
`mvl test <dir>` discovers all `*_test.mvl` files and runs `cargo test`.

External tests are permanent evidence. They MUST NOT be regenerated when the
implementation is regenerated from the spec (Article 4 of ISPE).

**Implementation:** `src/main.rs`

#### Scenario: mvl test finds _test.mvl files

- GIVEN a directory containing `foo.mvl` and `foo_test.mvl`
- WHEN `mvl test <dir>` is run
- THEN `foo_test.mvl` is compiled and executed via `cargo test`

**Tests:** `tests/integration/`

---

### Requirement 3: Assurance Report Includes Test Count [MUST]

`mvl assurance <file|dir>` MUST report the number of `test fn` declarations
found, as part of the per-module assurance evidence.

**Implementation:** `src/main.rs`

#### Scenario: assurance report shows test fn count

- GIVEN a module with two `test fn` declarations
- WHEN `mvl assurance` is run on the module
- THEN the report includes `test fn: 2`

**Tests:** `tests/integration/`

---

### Requirement 4: Assertion Style [SHOULD]

MVL tests SHOULD use `assert_eq`, `assert_ne`, and `assert` from the standard
library. Property-based testing using `forall` is a future extension (MAY).

**Implementation:** `src/main.rs`

#### Scenario: assert_eq in test body compiles

- GIVEN `test fn check_value() -> Unit { assert_eq(1 + 1, 2) }`
- WHEN transpiled and compiled via `cargo test`
- THEN the test passes

**Tests:** `tests/transpiler.rs`

---

### Requirement 5: BDD Naming Convention [MAY]

MVL supports BDD-style integration tests via naming conventions — no new keywords.
The convention (ADR-0020) uses three function prefixes and a context struct to thread
state between steps. This mirrors Python's native-language BDD approach (as opposed
to Cucumber feature files).

```
given_*  →  setup function, returns a context struct
when_*   →  pure transform on the context, returns updated context
then_*   →  assertion function on the context, returns Unit
test fn scenario_*  →  chains the above; is the test entry point
```

```mvl
type CalcCtx = struct {
    a: Int,
    b: Int,
    result: Int,
}

fn given_two_numbers(a: Int, b: Int) -> CalcCtx {
    CalcCtx { a: a, b: b, result: 0 }
}

fn when_added(ctx: CalcCtx) -> CalcCtx {
    CalcCtx { a: ctx.a, b: ctx.b, result: add(ctx.a, ctx.b) }
}

fn then_result_equals(ctx: CalcCtx, expected: Int) -> Unit {
    assert_eq(ctx.result, expected)
}

test fn scenario_adding_two_numbers() -> Unit {
    let ctx = given_two_numbers(2, 3)
    let ctx = when_added(ctx)
    then_result_equals(ctx, 5)
}
```

**Key properties:**
- Zero new language features — `given`, `when`, `then` are identifiers, not keywords
- State flows explicitly through a context struct (consistent with MVL ownership model)
- `scenario_*` names map 1:1 to `Scenario:` entries in `.openspec/specs/`
- `mvl test --bdd` MAY emit Gherkin-style reports derived from function names

**Implementation:** `src/main.rs` (test runner BDD report)

#### Scenario: BDD scenario chains given/when/then

- GIVEN a `_test.mvl` file with `given_*`, `when_*`, `then_*`, and `test fn scenario_*`
- WHEN transpiled and run via `mvl test`
- THEN all scenario tests pass and the runner identifies them as scenarios

**Tests:** `tests/corpus/12_bdd/`
