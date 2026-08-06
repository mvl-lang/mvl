# 18. Keywords Reference

Complete list of MVL keywords with definitions and the requirement they serve.

**Source of truth:** `mvl-spec` `grammar/keywords.yaml`, cross-checked against
`src/mvl/parser/lexer/mod.rs`, `compiler/lexer.mvl`, and the tree-sitter grammar
by `make validate-keywords`. If you edit this page, run that target — it is what
catches the four sources drifting apart.

**Counts.** The lexer reserves **43** words. `keywords.yaml` lists **45**
excluding the four pattern constructors, or **49** including them. The two-word
gap is `self` and `old`, which are contextual inside refinement clauses and are
not lexed as keywords.

## Declaration (13)

| Keyword | Definition | Req |
|---------|-----------|-----|
| `fn` | Define a function | — |
| `type` | Define a type — struct, enum, alias, or trait | 1 |
| `struct` | Product type (record with named fields) | 1 |
| `enum` | Sum type (tagged union of variants) | 1 |
| `const` | Compile-time constant | — |
| `use` | Bring names into scope | — |
| `pub` | Export from the module | — |
| `extern` | Foreign function interface | — |
| `impl` | Implement methods or traits for a type | 1 |
| `builtin` | Runtime-provided implementation; body is forbidden | — |
| `actor` | Define an actor — isolated state, message-passing | 9 |
| `effect` | Declare an effect | 7 |
| `test` | Define a test function | — |

## Totality (2)

| Keyword | Definition | Req |
|---------|-----------|-----|
| `total` | Function provably terminates | 8 |
| `partial` | Function may not terminate | 8 |

## Cast (1)

| Keyword | Definition | Req |
|---------|-----------|-----|
| `as` | Explicit type conversion | 1 |

## Control flow (8)

| Keyword | Definition | Req |
|---------|-----------|-----|
| `if` | Conditional branch (also an expression) | — |
| `else` | Alternative branch | — |
| `match` | Exhaustive pattern matching (also an expression) | 3 |
| `for` | Bounded iteration | 8 |
| `in` | Iteration source in a `for` loop | 8 |
| `while` | Unbounded loop — `partial` functions only | 8 |
| `return` | Early return from a function | — |
| `select` | Await one of several actor messages | 9 |

## Bindings (2)

| Keyword | Definition | Req |
|---------|-----------|-----|
| `let` | Immutable variable binding | 6 |
| `ghost` | Verification-only binding, erased before codegen | 10 |

There is **no `mut` keyword.** Mutability is expressed through reference
capabilities — see below.

## Ownership and reference capabilities (5)

| Keyword | Definition | Req |
|---------|-----------|-----|
| `iso` | Isolated — unique, sendable across actors | 9 |
| `val` | Deeply immutable — sendable | 9 |
| `ref` | Shared mutable — actor-local only | 6, 9 |
| `tag` | Identity only — sendable, exposes no data | 9 |
| `consume` | Transfer an isolated capability | 6, 9 |

Four capabilities, adapted from Pony's six (ADR-0029; Pony also has `box` and
`trn`). Only `iso` and `val` carry data across an actor boundary; `tag` crosses
but exposes nothing; `ref` never leaves its actor.

## Information flow control (2)

| Keyword | Definition | Req |
|---------|-----------|-----|
| `label` | Declare an IFC label for the module — `label Tainted;` | 11 |
| `relabel` | Declare or apply a label transition — `relabel name(expr, "TAG")` | 11 |

`declassify()` and `sanitize()` were **removed under #894.** Use `relabel`.

## Refinements and contracts (10)

| Keyword | Definition | Req |
|---------|-----------|-----|
| `where` | Refinement predicate or generic constraint | 10 |
| `requires` | Precondition | 10 |
| `ensures` | Postcondition | 10 |
| `invariant` | Loop or type invariant | 10 |
| `decreases` | Termination measure | 8 |
| `forall` | Universal quantifier | 10 |
| `exists` | Existential quantifier | 10 |
| `with` | Effect handler clause | 7 |
| `self` | The refined value in a predicate position — contextual | 10 |
| `old` | Pre-state of a value inside `ensures` — contextual | 10 |

`self` and `old` are contextual: valid inside refinement clauses, ordinary
identifiers elsewhere. They are in `keywords.yaml` but not in the lexer's 43.

## Booleans (2)

| Keyword | Definition | Req |
|---------|-----------|-----|
| `true` | Boolean true | — |
| `false` | Boolean false | — |

## Pattern constructors (4)

Reserved words, but constructors rather than syntax:

| Name | Definition | Req |
|------|-----------|-----|
| `Some` | Option variant — value present | 4 |
| `None` | Option variant — value absent | 4 |
| `Ok` | Result variant — success | 5 |
| `Err` | Result variant — failure | 5 |

## Built-in type names (24)

**Not reserved words.** The lexer does not protect them; they are reserved by
convention and should not be shadowed (`keywords.yaml:89-90`).

`Int` `Int8` `Int16` `Int32` `Int64` `UInt8` `UInt16` `UInt32` `UInt64`
`Float` `Float32` `Float64` `Bool` `Char` `Byte` `String` `Unit`
`Option` `Result` `List` `Array` `Map` `Set` `Never`

`Option` (Req 4) replaces null; `Result` (Req 5) replaces exceptions. `Never`
(§9.5) is the bottom type — the return type of `panic` and other always-
aborting functions; it has no values and unifies with any expected type.
Not yet listed in `mvl-spec`'s `grammar.ebnf`/`keywords.yaml` (#2217) — the
other 23 names are normative there, `Never` currently is not.

## Stdlib IFC labels (6)

**Ordinary identifiers, not reserved words** (`keywords.yaml:116`). Pre-seeded by
the stdlib; a module may declare its own with `label`.

| Label | Definition |
|-------|-----------|
| `Tainted` | External, untrusted data |
| `Secret` | Cryptographic material or credentials |
| `ConfigPath` | Capability label — config file path (#931) |
| `DbUrl` | Capability label — database connection string (#931) |
| `ApiEndpoint` | Capability label — outbound API target (#931) |
| `AuditTarget` | Capability label — audit sink (#931) |

There is **no `Public` label.** Public is spelled by *absence* — an unlabeled
type is public by default (`grammar.ebnf:139-140`). There is no `Clean` label
either; it appears nowhere in the specification.

## Built-in functions

| Name | Definition | Req |
|------|-----------|-----|
| `panic()` | Unrecoverable error — terminates the program, returns `Never` | — |

Declared in `std/core.mvl` as `pub builtin fn panic(message: String) -> Never`.
A stdlib builtin, not a keyword.
