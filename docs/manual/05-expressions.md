# 5. Expressions

Everything that produces a value is an expression. MVL is expression-oriented — `if`, `match`, and blocks are all expressions.

## 5.1 Literals

See [Section 1.4](01-lexical.md#14-literals).

## 5.2 Variable References

```mvl
x                                   // read a binding
```

## 5.3 Field Access and Method Calls

```mvl
point.x                             // field access
point.distance(other)               // method call
items.len()                         // method call
```

## 5.4 Function Calls

```mvl
add(1, 2)                           // by name
sort::<Int>(items)                   // with explicit type parameter (turbofish)
```

## 5.5 Operators

See [Chapter 19: Operators and Precedence](19-operators.md).

```mvl
a + b                               // arithmetic
a == b                              // comparison
a && b                              // logical
!a                                  // negation
```

No operator overloading. `+` means numeric addition, always. Use named methods for domain operations.

## 5.6 If Expressions

```mvl
let max = if a > b { a } else { b };
```

Both branches MUST have the same type. `else` is required when used as an expression.

## 5.7 Match Expressions

```mvl
let name = match user {
    Some(u) => u.name,
    None => "anonymous".to_string(),
};
```

See [Chapter 6: Pattern Matching](06-patterns.md).

## 5.8 Block Expressions

```mvl
let result = {
    let a = compute_a();
    let b = compute_b();
    a + b                            // last expression is the block's value
};
```

## 5.9 Propagation Operator (?)

```mvl
fn load_config() -> Result<Config, Error> ! FileRead {
    let text = read_to_string("config.toml")?;  // propagates Err
    let config = parse(text)?;                   // propagates Err
    Ok(config)
}
```

`?` works on both `Result<T, E>` and `Option<T>`:
- On `Result`: if `Err`, return the error to the caller
- On `Option`: if `None`, return `None` to the caller

The enclosing function's return type MUST be compatible.

## 5.10 Ownership Expressions

```mvl
move value                           // transfer ownership
consume isolated_value               // transfer isolated capability ([Req 9](../requirements.md#req-9))
```

See [Chapter 7: Ownership and Borrowing](07-ownership.md).

## 5.11 Security Expressions ([Req 11](../requirements.md#req-11))

```mvl
declassify(secret_value)             // Secret → Public (auditable)
sanitize(tainted_input)              // Tainted → Clean (auditable)
```

Both are greppable and show up in assurance reports. See [Chapter 10: IFC](10-ifc.md).

## 5.12 Lambdas

```mvl
let double = |x: Int| -> Int { x * 2 };
items.map(|x| x + 1)
items.filter(|x| x > 0)
```

Lambdas have **immutable captures only**. Mutable closures are banned (Req 7 — effects must be declared, not hidden in closures). Lambda types include effects: `fn(Int) -> Int ! Console`.

## 5.13 Struct Construction

```mvl
let p = Point { x: 1.0, y: 2.0 };
let user = User { name: name, age: age };
```

All fields MUST be initialized. No default values — the LLM generates them explicitly.
