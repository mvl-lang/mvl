# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.16.0] — 2026-04-14 (feat: termination checker — Req 8 structural recursion)

### Added
- `src/mvl/checker/termination.rs` — structural recursion checker for Req 8 (Termination)
  - Two decrease measures: integer decrement (`param - N`, N > 0) and structural subterm (sub-pattern bindings from direct parameter matches)
  - New error `CheckError::UnprovenRecursion` emitted for non-terminating `total fn` recursion
  - Integrates automatically with `BasicCheckPass` verdict framework (Req 8 verdict)
  - Pre-type-check architectural pattern (Req 8 verdict proves: no unbounded loops or unproven recursive calls)
- `docs/specs/007-termination.md` — formal specification of the termination checker
  - 5 requirements covering both decrease measures, scope/defaults, lambdas, for/while loops
  - Known limitations (mutual recursion, while-loop measures, signed-int soundness, subterm shadowing) with deferred tracking (#142)
  - Comprehensive test coverage map

### Fixed
- **Termination checker: multi-parameter function decrease detection** — now correctly accepts decreasing arguments by identifier against all parameters, not just positional match. `f(a, b - 1)` and `f(b - 1, a)` both correctly accepted when `a` and `b` are parameters.
- **Termination checker: refactoring and optimizations**
  - Extracted `check_match_arms` + `check_match_body` helpers to eliminate Stmt::Match/Expr::Match duplication
  - Eliminated unnecessary HashSet clone in match-arm iteration
  - Optimized `leaf_idents` to use `Option::into_iter()` (no Vec allocation)
  - Updated `ok_evidence` string in `passes.rs` to reflect recursive call checking
  - Added precondition comment for while-loop pass ordering dependency

### Tests
- 6 new termination-checker tests (all spec-linked to 007-termination.md):
  - `decrement_by_zero_in_total_fn_rejected` — boundary case: N==0 not a decrease
  - `decrement_on_second_param_accepted` — confirms any-parameter matching, not positional
  - `explicit_total_fn_keyword_unbounded_rejected` — explicit `total fn` checked like implicit
  - `structural_recursion_on_adt_single_field_accepted` — single-field TupleStruct subterm
  - `structural_recursion_via_non_param_match_rejected` — non-param scrutinee doesn't grant subterm
  - `recursion_inside_lambda_not_flagged` — lambda scope exclusion confirmed
- Tightened `increasing_recursion_in_total_fn_rejected` to assert `fn_name == "bad"`

### Part of
- Issue #135 (closes)
- Epic Phase 3 (#129)

---

## [0.15.0] — 2026-04-14 (feat: mvl linter — Phase 2 semantic lint rules)

### Added
- `mvl lint` Phase-2 semantic rules — 5 new rules catch logical issues in otherwise well-typed code:
  - `unreachable-code` — flags statements after `return` in a block
  - `redundant-match` — flags single-arm `match` with irrefutable pattern (suggests `let` instead)
  - `unnecessary-annotations` — flags `let x: Int = 42` where type is unambiguous from literal
  - `redundant-effects` — flags effect declarations on functions containing no calls
  - `redundant-ifc-labels` — flags `Public<T>` annotations (redundant base IFC label)
- All Phase-2 rules integrated with config system; individually disableable via `.mvllintrc`
- `--show-config` now displays Phase-1 and Phase-2 sections separately
- 35 new unit tests covering all Phase-2 rule edge cases

### Testing
- All 253 unit tests pass; no regressions in Phase-1 rules
- Phase-2 rules tested for config disable, nested control flow, literal type detection, and IFC label traversal

### Bug Fixes
- Deduped redundant-match detection (was split across two code paths)
- Added `else if` chain recursion to redundant-match and unnecessary-annotations rules
- Added tuple variant handling to redundant-ifc-labels for enum variants
- Fixed integer overflow in line_length column cast on pathological config values

## [0.14.0] — 2026-04-14 (feat: mvl linter — Phase 1 style rules)

### Added
- `mvl lint <file|dir>` command with 6 Phase-1 style rules:
  - trailing-whitespace, line-length (configurable, default 120)
  - indentation (space/tab consistency, width validation)
  - final-newline, naming conventions (snake_case/PascalCase/SCREAMING_SNAKE_CASE)
  - function body length (LLM-relevant decomposition signal, default 50 lines)
- Configuration system: `.mvllintrc` (project-local) and `~/.config/mvl/lintrc` (XDG global)
  - Settings: `line_length`, `indent_size`, `indent_style`, `max_fn_length`, `naming`, `trailing_ws`, `unused_bindings`
  - Supports simple `key = value` format; all optional with sensible defaults
- `mvl lint --show-config` to display active configuration
- `make mvl-lint` Makefile target to run linter across corpus and examples
- Full unit test suite for all Phase-1 rules

### Design Notes
- Phase 1 scope: style rules only. Phase 2 (semantic lint) and Phase 3 (LLM-specific rules) are follow-up work.
- No external dependencies; config parser and rule engine written in pure Rust.
- LLM-relevant: consistent formatting + function length limits improve model output quality.

## [0.13.0] — 2026-04-13 (feat: access_control — Phase 2 security reference example)

### Added
- `examples/access_control/` — multi-file MVL program demonstrating compile-time security guarantees: SQL injection impossible via `Secret<String>` consumed at extern boundary (IFC), credential leakage is a type error, missing permission checks fail to compile (totality), side effects separated from pure policy (effect declarations)
- `main.mvl` — entry point with 3 extern trust-boundary fns (`hash_verify`, `generate_token`, `get_demo_hash`), `total fn check_permission` exhaustive over Role × Resource × Action, IFC demonstration pipeline
- `model.mvl` — domain types: `Role`, `Resource`, `Action`, `Permission`, `AuthError`, `AppError`
- `auth.mvl` — credential verification with IFC: `Secret<String>` password hash passed to `hash_verify` — CANNOT flow to `println` (compile error); `Tainted<String>` → `sanitize()` → `Clean<String>` conversion
- `rbac.mvl` — `total fn check_permission` — exhaustive `match` on all Role × Resource × Action combinations; missing arm = compile error
- `audit.mvl` — audit logging with `! Log, Console` effect declarations; IFC enforces `Secret<T>` never reaches output
- `bridge.rs` — Rust stubs: `hash_verify`, `generate_token`, `get_demo_hash` (trust boundary implementations)
- `Makefile` — `build/check/test/run` targets (mirrors `log_analyzer` pattern)
- `rbac_test.mvl` — 17 standalone tests covering Role × Resource × Action combinations
