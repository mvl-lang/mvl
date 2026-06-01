# MVL Language Manual

This is the complete language manual for the Maximum Verifiable Language. It defines the language as implemented by the MVL compiler.

For design rationale, see the [ADRs](../adr/0001-eleven-requirements.md). For formal specifications, see the [Specs](../specs/001-type-system.md). For the research behind the language, see the [Introduction](../introduction.md).

## Contents

1. [Lexical Structure](01-lexical.md) — tokens, keywords, literals, comments
2. [Types](02-types.md) — primitives, algebraic data types, generics, refinements, security labels
3. [Declarations](03-declarations.md) — functions, types, constants, modules, extern
4. [Statements](04-statements.md) — let, assignment, control flow, loops
5. [Expressions](05-expressions.md) — operators, calls, propagation, lambdas, ownership
6. [Pattern Matching](06-patterns.md) — destructuring, exhaustiveness, guards
7. [Ownership and Borrowing](07-ownership.md) — move semantics, borrows, reference capabilities
8. [Effect System](08-effects.md) — effect declarations, propagation, handlers, purity
9. [Totality and Termination](09-totality.md) — total/partial functions, structural recursion
10. [Information Flow Control](10-ifc.md) — security lattice, labels, declassify, sanitize
11. [Refinement Types](11-refinements.md) — predicates, SMT verification, ranges
12. [Concurrency](12-concurrency.md) — actors, capabilities, structured concurrency
13. [Module System](13-modules.md) — imports, visibility, packages
14. [Foreign Function Interface](14-ffi.md) — extern blocks, trust boundaries
15. [Error Handling](15-errors.md) — Result, Option, propagation, patterns
16. [Standard Library](16-stdlib.md) — core, standard, extended tiers
17. [Compilation Model](17-compilation.md) — phases, Rust transpilation, LLVM, self-hosting
18. [Keywords Reference](18-keywords.md) — complete keyword list with definitions
19. [Operators and Precedence](19-operators.md) — operator table, associativity
20. [Grammar](20-grammar.md) — complete EBNF reference
21. [Testing Strategy](21-testing.md) — unit, integration, mocking, property testing, model checking

## Conventions

- **MUST** / **MUST NOT** — the compiler enforces this; violation is a compile error
- **SHOULD** — recommended; the compiler may warn
- `code` — MVL syntax
- *emphasis* — defined term on first use

## Design Philosophy

The MVL is designed for LLMs to generate, compilers to verify, and humans to review. Every language feature exists to increase *verification density* — the number of properties the compiler can prove per token of source code. Features that decrease verification density are removed (see [ADR-0002](../adr/0002-language-contraction.md)).
