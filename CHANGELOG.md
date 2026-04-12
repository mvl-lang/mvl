# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.2] — 2026-04-12 (Assurance UX: verbose by default, summary in PR)

### Fixed
- `make assurance` now runs verbose by default — shows per-requirement list with legend
- `make assurance-summary` added for compact dashboard (used by CI)
- CI PR comment posts summary only — no wall of per-requirement lines
- Legend added to verbose output: `[impl][tests][corpus]` symbols explained inline

## [0.5.1] — 2026-04-12 (CI: assurance report on PRs)

### Fixed
- Post ISPE assurance dashboard as a PR comment on every pull request — no longer buried in CI logs
- Removed duplicate assurance step from check job; assurance now runs once in its own job

## [0.5.0] — 2026-04-11 (Epic 4: Information Flow Control type checking)

### Added
- Security label types: `Public<T>`, `Tainted<T>`, `Secret<T>`, `Clean<T>` as first-class `Ty::Labeled` variants (#24)
- Security lattice: `lattice_rank()`, `can_flow()`, `join()`, `join_opt()` in `src/mvl/checker/ifc.rs` (#25)
- Label propagation: arithmetic ops propagate label join; comparisons yield unlabeled `Bool` (#26)
- Declassification chokepoints: `declassify()` (Secret→Public) and `sanitize()` (Tainted→Clean) with `InvalidDeclassify` and `InvalidSanitize` errors (#27)
- `Ty::unlabeled()` for structural operations that look through label wrappers
- `CheckError::InvalidDeclassify` and `CheckError::InvalidSanitize` variants
- 4 IFC corpus files under `tests/corpus/05_ifc/`
- 23 new integration tests covering all IFC scenarios (61 total)

### Fixed
- Silent downgrade via unlabeled sink: `Secret<T>` and other labeled types no longer silently pass to unlabeled parameters (any untyped parameter now treated as `Public` context)
- Implicit flow through `if`-expressions: condition label is joined into branch result types, preventing information leakage via guard value
- Pre-existing gap: implicit return type in `infer_block_type()` was not checked against the declared return type; `TypeMismatch` now emitted on mismatch
- `resolve()` was silently stripping `TypeExpr::Labeled`; now preserved as `Ty::Labeled`

## [0.4.0] — 2026-04-11 (Epic 2: Effects, termination, and concurrency checking)

### Added
- Effect propagation checking (Req 7): callee effects must be declared by caller; `UndeclaredEffect` and `MissingEffect` errors (#20)
- Totality/termination checking (Req 8): `total` functions may not contain `while` loops or call `partial` functions (#21)
- Reference capability checking (Req 9): `ref` and `tag` parameters rejected at actor-boundary `channel.send()` (#22)
- `Literal::Unit` AST variant to represent `()` unit expressions
- 4 corpus test files: `propagation.mvl`, `pure_vs_effectful.mvl`, `total_vs_partial.mvl`, `capabilities.mvl`
- 16 new type-checker integration tests; 179 total tests

### Fixed
- Parser infinite loop on `Ok(())` and similar unit-literal expressions: eagerly detect `()` in `parse_atom` (#18)
- Force-advance guard in `parse_block` and `parse_module_decl` prevents stall-loop in error recovery
- `VarInfo` extended with optional `Capability` field for actor-boundary enforcement

## [0.3.1] — 2026-04-11

### Added
- **Tree-sitter grammar** (`etc/tree-sitter-mvl/`) — full GLR grammar for MVL covering all language constructs: totality modifiers, security labels, capability annotations, effects, refinement types, sanitize/declassify special forms, and path expressions. Includes 24 corpus tests and highlight queries (`highlights.scm`, `folds.scm`, `indents.scm`). (#35)
- **VS Code extension** (`etc/vscode-mvl/`) — TextMate grammar with syntax highlighting for all MVL constructs, bracket matching, auto-close pairs, comment toggling, code folding, and 4-space indent rules.
- **Zed extension** (`etc/zed-mvl/`) — Native tree-sitter integration with highlight queries, smart indentation, and bracket matching for `.mvl` files.
- **Neovim plugin** (`etc/nvim-mvl/`) — nvim-treesitter integration with parser registration, highlight queries, smart indentation (`indents.scm`), folding, and MVL-specific filetype settings (`commentstring`, 4-space indent).

## [0.3.0] — 2026-04-11 (Epic 2: MVL type checker)

### Added
- Type checker: two-pass design (collect declarations → check bodies) with full error accumulation (#10)
- Type inference for basic types: Int, Float, String, Bool, Char, Byte; arithmetic, comparison, logic ops (#11)
- ADT checking: struct field presence/type validation, enum field-access rejection (#12)
- Exhaustive match: enums, Option<T>, Result<T,E>; bare-ident variant patterns handled (#13)
- Option/Result enforcement: no direct field access on Option, ResultIgnored detection, `?` propagation check (#14)
- Immutability enforcement: reject assignment to non-`mut` bindings and non-`mut` struct fields (#17)
- Ownership/borrow checking: use-after-`move(x)` detection (#15)
- Refinement types: integer-predicate refinements parse and type-check; corpus validates grammar (#16)
- 7 corpus files + 22 integration tests + 7 new unit tests; 163 total tests
- `types_compatible` made recursive: `Result<Int, Unknown>` unifies with `Result<Int, String>`
- Enum constructors as expressions (`Ok`, `Err`, `Some`, `None`, user variants) no longer emit false errors
- Assignment type-compatibility check (not just mutability)
- `infer_block_type` helper: implicit return values not flagged as `ResultIgnored`
- If-expression branch type mismatch detection

## [0.2.0] — 2026-04-11 (Epic 1: MVL parser, source → AST)

### Added
- Lexer: tokenize all MVL keywords, operators, literals with precise Span (line, col, offset, len) (#2)
- AST: typed node definitions for every grammar construct — Program, Decl, TypeExpr, Expr, Stmt, Pattern (#3)
- Parser — type declarations: struct, enum, alias, refined alias with where-predicates (#4)
- Parser — function declarations: totality, capabilities, security labels, effects, where-constraints (#5)
- Parser — statements: let/mut, if/else, match, for, while, return, assignment, expression stmts (#6)
- Parser — expressions: Pratt precedence climbing, calls, field/method access, ?, if/match, lists, struct init (#7)
- Corpus: security label tests (Public/Tainted/Secret/Clean) (#8)
- Diagnostics: error recovery, multi-error reporting, source-line caret rendering (#9)
- 106 unit tests + 4 integration tests; cargo clippy clean; cargo fmt compliant

### Fixed
- Lexer: recursive `next_token` → iterative loop (stack overflow on many unknown chars)
- Lexer: integer literal overflow now emits a `LexError` instead of silently producing `0`
- Lexer: `struct` and `enum` reserved as `TokenKind` variants (were plain identifiers)
- Parser: `try_parse_return_refinement` atomically restores pos + errors on failure
- Parser: `parse_const_decl` wired to `parse_expr` (removed `_stub_` placeholder)
- Parser: effect list restricted to `Ident`-only (keywords like `where` were silently consumed)
- Parser: `parse_enum_body` now breaks on variant parse error (matched `parse_struct_body`)
- Parser: negative integer literal patterns supported in `match` arms (`-1 => …`)
- Parser: `parse_program` force-advances to prevent theoretical infinite loop
- Parser: `parse_type_params_decl` reports error on missing closing `>`

### Changed
- `Parser::errors` field is now `pub(crate)`; external code uses the `errors()` accessor

## [0.1.0] — 2026-04-10

### Added
- Project structure: `src/mvl/{parser,checker,transpiler}`, test hierarchy
- OpenSpec: 3 specs (type system, effect system, IFC), 5 ADRs
- EBNF grammar (~100 productions, LL(1))
- Standard library specification (three tiers: core, standard, extended)
- Language reference and introduction documentation
- mkdocs site with Material theme
- Two corpus examples: auth_handler.mvl, safe_division.mvl
- 34 GitHub issues across 5 epics (Phase 1: Rust transpilation)
