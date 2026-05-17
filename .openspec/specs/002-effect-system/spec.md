---
domain: language
version: 0.2.0
status: draft
date: 2026-05-17
---

# 002 — Effect System

The MVL effect system covers Requirement 7 (effect tracking) and supports Requirement 9 (data race freedom) and Requirement 8 (termination). Every side effect MUST be declared in the function signature. Pure is the default.

## Philosophy

A function signature should tell the full truth about what the function does. If a function reads a file, it says so. If it's pure, the absence of effects proves it. Effects are the mechanism that makes Requirement 3 of the OWASP Top 10 (least privilege) a compile-time guarantee.

**Origin:** Koka (Leijen, 2014) for algebraic effects. Haskell IO monad (1992) for the principle. E language (Miller, 1997) for capability-based security.

**Design direction (ADR-0034):** The effect system evolves in three steps — (1) user-defined effect declarations replace the hardcoded name list, (2) effect aliases reduce signature proliferation, (3) effect masking at stdlib module boundaries allows implementation details to stay hidden. Full algebraic handlers with discharge (Koka-style `with`) are deferred to Phase 8.

## Requirements

### Requirement 1: Effect Declaration [MUST]

Functions with side effects MUST declare them in the signature using `! Effect` syntax. Functions without effect declarations MUST be pure — the compiler MUST reject any side-effecting operation in a pure function.

This is Design Principle 6 ("Effects in signatures"). Pure is the default; every side effect is an explicit, visible opt-in in the function's type.

**Implementation:** `src/mvl/checker.rs`

**Tests:** `tests/type_checker.rs::pure_function_calling_effectful_rejected`, `tests/type_checker.rs::effectful_function_with_correct_declaration_accepted`, `tests/type_checker.rs::caller_missing_callee_effect_rejected`, `tests/compile_and_run.rs::safe_division_check_passes`, `tests/compile_and_run.rs::safe_division_runs_and_produces_expected_output` (#191)

#### Scenario: Pure function attempts I/O

- GIVEN `fn add(a: Int, b: Int) -> Int { println("adding"); a + b }`
- THEN the compiler MUST reject: "function `add` has no effect declaration but calls `println` which requires `! Console`"

**Tests:** `tests/type_checker.rs::pure_function_calling_effectful_rejected`

#### Scenario: Effect declared correctly

- GIVEN `fn greet(name: String) -> String ! Console { println("Hello"); name }`
- THEN the compiler MUST accept

**Tests:** `tests/type_checker.rs::effectful_function_with_correct_declaration_accepted`

#### Scenario: Effect propagation

- GIVEN `fn a() -> Int ! FileRead { read_config()? }` and `fn b() -> Int { a() }`
- THEN the compiler MUST reject `b`: "calls `a` which requires `! FileRead` but `b` declares no effects"

**Tests:** `tests/type_checker.rs::caller_missing_callee_effect_rejected`

### Requirement 2: Fine-Grained Effects [MUST]

Effects MUST be fine-grained, not a single `IO` bucket. The canonical stdlib effects are declared in `std/effects.mvl` and registered by the checker before user code is analyzed. Users MAY declare additional domain effects (see Requirement 7).

**Implementation:** `src/mvl/checker.rs` (effect registration pass; validated in `check_fn_decl`; replaces `VALID_EFFECT_NAMES` constant — see ADR-0034)

**Tests:** `tests/type_checker.rs::invalid_effect_name_rejected`, `tests/type_checker.rs::valid_effect_names_accepted`, `tests/type_checker.rs::caller_missing_callee_effect_rejected`, `tests/type_checker.rs::caller_declaring_effect_union_accepted`

| Effect | What it permits |
|--------|----------------|
| `Console` | Read/write stdin/stdout/stderr |
| `FileRead` | Read from filesystem |
| `FileWrite` | Write to filesystem |
| `FileDelete` | Delete from filesystem |
| `Net` | Network access (TCP, UDP, HTTP) |
| `DB` | Database operations |
| `ProcessSpawn` | Spawn external processes |
| `Random` | Non-deterministic random generation (PRNG) |
| `CryptoRandom` | Cryptographically secure random generation (OS CSPRNG) |
| `Clock` | Read system clock |
| `Env` | Read/write environment variables |
| `Log` | Write to log system |
| `Async` | Asynchronous operations |
| `Terminal` | Raw terminal control (cursor, colors, single-keypress input, screen clear) — distinct from `Console` (line-oriented I/O). Used by `std.tui` / future `pkg.tui` (#174) |

#### Scenario: File read without network

- GIVEN `fn load_config() -> Config ! FileRead`
- WHEN the function attempts `http_get("https://...")`
- THEN the compiler MUST reject: "requires `! Net` but function only declares `! FileRead`"

**Tests:** `tests/type_checker.rs::caller_missing_callee_effect_rejected`

#### Scenario: Multiple effects

- GIVEN `fn sync_data() -> Result<(), Error> ! Net, DB, Log`
- THEN the compiler MUST accept network calls, database calls, and logging within this function

**Tests:** `tests/type_checker.rs::caller_declaring_effect_union_accepted`

### Requirement 3: Capability-Based Security [SHOULD]

Effects SHOULD support parameterization for fine-grained access control:

- `! FileRead("/etc/config")` — can only read from this path
- `! Net("api.example.com")` — can only access this host
- `! DB("SELECT")` — read-only database access

#### Scenario: Path-restricted file access

- GIVEN `fn read_config() -> String ! FileRead("/etc/app/")`
- WHEN the function attempts `read_file("/etc/shadow")`
- THEN the compiler SHOULD reject: "file access outside declared capability `/etc/app/`"

### Requirement 4: Effect Composition [MUST]

Effects MUST compose. A function calling two effectful functions MUST declare the union of their effects.

**Implementation:** `src/mvl/checker.rs`

**Tests:** `tests/type_checker.rs::caller_declaring_effect_union_accepted`, `tests/type_checker.rs::caller_missing_callee_effect_rejected`

#### Scenario: Effect union

- GIVEN `fn a() -> X ! FileRead` and `fn b() -> Y ! Net`
- WHEN `fn c() -> Z ! FileRead, Net { a(); b(); }`
- THEN the compiler MUST accept

**Tests:** `tests/type_checker.rs::caller_declaring_effect_union_accepted`

#### Scenario: Missing effect in composition

- GIVEN `fn a() -> X ! FileRead` and `fn b() -> Y ! Net`
- WHEN `fn c() -> Z ! FileRead { a(); b(); }`
- THEN the compiler MUST reject: "calls `b` which requires `! Net`"

**Tests:** `tests/type_checker.rs::caller_missing_callee_effect_rejected`

### Requirement 5: Totality as Effect [MUST]

Non-terminating functions MUST be marked `partial`. Total functions (the default) MUST provably terminate. `partial` is semantically an effect — it declares that the function may not return.

This is Design Principle 4 ("Total by default"). Functions terminate unless they explicitly opt out with `partial`.

**Implementation:** `src/mvl/checker.rs`, `src/mvl/parser/ast.rs::Totality`

**Tests:** `tests/type_checker.rs::for_loop_in_total_function_accepted`, `tests/type_checker.rs::while_loop_in_total_function_rejected`, `tests/type_checker.rs::while_loop_in_implicit_total_function_rejected`, `tests/type_checker.rs::while_loop_in_partial_function_accepted`, `tests/type_checker.rs::partial_call_in_total_function_rejected`, `tests/compile_and_run.rs::safe_division_check_passes`, `tests/compile_and_run.rs::safe_division_runs_and_produces_expected_output` (#191), `tests/compile_and_run.rs::linked_list_check_passes`, `tests/compile_and_run.rs::linked_list_runs_and_produces_expected_output` (#194)

#### Scenario: Total function with bounded loop

- GIVEN `total fn sum(items: Array[Int]) -> Int { for item in items { ... } }`
- THEN the compiler MUST accept: `for` over array is bounded

**Tests:** `tests/type_checker.rs::for_loop_in_total_function_accepted`, `tests/type_checker.rs::totality_corpus_parses_and_checks`

#### Scenario: Total function with unbounded loop

- GIVEN `total fn loop() -> Never { while true { } }`
- THEN the compiler MUST reject: "unbounded loop in total function"

**Tests:** `tests/type_checker.rs::while_loop_in_total_function_rejected`, `tests/type_checker.rs::while_loop_in_implicit_total_function_rejected`

#### Scenario: Partial function

- GIVEN `partial fn server() -> Never ! Net { while true { accept(); } }`
- THEN the compiler MUST accept: explicitly partial

**Tests:** `tests/type_checker.rs::while_loop_in_partial_function_accepted`

### Requirement 6: Concurrency Effects [MUST]

Spawning tasks and sending/receiving on channels MUST be effects. The effect system MUST prevent data races by requiring appropriate reference capabilities on values crossing actor boundaries.

This is Design Principle 8 ("Actors, not threads"). No shared mutable state, no locks, no deadlocks — the concurrency model is a directed graph of actors communicating via capability-checked channels.

**Implementation:** `src/mvl/checker.rs`, `src/mvl/parser/ast.rs::Capability`

**Tests:** `tests/type_checker.rs::sending_ref_param_rejected`, `tests/type_checker.rs::sending_iso_param_accepted`, `tests/type_checker.rs::sending_val_param_accepted`, `tests/type_checker.rs::capabilities_corpus_parses_and_checks`

#### Scenario: Sending non-sendable type

- GIVEN a type with `ref` capability (mutable, not sendable)
- WHEN the code attempts `channel.send(ref_value)`
- THEN the compiler MUST reject: "`ref` capability cannot be sent across actor boundary; use `iso` or `val`"

**Tests:** `tests/type_checker.rs::sending_ref_param_rejected`

#### Scenario: Isolated value transfer

- GIVEN a value with `iso` capability (isolated, single reference)
- WHEN `channel.send(consume iso_value)`
- THEN the compiler MUST accept: ownership transferred via `consume`

**Tests:** `tests/type_checker.rs::sending_iso_param_accepted`

### Requirement 7: User-Defined Effects [MUST]

Users MUST be able to declare domain-specific effects using the `effect` keyword. The compiler MUST validate effect names against the set of declared effects (stdlib + user-defined), replacing the hardcoded `VALID_EFFECT_NAMES` constant.

**Implementation:** `src/mvl/checker.rs`, `std/effects.mvl` (ADR-0034 §Decision 1)

```mvl
// std/effects.mvl — canonical stdlib effects
pub effect Console
pub effect FileRead
pub effect Net
// ...

// User application code
pub effect PaymentGateway
pub effect AuditTrail

fn charge(amount: Int) -> Result[Unit, Error] ! PaymentGateway { ... }
```

Effect declarations are module-scoped and imported via the standard import mechanism. An undeclared effect name in a function signature MUST produce `CheckError::UnknownEffect`.

#### Scenario: Domain effect declared and used

- GIVEN `pub effect AuditTrail` declared in scope
- WHEN `fn record(entry: Entry) -> Unit ! AuditTrail { ... }` is compiled
- THEN the compiler MUST accept

#### Scenario: Undeclared effect name rejected

- GIVEN no declaration of `effect Foo` in scope
- WHEN `fn do_foo() -> Unit ! Foo { ... }` is compiled
- THEN the compiler MUST reject: "unknown effect `Foo` — declare it with `effect Foo`"

### Requirement 8: Effect Aliases [SHOULD]

Effects SHOULD support aliasing so that a named set can stand for a union of effects, reducing signature proliferation in application code.

**Implementation:** `src/mvl/checker.rs` declaration pass (ADR-0034 §Decision 2)

```mvl
// Application domain module
effect App = Log + Clock + DB + Net

fn handle_request(req: Request) -> Response ! App { ... }
// Equivalent to: -> Response ! Log + Clock + DB + Net
```

Aliases MUST expand to their constituent effects before type checking. Alias cycles (e.g., `effect A = B`, `effect B = A`) MUST be rejected at declaration time. Error messages SHOULD show the expanded effect set alongside the alias name.

#### Scenario: Alias expands at call site

- GIVEN `effect Observability = Log + Clock`
- WHEN `fn f() -> Unit ! Observability { log_info("x", {}); now(); }`
- THEN the compiler MUST accept (expanded effects are satisfied)

#### Scenario: Alias cycle rejected

- GIVEN `effect A = B` and `effect B = A`
- THEN the compiler MUST reject: "effect alias cycle: A → B → A"

### Requirement 9: Effect Masking [SHOULD]

Public functions in `std.*` modules SHOULD be able to declare a subset of their actual effects as the public contract, hiding implementation-detail effects from callers using a `masks` clause.

This formalises the Phase-A exemption currently expressed as a comment in `runtime/rust/src/stdlib/log.rs:23–27` (timestamp acquisition exempted from `! Clock`) and allows it to be expressed in MVL source. See ADR-0034 §Decision 3 and #839.

**Implementation:** `src/mvl/checker/decls.rs`, `src/mvl/parser/ast.rs` (ADR-0034 §Decision 3)

```mvl
// std/log.mvl — Clock is masked: callers only see ! Log
pub fn log_info(msg: String, fields: Map[String, String]) -> Unit ! Log
    masks Clock {
    let ts = now();           // ! Clock — not propagated to callers
    log_write(format_entry(Level::Info, msg, fields, ts));
}
```

The `masks` clause is restricted to `pub` functions in `std.*` modules. The compiler MUST verify that masked effects are actually used in the body (dead `masks` clause is an error). The compiler does NOT verify semantic safety of masking — trust is bounded to stdlib authors.

#### Scenario: Masked effect not visible to caller

- GIVEN `pub fn log_info(...) -> Unit ! Log masks Clock { ... }` in `std.log`
- WHEN `fn app() -> Unit ! Log { log_info("x", {}); }`
- THEN the compiler MUST accept (Clock not required at call site)

#### Scenario: Masked effect actually used

- GIVEN `pub fn log_info(...) -> Unit ! Log masks Clock { ... }` but body does not call `now()`
- THEN the compiler MUST reject: "masked effect `Clock` is unused in body"

#### Scenario: Masking outside std.* rejected

- GIVEN user code `fn sneaky() -> Unit ! Log masks Net { http_get("..."); }`
- THEN the compiler MUST reject: "`masks` is only permitted in `std.*` modules"
