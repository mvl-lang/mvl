# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

## [0.1.0] — TBD (Phase 1 complete)

Target: both corpus examples compile via Rust transpilation, all 11 requirements demonstrated.

## [0.2.0] — TBD (Phase 2 complete)

Target: LLVM IR backend, self-hosting.

## [0.3.0] — TBD (Phase 3 complete)

Target: MVL compiler written in MVL, ecosystem (package manager, tooling).
