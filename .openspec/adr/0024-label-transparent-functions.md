# ADR-0024: Label-Transparent Functions

**Status:** Superseded by ADR-0036
**Date:** 2026-05-09
**Issues:** #179

---

## Context

Functions that transform data (e.g. `json.decode`, `regex.match`) lose the
security label of their input: `decode(tainted_string)` returns
`Result[Value, String]` — the `Tainted` label is silently stripped.

Without label propagation, the IFC system has a silent hole at stdlib
boundaries: code that decodes a network payload receives an unlabeled `Value`
and may pass it directly to a trusted sink without any compiler warning.

The `format()` builtin already handles this as a special case in
`checker/calls.rs` (lines 134–141): it joins the labels of all arguments and
applies the resulting label to its return type.  The same pattern needs to be
generalizable to other functions.

Issue #179 originally proposed full **label-polymorphic generics** —
`fn decode[L](s: L[String]) -> Result[L[Value], String]` — but that requires
~1000 lines of new syntax, parser, and type-inference machinery.

---

## Decision

Introduce a `transparent` modifier keyword for function declarations.

**Syntax:**

```mvl
pub transparent partial fn decode(s: String) -> Result[Value, String] { … }
pub transparent builtin fn match(pattern: Regex, s: String) -> Option[String]
```

**Semantics:** At a call site, the checker joins the security labels of all
arguments (using `ifc::join_opt`) and wraps the declared return type with that
joined label (using `ifc::apply_label`).  This is identical to what `format()`
does today, promoted from a hard-coded special case to a declared property.

**Restrictions:**
- `transparent` may be combined with any totality modifier (`partial`, `total`)
  or with `builtin`, but not with `test`.
- Only function authors (stdlib contributors) can mark a function transparent;
  the keyword is not restricted syntactically but relies on author discipline.
- The checker does not verify that the implementation actually propagates
  labels — that is the implicit contract of `transparent`.

**Implementation:**
1. Add `TokenKind::Transparent` to the lexer.
2. Add `is_label_transparent: bool` to `FnDecl` (AST) and `FnInfo` (checker).
3. Parse the keyword in `functions.rs`.
4. In `calls.rs`, replace the hard-coded `format` special case with a general
   `fn_info.label_transparent` branch.
5. Mark `json.decode` as `transparent`.

---

## Consequences

**Positive:**
- Closes the silent label-drop hole at stdlib decode/transform boundaries.
- Generalizes the existing `format()` special case — less ad-hoc checker code.
- Zero new syntax burden on LLM-generated user code; only stdlib authors use
  `transparent`.

**Negative:**
- The compiler trusts the author's claim: a `transparent` function with a
  body that ignores its input label will silently give wrong IFC results.
- Does not handle multi-argument label propagation beyond join (e.g. a function
  that propagates arg 1's label to one field and arg 2's to another).

---

## Rejected Alternatives

### Label-Polymorphic Generics (Issue #179 original proposal)

```mvl
pub fn decode[L](s: L[String]) -> Result[L[Value], String]
```

Full label variables with call-site inference.  Correct, but requires:
- New `GenericParam::Label` AST node
- New `TypeExpr::LabelApply` for `L[T]` syntax
- Label unification at call sites in the checker
- ~1000 lines of new machinery

Deferred to a future phase when generics are fully implemented (phase 7+).

### Warning on Label-Drop

Emit a compiler warning when a labeled value is passed to a function whose
return type is unlabeled.  Rejected because it cannot distinguish
`len(tainted)` (content-independent, drop is fine) from `decode(tainted)`
(content-carrying, drop is wrong) without semantic annotation — which is
exactly what `transparent` provides.

### Hardcoded Allowlist (extend the `format` special case)

Add more function names to the existing `if name == "format"` branch.
Rejected because it requires a compiler change for every new stdlib function
and is not declarative.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **Requirement 11 (IFC):** Strengthened — `transparent` closes a hole where
  labels were silently dropped at stdlib boundaries, improving the guarantee
  that tainted data remains tracked through transform functions.
- All other requirements: unchanged.

### Design Principles (README)

- **Safety by default:** Strengthened — fewer silent label-drop paths mean
  the safe default (tracked taint) is preserved across more call boundaries.
- **Simplicity:** Consistent with — `transparent` is a single keyword with
  clear semantics, no new type syntax, no inference.
- **Explicit over implicit:** Consistent with — the author declares that a
  function propagates labels; users see this in the signature.
- **Trusted but verified:** Tension — the compiler trusts the `transparent`
  claim without verifying the body.  Acceptable because `transparent` is
  restricted to stdlib authors and the alternative (no enforcement) is worse.

### Specifications

- **spec 003-information-flow:** Requirement 2 (External Input is Tainted,
  deferred) is partially addressed — `json.decode` will now propagate taint
  from its input string to the decoded value.  The spec comment in `json.mvl`
  referencing #179 should be updated to reference ADR-0024 instead.
- No other specs are directly affected.

---

## Future: Path to Full Label-Polymorphic Generics

When generics are fully implemented (phase 7+), `transparent` becomes
unnecessary — functions can instead declare their label propagation precisely:

```mvl
pub fn decode[L](s: L[String]) -> Result[L[Value], String]
```

At that point, `transparent` functions in the stdlib should be migrated to
label-polymorphic signatures.  The `transparent` keyword may then be deprecated
or restricted to functions with no body (builtins only).

The key stepping stones:
1. `GenericParam::Label(String)` — label variables in generic parameter lists
2. `TypeExpr::LabelApply { var, inner }` — `L[T]` type expressions
3. Label unification in `checker/calls.rs` — infer `L` from argument types
4. Substitution of inferred labels into return types
