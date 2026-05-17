# 8. Effect System

Every side effect MUST be declared in the function signature ([Req 7](../requirements.md#req-7)). Pure is the default. The signature IS the threat model.

## 8.1 Effect Declaration

```mvl
fn greet(name: String) -> Unit ! Console {
    println("Hello, " + name);
}

fn load(path: Path) -> Result[String, IOError] ! FileRead {
    read_to_string(path)
}
```

A function without `!` is pure — the compiler rejects any side-effecting operation.

## 8.2 Declaring Effects (std/effects.mvl)

Effects are declared in MVL source, not hardcoded in the compiler (ADR-0035). Base effects live in `std/effects.mvl`. User code may declare domain-specific effects.

### Base Effects

| Effect | Permits | Security Concern |
|--------|---------|-----------------|
| `Clock` | Read system clock | Timing attacks |
| `Console` | stdin/stdout/stderr | Exfiltration, injection |
| `FileRead` | Read from filesystem | Path traversal, sensitive files |
| `FileWrite` | Write to filesystem | Overwrite, malware |
| `FileDelete` | Delete from filesystem | Data destruction |
| `Net` | Network access (TCP, UDP, HTTP) | Exfiltration, C2, SSRF |
| `DB` | Database operations | SQL injection, data leakage |
| `ProcessSpawn` | Spawn external processes | Arbitrary code execution |
| `Env` | Read/write environment variables | Secret leakage |
| `Random` | Non-deterministic random generation | Predictable values |
| `Terminal` | Raw terminal control | N/A |

### Concurrency Effects

| Effect | Security Concern |
|--------|-----------------|
| `Spawn` | Resource exhaustion (DoS) |
| `Send` | Data exfiltration, trust boundary crossing |
| `Recv` | Blocking/DoS |

### Composite Effects (Subsumption)

| Effect | Subsumes |
|--------|---------|
| `Log` | `Clock` |
| `CryptoRandom` | `Random` |
| `IO` | `Console + FileRead + FileWrite + FileDelete + Net + DB + ProcessSpawn + Env + Log` |
| `Actor` | `Spawn + Send + Recv` |

Effects are fine-grained — not a single `IO` bucket. A function that reads files but doesn't touch the network declares `! FileRead`.

## 8.3 Declaring Custom Effects

Any MVL file may declare effects. Use `>` to specify subsumption (the new effect satisfies any requirement for its parents):

```mvl
effect Billing > DB + Log    // Billing subsumes DB and Log
```

This enables:

```mvl
fn charge(amount: Int) -> Unit ! Billing {
    db_insert(...)    // DB satisfied by Billing
    log_debug(...)    // Log satisfied by Billing
}
```

The compiler uses **dual-pass compilation**: all effect declarations are collected across all files before any validation, so forward references are allowed.

## 8.4 Effect Subsumption

If `A` subsumes `B` (`A > B`), declaring `! A` satisfies any `! B` requirement. Subsumption is transitive.

```mvl
// std/effects.mvl:
// effect Log > Clock
// effect IO > Log

fn now() -> Instant ! Clock { ... }
fn log_debug(msg: String) -> Unit ! Log { let ts = now(); ... }  // Log > Clock: OK
fn main() -> Unit ! IO { log_debug("ready"); }                    // IO > Log > Clock: OK
```

## 8.5 Effect Propagation

Effects propagate through the call chain:

```mvl
fn f() -> Int ! FileRead {
    g()                              // g requires FileRead
}

fn f_wrong() -> Int {
    g()                              // COMPILE ERROR: g requires ! FileRead but f is pure
}
```

## 8.6 Multiple Effects

Combine effects with `+`:

```mvl
fn sync_data() -> Result[Unit, Error] ! Net + DB + Log {
    let data = fetch_remote()?;      // ! Net
    store_local(data)?;              // ! DB
    log.info("synced");              // ! Log
    Ok(())
}
```

Or use a composite effect that subsumes all three:

```mvl
fn sync_data() -> Result[Unit, Error] ! IO {
    // IO subsumes Net, DB, Log
}
```

## 8.7 Purity Guarantees

Pure functions (no `!` declaration) are:
- Cannot perform I/O, network calls, filesystem access
- Cannot access global mutable state (there is none)
- Cannot call effectful functions
- Referentially transparent — same inputs always produce same outputs
- Safe to memoize, parallelize, and reorder

## 8.8 Testing with Effects

Effects make testing straightforward. The effect annotation is the contract:

```mvl
fn process(path: String) -> Result[Data, Error] ! FileRead {
    let content = read_file(path)?;
    parse(content)
}
```

Pure functions are trivially testable — no mocking needed. See [Chapter 16: Standard Library](16-stdlib.md) for stdlib test helpers.
