# MVL Language Reference

**Version:** 0.1.0 (draft)
**Date:** 2026-04-11

This is the complete language reference for the Minimum Verification Language. For design rationale and research, see `my-brain/study/mvl_research.md`.

## Overview

The MVL has three layers:
1. **Type system** (Spec 001) — ADTs, Option, Result, ownership, refinements, immutability
2. **Effect system** (Spec 002) — fine-grained effects, capabilities, totality, concurrency
3. **Information flow control** (Spec 003) — security labels, taint tracking, declassification

Three architectural decisions govern the design:
1. **ADR-0001:** Eleven compiler-verified requirements
2. **ADR-0002:** Language contraction — minimal syntax surface
3. **ADR-0003:** Compilation strategy — prototype Rust, production LLVM

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

module Name { declarations }          // namespace

const NAME: Type = expr;              // compile-time constant
```

### Statements

```
let x: T = expr;                      // immutable binding
let mut x: T = expr;                  // mutable binding
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
declassify(expr)                      // Secret → Public (auditable)
sanitize(expr)                        // Tainted → Clean (auditable)
```

### Types

```
Int, Int8..Int64, UInt8..UInt64       // integers (Int = arbitrary precision)
Float32, Float64                      // floating point
Bool, Char, Byte, String              // primitives
Array<T>, Map<K,V>, Set<T>           // collections
Option<T>                             // absence (Some | None)
Result<T,E>                           // fallibility (Ok | Err)
(T, U)                                // tuple
T where predicate                     // refinement type

Public<T>, Tainted<T>,                // security labels
Clean<T>, Secret<T>

&T, &mut T                            // shared / exclusive borrow
iso T, val T, ref T, tag T           // reference capabilities

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

## Design Completeness

| Area | Designed? | Specced? | Gaps | Ticket |
|------|-----------|----------|------|--------|
| Language syntax | Yes — EBNF, ~100 productions, LL(1) | Yes — grammar.ebnf | Lambda still in EBNF, needs cleanup | #51 |
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
| Module system | `module Name { }` in EBNF | Not specced | Imports, visibility, packages | #47 |
| Generics | Type params `<T>` in EBNF | Not specced | Constraints, monomorphization | #48 |
| Memory model | Ownership + borrow in spec 001 | Partially specced | Allocator, stack vs heap | — |
| Build system | `mvl build` = transpile + cargo | In ADR-0003 | Package manager not designed | — |

~85% designed, ~60% specced. The language shape is complete. Remaining gaps are module system and generics — implementation-critical but don't change the 11 requirements.

## Full EBNF

See `docs/grammar.ebnf` for the complete formal grammar (~100 productions).

## Standard Library

See `docs/stdlib.md` for the full stdlib specification (core ~30 types, standard ~200 functions, extended packages).
