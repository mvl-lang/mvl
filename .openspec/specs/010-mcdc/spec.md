---
domain: toolchain
version: 0.1.0
status: draft
date: 2026-04-28
---

# 010 — MC/DC Coverage Analysis

`mvl mcdc` measures Modified Condition/Decision Coverage for MVL programs.
MC/DC is the most stringent structural coverage metric required by DO-178C at
DAL-A (catastrophic failure level), ISO 26262 at ASIL-D, and EN 50128 at SIL 4.

---

### Requirement 1: Command Interface [MUST]

`mvl mcdc <file|dir>` MUST analyse MC/DC coverage for all `*_test.mvl` files
(and source files containing inline `test fn` declarations) under the given path.

```
mvl mcdc <file|dir>             — run MC/DC coverage analysis
mvl mcdc <file|dir> -q          — quiet: print only the score line
mvl mcdc <file|dir> --verbose   — full covered/missed clause table
```

The command MUST exit 0 when all obligations are met, and exit 1 otherwise.
This enables direct use in CI pipelines as a hard quality gate.

**Implementation:** `src/main.rs::cmd_mcdc`

**Tests:** `tests/corpus/15_verification/`, `src/cli/mcdc.rs` (module tests)

#### Scenario: exit 0 on full coverage

- GIVEN a project whose tests provide independence pairs for every clause
- WHEN `mvl mcdc <dir>` is run
- THEN the process exits 0 and prints `PASS`

#### Scenario: exit 1 on incomplete coverage

- GIVEN a project where at least one clause has no independence pair
- WHEN `mvl mcdc <dir>` is run
- THEN the process exits 1 and prints `FAIL`

#### Scenario: no compound conditions

- GIVEN a project whose production functions contain only single-clause conditions
- WHEN `mvl mcdc <dir>` is run
- THEN the process exits 0 and prints "No compound boolean conditions found"

---

### Requirement 2: Static Obligation Analysis [MUST]

The analyser MUST walk the AST of every non-test production function and
identify all *compound decisions*: boolean expressions containing at least one
`&&` or `||` operator that appear in any of these positions:

- The condition of an `if` expression
- The condition of a `while` loop
- The return expression of a `Bool`-valued function body

For each compound decision the obligation table records:
- Sequential decision ID (zero-based, incrementing across all input files)
- Source file stem and line number
- Clause count N — the number of atomic boolean leaf nodes in the `&&`/`||` tree
- The decision kind: `if`, `while`, or `fn` (Bool return expression)

Single-clause conditions (`if x > 0 { … }`) are NOT MC/DC obligations: a
single condition trivially affects the outcome by definition.

Test functions (`test fn`) MUST be excluded from obligation analysis — MC/DC
applies only to production code.

**Implementation:** `src/mvl/backends/rust/mcdc_emit.rs::MCDCMap`

#### Scenario: compound AND is one obligation with two clauses

- GIVEN `fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }`
- WHEN MC/DC analysis runs
- THEN one decision is registered with clause_count = 2

#### Scenario: test fn is excluded

- GIVEN `test fn t(a: Bool, b: Bool) -> Bool { if a && b { true } else { false } }`
- WHEN MC/DC analysis runs
- THEN no decisions are registered

#### Scenario: start_id offsets IDs across files

- GIVEN two source files each with one compound decision
- WHEN processed in order
- THEN the second file's decision has id = 1 (not id = 0)

**Tests:** `src/mvl/backends/rust.rs::tests::mcdc_test_fn_excluded`,
`src/mvl/backends/rust.rs::tests::mcdc_start_id_offset_applied`,
`src/mvl/backends/rust.rs::tests::mcdc_bool_return_expr_instrumented`,
`src/mvl/backends/rust.rs::tests::mcdc_non_bool_return_not_instrumented`,
`tests/transpiler.rs::transpile_mcdc_skips_single_clause_condition`

---

### Requirement 3: Instrumented Transpilation [MUST]

The transpiler MUST inject per-clause observation capture for every compound
decision in non-test functions when invoked in MC/DC mode.

For a decision with N clauses at id D, the transpiler MUST emit:

```rust
// Arrays initialised false; only observed clauses are set (short-circuit semantics)
let mut __dD_c = [false; N];   // clause values
let mut __dD_e = [false; N];   // evaluation flags (true = clause was reached)

// Short-circuit evaluation tree — each leaf sets e[i]=true and c[i]=value
// only when that clause is actually evaluated by &&/|| logic
let __dD_outcome: bool = <sc-tree>;

// Record observation (only in test builds)
#[cfg(test)] crate::__mvl_mcdc::record(D, <encoded>);

// Execute original control flow
if __dD_outcome { … }
```

The short-circuit tree emitted for `A && B` is:
```rust
{ let __dD_t0 = { __dD_e[0] = true; __dD_c[0] = A; __dD_c[0] };
  if __dD_t0 { { __dD_e[1] = true; __dD_c[1] = B; __dD_c[1] } } else { false } }
```

The observation encoding is a `u32`:
- bits 0..N-1: clause values (bit i = 1 iff clause i was true)
- bits N..2N-1: eval flags (bit N+i = 1 iff clause i was evaluated)
- bit 2N: decision outcome (1 = true)

The clause count MUST NOT exceed 15. The transpiler MUST panic at
code-generation time if this limit is exceeded.

`while` conditions MUST be restructured as `loop { … if !outcome { break; } … body … }`
so clause arrays are re-evaluated on every iteration.

**Implementation:** `src/mvl/backends/rust/emit_stmts.rs::emit_mcdc_if`,
`src/mvl/backends/rust/emit_stmts.rs::emit_mcdc_while`,
`src/mvl/backends/rust/emit_stmts.rs::emit_mcdc_sc_outcome`,
`src/mvl/backends/rust/emit_stmts.rs::emit_mcdc_record`

#### Scenario: if with A && B emits clause arrays and record call

- GIVEN `fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }`
- WHEN transpiled with MC/DC instrumentation
- THEN emitted Rust contains `let mut __d0_c = [false; 2]`,
  `let mut __d0_e = [false; 2]`, `let __d0_outcome: bool =`,
  and `__mvl_mcdc::record(0usize,`

**Tests:** `src/mvl/backends/rust.rs::tests::mcdc_if_emits_clause_locals_and_record`,
`src/mvl/backends/rust.rs::tests::mcdc_record_encoding_present`,
`tests/transpiler.rs::transpile_mcdc_if_emits_clause_arrays_and_record`,
`tests/transpiler.rs::transpile_mcdc_decisions_metadata_correct`

#### Scenario: while with A && B is restructured as loop

- GIVEN `partial fn f(a: Bool, b: Bool) -> Int { while a && b { … } … }`
- WHEN transpiled with MC/DC instrumentation
- THEN emitted Rust contains `loop {` and `if !__d0_outcome { break; }`

**Tests:** `src/mvl/backends/rust.rs::tests::mcdc_while_restructured_as_loop`

#### Scenario: short-circuit tree sets eval flags per clause

- GIVEN `fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }`
- WHEN transpiled with MC/DC instrumentation
- THEN emitted Rust contains `__d0_e[0] = true` and `__d0_e[1] = true`
  inside the short-circuit tree

**Tests:** `src/mvl/backends/rust.rs::tests::mcdc_if_recomposed_uses_clause_vars`

---

### Requirement 4: Observation Collection Runtime [MUST]

The generated crate MUST include a `__mvl_mcdc` module (emitted once, at crate
top level) that collects observations thread-safely across parallel test runs.

```rust
#[cfg(test)]
pub mod __mvl_mcdc {
    static OBS: OnceLock<Mutex<Vec<HashSet<u16>>>> = OnceLock::new();
    pub fn record(decision_id: usize, encoded: u16) { … }
    pub fn get(decision_id: usize) -> Vec<u16> { … }
}
```

The `record` function MUST be idempotent for identical observations (a `HashSet`
deduplicates). This bounds the observation set size to `2^(N+1)` per decision,
preventing unbounded memory growth.

After all tests run, a generated `zzz_mvl_mcdc_report` test writes observations
to the file path in `MVL_MCDC_OUT`. The `zzz_` prefix ensures it sorts last in
cargo's alphabetic test ordering so all observations are captured before the
file is written.

**Implementation:** `src/mvl/backends/rust/mcdc_emit.rs::emit_mcdc_preamble`,
`src/mvl/backends/rust/mcdc_emit.rs::emit_mcdc_report_test`

**Tests:** `src/mvl/backends/rust/mcdc_emit.rs::tests::emit_preamble_has_record_fn`,
`src/mvl/backends/rust/mcdc_emit.rs::tests::emit_preamble_empty_when_zero`,
`src/mvl/backends/rust/mcdc_emit.rs::tests::emit_report_test_has_report_fn`

---

### Requirement 5: Independence Analysis — Unique-Cause MC/DC [MUST]

The independence check MUST implement **Unique-Cause MC/DC** as defined in
DO-178C / ED-12C Section 6.4.4.2(d):

> For each condition C in a decision D, there exist two test cases t1 and t2
> such that:
> 1. C differs between t1 and t2 (and both observations evaluated C)
> 2. All other conditions that were evaluated in both t1 and t2 are identical
> 3. The decision outcome differs between t1 and t2

The independence check MUST handle **masked clauses**: a clause not evaluated
in an observation (eval flag = 0) due to short-circuit semantics imposes NO
constraint on the independence pair — it is treated as "not reachable under
that input", not as a conflicting value.

Unique-Cause is chosen over Masking MC/DC because:
- The instrumentation faithfully models short-circuit execution; masked clauses
  are identified precisely via eval flags
- DO-178C DAL-A qualification evidence is cleaner with the stricter criterion
- The O(|obs|²) per-clause algorithm is practical because the `HashSet` bounds
  |obs| to `2^(N+1)`

**Implementation:** `src/mvl/backends/rust/mcdc_emit.rs::is_clause_covered`

#### Scenario: B independently toggles outcome in A && B

- GIVEN observations: (A=1,B=1,out=1) and (A=1,B=0,out=0)
- WHEN checking clause B (bit 1) for clause_count=2
- THEN is_clause_covered returns true

#### Scenario: simultaneous change does not count

- GIVEN observations: (A=1,B=1,out=1) and (A=0,B=0,out=0)
- WHEN checking any clause for clause_count=2
- THEN is_clause_covered returns false (both clauses changed)

#### Scenario: three-clause condition requires per-clause pairs

- GIVEN `A && B && C` with all three independence pairs present
- WHEN checking clauses 0, 1, 2 for clause_count=3
- THEN all three is_clause_covered calls return true

**Tests:** `src/mvl/backends/rust/mcdc_emit.rs::tests::independence_covered_and_b`,
`src/mvl/backends/rust/mcdc_emit.rs::tests::three_clause_all_covered`,
`src/mvl/backends/rust/mcdc_emit.rs::tests::independence_not_covered_when_other_clause_varies`

---

### Requirement 6: Report Output [MUST]

`mvl mcdc` MUST print:

**Default (neither `-q` nor `--verbose`):**
```
Found N test file(s), M compound decisions, K obligations
<cargo test output, minus zzz_mvl_mcdc_report line>

MC/DC coverage: covered/K obligations met (pct%)
PASS   (or FAIL)
```

**Quiet (`-q`):** suppress all output; exit code is the only signal.

**Verbose (`--verbose`):** add a per-decision table after the summary line:
```
DETAILED RESULTS
────────────────────────────────────────────────────
  foo:12   if    (2 clauses) [✓ ✓] COVERED
  foo:34   while (3 clauses) [✓ ✗ ✓] MISSED
────────────────────────────────────────────────────
```

Each row shows: file stem, line, kind (if/while), clause count, per-clause
pass/fail markers, and COVERED/MISSED status.

**Implementation:** `src/main.rs::cmd_mcdc`

**Tests:** `src/mvl/backends/rust/mcdc_emit.rs::tests::emit_report_test_has_report_fn` (verifies the generated `zzz_mvl_mcdc_report` test that drives the report path), and `tests/transpiler.rs` exercises the end-to-end transpile+run pipeline that feeds `cmd_mcdc`.
