---
domain: language
version: 0.1.0
status: draft
date: 2026-04-11
---

# 002 — Effect System

The MVL effect system covers Requirement 7 (effect tracking) and supports Requirement 9 (data race freedom) and Requirement 8 (termination). Every side effect MUST be declared in the function signature. Pure is the default.

## Philosophy

A function signature should tell the full truth about what the function does. If a function reads a file, it says so. If it's pure, the absence of effects proves it. Effects are the mechanism that makes Requirement 3 of the OWASP Top 10 (least privilege) a compile-time guarantee.

**Origin:** Koka (Leijen, 2014) for algebraic effects. Haskell IO monad (1992) for the principle. E language (Miller, 1997) for capability-based security.

## Requirements

### Requirement 1: Effect Declaration [MUST]

Functions with side effects MUST declare them in the signature using `! Effect` syntax. Functions without effect declarations MUST be pure — the compiler MUST reject any side-effecting operation in a pure function.

**Implementation:** `src/mvl/checker/mod.rs`

**Tests:** `tests/type_checker.rs::pure_function_calling_effectful_rejected`, `tests/type_checker.rs::effectful_function_with_correct_declaration_accepted`, `tests/type_checker.rs::caller_missing_callee_effect_rejected`

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

Effects MUST be fine-grained, not a single `IO` bucket. The minimum set of effect categories:

**Implementation:** `src/mvl/checker/mod.rs` (constant `VALID_EFFECT_NAMES`; validated in `check_fn_decl`)

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
| `Random` | Non-deterministic random generation |
| `Clock` | Read system clock |
| `Env` | Read/write environment variables |
| `Log` | Write to log system |
| `Async` | Asynchronous operations |

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

**Implementation:** `src/mvl/checker/mod.rs`

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

**Implementation:** `src/mvl/checker/mod.rs`, `src/mvl/parser/ast.rs::Totality`

**Tests:** `tests/type_checker.rs::for_loop_in_total_function_accepted`, `tests/type_checker.rs::while_loop_in_total_function_rejected`, `tests/type_checker.rs::while_loop_in_implicit_total_function_rejected`, `tests/type_checker.rs::while_loop_in_partial_function_accepted`, `tests/type_checker.rs::partial_call_in_total_function_rejected`

#### Scenario: Total function with bounded loop

- GIVEN `total fn sum(items: Array<Int>) -> Int { for item in items { ... } }`
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

**Implementation:** `src/mvl/checker/mod.rs`, `src/mvl/parser/ast.rs::Capability`

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
