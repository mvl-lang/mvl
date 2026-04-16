---
domain: language
version: 0.1.0
status: draft
date: 2026-04-11
---

# 003 — Information Flow Control

The MVL information flow control system covers Requirement 11 (IFC). Security labels track data provenance through the type system. The compiler prevents secret leakage, injection attacks, and tainted data reaching trusted sinks.

## Philosophy

Every value carries a security label. Data flows up the security lattice freely; flowing down requires explicit declassification. The compiler enforces the lattice — the LLM cannot generate code that leaks secrets or passes tainted input to queries. This converts OWASP Top 10 categories A01, A03, A05, A07, A08, and A10 from discipline-based prevention to compile-time errors.

**Origin:** Denning's lattice model (1976). Perl's taint mode (1989) — runtime taint tracking. Jif (Myers, 1999) — compile-time IFC. The MVL makes it compile-time and LLM-annotated.

## The Security Lattice

```
Secret          (highest — cryptographic material, passwords, keys)
   |
Tainted         (from external sources — user input, network, env vars)
   |
Clean           (sanitized — passed through explicit validation)
   |
Public          (lowest — safe for any output channel)
```

Data flows UP freely: `Public` → `Tainted` is always allowed (safe data in an unsafe context is fine).
Data flows DOWN only via explicit declassification: `Tainted` → `Clean` requires calling a `sanitize` function. `Secret` → `Public` requires calling a `declassify` function.

## Requirements

### Requirement 1: Security Labels on Types [MUST]

Every type MUST support security labels: `Public<T>`, `Tainted<T>`, `Secret<T>`, `Clean<T>`. Labels MUST be part of the type — `Public<String>` and `Secret<String>` are different types.

**Implementation:** `src/mvl/checker/types.rs`, `src/mvl/checker/ifc.rs`, `src/mvl/parser/ast.rs`

**Tests:** `tests/type_checker.rs::secret_flows_to_public_rejected`, `tests/type_checker.rs::public_flows_to_secret_accepted`, `tests/type_checker.rs::label_types_corpus_parses_and_checks`

#### Scenario: Label mismatch

- GIVEN `fn log_message(msg: Public<String>) ! Log`
- WHEN the caller passes `secret_password: Secret<String>`
- THEN the compiler MUST reject: "cannot pass `Secret<String>` where `Public<String>` is expected"

#### Scenario: Label compatibility (upward flow)

- GIVEN `fn store_in_vault(data: Secret<String>)`
- WHEN the caller passes `public_name: Public<String>`
- THEN the compiler MUST accept: `Public` flows up to `Secret` freely

### Requirement 2: External Input is Tainted [MUST] *(Deferred — Phase 2)*

> **Status:** Not yet implemented. Auto-tainting requires runtime/stdlib integration (HTTP, stdin, env-var APIs). Tracked in #28.

Data from external sources MUST be automatically labeled `Tainted`. This includes:
- HTTP request bodies, headers, query parameters
- stdin input
- Environment variables
- File contents read from disk
- Network responses
- Database query results (when from user-influenced queries)

#### Scenario: Network response is tainted

- GIVEN `fn http_get(url: Clean<Url>) -> Result<Tainted<Response>, NetError> ! Net`
- WHEN the response body is used in `db.query(format("SELECT {}", response.body))`
- THEN the compiler MUST reject: "`Tainted<String>` cannot be used where `Clean<Query>` is expected"

### Requirement 3: Declassification is Explicit [MUST]

Lowering a security label MUST require an explicit function call. The declassification functions MUST be:
- `sanitize(input: Tainted<T>) -> Clean<T>` — for input validation
- `declassify(secret: Secret<T>) -> Public<T>` — for intentional secret release

These functions MUST be auditable — `grep declassify` and `grep sanitize` finds every point where the security boundary is crossed.

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::sanitize_tainted_returns_clean`, `tests/type_checker.rs::declassify_secret_returns_public`, `tests/type_checker.rs::sanitize_on_non_tainted_rejected`, `tests/type_checker.rs::declassify_on_non_secret_rejected`, `tests/type_checker.rs::direct_tainted_to_clean_without_sanitize_rejected`

#### Scenario: SQL injection prevention

- GIVEN user input `name: Tainted<String>`
- WHEN `db.query(format("SELECT * WHERE name = '{}'", name))`
- THEN the compiler MUST reject: tainted data in query

#### Scenario: Explicit sanitization

- GIVEN user input `name: Tainted<String>`
- WHEN `let clean_name: Clean<String> = sanitize(name)` followed by the query
- THEN the compiler MUST accept: data was explicitly sanitized

### Requirement 4: Secret Preservation [MUST]

Functions that process secrets MUST return secrets. The label MUST propagate through computation.

**Implementation:** `src/mvl/checker/mod.rs`, `src/mvl/checker/ifc.rs`

**Tests:** `tests/type_checker.rs::arithmetic_label_join_propagates`, `tests/type_checker.rs::arithmetic_label_join_downgrade_rejected`, `tests/type_checker.rs::propagation_ifc_corpus_parses_and_checks`, `tests/compile_and_run.rs::safe_division_check_passes`, `tests/compile_and_run.rs::safe_division_runs_and_produces_expected_output` (#191)

#### Scenario: Hashing preserves secrecy

- GIVEN `fn hash(pwd: Secret<String>) -> Secret<Hash>`
- THEN the hash is still `Secret` — it derived from secret material

#### Scenario: Comparison produces public result

- GIVEN `fn verify(pwd: Secret<String>, stored: Secret<Hash>) -> Public<Bool>`
- THEN the boolean result is `Public` — one bit of information from high-entropy source is safe

### Requirement 5: Error Messages Must Not Leak Secrets [MUST] *(Deferred — Phase 2)*

> **Status:** Not yet implemented. Requires ADT field-label analysis and channel-type tracking. Tracked in #29.

Error types containing `Secret` fields MUST NOT be sendable to `Public` channels (HTTP responses, logs, stdout).

#### Scenario: Secret in error message

- GIVEN `type AuthError = enum { InvalidPassword { attempted: Secret<String> } }`
- WHEN `http_respond(Err(AuthError::InvalidPassword { attempted: pwd }))`
- THEN the compiler MUST reject: "error type contains `Secret<String>`, cannot send to `Public` channel"

#### Scenario: Safe error message

- GIVEN `type AuthError = enum { InvalidPassword }`  (no secret fields)
- WHEN `http_respond(Err(AuthError::InvalidPassword))`
- THEN the compiler MUST accept: error type is fully `Public`

### Requirement 6: Logging Respects Labels [MUST]

> **Status:** Implemented. `println`/`print` and `std.log` (`log_debug`/`log_info`/`log_warn`/`log_error`) enforce IFC label check at call site. Map literal values propagate labels to the enclosing map type so embedded secrets in structured fields are also caught (#54).

Logging functions MUST accept only `Public<T>` arguments. Logging a `Secret` or `Tainted` value MUST be a compile error.

**Implementation:** `src/mvl/checker/mod.rs` (`infer_fn_call` — IFC label check for `println`/`print`/`log_*`), `std/log.mvl`, `src/mvl/checker/ifc.rs` (`PUBLIC_SINKS`)

**Tests:** `tests/type_checker.rs::println_rejects_secret_argument`, `tests/type_checker.rs::println_rejects_tainted_argument`, `tests/type_checker.rs::println_accepts_public_argument`, `tests/type_checker.rs::log_debug_rejects_secret_argument`, `tests/type_checker.rs::log_info_rejects_secret_argument`, `tests/type_checker.rs::log_error_rejects_tainted_argument`, `tests/type_checker.rs::log_warn_rejects_clean_argument`, `tests/type_checker.rs::log_info_rejects_secret_value_in_fields_map`, `tests/type_checker.rs::log_info_accepts_public_argument`, `tests/type_checker.rs::caller_missing_log_effect_rejected`, `tests/type_checker.rs::caller_missing_log_effect_with_other_effects_rejected`, `tests/compile_and_run.rs::safe_division_check_passes`, `tests/compile_and_run.rs::safe_division_runs_and_produces_expected_output` (#191)

#### Scenario: Logging a secret

- GIVEN `log.info("Password: {}", password)` where `password: Secret<String>`
- THEN the compiler MUST reject: "`Secret<String>` cannot be logged"

#### Scenario: Logging a public value

- GIVEN `log.info("User logged in: {}", username)` where `username: Public<String>`
- THEN the compiler MUST accept

### Requirement 7: IFC Applies to String Formatting [MUST]

The `format()` function MUST be IFC-aware. The result label MUST be the join (highest) of all argument labels.

**Implementation:** `src/mvl/checker/ifc.rs`

**Tests:** `tests/type_checker.rs::arithmetic_label_join_propagates`, `tests/type_checker.rs::arithmetic_label_join_downgrade_rejected`

#### Scenario: Tainted argument taints the result

- GIVEN `let msg = format("Hello {}", tainted_name)` where `tainted_name: Tainted<String>`
- THEN `msg` MUST be `Tainted<String>` — the tainted input propagates

#### Scenario: All public arguments

- GIVEN `let msg = format("Count: {}", count)` where `count: Public<Int>`
- THEN `msg` MUST be `Public<String>`

### Requirement 11: Implicit Flows Are Rejected [MUST]

The compiler MUST detect implicit information flows via control flow (Program Counter label analysis). A `println` or `print` call that appears inside a branch controlled by a `Secret` or `Tainted` condition MUST be a compile error, even if the printed arguments are `Public`.

> **Rationale:** Whether a print fires reveals the value of the controlling condition. This is a covert channel — information leaks through control flow rather than data flow.

**Implementation:** `src/mvl/checker/ifc.rs` (`check_implicit_flows`), `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::implicit_flow_secret_if_condition_rejected`, `tests/type_checker.rs::implicit_flow_tainted_if_condition_rejected`, `tests/type_checker.rs::implicit_flow_public_condition_accepted`, `tests/type_checker.rs::implicit_flow_print_sink_rejected`, `tests/type_checker.rs::implicit_flow_else_branch_rejected`, `tests/type_checker.rs::implicit_flow_label_propagated_through_let`, `tests/type_checker.rs::implicit_flow_while_secret_condition_rejected`, `src/mvl/checker/passes.rs::req11_proven_for_labeled_types_with_no_violations`

**Corpus:** `tests/corpus/05_ifc/implicit_flow.mvl`

#### Scenario: Secret condition controls println

- GIVEN `fn f(flag: Secret<Bool>) -> Unit`
- WHEN `if flag { println("access granted") }`
- THEN the compiler MUST reject: implicit flow from `Secret` condition to `println` sink

#### Scenario: Public condition is safe

- GIVEN `fn f(x: Public<Bool>) -> Unit`
- WHEN `if x { println("ok") }`
- THEN the compiler MUST accept: no high-security condition

#### Scenario: Else-branch also controlled

- GIVEN `fn h(flag: Secret<Bool>) -> Unit`
- WHEN `if flag { 0 } else { println("denied") }`
- THEN the compiler MUST reject: the else-branch is also controlled by the Secret condition

### Known Limitations (Phase 3)

- Cross-function implicit flows (a secret returned from a function that controls a branch in the caller) are not yet detected. Deferred to Phase 6.
- Label inference through unannotated intermediate bindings (without explicit type annotations) is conservative: the label may not be propagated. Users should annotate intermediate variables explicitly.
- `match` scrutinee implicit flows are detected when the scrutinee is a directly labeled variable.
