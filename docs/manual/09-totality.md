# 9. Totality and Termination

Functions are total by default ([Req 8](../requirements.md#req-8)). The compiler verifies that total functions always terminate — `total` means *terminates*, not *always returns a value by reaching the end of its body normally*. A total function may still call `panic` and abort; see §9.5.

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

A bare `while` (no measure) is only permitted in `partial` functions. In `total` functions, either use `for` over a finite iterator, or give the `while` a `decreases` measure — see §9.4.

## 9.4 The Totality Budget

| Construct | Permitted in `total` | Permitted in `partial` |
|-----------|---------------------|----------------------|
| `for x in iter` | Yes (bounded) | Yes |
| `while condition` (bare) | No | Yes |
| `while condition decreases m` | Yes, if the measure is proved to strictly decrease | Yes |
| Structural recursion | Yes (decreasing) | Yes |
| General recursion | No | Yes |
| `loop` | Does not exist | — |

`while … decreases m` in a total function is checked the same way structural recursion is: the compiler must prove `m` is bounded below and strictly decreases every iteration, or it rejects the function (`decreases` measures the checker cannot analyse are rejected, not silently accepted — see #2211).

```mvl
fn count_down(n: Int where self >= 0) -> Int {
    let i: ref Int = n;
    while i > 0 decreases i {
        i = i - 1;
    }
    i
}
```

## 9.5 Why This Matters

A total function that type-checks is guaranteed to:
- Never hang
- Either return a value or terminate by aborting (e.g. via `panic`) — it never runs forever

It is **not** guaranteed to run in bounded time or bounded memory: `total` means *terminates eventually*, not *terminates quickly*. `fn fib(n: Int where self >= 0) -> Int { if n <= 1 { n } else { fib(n - 1) + fib(n - 2) } }` is total and provably terminating, and `fib(60)` will not finish in practice — no pass in the compiler checks allocation or step count. If a bound on resource use matters, it has to be argued separately (e.g. via a refinement on the input size); Req 8 only rules out *never* finishing.

`total` is a claim about *termination*, not about *always producing a value by falling off the end of the body*. `panic` (and `exit`, `assert`, `assert_eq`, `assert_ne`) terminate — they abort rather than hang — so they are implicitly total and callable from any total function:

```mvl
fn unreachable_branch() -> Never {
    panic("this should never happen");
}
```

This is intentional, not a loophole: Req 8 exists to rule out non-termination, and an abort is not non-termination. Domain partiality — division by zero, out-of-bounds indexing, and similar runtime failure modes — is [Req 10](../requirements.md#req-10)'s job, enforced through refinement types (e.g. `fn safe_divide(a: Float, b: Float where b != 0.0) -> Float`), not Req 8's.

`Never` (the return type of `panic` and similar always-aborting functions) is the *bottom type*: it has no values and is compatible with any expected type. In a branch (`if`/`match`), an arm typed `Never` contributes nothing to the branches' combined type — the other arm's type wins, regardless of which arm is written first:

```mvl
fn unwrap_or_die(x: Option[Int]) -> Int {
    match x {
        None => panic("expected a value"),  // Never — contributes nothing
        Some(v) => v,                        // Int — this is the match's type
    }
}
```

This makes total functions safe to call in any context — including refinement type checking, compile-time evaluation, and safety-critical systems where *hanging* is a defect. (An explicit `panic` is not silent: it is a deliberate abort, visible in the source and, at runtime, in the failure message.)

The irony: the MVL parser itself had an infinite loop during development. A language that enforces termination on user code cannot guarantee the same for its own tooling — unless the compiler is itself written in MVL (Phase 3: self-hosting).
