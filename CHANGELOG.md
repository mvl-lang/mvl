# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.41.1] — 2026-04-18

### Fixed

- **String.concat() method chaining** — Fixed transpiler emitting concat with insufficient parentheses, causing chained method calls like `.len()` to bind to the argument expression instead of the concatenated String result. Now emits `((receiver).clone() + &(arg))` with outer parens to preserve method call precedence.

## [0.41.0] — 2026-04-18

### Added

- **Versioned toolchain stdlib directory layout** — `stdlib_path()` now returns `$XDG_DATA_HOME/mvl/toolchains/{version}/std/` instead of flat `$XDG_DATA_HOME/mvl/std/`, enabling multiple compiler versions to coexist on disk without overwriting each other's stdlib. Implements ADR-0009 Phase A (#220).

## [0.40.0] — 2026-04-18

### Added

- **`String.concat(other)` method** — idiomatic string concatenation via method syntax (`s.concat(other)`), consistent with other string methods. Transpiles to `(s).clone() + &(other)` preserving MVL value semantics (receiver not consumed). Type checker enforces exactly one `String` argument, emitting `WrongArgCount`/`TypeMismatch` for wrong-arity or non-String arguments (#231).
- **Smarter termination checking** — Extended decrease measures: `param / N` (N > 1) for logarithmic algorithms, `.tail()` / `.rest()` method accessors on parameters or structural subterms, and `subterm.len()` for structural subterm length. Catches binary search, merge sort, and recursive algorithms without requiring `partial` annotation (#237).

### Fixed

- **Clippy `manual_checked_ops` lint** — replaced `if x > 0 { a / x }` pattern in `coverage.rs` with `.checked_div(x).unwrap_or(100)` to satisfy Rust 1.95 clippy `-D warnings`.
## [0.39.0] — 2026-04-17

### Added

- **Generics constraint enforcement** — Parser rejects higher-kinded type syntax (`F<_>`) and inline constraint syntax (`<T: Ord>`). Type checker enforces trait bounds on type parameters used in comparison operators: `Ord` for ordering operators (`<`, `>`, `<=`, `>=`) and `Eq` for equality operators (`==`, `!=`). When a bound is missing, diagnostics point users to the required `where` clause syntax (#225).
- **Phase 4 gate test** — `range()` now has a real MVL body (partial fn with while loop and list mutation) instead of a stub. The transpiler emits non-stub prelude functions before user code, enabling MVL stdlib bodies to be transpiled from source rather than relying on hardcoded Rust mappings. Validates end-to-end: real stdlib function → use → transpile → compile → run (#229).
- **Comprehensive stdlib demo** — new `core_types_demo.mvl` exercises all 9 core types (Int, Float, Bool, String, List, Map, Set, Option, Result) with representative method calls and pattern matching, proving the foundation is complete.
- **stdlib_content() lookup helper** — centralised API for accessing embedded stdlib files by name; used in main.rs to eliminate hardcoded "core.mvl" string.

### Fixed

- **Prelude name collision guard** — user-defined functions now shadow prelude functions instead of producing duplicate Rust definitions when a user redefines a stdlib function.
- **Macro handling consistency** — added `eprintln` to the macro match arm in emit_exprs.rs (was missing, would have emitted plain `eprintln(...)` calls instead of `eprintln!` macros).
- **Silent parse error on core.mvl** — embedded stdlib parse errors now produce a clear diagnostic instead of silently producing a malformed AST and confusing Rust compilation errors.
- **Dead code removal** — removed `try_emit_special_fn` stub (always returned false) and its call site in emit_exprs.rs.

## [0.38.0] — 2026-04-17

### Added

- **`args.parse<T>()` — struct-derived CLI argument parsing** — The struct IS the arg spec. Field names become flag names (`--field`), `Bool` fields are presence flags, `Option<T>` fields are optional, refinement predicates validate at parse time. The transpiler generates `impl ParseFromArgs` for each concrete struct with parseable fields; no derive macro or DSL required (#55).

### Fixed

- **CLI argument parsing** — `get_arg()` now supports both `--flag value` (two-token) and `--flag=value` (single-token) syntax; previously only the two-token form was recognized.
- **Codegen safety** — Added `assert_safe_identifier()` validation in struct field parsing to prevent codegen injection if field names bypass lexer restrictions.
- **Error messages** — Removed tainted CLI values from parse error strings to avoid information-flow violations.

### Changed

- **ParseFromArgs visibility** — `emit_parse_from_args_impl` changed from `pub` to `pub(crate)` (internal use only).
- **Codegen robustness** — Silent catch-all in `emit_field_parse` replaced with `unreachable!()` to enforce sync between `is_parseable_field_type` and code emission.

## [0.37.0] — 2026-04-17

### Added

- **Iterator trait protocol** — full support for `impl Iterator<T> for X` declarations in the type checker and transpiler, enabling iteration over user-defined types via `for...in` loops (Spec 001 Req 11, #219).
- **Array iterator support** — `Array<T, N>` now recognized as implementing `Iterator<T>`, accepted in `for...in` loops alongside `List<T>` and user-declared iterators.
- **Iterator errors** — new `NotIterator` error for non-iterable types in `for...in` expressions, and `ForLoopInPartialFn` to enforce that `for` loops (which are bounded) cannot appear in `partial` functions.

### Fixed

- **Iterator type checking** — refactored `check_iterator_type` to eliminate duplicate error reporting; `List<T>`, `Array<T, N>`, and named types all flow through a single validation path.
- **Iterator transpilation** — replaced inefficient `split_at(len-1)` + `tail[0]` pattern with `split_last()` for cleaner method body emission in `impl Iterator`.
- **Iterator tests** — strengthened loop body assertions to verify element-type binding; added tests for error interaction (`ForLoopInPartialFn` + `NotIterator`) and fallback paths (missing `next` method).

## [0.36.0] — 2026-04-17

### Added

- **Lambda expression parsing** — full support for `|params: Type| expr` and `|params: Type| { block }` syntax in the recursive-descent parser, enabling iterator trait implementation (map, filter, fold) and unblocking Spec 001 Req 11 (#218).
- **Zero-parameter lambda syntax** — `|| expr` now parses correctly (handled as single `PipePipe` token dispatched in `parse_atom`).
- **Capability and mutable modifiers on lambda params** — lambda parameters now support `iso`, `val`, `ref`, `tag` capabilities and `mut` flag, matching function parameter syntax.
- **Recursion depth guard** — added configurable limit (`MAX_PARSE_DEPTH = 200`) on expression nesting depth to prevent stack overflow from adversarially nested lambdas or other deeply nested expressions.

### Fixed

- **Lambda parameter parsing** — corrected `recursion_inside_lambda_not_flagged` test to use typed parameter syntax `|x: Int|` instead of bare `|x|`.
- **Import organization** — moved `Param` import from function scope to module-level imports in expressions.rs for consistency with other AST types.

## [0.35.1] — 2026-04-17

### Fixed

- **println with non-string first arg** — transpiler now generates valid Rust format strings with one `{}` placeholder per argument when the first arg to `println`, `print`, or `format` is not a string literal. Addresses issue #198.
- **Test coverage** — added regression tests covering single non-string arg, multiple non-string args, and mixed-type args to prevent future regressions.

## [0.35.0] — 2026-04-16

### Added

- **FFI bridge smoke tests** — two minimal bridge corpus programs validate extern "rust" pipeline for `! Random` and `! Terminal` effect annotations: `random_dice` rolls a dice via bridge (std::time seeding, no ext crates), `tui_hello` clears screen and prints via ANSI escape codes (build-only in CI due to terminal dependency). Both include integration tests in `compile_and_run.rs` (#196).
- **log_analyzer CI test** — `examples/log_analyzer` (multi-file MVL with Rust bridge) now runs in CI: `log_analyzer_build_succeeds` and `log_analyzer_run_produces_json_summary` in `tests/compile_and_run.rs` (#195).

### Fixed

- **Concurrent bridge builds race** — changed `mvl_runtime` dependency path from shared `../mvl_runtime` to per-build `./mvl_runtime` inside `tmp_dir`; removed `remove_dir_all` guard to make copy idempotent, eliminating ENOENT races when multiple bridge tests run in parallel.
- **FFI bridge test helpers** — added `assert_build_ok` helper to eliminate repeated boilerplate, simplified `random_dice_runs_and_prints_dice_roll` to use `assert_run_output`.
- **Spec test links** — corrected 8 broken test identifiers in specs 001/002/003 (`*_compiles_and_runs` → split `*_check_passes` / `*_runs_and_produces_expected_output`).
- **Clippy `collapsible_match`** — collapsed nested `if` into match guards in `linter/rules.rs` and `transpiler/emit_functions.rs`.

## [0.34.0] — 2026-04-16 (feat: phase-4 complexity analysis rules)

### Added

- **Phase 4: Complexity analysis rules** — static complexity metrics measuring code regenerability.
  - `complexity-cyclomatic` — cyclomatic complexity per function (default max 10); counts if/else-if/match arms/while/for/&&/||.
  - `complexity-match-depth` — max nested match depth per function (default max 3).
  - `complexity-effect-width` — declared effects per function (default max 3); applies to both free functions and impl methods.
  - `complexity-trait-impl-count` — trait impl blocks per type (default max 5); replaces inheritance depth metric.
  - `complexity-module-fanout` — distinct root modules imported (default max 15).
  - `complexity-extern-ratio` — extern fns / total fns ratio (default max 20%); measures trust boundary width.
  - All thresholds configurable via `.mvllintrc`; `mvl lint --show-config` displays phase-4 settings.

### Fixed

- **Config parsing for `max_extern_ratio`** — now validates range [0.0, 1.0]; NaN or out-of-range values are silently ignored (forward-compat), preventing unintended rule disablement.

## [0.33.0] — 2026-04-16 (feat: recursive enum Box<T> support end-to-end)

### Added

- **Box<T> support for recursive ADTs** — enables recursive enums like `enum List { Nil, Cons(T, Box<List>) }`.
  - `Box::new(x)` constructor recognized in type checker, returns `Box<T>`.
  - `*expr` (dereference) unary operator in parser, checker, and Rust emitter.
  - Termination checker accepts `*subterm` as structurally decreasing for total function recursion.
  - `linked_list.mvl` corpus program — validates full transpilation pipeline end-to-end.

### Fixed

- **Type error diagnostics** — `*non_box_expr` now emits `TypeMismatch` instead of silently returning `Ty::Unknown` (prevents IFC label loss).
- **Box::new arity checking** — `Box::new(…)` with ≠ 1 argument now emits `WrongArgCount` error.

## [0.32.0] — 2026-04-16 (feature: end-to-end Result<T,E> validation)

### Added

- **End-to-end tests for `safe_division.mvl`** — validates Result<T,E>, match on Result, and error handling through the full transpiler pipeline (parse → check → transpile → rustc → run).
  - `safe_division_check_passes()` — confirms type checker accepts the program.
  - `safe_division_runs_and_produces_expected_output()` — validates runtime output and error handling via nested match patterns.
  - Demonstrates error visibility requirement (Req 5) in corpus, resolving #191.
## [0.31.2] — 2026-04-16 (fix: corpus test failures and code review improvements)

### Fixed

- **Corpus test failures** — resolved `statements.mvl` and `file_io.mvl` regressions that surfaced after PR #201.
  - Functions using explicit `return` in `if/else` branches were incorrectly flagged with `TypeMismatch` (e.g., `Unit` vs `Int`). Fixed by marking tail-return blocks as diverging (`Unknown` type).
  - Parser now accepts `.` as path separator in `use` declarations (e.g., `use std.io.{...}` instead of only `std::io`).
  - Parser now accepts `{…}` brace groups for selective imports in `use` declarations.
  - `std/io.mvl` now embedded and loaded in stdlib; `io` module is now visible to the type checker.
  - Pre-register stdlib declarations before checking user code via `check_with_prelude`.
  - Auto-derive stdlib submodule exports from `STDLIB_FILES` to prevent list drift.

### Changed

- **Type checker public API** — simplified `check()` to delegate to `check_with_prelude(&[], prog)`, eliminating code duplication.
- **CLI assurance mode** — now wires stdlib prelude to avoid false "undefined function" errors for stdlib imports.
- **Stdlib prelude loading** — falls back to embedded `STDLIB_FILES` when on-disk file is absent (read-only CI, unextracted `MVL_HOME`).

### Tests

- All 329 unit tests pass; corpus tests clean.

## [0.31.1] — 2026-04-15 (fix: tail-position match/if type checking)

### Fixed

- **`match` and `if/else` in tail position** — now correctly inferred as value-producing expressions with proper return-type checking (#189). Previous behavior silently treated them as statements returning `Unit`, masking type errors in tail-position patterns.
  - `else if` chains now recursively infer types for each branch; was only checking the then-branch type
  - IFC label promotion now correctly applied to all branches in `else if` chains
  - `ResultIgnored` check now fires for `match` in tail position, matching `Stmt::Expr` behavior
  - Overall return-type compatibility now checked for `if/else` in tail position
- **Tail-statement type checking** — fixed fallthrough logic to prevent double-execution of statement checks

### Tests

- Added 4 regression tests covering `else if` chains, `ResultIgnored` in tail `match`, and nested combinations

## [0.31.0] — 2026-04-15 (feat: stdlib structured logging — std.log with IFC enforcement)

### Added

- **std.log module** — structured logging with compile-time IFC enforcement, zero-cost effects (#54).
  - Four severity levels: `log_debug()`, `log_info()`, `log_warn()`, `log_error()`
  - Structured key-value fields (Map<String, String>) prevent accidental secret inclusion via string interpolation
  - `! Log` effect marker — functions without this effect provably never log
  - IFC enforcement: `Secret<T>`, `Tainted<T>`, and `Clean<T>` arguments rejected at compile time (OWASP A07 by construction)
- **IFC label propagation in map literals** — Map values' labels now join into the enclosing map type, catching secrets embedded in structured fields
- **Implicit flow analysis** — log functions added to `PUBLIC_SINKS` so calls inside high-PC branches are flagged (Phase 3)
- **Test coverage** — corpus test `tests/corpus/05_effects/logging.mvl` and 9 type checker tests including secret-in-fields-map validation

### Fixed

- **Map literal label propagation** — `{"key": secret_val}` now correctly types as `Secret<Map<String,String>>` so the log-sink IFC check catches embedded secrets
- **IFC spec cross-reference** — updated from wrong Req 11 to correct Req 6
- **Spec stale note** — removed "remains Phase 2" deferral from 003-information-flow/spec.md Req 6

## [0.30.0] — 2026-04-15 (feat: stdlib file I/O — std.io with effects and IFC)

### Added

- **std.io module** — comprehensive file I/O, path, and filesystem operations with effect tracking and information flow control (#44).
  - Types: `File`, `Path`, `BufReader`, `BufWriter`, `DirEntry`, `Metadata`, `Stdin`
  - Path construction (pure): `path()`, `join()`, `to_string()`
  - Path queries: `exists()`, `is_file()`, `is_dir()`
  - File I/O: `open()`, `close()`, `read_to_string()`, `write()`, `append()`, `buf_reader()`, `buf_writer()`, `read_line()`, `write_line()`
  - Filesystem operations: `create_dir_all()`, `remove()`, `read_dir()`, `metadata()`, `chmod()`, `create_symlink()`, `read_link()`
  - Standard input: `stdin()`, `stdin_read_line()`, `stdin_read_to_string()`
- **Effect system extension** — three new effects: `! FileRead`, `! FileWrite`, `! FileDelete` (distinct from `! Console` for line-oriented stdio).
- **IFC labeling** — all file-read and stdin operations return `Tainted<String>` for untrusted external data; symlink targets return `Tainted<String>` to prevent path-traversal attacks.
- Corpus test `tests/corpus/05_effects/file_io.mvl` covering effects, IFC, pure vs. effectful paths, and mixed-effect propagation.

### Fixed

- **Effect annotations** — `close()` is now effect-free (resource release, not I/O side-effect); read-only callers no longer forced to declare `! FileWrite`.
- **Security docstrings** — `create_symlink()` warns that targets must not derive from untrusted input; `chmod()` documents that setuid/setgid bits are unguarded.
- **IFC design** — `read_link()` returns `Tainted<String>` instead of bare `Path` to prevent silent path-traversal via disk data; `sanitize_path()` corpus placeholder updated with Phase 3 roadmap.
- **Test filter** — `file_io_corpus_parses_and_checks` now correctly handles opaque stdlib types (`UndefinedType` filtered) while catching real effect/IFC errors.
- **Negative tests** — added `caller_missing_file_write_effect_rejected` and `caller_missing_file_delete_effect_rejected`.

### Changed

- Phase 2 limitation: `open()` lacks `OpenMode` parameter, so write-mode effect visibility deferred to Phase 3 with `! FileWrite` enforcement.
- Corpus examples: removed unnecessary `Result<Bool>` wrappers on path queries; added `path_as_string()` exercising pure path conversion.

## [0.29.0] — 2026-04-15 (feat: Terminal effect and tui module — raw terminal control)

### Added

- **Terminal effect** — `! Terminal` fine-grained effect for raw terminal control (cursor positioning, colors, single-keypress input, screen clear), distinct from `! Console` (line-oriented I/O). Used by the upcoming `std.tui` / `pkg.tui` modules (#174).
- `std/tui.mvl` — Phase 2 API stubs covering `Terminal`, `Key`, `Direction`, `Style`, and `Color` types; core functions `open`, `close`, `clear`, `set_cursor`, `hide_cursor`, `show_cursor`, `size`, `print`, `print_styled`, `read_key`; style builders `plain`, `bold`, `italic`, `fg`, `bg`.
- `Terminal` zero-sized marker struct in `mvl_runtime/src/effects.rs` with doc comment explaining the Console/Terminal distinction.

### Fixed

- **Type checker** — `open()` no longer requires `! Terminal` (it is the capability entry point, not a consumer). Moved to `! Terminal` enforcement for functions that use the handle.
- **Error messages** — `InvalidEffectName` diagnostic now includes `Terminal` in the suggested valid-effects list.
- **Prelude** — `Terminal` added to `mvl_runtime::prelude` re-export so generated code compiles.
- **IFC analysis** — `tui.print_styled` added to `PUBLIC_SINKS` so secret-gated output is flagged as implicit-flow violation.
- **Silent failures** — `clear`, `set_cursor`, `hide_cursor`, `show_cursor`, `print` now return `Result<Unit, String>` instead of `Unit` so I/O failures are not discarded.

### Changed

- `Key::Char` — changed from `Char(String)` to `Char(Char)` — a single keypress is a single scalar, not a string.
- `open()` function signature — removed `! Terminal` effect annotation; now only functions that use the returned handle require `! Terminal`.

## [0.28.0] — 2026-04-15 (feat: multi-file module builds — resolver wired into codegen)

### Added

- **Multi-file project builds** — `mvl build` / `mvl run` now transpiles all modules reachable via `use` imports, not just the entry-point file. Each imported sibling `.mvl` file is compiled to a Rust module (`src/module.rs`) and declared with `pub mod` in the crate root (#177).
- `transpile_project` in the transpiler produces a `ProjectOutput` containing the entry-point source plus a `module_files` list — one Rust source per sibling module.
- `emit_program_with_mods` / `emit_sibling_module` in codegen: emits `pub mod name;` declarations and `use crate::module::item;` statements; sibling modules share `use mvl_runtime::prelude::*;` with the crate root to avoid duplicate type definitions.

### Fixed

- **`mvl check <file>`** — now loads imported sibling modules into the resolver so cross-module `use` imports are validated correctly when checking a single file (#177).
- `collect_undefined_types` no longer generates stub structs for types that are imported via `use` declarations, preventing duplicate-definition errors in multi-module projects.

### Changed

- `examples/access_control` — updated to proper multi-module patterns: exported items marked `pub`, local re-declarations that mirrored sibling module definitions removed.
- `examples/log_analyzer/utils.mvl` — removed unused generic stubs (`generic_min`, `generic_max`, `clamp<T>`, `in_range<T>`, `is_in_range_int`); added comment explaining `99999` magic constant; added zero-boundary test.
- Example Makefiles now prefer `../../target/debug/mvl` when present, falling back to the system `mvl`.
- Added `make clean` target to both example Makefiles.

## [0.27.0] — 2026-04-15 (feat: extended package model specification)

### Added

- **Extended package model specification** — Comprehensive design for the MVL package ecosystem (ADR-0012 and Spec 008)
  - Package manifest format using `mvl.toml` with `[dependencies]` and `[native]` tables
  - Visibility rules: `internal/` directory boundaries enforced by resolver, complementary to `pub`/private item-level visibility
  - Registry strategy: git-only for Phase 3, central registry `registry.mvl-lang.org` for Phase 4
  - Versioning: semver enforced, breaking changes detectable via type signature analysis
  - Build integration: `mvl build` handles dependency fetching and resolution (no separate install step)
  - Supply chain assurance: trust score per package visible in `mvl audit` (MVL verified lines / total lines), `extern-rationale` required for packages using `extern` blocks
  - SBOM generation in CycloneDX JSON format (Phase 4)
- **Trust boundary enforcement** — Formalises the pattern already established by ADR-0006 (FFI via extern "rust" + bridge.rs): extern code at the bottom, fully-verified MVL API at the top

## [0.26.0] — 2026-04-15 (feat: const generics and Array<T, N> fixed-size array type)

### Added

- **Const generic parameters** — New syntax for declaring const generic parameters: `type Buf<T, const N: Int> = ...` and `fn fill<T, const N: Int>(...) { }`. Const parameters are forwarded to Rust as `const N: usize` in the generated code (#68).
- **Fixed-size array type** — New built-in `Array<T, N>` type mapping to Rust's `[T; N]` syntax. Arrays with different sizes are treated as distinct types: `Array<Int, 16> ≠ Array<Int, 32>`. Size-aware type compatibility enforced in the checker.
- **Parser support** — Integer literals in type argument position (`Array<T, 16>`) and `const N: Type` syntax in generic parameter lists.
- **Checker support** — `Ty::Array(elem, size)` type variant with size-aware compatibility checking. Negative and invalid size arguments resolve to `Ty::Unknown` with proper error propagation.
- **Transpiler support** — Array<T, N> emits to Rust fixed-size array syntax `[T; N]`. Const generic parameters emit as `const N: usize` in struct, enum, and function signatures.
- **Phase 1 limitation handling** — Type-variable array sizes in generic functions (`Array<T, N>` where N is a const param) defer size verification to Rust (compile-time), with validation of literal sizes in the MVL checker.

### Changed

- `TypeDecl.params` and `FnDecl.type_params` now use `Vec<GenericParam>` enum instead of `Vec<String>`, allowing mixed type and const parameters.
- All type and function declaration emission (emit_struct, emit_enum, emit_alias, emit_generics) updated to handle const generic parameters.

### Fixed

- **Review findings** — Negative array size literals and wrong argument counts now properly return `Ty::Unknown` and propagate as type errors instead of silently resolving to zero-size arrays. Refined type aliases with const generics now include the generic parameters in the emitted struct definition.

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
