# 10. Information Flow Control

Security labels track data provenance through the type system ([Req 11](../requirements.md#req-11)). The compiler prevents secret leakage, injection attacks, and tainted data reaching observable functions.

## 10.1 User-Defined Labels

Labels are opaque types declared with `label`:

```mvl
label Secret
label Tainted
```

Labels wrap types: `Secret[String]` ≠ `String`. The type system rejects direct mismatches.

## 10.2 Labels on Types

```mvl
let api_key: Secret[String] = load_key();
let user_input: Tainted[String] = read_line();
let message: String = "hello";
```

`Secret[String]` and `String` are different types. You cannot pass one where the other is expected.

## 10.3 Label Propagation

All functions propagate labels unconditionally. Calling a function with a labeled argument yields a labeled result:

```mvl
fn trim(s: String) -> String { ... }

let input: Tainted[String] = read_line();
let trimmed: Tainted[String] = trim(input);  // label propagates automatically
```

## 10.4 The `relabel` Keyword

`relabel` is the **only** IFC keyword beyond `label`. It is the sole mechanism for crossing label boundaries:

```mvl
// Remove a label (trust boundary)
fn handle(input: Tainted[String]) -> String {
    relabel trust(input, "XSS-001")
}

// Add a label (classify)
fn protect(data: String) -> Secret[String] {
    relabel classify(data, "PII-001")
}
```

Every `relabel` call includes an audit tag. `grep "relabel"` finds every trust boundary crossing.

## 10.5 Compile-Time Enforcement

```mvl
fn log_message(msg: String) -> Unit ! Log { ... }

let secret: Secret[String] = load_key();
log_message(secret);
// COMPILE ERROR: cannot pass Secret[String] where String expected
```

## 10.6 Implicit Flow Detection

The effect system detects information leaks through control flow:

```mvl
fn check(flag: Secret[Bool]) -> Unit ! Console {
    if flag {
        println("branch taken")  // COMPILE ERROR: implicit flow
    }
}
```

Any effectful function call (`! Console`, `! Log`, etc.) inside a branch controlled by a labeled condition is an implicit flow violation. The effect system provides observability information — no dedicated `sink` keyword needed.

## 10.7 OWASP Coverage

| OWASP Category | How MVL prevents it |
|----------------|-------------------|
| A01 Broken Access Control | Effect tracking — auth checks in type |
| A03 Injection | IFC — tainted input cannot reach query builder |
| A05 Security Misconfiguration | Effect tracking — config access declared |
| A07 Auth Failures | Secret labels — credentials tracked through types |
| A08 Software Integrity | IFC — untrusted data flows visible |
| A10 SSRF | IFC — tainted URLs cannot reach network calls |
