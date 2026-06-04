# Pattern 003: Error Message Exposure (Display vs Debug + IFC)

## Summary

Never expose raw error messages to external users. Error internals (SQL queries, file paths,
stack traces, connection strings) are security-sensitive. MVL enforces this boundary at compile
time through IFC labels.

## Reference implementation

All `std.*` error types (IoError, NetError, HttpError, etc.).

## The pattern

Every error type provides two rendering methods:

| Method | Returns | Purpose | Safe for HTTP? |
|--------|---------|---------|----------------|
| `user_message()` | `String` | External users — sanitized, generic | Yes |
| `debug_message()` | `Secret[String]` | Internal logs — full detail, IFC-wrapped | No (compile error) |

## Code pattern

```mvl
use std.io.{IoError}
use std.log.{Logger}

fn handle_request(logger: Logger) -> String ! Log {
    match do_work() {
        Ok(result) => result,
        Err(e) => {
            // Full detail to internal logs (Secret accepted by audit sinks)
            let debug: Secret[String] = e.debug_message();

            // Safe message to external user
            e.user_message()
        },
    }
}
```

## How it works

1. `user_message()` returns a plain `String` — safe for HTTP responses, CLI output, user-facing APIs
2. `debug_message()` wraps full error detail in `Secret[String]` via `relabel classify(msg, "ERR-DEBUG")`
3. IFC enforcement: `Secret[String]` cannot flow to public sinks (`println`, HTTP response bodies)
4. The audit tag `ERR-DEBUG` records that the Secret originates from error diagnostics

## Anti-patterns

```mvl
// WRONG: exposes internal error detail to user
http_response(500, net_error_msg(e))

// WRONG: compile error — Secret[String] cannot flow to String response
http_response(500, e.debug_message())

// WRONG: leaks file paths to external user
Err(IoError::NotFound) => http_response(404, "File not found: ".concat(path))

// CORRECT: safe message to user
http_response(500, e.user_message())
```

## Adding to custom error types

Follow the same pattern for application-level errors:

```mvl
pub type AppError = enum {
    Database(IoError),
    Validation { field: String, msg: String },
}

pub fn AppError::user_message(self) -> String {
    match self {
        AppError::Database(e) => e.user_message(),
        AppError::Validation { field: f, msg: _ } => "invalid ".concat(f),
    }
}

pub fn AppError::debug_message(self) -> Secret[String] {
    let msg: String = match self {
        AppError::Database(e) => "AppError::Database: ".concat(relabel release(e.debug_message(), "ERR-NEST")),
        AppError::Validation { field: f, msg: m } => "AppError::Validation: ".concat(f).concat(": ").concat(m),
    };
    relabel classify(msg, "ERR-DEBUG")
}
```

## Why this matters

| Error type | What leaks | Attack enabled |
|------------|-----------|----------------|
| SQL errors | Schema, queries | Injection refinement |
| File errors | Paths, structure | Directory traversal |
| Auth errors | Usernames | Enumeration attacks |
| Stack traces | Architecture | Targeted exploits |
| Connection strings | Credentials, hosts | Direct access |

## Related

- ADR-0032 — Structured error enums
- Spec 003, Req 5 — Error types with Secret fields
- Spec 003, Req 6 — Secret rejection at public sinks
- `std/error.mvl` — Convention module documentation
- Pattern 001 — Layered Configuration (IFC boundary crossing reference)
