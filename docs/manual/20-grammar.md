# 20. Grammar

The complete formal grammar is defined in EBNF notation in [`grammar.ebnf`](../grammar.ebnf).

## 20.1 Design Constraints

The grammar is LL(1) — parseable by a single-lookahead recursive descent parser with no backtracking. This was a deliberate choice (ADR-0005):

- **Predictable parsing:** Every token determines the next production
- **Simple error recovery:** The parser always knows what it expected
- **No ambiguity:** No construct has two valid interpretations
- **Fast compilation:** Linear-time parsing

## 20.2 Grammar Size

~100 EBNF productions organized into:

| Category | Productions | Examples |
|----------|------------|---------|
| Top-level | ~5 | program, declaration |
| Types | ~15 | type_expr, refinement, security labels |
| Declarations | ~10 | fn_decl, type_decl, use_decl, reexport_decl |
| Statements | ~10 | let, if, match, for, while |
| Expressions | ~15 | binary, unary, call, lambda, propagation |
| Patterns | ~5 | destructuring, guards |
| Literals | ~5 | integer, float, string, collections |
| Lexical | ~10 | identifiers, keywords, operators |

## 20.3 LL(1) Violations Avoided

Seven common constructs in popular languages break LL(1). MVL avoids all by design:

1. **Expression statements vs declarations** (C/C++) — MVL: `let` always starts a binding, bare expressions are always statements
2. **Angle bracket ambiguity** (Java/C++ generics) — MVL: square brackets `name[T]()` in call position; `<>` only in type definitions (ADR-0005)
3. **Lambda vs grouping** (JavaScript arrow functions) — MVL: lambdas use `|params|`, not `(params) =>`
4. **Ternary operator** — MVL: no ternary; use `if`/`else` expression
5. **Type annotation vs label** (TypeScript) — MVL: `where` keyword disambiguates refinements
6. **Pattern matching with guards** — MVL: `if expr` after pattern, unambiguous (`if` cannot start a pattern)
7. **String interpolation** — MVL: no interpolation; use `format()`
8. **Visibility prefix** — `pub` could precede multiple declaration kinds. MVL factors it out: `declaration = [ "pub" ] decl_body` where each `decl_body` alternative (`type`, `fn`/totality, `const`, `use`) starts with a distinct token, so LL(1) is preserved.

## 20.4 Full EBNF

See [`grammar.ebnf`](../grammar.ebnf) for the complete formal grammar.
