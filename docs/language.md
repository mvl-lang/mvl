# MVL Language Reference

This is the complete language reference for the Maximum Verifiable Language. For design rationale and research, see [mvl_rationale.md](mvl_rationale.md).

## Overview

The MVL design has three layers, ordered by stability:

### 1. Eleven Requirements (frozen)

What the compiler verifies. Well-formedness — internal quality proven at compile time.

Derived from the convergence of formal methods (Curry-Howard, Hoare, Girard, Plotkin & Pretnar) and safety-critical practice (MISRA C, DO-178C, IEC 61508). Requirements 8-11 added because LLM generation makes the annotation burden zero. Adding a 12th needs the same bar: "catches bugs no combination of the other 11 catches."

**Spec:** ADR-0001. Specs 001 (type system), 002 (effects), 003 (IFC).

### 2. Language Constructs (nearly frozen)

The minimal syntax to express programs. ~25 keywords, ~10 statement forms, LL(1) grammar. Validated against Python, Rust, Go, and Zig for expressiveness — no missing constructs, all gaps resolved as stdlib.

The contraction principle: features are only added if they increase verification density. The language shrinks by policy. The grammar fits in 100 EBNF productions.

**Spec:** ADR-0002 (contraction), ADR-0004 (size), ADR-0005 (parser). Grammar: `docs/grammar.ebnf`.

### 3. Standard Library + External Packages (evolves)

How programs do work. Collection ops, formatting, I/O, networking, crypto — all here. This is where the language grows without the language changing. Vocabulary compression: named, typed, verifiable functions that the compiler understands.

Three tiers: core (~30 types), standard (~200 functions), extended (packages with extern inside, verified API outside).

**Spec:** `docs/stdlib.md`. Epic 6 (#41).

### The boundary

Requirements define what the compiler proves. Constructs define what the programmer writes. Stdlib defines how work gets done. The boundary only moves in one direction: **stdlib grows, language doesn't.**

Testing, BDD, property testing, and model checking are all tooling on top of the same AST — zero language extensions. Mocking is free because effects are explicit. The language is the minimum. Everything else is tooling or library.

**Input boundary policy (ADR-0026):** MVL is post-Postel. Parsers MAY accept multiple syntactic formats; validators MUST enforce refinement predicates before values enter the proven core. Invalid input is rejected, never coerced. Unvalidated input carries the `Tainted` IFC label until proven.

### Architectural decisions

| ADR | Decision |
|-----|----------|
| ADR-0001 | Eleven compiler-verified requirements |
| ADR-0002 | Language contraction — what to drop and why |
| ADR-0003 | Compilation strategy — prototype Rust, production LLVM |
| ADR-0004 | Language size — deliberately the smallest |
| ADR-0005 | Hand-written recursive descent parser |

## Grammar Summary

~10 statement forms, ~5 expression forms, ~3 declaration forms.

### Declarations

```
type Name = struct { fields }        // product type
type Name = enum { Variant, ... }    // sum type
type Name = ExistingType              // alias

fn name(params) -> ReturnType { }              // pure function
fn name(params) -> ReturnType ! Effects { }    // effectful function
total fn name(params) -> ReturnType { }        // provably terminating
partial fn name(params) -> ReturnType { }      // may not terminate

use module::Item;                     // import (one item per line, at top of file)
pub fn name(...) -> T { }            // export (private by default)
pub type Name = ...                   // export a type
                                      // no re-exports — every symbol traces to its source

const NAME: Type = expr;              // compile-time constant

extern "rust" {                       // foreign function interface
    fn name(params) -> Type;          // trust boundary — not verified by MVL
}
```

### Comments

```
// line comment                       // the only comment form
/// doc comment                       // convention — parser sees //, doc tool reads ///
```

`//` line comments only. No `/* */` block comments. Rationale: the LLM generates all code — it doesn't need block comments for "commenting out large sections." One comment form means one parsing rule. Block comments nest badly (`/* /* */ */`) and add parser complexity for zero verification value. Go made the same choice. `///` is not separate syntax — it's a `//` comment that the doc tool recognizes by convention.

### Statements

```
let x: T = expr;                      // immutable binding
let x: ref T = expr;                  // mutable binding
x = expr;                             // assignment (mut only)
return expr;                          // early return
if expr { } else { }                  // branch (also an expression)
match expr { pattern => expr, }       // exhaustive match (also an expression)
for x in iter { }                     // bounded iteration
```

### Expressions

```
literal                               // 42, 3.14, "hello", true, [1,2,3]
name                                  // variable reference
expr.field                            // field access
expr.method(args)                     // method call
name(args)                            // function call
expr?                                 // Result/Option propagation
if expr { a } else { b }             // conditional expression
match expr { arms }                   // match expression
move expr                             // transfer ownership
consume expr                          // transfer isolated capability
declassify(expr)                      // Secret -> Public (auditable)
sanitize(expr)                        // Tainted -> Clean (auditable)
|x, y| x + y                         // lambda (immutable captures only)
|x: Int| -> Bool { x > 0 }           // lambda with type annotations
```

Lambdas have immutable captures only. Mutable closures are banned (violate Req 7). The lambda type includes effects: `fn(Int) -> Bool ! Console`. The compiler verifies lambdas identically to named functions.

### Types

```
Int, Int8..Int64, UInt8..UInt64       // integers (Int = arbitrary precision)
Float32, Float64                      // floating point
Bool, Char, Byte, String              // primitives
Array[T], Map[K,V], Set[T]           // collections
Option[T]                             // absence (Some | None)
Result[T,E]                           // fallibility (Ok | Err)
(T, U)                                // tuple
T where predicate                     // refinement type

Public[T], Tainted[T],                // security labels
Clean[T], Secret[T]

val T, ref T                          // shared (immutable) / exclusive (mutable) reference
iso T, tag T                         // reference capabilities (Phase 8)

fn(A) -> B                            // pure function type
fn(A) -> B ! Effect                   // effectful function type
```

### Effects

```
! Console                             // stdin/stdout/stderr
! FileRead, ! FileWrite, ! FileDelete // filesystem
! Net                                 // network access
! DB                                  // database operations
! ProcessSpawn                        // spawn external processes
! Random                              // non-deterministic randomness
! Clock                               // system clock
! Env                                 // environment variables
! Log                                 // logging
! Async                               // asynchronous operations
```

## Quality Model

| | Well-formed (internal quality) | Validated (external quality) |
|---|---|---|
| **What** | Structural correctness | Semantic correctness |
| **Checked by** | MVL compiler (11 requirements) | Test suite (from spec S) |
| **When** | Compile time | Test time |
| **Cost** | Free | Tests must be written |
| **ISPE layer** | S → P | P → I |

Code that compiles is well-formed. Code that passes tests is validated. Both are needed. Well-formedness reduces the validation surface.

## Mocking and Stubbing

MVL does not need a mock framework. Effects (Req 7) + no global state + traits make testability free.

### Why it works

In most languages, mocking is hard because dependencies are hidden — globals, singletons, ambient I/O. You need frameworks (Mockito, unittest.mock, mockall) to intercept calls at runtime. In MVL, every dependency is in the function signature as a parameter or effect declaration. There is nothing hidden to intercept.

```
// Production
fn get_user(db: val DbConn, id: UserId) -> Result[User, DbError] ! DB {
    db.query("SELECT ...", id)?
}

// Test — pass a different db. No framework needed.
fn test_get_user() {
    let db = in_memory_db([test_user]);
    let result = get_user(db, test_user.id);
    assert_eq(result, Ok(test_user));
}
```

### Effect stubbing via traits

Traits define contracts. Production and test implementations are swappable:

```
type FileSystem = trait {
    fn read(self, path: Path) -> Result[String, IOError] ! FileRead
}

type RealFS = struct {}              // production
impl FileSystem for RealFS { ... }

type StubFS = struct {               // test — stdlib provides this
    files: Map[Path, String]
}
impl FileSystem for StubFS { ... }
```

### Stdlib test helpers

| Helper | Stubs |
|--------|-------|
| `StubFS { files }` | Filesystem (in-memory) |
| `in_memory_db(rows)` | Database (no connection) |
| `mock_channel()` | Channel (records sent messages) |
| `fixed_clock(timestamp)` | Clock (deterministic) |
| `seeded_random(seed)` | Random (reproducible) |
| `capture_log()` | Logging (captures entries for assertion) |

### Why no framework is needed

| Requirement | What it enables for testing |
|------------|---------------------------|
| Req 7 (effects) | You know exactly what to stub — it's in the type signature |
| No global state | Nothing to monkey-patch |
| Traits (ADR-0002) | Swap implementations by passing a different value, not by subclassing |
| Req 6 (ownership) | Test owns its stubs — no shared mutable test state |

This is a stdlib concern, not a language feature. Zero keywords added.

## Design Completeness

| Area | Designed? | Specced? | Gaps | Ticket |
|------|-----------|----------|------|--------|
| Language syntax | Yes — EBNF, ~100 productions, LL(1) | Yes — grammar.ebnf | Lambda resolved: immutable captures kept (#61) | #51 |
| 11 requirements | Yes — derivation, origins, code examples | Yes — ADR-0001 | Solid | — |
| Type system | Yes — ADTs, Option, Result, ownership, refinements, IFC | Yes — spec 001 | Trait system needs detail | — |
| Effect system | Yes — fine-grained effects, capabilities, totality | Yes — spec 002 | Effect handler syntax undefined | — |
| IFC | Yes — lattice, labels, declassify/sanitize | Yes — spec 003 | Solid | — |
| Contraction | Yes — 16 features dropped with origins | Yes — ADR-0002 | Solid | — |
| Stdlib | Yes — three tiers, Unix complete | docs/stdlib.md | Needs formal spec (004) | #49 |
| Parser strategy | Yes — recursive descent LL(1) | Yes — ADR-0005 | Solid | — |
| Compilation | Yes — Phase 1 Rust, Phase 2 LLVM, Phase 3 self-host | Yes — ADR-0003 | Solid | — |
| Testing | Yes — external/internal, BDD, property, model checker | Tickets #37-40 | Needs spec (005) | #50 |
| Concurrency | Yes — actors, capabilities, WCET | In research doc | No dedicated spec yet | — |
| Module system | File = module, `use`, `pub` | Yes — spec 005 | Packages not formally specced yet | #47 |
| Generics | Type params `[T]` | Not specced | Constraints, monomorphization | #48 |
| Memory model | Ownership + borrow in spec 001 | Partially specced | Allocator, stack vs heap | — |
| FFI / interop | `extern "rust"` blocks with trust boundaries | Not specced | Explicit, greppable, assurance-tracked | #52 |
| Build system | `mvl build` = transpile + cargo | In ADR-0003 | Package manager not designed | — |

~85% designed, ~60% specced. The language shape is complete. Remaining gaps are module system and generics — implementation-critical but don't change the 11 requirements.

## Full EBNF

See `docs/grammar.ebnf` for the complete formal grammar (~100 productions).

## Standard Library

See `docs/stdlib.md` for the full stdlib specification (core ~30 types, standard ~200 functions, extended packages).
