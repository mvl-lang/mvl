# 21. Testing Strategy

MVL has four testing concerns, zero language extensions. Testing is tooling on the same AST — the language stays at ~25 keywords.

## 21.1 Philosophy

The compiler handles *internal quality* (well-formedness — the 11 requirements). Tests handle *external quality* (validation — does it do the right thing?). Every property the compiler proves is a category of tests you never write.

| | Well-formed (compiler) | Validated (tests) |
|---|---|---|
| **What** | Structural correctness | Semantic correctness |
| **When** | Compile time | Test time |
| **Cost** | Free | Tests must be written |
| **ISPE layer** | S → P | P → I |

The stronger the compiler, the less validation work remains. MVL's 11 requirements eliminate entire test categories:

| Eliminated by compiler | No longer needs testing |
|----------------------|------------------------|
| [Req 1](../requirements.md#req-1) (types) | Type confusion, invalid state construction |
| [Req 2](../requirements.md#req-2) (memory) | Buffer overflows, use-after-free |
| [Req 3](../requirements.md#req-3) (totality) | Missing cases, unhandled variants |
| [Req 4](../requirements.md#req-4) (null) | Null pointer exceptions |
| [Req 5](../requirements.md#req-5) (errors) | Unhandled errors, silent failures |
| [Req 6](../requirements.md#req-6) (ownership) | Resource leaks, double-free |
| [Req 7](../requirements.md#req-7) (effects) | Hidden side effects, unexpected I/O |
| [Req 8](../requirements.md#req-8) (termination) | Infinite loops (in total functions) |
| [Req 9](../requirements.md#req-9) (data races) | Race conditions |
| [Req 10](../requirements.md#req-10) (refinements) | Out-of-range values, division by zero |
| [Req 11](../requirements.md#req-11) (IFC) | Secret leakage, injection attacks |

What remains: **business logic correctness**. Does `sort()` actually sort? Does `calculate_tax()` return the right amount? Tests prove intent.

## 21.2 Unit Tests

Unit tests are external to the source file. Test files live alongside source with a `_test.mvl` suffix:

```
src/
  math.mvl
  math_test.mvl          ← unit tests for math.mvl
  http/
    client.mvl
    client_test.mvl       ← unit tests for client.mvl
```

### Syntax

```mvl
#[test]
fn test_add() {
    assert_eq(add(2, 3), 5);
}

#[test]
fn test_divide_nonzero() {
    assert_eq(divide(10, 2), 5);
}

#[test]
fn test_find_missing() {
    let result = find_user(999);
    assert_eq(result, None);
}
```

### Built-in Assertions

| Assertion | Checks |
|-----------|--------|
| `assert(condition)` | Condition is true |
| `assert_eq(a, b)` | `a == b` |
| `assert_ne(a, b)` | `a != b` |
| `assert_ok(result)` | Result is `Ok` |
| `assert_err(result)` | Result is `Err` |
| `assert_some(option)` | Option is `Some` |
| `assert_none(option)` | Option is `None` |

### Why external test files

Test files (`_test.mvl`) are *evidence* — they survive code regeneration. If the LLM regenerates `math.mvl`, the tests in `math_test.mvl` still verify the regenerated code does the same thing. Internal tests would be lost. This is the ISPE model applied to testing: tests are E (evidence), not P (program).

Run with: `mvl test`

## 21.3 Integration Tests

Integration tests verify multiple modules working together. They live in a dedicated directory:

```
tests/
  integration/
    api_flow_test.mvl        ← end-to-end API test
    migration_test.mvl       ← database migration test
```

### Example

```mvl
// tests/integration/user_flow_test.mvl

#[test]
fn test_create_and_retrieve_user() ! DB {
    let db = test_database();

    let user = User { name: "Alice", email: "alice@example.com" };
    let id = create_user(&db, user)?;
    let retrieved = get_user(&db, id)?;

    assert_eq(retrieved.name, "Alice");
    assert_eq(retrieved.email, "alice@example.com");

    db.rollback();
}
```

Integration tests declare their effects — the test runner knows which resources are needed and can parallelize safely (tests with `! DB` on the same database run sequentially; tests with `! FileRead` on different paths run in parallel).

## 21.4 Mocking and Stubbing

MVL does not need a mock framework. Effects ([Req 7](../requirements.md#req-7)) + no global state + traits make test doubles trivial.

### Why it works

In most languages, mocking is hard because dependencies are hidden — globals, singletons, ambient I/O. You need frameworks (Mockito, unittest.mock, mockall) to intercept calls at runtime. In MVL, every dependency is in the function signature. There is nothing hidden to intercept.

### Stub via parameter passing

```mvl
// Production
fn get_user(db: val DbConn, id: UserId) -> Result[User, DbError] ! DB {
    db.query("SELECT ...", id)?
}

// Test — pass a different db. No framework needed.
#[test]
fn test_get_user() {
    let db = in_memory_db([test_user]);
    let result = get_user(val db, test_user.id);
    assert_eq(result, Ok(test_user));
}
```

### Stub via traits

Traits define contracts. Production and test implementations are swappable:

```mvl
type FileSystem = trait {
    fn read(self, path: Path) -> Result[String, IOError] ! FileRead
}

type RealFS = struct {}
impl FileSystem for RealFS { /* real implementation */ }

type StubFS = struct {
    files: Map[Path, String]
}
impl FileSystem for StubFS {
    fn read(self, path: Path) -> Result[String, IOError] ! FileRead {
        match self.files.get(path) {
            Some(content) => Ok(content),
            None => Err(IOError.not_found(path)),
        }
    }
}
```

### Stdlib test helpers

| Helper | Stubs |
|--------|-------|
| `in_memory_db(rows)` | Database (no connection) |
| `StubFS { files }` | Filesystem (in-memory) |
| `mock_channel()` | Channel (records sent messages) |
| `fixed_clock(timestamp)` | Clock (deterministic) |
| `seeded_random(seed)` | Random (reproducible) |
| `capture_log()` | Logging (captures entries for assertion) |

### Why no framework is needed

| Requirement | What it enables for testing |
|------------|---------------------------|
| [Req 7](../requirements.md#req-7) (effects) | You know exactly what to stub — it's in the type signature |
| No global state | Nothing to monkey-patch |
| Traits | Swap implementations by passing a different value |
| [Req 6](../requirements.md#req-6) (ownership) | Test owns its stubs — no shared mutable test state |

## 21.5 Property-Based Testing

Property tests verify that a property holds for *all* valid inputs, not just specific examples. In MVL, refinement types ([Req 10](../requirements.md#req-10)) make property testing a library — the type tells the framework what to generate.

### Syntax

```mvl
#[property]
fn sort_preserves_length(items: Array[Int]) -> Bool {
    sort(items).len() == items.len()
}

#[property]
fn sort_is_ordered(items: Array[Int]) -> Bool {
    let sorted = sort(items);
    sorted.windows(2).all(|w| w[0] <= w[1])
}

#[property]
fn sort_is_idempotent(items: Array[Int]) -> Bool {
    sort(items) == sort(sort(items))
}
```

### Refinement-guided generation

The key insight: `Int where x > 0` tells the property testing framework exactly what values to generate. No separate generator definition needed.

```mvl
#[property]
fn divide_inverse(a: Int, b: Int where b != 0) -> Bool {
    divide(a, b) * b + (a % b) == a
}

#[property]
fn port_valid(p: UInt16 where p >= 1 && p <= 65535) -> Bool {
    Port.new(p).is_ok()
}
```

The framework reads the refinement predicates and generates values satisfying them. No `forall` keyword, no `Arbitrary` trait, no generator combinators. The type *is* the generator specification.

### Shrinking

When a property fails, the framework automatically shrinks the counterexample to the smallest failing input. Refinement types constrain shrinking — the framework never shrinks below the refinement boundary.

### Running

```bash
mvl test --property              # run property tests only
mvl test --property --seed 42    # reproduce with specific seed
mvl test --property --trials 10000  # more trials
```

## 21.6 BDD / Scenario Tests

Behavior-driven tests use a structured given/when/then format. This is a test runner convention, not language syntax.

```mvl
#[scenario("User login")]
fn test_valid_login() ! DB {
    // GIVEN
    let db = test_database();
    let user = create_test_user(&db, "alice", "password123");

    // WHEN
    let result = login(&db, "alice", "password123");

    // THEN
    assert_ok(result);
    let session = result.unwrap();
    assert_eq(session.user_id, user.id);
}

#[scenario("User login with wrong password")]
fn test_invalid_login() ! DB {
    // GIVEN
    let db = test_database();
    create_test_user(&db, "alice", "password123");

    // WHEN
    let result = login(&db, "alice", "wrong");

    // THEN
    assert_err(result);
}
```

BDD scenarios map directly to spec scenarios (ISPE: S layer). The `#[scenario]` attribute links tests to specification requirements for assurance traceability.

## 21.7 Model Checking

Model checking verifies properties about state machines and concurrent systems. It operates on the same AST as the compiler — no separate modeling language.

### What it checks

- **Deadlock freedom:** No reachable state where all actors are blocked
- **Livelock freedom:** No infinite cycle of state changes without progress
- **Invariant preservation:** A property holds in every reachable state
- **Temporal properties:** "Eventually X happens" or "X always leads to Y"

### Syntax

```mvl
#[model]
type TrafficLight = enum { Red, Yellow, Green }

#[invariant]
fn never_both_green(a: TrafficLight, b: TrafficLight) -> Bool {
    !(a == Green && b == Green)
}

#[transition]
fn next(light: TrafficLight) -> TrafficLight {
    match light {
        Red => Green,
        Green => Yellow,
        Yellow => Red,
    }
}
```

### How it works

The model checker is a compiler pass that:

1. Enumerates the state space from type definitions
2. Explores all reachable states via transition functions
3. Verifies invariants hold at every state
4. Reports counterexample traces when an invariant is violated

For bounded state spaces (enums, refinement-typed integers), this is exhaustive. For unbounded spaces, it uses bounded model checking (explore up to depth N).

### Concurrency models

```mvl
#[model]
fn producer_consumer() {
    let (tx, rx) = Channel.new[Int]();

    #[actor]
    fn producer(tx: iso Sender[Int]) {
        for i in 0..10 {
            tx.send(i);
        }
    }

    #[actor]
    fn consumer(rx: iso Receiver[Int]) {
        for msg in rx {
            process(msg);
        }
    }

    #[invariant]
    fn no_lost_messages(sent: UInt, received: UInt) -> Bool {
        received <= sent
    }
}
```

### Running

```bash
mvl check --model              # run model checker
mvl check --model --depth 100  # bounded depth
mvl check --model --verbose    # show explored states
```

## 21.8 Test Organization Summary

```
src/
  math.mvl                     # source
  math_test.mvl                # unit tests (survive regeneration)
tests/
  integration/
    api_flow_test.mvl          # integration tests
  property/
    sort_properties_test.mvl   # property tests
  models/
    protocol_model_test.mvl    # model checking
```

## 21.9 The Four Concerns — No Language Extensions

| Concern | Mechanism | Language change | Ticket |
|---------|-----------|----------------|--------|
| Unit tests | `#[test]` attribute, assertions | Zero — stdlib | #38 |
| BDD scenarios | `#[scenario]` attribute, given/when/then convention | Zero — test runner | #39 |
| Property tests | `#[property]` attribute, refinement-guided generation | Zero — library reads types | #40 |
| Model checking | `#[model]`, `#[invariant]`, `#[transition]` attributes | Zero — compiler pass on AST | #37 |

The language stays at ~25 keywords. Everything else is tooling on the same AST.

## 21.10 Compiler Grammar Tests

The MVL compiler itself has a grammar test suite that validates the formal definition stays consistent with the two implementations that derive from it.

### What is tested

| Layer | Source of truth | Test |
|-------|----------------|------|
| EBNF formal grammar | `docs/grammar.ebnf` (ISO 14977 notation) | Human-readable reference |
| Rust recursive-descent parser | `src/mvl/parser/` | `cargo test` — 154 tests |
| Tree-sitter grammar (editor support) | `etc/tree-sitter-mvl/grammar.js` | `make test-tree-sitter` — 26 corpus tests |
| EBNF ↔ tree-sitter coverage | `tools/check_grammar_coverage.py` | `make test-grammar-coverage` |

### EBNF ↔ tree-sitter coverage check

`tools/check_grammar_coverage.py` cross-validates `docs/grammar.ebnf` against `grammar.js` at the production-name level:

- Extracts all lowercase production rule names from the EBNF (`rule = body ;` pattern)
- Extracts all rule names from `grammar.js` (`rulename: ($) =>` pattern)
- Reports EBNF rules with no tree-sitter counterpart as **unexpected gaps** (exit 1)
- Documents intentional divergences (inlined rules, renames, unimplemented features) in two allow-lists so future gaps are always detected

Run it:

```bash
make test-grammar-coverage
```

Example output (all passing):

```
EBNF productions:         79
Tree-sitter rules:        85

✅  No unexpected gaps — all EBNF rules are covered or documented.

ℹ️   Known intentional absences in tree-sitter (documented):
     alias_type            inlined: type_body uses type_expr directly
     map_literal           not yet implemented in tree-sitter grammar
     ...

RESULT: PASS
```

### Tree-sitter corpus tests

26 corpus test cases in `etc/tree-sitter-mvl/test/corpus/` verify that the tree-sitter grammar parses representative MVL programs into the correct parse trees:

```bash
make test-tree-sitter
```

The corpus covers: literals, type declarations (struct/enum/alias/generic), function declarations (effects, capabilities, where-clauses), statements, expressions, patterns, module declarations, and extern blocks.

### Running all grammar tests

```bash
make test   # runs test-corpus + test-tree-sitter + test-grammar-coverage + cargo test
```

## 21.11 Assurance Traceability

Tests connect to specifications through attributes:

```mvl
#[test]
#[spec("001", "Req 2", "Scenario: Option forces handling")]
fn test_option_access() {
    let user: Option[User] = find_user(42);
    // Cannot access user.name directly — must match
    match user {
        Some(u) => assert_eq(u.name, "Alice"),
        None => assert(true),  // both branches handled
    }
}
```

`mvl assurance` generates a traceability matrix:
- Which specs have tests (coverage)
- Which tests link to specs (completeness)
- Which implementations link to specs (traceability)
- Extern function count (trust boundary surface)
