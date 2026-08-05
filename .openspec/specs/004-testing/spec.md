---
domain: language
version: 0.5.0
status: draft
date: 2026-08-05
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

**What requires `pub test fn` (now available — see Req 8):**

Actor behaviors are fire-and-forget — `pub fn` on an actor returns `Unit`, so
internal state cannot be read back with `! Spawn + Send` alone. `pub test fn`
(Req 8, #1506) provides synchronous state access and is now implemented.

**Two-call pattern for covering both match arms (still useful without pub test fn):**

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

### Requirement 7: Testing Standard Library [MUST]

`std/testing.mvl` provides assertion helpers beyond the three core builtins
(`assert`, `assert_eq`, `assert_ne`) from `std/core.mvl`.

```mvl
use std.testing.{assert_contains, assert_len, assert_empty, assert_some, assert_none}

test fn example() -> Unit {
    assert_contains("hello world", "world");
    assert_len([1, 2, 3], 3);
    assert_some(Some(42));
    assert_none(None as Option[Int]);
}
```

| Function | Signature | Asserts |
|----------|-----------|---------|
| `assert_contains` | `(String, String) -> Unit` | Substring present |
| `assert_len[T]` | `(List[T], Int) -> Unit` | List length equals expected |
| `assert_empty[T]` | `(List[T]) -> Unit` | List is empty |
| `assert_some[T]` | `(Option[T]) -> Unit` | Option is `Some(_)` |
| `assert_none[T]` | `(Option[T]) -> Unit` | Option is `None` |

**Implementation:** `std/testing.mvl`

**Tests:** `tests/stdlib/testing_test.mvl`

---

### Requirement 8: pub test fn — Synchronous Actor State Assertions [MUST]

`pub test fn` declarations inside actor bodies run synchronously on the actor
thread and return a value. They are only callable from `#[cfg(test)]` contexts
and are stripped from production builds.

```mvl
actor Counter {
    count: Int

    pub fn increment(val n: Int) { self.count = self.count + n }

    pub test fn get_count() -> Int { self.count }
}

test fn state_is_correct() -> Unit ! Spawn + Send {
    let c: Counter = actor Counter { count: 0 };
    c.increment(5);
    c.increment(3);
    assert_eq(c.get_count(), 8)   // synchronous; sees state after both sends
}
```

**Key properties:**

- `pub test fn` may return any type (unlike `pub fn`, which must return `Unit`)
- FIFO mailbox ordering guarantees causal consistency: all prior `pub fn` sends
  are processed before the `pub test fn` call executes
- Emitted as `#[cfg(test)]` mailbox variant + `std::sync::mpsc` reply channel;
  no overhead in production builds
- Parameters follow the same rules as `pub fn` (sendable types)

**Generated Rust pattern (request-reply over mailbox):**

```rust
// Mailbox variant (cfg(test) only)
#[cfg(test)]
_TestGetCount { _reply: std::sync::mpsc::Sender<i64> },

// Handle method (cfg(test) only) — blocks until actor replies
#[cfg(test)]
pub fn get_count(&self) -> i64 {
    let (_tx, _rx) = std::sync::mpsc::channel();
    self._sender.send(CounterMailbox::_TestGetCount { _reply: _tx });
    _rx.recv().expect("actor thread died")
}
```

**Implementation:** `src/mvl/parser/actors.rs`, `src/mvl/parser/ast.rs`,
`src/mvl/ir.rs`, `src/mvl/ir/lower.rs`, `src/mvl/checker/decls.rs`,
`src/mvl/backends/rust/emit_actors.rs`

#### Scenario: pub test fn returns initial actor state

- GIVEN an actor with `pub test fn get_count() -> Int { self.count }`
- WHEN spawned with `count: 0` and `get_count()` called
- THEN returns `0`

#### Scenario: pub test fn sees state after async sends

- GIVEN a Counter actor with `pub fn increment(val n: Int)` and `pub test fn get_count() -> Int`
- WHEN `increment(5)` and `increment(3)` are sent, then `get_count()` is called
- THEN returns `8` (FIFO ordering guarantees causal consistency)

#### Scenario: pub fn with non-Unit return is still rejected

- GIVEN `pub fn get_x() -> Int` on an actor (no `test` keyword)
- WHEN type-checked
- THEN `NonUnitBehaviorReturn` error is emitted

**Tests:** `tests/corpus/12_actors/actor_pub_test_fn.mvl`,
`tests/stdlib/actors_test.mvl::pub_test_fn_*`,
`tests/transpiler.rs::actor_pub_test_fn_emits_cfg_test_infrastructure`,
`tests/transpiler.rs::actor_pub_test_fn_with_params_emits_fields_in_variant`,
`tests/type_checker.rs::actor_pub_test_fn_non_unit_return_accepted`

---

### Requirement 9: Sibling Module Inclusion for `mvl test` [MUST]

`mvl test <file>` compiles the file under test together with every module
it transitively `use`-imports in the same directory (its "siblings"),
resolved via a fixed-point walk of `use` declarations
(`loader::load_sibling_modules_transitive`). Inclusion is **import-scoped**,
not directory-scoped: a `.mvl` file in the same directory that the file
under test does not import from is not pulled in.

A `test fn` MAY live inline in a non-`_test.mvl` file alongside its
production code (Requirement 1), including a file that also declares the
project's `fn main` (see Requirement 10 for what happens to that `fn main`
during test compilation). A standalone `_test.mvl` file (Requirement 2)
tests its corresponding module via the same import mechanism — nothing
distinguishes an inline `test fn`'s sibling from a standalone
`_test.mvl`'s sibling.

Each backend dispatches individual tests differently once compiled:

| Backend | Compilation shape | Per-test dispatch |
|---|---|---|
| Rust (default) | `src/lib.rs`, always a library crate | `#[test]` attribute + `cargo test` |
| LLVM (`--backend=llvm`) | one flat, single-namespace LLVM IR module | synthesized dispatch `main` reads `argv[1]`, calls the named test fn (`lli <module.ll> <test_name>`) |
| WASM (`--backend=wasm`) | one flat, single-namespace WAT module | `wasmtime run --invoke <test_name> <module.wasm>` calls the named export directly |

**Implementation:** `src/mvl/loader.rs::load_sibling_modules_transitive`,
`src/cli/llvm_text.rs::prepare_llvm_text_tir_multi`,
`src/cli/wasm_text.rs::compile_wat_multi`, `src/cli/test.rs`

#### Scenario: sibling import pulls in a needed module

- GIVEN `entry_test.mvl` with `use helper.{add}` and `helper.mvl` in the same directory
- WHEN `mvl test entry_test.mvl` is run on any backend
- THEN `helper.mvl` is compiled alongside `entry_test.mvl` and `add` is callable

#### Scenario: unrelated file in the same directory is not pulled in

- GIVEN `entry_test.mvl` (no `use` of `unrelated.mvl`) and `unrelated.mvl` in the same directory
- WHEN `mvl test entry_test.mvl` is run
- THEN `unrelated.mvl` is not compiled into the test crate, even though it sits in the same directory

**Tests:** `src/cli/llvm_text.rs::tests::multi_file_sibling_fn_appears_in_ir`,
`src/cli/llvm_text.rs::tests::test_crate_includes_sibling_fns`

---

### Requirement 10: Test-Crate `fn main` Exclusion — Backend Parity [MUST]

`fn main` is not special MVL syntax — it is an ordinary function that
happens to be the whole program's entry point. A file that declares `fn
main` MAY still be a legitimate sibling (Requirement 9) of some other file
under test, because MVL compiles whole modules, not individual functions:
importing any one function from a file necessarily includes that file's
`fn main` too, if it has one.

The LLVM and WASM backends emit one flat, single-namespace module per test
compilation (Requirement 9's table) and MUST NOT let an incidentally
included `fn main` — the entry file's own, or a sibling's — reach emission
in a test crate:

- LLVM MUST drop every `fn main` from the entry file and every sibling
  before emitting the test crate, and before synthesizing its own
  dispatch `main` (Requirement 9). Nothing in a test ever calls `main()`
  by name, so no test can be broken by its absence; keeping it would
  produce two `@main` definitions in the same module.
- WASM MUST drop every `fn main` from the entry file and every sibling
  before emission, and MUST exempt `main` specifically from the
  duplicate-free-function-name check that otherwise runs across
  entry+siblings before compilation. `wasmtime run --invoke <test_name>`
  never calls `main`/`_start`, so two files each declaring their own
  unrelated `fn main` is not a real naming conflict once both are
  dropped — a genuine duplicate of any *other* function name MUST still
  be rejected with the existing diagnostic.
- Rust needs no equivalent exclusion: `mvl test`'s Rust-backend path
  always compiles the test crate as a library crate (`src/lib.rs`), where
  `fn main` is an ordinary, module-scoped function name with no special
  meaning to `rustc`/`cargo test`. Multiple sibling files may each freely
  declare their own `fn main` with no collision.

This exclusion applies **only** to test-crate compilation. `mvl
build`/`mvl run` (a real program) and the `// expect:` corpus-style test
runner (which executes `main` and compares its stdout) MUST keep every
`fn main` exactly as declared — dropping it there would be a correctness
bug, not a fix.

**Implementation:** `src/cli/llvm_text.rs::prepare_llvm_text_tir_multi`
(`for_test_dispatch` parameter),
`src/mvl/backends/llvm_text/emitter.rs::compile_to_ir_test_crate_with_siblings`,
`src/mvl/backends/llvm_text/emit_program_tir.rs::emit_program_tir_test_crate`,
`src/cli/wasm_text.rs::compile_wat_multi`/`build_and_assemble`
(`for_test_dispatch` parameter)

**ADR:** [ADR-0063 — Test-crate `fn main` exclusion, LLVM/WASM parity](../../adr/0063-test-crate-main-exclusion-backend-parity.md)

#### Scenario: entry file declares both test fns and its own fn main

- GIVEN a file with `test fn foo() -> Unit { ... }` and `fn main() -> Unit ! Console { ... }` declared in the same file
- WHEN `mvl test <file> --backend=llvm` (or `--backend=wasm`) is run
- THEN the file's own `fn main` is dropped from the test crate, and `foo` runs and reports its result normally

#### Scenario: a sibling (not the entry file) declares fn main

- GIVEN `entry_test.mvl` (no `fn main`) with `use lib.{helper}`, and `lib.mvl` (a sibling) declaring both `pub fn helper()` and `fn main()`
- WHEN `mvl test entry_test.mvl --backend=llvm` (or `--backend=wasm`) is run
- THEN `lib.mvl`'s `fn main` is dropped from the test crate, `helper` remains callable, and no "duplicate"/"redefinition of main" error occurs

#### Scenario: entry file and a sibling each declare their own unrelated fn main

- GIVEN `entry.mvl` with its own `fn main()` and a `test fn`, and a sibling `lib.mvl` also declaring an unrelated `fn main()`
- WHEN `mvl test entry.mvl --backend=wasm` is run
- THEN WASM's duplicate-function-name check does NOT reject the build on account of `main` (both are dropped before emission), and the test fn runs normally

#### Scenario: real builds keep fn main untouched

- GIVEN a project's real entry file with `fn main() -> Unit ! Console { println("hi") }`
- WHEN `mvl build <file> --backend=llvm` (or `--backend=wasm`, or `mvl test <file> --backend=llvm` on an `// expect:`-annotated file) is run
- THEN `fn main` is preserved and actually executes — the test-crate exclusion in this requirement does not apply

**Tests:** `src/cli/llvm_text.rs::tests::test_crate_drops_sibling_fn_main`,
`src/cli/llvm_text.rs::tests::test_crate_drops_entry_and_sibling_fn_main`,
`src/cli/wasm_text.rs::test_dispatch_main_exclusion_tests::sibling_fn_main_is_dropped_for_test_dispatch`,
`src/cli/wasm_text.rs::test_dispatch_main_exclusion_tests::entry_and_sibling_fn_main_both_dropped_no_duplicate_error`
