# sql_injection_prevention — QF-Strings + IFC-on-taint case study

A database query-construction kernel that structurally prevents SQL injection
(OWASP A03:2021, CWE-89) by combining information-flow control (IFC) with
string-refinement types.

**Standard.** OWASP Top 10 A03:2021 Injection; PCI-DSS §6.5.1.
**Ticket.** `mvl-lang/mvl#1911`.
**Domain distinctiveness.** Eighth case study in the refinement-paper corpus.
First to exercise **QF-Strings** theory as a refinement predicate kind
(`String::contains` via #1919, `String::matches` regex via #1921). Extends
the IFC-on-taint story from structured data (ETCS, grid, CBTC) to **string
content** — the actual attack surface of most real-world web applications.

## Files

- `model.mvl` — refined string types (`SafeSqlParam`, `BoundedInput`,
  `SqlIdentifier`, `EscapedSqlParam`, `SqlQuery`, `InputRejectReason`,
  `AdmitResult`).
- `injection.mvl` — the kernel: IFC boundary, L5-forcing QF-Strings
  predicates, compound safety decision, query builder, inline unit tests.
- `main.mvl` — six scenarios walking through canonical attack payloads and
  the accept path.
- `injection_test.mvl` — end-to-end scenario tests + IFC round-trip +
  priority-ordering coverage.
- `Makefile` — standard targets plus `make test-owasp` (OWASP A03:2021
  assurance envelope).

## What is proven

`make prove` reports (production file only):

```
Summary: 7 proven (L1:3 L2:0 L3:0 L4:0 L5:4), 0 runtime, 0 failed
```

- **L1 (3)** — trivial literal subsumption: call-site `remaining_param_slots`
  checks for the three literal test values (0, 20, 32).
- **L5:z3 (3)** — Z3 **QF-Strings** discharges — the paper's fullest
  QF-Strings demonstration:
  - `is_safe_input` → `!result.contains("'") && !result.contains(";") && !result.contains("--")` — metacharacter absence propagated from `SafeSqlParam` type via string-content predicates (#1919)
  - `is_properly_escaped` → `result.matches("^[a-zA-Z0-9_ .,=()@-]*$")` — escape-completeness propagated from `EscapedSqlParam` regex type via RegLan (#1921)
  - `no_injection_possible` → `result.matches("^[a-zA-Z_][a-zA-Z0-9_]*$")` — identifier well-formedness propagated from `SqlIdentifier` via RegLan (#1921)
- **L5:z3 (1)** — Z3 QF-NIA discharge (arithmetic):
  - `remaining_param_slots: result >= 0 && result <= 32` — Fourier–Motzkin from the 32-slot bound

## IFC boundary (the distinguishing feature)

Single audit anchor — sole declassification of user-controlled input:

- **`SQL-DECLASSIFY-001`** — the only `relabel trust` in the entire example,
  in `declassify_user_input`. HTTP input arrives as `Tainted[String]`;
  this is the single checkpoint before the value becomes usable.

Reproduce the audit:

```bash
grep -rn "SQL-DECLASSIFY-001" .
```

Returns exactly one line. Every crossing of the IFC boundary is grep-able.

**Why this direction.** Earlier IFC cases (ETCS, grid, CBTC) trace `Tainted`
data arriving from an untrusted external source. This case does the same for
**string content** — HTTP request bodies, URL parameters, form fields. The
refinement types then certify that user-controlled strings are SQL-safe before
they can reach any query-construction function.

## Compound decision for MC/DC

`should_reject_input(length_exceeded, contains_metachar, is_from_trusted_zone,
has_explicit_bypass)` — four-atom compound predicate with a structurally
coupled term: `contains_metachar` appears in two clauses
(`contains_metachar && !is_from_trusted_zone` and `contains_metachar &&
!has_explicit_bypass`).

Audit anchor: `MCDC-SQLINJ-001`.

Unique-cause MC/DC is structurally impossible for `contains_metachar`.
Under DO-178C masking MC/DC (Appendix A 6.4.4.2), the coupled term is
exempt: `make mcdc` without `--masking` reports MISSED; with `--masking`
reports PASS.

## QF-Strings coverage

The three L5:z3 QF-Strings obligations exercise two distinct predicate kinds:

| Function | Obligation kind | L5 clause |
|---|---|---|
| `is_safe_input` | `StringOp::Contains` (#1919) | `!result.contains("'")`… |
| `is_properly_escaped` | `RegexMatch` (#1921) | `result.matches("^[a-zA-Z0-9_ .,=()@-]*$")` |
| `no_injection_possible` | `RegexMatch` (#1921) | `result.matches("^[a-zA-Z_][a-zA-Z0-9_]*$")` |

Both predicate kinds are propagated symbolically: the hypothesis comes from
the parameter's refined type (via cross-module type alias resolution), and
the `ensures` clause states the same predicate, forcing Z3 to discharge it
as L5 rather than constant-folding it at L1.

**Compiler improvement landed as part of this case study:**

Two fixes to the QF-Strings solver path were needed to reach L5 discharge:

1. **Cross-module type alias propagation** (`refinements.rs` → contracts
   checker): `build_type_alias_refinements_combined` now merges type aliases
   from all loaded modules so that `s: SafeSqlParam` (defined in `model.mvl`)
   supplies its refinement predicate to the contracts checker in `injection.mvl`.

2. **Symbolic argument hypothesis propagation** (`layer5.rs` → `impl_z3_str`):
   when the `ensures` clause's return expression is a variable name (e.g. `e`)
   rather than a literal, the solver now looks up `e`'s type predicate in
   `var_refs` and asserts it on the Z3 `self` string variable, making the
   hypothesis visible for UNSAT checking.

3. **Partial hypothesis encoding** (`assert_str_hyp_partial`): for compound
   type predicates combining `StringOp`/`RegexMatch` with `len(self)` (e.g.
   `EscapedSqlParam`), the string-op parts are now asserted independently so
   a non-encodable `len` sub-expression does not silence the provable regex
   portion.

## Standard mapping (OWASP A03:2021)

`make test-owasp` composes the assurance envelope across three axes:

- **Static proof** (compile-time, all inputs) — 7 proven / 0 runtime / 0 failed
- **Behavioural unit tests** (dynamic, specific attack payloads) — 16 passed
- **MC/DC** (compound reject-decision, `--masking` mode) — PASS

## Running the demo

```bash
make run
```

Produces:
```
1. clean web input:                 accepted: input admitted to query envelope
2. ' OR 1=1 --:                     rejected: metacharacter from untrusted origin
3. ; DROP TABLE users --:           rejected: metacharacter from untrusted origin
4. length bomb (6000 chars):        rejected: input exceeds length envelope
5. internal + override (O'Brien):   accepted: input admitted to query envelope
6. internal + no override:          rejected: metacharacter without operator override
```

## Design decisions worth naming

**Four refined string types, not one.** `SafeSqlParam`, `BoundedInput`,
`SqlIdentifier`, and `EscapedSqlParam` each state a distinct QF-Strings
invariant. The distinct types make the paper's §Predicate Language taxonomy
concrete: substring absence, length bounds, regex format, and composed regex+len.

**IFC on input, not output.** Earlier cybersecurity case `data_integrity`
traces the *output* direction (`Secret[T]` → `relabel release`). This case
traces the *input* direction (`Tainted[T]` → `relabel trust`). Both
directions are needed for realistic security software; together they close
the paper's IFC-completeness claim.

**Runtime obligations are zero.** All seven proof obligations discharge
statically (L1 or L5). This is the strongest assurance posture in the
corpus — no obligations deferred to runtime.

**Grep-able audit surface.** One `SQL-DECLASSIFY-001` grep line, one
`MCDC-SQLINJ-001` anchor. Any future violation is grep-visible.
