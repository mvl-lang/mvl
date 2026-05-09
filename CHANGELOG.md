# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.83.0] — 2026-05-08

### Added

- **Property-based testing stdlib module** — `std/pbt.mvl` implements Phase A (generators, combinators, property_check) and Phase B (mutation operators, targeted property checking) of #40 and #425. Five concrete generator types (IntGen, FloatGen, BoolGen, StringGen, ListIntGen) encode generation strategies as data. All function types are pure MVL atop `std.random.*` (Tier 3, no new C-ABI). Closes #555.

### Changed

- **Function pointer parameters emit as bare `fn` instead of `impl Fn`** — Matches enum variant field emission and ensures `Copy+Clone` compatibility for function-typed values stored in enum variants. Fixes type mismatch when user-defined functions with `List[T]` parameters are passed as callbacks to higher-order functions.
- **Prelude programs scanned for Rust-backed stdlib imports** — `emitter.rs` now includes stdlib imports from both user and prelude programs, enabling `std/pbt.mvl`'s `use std.random.*` to auto-generate `use mvl_runtime::stdlib::random::*` in transpiled output.


## [Unreleased]

## [0.88.0] — 2026-05-09

### Added

- **Property-based testing stdlib complete (Phase A/B + fuzz)** — `std/pbt.mvl` now implements the full PBT stack: Phase A generators (`gen_int`, `gen_float`, `gen_bool`, `gen_string`, `gen_list_int`, `gen_filter_int`, `gen_one_of_int`, `gen_weighted_int`, `gen_boundary_int`) with binary-search shrinking on failure; Phase B mutation operators (`mutate_int`, `mutate_float`, `mutate_string`, `mutate_list_int`), targeted property checking (`property_check_targeted_int`), and mutation-based checking (`property_check_with_mutation_int`); fuzz testing with raw-input generators (`gen_raw_bytes`, `gen_raw_string`) and `fuzz_check_bytes`/`fuzz_check_string`. Verbose and persistence variants added for all typed property checks. All public `property_check_*` and `fuzz_check_*` functions marked `partial`. Closes #40, #425, #617.

## [0.87.0] — 2026-05-09

### Added

- **Label-transparent functions (ADR-0024)** — Functions marked `transparent` signal to the checker that they propagate security labels from arguments to return type, closing the silent label-drop hole at stdlib boundaries. `json.decode(tainted_str)` now returns `Tainted[Result[Value, String]]` instead of silently stripping the label. Generalizes the existing `format()` special case to any stdlib transform function. Closes #179.

### Changed

- **`json.encode()` marked label-transparent** — Ensures round-trip encode(decode(tainted)) preserves taint through both operations.

### Added

- **Stdlib proven profile** — `--stdlib=proven` now runs full 11-requirement verification on all pure-MVL stdlib files (`core`, `strings`, `lists`, `math`, `collections`, `json`, `pbt`) before checking user code. Verification failures exit non-zero. OS/hardware-backed modules remain trusted builtins. Closes #538, #539. Part of epic #533.
- **Stdlib profiles documentation** — `docs/stdlib-profiles.md` user guide and ADR-0023 document the trusted/proven split, irreducible-builtins principle, and certification path. Closes #541, #542.

## [0.86.0] — 2026-05-09

### Changed

- **Linter style rules OFF by default** — `line_length`, `trailing_ws`, `indentation`, `final_newline`, and `consistent_comment_style` are now disabled in `LintConfig::default()` to prioritize semantic correctness over style preferences. MVL is designed for LLM-generated code where correctness matters more than formatting. Semantic rules (`unreachable_code`, `redundant_match`, `redundant_effects`) remain ON. Closes #599.

### Added

- **Style master toggle** — New `style = true` key in `.mvllintrc` enables all style rules at once with standard values. Individual keys always override the toggle regardless of file order.
- **Config fields** — `indentation: bool` and `final_newline: bool` fields added to `LintConfig` (previously these rules always fired, ignoring config).

## [0.85.0] — 2026-05-09

### Added

- **Type-aware direct Rust method dispatch** — Transpiler now queries `expr_types` (from type checker) to emit type-specific Rust for `.map()`, `.pow()`, `.contains()`, `.get()`, `.len()` instead of trait-based dispatch. Eliminates `Mvl*` trait definitions and `emit_method_traits()` entirely. Closes #554.
- **`eprint` / `eprintln` / `assert` / `panic` as first-class builtins** — Registered in checker, handled in transpiler via Rust macros, and supported in the LLVM backend via `dprintf(2, ...)`. Symmetric with `println`/`print`. IFC guard prevents Secret-labeled values reaching stderr. Closes #556.
- **Cross-backend stderr parity test** — `cross_backend_eprint_stderr` validates that both Rust and LLVM backends produce identical stderr output for `eprint`/`eprintln` programs.

## [0.84.0] — 2026-05-09

### Added

- **Layer 2 interval arithmetic for refinement solver** — Adds interval-based reasoning to the layered refinement checker. Converts variable hypotheses to bounded integer intervals and checks predicate containment, proving calls where Layer 1 (trivial patterns) cannot. Handles compound bounds via `&&` intersection. Closes #590.
- **If-condition narrowing in refinement context** — Injects condition constraints into then-block scope for local narrowing without propagation to else-branch (conservative, sound). Enables Layer 2 to prove calls inside `if x > N { require_something(x) }` blocks.

## [0.82.0] — 2026-05-08

### Added

- **Dynamic stdlib dispatch from `pub builtin fn` declarations** — Replaces 27-entry hardcoded dispatch table with runtime derivation from embedded stdlib declarations. Adding a new `pub builtin fn` now works automatically in both Rust and LLVM backends. Closes #557.
- **`std/core.mvl` stubs as `pub builtin fn`** — Converts `println`, `print`, `eprintln`, `eprint`, `format`, `assert`, `assert_eq`, `panic` to `pub builtin fn` declarations. LLVM backend handles via inline emission. Closes #556.

### Changed

- **Deleted `std/primitives.mvl`** — Consolidated 25 `extern "rust"` kernel functions into their domain-specific modules: 17 string operations in `std/strings.mvl`, 6 list operations in `std/lists.mvl`. Re-exports preserved. Closes #553.
- **Removed `Mvl*` dispatch traits from `mvl_runtime`** — Transpiler now emits direct Rust method calls instead of trait dispatch (e.g., `s.len()` instead of `MvlString::mvl_len(&s)`). Reduces indirection and improves type clarity. Closes #554.
- **Makefile `test-llvm` target** — Reformatted output to show per-file ✓/✗ checkmarks matching `test-corpus` display style.

### Fixed

- **Stdlib `Map.get()` dispatch in generic functions** — Fixed transpiler `transpile_with_prelude` and `transpile_source_with_prelude` to merge prelude expression types (`collect_prelude_expr_types`) into `cg.expr_types` before emission. Previously only test-program types were available, causing `Map.get(key)` to fall through to the List-index pattern. All 403 stdlib tests now pass.
- **Tree-sitter highlights query** — Replaced invalid `(bitxor_op)` named node reference with literal `"^"` (bitxor is an inline anonymous token in the grammar).

## [0.80.2] — 2026-05-07

### Fixed

- **Tree-sitter grammar syntax error** — `module_path` updated from `::` separators to `.` separators with optional brace import group to match real MVL syntax (`use std.io.{File, Path}`). Fixes tree-sitter parser unable to parse any real MVL imports. Closes #479.
- **Highlights.scm "Invalid node type" error** — Removed unnecessary `alias("^", $.bitxor_op)` from grammar.js; `^` is now a plain anonymous token like `&`, `|`, `~`, `<<`, `>>`. Fixes tree-sitter v0.24+ compatibility.

## [0.81.0] — 2026-05-07

### Added

- **MC/DC match statement coverage** — `DecisionKind::Match` and `DecisionKind::MatchGuard` variants added to MC/DC analysis; each arm of a match with ≥2 arms is tracked as a separate observation. Transpiler emits `__mvl_mcdc::record(mid, arm_idx)` in each match arm body. Compound `else if` conditions now correctly instrumented. Line-number offset applied to match decisions in test files. Closes #548.

### Fixed

- **Stdlib prelude not excluded from MC/DC reports** — `emitter.rs` now saves/restores `self.mcdc` during stdlib prelude emission, preventing stdlib functions from appearing in coverage reports.
- **Compound `else if` conditions not instrumented** — `emit_else_branch` now calls `emit_mcdc_if` for compound conditions (clause count ≥2), wrapped in `{ }` block to satisfy Rust syntax.
- **Match arm line numbers offset in test files** — `main.rs` applies line-number offset calculation to `Match`/`If`/`While` decisions (previously only applied to `Return`).

## [0.80.1] — 2026-05-07

### Fixed

- **Neovim 0.12 tree-sitter crash** — tree-sitter ≥ 0.24 repurposed `^` as a query anchor, making `"^"` an invalid literal in highlights.scm. Alias the BitXor token to the named node `bitxor_op` in grammar.js and query via `(bitxor_op) @operator`. Parser regenerated. Fixes Neovim crash on `.mvl` files.
## [0.80.0] — 2026-05-06

### Added

- **`builtin` keyword for stdlib functions** — establishes explicit trust boundary: `pub builtin fn` declarations delegate directly to runtime (mvl_runtime/mvl_runtime_c) without MVL implementation. Parser, type checker, transpiler, and LLVM backend updated. Closes #534.
- **Stdlib builtin annotations** — mark 55 Rust-backed stdlib functions as `pub builtin fn` across args, crypto, env, io, log, process, random, regex, time modules. Closes #535.
- **LLVM backend stdlib parity** — add 15+ string/list/io C-ABI operations (len, trim, starts_with, ends_with, contains, find, replace, split, substring, char_at, from_chars, byte_at, from_bytes, slice, concat, exists, is_file, is_dir, read_file, create_symlink, read_link, chmod). Closes #536.
- **`--stdlib=trusted` CLI flag** — accept and validate profile selection; default is trusted (current behavior). Lays groundwork for proven profile in #538. Closes #537.

### Fixed

- **LLVM type mismatches** — add `trunc_int_to_ret()` helper to handle i64→i1/i8 return type narrowing for Bool/Byte functions.

## [0.79.2] — 2026-05-06

### Added

- **`config_server` example** — Multi-file example demonstrating network effects (`! Net`, `! FileRead`, `! Console`, `! Log`), IFC labels (`Tainted[String]`, `Secret[String]`), and refinement types (`Port = Int where self > 0 && self <= 65535`) working together. Features a pure dispatch layer (`handler.mvl`) separated from effectful edges (`main.mvl`), constant-time auth verification at the trust boundary, and property test suite for `Secret[String]` compile-time invariant. `mvl test handler_test.mvl --backend=llvm` demonstrates LLVM cross-backend support for pure types. Closes #170.
## [0.79.1] — 2026-05-06

### Fixed

- **Stdlib type stubs suppression** — LLVM backend now correctly suppresses type stubs for types imported from Rust-backed stdlib modules, preventing spurious duplicate symbol errors. Closes #530.

## [0.78.1] — 2026-05-05

### Added

- **`missing-annotation` linter rule**
- **LLVM primitives for JSON encode** — C-ABI functions `mvl_string_chars`, `mvl_map_keys`, `mvl_map_remove` in `mvl_runtime_c`. LLVM backend can now call `std/json.mvl` encode path. `compile_to_ir` delegates to `compile_to_ir_with_prelude`. `RUST_BACKED_STDLIB` made public and `regex` added to the list. Closes #437.
- **stdlib json_test** — 35+ tests for JSON encode/decode primitives, arrays, objects, round-trips, and error cases.
- **stdlib collections_test** — 4 new Map operation tests (`map_put`, `map_without`, `map_get`, `map_len`).
- **corpus json_decode** — cross-backend corpus test for JSON decoding.

### Fixed

- **`assert_eq`/`assert_ne` E0283** — string literal args no longer get `.into()` in macro context; eliminates type-ambiguity errors across 29 stdlib tests.
- **Labeled type coercion E0308** — `let x: Labeled[String] = "..."` now emits `.into()` at binding site where the annotation makes the target type unambiguous.
- **Map/Set param mutability** — transpiler now scans function bodies for `.insert()`/`.remove()`/`.retain()` calls and adds `mut` only to parameters that actually need it; eliminates 216 spurious "variable does not need to be mutable" warnings.
- **Secret label declassify in corpus** — `crypto_random_bytes_shape.mvl` and `crypto_random_bytes_zero.mvl` now correctly declassify `Secret` values before passing to `println`.
- **`test-llvm` Makefile target** — now depends on `build-llvm-runtime` (was `build-memory`); ensures `mvl_runtime_c` C-ABI symbols (`_mvl_io_*`, `_mvl_log_*`) are available when running LLVM cross-backend tests. Re-enables `cross_backend_io_write_read_roundtrip` and `cross_backend_log_stderr` tests.

## [0.79.0] — 2026-05-05

### Added

- **`mvl test --backend=llvm` harness for `*_test.mvl` files** — detects `test fn` declarations, synthesises a `fn main()` caller, and runs each file as an LLVM test case. Closes #500.
- **String literal `match` in LLVM backend** — `emit_string_match` emits an if-else chain using `mvl_string_eq` when any match arm is a `Pattern::Literal(Str)`.
- **`String.to_lower` / `String.to_upper`** — new C-ABI functions `_mvl_str_to_lower` / `_mvl_str_to_upper` in `mvl_runtime_c`; wired into LLVM method dispatch.
- **`Int.clamp(lo, hi)`** — inline `build_select` chain in LLVM codegen.
- **Qualified constructors** — `Result::Ok`, `Result::Err`, `Option::Some` now resolve before the general enum dispatch path in LLVM.
- **`Secret<T: MvlLen>::mvl_len()`** — propagates the IFC label so `Secret[List[T]].len()` yields `Secret<i64>`; callers must `declassify` before logging (req11).

### Fixed

- **`crypto_random_bytes` corpus tests** — used `bs.len()` (Secret) directly in `println`, violating IFC req11. Fixed with `declassify(bs.len())`.

## [0.78.0] — 2026-05-05

### Added

- **ADR template and CI enforcement** (#429) — New `## Relation to language definition` section required in all ADRs numbered >= 0017 forces every architectural decision to explicitly confront the eleven requirements and design principles. Prevents silent drift (see #408). Includes `tools/check_adr.py` CLI check and CI job.
- **`.openspec/adr/README.md`** — Comprehensive ADR conventions guide covering file naming, template usage, exemption policy, and CI enforcement.

### Fixed

- **Orphaned ADR-0018 draft removed** — `.openspec/adr/0018-llvm-runtime-c-abi.md` was superseded by ADR-0019 but never cleaned up, causing spurious duplicate-number CI failures. Removed.

## [0.77.0] — 2026-05-05

### Added

- **`crypto_random_bytes` LLVM dispatch** — wires `crypto_random_bytes(n)` as a tier-1 LLVM builtin via new `StdlibSig::I64ReturnsPtrArg` variant and `emit_stdlib_call_i64_returns_ptr` emitter. Previously the function fell through to a no-op on the LLVM path. Closes #507.
- **`_mvl_crypto_random_bytes` returns `*mut MvlArray`** — replaces the custom length-prefixed heap layout with the standard `MvlArray` type, making the result compatible with all list stdlib operations (`list_len`, `list_get`, etc.).
- **Codegen-level IFC defense** — `is_secret_labeled` helper and `assert!` guards on `println`, `print`, and `log_*` sinks catch Secret-labeled values routed to public sinks without declassify. Guard is active in both debug and release builds. Closes #508.
- **Secret IFC label stripping in `.len()` dispatch** — `Secret[List[T]].len()` now correctly routes to `mvl_array_len` instead of `mvl_string_len` on the LLVM path.
- **Cross-backend shape tests** — `crypto_random_bytes_shape.mvl` and `crypto_random_bytes_zero.mvl` verify correct list length on both transpiler and LLVM backends (#507).
- **Complete bzip2 compression example** — `examples/bzip/` demonstrates native bit operators, borrowed references for large-buffer efficiency, recursive ADTs (HuffmanTree), and a pure algorithmic core with sharp effect boundary. Implements RLE, BWT, MTF, Huffman entropy coding, and bitstream layers. Includes 8 roundtrip property tests validating compress→decompress fidelity. Closes #498.

### Security

- **`_mvl_crypto_random_bytes` size cap** — input `n` is now capped at 131,072 bytes (1 MiB); returns null for larger values, preventing unbounded allocation on adversarial input.
- **`getrandom` failure is now an abort** — replaced `.expect()` (which unwinds across the `extern "C"` boundary, UB) with `.unwrap_or_else(|_| std::process::abort())` for clean termination when the OS CSPRNG is unavailable.
## [0.76.0] — 2026-05-05

### Added

- **Real `std.regex` stdlib implementation** — Rust and LLVM backends. All 5 stdlib functions (compile, find, find_all, replace, captures) backed by the regex crate. C-ABI exports in `libmvl_runtime_c` for compile/replace. LLVM codegen for compile/replace verified via cross-backend tests. find_all/captures C-ABI symbols deferred (requires List[Struct]/nested Option marshalling). Closes #420, #439.
- **`mvl_runtime_c` C-ABI cdylib** — bootstraps the two-path stdlib architecture (ADR-0018/ADR-0019): the LLVM backend now loads `libmvl_runtime_c` via `lli --load` to access `std.env`, `std.process`, and `std.regex` symbols at runtime. Closes #431, #432.
- **Cross-backend corpus test** — `tests/corpus/01_basics/env_identity_llvm.mvl` verifies `getuid()`/`getgid()` produce identical output on both backends. Extended with regex/crypto cross-backend verification.

## [0.76.0] — 2026-05-05

### Changed

- **Reference syntax: `&T`/`&mut T` → `val T`/`ref T`** — Replaced Rust-style borrow syntax with capability-based terminology. `val T` denotes deeply immutable (shareable) references; `ref T` denotes exclusive (mutable) references. Phase 6 of capability system (Phase 8 adds `iso`/`tag` for actor safety). Closes #503.
  - `&T` in type position now produces parse error: "use `val T` instead"
  - `&mut T` in type position now produces parse error: "use `ref T` instead"
  - Expression-level: `&expr` → `val expr`, `&mut expr` → `ref expr`
  - Transpiler output to Rust (`&T`/`&mut T`) remains unchanged
  - All parser, checker, and transpiler logic preserved — only surface syntax changed
  - Fixed fuzzer to generate `Option[T]` and `Result[T, E]` with square brackets (MVL syntax, not Rust)

## [0.75.0] — 2026-05-05

### Added

- **Unsigned integer types** — `UByte` (u8) and `UInt` (u64) as first-class `Ty` variants in
  the checker and transpiler. Both types support all standard arithmetic and comparison
  operations. Closes #481.

- **First-class Map and Set types** — `Ty::Map<K,V>` and `Ty::Set<T>` replace string-based
  `Named("Map", ...)` and `Named("Set", ...)`. Full structural type checking with key/value
  constraints. Map keys must be `Hashable`, Set elements must be `Hashable`. Closes #482.

- **Bitwise operators** — `&` (and), `|` (or), `^` (xor), `~` (not), `<<` (shl), `>>` (shr)
  for integer types (Int, Byte, UByte, UInt). Pratt precedence 60 (same as arithmetic).
  Full IFC label propagation: mixing Secret and Public operands produces Secret result.
  Closes #483, #484.

- **Overflow-checking arithmetic methods** — `checked_add`, `checked_sub`, `checked_mul`,
  `checked_div` and `wrapping_add`, `wrapping_sub`, `wrapping_mul` methods on Int, Byte,
  UByte, UInt. Checked methods return `Option<T>` (None on overflow); wrapping methods
  return the wrapping result directly. Closes #485.

- **Slimmed prelude** — `mvl_runtime::prelude` now exports only language fundamentals:
  `ParseFromArgs`, `get_arg`, `parse` (struct-parsing infra), and type trait bounds. All
  module re-exports (env, io, fs, process, etc.) removed in favor of targeted imports
  via `use std.X.*` declarations. Closes #488.

- **Targeted stdlib imports** — Compiler now emits `use mvl_runtime::stdlib::X::*` for each
  `use std.X.*` declaration in MVL source. Previously, all stdlib modules were imported
  unconditionally via the prelude. Closes #489.

- **Memory architecture refactoring** — Heap-collection operations (`mvl_string_*`,
  `mvl_array_*`, `mvl_map_*`) moved from `mvl_memory` to `mvl_runtime_c::memory_ops`.
  `mvl_memory` now contains only lifecycle (alloc/drop) and core types. Clarifies division:
  `mvl_memory` = types + lifecycle (Miri-safe), `mvl_runtime_c` = C-ABI operations. Closes #490.

### Fixed

- **Security issues in Map operations** — Added zero-length key guard in `mvl_map_insert`;
  prevented dangling pointer storage for zero-length values by using `ptr::null_mut()`.
  Added invariant assertion in `mvl_map_get`.

- **Type inference for UInt wrapping methods** — `wrapping_add`, `wrapping_sub`, `wrapping_mul`
  on `UInt` now correctly resolve to `Ty::UInt` instead of `Ty::Unknown`.

- **Bitwise operators on invalid types** — Bitwise operations on Float (or other non-integer
  types) now correctly produce `TypeMismatch` errors. Fixed label-checking to use
  `.unlabeled()` for type dispatch.

## [0.74.0] — 2026-05-05

### Added

- **Native Map/Set implementations** — `std/collections.mvl` stubs replaced with real MVL
  method bodies that work on both the Rust transpiler and LLVM backends. The transpiler
  dispatches via `MvlGet<K,V>` and `MvlLen` traits; the LLVM backend dispatches via explicit
  codegen arms in `exprs.rs`. Closes #418.
  - Map: `get`, `insert`, `remove`, `contains_key`, `keys`, `values`, `len`, `is_empty`
  - Set: `contains`, `insert`, `remove`, `to_list`, `len`, `is_empty`, `intersection`,
    `union`, `difference` (LLVM-side for `remove`, `keys`, `values`, set-algebra deferred to #436)
  - `MvlGet<K,V>` and `MvlLen` traits added to `mvl_runtime::prelude` and transpiler preamble
  - Auto-injects `Hash + Eq + Clone` bounds for Map/Set type parameters in generic functions — Opt-in Warning-severity rule that fires when a
  function body contains calls but no effect annotation is declared. The inverse of
  `unnecessary-annotation` (removed in v0.66.1), implementing MVL's "Explicit over implicit"
  principle (#428). Disabled by default (`missing_annotations = false`); enable in
  `.mvllintrc`. `test fn` declarations are excluded. See Spec 011 Req 4 and ADR-0017
  amendment.

## [0.73.0] — 2026-05-05

### Added

- **BDD naming convention** — Test functions with `given_*`, `when_*`, `then_*` prefixes and
  `test fn scenario_*` entry points follow the BDD pattern (ADR-0020). No language changes;
  purely a library-style testing approach with explicit state threading via context structs.
  Spec 004 Req 5, Issue #39 (#477).

- **`mvl test --bdd` Gherkin reporter** — Emits a `BDD scenarios:` block after test runs,
  listing each `scenario_*` function as `Scenario: <name> ... ok`. Extracts scenario names
  from function declarations; no parser changes. Implemented in `src/main.rs::cmd_test`.

### Fixed

- **BDD corpus syntax errors** — Added missing semicolons and type annotations to `let`
  bindings in calculator_bdd_test.mvl; all 5 scenarios now parse and pass.

### Changed

- **`make assurance` interface** — Changed from verbose-by-default to summary-by-default;
  use `make assurance VERBOSE=true` for full output with legend. Dropped `make assurance-summary`.

### Docs

- **BDD documentation** — ADR-0020 formalizes the decision (Option B+A hybrid); Spec 004 Req 5
  defines the pattern; tests link to concrete scenarios. Two Gherkin test scenarios verify both
  the naming convention and the `--bdd` reporter output.

## [0.72.2] — 2026-05-04

### Added

- **`std.io` real implementation (Rust transpiler path)** — Replaces stubs in `std/io.mvl` with real `std::fs` backing in `mvl_runtime::stdlib::io`. Provides `path(s: String) → Path` (identity), `write(p: Path, content: Tainted[String]) → Result[Unit, String]`, `append(p: Path, content: Tainted[String]) → Result[Unit, String]`, `read_to_string(p: Path) → Result[Tainted[String], String]`, `create_dir_all(p: Path) → Result[Unit, String]`, `remove(p: Path) → Result[Unit, String]`. Path type is a transparent wrapper around String; errors are mapped to IFC-safe categories ("file not found", "permission denied", "I/O error") (#417).

- **IO C-ABI exports for LLVM backend** — `mvl_runtime_c::stdlib::io` exports `_mvl_io_path`, `_mvl_io_write`, `_mvl_io_append`, `_mvl_io_read_to_string`, `_mvl_io_create_dir_all`, `_mvl_io_remove` with matching signatures. Returns wrapped `LlvmResult {tag, payload}` using stack allocation pattern for payload indirection. LLVM codegen gains four new `StdlibSig` variants (`PtrIdentArg`, `ResultUnitOnePtrArg`, `ResultUnitTwoPtrArgs`, `ResultStringOnePtrArg`) and `wrap_c_result_with_slot` helper for C → LLVM result layout conversion. Cross-backend tests verify identical I/O behavior on both transpiler and LLVM backends (#435).

- **Fix for `Result[Unit, String]` in LLVM backend** — Changed `infer_result_ok_llvm_ty` to return `Option<BasicTypeEnum>` (None = Unit, Some = other types) to avoid segfault from loading null payload pointers. `emit_propagate` and `emit_match` now skip load when ok_ty is None (#435).

### Changed

- **Corpus test `io_basic.mvl` restructured for IFC compliance** — Added `Console` effect to `run_io()` and avoided printing `Tainted[String]` file contents directly (violates Req 11: `println` only accepts `Public[T]`). Test now prints fixed confirmation strings instead of tainted data, verifying I/O operations succeed via error propagation (#417).

## [0.72.1] — 2026-05-04

### Fixed

- **`mvl mcdc --json` source field now shows correct stdlib lines** — Decisions in stdlib functions (`take_while`, `skip_while`, `find_index` while loops from `lists.mvl`) were attributed to the test module's file stem, causing the `"source"` field to show unrelated lines from the test file. Fix: post-process decisions to reassign `file` to the correct prelude stem and load prelude source texts into the lookup map (#472).
- **Example files updated to require explicit type annotations** — All 190+ bare `let x = expr` bindings across `examples/access_control/`, `examples/flight_clearance/`, and `examples/medical_triage/` now include `: Type` annotations as required since #408 (#470, #471).

## [0.72.0] — 2026-05-04

### Added

- **MC/DC coverage analysis now outputs machine-readable JSON** — `mvl mcdc <file|dir> --json` produces structured JSON with test counts, decision/obligation metrics, and per-clause coverage detail. `--json --quiet` emits summary only. Enables CI integration, coverage dashboards, and qualification evidence packages (DO-178C, IEC 62304). `independence_pair` is `null` pending test trace integration (#319); `coupled_with` is populated from coupled condition analysis (#325) (#326).
- **`make mutants` — cargo-mutants infrastructure for transpiler codegen** — `cargo-mutants` is now wired to the three transpiler emit modules (`emit_exprs.rs`, `emit_stmts.rs`, `emit_types.rs`) via `make mutants` (long-running, not per-PR CI). Target mutation score: ≥80%. 26 regression tests added to `tests/transpiler.rs` covering the most mutation-prone paths: the full binary-operator table (13 operators), bool/float literal dispatch, let-mutability dispatch, string-match `.as_str()` coercion, `else if` inline emission, and field-access/ident clone-on-pass. These tests kill mutants that previously survived undetected (#206).

## [0.71.1] — 2026-05-03

### Fixed
- **Design Principles are now executable OpenSpec Requirements (Spec 001 Reqs 12–14)** — All 10 README Design Principles and all 11 ADR-0001 requirements are now pinned to spec requirements with GIVEN/WHEN/THEN scenarios and `**Tests:**` pointers. Three previously undocumented principles were added to Spec 001: Req 12 (Explicit Type Annotations — Principle 1), Req 13 (Minimal Control-Flow Surface — Principle 2), Req 14 (Vocabulary over Syntax — Principle 3). Drift from the language definition now produces a `make assurance` failure rather than a silent gap (#427).

## [0.72.1] — 2026-05-04

### Fixed

- **`mvl mcdc --json` source field now shows correct stdlib lines** — Decisions in stdlib functions (`take_while`, `skip_while`, `find_index` while loops from `lists.mvl`) were attributed to the test module's file stem, causing the `"source"` field to show unrelated lines from the test file. Fix: post-process decisions to reassign `file` to the correct prelude stem and load prelude source texts into the lookup map (#472).
- **Example files updated to require explicit type annotations** — All 190+ bare `let x = expr` bindings across `examples/access_control/`, `examples/flight_clearance/`, and `examples/medical_triage/` now include `: Type` annotations as required since #408 (#470, #471).

## [0.72.0] — 2026-05-04

### Added

- **MC/DC coverage analysis now outputs machine-readable JSON** — `mvl mcdc <file|dir> --json` produces structured JSON with test counts, decision/obligation metrics, and per-clause coverage detail. `--json --quiet` emits summary only. Enables CI integration, coverage dashboards, and qualification evidence packages (DO-178C, IEC 62304). `independence_pair` is `null` pending test trace integration (#319); `coupled_with` is populated from coupled condition analysis (#325) (#326).

## [0.71.1] — 2026-05-03

### Fixed

- **Borrow-inferred params in struct literals and map expressions now emit `&x` correctly** — `Expr::Construct` and `Expr::Map` were creating a fresh `RustEmitter::new()` (empty `borrow_params_map`) for each field/value expression, so borrow-inferred function arguments inside struct literals emitted `x.clone()` instead of `&x`. Fixed by emitting directly into the parent `cg` emitter, which carries the real `borrow_params_map`. Regression tests added (#465).

- **Medical triage example now type-checks under the Rust transpiler** — ~89 bare `let` bindings in `examples/medical_triage/triage_test.mvl` lacked the explicit type annotations required since #408. Added `: Vitals`, `: Patient`, `: Priority`, `: Assessment` annotations. The example now compiles and runs end-to-end with `mvl test`.

- **Release build no longer warns about unused variable `other`** — `_other` prefix applied in `src/mvl/codegen/exprs.rs` where the variable is only referenced inside a `#[cfg(debug_assertions)]` block invisible in release mode.

## [0.71.0] — 2026-05-03

### Added

- **`std.pbt` — property-based testing stdlib (Phase A + B)** — New `std/pbt.mvl` declares the full PBT API surface: generators (`gen_int`, `gen_float`, `gen_bool`, `gen_string`, `gen_list_int`), combinators (`gen_filter_int`, `gen_one_of_int`, `gen_map_int_bool`), property runners (`property_check_int/bool/string/list_int`), Phase B mutation operators (`mutate_int/float/string/list_int`), and targeted + mutation-based property checkers (`property_check_targeted_int`, `property_check_with_mutation_int`). All stubs use `panic("stub")`. Import via `use std.pbt.{...}` (#40, #425).

- **`tests/corpus/03_stdlib/pbt_operations.mvl`** — Corpus file exercising the full PBT API: `test_divide_never_fails`, `test_list_len_nonneg`, `test_string_len_nonneg`, `test_bool_property`, combinator demos (`test_filtered_generator`, `test_one_of_generator`), Phase B mutation demos, and targeted + mutation-based property check demos (#40, #425).

- **`stdlib_pbt_corpus_parses_and_checks` type-checker test** — Integration test asserting the PBT corpus parses and type-checks with no serious errors (filters expected `UndefinedFunction`, `UndefinedVariable`, and `UndefinedType` — the latter because `Generator[T]` is not yet a built-in type) (#40, #425).

- **`std.log` real implementation (Rust transpiler path)** — Replaces no-op stubs in `std/log.mvl` with real `eprintln!`-backed implementation. Format: `[LEVEL ISO_8601_TIMESTAMP] msg field=value ...`. Field keys are sorted for deterministic test output. Timestamp from `time::now()` + `format_instant()`. Passes `Secret[T]` and `Tainted[T]` label checks in the type system (IFC symmetry with `! Log` effect). No configurable sink in Phase A (follow-up for Phase 3 / #54).

- **Log C-ABI exports for LLVM backend** — `mvl_runtime_c::stdlib::log` exports `_mvl_log_debug`, `_mvl_log_info`, `_mvl_log_warn`, `_mvl_log_error` with `(MvlString*, MvlMap*) → void` signature. Handles null pointers robustly and reconstructs field map iteration from open-addressing hash storage. LLVM codegen gains `VoidStringMapArg` dispatch variant. Cross-backend tests verify identical log output on both transpiler and LLVM backends (#434).

- **Log safety fixes and extended test coverage** — Field key names now sanitized (was value-only; keys with newlines or `=` would corrupt the format). `read_mvl_string` and `read_mvl_map` in the C-ABI bridge include guards against corrupt sizes and null pointers. Extended `sanitize()` to cover `\t` and `\0` in addition to `\n` and `\r`. Added 5 unit tests to `mvl_runtime_c/src/stdlib/log.rs` including double-pointer roundtrip test for value reconstruction. Added IFC test for `Clean[String]` in map field value position.

### Changed

- **`format_instant` signature: `String` → `&str`** — Eliminates per-call `String` allocation for a constant format pattern. Reduces allocation pressure in hot path (every log call).

- **Cross-backend log test robustness** — `cross_backend_log_stderr` now always runs transpiler path assertions regardless of LLVM availability; only the LLVM parity half is conditional. Line-count filter tightened to exact `[LEVEL space]` patterns to avoid false matches on LLVM diagnostics.
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
