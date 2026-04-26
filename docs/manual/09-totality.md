# 9. Totality and Termination

Functions are total by default ([Req 8](../requirements.md#req-8)). The compiler verifies that total functions always terminate and return a value.

## 9.1 Total Functions (Default)

```mvl
fn factorial(n: UInt where n <= 20) -> UInt {
    match n {
        0 => 1,
        n => n * factorial(n - 1),
    }
}
```

The compiler checks structural recursion: the recursive argument (`n - 1`) is strictly smaller than the input (`n`). If the compiler cannot prove termination, it rejects.

## 9.2 Structural Recursion

The compiler accepts recursion where the recursive call operates on a structurally smaller argument:

- Integer decreasing toward a base case
- List/array getting shorter (head/tail decomposition)
- Tree getting shallower (recursion on children)

```mvl
fn sum(items: Array[Int]) -> Int {
    match items {
        [] => 0,
        [head, ..tail] => head + sum(tail),   // tail is smaller — accepted
    }
}
```

## 9.3 Partial Functions

```mvl
partial fn repl() -> Never ! Console {
    while true {
        let input = readline()?;
        let output = eval(input);
        println(output);
    }
}
```

`partial` opts out of termination checking. Use for:
- Server loops
- REPLs
- Event loops
- Any intentionally non-terminating computation

`while` is only permitted in `partial` functions. In `total` functions, use `for` over a finite iterator.

## 9.4 The Totality Budget

| Construct | Permitted in `total` | Permitted in `partial` |
|-----------|---------------------|----------------------|
| `for x in iter` | Yes (bounded) | Yes |
| `while condition` | No | Yes |
| Structural recursion | Yes (decreasing) | Yes |
| General recursion | No | Yes |
| `loop` | Does not exist | — |

## 9.5 Why This Matters

A total function that type-checks is guaranteed to:
- Always return a value
- Never hang
- Never consume unbounded resources

This makes total functions safe to call in any context — including refinement type checking, compile-time evaluation, and safety-critical systems where non-termination is a defect.

The irony: the MVL parser itself had an infinite loop during development. A language that enforces termination on user code cannot guarantee the same for its own tooling — unless the compiler is itself written in MVL (Phase 3: self-hosting).
