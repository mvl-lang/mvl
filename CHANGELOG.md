# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.25.0] — 2026-04-15 (feat: extended collection operations — take, skip, windows, chunks, flatten, partition, group_by)

### Added

- **Extended collection operations** — Nine new methods for List<T> providing advanced collection manipulation (#64):
  - **Slicing operations**: `take(n)`, `skip(n)` — take/drop first N elements
  - **Predicate-based slicing**: `take_while(f)`, `skip_while(f)` — take/skip while predicate holds
  - **Windowing**: `windows(n)` — sliding window over elements, `chunks(n)` — fixed-size chunking
  - **Merging**: `flatten()` — flatten List<List<T>> to List<T>
  - **Partitioning**: `partition(f)` — split list by predicate, `group_by(f)` — group by key function returning Map<K, List<T>>
- **Full runtime coverage** — 26 new tests for collection operations validating element preservation, edge cases (empty lists, boundaries), and correct value assertions. Brings stdlib test suite from 76 to 114 tests.
- **Transpiler support** — Special-case Rust emission for 11 methods that don't map 1:1 to stdlib methods (iterator adapters, map/collect patterns, HashMap fold for group_by, Option<&T> to Option<T> conversion for first/last, reference borrowing for contains).

### Fixed

- **Review findings from PR #166** — Added missing type inference rules and transpiler handlers for all 9 new collection methods, ensured complete test coverage for behavior validation.

## [0.24.1] — 2026-04-15 (fix: prevent runtime panics in slice/substring operations)

### Fixed

- **Slice/substring safety** — Emit safe Rust block expressions for `slice` and `substring` that prevent panics on negative or out-of-bounds indices. Both operations now clamp indices to valid ranges, handle inverted ranges gracefully, and never panic. `substring` uses char-based iteration for UTF-8 safety.
- **Checker validation for slice/substring** — Add argument count and type validation in the type checker. Wrong argument counts or non-`Int` arguments now return `Ty::Unknown` instead of silently accepting the call, allowing the checker to catch misuse before transpilation.
- **Safety contract test coverage** — Add comprehensive tests for documented safety contracts: out-of-bounds indices, inverted ranges, and boundary conditions for both `slice` and `substring`.
## [0.24.0] — 2026-04-15 (feat: stdlib correctness test suite with 76 tests)

### Added

- **Stdlib correctness test suite** — 76 runtime tests across 5 modules (arithmetic, option, result, string, collections) validating stdlib method behavior. Separate from corpus tests which validate parser/type-checker. Includes tests for edge cases (zero values, negative rounds, ? propagation) and known transpiler limitations documented in file headers.
- **`make test-stdlib` target** — Integrates with CI to catch stdlib regressions. Wired into `make test` gate.
- **MVL variable in Makefile** — `MVL ?= ./target/debug/mvl` allows CI override without modifying Makefile.

### Fixed

- **int_max/int_min false-positive coverage gap** — Added `int_max_when_self_is_larger` and `int_min_when_self_is_smaller` tests to prevent trivial implementations from passing the suite.

## [0.23.0] — 2026-04-15 (feat: -- path separator support; fix: test-transpiler corpus resilience)

### Added

- **`--` path separator for all subcommands** — All commands (`run`, `check`, `build`, `transpile`, `test`, `lint`, `assurance`) now accept an optional `--` before the file/directory argument, following standard POSIX/Cargo convention. This allows unambiguous handling of paths that start with a dash. For `run`, args after a second `--` continue to be forwarded to the compiled binary: `mvl run [--] <file.mvl> [-- <binary-args>]`.

### Fixed

- **test-transpiler corpus resilience** — Programs were relocated from `corpus/09_full_programs` to `corpus/11_programs` during corpus restructure (PR #160). Manual session now uses `find` to dynamically locate files, skipping gracefully if not found instead of failing hard.
- **Binary versioning in test-transpiler** — Use `./target/debug/mvl` built by the `build` prerequisite instead of repeated `cargo run`, eliminating risk of stale system binary and removing per-file cargo overhead.
- **Manual session error handling** — Lowercase `mvl` variable to avoid shadowing environment vars; strip trailing newlines from `find` output; fail fast on any non-zero exit from the compiled binary.

## [0.22.1] — 2026-04-15 (fix: corpus test output suppression)

### Fixed

- **Corpus test output** — Suppress checker error output for `corpus:expect-fail` files and show clean confirmation (`OK (violations detected as expected)`) instead of raw error lines. Restores per-file progress output for normal files.

## [0.22.0] — 2026-04-15 (feat: embed stdlib source in binary, extract to XDG on first run)

### Added

- **Stdlib embedding and XDG extraction** — MVL stdlib source files are now embedded in the binary at compile time using Rust's `include_str!` macro. On first run, they are extracted to `$XDG_DATA_HOME/mvl/std/` (or `$MVL_HOME/std/` if set). Provides verifiable, portable stdlib distribution.
  - `.version` stamp tracks compiler↔stdlib version match; auto-re-extracts on version mismatch.
  - Three-location resolver: project modules → extracted stdlib → stdlib packages (future).
  - `mvl init [--stdlib]` command for explicit extraction (called automatically by check/build/run).

- **Specification registration** — Added Specs 004 (Testing) and 006 (Trust Boundary Bridge) with YAML frontmatter and symlinks from `docs/specs/`. Integrated ADR-0009 (XDG paths and source resolution) with rationale for no-compression approach.

### Fixed

- **Stdlib module surface consistency** — Removed `"print"` from the Phase 1 stub so it matches `std/core.mvl`. Code now resolves identically against filesystem-backed and fallback stubs.
- **Resolver integration in assurance** — `cmd_assurance` now calls `ensure_stdlib()` and `resolve_project()` to surface import errors before reporting. Previously only `cmd_check` and `build_project` had stdlib wiring.
- **Silent I/O error in stdlib loading** — `load_stdlib_module()` now emits a warning when `core.mvl` cannot be read (permissions, missing file). Previously all read errors were silent.
- **Test harness robustness** — `with_mvl_home()` test helper now uses RAII guard (`MvlHomeGuard` Drop impl) to clean up `MVL_HOME` even if the test panics. Replaced mtime-based idempotency checks with content comparison (fixes false positives on fast filesystems).

## [0.21.0] — 2026-04-15 (feat: stdlib method resolution for Int + Float types)

### Added

- **Stdlib method dispatch for Int and Float** — Method calls on Int and Float now resolve to concrete types instead of `Unknown`. Completes prelude type method coverage alongside String, List, Map, Set.
  - **Int methods**: to_float, to_string, abs, pow, min, max, clamp, is_positive, is_negative, is_zero
  - **Float methods**: to_int, to_string, abs, ceil, floor, round, sqrt, min, max, clamp, is_nan, is_finite, is_infinite, is_positive, is_negative
- **Corpus: core_types.mvl** — Demonstrates all eight prelude types (Int, Float, Bool, String, Array, Map, Set, Option, Result) with basic operations. Validates type checking and method dispatch across the full stdlib surface.

### Fixed

- **Corpus test harness** — New core_types.mvl corpus is now wired into tests/type_checker.rs via test function, ensuring all method types are actively validated.

## [0.20.0] — 2026-04-14 (feat: stdlib method resolution for string + collection ops)

### Added

- **Stdlib method dispatch** — Method calls on String, List, Map, Set now resolve to concrete types instead of `Unknown`. Supports 40+ methods across all collection types with correct return types (Option<T> for safe access, proper type inference for map/filter/fold).
  - **String methods**: split, trim, find, replace, to_upper/lower, len, contains, starts_with, ends_with, chars, lines, parse_int, parse_float, format
  - **List methods**: map (infers element type from function return), filter, fold, reduce, sort, enumerate, zip, join, min/max, find, any/all, flat_map, push/extend, first/last/get, dedup
  - **Map methods**: get, contains_key, keys, values, entries, len, insert, remove
  - **Set methods**: contains, len, to_list, insert, remove, union/intersection/difference

### Fixed

- **IFC label propagation for method calls** — Receiver and argument labels now propagate to method results. Previously, `secret_str.contains("x")` lost the Secret label.
- **Implicit flow analysis for MethodCall** — Added MethodCall arm to `infer_label` so that method results used in control flow are tracked for implicit-flow violations.
- **For-loop PC elevation** — Iterator security label now elevates the Program Counter in the loop body, consistent with While/If handlers.
- **format() IFC compliance** — Argument labels are joined into the result per spec 003/Req 7. `format("x={}", secret_val)` correctly returns `Secret<String>`.
- **reduce return type** — Separated from fold; reduce now returns `Option<T>` (empty list case) instead of the first argument type.

### Tests

- **Corpus: collections.mvl** — 30+ functions demonstrating all string and collection methods, all return types verified.

## [0.19.2] — 2026-04-14 (fix: checker label-promotion refactoring and regression tests)

### Fixed

- **Label promotion in if-statements** — Type checker now correctly promotes branch result types when the condition is labeled (`Secret<Bool>`, `Tainted<Bool>`, etc.). The implicit return of a branch inherits the condition's security label if it carries information (non-Unit, non-Unknown).
- **Labeled Bool condition acceptance** — Conditions of type `Secret<Bool>` and `Tainted<Bool>` are now accepted (previously rejected with spurious TypeMismatch). The `is_bool()` method correctly strips security labels before checking for Bool base type.

### Changed

- **Refactored branch label promotion** — Extracted duplicated promotion logic from then-branch and else-branch into `check_branch_label_promotion()` helper method, improving code maintainability.

### Tests

- **Added regression tests** — `secret_bool_if_condition_accepted`, `tainted_bool_while_condition_accepted`, `secret_int_if_condition_rejected` verify that labeled Bool conditions work correctly and unlabeled non-Bool types are still rejected.

## [0.19.1] — 2026-04-14 (fix: bridge.rs hardening and test coverage)

### Added

- **Bridge discovery and injection tests** — Spec 006 compliance: unit tests for `inject_mod_bridge` (inserts after marker, fallback prepend, no truncation), unit tests for `has_extern_rust_decls` (ABI discrimination: rust vs c), integration tests for missing-bridge error and valid-bridge build success, and integration test for symlink-escape hardening.
- **Symlink-escape test** — `bridge_symlink_outside_source_dir_rejected` verifies that `mvl build` rejects bridge.rs files that symlink outside the source directory.

### Changed

- **Bridge path security** — Replaced `exists()` + `canonicalize()` pattern (TOCTOU race window) with direct `canonicalize()` call handling `NotFound` as the no-bridge case.
- **Bridge copy atomic operation** — Replaced `read_to_string()` + `write()` with `fs::copy()` (single syscall) to eliminate the race window between scope validation and file read.
- **Runtime copy guard** — Changed condition from `extern_count > 0` to `has_extern_rust` (Spec 006 Req 6), so only `extern "rust"` programs trigger MVL runtime copy, not `extern "c"`.
- **Error message disclosure** — Symlink-escape error no longer prints canonical internal paths.

## [0.19.0] — 2026-04-14 (feat: checker phase 3 — implicit flow analysis and Proven verdict)

### Added

- **Implicit Flow Control (IFC Phase 3)** — Requirement 11: Information Flow Control now detects
  implicit information flows via control flow (Program Counter label analysis). A `println` or `print`
  call that appears inside a branch controlled by a `Secret` or `Tainted` condition is now a compile error,
  even if the printed arguments are `Public`. The rationale: whether a print fires reveals the value
  of the controlling condition, creating a covert channel.

  - **`ImplicitFlowViolation`** — new error type for control-flow leaks.
  - **`IFCPass`** — new verification pass that combines Phase 1 direct-flow violations with Phase 3
    implicit-flow analysis to produce verdicts: `Failed` (violations), `Proven` (no violations + labeled types),
    or `Unchecked` (no violations but no labeled types).
  - **`check_implicit_flows`** — new analyzer that performs Program Counter label inference:
    - Tracks PC label through `if`, `else`, `while`, `for`, and `match` statements.
    - Flags implicit flows to `println`/`print` sinks.
    - Supports `declassify()` as an escape hatch for lowering the PC label.
    - Includes known limitations: cross-function flows, label inference through unannotated bindings,
      and nested-loop PC reset deferred to Phase 6.
  - **Assurance evidence** — `Proven` verdicts include audit counts of declassification and
    sanitization points so that auditors can verify every downgrade point.

### Fixed

- **Spec numbering** — Requirement 11 (Implicit Flows) in `specs/003-information-flow/spec.md` was locally
  numbered as "Requirement 8"; renamed to "Requirement 11" for correct system-level traceability.
- **Missing `Proven` test** — added integration tests for Req 11: `req11_proven_for_labeled_types_with_no_violations`
  and `req11_proven_evidence_contains_audit_counts` exercise the `Proven` verdict path.
- **`Stmt::While` with Secret condition** — added `implicit_flow_while_secret_condition_rejected` test
  to verify while-loops with secret-controlled conditions are flagged.

## [0.18.0] — 2026-04-14 (feat: linter phase 3 — LLM corpus quality rules)

### Added

- **`consistent-comment-style`** — source rule that flags block comments (`/* */`),
  which are not part of the MVL grammar. Only `//` and `///` are allowed.
  Enabled by default; disable with `consistent_comment_style = false`.

- **`missing-doc-comment`** — hybrid rule (AST + source) that requires a `///` doc
  comment on every `pub` function, type, and const declaration.
  Enabled by default; disable with `require_doc_comments = false`.

- **`doc-comment-example`** — source rule that recommends an `Example:` section
  inside `///` doc-comment blocks on public items.
  Opt-in (`doc_comment_examples = false` by default).

- 23 new unit tests covering all three rules (positive detection, clean cases,
  config-disable paths, edge cases, and design-decision pins from review).

### Fixed

- `consistent_comment_style`: skip `/*` appearing after `//` on the same line
  (false positive when `/*` was inside a line comment).
- `collect_doc_lines_before`: replaced fragile manual index loop with idiomatic
  `for i in (0..n).rev()` iterator.

### Notes

- Function body length (`fn-length`, Phase 1) already covers the fourth Phase 3
  requirement; no duplication added.
- Comments remain discarded by the lexer; Phase 3 rules use source-line
  correlation with AST spans for doc-comment detection (same approach as `fn-length`).

## [0.17.0] — 2026-04-14

### Added
- **Data Race Freedom Checker (Req 9, Phase 3 partial)** — `src/mvl/checker/data_race.rs`
  - `check_iso_aliasing()` — detects `iso` variable aliasing via bare let-bindings, assignments, and lambda captures
  - `count_race_free_fns()` — classifies functions as provably race-free when they have no `ref` parameters
  - `DataRaceFreedomPass` — verification pass that returns `Proven` when all functions are race-free, `Unchecked` when `ref` parameters require actor-model analysis (Phase 6)
  - `docs/specs/008-data-race-freedom.md` — formal specification of the reference capability model (iso/val/ref/tag), sendability rule (Req 1), isolation rule (Req 2), function classification (Req 3), and known limitations (L1–L5)
  - 16 new tests covering aliasing detection, control flow integration, limitation regression, and lambda captures (AST-level)

### Fixed
- **Data race freedom aliasing detection improvements:**
  - `Stmt::Assign` now applies the same aliasing guard as `Stmt::Let` — `y = iso_x` is flagged as a violation
  - Lambda body recursion — `check_expr_iso` now recurses into `Expr::Lambda` bodies with correct parameter shadowing
  - `DataRaceFreedomPass` now uses `self.requirement()` instead of hardcoded `[9]` index (maintenance safety)
  - Corrected spec limitation L4 — both alias sites are reported independently, not just the first
  - Added L5 limitation — iso rebinding after `consume()` is not tracked (Phase 6 work)

### Tests
- `req9_failed_for_iso_aliasing_violation` — exercises `Verdict::Failed` branch (was completely untested)
- `req9_unchecked_for_empty_program` — covers zero-function edge case
- `req9_proven_evidence_references_phase6` — verifies evidence string requirement
- `iso_aliasing_via_assignment_rejected` — integration test for Stmt::Assign fix
- `iso_aliasing_inside_if_branch_rejected` — control flow coverage
- `iso_aliasing_inside_lambda_body_rejected` — AST-level unit test (lambda syntax not yet parsed)
- `lambda_param_shadowing_iso_not_flagged` — shadowing semantics correctness
- Limitation regression tests: L1 (`iso_passed_to_fn_call_not_detected_l1`), L5 (`iso_rebound_after_consume_not_detected_l5`), and L4 documentation (`iso_multiple_aliasing_all_sites_reported`)
- Test count increased: 458 passing (from 255)
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
