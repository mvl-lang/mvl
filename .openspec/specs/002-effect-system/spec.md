---
domain: language
version: 0.2.0
status: draft
date: 2026-05-17
---

# 002 — Effect System

The MVL effect system tracks side effects for security audit and functional safety. Every side effect MUST be declared in the function signature. Pure is the default. Effects enable compile-time enforcement of least privilege (OWASP A01).

## Philosophy

A function signature tells the full truth about what the function can do. If a function reads a file, it says so. If it's pure, the absence of effects proves it. The signature IS the threat model.

**Purpose:** Security tracking and functional safety — not abstraction or composition. Effects propagate; they do not discharge. Closest language: Austral (capability tracking).

## Requirements

### Requirement 1: Effect Declaration [MUST]

Functions with side effects MUST declare them in the signature using `! Effect` syntax. Functions without effect declarations MUST be pure — the compiler MUST reject any side-effecting operation in a pure function.

**Implementation:** `src/mvl/checker.rs`, `src/mvl/checker/calls.rs`

#### Scenario: Pure function attempts I/O

- GIVEN `fn add(a: Int, b: Int) -> Int { println("adding"); a + b }`
- THEN the compiler MUST reject: "function `add` has no effect declaration but calls `println` which requires `! Console`"

#### Scenario: Effect declared correctly

- GIVEN `fn greet(name: String) -> Unit ! Console { println("Hello"); }`
- THEN the compiler MUST accept

#### Scenario: Effect propagation

- GIVEN `fn a() -> Int ! FileRead { read_config()? }` and `fn b() -> Int { a() }`
- THEN the compiler MUST reject `b`: "calls `a` which requires `! FileRead` but `b` declares no effects"

### Requirement 2: Effects Declared in MVL Source [MUST]

Effects MUST be declared in MVL source, not hardcoded in the compiler. Base effects live in `std/effects.mvl`. User code MAY declare domain-specific effects that extend the hierarchy.

**Implementation:** `src/mvl/checker.rs`, `src/mvl/checker/effects.rs`

The compiler uses dual-pass compilation:
1. **Parse pass:** Parse all files, collect `EffectDecl` nodes (no validation)
2. **Resolve pass:** Build hierarchy, validate parents exist, detect cycles
3. **Check pass:** Type-check with complete hierarchy

#### Scenario: Unknown effect

- GIVEN `fn foo() ! UnknownEffect { }`
- WHEN `UnknownEffect` is not declared anywhere
- THEN the compiler MUST reject: "unknown effect `UnknownEffect`"

#### Scenario: User-defined domain effect

- GIVEN `effect Billing > DB + Log` in user code
- AND `fn charge(amount: Int) ! Billing { db_insert(...); log_debug(...); }`
- THEN the compiler MUST accept: Billing subsumes DB and Log

### Requirement 3: Fine-Grained Effects [MUST]

Effects MUST be fine-grained, not a single `IO` bucket. The base effects are:

| Effect | What it permits |
|--------|----------------|
| `Clock` | Read system clock |
| `Console` | Read/write stdin/stdout/stderr |
| `FileRead` | Read from filesystem |
| `FileWrite` | Write to filesystem |
| `FileDelete` | Delete from filesystem |
| `Net` | Network access (TCP, UDP, HTTP) |
| `DB` | Database operations |
| `ProcessSpawn` | Spawn external processes |
| `Env` | Read/write environment variables |
| `Random` | Non-deterministic random generation |
| `Spawn` | Create actors |
| `Send` | Send on channels |
| `Recv` | Receive on channels (blocking) |

#### Scenario: File read without network

- GIVEN `fn load_config() -> Config ! FileRead`
- WHEN the function attempts `http_get("https://...")`
- THEN the compiler MUST reject: "requires `! Net` but function only declares `! FileRead`"

### Requirement 4: Effect Subsumption [MUST]

Effects MUST support subsumption. If effect `A` subsumes effect `B` (`A > B`), declaring `! A` satisfies any `! B` requirement. Subsumption is transitive.

**Implementation:** `src/mvl/checker.rs`

Syntax:
```
effect_decl = "effect" IDENT [ ">" IDENT ( "+" IDENT )* ] ;
```

#### Scenario: Log subsumes Clock

- GIVEN `effect Log > Clock` in std/effects.mvl
- AND `fn now() -> Instant ! Clock`
- AND `fn log_debug(msg: String) -> Unit ! Log { let ts = now(); ... }`
- THEN the compiler MUST accept: `Log > Clock` means `! Log` satisfies `! Clock`

#### Scenario: IO subsumes multiple effects

- GIVEN `effect IO > Console`, `effect IO > FileRead`, `effect IO > Net` in std/effects.mvl
- AND `fn main() ! IO { println("hello"); let cfg = read_file("x")?; }`
- THEN the compiler MUST accept: `! IO` satisfies both `! Console` and `! FileRead`

#### Scenario: Transitive subsumption

- GIVEN `effect IO > Log` and `effect Log > Clock`
- AND `fn foo() ! IO { let ts = now(); }`
- THEN the compiler MUST accept: `IO > Log > Clock` means `! IO` satisfies `! Clock`

#### Scenario: Subsumption cycle rejected

- GIVEN `effect A > B` and `effect B > A`
- THEN the compiler MUST reject: "effect subsumption cycle detected: A > B > A"

### Requirement 5: Effect Composition [MUST]

Effects MUST compose. A function calling two effectful functions MUST declare effects that satisfy both callees (either directly or via subsumption).

**Implementation:** `src/mvl/checker.rs`

#### Scenario: Effect union

- GIVEN `fn a() -> X ! FileRead` and `fn b() -> Y ! Net`
- WHEN `fn c() -> Z ! FileRead + Net { a(); b(); }`
- THEN the compiler MUST accept

#### Scenario: Subsumption satisfies composition

- GIVEN `fn a() -> X ! FileRead` and `fn b() -> Y ! Net`
- AND `effect IO > FileRead` and `effect IO > Net`
- WHEN `fn c() -> Z ! IO { a(); b(); }`
- THEN the compiler MUST accept

### Requirement 6: Capability-Based Restriction [SHOULD]

Effects SHOULD support parameterization for fine-grained access control:

- `! FileRead("/etc/config")` — can only read from this path
- `! Net("api.example.com")` — can only access this host
- `! DB("SELECT")` — read-only database access

Path parameters use prefix matching: `! FileRead("/etc")` satisfies `! FileRead("/etc/config.toml")`.

#### Scenario: Path-restricted file access

- GIVEN `fn read_config() -> String ! FileRead("/etc/app/")`
- WHEN the function attempts `read_file("/etc/shadow")`
- THEN the compiler SHOULD reject: "file access outside declared capability `/etc/app/`"

### Requirement 7: Concurrency Effects [MUST]

Spawning actors, sending messages, and receiving messages MUST be separate effects. The `Async` effect is removed in favor of fine-grained tracking.

| Effect | Security Concern |
|--------|-----------------|
| `Spawn` | Resource exhaustion (DoS) |
| `Send` | Data exfiltration, trust boundary crossing |
| `Recv` | Blocking, waiting |

**Implementation:** `std/effects.mvl`

#### Scenario: Spawn without send

- GIVEN `fn start_worker() ! Spawn { spawn(worker); }`
- WHEN the function attempts `ch.send(data)`
- THEN the compiler MUST reject: "requires `! Send` but function only declares `! Spawn`"

### Requirement 8: No FFI Effect Hiding [MUST]

Effects MUST NOT be hidden in FFI implementations. If a builtin function uses an effect internally, it MUST either:
1. Declare the effect explicitly, OR
2. Have the effect subsumed by a declared effect

#### Scenario: Log uses Clock via subsumption

- GIVEN `effect Log > Clock`
- AND `builtin fn log_debug(msg: String) -> Unit ! Log`
- THEN the implementation MAY call `now()` because `Log > Clock`
- AND the caller sees only `! Log` (Clock is subsumed, not hidden)

## std/effects.mvl

The canonical effect hierarchy:

```mvl
// std/effects.mvl

// === Base Effects (primitives) ===
effect Clock
effect Console  
effect FileRead
effect FileWrite
effect FileDelete
effect Net
effect DB
effect ProcessSpawn
effect Env
effect Random

// === Concurrency ===
effect Spawn
effect Send
effect Recv

// === Composite Effects (subsumption) ===
effect Log > Clock
effect CryptoRandom > Random

effect IO > Clock
effect IO > Console
effect IO > FileRead
effect IO > FileWrite
effect IO > FileDelete
effect IO > Net
effect IO > DB
effect IO > ProcessSpawn
effect IO > Env
effect IO > Log

effect Actor > Spawn
effect Actor > Send
effect Actor > Recv
```

## Security Rationale

Effects track the attack surface. The signature IS the threat model:

| Effect | Security Concern |
|--------|------------------|
| `Console` | Data exfiltration (stdout), injection (stdin) |
| `FileRead` | Read sensitive files, path traversal |
| `FileWrite` | Overwrite files, plant malware |
| `FileDelete` | Data destruction |
| `Net` | Exfiltration, C2, SSRF |
| `DB` | SQL injection, data leakage |
| `ProcessSpawn` | Arbitrary code execution |
| `Env` | Read secrets, config manipulation |
| `Random` | Predictable values → crypto weakness |
| `Clock` | Timing attacks |
| `Spawn` | Resource exhaustion |
| `Send` | Trust boundary crossing |
| `Recv` | Blocking/DoS |

A pure function (no `!`) is maximally sandboxed: it cannot exfiltrate data, cannot access files, cannot hit network. Pure by default, effects are explicit opt-in to danger.

## ADR

ADR-0035 — Effect System Upgrade
