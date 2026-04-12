# MVL EBNF Grammar

The complete formal grammar for the Minimum Verification Language. ~100 productions, LL(1) — parseable by recursive descent with one token of lookahead.

See [ADR-0005](adr/0005-recursive-descent-parser.md) for the parser design decision. See [Chapter 20](manual/20-grammar.md) of the language manual for the design rationale and LL(1) analysis.

## Notation — ISO 14977 EBNF

The grammar uses ISO 14977 Extended BNF. Quick reference:

| Notation | Meaning | Example |
|----------|---------|---------|
| `rule = body ;` | Production definition | `program = { declaration } ;` |
| `"lit"` | Terminal string literal | `"fn"`, `"where"` |
| `UPPER` | Terminal token (defined in Lexical section) | `IDENT`, `INTEGER` |
| `[ x ]` | Optional (zero or one) | `[ "mut" ]` |
| `{ x }` | Zero or more repetitions | `{ declaration }` |
| `( a \| b )` | Alternation | `( "total" \| "partial" )` |
| `(* text *)` | Comment — ignored by parser | `(* LL(1) property *)` |

Lowercase names are grammar productions (nonterminals). Uppercase names are lexical tokens (terminals) defined at the bottom of the file.

## Design Constraints

- **LL(1):** Every token determines the next production. No backtracking.
- **~100 productions:** Organized into top-level, types, declarations, statements, expressions, patterns, literals, and lexical rules.
- **~25 keywords:** Each justified by a verification requirement.
- **No ambiguity:** Eight common LL(1)-breaking constructs in popular languages are avoided by design (see Chapter 20).

## Production Categories

| Category | Count | Key constructs |
|----------|-------|----------------|
| Top-level | ~5 | `program`, `declaration`, `decl_body` |
| Modules | ~3 | `use_decl`, `reexport_decl`, `module_path` |
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

## Tree-sitter Grammar

The file `etc/tree-sitter-mvl/grammar.js` is a hand-translation of this EBNF into tree-sitter's DSL, used for syntax highlighting in Zed and Neovim. It follows the same structure but adapts for tree-sitter's GLR parser:

- Left-recursive rules (e.g. `expr`) are written as `prec.left` branches instead of EBNF repetition
- Some rules are split or renamed to avoid ambiguity (e.g. `fn_call` → `fn_call_expr`)
- Uppercase terminals (e.g. `IDENT`) become named regex rules (e.g. `identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_]*/`)
- Tree-sitter extensions not yet in the EBNF: `module_decl`, `extern_decl`, `extern_fn_decl`

### Coverage check

```bash
make test-grammar-coverage   # diff EBNF productions against grammar.js rules
make test-tree-sitter        # run tree-sitter corpus tests (26 test cases)
```

`tools/check_grammar_coverage.py` extracts all production names from this EBNF file and all rule names from `grammar.js`, then reports unexpected gaps. All intentional divergences (inlined rules, renames, unimplemented features) are documented in the script's `EBNF_KNOWN_ABSENT` and `TS_KNOWN_EXTENSIONS` tables.

## Complete EBNF

```ebnf
--8<-- "grammar.ebnf"
```
