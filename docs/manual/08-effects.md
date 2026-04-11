# 8. Effect System

Every side effect MUST be declared in the function signature (Req 7). Pure is the default.

## 8.1 Effect Declaration

```mvl
fn greet(name: String) -> () ! Console {
    println("Hello, " + name);
}

fn load(path: Path) -> Result<String, IOError> ! FileRead {
    read_to_string(path)
}
```

A function without `!` is pure — the compiler rejects any side-effecting operation.

## 8.2 Effect Categories

| Effect | Permits |
|--------|---------|
| `Console` | stdin/stdout/stderr |
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

Effects are fine-grained — not a single `IO` bucket. A function that reads files but doesn't touch the network declares `! FileRead`, not `! IO`.

## 8.3 Effect Propagation

Effects propagate through the call chain. If `f` calls `g` and `g` has effect `E`, then `f` must also declare `E`:

```mvl
fn f() -> Int ! FileRead {
    g()                              // g requires FileRead
}

fn f_wrong() -> Int {
    g()                              // COMPILE ERROR: g requires FileRead but f is pure
}
```

## 8.4 Multiple Effects

```mvl
fn sync_data() -> Result<(), Error> ! Net, DB, Log {
    let data = fetch_remote()?;      // ! Net
    store_local(data)?;              // ! DB
    log.info("synced");              // ! Log
    Ok(())
}
```

## 8.5 Purity Guarantees

Pure functions (no `!` declaration) have strong guarantees:
- Cannot perform I/O
- Cannot access global mutable state (there is none)
- Cannot call effectful functions
- Are referentially transparent — same inputs always produce same outputs
- Are safe to memoize, parallelize, and reorder

## 8.6 Testing with Effects

Effects make testing trivial. Stub the effect at the call site:

```mvl
fn process(fs: &FileSystem) -> Result<Data, Error> ! FileRead {
    let content = fs.read("data.json")?;
    parse(content)
}

// Test: pass a stub filesystem
fn test_process() {
    let stub = StubFS { files: {"data.json": "{\"key\": 1}"} };
    let result = process(&stub);
    assert_eq(result, Ok(expected_data));
}
```

No mock framework needed. See [Chapter 16: Standard Library](16-stdlib.md) for stdlib test helpers.
