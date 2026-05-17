# The Eleven Requirements

**The compiler verifies these. Code that compiles satisfies all eleven.**

Derived from the convergence of formal methods (Curry-Howard, Hoare, Girard, Plotkin & Pretnar, Martin-Löf, Denning) and safety-critical industrial practice (MISRA C, DO-178C, IEC 61508). Theory asked "what can a compiler prove?" Practice asked "what kills people when unproven?" Same answer.

Requirements 1–7 were always needed. Requirements 8–11 were known for decades but considered impractical — the annotation burden was too high for human developers. When LLMs generate all code, that burden is zero.

---

## Core Requirements (1–7)

### Req 1: Type Safety {#req-1}

**Algebraic data types — no impossible states.**

The type system MUST support sum types (enums) and product types (structs) as first-class constructs. All domain states MUST be representable. Impossible states MUST be unrepresentable.

**If dropped:** Catastrophic — can't reason about anything. Every downstream requirement depends on this.

**Origin:** Curry-Howard correspondence — types are propositions, programs are proofs. Curry (1934 observation), Howard (1969 ms, pub. 1980). Algebraic data types from Hope (Burstall, MacQueen, Sannella, 1980), adopted into Standard ML, OCaml, Haskell, Rust.

**Industrial validation:** MISRA C Rule 10.x (type discipline), DO-178C verification conditions.

```mvl
// Sum type — exactly these states, nothing else
type Shape = enum {
    Circle(Float64),
    Rect(Float64, Float64),
}

// Match MUST be exhaustive — compiler rejects if a variant is missing
match shape {
    Circle(r) => pi() * r * r,
    Rect(w, h) => w * h,
    // Adding a variant forces ALL match sites to update — compile error
}
```

**What it eliminates:** Type confusion, invalid state construction, unchecked casts, stringly-typed APIs.

**Manual:** [Types](manual/02-types.md), [Pattern Matching](manual/06-patterns.md)

---

### Req 2: Memory Safety {#req-2}

**No use-after-free, no buffer overflow, no undefined behavior.**

The compiler MUST guarantee memory safety without a garbage collector. Ownership and borrowing rules prevent use-after-free, double-free, dangling pointers, and buffer overflows at compile time.

**If dropped:** Catastrophic — undefined behavior destroys all other guarantees. 70% of Microsoft CVEs and ~70% of Chromium security bugs are memory safety issues.

**Origin:** Emerged from C's footgun history (1970s–2010s). Formalized as ownership + borrowing in Rust (2010s), building on linear types (Girard, 1987).

**Industrial validation:** MISRA C Chapters 18–22, Microsoft Security Response Center data (2019), Chromium memory safety statistics.

```mvl
let a = create_buffer();
let b = move a;           // ownership transferred
// a is no longer valid
// println(a);            // COMPILE ERROR: use after move

let data = [1, 2, 3];
let x = data.get(10);    // returns Option[Int], not a segfault
```

**What it eliminates:** Buffer overflows, use-after-free, double-free, dangling pointers, undefined behavior.

**Manual:** [Ownership and Borrowing](manual/07-ownership.md)

---

### Req 3: Totality (Exhaustive Matching) {#req-3}

**All cases handled — the compiler rejects incomplete logic.**

Every `match` expression MUST be exhaustive. The compiler MUST reject if any variant of a sum type is unhandled. Adding a variant to an enum forces every match site to be updated.

**If dropped:** Severe — types become documentation, not guarantees. Unhandled cases become runtime crashes.

**Origin:** Total functions and exhaustive matching from Hope (Burstall et al., 1980), adopted into Standard ML. Martin-Löf type theory (1972).

**Industrial validation:** MISRA C Rule 16.x (switch completeness), DO-178C MC/DC coverage requirements.

```mvl
type Color = enum { Red, Green, Blue }

match color {
    Red => "red",
    Green => "green",
    // COMPILE ERROR: non-exhaustive match — Blue not handled
}
```

**What it eliminates:** Unhandled cases, default-branch bugs, silent fallthrough, missing enum coverage.

**Manual:** [Pattern Matching](manual/06-patterns.md)

---

### Req 4: Null Elimination {#req-4}

**`Option[T]` instead of null — absence is in the type, not the value.**

The type system MUST NOT have null, nil, or undefined. Absence MUST be represented by `Option[T]` (either `Some(value)` or `None`). Accessing the inner value MUST require pattern matching.

**If dropped:** Severe — Hoare's billion-dollar mistake. Every reference becomes potentially invalid. Null checks are discipline, not proof.

**Origin:** Hoare invented null in 1965, called it "my billion dollar mistake" at QCon 2009. `Option` types from Standard ML (1990), OCaml, Haskell (`Maybe`), Rust.

**Industrial validation:** Cross-industry — null pointer exceptions are the single most common runtime error class.

```mvl
fn find_user(id: UserId) -> Option[User] ! DB {
    db.query_optional("...", id)
}

let user = find_user(42);
// user.name;             // COMPILE ERROR: cannot access field on Option[User]

match user {
    Some(u) => u.name,    // safe — compiler verified Some
    None => "unknown",
}
```

**What it eliminates:** Null pointer exceptions, null checks as discipline, "billion dollar mistake."

**Manual:** [Types — Option](manual/02-types.md#23-built-in-parameterized-types)

---

### Req 5: Error Path Visibility {#req-5}

**`Result[T, E]` — every error in the type signature, no hidden paths.**

Functions that can fail MUST return `Result[T, E]`. Error types MUST be visible in the function signature. The caller MUST handle the error via `match`, `?` propagation, or combinators.

**If dropped:** High — hidden failure paths, callers can't reason about control flow. Exceptions make error paths invisible; ignored return codes make them silent.

**Origin:** Monadic error handling (Wadler, 1995). OCaml `result`, Haskell `Either`, Rust `Result[T,E]`. Built on algebraic data types (Req 1).

**Industrial validation:** MISRA C Rule 17.x (return values must be used), IEC 61508 error handling requirements.

```mvl
fn parse_config(text: String) -> Result[Config, ParseError] {
    // ...
}

fn load() -> Result[Config, AppError] ! FileRead {
    let text = read_to_string("config.toml")?;   // ? propagates Err
    let config = parse_config(text)?;             // ? propagates Err
    Ok(config)
}

// Ignoring an error is a COMPILE ERROR:
// parse_config(text);    // Result[Config, ParseError] unused
```

**What it eliminates:** Unhandled exceptions, ignored return codes, silent failures, hidden control flow.

**Manual:** [Error Handling](manual/15-errors.md)

---

### Req 6: Resource Linearity {#req-6}

**Ownership and borrowing — every resource has exactly one owner.**

Values have exactly one owner. Ownership transfers via `move`. Borrowing allows temporary access without transfer. Linear resources (files, connections) MUST be explicitly consumed.

**If dropped:** High — resource leaks, use-after-free, data races. Hardest bugs to diagnose.

**Origin:** Linear logic (Girard, 1987): each resource used exactly once. Realized as ownership + borrowing in Rust (2010s).

**Industrial validation:** Use-after-free as top CVE category, MISRA C dynamic memory rules.

```mvl
let file = File.open("data.txt")?;
let content = file.read_all()?;
file.close();                       // must consume — linear resource
// Forgetting file.close() → COMPILE ERROR: linear resource not consumed

let a = create_connection();
let b = move a;                     // ownership transferred
// a is invalid after move
```

**What it eliminates:** Resource leaks, double-free, use-after-free, data races (with Req 9).

**Manual:** [Ownership and Borrowing](manual/07-ownership.md)

---

### Req 7: Effect Tracking {#req-7}

**Side effects visible in function signatures — pure is the default.**

Functions with side effects MUST declare them using `! Effect` syntax. Functions without `!` are pure — the compiler rejects any side-effecting operation in a pure function. Effects are fine-grained and support subsumption hierarchies.

**If dropped:** Moderate-high — can't reason locally about what functions do. Hidden I/O, hidden state mutation, hidden network calls.

**Industrial validation:** Aligns with OWASP least privilege (A01) and DO-178C traceability requirements.

```mvl
fn add(a: Int, b: Int) -> Int {     // pure — no effects
    a + b
}

fn greet(name: String) -> Unit ! Console {  // declares Console effect
    println("Hello, " + name)
}

fn process() -> Unit ! IO {         // IO subsumes Console, FileRead, Net, etc.
    println("starting")             // Console satisfied by IO
    let cfg = read_file("x")?       // FileRead satisfied by IO
}
```

**The signature IS the threat model.** A pure function cannot exfiltrate data, access files, or hit the network. Effects are explicit opt-in to danger.

**What it enables:** Security audit at a glance, compile-time least privilege, local reasoning about any function.

**Spec:** [002-effect-system](../.openspec/specs/002-effect-system/spec.md) | **ADR:** [ADR-0035](../.openspec/adr/0035-effect-system-upgrade.md)

---

## Extended Requirements (8–11)

These were known for decades but considered impractical due to annotation burden. When LLMs generate all code, the annotations are free.

### Req 8: Termination Checking {#req-8}

**Total by default — functions provably halt.**

Functions are total by default. The compiler verifies termination via structural recursion (argument decreases on recursive call). `partial` is an explicit opt-in for intentionally non-terminating code. `while` is only permitted in `partial` functions.

**If dropped:** Functions that type-check but loop forever. Unprovable progress guarantees.

**Origin:** Martin-Löf type theory (1972). Lean 4, Idris 2. Structural recursion checking is decidable for the restricted fragment.

**Why LLMs unblock it:** Termination proofs require structural recursion annotations. Humans find this tedious. LLMs generate the structural argument automatically.

```mvl
fn factorial(n: UInt where n <= 20) -> UInt {
    match n {
        0 => 1,
        n => n * factorial(n - 1),  // structural recursion — compiler proves termination
    }
}

partial fn server() -> Never ! Net {
    while true {                    // while only in partial functions
        handle(accept()?);
    }
}
```

**What it eliminates:** Infinite loops in total functions, non-terminating computation where halting is expected.

**Manual:** [Totality and Termination](manual/09-totality.md)

---

### Req 9: Data Race Freedom {#req-9}

**Reference capabilities — no concurrent access on shared mutable state.**

Values carry reference capabilities (`iso`, `val`, `ref`, `tag`) that determine sendability and mutability. Only `iso` (isolated) and `val` (deeply immutable) can cross actor boundaries. The compiler rejects data races at compile time.

**If dropped:** Concurrent access on legal references. Ownership (Req 6) prevents use-after-free but not races.

**Origin:** Pony reference capabilities (Clebsch et al., 2015). Rust `Send`/`Sync` traits. Conceptually distinct from linearity.

**Why LLMs unblock it:** Reference capability annotations are verbose. LLMs generate capability annotations automatically.

```mvl
let data: iso Array[Int] = [1, 2, 3];
actor_a.send(consume data);        // iso → sendable, data moved

let shared: val Config = load_config();
actor_b.send(shared);              // val → sendable, shared immutably

let local: ref Buffer = Buffer.new();
// actor_c.send(local);            // COMPILE ERROR: ref is not sendable
```

**What it eliminates:** Data races, race conditions on shared mutable state, need for locks in user code.

**Manual:** [Concurrency](manual/12-concurrency.md)

---

### Req 10: Refinement Types {#req-10}

**Values within valid ranges at compile time — right type, right value.**

Types can carry predicates: `Int where x > 0`, `Array[T] where len(self) > 0`. The compiler verifies predicates via SMT solving. Fixed-width arithmetic is checked by default — overflow is a compile error.

**If dropped:** Right type, wrong value. `divide(x, 0)` type-checks. Array bounds violations. Integer overflow.

**Origin:** Girard (1987). Ada/SPARK (40 years of avionics). Liquid Haskell (Vazou et al., 2014). F* (Swamy et al.). SMT-backed, decidable for restricted fragment.

**Why LLMs unblock it:** Developers won't write `x: Int where x > 0` on every variable. LLMs infer and write refinement predicates from context.

```mvl
fn divide(a: Int, b: Int where b != 0) -> Int {
    a / b                           // safe — compiler proved b ≠ 0
}

fn first[T](items: Array[T] where len(items) > 0) -> T {
    items[0]                        // safe — compiler proved non-empty
}

let a: Int32 = Int32.MAX;
let b = a + 1;                      // COMPILE ERROR: potential overflow
let b = a.checked_add(1);           // Option<Int32> — explicit handling
```

**What it eliminates:** Division by zero, out-of-bounds access, integer overflow, invalid argument values.

**Manual:** [Refinement Types](manual/11-refinements.md)

---

### Req 11: Information Flow Control {#req-11}

**Secret and tainted data tracked through types — no leaks, no injection.**

Every value carries a security label: `Public`, `Clean`, `Tainted`, `Secret`. Data flows up the lattice freely; flowing down requires explicit `declassify()` or `sanitize()` — auditable, greppable operations. External data is automatically `Tainted`.

**If dropped:** Secret data flows to public outputs. SQL injection, log leakage, XSS, SSRF.

**Origin:** Denning's lattice model (1976). Perl taint mode (1989, runtime). Jif (Myers, 1999, compile-time). LIO. The MVL makes it compile-time and LLM-annotated.

**Why LLMs unblock it:** Security lattice annotations on every variable are prohibitive for humans. LLMs propagate taint labels through the codebase automatically.

```mvl
let user_input: Tainted[String] = read_line();
let api_key: Secret[String] = load_key();

fn log_msg(msg: Public[String]) -> () ! Log { ... }

log_msg(api_key);                   // COMPILE ERROR: Secret → Public
log_msg(user_input);                // COMPILE ERROR: Tainted → Public

let clean = sanitize(validate(user_input));  // Tainted → Clean (explicit)
log_msg(declassify(clean));                  // Clean → Public (auditable)
```

**OWASP coverage:** 9/10 categories addressed. A03 (Injection) → tainted input can't reach query builder. A07 (Auth Failures) → credentials tracked as `Secret`. A10 (SSRF) → tainted URLs can't reach network calls.

**What it eliminates:** SQL injection, XSS, SSRF, secret leakage to logs, tainted data in trusted sinks.

**Manual:** [Information Flow Control](manual/10-ifc.md)

---

## Summary Table

| # | Requirement | If dropped | Origin | Req since |
|---|------------|-----------|--------|-----------|
| 1 | [Type safety (ADTs)](#req-1) | Catastrophic | Curry-Howard 1934/1969, Hope 1980 | Always |
| 2 | [Memory safety](#req-2) | Catastrophic | C history, Girard 1987, Rust 2015 | Always |
| 3 | [Totality (exhaustive match)](#req-3) | Severe | Martin-Löf 1972, Hope 1980 | Always |
| 4 | [Null elimination](#req-4) | Severe | Hoare 1965/2009, SML 1990 | Always |
| 5 | [Error path visibility](#req-5) | High | Wadler 1995, Rust 2015 | Always |
| 6 | [Resource linearity](#req-6) | High | Girard 1987, Rust 2015 | Always |
| 7 | [Effect tracking](#req-7) | Moderate-high | Plotkin & Pretnar 2009, Koka 2014 | Always |
| 8 | [Termination checking](#req-8) | Moderate | Martin-Löf 1972, Lean 4, Idris 2 | LLM-enabled |
| 9 | [Data race freedom](#req-9) | Moderate | Pony 2015, Rust Send/Sync | LLM-enabled |
| 10 | [Refinement types](#req-10) | Moderate | SPARK, Liquid Haskell, F* | LLM-enabled |
| 11 | [Information flow control](#req-11) | Moderate | Denning 1976, Jif 1999 | LLM-enabled |

## The Convergence

The first seven requirements are the intersection of two independent evidence streams:

- **Formal methods (top-down):** What can a compiler prove?
- **Industrial failure analysis (bottom-up):** What kills people when unproven?

Requirements 8–11 add a third question: **What was too expensive for humans to annotate but becomes free when machines generate?**

Adding a 12th requirement needs the same bar: "catches bugs no combination of the other 11 catches."

**ADR:** [ADR-0001 — Eleven Compiler-Verified Requirements](adr/0001-eleven-requirements.md)
