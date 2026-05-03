# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.70.0] — 2026-05-03

### Added

- **`std.time` real implementation (Rust transpiler path)** — Replaces stubs in `std/time.mvl` with real Rust backing in `mvl_runtime::stdlib::time`. Provides `Instant`, `DateTime`, `Duration` types; `now()`, `sleep()`, `format_instant()`, `format_datetime()`, `parse()`, `seconds()`, `millis()`. UTC-only (Phase A); epoch-to-date via Hinnant civil-from-days algorithm, no external crates (#415).

- **`std.random` real implementation (Rust transpiler path)** — Replaces stubs in `std/random.mvl` with xorshift64 PRNG backed by `thread_local! { Cell<u64> }`, seeded from `SystemTime` with Fibonacci-mixed nanos. Provides `int(min,max)`, `float()`, `bytes(n)`, `choice[T]`, `shuffle[T]` (Fisher-Yates). No `rand` crate (#415).

- **`time` and `random` C-ABI exports for LLVM backend** — `mvl_runtime_c::stdlib::time` exports `_mvl_time_now_systemtime`, `_mvl_time_now_instant`, `_mvl_time_thread_sleep`, and `_mvl_time_iso8601_format`. `mvl_runtime_c::stdlib::random` exports `_mvl_random_int`, `_mvl_random_float`, `_mvl_random_bytes`, `_mvl_random_choice_index`, and `_mvl_random_shuffle_i64`. `Duration` is flattened to `(secs: i64, nanos: i64)` at the C boundary (#433).

- **LLVM codegen dispatch for `time.sleep`, `random.int`, `random.float`** — Extended `StdlibSig` enum with `VoidDurationArg`, `I64TwoI64Args`, and `F64NoArg` variants. `VoidDurationArg` uses LLVM `build_extract_value` to flatten the Duration struct into two i64 arguments before calling `_mvl_time_thread_sleep` (#433).

- **Cross-backend parity tests for `time` and `random`** — `cross_backend_random_int`, `cross_backend_random_float_shape`, and `cross_backend_time_sleep` verify that both backends agree on deterministic random and zero-duration sleep output (#433).

## [0.69.1] — 2026-05-03

### Fixed

- **Corpus files updated for mandatory explicit `let` type annotations** — Commits #408 made explicit type annotations required in all `let` bindings; 11 corpus files were not updated. Adds `: Type` annotations throughout, also adds `Console` to `env_basic.mvl` effect set and relaxes `bounded_sum` return type to `Int` (arithmetic on refinement types yields `Int`). Resolves `make test-corpus` going from 57 passed / 11 failed to 68 passed / 0 failed.

- **`make test-llvm` now shows individual test names** — Added `--verbose` flag so each test file path is printed as it runs.
## [0.69.0] — 2026-05-03

### Added

- **`mvl_runtime_c` cdylib — C-ABI stdlib for LLVM backend** — New crate wraps `mvl_runtime` Rust APIs with `#[no_mangle] extern "C"` symbols for LLVM-compiled programs. Implements the two-path stdlib architecture: Path 1 (Rust transpiler) uses native Rust APIs; Path 2 (LLVM backend) calls C-ABI exports via `lli --load`. Includes marshalling types (`MvlOption`, `MvlResult`), `string_to_c`/`c_to_string` helpers, and declarative `mvl_c_export!` macro (#431).

- **`env` and `process` stdlib bindings for LLVM backend** — All public functions from `mvl_runtime::stdlib::env` and `mvl_runtime::stdlib::process` exported as `_mvl_env_*` and `_mvl_process_*` C-ABI symbols. Includes getuid/getgid, environment variable access, working directory management, and process spawning with deterministic output capture. Process handles use opaque `Box` pointers to prevent use-after-free. LLVM codegen auto-discovers and loads the library via `find_mvl_runtime_c_lib()`, wired into `run_project_llvm` and `cmd_test_llvm` (#432).

- **Cross-backend stdlib parity tests** — `cross_backend_env_basic` verifies identical output from both transpiler and LLVM backends when calling `env.getuid()` and `env.getgid()`. Serves as smoke test that `libmvl_runtime_c` loads and symbols resolve correctly via `lli`.

- **ADR-0019: Two-Path Stdlib Architecture** — Documents the rationale for Rust crate + C-ABI cdylib split, ABI marshalling types, symbol naming convention, and build integration.

- **`make build-llvm-runtime` target** — Builds both `mvl_memory` and `mvl_runtime_c` cdylibs needed for LLVM backend at runtime.

### Fixed

- **Signal constructor / argument-passing ABI mismatch** — Removed `sigint`, `sigterm`, `sighup`, `sigusr1`, `sigusr2` (return `i8`, not `i64`) and `signal_reset`/`signal_ignore` (take `i8` argument) from auto-dispatch table. These require a follow-up with non-i64 / argument-passing dispatch (#450).

- **Use-after-free in `_mvl_process_kill` on error** — Clarified ownership contract: the child handle is unconditionally consumed whether `kill()` succeeds or fails. Callers must not use the original pointer after calling this function (#450).

- **Negative index handling in `_mvl_env_args_get`** — Added guard to prevent negative `i64` indices from wrapping to `usize::MAX` and causing O(n) CPU spin (#450).

### Testing

- **19 unit tests in `mvl_runtime_c`** (up from 15 pre-fix): added tests for null-handle guards (`wait_null`, `kill_null`, `output_free_null`) and negative array index handling.

## [0.68.2] — 2026-05-03

### Changed

- **refactor(arch): relocate AST transformations under `src/mvl/passes/`** — coverage, MC/DC, and mutation instrumentation modules moved out of `transpiler/` and `checker/` into a new backend-agnostic `passes/` layer. MC/DC analysis and instrumentation are now co-located under `passes/mcdc/`. Rust-specific emission helpers extracted to `transpiler/coverage_emit.rs` and `transpiler/mcdc_emit.rs`. No behaviour change; all existing tests pass (#443, #444, ADR-0018).

### Fixed

- **Coverage measurement via `make coverage`** — Pre-build `mvl_memory` cdylib into `cargo-llvm-cov`'s isolated target directory (`target/llvm-cov-target/`) before running the coverage tool. Resolves symbol resolution errors when LLVM backend tests run under coverage (#451).

## [0.68.1] — 2026-05-02

### Fixed

- **Stdlib test type annotations** — 94 bare `let` bindings across 8 stdlib test files now carry explicit type annotations, satisfying the parser requirement from #408. Fixes `make test-stdlib` parse errors (#447).

## [0.68.0] — 2026-05-02

### Added

- **Real `std.env` implementation** — `get`, `set`, `remove_var`, `all`, `args`, `current_dir`, `chdir`, `exit`, `getuid`/`getgid` (real POSIX syscalls via `extern "C"`), signal constructors and no-op registration; backed by `mvl_runtime::stdlib::env` (#414).
- **Real `std.process` implementation** — `spawn`, `wait`, `kill`, `stdin_write`, `stdout_read`, `stderr_read`, `is_success`, `exit_code`; full `Stdio` mode support (Pipe/Capture/Inherit/Devnull); backed by `mvl_runtime::stdlib::process` (#414).
- **Effect markers** — `Env`, `ProcessSpawn`, `Clock`, `Random` ZST types added to `mvl_runtime::effects`.
- **MVL integration tests** — `tests/stdlib/env_test.mvl` (17 tests) and `tests/stdlib/process_test.mvl` (15 tests) so `make test-stdlib` validates real runtime behaviour.

### Changed

- `mvl_runtime`: `forbid(unsafe_code)` relaxed to `deny(unsafe_code)` to allow targeted `extern "C"` wrappers for POSIX `getuid`/`getgid`.
- All `std/*.mvl` and `tests/stdlib/*.mvl` files: phase labels removed; current limitations described in plain language.

## [0.67.0] — 2026-05-02

### Added

- **Grammar-based fuzzing for compiler backends** — Three-phase fuzzing harness:
  - **Phase 1 (Rust transpiler)**: ~26k iter/sec in-process fuzzing via `make fuzz-rust`
  - **Phase 2 (LLVM codegen)**: ~15k iter/sec in-process fuzzing via `make fuzz-llvm`
  - **Phase 3 (Differential)**: ~20 iter/sec subprocess-based fuzzing comparing Rust vs LLVM output via `make fuzz-diff`
  - Bounded-depth grammar-guided generator using `arbitrary::Unstructured` for coverage-guided mutations
  - 70-file seeded corpus from `tests/corpus/`
  - Documentation in `tests/fuzz/README.md` for running, triaging, and minimizing crashes (#422)

## [0.66.1] — 2026-05-02

### Fixed

- **Explicit `let` type annotations required** — The checker now rejects `let` bindings without an explicit type annotation, emitting `error[req1]: let binding requires an explicit type annotation`. MVL Design Principle #1 ("Explicit over implicit") forbids implicit types: they create audit gaps, break non-rustc back-ends, and were already causing ambiguous method dispatch in the Rust transpiler. All corpus files updated to carry explicit annotations. (#408)

### Removed

- **`unnecessary-annotation` linter rule** — The rule (and its `obvious_literal_type` carve-out for `Int`/`Float`) is now contradictory: since all `let` bindings must be annotated, no annotation can be "unnecessary". The rule and `unnecessary_annotations` config field have been deleted. (#408, #404)

## [0.66.0] — 2026-05-02

### Added

- **`mvl check --error-limit=N` flag** — Stop reporting errors after N errors (default 10) and print `... and N more errors (use --error-limit=0 to show all)`. Prevents terminal flooding when a systemic issue produces dozens of cascading errors from the same root cause. Use `--error-limit=0` to restore the previous unlimited behaviour (#333).

## [0.65.1] — 2026-05-02

### Fixed

- **Makefile: `make test-llvm` in fresh worktrees** — Added `build-memory` target and made `test-llvm` depend on it, so the `mvl_memory` cdylib is always built before running LLVM backend tests. Previously, all LLVM tests silently produced empty output in fresh worktrees (#410).

## [0.65.0] — 2026-05-01

### Fixed

- **Phase D Borrow State Machine Robustness** — Corrected implementation of `BorrowState` transitions to prevent false positives and order-dependency bugs.
  - **Order-Independent Alias Check**: Two-pass parameter check ensures `&T` + `&mut T` pairs are rejected regardless of parameter order (fixes #362).
  - **Prevented State Leaks**: Moved `borrow_state` updates from expression-level type inference to `Stmt::Let` binding so that borrow state is only set when `borrows_var` is simultaneously recorded; prevents permanent state retention when borrows appear outside `let` bindings.

## [0.64.0] — 2026-05-01

### Added

- **L5-15: Ownership-based drop — move transfers pointer, last owner frees (closes #394)** — Precise drop insertion for heap-allocated collections.
  - **Ownership Transfer on Move**: `let y = x` moves heap ownership from source to destination; only destination is tracked for drop at function exit.
  - **Function Parameter Ownership**: Value parameters of heap types are owned by the callee; registered in `heap_locals` for drop at function exit. Borrow parameters (`&T`) excluded — caller retains ownership.
  - **Call Site Ownership**: Heap-typed arguments passed by value to user-defined functions are marked as moved; caller no longer drops what the callee owns.
  - **Return-Value Exclusion**: Return expressions exclude their heap values from drops via `emit_heap_drops_except(ret_heap_name)`.

## [0.63.0] — 2026-05-01

### Added

- **LLVM Phase C: Heap Allocation & Reference Counting for Collections (closes #391)** — Efficient memory management for String, Array, and Map types with runtime-assisted deallocation.
  - **Rust cdylib Runtime (`mvl_memory`)**: Implement `MvlString`, `MvlArray`, and `MvlMap` opaque heap types with reference counting and safe allocation/deallocation strategies.
  - **LLVM Backend Emission**: Generate calls to `mvl_string_new`, `mvl_array_new`, `mvl_map_new` for collection literals; automatic RC increment/decrement on clone/drop; proper stack cleanup at function exit with `emit_heap_drops_except`.
  - **Memory Safety Hardening**: Add `checked_mul_size` and `checked_add_size` helpers in runtime; bounds-check all RC counter operations; prevent integer overflow in allocation size arithmetic.
  - **Heap Local Tracking**: Track heap-allocated collections per scope; drop non-returned values at function exit; preserve returned heap value by passing its name to `emit_heap_drops_except`.
  - **Expression-level Methods**: Implement `String.len()`, `Array.len()`, `Array.first()`, `Set.contains()` using runtime `mvl_array_len` and `mvl_array_get` for heap-based layouts.
  - **Printf Integration**: Wrap `snprintf` results in `mvl_string_new` so `format()`, `int_to_string()`, `float_to_string()`, and `bool_to_str_ptr()` return proper `MvlString*` instead of dangling stack pointers.
  - **Architectural Decision Record**: ADR-0016 documents the memory runtime design, FFI boundary strategy, and reference-counting approach.

## [0.62.0] — 2026-05-01

### Added

- **LLVM Phase E: Generic Functions & Option[T] with Struct Payloads (closes #380)** — JIT monomorphization and pointer-based `Option[T]`/`Result[T,E]`.
  - **Generic Function Monomorphization**: User-defined generic functions (e.g. `fn identity[T](x: T) -> T`) monomorphize at LLVM level; each concrete type instantiation produces a separate LLVM function body (`identity_Int`, `identity_Ptr`, etc.) on first call.
  - **Pointer-Based Option/Result**: Changed layout from `{i8, [8×i8]}` (fixed 8-byte payload) to `{i8, ptr}` so `Option[Point]` and other struct payloads of any size are supported.
  - **Type Checker Support**: Generic function calls now pass type checking; `infer_fn_call` skips argument type checking for generic functions and returns `Ty::Unknown` (monomorphization correctness enforced by LLVM backend).
  - **Local Type Tracking**: Added `local_mvl_types` to track MVL type annotations on function parameters and let-bindings, enabling correct LLVM type inference for `Option[T]` payload extraction in match arms.
  - **Test Coverage**: Added `tests/corpus/11_programs/generic_fns.mvl` covering `identity[T]` instantiation and `Option[Point]` Some/None match.

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
