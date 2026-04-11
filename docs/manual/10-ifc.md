# 10. Information Flow Control

Security labels track data provenance through the type system (Req 11). The compiler prevents secret leakage, injection attacks, and tainted data reaching trusted sinks.

## 10.1 The Security Lattice

```
Secret          (highest — keys, passwords, tokens)
   ↑
Tainted         (external — user input, network, files)
   ↑
Clean           (sanitized — validated through explicit check)
   ↑
Public          (lowest — safe for any channel)
```

Data flows **up** freely. Flowing **down** requires explicit declassification.

## 10.2 Labels on Types

```mvl
let api_key: Secret<String> = load_key();
let user_input: Tainted<String> = read_line();
let safe_name: Clean<String> = sanitize(user_input);
let message: Public<String> = "hello";
```

`Secret<String>` and `Public<String>` are different types. You cannot pass one where the other is expected (unless flowing up).

## 10.3 Automatic Labeling

Data from external sources is automatically `Tainted`:

| Source | Label |
|--------|-------|
| stdin | `Tainted` |
| HTTP request body/headers | `Tainted` |
| File contents | `Tainted` |
| Network responses | `Tainted` |
| Environment variables | `Tainted` |
| Database query results | `Tainted` |
| Process stdout | `Tainted` |

## 10.4 Declassification

### sanitize — Tainted → Clean

```mvl
fn sanitize_email(input: Tainted<String>) -> Result<Clean<Email>, ValidationError> {
    let trimmed = input.trim();
    if is_valid_email(trimmed) {
        Ok(sanitize(Email.parse(trimmed)))
    } else {
        Err(ValidationError.new("invalid email"))
    }
}
```

`sanitize()` is an explicit, auditable operation. It appears in assurance reports and is greppable.

### declassify — Secret → Public

```mvl
fn log_key_fingerprint(key: Secret<ApiKey>) -> () ! Log {
    let fingerprint = key.sha256_prefix(8);
    log.info("Key fingerprint: " + declassify(fingerprint));
}
```

`declassify()` is the nuclear option — it intentionally exposes secret data. Every call is tracked.

## 10.5 Compile-Time Enforcement

```mvl
fn log_message(msg: Public<String>) -> () ! Log { ... }

let secret: Secret<String> = "password123";
log_message(secret);
// COMPILE ERROR: cannot pass Secret<String> where Public<String> expected

let tainted: Tainted<String> = read_line();
sql_query("SELECT * WHERE name = " + tainted);
// COMPILE ERROR: cannot concatenate Clean<String> with Tainted<String>
```

## 10.6 OWASP Coverage

| OWASP Category | How MVL prevents it |
|----------------|-------------------|
| A01 Broken Access Control | Effect tracking — auth checks in type |
| A03 Injection | IFC — tainted input cannot reach query builder |
| A05 Security Misconfiguration | Effect tracking — config access declared |
| A07 Auth Failures | Secret labels — credentials tracked through types |
| A08 Software Integrity | IFC — untrusted data flows visible |
| A10 SSRF | IFC — tainted URLs cannot reach network calls |
