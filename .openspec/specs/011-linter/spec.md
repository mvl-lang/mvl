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

**Implementation:** `src/mvl/linter/mod.rs::LintResult`

**Tests:** `src/mvl/linter/mod.rs` (inline tests for `hint_count`, `warning_count`, `is_ok`)

#### Scenario: Hint-only result is ok

- GIVEN a `LintResult` with 3 hints and 0 warnings and 0 errors
- WHEN `is_ok()` is called
- THEN it returns `true`
- AND `hint_count()` returns `3`
- AND `warning_count()` returns `0`

---

### Requirement 4: Unnecessary Type Annotations [REMOVED]

The `unnecessary-annotation` rule has been removed (#408). All `let` bindings now
**require** an explicit type annotation — a language-level invariant enforced at
parse time. With annotations mandatory, no annotation can be "unnecessary"; the rule
was contradictory and has been deleted along with its `LintConfig` field and helper
functions.

**Removed in:** v0.66.1 (#408)

---

### Requirement 5: Corpus and Examples Are Warning-Free [MUST]

All files under `tests/corpus/` and `examples/` MUST produce zero `Warning`-severity
or `Error`-severity diagnostics when linted with the default configuration.

Hint-severity findings are permitted.

The `corpus/04_linting/complexity_demo.mvl` file is excluded: it intentionally
demonstrates complexity rule violations and is permitted to carry warnings.

**Implementation:** `Makefile::mvl-lint`

#### Scenario: Clean corpus

- GIVEN all corpus and example files in their committed state
- WHEN `make mvl-lint` runs
- THEN the output is "MVL lint: all clean." and exit code is 0
