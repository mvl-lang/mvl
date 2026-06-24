---
domain: language
version: 0.3.0
status: draft
date: 2026-06-24
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

**Syntax:** `test fn <name>(<params>) -> <return_type> [! <effects>] { <body> }`

- `test` is a keyword prefix (like `total` / `partial`).
- Test functions MUST NOT be `pub` — they are development artefacts.
- Test functions MAY be combined with `total` / `partial`: `test total fn …`.
- Test functions MAY declare effects: `test fn name() -> Unit ! Spawn + Send { … }`.
  This is required to test actor-backed code (see Requirement 6).
- Regeneration (Article 4): internal tests are regenerated with the module.
  External tests (`_test.mvl` files) are permanent evidence (E layer) and
  survive regeneration.

**Implementation:** `src/mvl/backends/rust/emit_functions.rs`

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
    assert_eq(ctx.result, expected);
}

test fn scenario_adding_two_numbers() -> Unit {
    let ctx: CalcCtx = given_two_numbers(2, 3);
    let ctx: CalcCtx = when_added(ctx);
    then_result_equals(ctx, 5);
}
```

**Key properties:**
- Zero new language features — `given`, `when`, `then` are identifiers, not keywords
- State flows explicitly through a context struct (consistent with MVL ownership model)
- `scenario_*` names map 1:1 to `Scenario:` entries in `.openspec/specs/`
- `mvl test --bdd` emits a Gherkin-style `BDD scenarios:` report derived from `scenario_*` function names

**Implementation:** `src/main.rs::cmd_test`

#### Scenario: BDD scenario chains given/when/then

- GIVEN a `_test.mvl` file with `given_*`, `when_*`, `then_*`, and `test fn scenario_*`
- WHEN compiled and run via `mvl test`
- THEN all `test fn scenario_*` functions pass

#### Scenario: --bdd flag emits Gherkin-style report

- GIVEN a `_test.mvl` file with `test fn scenario_*` functions
- WHEN run via `mvl test --bdd`
- THEN a `BDD scenarios:` block is printed listing each scenario as `Scenario: <name> ... ok`

**Tests:** `tests/compile_and_run.rs::bdd_scenarios_run_and_pass`, `tests/compile_and_run.rs::bdd_report_emits_gherkin_scenarios`

---

### Requirement 6: Effect Annotations on Test Functions [MUST]

`test fn` declarations MUST accept effect annotations so that effectful code —
in particular actor-backed libraries — can be unit-tested.

```mvl
// Spawn an actor
test fn new_metrics_spawns() -> Unit ! Spawn {
    let _m: Metrics = new_metrics();
    assert(true)
}

// Send behaviors (fire-and-forget)
test fn histogram_record_ms_sends() -> Unit ! Spawn + Send {
    let m: Metrics = new_metrics();
    histogram_record_ms(m, "latency_ms", 42, Map::new());
    assert(true)
}
```

**What is testable with `! Spawn + Send`:**

| Operation | Effect needed | What you can assert |
|-----------|--------------|---------------------|
| Spawn actor | `! Spawn` | No panic; handle is valid |
| Send a behavior | `! Send` | No panic; message enqueued |
| Both `None` and `Some` arms of actor-internal match | `! Spawn + Send` | Send to new key (None arm), then same key again (Some arm) |

**What requires `pub test fn` (#1500):**

Actor behaviors are fire-and-forget — `pub fn` on an actor returns `Unit`, so
internal state cannot be read back. Asserting _that_ a counter was incremented
or a histogram was updated requires `pub test fn` (#1506), a future feature that
exposes synchronous state access on the actor thread for the duration of a test.

**Two-call pattern for covering both match arms:**

When an actor initialises state on first use (a `match self.map.get(key)` with
`None =>` create and `Some(h) =>` update), covering both branches requires two
sends to the same key:

```mvl
test fn covers_both_arms() -> Unit ! Spawn + Send {
    let m: Metrics = new_metrics();
    histogram_record_ms(m, "req_ms", 10, Map::new());  // None arm: fresh key
    histogram_record_ms(m, "req_ms", 20, Map::new());  // Some arm: existing key
    assert(true)
}
```

**Implementation:** `src/mvl/parser/ast.rs` (FnDecl.effects), `src/mvl/checker/effects.rs`,
`src/mvl/backends/rust/emit_functions.rs`

#### Scenario: test fn with ! Spawn compiles and runs

- GIVEN `test fn spawn_test() -> Unit ! Spawn { let _: Counter = actor Counter { count: 0 }; assert(true) }`
- WHEN compiled and run via `mvl test`
- THEN the test passes

**Tests:** `tests/corpus/12_actors/actor_test_fn.mvl`

#### Scenario: test fn without effect annotation cannot call Spawn functions

- GIVEN a `test fn` without `! Spawn` that calls a function returning `T ! Spawn`
- WHEN type-checked
- THEN a missing-effect error is reported

---

### Requirement 7: Testing Standard Library [SHOULD]

A `std/testing.mvl` module SHOULD provide helpers beyond the three core
assertions (`assert`, `assert_eq`, `assert_ne`) for common test patterns.
Tracked in #1505.

Until `std/testing.mvl` exists, tests rely on `assert_eq` / `assert` from
`std/core.mvl` (always in scope) and inline helpers.

**Implementation:** `std/testing.mvl` (planned)
