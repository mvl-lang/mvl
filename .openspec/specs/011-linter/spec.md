---
domain: toolchain
version: 0.1.0
status: draft
date: 2026-05-02
---

# 011 — MVL Linter

The MVL linter performs static style, structural, and semantic checks on MVL source
files. It is designed to keep the corpus and examples consistent, catch common mistakes
early, and produce actionable diagnostics at three severity levels.

## Philosophy

MVL is a language intended to be **generated** as well as hand-written. Generated code
benefits from maximally explicit annotations — every type, every IFC label, every effect
spelled out. The linter therefore distinguishes between *correctness concerns* (warnings
and errors) and *style preferences* (hints). Explicit annotations are never wrong; they
are the preferred default for generated code.

**Semantic-first defaults:** Style rules (line length, trailing whitespace, indentation,
final newline, comment style) are **OFF by default**. They have zero semantic value —
a long line or trailing space does not affect program correctness. Semantic rules
(unreachable code, redundant match, redundant effects) and complexity rules (cyclomatic
complexity, function length) remain ON. This ensures that LLM-generated code that is
correct and compiler-verified produces no spurious lint warnings.

**Origin:** ADR-0017 — Linter Hint severity and explicit IFC annotations as preferred style.

---

## Requirements

### Requirement 1: Three-Level Severity [MUST]

The linter MUST support three diagnostic severity levels, ordered by severity:

```
Hint < Warning < Error
```

- **Hint** — a stylistic observation; both the flagged form and the alternative are valid.
  Hints appear in the output but do NOT count toward the warning total and do NOT fail
  `make mvl-lint`.
- **Warning** — something is likely wrong or suboptimal; SHOULD be resolved before shipping.
  Warnings DO count toward the warning total. `make mvl-lint` fails if any warning is present.
- **Error** — a definite problem; MUST be fixed. Errors block the build.

**Implementation:** `src/mvl/linter/errors.rs::Severity`

**Tests:** `tests/unit/linter/` (severity ordering, hint_count, warning_count, is_ok)

#### Scenario: Hints do not fail the lint gate

- GIVEN a file with only `hint`-severity findings
- WHEN `make mvl-lint` runs
- THEN the exit code is 0 and the summary reads "MVL lint: all clean."

#### Scenario: Warnings fail the lint gate

- GIVEN a file with at least one `warning`-severity finding
- WHEN `make mvl-lint` runs
- THEN the exit code is non-zero and the warning is printed

#### Scenario: Hint output format

- GIVEN a `hint`-severity diagnostic for rule `redundant-ifc-label` at line 7, col 23
- WHEN rendered
- THEN the output is `file.mvl:7:23: hint: [redundant-ifc-label] <message>`

---

### Requirement 2: Redundant IFC Label is a Hint [MUST]

The `redundant-ifc-label` rule MUST be emitted at **Hint** severity.

`Public[T]` is semantically equivalent to unannotated `T` (the implicit default is public),
but explicit annotation is the **preferred style** — especially in IFC-focused code and
generated programs where the security lattice should be visible without requiring knowledge
of implicit defaults.

The hint message MUST read:
> `` `Public<T>` is explicit — unannotated types are implicitly public. Consider dropping
> the label in non-IFC-focused code. ``

**Implementation:** `src/mvl/linter/rules.rs::redundant_ifc_labels` → `LintDiag::hint`

**ADR:** ADR-0017

**Tests:** `src/mvl/linter/rules.rs` (inline tests for `redundant_ifc_labels`)

#### Scenario: IFC corpus file retains Public[T] without lint failure

- GIVEN `tests/corpus/06_ifc/declassification.mvl` contains `-> Public[Token]`
- WHEN `make mvl-lint` runs
- THEN no warning is reported for that file; a hint MAY appear

#### Scenario: Explicit Public[T] is reported as hint, not warning

- GIVEN `fn f(x: Public[Int]) -> Int { x }`
- WHEN `redundant_ifc_labels` runs
- THEN exactly one diagnostic is emitted with severity `Hint` and rule `redundant-ifc-label`

---

### Requirement 3: LintResult Exposes Hint Count [MUST]

`LintResult` MUST provide a `hint_count() -> usize` method returning the number of
`Hint`-severity diagnostics.

`is_ok()` MUST return `true` when there are no `Error`-severity diagnostics (hints and
warnings do not affect `is_ok`).

`warning_count()` MUST count only `Warning`-severity diagnostics, not hints.

**Implementation:** `src/mvl/linter.rs::LintResult`

**Tests:** `src/mvl/linter.rs` (inline tests for `hint_count`, `warning_count`, `is_ok`)

#### Scenario: Hint-only result is ok

- GIVEN a `LintResult` with 3 hints and 0 warnings and 0 errors
- WHEN `is_ok()` is called
- THEN it returns `true`
- AND `hint_count()` returns `3`
- AND `warning_count()` returns `0`

---

### Requirement 4: Missing Annotations [SHOULD]

The `missing-annotation` rule SHOULD warn (at **Warning** severity) when a function body
contains calls but no effect annotation is declared. This is the inverse complement of
`redundant-effects`: where that rule flags declared effects with no calls, this rule
flags calls with no declared effects.

The rule embodies MVL's "Explicit over implicit" principle (#428). It is **opt-in**
(`missing_annotations = false` by default) because the linter cannot distinguish calls
to pure MVL helpers from calls to effectful stdlib functions without a full symbol table.
Enable with `missing_annotations = true` in `.mvllintrc` for code bases that enforce
explicit-everywhere annotation density.

`test fn` declarations are excluded — test bodies call the system under test and need
not declare effects themselves.

**Implementation:** `src/mvl/linter/rules.rs::missing_annotations` → `LintDiag::warning`

**Config field:** `LintConfig::missing_annotations` (default: `false`)

**ADR:** ADR-0017 (amendment, #428)

**Tests:** `src/mvl/linter/rules.rs` (inline tests for `missing_annotations`)

#### Scenario: Rule fires when enabled and effects are missing

- GIVEN `missing_annotations = true` in config
- AND a function `fn foo() -> Unit { bar() }` with no declared effects
- WHEN `missing_annotations` runs
- THEN exactly one diagnostic is emitted with severity `Warning` and rule `missing-annotation`

#### Scenario: Rule is silent by default

- GIVEN default `LintConfig` (missing_annotations = false)
- AND a function with calls but no effects
- WHEN `missing_annotations` runs
- THEN no `missing-annotation` diagnostic is emitted

#### Scenario: No warning when effects are declared

- GIVEN `missing_annotations = true`
- AND `fn foo() -> Unit ! Console { bar() }`
- WHEN `missing_annotations` runs
- THEN no `missing-annotation` diagnostic is emitted

#### Scenario: No warning on call-free function

- GIVEN `missing_annotations = true`
- AND `fn add(x: Int, y: Int) -> Int { x + y }` (no calls, only arithmetic)
- WHEN `missing_annotations` runs
- THEN no `missing-annotation` diagnostic is emitted

**Historical note:** The predecessor rule `unnecessary-annotation` was removed in
v0.66.1 (#408) when `let` bindings became parser-enforced to require explicit types.
`missing-annotation` is its directional inverse — warning on absent annotations
rather than present ones.

---

### Requirement 5: Corpus and Examples Are Warning-Free [MUST]

All files under `tests/corpus/` and `examples/` MUST produce zero `Warning`-severity
or `Error`-severity diagnostics when linted with the default configuration.

Hint-severity findings are permitted.

The `corpus/04_linting/complexity_demo.mvl` file is excluded: it intentionally
demonstrates complexity rule violations and is permitted to carry warnings.

**Implementation:** `Makefile::mvl-lint`

**Tests:** `Makefile::mvl-lint` (CI gate), `tests/corpus/03_linting/`

#### Scenario: Clean corpus

- GIVEN all corpus and example files in their committed state
- WHEN `make mvl-lint` runs
- THEN the output is "MVL lint: all clean." and exit code is 0

---

### Requirement 6: Style Rules Are OFF by Default [MUST]

The following rules MUST be disabled in `LintConfig::default()`:

| Rule                   | Config key                  | Default |
|------------------------|-----------------------------|---------|
| Line length            | `line_length`               | `0` (disabled) |
| Trailing whitespace    | `trailing_ws`               | `false` |
| Indentation checks     | `indentation`               | `false` |
| Final newline          | `final_newline`             | `false` |
| Block comment style    | `consistent_comment_style`  | `false` |

Semantic rules (`unreachable_code`, `redundant_match`, `redundant_effects`,
`redundant_ifc_labels`) and complexity rules (`max_fn_length`,
`max_cyclomatic_complexity`, etc.) MUST remain ON by default.

**Implementation:** `src/mvl/linter/config.rs::LintConfig::default`

**Tests:** `src/mvl/linter/config.rs::default_config_has_expected_values`

#### Scenario: Fresh lint produces no style warnings

- GIVEN any syntactically valid MVL file
- AND no `.mvllintrc` present
- WHEN `mvl lint` runs
- THEN zero warnings are emitted due to line length, whitespace, indentation,
  final newline, or comment style

---

### Requirement 7: Style Master Toggle [MUST]

The linter MUST support a `style` key in `.mvllintrc` that enables all style rules
with standard values. Individual keys override the toggle regardless of file order.

Config parsing order:
1. Start with hardcoded defaults (all style rules OFF)
2. If `style = true` is present, enable all style rules with standard values
3. Apply individual key overrides

**Implementation:** `src/mvl/linter/config.rs::load_from`

**Tests:** `src/mvl/linter/config.rs::style_toggle_enables_all_style_rules`

#### Scenario: style = true enables all style rules

- GIVEN `.mvllintrc` containing `style = true`
- WHEN `LintConfig::load` runs
- THEN `line_length = 120`, `trailing_ws = true`, `indentation = true`,
  `final_newline = true`, `consistent_comment_style = true`

#### Scenario: Individual key overrides style toggle

- GIVEN `.mvllintrc` containing `style = true` and `line_length = 80`
- WHEN `LintConfig::load` runs
- THEN `line_length = 80` (individual override wins)
- AND all other style rules are enabled by the toggle

#### Scenario: Individual key wins regardless of file order

- GIVEN `.mvllintrc` containing `line_length = 60` on line 1 and `style = true` on line 2
- WHEN `LintConfig::load` runs
- THEN `line_length = 60` (earlier individual key still wins over later style toggle)
