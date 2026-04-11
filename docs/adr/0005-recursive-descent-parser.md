# ADR-0005: Hand-Written Recursive Descent Parser

**Status:** Accepted
**Date:** 2026-04-11
**Context:** How should the MVL parser be implemented? Parser generators (yacc, ANTLR) vs parser combinators (nom, chumsky) vs hand-written recursive descent.

## Decision

The MVL parser SHALL be a hand-written recursive descent (LL(1)) parser in Rust.

## Rationale

### The grammar is deliberately LL(1)

The MVL grammar was designed for regularity (ADR-0002, ADR-0004). Every construct is unambiguous. No operator precedence puzzles (operators only on built-in numerics). No dangling else (braces required). No context-dependent parsing. The grammar is ~100 productions and fits the LL(1) class — one token of lookahead is sufficient.

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

## Alternatives reconsidered for later phases

- **tree-sitter grammar** for IDE support (syntax highlighting, incremental parsing) — add in Phase 3 alongside the language server.
- **chumsky** if error recovery proves too complex hand-written — revisit if error message quality suffers.

## Consequences

- Parser is zero-dependency pure Rust
- Error messages are fully controlled
- Parser code is auditable and portable to MVL for self-hosting
- Grammar changes require updating parser code manually (no regeneration)
- Parser development takes slightly longer than using a generator — acceptable for a ~100 production grammar
