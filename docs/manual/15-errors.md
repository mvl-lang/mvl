# 15. Error Handling

MVL has one error handling mechanism: `Result<T, E>` ([Req 5](../requirements.md#req-5)). No exceptions, no panics (except truly unrecoverable), no sentinel values.

## 15.1 Result\<T, E\>

```mvl
type Result<T, E> = enum {
    Ok(T),
    Err(E),
}
```

Functions that can fail return `Result`. The error type is visible in the signature.

## 15.2 Propagation with ?

```mvl
fn load_user(id: UserId) -> Result<User, AppError> ! DB {
    let row = db.query("...", id)?;          // propagates DbError
    let user = parse_user(row)?;             // propagates ParseError
    Ok(user)
}
```

`?` converts the error type if the enclosing function's error type implements `From<InnerError>`.

## 15.3 Combinators

```mvl
let name = find_user(id)
    .map(|u| u.name)                         // transform Ok value
    .unwrap_or("unknown".to_string());       // default on Err

let config = read_config()
    .and_then(|text| parse(text))            // chain fallible operations
    .map_err(|e| AppError.from(e));          // transform error type
```

## 15.4 Error Types

Define domain errors as enums:

```mvl
type AppError = enum {
    NotFound(String),
    Unauthorized,
    DatabaseError(DbError),
    ValidationError(String),
}

impl Error for AppError {
    fn message(self) -> String {
        match self {
            NotFound(id) => format("not found: {}", id),
            Unauthorized => "unauthorized",
            DatabaseError(e) => e.message(),
            ValidationError(msg) => msg,
        }
    }

    fn source(self) -> Option<&Error> {
        match self {
            DatabaseError(e) => Some(&e),
            _ => None,
        }
    }
}
```

## 15.5 Option\<T\> for Absence

```mvl
fn find(items: Array<Int>, target: Int) -> Option<UInt> {
    for (i, item) in items.enumerate() {
        if item == target {
            return Some(i);
        }
    }
    None
}
```

`Option` is for "not found" / "absent." `Result` is for "something went wrong."

## 15.6 Panic

`panic` exists for truly unrecoverable errors — logic bugs, violated invariants that should be impossible:

```mvl
fn unreachable_branch() -> Never {
    panic("this should never happen");
}
```

`panic` terminates the program. It is not for expected errors — those use `Result`.

## 15.7 No Exceptions

There are no exceptions, no `try`/`catch`, no `throw`. Error paths are always visible in the function signature. You cannot ignore an error without explicitly handling it (via `match`, `?`, or a combinator).
