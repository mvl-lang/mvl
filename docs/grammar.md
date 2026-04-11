# MVL EBNF Grammar

The complete formal grammar for the Minimum Verification Language. ~100 productions, LL(1) — parseable by recursive descent with one token of lookahead.

See [ADR-0005](adr/0005-recursive-descent-parser.md) for the parser design decision. See [Chapter 20](manual/20-grammar.md) of the language manual for the design rationale and LL(1) analysis.

## Design Constraints

- **LL(1):** Every token determines the next production. No backtracking.
- **~100 productions:** Organized into top-level, types, declarations, statements, expressions, patterns, literals, and lexical rules.
- **~25 keywords:** Each justified by a verification requirement.
- **No ambiguity:** Seven common LL(1)-breaking constructs in popular languages are avoided by design.

## Production Categories

| Category | Count | Key constructs |
|----------|-------|----------------|
| Top-level | ~5 | `program`, `declaration` |
| Modules | ~1 | `module_decl` |
| Type declarations | ~6 | `type_decl`, `struct_body`, `enum_body`, `variant` |
| Function declarations | ~8 | `fn_decl`, `totality`, `param_list`, `effect_list` |
| Type expressions | ~10 | `type_expr`, `option_type`, `result_type`, `ref_type`, `refined_type`, `fn_type` |
| Refinement predicates | ~4 | `refinement`, `ref_expr`, `ref_term`, `ref_atom` |
| Statements | ~8 | `let_stmt`, `if_stmt`, `match_stmt`, `for_stmt`, `while_stmt` |
| Expressions | ~12 | `expr`, `fn_call`, `method_call`, `lambda`, `propagate` |
| Patterns | ~3 | `pattern`, `pattern_list` |
| Literals | ~5 | `literal`, `list_literal`, `map_literal`, `set_literal` |
| Constants | ~1 | `const_decl` |
| Lexical | ~5 | `IDENT`, `INTEGER`, `FLOAT`, `STRING`, `COMMENT` |

## Complete EBNF

```ebnf
--8<-- "grammar.ebnf"
```
