# ADR-0005: Hand-Written Recursive Descent Parser

**Status:** Accepted
**Date:** 2026-04-11
**Context:** How should the MVL parser be implemented? Parser generators (yacc, ANTLR) vs parser combinators (nom, chumsky) vs hand-written recursive descent.

## Decision

The MVL parser SHALL be a hand-written recursive descent (LL(1)) parser in Rust.

## Rationale

### The grammar is deliberately LL(1)

The MVL grammar was designed for regularity (ADR-0002, ADR-0004). Every construct is unambiguous. The grammar is ~100 productions and fits the LL(1) class — one token of lookahead is sufficient.

Popular languages have constructs that break LL(1). The MVL avoids all of them by design:

| Language | Construct that breaks LL(1) | Why | MVL avoidance |
|----------|---------------------------|-----|---------------|
| **C/C++** | `a * b` — multiplication or pointer declaration? | Needs type info to disambiguate (the "lexer hack") | No pointer syntax. Ownership, not pointers. |
| **C++** | `a<b>c` — template or comparison? `>>` closes two templates or is right-shift? | Arbitrary lookahead needed | `<>` only in type position, never as comparison operator in expressions |
| **Python** | Indentation-based blocks | Not context-free — lexer must track indent stack | Braces `{}` for all blocks |
| **Rust** | Turbofish `foo::<T>()` — `<` could be comparison or type parameter | Needs context to resolve | Square brackets `foo[T]()` — LL(1) with single token lookahead |
| **Java/C#** | `List<List<Integer>>` — `>>` ambiguity | Same as C++ templates | Same: `<>` only in type position |
| **JavaScript** | `(a) => b` vs `(a)` — arrow function or grouping? | Can't tell until `=>` | No arrow functions. Named functions only. |
| **Go** | Semicolons inserted by lexer based on line endings | Lexer has context-dependent behavior | Explicit semicolons |
| **MVL (pre-v0.46)** | `parse<T>()` — angle-bracket generic call | `<` after identifier ambiguous with comparison; needs 3-token lookahead | Square brackets `parse[T]()` — `[` after identifier is unused in expression position |

The MVL's LL(1) property is not accidental — it's a consequence of the language contraction (ADR-0002). Every dropped feature also dropped a parsing ambiguity. The smallest language is also the easiest to parse.

A parser generator is overkill for a grammar this small and regular.

### Why not parser generators?

| Tool | Rejected because |
|------|-----------------|
| **ANTLR** | Java ecosystem. Heavy runtime. Generated code is hard to debug. Grammar maintenance becomes its own project. |
| **lalrpop** | Rust-native, but LR parsing is more powerful than needed. The grammar is LL(1) — LR complexity buys nothing. |
| **tree-sitter** | Designed for incremental parsing in editors. Excellent for IDE support (Phase 3), wrong tool for a compiler. |

### Why not parser combinators?

| Tool | Considered but rejected |
|------|----------------------|
| **nom** | Popular in Rust, excellent for binary formats. For a programming language grammar, the combinator style makes error messages harder to control and the code harder to read than explicit recursive descent. |
| **chumsky** | Excellent error recovery built in. Strong candidate. Rejected because: adds a dependency, combinator code is less obvious to audit, and the MVL grammar is simple enough that hand-written error recovery is tractable. |

### Why hand-written recursive descent?

1. **Maximum control over error messages.** Parser errors are the user-facing (and LLM-facing) output of the compiler. "Expected `}` to close block started at line 12" is better than "parse error at token 47." Hand-written parsers produce the best diagnostics — this is why GCC, Clang, Rust, Go, V8, and TypeScript all use hand-written parsers.

2. **No dependencies.** The parser is pure Rust with zero crates. Smaller trusted computing base. Easier to audit. Aligns with the MVL philosophy: minimal, explicit, no hidden complexity.

3. **The grammar fits in one developer's head.** ~100 productions, ~25 keywords, LL(1). A recursive descent parser for this grammar is ~1000-2000 lines of Rust. Readable, debuggable, modifiable.

4. **Self-hosting readiness.** When the MVL compiler is rewritten in MVL (bootstrap Step 3), the parser must be expressible in MVL. A hand-written recursive descent parser is straightforward to port. A parser generator dependency is not.

5. **Precedent.** Rust's own compiler (`rustc`) uses a hand-written recursive descent parser. Go's compiler uses a hand-written parser. These are languages with far more complex grammars than the MVL.

## Implementation approach

```
Source text → Lexer (tokenizer) → Token stream → Parser (recursive descent) → AST
```

- **Lexer:** Hand-written. Emits tokens with source locations (Span: file, line, column, byte offset). Keywords recognized by table lookup after identifier scan.
- **Parser:** One function per grammar production. `parse_fn_decl()`, `parse_type_expr()`, `parse_match_arm()`, etc. Each function consumes tokens and returns an AST node or an error with source location.
- **Error recovery:** On error, skip to the next synchronization point (`;`, `}`, `fn`, `type`). Collect multiple errors per file. Report all of them, not just the first.

## Generic type argument syntax: square brackets `[T]`

The function call `parse[CliArgs]()` uses square brackets for generic type arguments — not angle brackets `parse<CliArgs>()`. This is a direct consequence of the LL(1) commitment.

### Why angle brackets break LL(1)

After an identifier, `<` is ambiguous: it could be a comparison operator (`x < y`) or the start of a generic type argument (`parse<T>()`). Disambiguating requires 3-token lookahead (`< Ident > (`) — a violation of LL(1).

An earlier implementation used a `peek_at()` function for multi-token lookahead. This was removed as a violation of this ADR.

### Why square brackets work

After an identifier in expression position, `[` is currently unused. List literals (`[1, 2, 3]`) only appear at statement-start or after `=`, never immediately after an identifier. This means `name[` unambiguously signals a generic type argument with single-token lookahead.

The parser handles this in `parse_atom()`:

```
Ident → peek LBracket → consume [ → parse_type_expr() → expect ] → expect ( → parse args → FnCall
Ident → peek LParen   → consume ( → parse args → FnCall (no type args)
```

### Precedent

| Language | Generic syntax | Reason |
|----------|---------------|--------|
| **Go 1.18** | `f[T]()` | Chose `[]` because `<>` breaks LL(1) — same reasoning |
| **Scala** | `f[T]()` | Square brackets for type parameters |
| **Nim** | `f[T]()` | Square brackets for generic instantiation |
| **V** | `f[T]()` | Square brackets, influenced by Go |
| **C++** | `f<T>()` | 40 years of disambiguation pain |

### MVL row for the LL(1) table

| **MVL** | `parse<T>()` — angle-bracket generic call | `<` after identifier ambiguous with comparison; needs 3-token lookahead | Square brackets `parse[T]()` — `[` after identifier is unused in expression position |

*Call sites migrated in v0.46.0. Full migration (all positions) completed in v0.50.0 (#312). `<` is now rejected as a generic delimiter everywhere — in type declarations (`fn foo[T]`, `type List[T]`), type expressions (`Option[T]`, `Result[T, E]`), and security labels (`Secret[T]`). `<` remains valid only as a comparison operator. See also: Go 1.18 type parameter proposal.*

## Block-terminating statements do not take a trailing semicolon

The grammar distinguishes two statement forms in a function body:

```ebnf
match_stmt  = "match" expr "{" { match_arm } "}" ;   -- no trailing ";"
if_stmt     = "if" expr block [ "else" ( if_stmt | block ) ] ;
for_stmt    = "for" pattern "in" expr block ;
while_stmt  = "while" expr [ "decreases" expr ] block ;
expr_stmt   = expr ";" ;                              -- trailing ";" required
```

`match`, `if`, `for`, and `while` are syntactically self-delimiting: every branch ends with `}`. The parser can identify the end of the construct from a single closing brace token — no `;` is needed, and the grammar rejects one if present.

`expr_stmt` covers everything else used as a statement: function calls, assignments, and `let` bindings. These are not self-delimiting — a bare `logger.warn(...)` could be the start of a larger expression — so `;` is required as a terminator.

### Consequence for result discarding

This means the two most common ways to use a `Result` as a statement look different:

```mvl
// function call used as statement — expr_stmt, needs ";"
let _: Result[Unit, IoError] = auditor.emit(ev);    // triggers [silent-result-discard] lint

// match used as statement — match_stmt, no ";"
match auditor.emit(ev) {
    Err(e) => logger.warn("audit failed", {"error": e}),
    Ok(_) => { },
}
```

The `match` form is preferred: it forces explicit handling of both arms. The `let _` form is available but linted.

### Precedent

| Language | Block-terminating forms need `;`? |
|----------|----------------------------------|
| **Rust** | Optional — `match x { ... };` is legal (discards value), `match x { ... }` is also legal |
| **Swift** | No — `if`, `switch`, `for`, `while` never take `;` |
| **Go** | No — block-terminated statements never take `;` |
| **MVL** | No — `;` after `}` is a parse error; the grammar is unambiguous without it |

Rust's optionality is a source of minor confusion (does `;` change the type?). MVL eliminates the question by making `;` after `}` illegal.

## Alternatives reconsidered for later phases

- **tree-sitter grammar** for IDE support (syntax highlighting, incremental parsing) — add in Phase 3 alongside the language server.
- **chumsky** if error recovery proves too complex hand-written — revisit if error message quality suffers.

## Consequences

- Parser is zero-dependency pure Rust
- Error messages are fully controlled
- Parser code is auditable and portable to MVL for self-hosting
- Grammar changes require updating parser code manually (no regeneration)
- Parser development takes slightly longer than using a generator — acceptable for a ~100 production grammar
