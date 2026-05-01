# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.61.0] — 2026-05-01

### Added

- **LLVM Backend Hardening (closes #384, #385, #386, #387, #388, #389)** — Security and robustness improvements to LLVM code generation.
  - **Error Propagation**: Replace silent `undef` emission with proper `None` propagation; unsupported constructs now surface as compilation failures rather than producing invalid IR.
  - **Module Refactoring**: Split 2,942-line `codegen/mod.rs` into four focused modules (`types.rs`, `exprs.rs`, `stmts.rs`, `builtins.rs`) for improved maintainability.
  - **Buffer Safety**: Replace global `format_buf` + unbounded `sprintf` with per-call stack allocation + `snprintf`; eliminates aliasing hazard and buffer-overflow risk in `format()` builtin.
  - **Grammar Updates**: Add `extern_decl`, `impl_decl`, and `borrow_expr` productions to `docs/grammar.ebnf` to match parser coverage.
  - **Cross-Backend Regression Tests**: Add `tests/cross_backend.rs` to verify identical stdout between Rust transpiler and LLVM backends on hello_world, calculator, and shapes corpus programs.
  - **Extern Linkage**: Fix `extern "c"` pre-declarations to use `Linkage::External` instead of internal linkage for correct FFI behavior.
  - **Test Infrastructure**: Update binary path resolution for robustness under `cargo nextest` and cross-compiled builds.

## [0.60.0] — 2026-05-01

### Added

- **LLVM Phase B: Advanced Type System (closes #367, #371, #381, #382)** — Complete LLVM IR generation for structs, enums, match expressions, control flow, and FFI bridges.
  - **Structs & Field Access**: LLVM named structs with extractvalue/insertvalue GEP operations
  - **Enums & ADTs**: Unit enum discriminants (i8), tagged unions {i8, [N×i8]} for `Result[T,E]` and `Option[T]`
  - **Pattern Matching**: LLVM switch statements with phi node merging for `match` expressions
  - **Control Flow**: `while` loops, `for` loops over ranges, `?` result propagation (early return)
  - **Extern "rust" Bridges**: Pre-declared signatures + real LLVM IR implementations; `roll_dice()` calls libc `rand() % 6 + 1`
  - **Method Calls**: `.len()` for String/List/Map/Set/Range, `.to_string()` for all types, math intrinsics for `Int`/`Float` (`abs`, `min`, `max`, `ceil`, `floor`, `sqrt`)
  - **Collection Literals**: List/Map/Set constructors with proper struct layout
  - **Built-in Conversions**: `format()` string interpolation
  - **Pattern Matching for Non-Deterministic Output**: `// expect-pattern:` annotation with glob-style matching (`?` = any char, `*` = any sequence)
- 15/15 LLVM corpus tests pass; 722 unit tests pass
- Improved Makefile: `make test` shows per-suite PASS/FAIL summary; individual `test-*` targets retain full output

## [0.59.0] — 2026-05-01

### Added

- **Phase C return-flow verification (closes #364)** — Extended the Phase C escape check to verify that when a function returning `&T` has a `&T` parameter, the tail expression actually flows from one of those parameters—not a local variable, literal, or non-reference value. Previous behavior only syntactically checked that the function *has* at least one `&T` param, which could allow code like `fn bad(x: &Int) -> &Int { 42 }` to pass the checker but fail in rustc.
  - `block_return_flows_from_ref_param()` / `stmt_return_flows_from_ref_param()` / `expr_return_flows_from_ref_param()` recursively trace return expressions through tail-position `if/else` and `match` branches.
  - `block_early_return_violation()` / `stmt_early_return_violation()` / `expr_early_return_violation()` scan all statements at any depth to catch early `return` statements that don't flow from a reference parameter.
  - `check_match_arms_flow()` helper deduplicates match-arm checking logic.
  - Handles `Expr::Borrow` correctly: `&x` where `x` is a reference parameter is accepted.
  - Rejects empty match arms (no valid return path).
  - Error spans now point to the problematic return expression, not the function declaration.

## [0.58.0] — 2026-05-01

### Added

- **Phase C scope-depth checking for reference bindings (closes #363)** — When a local binding is assigned a reference to a variable (implicit borrow `let r: &T = x` or explicit borrow `let r: &T = &x`), the checker verifies the referent lives at least as long as the binding. Emits `ReferenceOutlivesOwner` when the referent is defined at a deeper scope (shorter lifetime) or inside an initializer block that exits before the binding is made.
  - `referent_ident()` helper extracts root identifiers from complex expressions, supporting plain idents, block tails, and explicit borrows `&expr`.
  - Scope comparison uses `VarInfo.scope_depth` (0-based index) to detect lifetime mismatches.
  - Block-local variables (not in scope after init evaluation) are conservatively treated as always-dangling.
  - Covers both implicit (`let r: &T = x`) and explicit (`let r: &T = &x`) borrow forms.

### Fixed

- `check_stmt` Phase C logic extracted to `check_borrow_lifetime()` method — reduces nesting from 7 levels to ~3 and improves readability.
- Unified reference-assignment detection eliminates duplicated TypeMismatch emission.
- Added clarifying comment on scope_depth dual-convention (raw count vs. 0-based index).

## [0.57.0] — 2026-04-30

### Added

- **Expression-level borrow operator (closes #366)** — `&expr` and `&mut expr` are now valid MVL expressions. The parser creates `Expr::Borrow { mutable, expr }`, the checker types them as `Ty::Ref(mutable, T)` and rejects `&mut x` on immutable bindings and nested borrows `&&x`. The transpiler emits correct `&x` / `&mut x` Rust with proper precedence handling.
  - Integrated with Phase B borrow inference: function parameters with explicit `&T` are recognized by the transpiler's borrow_params_map.
  - Propagated through all 14 analysis passes (linter, checker, data-race, ifc, mcdc, refinements, termination, last_use, borrow_params, mcdc_instr, const_eval).
  - Fixes `group_by` transpiler bug: key functions with `&T` params now receive `&__v.clone()` instead of `__v.clone()`.

## [0.56.0] — 2026-04-30

### Added

- **Phase B borrow inference (closes #365)** — Conservative static analysis in the transpiler detects when function parameters are read-only (no mutation, assignment, return, or passing to other functions) and emits them as `&T` in Rust with `&x` at call sites, eliminating unnecessary `.clone()` calls. Includes fixes for direct for-loop iterables, binary operands, lambda captures, `Char` Copy type, and `Deref` unary operator handling.

## [0.55.0] — 2026-04-30

### Added

- **LLVM backend Phase A — Hello World (closes #352)** — Direct LLVM IR codegen via `inkwell` 0.9 / LLVM 22, enabled with `--features llvm`. Adds `--backend=llvm` flag to `mvl build`, `mvl run`, and `mvl test`. The `mvl test --backend=llvm` harness reads `// expect:` annotations from corpus files, compiles via LLVM, runs with `lli`, and asserts stdout.
  - **L5-01**: `inkwell` optional dependency, `llvm` Cargo feature gate — default Rust backend unchanged (closes #355).
  - **L5-02**: LLVM module setup: target triple from `TargetMachine`, data layout, `main()` returning `i32 0` (closes #353).
  - **L5-03**: `mvl test --backend=llvm` dual-backend integration test harness with `// expect:` and `// Expected stdout:` annotation support (closes #354).
  - **L5-04**: Primitive type codegen — `Int→i64`, `Float→f64`, `Bool→i1`, `Byte→i8`, `Char→i32`, `Unit→void`, `String→ptr` (closes #357).
  - **L5-07**: Function declarations, parameters, return values, basic calls — two-pass emit, parameter alloca pattern, if-expressions with phi nodes (closes #356).
  - **L5-10**: Arithmetic with checked overflow (`llvm.sadd/ssub/smul.with.overflow` + `llvm.trap`), comparison (`icmp SLT/SGT` etc.), logical, float ops (closes #359).
  - **L5-17**: `print`/`println` → libc `printf`; string literals as direct format strings, typed values dispatch to `%lld`/`%f`/`%s` (closes #358).
- `.cargo/config.toml` — sets `LLVM_SYS_221_PREFIX` for macOS Homebrew keg-only LLVM 22 (overridable via env).

## [0.54.0] — 2026-04-30

### Added

- **Rust backing for std/crypto stdlib (closes #349)** — Real implementations for `sha256`, `sha512`, and `crypto_random_bytes` in `mvl_runtime/src/stdlib/crypto.rs` using `sha2` and `hex` crates. CSPRNG uses `getrandom` for cross-platform support (Unix, Windows, WASI). Includes 11 comprehensive unit tests: NIST vectors for SHA-256/512 (empty and "abc"), determinism, output format, and randomness uniqueness.
- **Pure MVL higher-order list methods (closes #307)** — `filter`, `fold`, `take_while`, `skip_while`, and `any`/`all` are now implemented as genuine pure MVL bodies in `std/lists.mvl` using for/while loops and kernel primitives, replacing transpiler special-case emission. The `map` method retains trait dispatch for polymorphism across List/Option/Result. Short-circuit evaluation: `any` and `all` now stop early when the predicate match succeeds/fails rather than consuming the entire list.

### Changed

- **Removed std/tui stdlib (closes #349)** — TUI module deleted from stdlib; it belongs in userspace, not the language's core stdlib. The `Terminal` effect marker remains a valid language-level concept for programs that interact with raw terminal control. Aligned with stdlib scope decisions in #217.
- **Function-type parameters emit as `impl Fn` (PR #351)** — MVL function parameters typed as `fn(T) -> U` now emit as `impl Fn(T) -> U` in Rust, allowing both bare function pointers and closures to be accepted at call sites.
- `mvl_runtime/Cargo.toml` — added `getrandom = "0.2"` alongside `sha2 = "0.10"` and `hex = "0.4"`.

### Fixed

- **CSPRNG security hardening** — Replaced `/dev/urandom` direct open with `getrandom` crate: now panics on CSPRNG unavailability (unrecoverable failure) instead of silently returning zero-filled bytes. Cross-platform support on Unix, Windows, WASI, and beyond.
- **Stdlib test accuracy** — Added 8 runtime tests for `any`/`all` covering empty lists, all-match, none-match, and partial-match cases. Added transpiler tests verifying `any`/`all` UFCS dispatch and `impl Fn` parameter emission.
## [0.53.0] — 2026-04-29

### Added

- **Boundary value analysis for mutation testing (closes #331)** — New `mvl mutate --gen-boundary` flag prints a targeted report identifying surviving `IntLiteral` and comparison-operator mutants that can be killed with boundary value tests. For each survivor, shows the field name extracted from source, the exact kill value that distinguishes the original threshold from the mutant, and N-1/N/N+1 boundary sweep hints. Phase 1 (IntLiteral mutants) fully implemented; Phase 2 (comparison operator mutants) fully implemented.

### Fixed

- **Stdlib test accuracy and coverage (closes #342)** — Corrected test documentation for real implementations (`get_arg`, `get_env`, `get_args`) mischaracterized as Phase 2 stubs. Removed 11 redundant/duplicate tests from args, io, and log suites with no coverage loss. Fixed empty-base join comment to document Rust runtime vs MVL source divergence. Added STUB markers to all vacuous tests. Standardized log section headers.

## [0.52.0] — 2026-04-29
