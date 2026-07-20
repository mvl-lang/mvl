---
domain: toolchain
version: 0.2.0
status: active
date: 2026-07-20
---

# 026 — Contract Hardening (`mvl harden`)

`mvl harden` uses the compiler's own proof machinery to tell developers whether
their contracts are *tight* and *complete*. Where `mvl prove` reports whether
obligations hold, `harden` reports whether they could be strengthened, promoted
from runtime to static, or exercised by generated test inputs.

The command has four axes, each backed by an existing solver capability:

| Axis | Role | Backing solver work |
|------|------|---------------------|
| 1 | Runtime → static promotion diagnosis | `RefinementCounts.sites` from `check_call_site` |
| 2 | Contract tightening (`ensures` bounds) | `try_z3_tighten` in `layer5.rs` |
| 3 | Boundary test generation | `try_z3_witness` in `layer5.rs` |
| 4 | MC/DC gap synthesis | `try_z3_witness` + MC/DC analysis pass |

Related: ADR-0025 (function contracts), spec 010-mcdc, spec 018-refinement-solver.

---

### Requirement 1: Command Interface [MUST]

`mvl harden <file|dir>` MUST analyse every `.mvl` source file under the given
path (excluding `*_test.mvl`) and print a hardening report covering all
enabled axes.

```
mvl harden <file|dir>                — run all axes, human report
mvl harden <file|dir> --verbose      — include per-site predicate text
mvl harden <file|dir> --json         — machine-readable output
mvl harden <file|dir> --emit-tests   — write generated tests to *_boundary_test.mvl / *_mcdc_gap_test.mvl
mvl harden <file|dir> --callee <fn>  — restrict axis-1 analysis to a specific callee
mvl harden <file|dir> --mcdc         — enable axis 4 (MC/DC gap synthesis)
```

The command MUST exit 0 when no obligation failed, and exit 1 when any
`ProofOutcome::Failed` site was encountered — matching `mvl prove` exit
semantics.

**Implementation:** `src/cli.rs::dispatch` (harden case), `src/cli/harden.rs::run`

#### Scenario: no contracts

- GIVEN a file with no `requires`/`ensures` clauses
- WHEN `mvl harden <file>` is run
- THEN the file is skipped in the report and exit code is 0

#### Scenario: failed obligation exits 1

- GIVEN a file with an obligation that `mvl prove` reports as `Failed`
- WHEN `mvl harden <file>` is run
- THEN the process exits 1 after printing the report

**Tests:** `tests/corpus/12_dsl/`, `src/cli/harden.rs` (module tests)

---

### Requirement 2: Axis 1 — Runtime → Static Promotion [MUST]

For each call-site refinement obligation whose `ProofOutcome` is
`RuntimeCheck` or `RuntimeCheckWithWitness`, `harden` MUST report:

- Source location (file, line)
- Caller function, callee function, parameter name
- The predicate text
- A heuristic **hint** classifying why static promotion failed

Hint classification is a pure function of the predicate text (see
`HardenHint::classify`). The classification set is closed:

| Hint | Trigger | Suggestion |
|------|---------|-----------|
| `QuantifiedPredicate` | contains `forall` or `exists` | introduce a refined wrapper type |
| `OldPredicate` | contains `old(` | enable `--refinement-solver=z3-only` |
| `LengthPredicate` | contains `len(` | add a `where len(self) > N` refinement |
| `NonlinearPredicate` | contains `*` or `/` (outside `len(`/`old(`) | factor into linear steps or introduce a refined intermediate type |
| `Complex` | none of the above | add a proof anchor assertion before the call |

Axis 1 MUST NOT invoke any solver — it consumes results already produced by
`check_call_site` during the normal type-check pass.

**Implementation:** `src/cli/harden.rs::HardenHint`, `src/cli/harden.rs::run`

#### Scenario: nonlinear predicate gets nonlinear hint

- GIVEN a runtime-check site whose predicate is `self * 2 <= max`
- WHEN axis 1 runs
- THEN the site is reported with hint `NonlinearPredicate`

#### Scenario: length predicate is not misclassified as nonlinear

- GIVEN a runtime-check site whose predicate is `len(self) > 0`
- WHEN axis 1 runs
- THEN the site is reported with hint `LengthPredicate`, not `NonlinearPredicate`

**Tests:** `src/cli/harden.rs::tests` (classification unit tests)

---

### Requirement 3: Axis 2 — Contract Tightening [MUST]

For each `ensures` clause whose bound has the form `result >= N`, `result > N`,
`result <= N`, or `result < N`, `harden` MUST invoke `try_z3_tighten` to find
the tightest bound Z3 can prove and report the delta.

When a function has multiple return branches, `harden` MUST deduplicate per
`(fn_name, declared_predicate)` and keep the globally-sound bound:
- Minimum of tighter bounds for lower-bound predicates (`>=`, `>`)
- Maximum of tighter bounds for upper-bound predicates (`<=`, `<`)

This ensures the reported tighter bound holds on **all** return branches, not
just one.

Only predicates involving `Int`-typed `result` are in scope for this axis.
Predicates over other types are silently skipped (they may still appear in
axis 4).

**Implementation:** `src/cli/harden.rs::deduplicate_tightenings`,
`src/mvl/checker/solver/layer5.rs::try_z3_tighten`

#### Scenario: two-branch fn keeps the weakest tighter bound

- GIVEN `fn f(x: Int) -> Int ensures result >= 0 { if x > 0 { 5 } else { 1 } }`
- WHEN axis 2 runs
- THEN the reported tighter bound is `result >= 1` (minimum across branches)

#### Scenario: no tightening opportunity

- GIVEN a function whose declared `ensures` is already the tightest provable bound
- WHEN axis 2 runs
- THEN no tightening candidate is emitted for that function

**Tests:** `src/cli/harden.rs::tests::deduplicate_tightenings_keeps_min_for_lower_bound`

---

### Requirement 4: Axis 3 — Boundary Test Generation [MUST]

For each tightening candidate found in axis 2, `harden` MUST invoke
`try_z3_witness` (via `synthesize_witness`) to generate a concrete parameter
assignment that reaches the return branch and satisfies the tighter
postcondition.

When `--emit-tests` is passed AND at least one witness was found, `harden`
MUST write a `<stem>_boundary_test.mvl` file next to the source containing:

- A header comment marking the file as generated
- `use <stem>::<fn>;` imports for every witnessed function
- One `test fn harden_boundary_<fn>()` block per witness, containing:
  - The parameter bindings from the Z3 model, as `let` statements with explicit types
  - A call to the target function with those parameters
  - An `assert_eq(<tighter postcondition>, true)`

The generated file MUST be a valid MVL test file — parsing and type-checking
it MUST succeed against the sibling production file.

Struct-typed parameters are supported: `try_z3_witness` builds `param__field`
Z3 variables for each struct field using the type map from
`build_struct_fields`, and `format_witness_value` renders the witness as a
struct constructor expression (`TypeName { field: value, ... }`).

**Implementation:** `src/cli/harden.rs::synthesize_test_fn`,
`src/cli/harden.rs::build_struct_fields`,
`src/mvl/checker/solver/layer5.rs::try_z3_witness`,
`src/mvl/checker/refinements.rs::synthesize_witness`

#### Scenario: Int parameter witness

- GIVEN `fn f(n: Int) -> Int ensures result >= 1 { n + 1 }` with tighter bound `result >= 2` on branch where `n >= 1`
- WHEN axis 3 runs
- THEN a witness `n = 1` (or any value satisfying the branch) is generated

#### Scenario: --emit-tests writes compilable test file

- GIVEN axis 3 produces at least one witness for `foo.mvl`
- WHEN `mvl harden foo.mvl --emit-tests` is run
- THEN `foo_boundary_test.mvl` exists and `mvl check foo_boundary_test.mvl` exits 0

#### Scenario: no witness on non-integer parameter

- GIVEN a tightening candidate whose parameter is `String`
- WHEN axis 3 runs
- THEN the report notes "No witness found" for that function and no test is emitted

**Tests:** `src/cli/harden.rs::tests::synthesize_test_fn_emits_valid_mvl`,
`tests/corpus/12_dsl/harden_boundary_smoke.mvl`

---

### Requirement 5: Axis 4 — MC/DC Gap Synthesis [MUST]

When `--mcdc` is passed, `harden` MUST enumerate every compound decision in
every non-test production function (using the same analysis pass as `mvl mcdc`,
see spec 010) and, for each clause C in each decision D, query Z3 for an
**independence pair** (t1, t2) satisfying the Unique-Cause MC/DC criterion:

```
∃ t1, t2 :
    preconditions(t1) ∧ preconditions(t2) ∧
    clause_C(t1) ≠ clause_C(t2) ∧
    (∀ other clauses O evaluated in both t1, t2: clause_O(t1) = clause_O(t2)) ∧
    decision_outcome(t1) ≠ decision_outcome(t2)
```

The query MUST NOT require prior `mvl mcdc` runtime observations — it is a
**one-shot** synthesis based on the static decision AST alone.

For each clause the outcome is one of:

| Z3 result | Semantics | Report |
|-----------|-----------|--------|
| **SAT** with model | Independence pair exists | Emit test pair |
| **UNSAT** | Clause is *coupled* (structurally impossible to independently vary) | Mark as coupled; recommend Masking MC/DC |
| **Unknown** / timeout | Solver could not decide within 1s | Report "no witness" |

Parameter types in scope for this axis:
- `Int` — direct Z3 Int variable
- `Bool` — encoded as Z3 Int in `{0, 1}`
- `String` — Z3 String variable; predicates supported are equality
  (`s == "lit"`, `s != "lit"`), `s.contains("lit")`, `s.starts_with("lit")`,
  `s.ends_with("lit")`
- Structs whose fields are `Int`/`Bool` — via `build_struct_fields` map

Other parameter types (`Float`, `List`, `Map`) MUST be reported as
"unsupported clause type" and skipped. `Float` support depends on Z3 Real
theory being added to Layer 5 (tracked in #1957).

**Unique-Cause caveat for mixed decisions.** The "other clauses pinned to
their Q1 truth values" step uses a purely-integer structural evaluator.
String and struct clauses fall through as `None` and are silently skipped
from pinning. Decisions mixing Int/Bool + String clauses therefore land in
masking-MC/DC rather than strict Unique-Cause for their String clauses;
outcomes still differ correctly, but non-target String clauses may take
different values between t1 and t2.

**Decision kinds covered by axis 4:**
- Compound `if` conditions (statement and expression forms)
- Compound `while` conditions
- Compound match-arm guards (`n if a && b => …`) — guards using `RefExpr`
  are converted to `Expr` via `refexpr_to_expr` for the shared pipeline
- **Not** covered: match arms as independent outcomes (see #1958 —
  distinct semantics; requires `SingleWitness` outcome and pattern-to-predicate
  encoding), guards that reference pattern-bound identifiers

`requires` clauses on the enclosing function MUST be added as preconditions
to both t1 and t2 — reusing the branch-hypothesis threading pattern from
`try_z3_witness` (see spec 018-refinement-solver).

When `--emit-tests` is passed AND at least one independence pair was
generated, `harden` MUST write a `<stem>_mcdc_gap_test.mvl` file next to
the source, structured identically to the axis-3 boundary test file. Each
independence pair MUST produce two `test fn` blocks:

```mvl
// MC/DC independence pair for <fn>@line: clause <i> = <name>
test fn harden_mcdc_<fn>_c<i>_t() -> Unit { ... clause evaluates true ... }
test fn harden_mcdc_<fn>_c<i>_f() -> Unit { ... clause evaluates false ... }
```

**Implementation:** `src/cli/harden.rs::axis4_mcdc_gaps` (to be added),
`src/mvl/passes/mcdc/analysis.rs::analyze_mcdc`,
`src/mvl/checker/solver/layer5.rs::try_z3_witness`

#### Scenario: two-clause conjunction generates independence pair per clause

- GIVEN `fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }`
- WHEN `mvl harden f.mvl --mcdc` is run
- THEN two independence pairs are generated: one varying `a` (with `b = true`), one varying `b` (with `a = true`)

#### Scenario: coupled clauses report UNSAT

- GIVEN a decision `if a && a { … }` (or any structurally coupled expression)
- WHEN `mvl harden --mcdc` is run
- THEN the clause is reported as "coupled — masking MC/DC required" and no test pair is emitted

#### Scenario: requires clause is honored as precondition

- GIVEN `fn f(x: Int, y: Int) -> Int requires x > 0 { if x > y && y >= 0 { 1 } else { 0 } }`
- WHEN axis 4 runs
- THEN every generated witness satisfies `x > 0` (the requires clause)

#### Scenario: String clause type generates independence pair

- GIVEN `fn route(path: String, admin: Bool) -> Int { if path.starts_with("/api/") && admin { 1 } else { 0 } }`
- WHEN `mvl harden route.mvl --mcdc` is run
- THEN clause 0 (`path.starts_with("/api/")`) yields a pair where t1's `path` matches the prefix and t2's does not
- AND clause 1 (`admin`) yields a pair where t1's `admin = true` and t2's `admin = false`

#### Scenario: match-arm compound guard generates independence pair

- GIVEN `fn f(a: Bool, b: Bool, x: Int) -> Int { match x { n if a && b => n, _ => 0 } }`
- WHEN `mvl harden f.mvl --mcdc` is run
- THEN one MatchGuard decision is emitted with two independence pairs (one per clause)

#### Scenario: unsupported parameter type is skipped

- GIVEN a decision whose clauses reference a `Float` parameter
- WHEN axis 4 runs
- THEN the decision is reported with "unsupported clause type" and no witness is attempted (until #1957 lands)

#### Scenario: --emit-tests writes compilable MC/DC gap test file

- GIVEN axis 4 produces at least one independence pair for `foo.mvl`
- WHEN `mvl harden foo.mvl --mcdc --emit-tests` is run
- THEN `foo_mcdc_gap_test.mvl` exists and `mvl check foo_mcdc_gap_test.mvl` exits 0

**Tests:** `src/cli/harden.rs::tests::axis4_two_clause_conjunction`,
`src/cli/harden.rs::tests::axis4_coupled_clause_reports_unsat`,
`src/cli/harden.rs::tests::axis4_requires_threaded_as_precondition`,
`tests/corpus/12_dsl/harden_mcdc_smoke.mvl`

---

### Requirement 6: Report Output [MUST]

The default text report MUST be organised per input file, with one section
per axis, followed by a per-file summary line and (for multi-file inputs) a
grand total.

```
══════════════════════════════════════════════════════════════════════
  HARDEN REPORT: <file>
══════════════════════════════════════════════════════════════════════

── Axis 1: Runtime → Static Promotion ──────────────────────────────
  [01] <caller>:<line>  →  <callee>(<param>)
       hint: <suggestion>

── Axis 2: Contract Tightening ──────────────────────────────────────
  [01] <fn>:<line>
       declared: ensures <declared>
       provable: ensures <tighter>
       → Suggest strengthening the postcondition

── Axis 3: Boundary Test Generation ─────────────────────────────────
  Witness for <fn>:
    <param> = <value>

── Axis 4: MC/DC Gap Synthesis ──────────────────────────────────────    (only with --mcdc)
  Decision <fn>:<line> (<N> clauses):
    clause 0 (<name>): pair generated
    clause 1 (<name>): coupled — masking MC/DC required

  Summary: <P> proven, <R> runtime obligations, <F> failed,
           <T> tightening suggestion(s), <W> witness(es)[, <M> MC/DC pair(s)]
```

`--verbose` MUST additionally print predicate text for each axis-1 site.

`--json` MUST replace the human report with a single JSON object. All five
top-level array fields MUST be present (empty arrays when nothing to report):

```json
{
  "total_proven": N,
  "total_runtime": N,
  "total_failed": N,
  "axis1_promotion_candidates": [
    { "file": "...", "line": N, "caller": "...", "callee": "...", "param": "...", "predicate": "...", "suggestion": "..." }
  ],
  "axis2_tightening_candidates": [
    { "fn_name": "...", "line": N, "declared": "...", "tighter": "..." }
  ],
  "axis3_boundary_witnesses": [
    { "fn_name": "...", "line": N, "declared": "...", "tighter": "...",
      "args": [{ "name": "...", "type": "...", "value": "..." }] }
  ],
  "axis4_mcdc_pairs": [
    { "fn_name": "...", "line": N, "clause_idx": N, "clause_text": "...",
      "outcome": "pair",
      "t1": [{ "name": "...", "type": "...", "value": "..." }],
      "t2": [{ "name": "...", "type": "...", "value": "..." }] },
    { "fn_name": "...", "line": N, "clause_idx": N, "clause_text": "...",
      "outcome": "coupled" },
    { "fn_name": "...", "line": N, "clause_idx": N, "clause_text": "...",
      "outcome": "unsupported", "reason": "..." }
  ]
}
```

`axis4_mcdc_pairs[*].outcome` MUST be one of `"pair"`, `"coupled"`, or
`"unsupported"`. `t1`/`t2` MUST be present only for `"pair"` outcomes.
`reason` MUST be present only for `"unsupported"` outcomes.

Value strings (in `axis3_boundary_witnesses[*].args[*].value` and
`axis4_mcdc_pairs[*].{t1,t2}[*].value`) MUST be pre-rendered MVL literals —
`"true"`/`"false"` for Bool, decimal integers for Int, and quoted-and-escaped
strings for String. Consumers can paste them directly into generated code.

The JSON emitter MUST escape backslashes (`\\`) and double-quotes (`\"`) in
all string values.

**Implementation:** `src/cli/harden.rs::print_json`, `src/cli/harden.rs::compute_axis3_witnesses`, `src/cli/harden.rs::compute_axis4_results`

#### Scenario: JSON round-trip

- GIVEN any input file with axis 4 findings
- WHEN `mvl harden <file> --mcdc --json` is run
- THEN the output parses successfully as JSON and contains all five
  top-level array fields

---

### Requirement 7: Generated Test File Convention [MUST]

Test files emitted by `--emit-tests` MUST follow the rules in
`CLAUDE.md::Test files must import, not redeclare`:

- Zero `type` declarations
- Zero standalone `fn`/`total fn`/`partial fn` declarations
- Every referenced production symbol MUST be imported via `use <module>::<name>;`
- The file MUST parse and type-check successfully

The file name pattern is:

| Axis | Emitted file |
|------|-------------|
| 3 | `<stem>_boundary_test.mvl` |
| 4 | `<stem>_mcdc_gap_test.mvl` |

Each emitted file MUST begin with the marker comment:

```mvl
// Generated by `mvl harden --emit-tests` — do not edit by hand.
```

`harden` MUST NOT overwrite user-authored test files. If a target
`*_test.mvl` file exists but lacks the marker comment, `harden` MUST
refuse to write and emit a warning.

**Implementation:** `src/cli/harden.rs::run` (emit block)

#### Scenario: marker comment is present

- GIVEN axis 3 emits `foo_boundary_test.mvl`
- WHEN the file is read
- THEN the first line contains "Generated by `mvl harden --emit-tests`"

#### Scenario: does not overwrite user-authored tests

- GIVEN `foo_boundary_test.mvl` exists and does not contain the marker
- WHEN `mvl harden foo.mvl --emit-tests` is run
- THEN the file is not modified and a warning is printed

**Tests:** `src/cli/harden.rs::tests::emit_tests_refuses_to_overwrite_user_file`
