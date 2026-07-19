# ADR-0057: Regex-membership refinement predicates

**Status:** Accepted
**Date:** 2026-07-19
**Issues:** #1921, #1911 (SQL-injection motivating case), #1919 (sibling string-content refinements), #1939 (follow-up: non-ASCII regex support)

---

## Context

MVL's refinement predicate language now supports string-content operations
(`contains`, `starts_with`, `ends_with`; ADR-0055 / #1919). Those primitives
cover the *absence* half of the string refinement story — "must not contain",
"must not start with". They do not cover the *format-validation* half:
"the value must match this shape".

Industrial security cases require both. A refinement type for an IBAN
candidate needs to state `^[A-Z]{2}[0-9]{2}[A-Z0-9]+$`; a bearer token
needs `^Bearer [A-Za-z0-9._~+/=-]+$`; a safe identifier needs
`^[a-zA-Z_][a-zA-Z0-9_]*$`. None of these can be expressed with the
primitive content operations alone.

The obvious surface — `self.matches(pattern_literal)` — brings a design
question the string-content ops did not: **decidability**. Full PCRE regex
is Turing-equivalent (backreferences alone are enough to encode
context-sensitive languages); the SMT solver Z3 cannot decide it. We must
commit to a fragment MVL's solver can handle deterministically.

---

## Decision

Add `self.matches(pattern_literal)` as a refinement predicate over the
**regular** fragment of regex syntax, discharged by:

- **L1** — constant-fold literal-string × literal-pattern cases via the Rust
  `regex` crate.
- **L2** — extract length bounds from anchored fixed-quantifier patterns
  (helper landed; solver integration deferred — see Consequences).
- **L5** — translate the pattern into a Z3 `Regexp` AST and discharge via
  `(str.in.re self <regex-tree>)`.

### 1. Grammar

```
ref_atom := ...
         |  ref_atom '.' 'matches' '(' string_literal ')'
```

The argument must be a compile-time string literal. Symbolic patterns are
irregular by construction (they may be built at runtime from unverified
inputs) and are rejected.

### 2. Admitted fragment

Only the **regular** subset of regex is admitted:

- Literal characters and standard escapes (`\.`, `\\`, `\n`, `\t`, `\r`,
  `\0`, and any escaped regex metacharacter)
- Character classes `[abc]`, `[a-z]`, `[^A-Z]`
- Predefined classes `\d \D \w \W \s \S`
- `.` — any single character
- Alternation `a|b`
- Quantifiers `*`, `+`, `?`, `{n}`, `{n,m}`, `{n,}` (non-greedy modifier `?`
  accepted and ignored — greediness has no bearing on set membership)
- Anchors `^`, `$` (Z3's `str.in.re` requires full-string match, so anchors
  are semantic no-ops in the translation)
- Non-capturing groups `(?:...)`
- Plain groups `(...)` (treated as non-capturing; MVL admits no captures)

### 3. Rejected fragment

The following are irregular or otherwise outside the SMT-decidable domain
and are rejected **at parse time** by
`src/mvl/parser/regex_frag.rs::validate`, with a diagnostic naming the
offending feature:

- Backreferences `\1`..`\9`, `\k<name>`, `\g<name>` — makes the language
  context-sensitive; not decidable via automata.
- Lookahead / lookbehind `(?=...)`, `(?!...)`, `(?<=...)`, `(?<!...)` —
  irregular.
- Named capture `(?<name>...)`, `(?P<name>...)` — the declaration is
  technically regular but opens the door to backreferences downstream;
  rejected defensively.
- Recursion `(?R)`, `(?N)` — irregular.
- Atomic groups `(?>...)` — non-standard, PCRE-specific.
- Conditional expressions `(?(cond)yes|no)` — irregular.
- Inline flags `(?i)`, `(?-i)` — flag semantics differ across regex
  engines and Z3.

### 4. L1 discharge

For a call like `validate("hello")` with predicate
`self.matches("^[a-z]+$")`, the L1 evaluator uses the `regex` crate to
compile the pattern and check `is_match(literal)`. `Some(bool)` is
returned; `None` on the (unexpected) case of a pattern that the crate
rejects after having cleared the fragment validator.

### 5. L5 discharge

`src/mvl/checker/solver/regex_z3.rs` implements a compact
recursive-descent parser over the admitted fragment that emits Z3
`Regexp` values via the z3 crate's constructors:

| MVL construct       | Z3 construction                                          |
|---------------------|----------------------------------------------------------|
| `abc` (literal)     | `Regexp::literal(ctx, "abc")`                            |
| `.`                 | `Regexp::range(ctx, '\u{1}', '\u{7F}')` — see limitation |
| `[a-z]`             | `Regexp::range(ctx, 'a', 'z')`                           |
| `[abc]`             | `union(literal("a"), literal("b"), literal("c"))`        |
| `[^0-9]`            | Union of complementary ASCII ranges (see below)          |
| `a\|b`              | `Regexp::union(&[a, b])`                                 |
| `a*` / `a+` / `a?`  | `a.star()` / `a.plus()` / `union(a, "")`                 |
| `a{n,m}`            | `a.loop(n, m)`                                           |
| `a{n,}`             | `concat(a.loop(n, n), a.star())`                         |
| `(?:...)` / `(...)` | Inner group directly                                     |
| `^`, `$`            | Empty-string regex (no-op)                               |

The predicate then encodes as `self_str.regex_matches(&re)` — Z3 emits
`(str.in.re self regex)` in SMT-LIB2.

### 6. Runtime fallback

When neither L1 nor L5 can decide (symbolic argument, translator gap,
solver timeout), the checker emits a runtime assertion via
`mvl_runtime::refine::mvl_regex_matches(&value, pattern)`. This helper
uses the `regex` crate and lives in the runtime crate so generated code
doesn't take a direct `regex` dependency. Runtime asserts compile out in
release builds via `debug_assert!`.

---

## Consequences

### Positive

- **Format-validation refinements** for the industrial case studies MVL
  targets: IBAN, bearer tokens, safe identifiers, syslog lines, XML tag
  names, SQL identifiers.
- **Decidability preserved.** Rejecting the irregular fragment at parse
  time keeps every accepted pattern within Z3's `RegLan` theory.
- **Zero-runtime for provable cases.** Refinements discharged at L1 or L5
  produce no runtime code; only symbolic-argument paths pay the
  compile-once/check-once `Regex` cost, and only in debug builds.

### Neutral

- **Composable with string-content ops (#1919).** A predicate can mix
  `matches` and `contains` clauses:
  `self.matches("^Bearer .+$") && !self.contains("..")`. Both go through
  `impl_z3_str` in L5.

### Negative

- **ASCII-only ranges at L5.** The z3 0.12 crate encodes `re.range`
  bounds as UTF-8 byte sequences; multi-byte bounds produce an empty
  language. `regex_z3::MAX_ADMITTED_CHAR` clamps to `0x7F`; non-ASCII
  refinement patterns fall through to `RuntimeCheck` rather than get a
  wrong static answer. Tracked as follow-up **#1939**.
- **L2 length extraction landed as a helper only.** The
  `regex_length_interval` function extracts `[min, max]` from anchored
  fixed-quantifier patterns (`^.{n,m}$`, `^literal$`), but L2's interval
  machinery is integer-domain over `self` and does not model
  `len(self)` intervals for String parameters. Wiring the helper into a
  proper length-interval abstraction is future work — deliberately
  scoped out of this ADR to keep the change surgical.
- **NUL characters excluded.** Z3's C API is NUL-terminated;
  `any_char` starts at U+0001. Refinements matching NUL bytes are not
  supported.
- **Negated classes containing predefined class shorthands** (e.g.
  `[^\d]`) fall through to `RuntimeCheck` — the negation path
  materialises complementary ranges, which doesn't cleanly compose
  with predefined-class regexes. Rare in practice; the equivalent
  `[^0-9]` form works.

---

## Rejected Alternatives

### A. Admit full PCRE, escape hatches for undecidable features

Would let users write patterns compilers can't reason about, silently
demoting refinements to runtime-only. Contradicts MVL's "the signature
IS the threat model" principle: a static-looking refinement that isn't
statically enforced is worse than no refinement.

### B. Symbolic patterns

Allow `self.matches(some_var)`. Rejected: a runtime pattern is arbitrary
code — it may itself be tainted user input, defeating the point of a
refinement. The literal restriction is the fragment MVL commits to.

### C. Z3 `re.comp` for negation

The obvious encoding of `[^X]` is `intersect(anychar, comp(X))`. In
practice, Z3 4.x + z3 crate 0.12 returns UNSAT for `intersect(anychar,
comp(digit_range)).plus()` on strings that clearly should match — the
`re.comp` interacts poorly with subsequent quantifiers over
sequence-domain complements. Materialising negation as a union of
complementary ranges is cleaner and faster to solve.

### D. Bridge L2 to a length-interval domain

Given `self.matches("^.{5,10}$")`, a proper L2 integration would let
downstream `len(param) op N` checks discharge. Implementing this
correctly requires generalising L2 from integer intervals on `self` to
length intervals on `len(self)` for String parameters — a structural
change with knock-on effects on every existing L2 consumer. Landed the
extraction *helper* (`regex_length_interval`) here so the future L2
generalisation can use it without duplicating the pattern analysis;
deferred the wiring itself.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **R6 (refinement types).** **Strengthens.** Adds a decidable predicate
  form that expresses format-validation invariants without escaping into
  a runtime check.
- **R7 (contracts).** **Strengthens.** `matches` is admissible in
  `requires` / `ensures` clauses (the `expr_to_ref_expr_ext` path handles
  it); function callers benefit from static regex reasoning at the call
  site.
- Other requirements: unchanged.

### Design Principles (README)

- **Explicit over implicit** — consistent with. The admitted fragment
  and its boundary are named in this ADR; the parser rejects irregular
  features with a diagnostic naming the offending feature.
- **One way to do it** — consistent with. `matches` is the sole regex
  form; there is no PCRE mode, no runtime pattern.
- **The signature IS the threat model** — strengthens. Format
  invariants that today live in comments or ad-hoc runtime checks can
  be promoted into the signature and verified.
- **No bare unwrap** — consistent with. The runtime helper uses
  `.expect("compiler bug")` intentionally — a panic here signals a
  fragment-validator vs. `regex` crate disagreement, which is a
  compiler bug worth crashing on.
- Other principles: consistent with.

### Specifications

- Spec `.openspec/specs/006-refinement-types/spec.md` (if present) will
  gain a `self.matches` clause in the predicate grammar section. Add
  when the spec next moves to reflect this change.

---

## References

- Sibling ticket #1919 — string-content refinement predicates (merged;
  provides the `StringOp` AST plumbing this ADR mirrors)
- Motivating case #1911 — SQL injection prevention via refinement types
- Follow-up #1939 — z3 crate 0.12 limits regex ranges to ASCII
- Z3 regex theory reference — https://microsoft.github.io/z3guide/docs/theories/Regular%20Expressions
- Undecidability of PCRE features — Biró et al. 2023, *On the impossibility
  of general-purpose regex verification*
