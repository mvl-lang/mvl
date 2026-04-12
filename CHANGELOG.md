# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.0] — 2026-04-12 (Unified CLI: check/build/test/assurance + grammar coverage)

### Added
- **`mvl check <dir>`** — type-check all `.mvl` files in a directory (closes #94)
- **`mvl build <dir>`** — transpile directory-based projects (looks for `main.mvl`/`mod.mvl`/`lib.mvl` entry point)
- **`mvl test <file|dir>`** — find `*_test.mvl` files, transpile to a combined Rust test crate, run `cargo test`
- **`mvl assurance <file|dir> [--json]`** — report function totality, extern trust boundary, and type error counts in human or JSON format
- **`tools/check_grammar_coverage.py`** — cross-validates `docs/grammar.ebnf` production names against `etc/tree-sitter-mvl/grammar.js` rules; exits 1 on unexpected gaps in either direction
- Tree-sitter grammar: `module_decl`, `extern_decl`, `extern_fn_decl` rules; 26/26 corpus tests passing
- `make test-grammar-coverage` and `make test-tree-sitter` targets; both hooked into `make test`

### Fixed
- `cmd_test` uses a per-PID temp directory to prevent concurrent invocation collisions
- `cmd_assurance` correctly counts extern function *signatures* (not blocks) for trust-boundary percentage
- `.expect()` on I/O operations replaced with clean user-facing error messages
- `cargo`-not-in-PATH now prints actionable install message rather than panicking
- `strip_suffix("_test")` replaces `trim_end_matches` (which stripped individual chars, not the literal suffix)

## [0.6.0] — 2026-04-12 (FFI: extern blocks, mvl_runtime, password_checker demo)

### Added
- **FFI infrastructure** — `extern "rust"` and `extern "c"` blocks for explicit trust boundaries (closes #52)
- **`mvl_runtime` crate** — zero-dependency Rust crate providing:
  - IFC newtypes: `Public<T>`, `Tainted<T>`, `Secret<T>`, `Clean<T>` with `#[repr(transparent)]`
  - Effect markers: `Console`, `Net`, `Db`, `FileRead`, `FileWrite`, `Concurrent`, `Alloc`, `Panic`
  - Refinement macro: `mvl_refine!(pred)` for debug assertions
  - Prelude: single `use mvl_runtime::prelude::*` for generated files (closes #91)
- **`password_checker.mvl` demo** — non-trivial FFI example showing full stack: extern Rust trust boundary, IFC label flow (Tainted → Clean → Secret), refinement types, effects (closes #93)

### Fixed
- Checker: `extern_count` now reflects only validated (non-rejected) extern blocks toward assurance surface
- Transpiler: extern block codegen skips unknown ABIs instead of passing through; no `pub` in extern block fn decls (invalid Rust)
- IFC security: `Secret<T>` no longer implements `Display` (prevents accidental confidential data leaks); `Debug` prints `"Secret([REDACTED])"`
- Demo: `sanitize()` called after guard check (correct IFC contract ordering); `stored_hash` typed as `Secret<String>` in extern signature (no manual IFC bypass)
- Cargo.toml generation: replaced wildcard `"*"` version with `"0.1"` placeholder + pin-before-publish comment

## [0.5.6+modules] — 2026-04-12 (Module system spec)

### Added
- Spec 005: module system — file=module, `pub` visibility, `use` imports, re-exports,
  circular import rejection, explicit stdlib (closes #47)
- Grammar: `use_decl`, `reexport_decl`, `module_path`, `decl_body` productions;
  `pub` modifier factored out to preserve LL(1); `module_decl` block removed
- tree-sitter `grammar.js`: `use_decl`, `reexport_decl`, `module_path` nodes;
  fixes broken highlight queries in nvim/zed/tree-sitter backends
- Syntax highlighters (nvim, zed, vscode, tree-sitter): `use` and `pub` keywords;
  module path namespace highlighting
- Docs: manual chapter 13 rewritten for file=module model

### Fixed
- LL(1) property restored: `pub` factored into `declaration = ["pub"] decl_body`
  so each alternative starts with a distinct token
- Spec EBNF aligned with canonical `grammar.ebnf` (added `[security]` to `fn_decl`)

## [0.5.6] — 2026-04-12 (Transpiler: end-to-end compile for reference examples)

### Fixed
- Transpiler: external types referenced in function signatures (e.g. `UserStore`)
  now get auto-generated `pub struct Stub;` placeholders so the emitted Rust compiles
- Transpiler: method calls on external-type parameters now produce `impl Stub { fn method() }`
  stubs with return types inferred from let-binding annotations and `?`-propagation
- Transpiler: security label newtypes (`Public`, `Tainted`, `Secret`, `Clean`) now
  emit `Copy` (when inner type is `Copy`), `Display`, and arithmetic operator impls
  (`Add`, `Sub`, `Mul`, `Div`, `Rem`, `Neg`) — enabling labeled arithmetic and `println!`
- Transpiler: `Public<i64>` gains a `to_float()` helper for integer→float conversions
- Transpiler: refined newtypes over primitive MVL types (`Int`, `Float`, `Bool`, `Char`,
  `Byte`) now derive `Copy`, eliminating spurious "value moved" errors
- Transpiler: tail expressions of labeled return type (`Secret<String>`, `Public<Float>`)
  are automatically wrapped — e.g. `{ "token" }` → `Secret("token".to_string())`;
  `Ok(f)` where f is an unlabeled param → `Ok(Public(f))`
- Corpus: `auth_handler.mvl` — renamed `DbConn` → `UserStore`, effect `! DB` → `! IO`

All 7 corpus full programs now build end-to-end with `mvl build` (#90).

## [0.5.5] — 2026-04-12 (Corpus validation + Phase 1 transpiler)

### Added
- Phase 1 transpiler: MVL → Rust source (closes #29–#34)
  - `src/mvl/transpiler/` — codegen, emit_exprs, emit_functions, emit_stmts, emit_types, cargo modules
  - `mvl build <file.mvl>` — transpile + `cargo build`
  - `mvl run <file.mvl>` — transpile + build + execute
  - `mvl transpile <file.mvl>` — print generated Rust to stdout
  - Security label preamble (`Public<T>`, `Secret<T>`, `Tainted<T>`, `Clean<T>`) in every generated crate
  - Refinement type constructors with `debug_assert!` guards
  - Effect and totality annotations preserved as doc comments
- New corpus programs: `hello_world`, `hello_mvl`, `calculator`, `shapes`, `simple_math`
- `make test-transpiler` — end-to-end build chain tests
- `docs/compilation-model.md` — requirement preservation across Phase 1 (Rust) and Phase 2 (LLVM)
- Parser: path expressions (`Enum::Variant`) in expressions and patterns
- Parser: inline refinements in labeled types (`Public<Int where self > 0>`)
- Parser: float literals in refinement predicates

### Fixed
- Checker: field assignment now checks field type vs assigned value (not base struct type)
- Checker: match arm blocks use `infer_block_type` so tail `Ok(…)` / `Err(…)` expressions
  are treated as the arm's return value instead of being discarded as `ResultIgnored`
- Checker: named type aliases (e.g. `Amount = Float where …`) resolved transparently in
  return-type checks and arithmetic operand checks
- Checker: `abs`, `max`, `min`, `parse_int` registered as built-in functions
- Corpus: 10 files fixed across `01_basics`, `04_effects`, `05_ifc`, `09_full_programs`
- Transpiler: match block arms emit tail expression correctly (no spurious semicolon)
- `make test` now depends on `test-corpus` so corpus failures are caught by default

## [0.5.4] — 2026-04-12 (Roadmap accuracy)

### Fixed
- roadmap.md: version 0.5.2 → 0.5.3; Req 9 status corrected to partial (consistent with ADR-0001); ISPE PR report marked Done

## [0.5.3] — 2026-04-12 (Spec link audit + doc accuracy)

### Fixed
- Corrected implementation links in 001-type-system (Reqs 1-7), 002-effect-system (Req 1),
  and 003-information-flow (Reqs 1,3,4,7) — paths pointed to nonexistent src/mvl/types/
  and src/mvl/effects/ directories; all logic lives in src/mvl/checker/
- Added Tests links for all 20 newly-linked requirements; assurance ratio now 20/20 (100%)
- Assurance completeness: 8/29 (28%) → 20/29 (69%)
- introduction.md: corrected Rust requirement score from seven to six (no effect system in Rust)
- roadmap.md: updated to v0.5.2, marked 11/11 enforced (Reqs 10+11 promoted from parse-only)
- ADR-0001: updated implementation status table to v0.5.2 with accurate enforcement status

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
