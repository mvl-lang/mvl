# Pattern 006: No Test Shadows

## Summary

Test files (`*_test.mvl`) must exercise **production code** — not parallel
implementations of it. Every type, function, and enum variant a test refers
to must be reachable via `use module::Item;` from a sibling production file
or the standard library. **Any `type` or `fn` declared inside a `_test.mvl`
is a red flag.**

The guiding principle: **test code follows production, not the other way
around.** If production drifts, tests must be updated (or fail loudly).
Tests that redeclare their subject drift silently — and every drift bug
this project has caught started as a shadow that made the tests keep
passing while production changed underneath.

## The anti-patterns (all forbidden)

### 1. Redeclaration

The test file copies a type or function from production verbatim, then runs
tests against the copy. Any subsequent change to production is not reflected
in the copy. The tests pass; production is broken.

```mvl
// BAD — dosing_test.mvl duplicating dosing.mvl
type DrugOrder = struct { rate: Int, hours: Int }   // shadow of production

fn total_dose(w: Int, r: Int, h: Int) -> Int {      // shadow of production
    w * r * h
}

test fn total_dose_max() -> Unit {
    assert_eq(total_dose(200, 50, 24), 240000)      // tests the shadow, not production
}
```

Fix: use imports.

```mvl
// GOOD — dosing_test.mvl importing dosing.mvl
use dosing.{DrugOrder, total_dose}

test fn total_dose_max() -> Unit {
    assert_eq(total_dose(200, 50, 24), 240000)      // tests production
}
```

### 2. Ghost enum variants

The test file redeclares an enum with extra variants that no longer exist
in production, and writes tests for them.

```mvl
// BAD — production has RunError { IOFailure, PipelineFailed }
type RunError = enum { MissingArg, IOFailure, PipelineFailed }  // MissingArg is a ghost

test fn missing_arg_message() -> Unit {                          // tests a variant that doesn't exist
    assert_eq(run_error_message(RunError::MissingArg), "…")
}
```

Fix: delete the ghost. If the variant is genuinely needed, add it to
production first, then test it.

### 3. Effect-stripped shims

A production function carries `! Log` or `! CryptoRandom` and cannot be
called from a pure test. Someone re-declares it in the test file with the
effect stripped, and writes coverage tests against the shim.

```mvl
// BAD — production log_access carries ! Log
total fn log_access(…) -> Unit {                    // shim: production has ! Log
    ()                                              // body doesn't log; tests test nothing
}

test fn log_access_allow_branch_covered() -> Unit {
    log_access(…);
    assert_eq(1, 1)                                 // asserts literally nothing about production
}
```

Fix: **delete the shim and the shim-tests.** Effect-bearing functions are
tested via integration (`make run`), not by removing the effect and testing
the corpse. If the branch structure inside the function needs unit tests,
extract the pure branching into a `pub` helper on production and test that.

### 4. Phantom types and functions

The test file declares types/functions that never existed in production. The
example directory has a suggestive name (`csv_transactions/`) but production
has no matching domain code.

```mvl
// BAD — main.mvl has no Transaction type
type Transaction = struct { … }                     // phantom
fn encode_transaction(tx: Transaction) -> …         // phantom

test fn encode_produces_three_fields() -> Unit { … } // tests a fiction
```

Fix: either promote the phantom to real production code (add it to a real
module) or delete the phantom and its tests.

## When to reach for a sibling module

If a test needs to import something that lives inside `main.mvl` (the entry
point, not importable) or is currently declared without `pub`, the correct
fix is not a shim. It is either:

1. **Add `pub` to the production item.** This is the cheapest fix when the
   item is safe to expose (pure, no invariants that require encapsulation).
   Only three of the five clamp helpers in `flight_clearance/clearance.mvl`
   needed `pub` to unblock the entire `clearance_test.mvl` sweep.
2. **Extract the item to a sibling module.** When the production home is
   `main.mvl` (not importable) or when the API surface is growing, move the
   type + function into a new file (`errors.mvl`, `paths.mvl`, `security.mvl`
   are the extraction sites used across the corpus). `main.mvl` re-imports;
   the test imports directly.

## Historical cases

Each of these was caught by the `#96` fossil sweep on branch
`chore/exterminate-96-workaround` (PR #1899):

- **`flight_clearance/clearance_test.mvl`** — the test used
  `MaintenanceStatus::Cleared` while production had `Airworthy` (issue
  #1900). 19 test sites had been passing against a shadow enum for months.
  Fixed by aligning the test with production; 5 helpers made `pub`.
- **`log_analyzer/main_test.mvl`** — ghost `RunError::MissingArg` variant
  no longer in production. Dead test + shadow enum removed; `RunError` and
  its message helper extracted into `errors.mvl`.
- **`task_pipeline/main_test.mvl`** — same shape, plus a drifted signature
  (`threshold_or_default(Option[String])` in the test vs
  `Option[Float]` in production). Realigned.
- **`access_control/{audit,main}_test.mvl`** — effect-stripped `log_access`
  shim and four `assert_eq(1, 1)` shim-coverage tests deleted; pure
  helpers made `pub` or extracted to `security.mvl`.
- **`csv_transactions/main_test.mvl`** — phantom `Transaction` +
  `encode_transaction` + `decode_transaction` never existed in
  production. Deleted the phantoms and their 12 tests; kept the 5 tests
  that exercise real `std.csv` functions.

## Enforcement

The rule is enforceable mechanically. A CI check that scans every
`*_test.mvl` file for `^(type|fn|total fn|partial fn|pub …)` declarations
would fail the build whenever a shadow appears. Until that check exists,
reviewers watching for this pattern is the fallback — and this pattern
document is the reference. Every subagent editing test files should read
this document first.

## Related

- `chore/exterminate-96-workaround` branch — the sweep that surfaced
  every case above
- ADR-0050 (backend AST-import audit) — same shape: mechanical audit
  prevents an anti-pattern that reviewers keep missing
