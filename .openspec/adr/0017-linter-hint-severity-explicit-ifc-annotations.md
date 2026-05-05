# ADR-0017: Linter Hint Severity — Explicit IFC Annotations as the Preferred Style

**Status:** Accepted
**Date:** 2026-05-02
**Issues:** #404

---

## Context

The MVL linter has two severity levels: `Warning` and `Error`. The `redundant-ifc-label` rule
was introduced to flag explicit `Public[T]` type annotations as redundant because unannotated
types are implicitly public in MVL's information-flow type system.

When resolving lint warnings in the corpus (#404), it became clear that blanket removal of
`Public[T]` annotations is wrong in IFC-focused code. In files like `declassification.mvl`,
`lattice.mvl`, and `propagation.mvl`, the explicit `Public[T]` annotation is not noise — it
is the point of the code. `fn declassify_token(secret: Secret[Token]) -> Public[Token]`
communicates the security lattice transition explicitly; stripping it to `-> Token` loses
the intent.

Additionally, MVL is a language designed to be **generated**, not primarily hand-written.
Generated code benefits from maximally explicit annotations: every type, every label, every
effect spelled out. Implicit defaults are convenient for humans writing by hand; they are
opaque to toolchains reading and transforming code.

The tension: the linter rule is technically correct (semantics are unchanged), but it
optimises for terseness over explicitness — the wrong default for a generated language.

---

## Decision

### 1. Add a `Hint` severity level to the linter

Extend `errors::Severity` with a third variant below `Warning`:

```
Hint < Warning < Error
```

- `Hint` — stylistic observations; valid alternatives exist; neither is wrong.
  The linter reports them (visible in output) but they do not count toward
  the warning total and do not fail `make mvl-lint`.
- `Warning` — something is likely wrong or should be changed before shipping.
- `Error` — must be fixed; blocks the build.

`LintResult::warning_count()` counts only `Warning` and above.
`LintResult::is_ok()` is unaffected by hints.

### 2. Downgrade `redundant-ifc-label` from `Warning` to `Hint`

The rule stays enabled by default but no longer fails the lint gate. Its message is
updated to reflect the recommendation framing:

> `Public<T>` is explicit — unannotated types are implicitly public.
> Consider dropping the label in non-IFC-focused code.

### 3. Establish the explicit-annotation principle in corpus style

IFC corpus files (`06_ifc/`, security-labelled fields in structs and function signatures)
**should** retain explicit `Public[T]` annotations. This makes the security lattice visible
to readers, generators, and analysis tools without requiring them to know the implicit
default rule.

Non-IFC corpus files (basic type demos, LLVM tests, etc.) **may** drop `Public[T]` if it
appears as incidental noise rather than deliberate annotation.

---

## Consequences

- `make mvl-lint` no longer fails on `redundant-ifc-label` findings; they appear in the
  output as `hint:` lines but the exit code is clean.
- All IFC corpus files revert to explicit `Public[T]` annotations.
- The linter output gains a third prefix: `hint:` alongside `warning:` and `error:`.
- `LintResult` gains a `hint_count()` method for tests and CI reporting.
- Future rules that are style preferences rather than correctness concerns should default
  to `Hint` severity (e.g., `unnecessary-annotation` for generated code contexts).

---

## Rejected Alternatives

**Suppression pragmas** (`// lint:allow(redundant-ifc-label)`) — adds noise at the use site
and requires every IFC file to opt out individually. Rejected: the rule framing was wrong,
not the corpus files.

**Remove the rule entirely** — loses the signal for hand-written code where `Public[T]` on
a plain integer parameter genuinely is redundant noise. Rejected: the rule has value, just
at the wrong severity.

**Keep `Warning`, update corpus** — removes explicit security annotations from IFC
demonstration files, making them less useful as teaching material and for code generation
pipelines. Rejected.

---

## Amendment: `missing-annotation` rule (#428)

**Added in:** v0.71.0 (#428)

The `missing-annotation` rule is the directional inverse of `redundant-ifc-label`: where
that rule hints when explicit `Public[T]` annotations are present (redundant but preferred),
`missing-annotation` warns when effect annotations are *absent* on functions that make calls.

This aligns with ADR-0017's stated principle — "explicit annotations are never wrong; they
are the preferred default" — applied to the effect dimension rather than the IFC dimension.

The rule is **disabled by default** (`missing_annotations = false` in `LintConfig`) because
the linter lacks a symbol table and cannot distinguish calls to pure MVL helpers from calls
to effectful stdlib functions. Enabling it opt-in allows teams that enforce
explicit-everywhere annotation density to use it without imposing the requirement on the
entire corpus.

**Spec:** Spec 011 Req 4

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

**Requirement 7 — Information flow control:** strengthens. Retaining explicit
`Public[T]` annotations in IFC corpus files preserves the security lattice as
visible, machine-readable information rather than relying on the implicit default.
Code generation pipelines and analysis tools benefit from annotations that are
spelled out.

Requirements 1–6 and 8–11 are not directly affected by this decision.

### Design Principles (README)

**Principle 1 — Explicit over implicit:** strengthens. The core decision demotes
`redundant-ifc-label` from Warning to Hint precisely to *preserve* explicit
annotations rather than silently elide them. Explicit `Public[T]` is preferred
over relying on the implicit public default.

**Principle 7 — Security labels on all data:** strengthens. IFC corpus files
retain their full label vocabulary (`Public[T]`, `Secret[T]`, `Tainted[T]`),
making the security lattice visible to readers and toolchains.

**Principle 1 — tension in the Amendment:** The `missing-annotation` rule
(Amendment, #428) is disabled by default, which means the *absence* of effect
annotations is the default enforcement stance. This leans implicit, in tension
with Principle 1. The tension is accepted because the linter currently lacks a
symbol table and cannot reliably distinguish calls to pure MVL helpers from
effectful stdlib functions; enabling the rule globally would produce too many
false positives. The rule should be re-evaluated for opt-out (rather than opt-in)
once symbol resolution is complete.

Principles 2–6, 8–10 are not directly affected.

### Specifications

**Spec 011 (linter):** updated — Requirement 4 covers the Hint severity level
and `hint_count()` method. No other specs are affected by this decision.
