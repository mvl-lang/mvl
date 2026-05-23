---
domain: language
version: 0.2.0
status: draft
date: 2026-05-23
---

# 003 — Information Flow Control

The MVL information flow control system covers Requirement 11 (IFC). Security labels track data provenance through the type system. The compiler prevents secret leakage, injection attacks, and tainted data reaching trusted sinks.

## Philosophy

Values are either bare (unlabeled, implicitly public) or carry an explicit security label. Labels are opaque category identifiers — there is no hierarchy or lattice. A `Tainted[String]` and a bare `String` are distinct types; assignment between them requires an explicit `relabel` transition. The compiler enforces label compatibility at every call site — the LLM cannot generate code that passes tainted input to trusted sinks without a visible, auditable relabeling step. This converts OWASP Top 10 categories A01, A03, A05, A07, A08, and A10 from discipline-based prevention to compile-time errors.

**Origin:** Denning's lattice model (1976) — inspiration. Perl's taint mode (1989) — runtime taint tracking. Jif (Myers, 1999) — compile-time IFC. MVL departs from the lattice model (#894): labels are user-defined categories with no implicit ordering. Transitions between labels (including removing a label) require explicit `relabel` calls, making every trust-boundary crossing auditable by grep.

## Labels

Two labels are pre-seeded in `std/ifc.mvl`:

| Label | Meaning | Produced by | Removed by |
|-------|---------|-------------|------------|
| `Tainted[T]` | From external sources — user input, network, env vars | `relabel taint(v, "TAG")` | `relabel trust(v, "TAG")` |
| `Secret[T]` | Cryptographic material, passwords, keys | `relabel classify(v, "TAG")` | `relabel release(v, "TAG")` |

Bare `T` (no label) represents public/trusted data. `Tainted[T]` and `Secret[T]` are distinct types from each other and from bare `T` — no implicit conversions exist.

## Requirements

### Requirement 1: Security Labels on Types [MUST]

Types MUST support user-defined security labels via the `label`/`relabel` system. Pre-seeded labels are `Tainted[T]` and `Secret[T]`. Labels MUST be part of the type — `Tainted[String]` and `Secret[String]` and bare `String` are three distinct, mutually incompatible types. No implicit conversions exist between labeled and unlabeled types in either direction.

This is Design Principle 7 ("Security labels on all data"). Labels are types, not conventions — the compiler enforces them.

**Implementation:** `src/mvl/checker/types.rs`, `src/mvl/checker/ifc.rs`, `src/mvl/parser/ast.rs`

**Tests:** `tests/type_checker.rs::secret_flows_to_public_rejected`, `tests/type_checker.rs::label_types_corpus_parses_and_checks`, `tests/type_checker.rs::types_compatible_labeled_vs_bare_rejected`, `tests/type_checker.rs::types_compatible_different_labels_rejected`

#### Scenario: Label mismatch

- GIVEN `fn log_message(msg: String) ! Log`
- WHEN the caller passes `secret_password: Secret[String]`
- THEN the compiler MUST reject: "cannot pass `Secret[String]` where `String` is expected"

#### Scenario: Labeled type incompatible with bare type

- GIVEN `fn needs_string(s: String) -> Int`
- WHEN the caller passes `t: Tainted[String]`
- THEN the compiler MUST reject: `Tainted[String]` ≠ `String` — `relabel trust` required first

### Requirement 2: External Input is Tainted [MUST] *(Deferred — Phase 2)*

> **Status:** Partially addressed. Auto-tainting of external sources is not yet implemented (tracked in #28, requires runtime/stdlib integration). Label propagation through stdlib transform functions (e.g. `json.decode`) is addressed by ADR-0024 (`transparent` keyword): `decode(tainted_str)` now returns `Tainted[Result[Value, String]]` rather than silently dropping the label.

Data from external sources MUST be automatically labeled `Tainted`. This includes:
- HTTP request bodies, headers, query parameters
- stdin input
- Environment variables
- File contents read from disk
- Network responses
- Database query results (when from user-influenced queries)

#### Scenario: Network response is tainted

- GIVEN `fn http_get(url: String) -> Result[Tainted[Response], NetError] ! Net`
- WHEN the response body is used in `db.execute(db, format("SELECT {}", response.body), [])`
- THEN the compiler MUST reject: "`Tainted[String]` cannot be used where `String` is expected"

### Requirement 3: Label Transitions are Explicit [MUST]

Removing or changing a security label MUST require an explicit `relabel` call. The standard transitions are:
- `relabel trust(input: Tainted[T], tag: String) -> T` — validate tainted input at a trust boundary
- `relabel release(secret: Secret[T], tag: String) -> T` — intentional secret release

These transitions MUST be auditable — `grep "relabel trust"` and `grep "relabel release"` finds every point where the security boundary is crossed. The `tag` string documents the reason.

**Implementation:** `src/mvl/checker.rs`, `src/mvl/checker/ifc.rs`, `std/ifc.mvl`

**Tests:** `tests/type_checker.rs::tainted_string_rejected_where_bare_string_expected`, `tests/type_checker.rs::types_compatible_labeled_vs_bare_rejected`

#### Scenario: SQL injection prevention

- GIVEN user input `name: Tainted[String]`
- WHEN `execute(db, format("SELECT * WHERE name = '{}'", name), [])`
- THEN the compiler MUST reject: `format` propagates `Tainted` — result is `Tainted[String]`, incompatible with `String`

#### Scenario: Explicit trust transition

- GIVEN user input `name: Tainted[String]`
- WHEN `let safe: String = relabel trust(name, "VALIDATED")` followed by the query
- THEN the compiler MUST accept: the transition is explicit and auditable

### Requirement 4: Secret Preservation [MUST]

Functions that process secrets MUST return secrets. The label MUST propagate through computation.

**Implementation:** `src/mvl/checker.rs`, `src/mvl/checker/ifc.rs`

**Tests:** `tests/type_checker.rs::arithmetic_label_join_propagates`, `tests/type_checker.rs::arithmetic_label_join_downgrade_rejected`, `tests/type_checker.rs::propagation_ifc_corpus_parses_and_checks`, `tests/compile_and_run.rs::safe_division_check_passes`, `tests/compile_and_run.rs::safe_division_runs_and_produces_expected_output` (#191)

#### Scenario: Hashing preserves secrecy

- GIVEN `fn hash(pwd: Secret[String]) -> Secret[Hash]`
- THEN the hash is still `Secret` — it derived from secret material

#### Scenario: Comparison produces public result

- GIVEN `fn verify(pwd: Secret[String], stored: Secret[Hash]) -> Public[Bool]`
- THEN the boolean result is `Public` — one bit of information from high-entropy source is safe

### Requirement 5: Error Messages Must Not Leak Secrets [MUST] *(Deferred — Phase 2)*

> **Status:** Not yet implemented. Requires ADT field-label analysis and channel-type tracking. Tracked in #29.

Error types containing `Secret` fields MUST NOT be sendable to `Public` channels (HTTP responses, logs, stdout).

#### Scenario: Secret in error message

- GIVEN `type AuthError = enum { InvalidPassword { attempted: Secret[String] } }`
- WHEN `http_respond(Err(AuthError::InvalidPassword { attempted: pwd }))`
- THEN the compiler MUST reject: "error type contains `Secret[String]`, cannot send to `Public` channel"

#### Scenario: Safe error message

- GIVEN `type AuthError = enum { InvalidPassword }`  (no secret fields)
- WHEN `http_respond(Err(AuthError::InvalidPassword))`
- THEN the compiler MUST accept: error type is fully `Public`

### Requirement 6: Logging Respects Labels [MUST]

> **Status:** Implemented. `println`/`print` and `std.log` (`log_debug`/`log_info`/`log_warn`/`log_error`) enforce IFC label check at call site. Map literal values propagate labels to the enclosing map type so embedded secrets in structured fields are also caught (#54).

Logging functions MUST accept only `Public[T]` arguments. Logging a `Secret` or `Tainted` value MUST be a compile error.

**Implementation:** `src/mvl/checker.rs` (`infer_fn_call` — IFC label check for `println`/`print`/`log_*`), `std/log.mvl`, `src/mvl/checker/ifc.rs` (`PUBLIC_SINKS`)

**Tests:** `tests/type_checker.rs::println_rejects_secret_argument`, `tests/type_checker.rs::println_rejects_tainted_argument`, `tests/type_checker.rs::println_accepts_public_argument`, `tests/type_checker.rs::log_debug_rejects_secret_argument`, `tests/type_checker.rs::log_info_rejects_secret_argument`, `tests/type_checker.rs::log_error_rejects_tainted_argument`, `tests/type_checker.rs::log_warn_rejects_clean_argument`, `tests/type_checker.rs::log_info_rejects_secret_value_in_fields_map`, `tests/type_checker.rs::log_info_accepts_public_argument`, `tests/type_checker.rs::caller_missing_log_effect_rejected`, `tests/type_checker.rs::caller_missing_log_effect_with_other_effects_rejected`, `tests/compile_and_run.rs::safe_division_check_passes`, `tests/compile_and_run.rs::safe_division_runs_and_produces_expected_output` (#191)

#### Scenario: Logging a secret

- GIVEN `log.info("Password: {}", password)` where `password: Secret[String]`
- THEN the compiler MUST reject: "`Secret[String]` cannot be logged"

#### Scenario: Logging a bare value

- GIVEN `log.info("User logged in: {}", username)` where `username: String` (bare, unlabeled)
- THEN the compiler MUST accept

### Requirement 7: IFC Applies to String Formatting and Transform Functions [MUST]

The `format()` function MUST be IFC-aware. The result label MUST be the join (highest) of all argument labels. Functions declared `transparent` (ADR-0024) MUST propagate argument labels to their return type using the same join semantics. This closes the silent label-drop hole at stdlib boundaries (e.g. `json.decode`, `json.encode`).

**Implementation:** `src/mvl/checker/ifc.rs`, `src/mvl/checker/calls.rs`, `src/mvl/parser/lexer.rs` (`transparent` keyword), ADR-0024

**Tests:** `tests/type_checker.rs::arithmetic_label_join_propagates`, `tests/type_checker.rs::arithmetic_label_join_downgrade_rejected`, `tests/type_checker.rs::format_propagates_secret_label`, `tests/type_checker.rs::transparent_fn_propagates_label`, `tests/type_checker.rs::decode_propagates_tainted_label`

#### Scenario: Tainted argument taints the result

- GIVEN `let msg = format("Hello {}", tainted_name)` where `tainted_name: Tainted[String]`
- THEN `msg` MUST be `Tainted[String]` — the tainted input propagates

#### Scenario: All public arguments

- GIVEN `let msg = format("Count: {}", count)` where `count: Public[Int]`
- THEN `msg` MUST be `Public[String]`

### Requirement 11: Implicit Flows Are Rejected [MUST]

The compiler MUST detect implicit information flows via control flow (Program Counter label analysis). A `println` or `print` call — whether invoked directly or through a chain of user-defined helper functions — that appears inside a branch controlled by a `Secret` or `Tainted` condition MUST be a compile error, even if the printed arguments are `Public`.

> **Rationale:** Whether a print fires reveals the value of the controlling condition. This is a covert channel — information leaks through control flow rather than data flow. The check is interprocedural: wrapping `println` in a helper does not bypass the rule.

**Implementation:** `src/mvl/checker/ifc.rs` (`check_implicit_flows`, `build_sink_reachability`), `src/mvl/checker.rs`

**Tests:** `tests/type_checker.rs::implicit_flow_secret_if_condition_rejected`, `tests/type_checker.rs::implicit_flow_tainted_if_condition_rejected`, `tests/type_checker.rs::implicit_flow_public_condition_accepted`, `tests/type_checker.rs::implicit_flow_print_sink_rejected`, `tests/type_checker.rs::implicit_flow_else_branch_rejected`, `tests/type_checker.rs::implicit_flow_label_propagated_through_let`, `tests/type_checker.rs::implicit_flow_while_secret_condition_rejected`, `tests/type_checker.rs::cross_function_implicit_corpus_has_violations`, `tests/type_checker.rs::interprocedural_taint_corpus_has_violations`, `tests/type_checker.rs::return_label_inference_corpus_has_no_req11_violations`, `tests/type_checker.rs::interprocedural_clean_corpus_has_no_req11_violations`, `tests/type_checker.rs::call_chain_error_names_callee_and_sink`, `src/mvl/checker/passes.rs::req11_proven_for_labeled_types_with_no_violations`

**Corpus:** `tests/corpus/06_ifc/implicit_flow.mvl`, `tests/corpus/06_ifc/cross_function_implicit.mvl`, `tests/corpus/06_ifc/interprocedural_taint.mvl`, `tests/corpus/06_ifc/return_label_inference.mvl`, `tests/corpus/06_ifc/interprocedural_clean.mvl`, `tests/corpus/06_ifc/call_chain_error_message.mvl`

#### Scenario: Secret condition controls println directly

- GIVEN `fn f(flag: Secret[Bool]) -> Unit`
- WHEN `if flag { println("access granted") }`
- THEN the compiler MUST reject: implicit flow from `Secret` condition to `println` sink

#### Scenario: Public condition is safe

- GIVEN `fn f(x: Public[Bool]) -> Unit`
- WHEN `if x { println("ok") }`
- THEN the compiler MUST accept: no high-security condition

#### Scenario: Else-branch also controlled

- GIVEN `fn h(flag: Secret[Bool]) -> Unit`
- WHEN `if flag { 0 } else { println("denied") }`
- THEN the compiler MUST reject: the else-branch is also controlled by the Secret condition

#### Scenario: Cross-function implicit flow via helper wrapper

- GIVEN `fn log_access() -> Unit { println("access granted") }`
- AND `fn check(flag: Secret[Bool]) -> Unit { if flag { log_access() } }`
- THEN the compiler MUST reject: `log_access` transitively reaches `println`; calling it under a `Secret` branch leaks `flag`'s value

#### Scenario: Transitive sink reachability through two hops

- GIVEN `fn inner() { println("x") }`, `fn middle() { inner() }`, `fn outer(t: Tainted[Bool]) { if t { middle() } }`
- THEN the compiler MUST reject: `middle` is in the transitive sink-reach set (via `inner`)

#### Scenario: Pure computation helper under high-PC is safe

- GIVEN `fn hash(s: Secret[String]) -> Int { s.len() }` (no I/O)
- AND `fn process(flag: Secret[Bool], s: Secret[String]) -> Int { if flag { hash(s) } else { 0 } }`
- THEN the compiler MUST accept: `hash` does not reach any public sink

#### Scenario: Sink-reaching function called unconditionally is safe

- GIVEN `fn announce(msg: String) -> Unit { println(msg) }` (reaches a public sink)
- AND `fn send_status() -> Unit { announce("ok") }` (unconditional — no high-PC branch)
- THEN the compiler MUST accept: the program counter label is `None` at the call site

### Requirement 12: Capability Labels as IFC Tokens [MUST]

IFC labels MUST be usable as capability tokens for resource provenance tracking. The same `label`/`relabel` machinery that tracks `Tainted` and `Secret` data MUST also track configuration-sourced values that gate access to external resources. The compiler MUST enforce label compatibility at call boundaries — bare `String` or differently-labeled values MUST be rejected where a capability label is expected.

Capability labels absorb the "Capability Security" requirement (originally proposed as Req 13, absorbed into Req 11 per #931). Effects (`! FileRead`) tell you the *class* of action; capability labels tell you *which* resource — completing the security picture.

**Security model:** Capability labels are provenance-tracking tokens, not access-control tokens. Any code that can call the wrap `relabel` transition can create a labeled value; the guarantee is that every label transition is auditable (grep for `relabel config_path` / `relabel unconfig_path` finds every crossing). For stronger isolation, restrict imports of the wrap relabel to designated producer modules.

**Standard capability labels:**

| Label | Produced by | Consumed by |
|-------|-------------|-------------|
| `ConfigPath[T]` | `config.load_path` / `config.default_path` | `io.read_config_file`, `io.write_config_file` |
| `DbUrl[T]` | `db.load_db_url` / `db.default_db_url` | database connect functions |
| `ApiEndpoint[T]` | `net.load_endpoint` / `net.default_endpoint` | `net.endpoint_connect`, `net.endpoint_listen` |
| `AuditTarget[T]` | `audit.load_audit_target` / `audit.default_audit_target` | audit actor initialization |

**Implementation:** `std/config.mvl`, `std/db.mvl`, `std/net.mvl`, `std/audit.mvl`, `src/mvl/parser.rs`, `src/mvl/checker/context.rs`

**Tests:** `tests/type_checker.rs::capability_labels_corpus_parses_and_checks`, `tests/type_checker.rs::config_path_to_bare_string_return_rejected`, `tests/type_checker.rs::raw_string_to_config_path_rejected`, `tests/type_checker.rs::db_url_rejects_tainted_string`, `tests/type_checker.rs::api_endpoint_rejects_raw_string`, `tests/type_checker.rs::audit_target_rejects_raw_string`, `tests/type_checker.rs::config_path_relabel_roundtrip`, `tests/type_checker.rs::capability_labels_are_distinct`, `tests/type_checker.rs::config_path_call_site_rejects_raw_string`, `tests/type_checker.rs::db_url_call_site_rejects_raw_string`, `tests/type_checker.rs::api_endpoint_call_site_rejects_raw_string`, `tests/type_checker.rs::audit_target_call_site_rejects_raw_string`, `tests/type_checker.rs::db_url_relabel_roundtrip`, `tests/type_checker.rs::api_endpoint_relabel_roundtrip`, `tests/type_checker.rs::audit_target_relabel_roundtrip`, `tests/type_checker.rs::unconfig_path_on_bare_string_invalid`, `tests/type_checker.rs::undb_url_on_config_path_invalid`

**Corpus:** `tests/corpus/06_ifc/capability_labels.mvl`

#### Scenario: Raw string rejected at capability boundary

- GIVEN `fn read_config_file(p: ConfigPath[String]) -> Result[Tainted[String], IoError] ! FileRead`
- WHEN the caller passes a bare `String`
- THEN the compiler MUST reject: "expected `ConfigPath[String]`, got `String`"

#### Scenario: Tainted string rejected at capability boundary

- GIVEN `fn endpoint_connect(endpoint: ApiEndpoint[String], port: Int) -> Result[TcpStream, NetError] ! Net`
- WHEN the caller passes `Tainted[String]` from user input
- THEN the compiler MUST reject: different labels are not interchangeable

#### Scenario: Capability labels are distinct

- GIVEN a function expecting `DbUrl[String]`
- WHEN the caller passes `ConfigPath[String]`
- THEN the compiler MUST reject: `ConfigPath` and `DbUrl` are distinct labels

#### Scenario: Relabel round-trip preserves type safety

- GIVEN `relabel config_path(s, "CONFIG-DEFAULT")` wraps bare String as ConfigPath
- AND `relabel unconfig_path(p, "IO-READ")` unwraps ConfigPath to bare String
- THEN the round-trip type-checks and each transition produces an audit event

### Known Limitations

- Label inference through unannotated intermediate bindings (without explicit type annotations) is conservative: the label may not be propagated. Users should annotate intermediate variables explicitly.
- `match` scrutinee implicit flows are detected when the scrutinee is a directly labeled variable.
