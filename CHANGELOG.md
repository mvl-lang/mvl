# Changelog

## [0.245.3] - 2026-07-11

### Fixed — Wildcard let bindings type annotations

- Restore type annotations on `let _: T = expr` bindings; commit 72426026 suppressed them to avoid a hypothetical E0308 reborrow case that never actually manifests in generated code. The suppression broke `corpus_bitwise_transpiles` which asserts `let _: u8 = (a & b)` — loss of Byte → u8 type mapping weakens correctness verification (#1766).

## [0.245.2] - 2026-07-11

### Fixed — Rust Emitter: ref T param deref in binary ops

- Restore `emit_operand_deref_cap` lost during rebase conflict resolution in #1760; `ref T` capability params (e.g. `budget: ref Int`) become `&mut T` in Rust and require `(*param)` wrapping before comparisons and arithmetic — without it `compiler/make test` failed with 56 E0308 errors (#1764).

## [0.245.1] - 2026-07-11

### Fixed — IFC Label Unwrapping in Rust Emitter

- Emit `.0` unwraps for labeled Int/Bool operands in arithmetic, comparison, conditional, and match expressions (`Secret[Int]`, `Tainted[Bool]`, etc. are Rust newtypes) (#1708).
- Remove stale `lib.rs` when building a binary crate and stale `bridge.rs` when no bridge is needed, preventing cargo from trying to compile files with missing dependencies (sibling dispatch test fix).
- Regenerate `checker_parity/baseline.tsv` to include new solver-layer corpus files.

## [0.245.0] - 2026-07-11

### Added — Self-Hosted Refinements & Contracts Checker

- **Refinement type checking**: `compiler/refinements.mvl` ports refinements.rs from Rust — call-site validation of function parameters against `where`-predicates, hypothesis injection from match patterns and if-conditions, and 5-layer solver dispatch (trivial / interval / symbolic / Cooper / Z3). Per-call-site proof tracking with outcome logging (#1739).
- **Contract checking**: `compiler/contracts.mvl` ports contracts/ from Rust — `requires` clause validation at function call sites with single-parameter argument checking, `ensures` clause validation at return points, and stubs for loop invariants and field refinement validation. Parser-stage contracts not yet preserved (awaiting parser extension).
- **Proof tracking**: RefCounts, ProofEntry, ProofSite, and LayerCounts types record proof outcomes per solver layer and call site — enables assurance dashboards and proof-layer profiling in the self-hosted checker.

## [0.244.1] - 2026-07-10

### Fixed — Compiler Self-Hosting REQ8 Errors

- Add `decreases` clauses to `split_once` functions in `calls.mvl`, `decls.mvl`, `infer.mvl` (REQ8 unbounded loop)
- Convert pure `method_ty` dispatch functions from `partial` to `total` in `calls.mvl` (REQ8 total-calls-partial)
- Mark `test partial fn` on test functions that call partial functions in `calls.mvl`, `checker.mvl`, `decls.mvl` (REQ8)

## [0.244.0] - 2026-07-10

### Added — Self-Hosted IFC Checker

- **Information flow control (IFC)**: `compiler/ifc.mvl` and `compiler/ifc_propagation.mvl` port implicit flow analysis and interprocedural label propagation from Rust. Tainted value tracking, secret classification, and policy enforcement now run in the MVL self-hosted checker (#1738).
- **Label tracking**: tracks `Tainted[T]` and `Secret[T]` types through assignments, function calls, and data flow — a complete rewrite of the implicit flow engine to handle refinement type labels in a self-hosted context.
- **Interprocedural propagation**: label refinement information flows across function boundaries — callers' label constraints are discharged using callee context.

## [0.243.1] - 2026-07-10

### Fixed — CI

- **Corpus codegen gate**: `mvl build --emit-only` transpiles but skips `cargo build`, enabling fast emitter panic detection (~0.1s per file). New `test-corpus-codegen` target runs emitter on all 178 corpus files in the fast `make test` gate. Catches panics that `mvl check` alone cannot (e.g., unreachable!() in emit_types.rs, emit_exprs.rs) (#1705).

## [0.243.0] - 2026-07-10

### Added — Self-Hosted Checker

- **Call inference**: `compiler/calls.mvl` ports call inference from Rust — `infer_fn_call`, `infer_method_call`, and related helpers now run in the MVL self-hosted checker (#1117)
- **Declaration registration**: `compiler/decls.mvl` ports declaration collection from Rust — top-level declaration gathering and symbol registration now run in the MVL self-hosted checker (#1117)
- **Test coverage**: comprehensive tests for `infer_fn_call`, `infer_method_call`, and `collect_declarations` added to the checker test suite (#1117)

## [0.242.0] - 2026-07-10

### Added — Bootstrap

- **Bootstrap E2E target**: `make test-bootstrap-e2e` runs the complete self-hosting tracer bullet: `mvl tir hello_world.mvl | mvl run compiler/backends/llvm/emitter.mvl | llc | cc | ./hello` → assert `Hello, world!`. Serves as a permanent regression guard for the MVL → self-hosted emitter → binary pipeline (#1746).

## [0.241.2] - 2026-07-10

### Fixed — Transpiler

- **Sibling borrow flags**: `emit_sibling_module` now passes `peer_tirs` as `sibling_tirs` to `build_capability_params_map_tir_with_siblings`, so borrow flags for cross-module functions (e.g., `clone_locals`, `clone_structs`, `extend_locals`, `enum_name_of_variant`, `is_aggregate_ty`) are populated for every sibling module. Without this, call sites emitted `x.clone()` instead of `&x`, causing 43 E0308/E0277 compile errors in the self-hosted LLVM emitter (#1745). End-to-end tracer bullet now passes: `mvl tir hello_world.mvl | mvl run emitter.mvl → llc → cc → Hello, world!`

## [0.241.1] - 2026-07-10

### Fixed — Transpiler

- **Hybrid prelude module imports**: inject synthetic `use std.<module>` declaration when stripping types from hybrid stdlib modules (e.g., `std/env.mvl`) to avoid duplicate definitions with the runtime. This ensures the emitter records the dependency in TIR and emits `use mvl_runtime::stdlib::<module>::*;` in every file that receives prelude functions, resolving dangling `Signal` type references in `mvl build` output (#1744).

## [0.241.0] - 2026-07-10

### Added — Self-Hosted Checker (#1117)

- **Core type inference**: ported `checker.mvl`, `infer.mvl`, `check_stmts.mvl` — the main type inference engine now has a self-hosted MVL implementation alongside the Rust reference.
- **Call graph & termination**: ported `call_graph.mvl`, `verify_termination.mvl`, `verify_data_race.mvl`.
- **Context, sessions, patterns**: ported `context.mvl` (capability release), `verify_session.mvl`, `verify_patterns.mvl`.
- **Checker parity harness**: captures a baseline TSV of diagnostics emitted by both Rust and MVL checkers; CI flags regressions.

### Fixed — Rust Backend

- **`val self` extension methods**: restored `val self` as `&self` in emitted Rust (consistent with callers that pass `val T` params as `&T`).
- **FieldAccess receiver clone**: `emit_user_method_receiver` now adds `.clone()` for `FieldAccess` receivers on Named types, fixing E0507 move-out-of-`&mut`-reference errors in actor bodies (e.g., `self.logger.warn(...)`).
- **MethodCall disqualifying (narrowed)**: method calls on user-defined (Named) type parameters now disqualify borrow inference, preventing E0507 when user extension methods take `self` by value. Builtin-type receivers (List, Map, String, …) are unaffected — their stdlib methods take `&self` and do not consume the value.

### Fixed — Examples

- **`log_analyzer` parser_test**: aligned local `parse_level` signature to `Tainted[String]` to match the production version, eliminating a silent name-collision that produced incorrect borrow inference when compiled together.

## [0.240.0] - 2026-07-09

### Added — Refinement Solver

- **Layer 1 string concat support** (#1743): `min_str_len_lower` helper computes a conservative lower bound on string lengths in `.concat()` chains by summing all literal substrings. Enables static proofs of `len(result) > 0` when a concat chain has a non-empty literal prefix or other measurable component. Example: `"[".concat(acc).concat("]")` proves `len > 0` without runtime checks.

## [0.239.2] - 2026-07-09

### Fixed — Stdlib

- **`std/args.mvl`** `schema_has_name`: pattern arms that only use `name: n`
  now bind `ty: _` instead of `ty: t`, suppressing unused-variable warnings
  in every transpiled module that imports `std.args`.
- **`std/json.mvl`** `parse_array` / `parse_object`: removed dead
  `let len: Int = chars.len()` — these functions switched to
  `chars.get(cur) → Option` for bounds checking and no longer reference
  `len`.  Eliminates two unused-variable warnings per compiled module.

## [0.239.1] - 2026-07-09

### Fixed

- **Resolver qualified module imports** (#1734): `find_module_file` now walks ancestors when looking up qualified module paths (e.g., `backends.llvm.emit_context`). Bounded by project-root markers (`mvl.toml` / `.git`). Fixes "unknown module" errors when entry files live inside a qualified module tree.

- **Transitive sibling module discovery** (#1734): New `load_sibling_modules_transitive` helper (BFS) ensures peers-of-peers are loaded before resolution — e.g., when `emitter.mvl` imports `emit_program.mvl` which imports `emit_types.mvl`, all three are now available to the resolver.

- **Rust backend dotted module names** (#1734): Added `mvl_mod_to_rust_ident` helper to fold dot-qualified MVL module names into single valid Rust identifiers (`backends.llvm.emit_context` → `backends_llvm_emit_context`). Applied to `pub mod` declarations, `use` statements, and sibling file paths in `build.rs`. Bare names unchanged for compatibility.

Enables `mvl build / mvl run` on split MVL-hosted backends (e.g., LLVM emitter) with qualified peer imports.

## [0.239.0] - 2026-07-09

Ships alongside **runtime 0.198.0** (mvl_runtime_rust, mvl_runtime_llvm,
mvl_runtime_tokio) — the runtime crate is bumped to reflect the self-hosted
LLVM backend modularization (#1693).

### Changed — MVL compiler (self-hosting refactor)

- **LLVM backend modularization** (#1693, ADR-0054): The 2983-LOC monolith
  `compiler/backends/llvm/backend_llvm.mvl` has been split into eight
  peer modules via sibling method dispatch (#1710, ADR-0052):
  - `emit_context.mvl` — context types and accessors (EmitCtx, LocalRef, StructInfo)
  - `emit_types.mvl` — TIR→LLVM type lowering and registry builders
  - `emit_helpers.mvl` — JSON helpers, LLVM constants, string escaping
  - `emit_exprs.mvl` — expression emission and if-block handling
  - `emit_match.mvl` — match-statement lowering to LLVM IR
  - `emit_stmts.mvl` — statement and loop emission
  - `emit_program.mvl` — top-level driver and string collection
  - `emitter.mvl` — main entry point
  
  Each module is a concern-focused peer implementing related EmitCtx methods,
  mirroring the Rust backend structure. ~200 unit tests added; coverage
  increased from 42% → 72%.

- **Method receiver clone semantics** (ADR-0054): User-defined method calls
  now apply the same last-use clone logic as free-function arguments. Methods
  called on locals used multiple times in a fn body no longer raise E0382
  move errors. Stdlib methods remain borrow-neutral per dispatch arm.

### Fixed — Rust backend

- **Test-runner sibling module resolution** (#1714): `mvl test` now matches
  nested library files (e.g., `backends.llvm.emit_program`) by both bare
  file stem and qualified module path, fixing 196 test-discovery failures
  after qualified module paths landed on main.

- **Binary expression string collection**: `collect_strings_in_expr` in
  `emit_program.mvl` now reads `left`/`right` fields (not stale `lhs`/`rhs`),
  ensuring string literals in binary ops are captured for emission.

## [0.238.16] - 2026-07-09

Ships alongside **runtime 0.197.1** (mvl_runtime_rust, mvl_runtime_llvm,
mvl_runtime_tokio) — the runtime crate is bumped because `Match` and
`Captures` in `stdlib/regex.rs` gained `#[derive(Debug, Clone, PartialEq, Eq)]`,
matching every other stdlib struct.  Compiler is bumped because MVL stdlib
(`std/*.mvl`) shipped with a compiler is versioned alongside it.

### Changed — MVL language (ADR-0053)

- **Parser** rejects the trailing `where T: Trait` clause on fn
  signatures.  MVL has no trait system; the syntax was Rust vocabulary
  that leaked in without semantic backing on the MVL side.  `where` in
  MVL now means one thing only: a solver-discharged predicate on a
  param, return type, struct field, or type alias.  Any `where T:
  BananaBread`-shaped clause is a hard parse error citing ADR-0053.
- **Stdlib** (`std/lists.mvl`, `std/collections.mvl`) stripped of the
  ~27 sites that used the removed grammar.  The bounds were never
  enforced by MVL and only load-bearing because the Rust backend
  re-emitted them verbatim.
- **Tree-sitter grammar** (`etc/tree-sitter-mvl/grammar.js`) synced —
  `constraint` / `constraints` / `trait_bound` rules removed and the
  `fn_decl` production no longer terminates with an optional
  `where`-clause.
- **Documentation**: new ADR-0053, index updated, `CLAUDE.md` guidance
  section added, `docs/grammar.ebnf` production reduced.

### Fixed — Rust backend

- **Bound derivation**: `emit_generics_with_tir_params` now derives
  `T: Clone` (and Hash/Eq for Map/Set positions) automatically from
  the fn signature.  Rust bounds stay inside the emit; MVL source
  never references them.  Replaces what the deleted `where T: Clone`
  stdlib clauses used to provide.
- **`.sort()` emit**: clones the receiver into an owned `__v` so the
  block returns `Vec<T>` even when `capability_params` inferred a
  borrow at the caller.  Without the clone, a borrowed receiver made
  the block return `&Vec<T>` (E0308 return-type mismatch).
- **Float `is_positive` / `is_negative`**: receiver-type dispatch
  routes to Rust's non-deprecated `is_sign_positive` /
  `is_sign_negative`.  Int keeps the same-named non-deprecated `i64`
  methods.  Removes the two `deprecated` warnings the emit was
  producing per callsite.
- **Checker diagnostic**: `MissingConstraint` no longer advises the
  (now-invalid) `where T: Eq` remedy — points users at concrete-type
  specialization instead.
- **Corpus** (`tests/corpus/02_functions/functions.mvl`): the two
  demonstrative generic wrappers replaced with concrete-Int
  specializations.  New expect-fail negative
  `tests/corpus/02_functions/no_trait_bound_where_clause.mvl`
  documents the parser rejection.

### Fixed — Rust runtime (0.197.1)

- **`stdlib::regex::Match`** and **`Captures`** now derive `Debug,
  Clone, PartialEq, Eq`.  Every field was already Clone-able; the
  missing derive blocked value-semantic call patterns.  Fixes
  `tests/stdlib/regex_test.mvl` failure ("`Match: Clone` is not
  satisfied") that surfaced once the compiler stopped accepting
  user-declared `where` bounds.

### Test crate

- `mvl test tests/corpus/` — **0 errors, 0 warnings, 291 tests
  passing** (up from 220 errors / 10 warnings at the start of #1707).

## [0.238.15] - 2026-07-09

### Fixed

- **`rust-backend`: inline dispatch for `List::find`** (#1707 phase 14) — MVL corpus calls `xs.find(target)` on `List[Int]` expecting `Option[Int]`.  Follows the existing `concat` / `get` receiver-type-specific dispatch pattern in `emit_method_call.rs`.  Adds a `find` arm that fires only when the receiver is a `List` — String's `find` still goes through `BUILTINS` (`str_find`).  Emits `xs.iter().position(|__x| __x == &target).map(|n| n as i64)` — returns `Option<i64>` matching MVL's `Option[Int]`.  Cleared 2× E0277 misdispatch (or 1× E0599 after phase 12) → 0 errors.

## [0.238.14] - 2026-07-09

### Fixed

- **`rust-backend`: strip `ref` wrapper on struct field types** (#1707 phase 13) — MVL's `count: ref Int` on a struct field is a mutability modifier (writable when the containing struct is reached via a `ref` binding) — it is NOT a reference type.  MVL usage confirms: constructors take plain values (`Counter { count: 0 }`).  The Rust emitter fed the field type through `emit_ty` which mapped `Ty::Ref(true, Int)` → `&mut i64`, requiring lifetime injection on every struct use (E0106).  Fix: strip `Ty::Ref(_, inner)` on struct fields before `emit_ty`.  Mutation at use sites comes from Rust ownership on the containing binding.  Cleared 2× E0106 on `03_types/{structs,immutability}.mvl`.

## [0.238.13] - 2026-07-09

### Fixed

- **`rust-backend`: type-aware method dispatch for builtins and UFCS** (#1707 phase 12) — Two shared lookups in `emit_method_call.rs` were name-only: `rust_emit_by_name(m)` hunted `BUILTINS` for any entry with method name `m`, ignoring receiver type.  For `xs.find(target)` on `List[Int]` this returned `Some("str_find")` (from `("find", "String")`) and emitted broken code with two confusing E0277 trait errors.  Fix: new `ty_builtin_key(ty)` helper mapping `Ty` variants to `BUILTINS` string keys, use `rust_emit_for(name, ty_key)` at the dispatch arm, add `is_stdlib_ufcs_method_for(name, ty)` as the type-aware companion to `is_stdlib_ufcs_method`.  Sets up infrastructure so future `List::find` / `Set::find` / `Map::find` additions can coexist with `String::find` without silently poaching each other's dispatch.  Cleared 2× E0277 misdispatch (converted to honest E0599 "no method").

## [0.238.12] - 2026-07-09

### Fixed

- **`rust-backend`: skip redundant `.into()` when call result already matches target** (#1707 phase 11) — Two symmetric emit sites unconditionally appended `.into()` when the enclosing context expected a labeled type and the value was a `FnCall` / `MethodCall`: `emit_functions::emit_expr_tail_with_return_type_tir` and `emit_stmts::emit_stmt` (Let).  Useful when the call returns a plain `T` needing coercion to `Label<T>`.  But when the call already returns the labeled type (e.g. `identity[T](x: T) -> T` invoked with `t: Tainted[String]`), the `.into()` is a no-op AND blocks Rust from inferring `T` — E0282.  Fix: gate both emissions on `expr.ty != *ret_ty` (or `init.ty != *ty`).  Purely type-driven — no method-name lists.  Cleared 2× E0282 on `02_functions/generic_instantiation.mvl` and `08_ifc/secret_env.mvl`.

## [0.238.11] - 2026-07-08

### Fixed

- **`rust-backend`: auto-clone Var on let-init when not last use** (#1707 phase 10) — MVL has value semantics: `let a: Pair = p; let b: Pair = p;` treats each `let x = p` as a *copy*, leaving `p` alive.  Emitter was writing `let a = p; let b = p;` — Rust interprets both as MOVE, invalidating the second read with E0382.  Fix: in `TirStmt::Let`, when the init is a bare `Var` and its span is NOT in `self.last_uses`, append `.clone()`.  Mirrors the existing `field_needs_clone` check and reuses last-use analysis already computed per-body.  Cleared 1× E0382 on `06_ownership/value_semantics.mvl`.

## [0.238.10] - 2026-07-08

### Fixed

- **`rust-backend`: skip `.into()` on map values with primitive V type** (#1707 phase 9) — `TirExprKind::Map` unconditionally emitted `.clone().into()` on every value.  Useful for labeled/refined value types (`Secret[String]`, `PosInt`) which coerce via `From`.  For primitive value types there's no target — `HashMap::from([("a", 1.into()), ("b", -2.into())])` failed E0282 on numeric-literal inference.  Fix: inspect `expr.ty` and only emit `.clone().into()` when V is `Ty::Labeled(..)` or `Ty::Refined(..)`.  Cleared 2× E0282 on `05_collections/map_set_hof.mvl`.

## [0.238.9] - 2026-07-08

### Fixed

- **`test-runner`, `rust-backend`: skip typecheck-only corpus + strip val let annot** (#1707 phase 8) — Three fixes.  (1) New `is_typecheck_only(prog)` helper in `src/cli/test.rs` skips files whose every `test fn` is either named `*_typecheck` (MVL corpus convention) or bodied only by `touch(...)` calls — these files exist to demonstrate declaration forms parse and type-check, not to run. `make test-corpus` still validates them via `mvl check`.  (2) In `src/mvl/backends/rust/emit_stmts.rs`, `let x: val T = init` was emitting `let x: &T = <owned>` (E0308).  MVL `val` at a let site conveys value semantics; the RHS is owned in Rust, so strip the wrapper just like `ref`.  (3) `tests/corpus/01_syntax/expressions.mvl` used non-existent free functions (`abs`, `max`, `parse_int`); MVL check treated them as opaque unknown calls (too lenient — separate issue).  Replaced with `int_abs`/`int_max`/method-form `input.parse_int()` and added `use std.math.{int_abs, int_max}`.  Cut 58 → 11 rustc errors on `mvl test tests/corpus/` (cumulative 95% reduction).

## [0.238.7] - 2026-07-08

### Fixed

- **`rust-backend`: unwrap refinement newtype operands in checked arithmetic** (#1707 phase 7) — MVL's checked-arithmetic emission wraps both operands in `<i64>::clone(&(x))` to force `i64::checked_add`-family dispatch (matches LLVM overflow behaviour).  When the operand is a refined alias like `Positive` (a newtype wrapping `i64`), the borrow resolves to `&Positive`, not `&i64`, and rustc rejected with E0308.  Fix: at the `Ty::Int` arithmetic emit site, consult `self.refined_alias_base(&operand.ty)` for each operand and append `.0` when the operand is a refined alias.  Both operands are checked independently — `Positive + Int`, `Int + Positive`, `Positive + Positive` all work.  Cleared 3× E0308 in `refinement_totality_interaction::{positive_sum,bounded_sum}`; total corpus errors 61 → 58 (74% reduction).

## [0.238.6] - 2026-07-08

### Fixed

- **`rust-backend`: keep `let mut` on refs passed as `&mut` args** (#1707 phase 6) — `mut_analysis::compute_readonly_names` treated free-function args as pure reads and stripped `mut` from bindings only ever passed to callees.  When the emitter then consulted `capability_params_map` and emitted `&mut tf` at the call site, rustc rejected with E0596.  Concrete case in `tests/corpus/13_stdlib/temp_files.mvl`: `let tf: ref TempFile = temp_file()?; get_temp_path(&mut tf);` emitted as `let tf: TempFile = ...; get_temp_path(&mut tf);` — E0596.  Fix: thread `capability_params_map` into `compute_readonly_names` so the walk sees which arg positions the emitter will `&mut`, and marks the source binding as mutated.  New helper `visit_arg_expr(expr, is_mut_borrow)` dispatched from `TirExprKind::FnCall`.  `TirExprKind::MethodCall` args stay at `is_mut_borrow = false` for now.  Cleared 2× E0596; total corpus errors 63 → 61 (72% reduction).

## [0.238.5] - 2026-07-08

### Fixed

- **`rust-backend`: qualify unit-variant patterns in match arms** (#1707 phase 5) — Match arms like `match dir { North => "north", South => "south" }` where `North`/`South` are variants of `Direction` were emitted as bare Rust identifiers.  Rust interprets bare `North` in pattern position as a fresh *binding*, not a match against `Direction::North` — yielding E0170 warnings/errors and, more critically, wrong runtime semantics: the first arm always matches everything.  Fix: new `unit_variants_per_enum` registry on `RustEmitter` populated from `TirTypeBody::Enum` variants with `TirVariantFields::Unit`, threaded from `Match { scrutinee, arms }` down through `emit_match_arm` → `emit_pattern_with_enum(pat, Option<&str>)`.  When the scrutinee's `Ty::Named(name, _)` resolves to a registered enum, bare-ident patterns matching a known unit variant are emitted qualified.  `Pattern::Or` propagates the hint through alternatives.  Cleared 12× E0170; total corpus errors 75 → 63 (71% reduction).

## [0.238.4] - 2026-07-07

### Fixed

- **`test-runner`: skip corpus:expect-fail files in mvl test** (#1707 phase 4) — Files annotated `// corpus:expect-fail` are negative test cases for `mvl check` — they intentionally contain IFC/ownership/type violations that MUST cause the checker to reject them. `make test-corpus` handles them via the Makefile annotation. `mvl test` was bundling them into the shared test crate anyway, then transpiling code that MVL itself declared invalid (e.g. `if secret_bool` in `08_ifc/implicit_flow.mvl`). Fix: add `is_expect_fail(path)` in `src/cli/test.rs` and filter both `test_files` and inline-test `source_files` through it. Cut 83 → 75 rustc errors on `mvl test tests/corpus/` (cumulative 66% reduction).

## [0.238.3] - 2026-07-07

### Fixed

- **`test-runner`: scope stdlib prelude per-file in bundle mode** (#1707 phase 3) — Two coupled fixes for `mvl test`'s bundling path in `src/cli/test.rs`. (A) Inline-test corpus files (`.mvl` with `test fn`, not `_test.mvl`) never reached the prelude pre-scan, so their `use std.X` imports never triggered stdlib loading — 100+ E0433/E0425 errors on `RestartStrategy`, `AuditEvent`, `Logger`, etc. Pre-scan now folds inline-test source files into `all_for_extras`. (B) With `std.log` now in the shared prelude, the stdlib `pub struct Logger` was injected into every mod, colliding with corpus files declaring their own `actor Logger` (E0428). Fixed by splitting `stdlib_prelude_progs` at `n_universal_prelude_outer`: below is universal (implicit + siblings + pkg), above is filtered per-file via `load_mvl_native_stdlib_extras(&[prog, ...sibling_progs])`. Cut 220 → 83 rustc errors on `mvl test tests/corpus/` (62% reduction).

## [0.238.1] - 2026-07-07

### Fixed

- **Module resolver: qualified module paths for same-basename files** (#1714) — Two `.mvl` files in different directories that share a basename (e.g. `compiler/context.mvl` and `compiler/backends/llvm/context.mvl`) previously collided silently: the resolver's HashMap registered whichever was enumerated first and discarded the other. Imports bound to the wrong module, producing misleading diagnostics like "`LocalRef` is not exported from `context`" when it actually lived in `backends/llvm/context.mvl`. Fix: derive module names from the file's path relative to the CLI base directory, replacing path separators with dots. Files now register under distinct qualified names (`"context"` vs. `"backends.llvm.context"`), and imports use the full dot-qualified path: `use backends.llvm.context::EmitCtx`. Same-basename files in different subdirectories can now coexist without renaming or collision. Includes: (1) `qualified_stem(base_dir, file)` function; (2) updated `collect_imported_module_names` to return dot-joined module paths; (3) updated `find_module_file` to resolve dot-paths to filesystem paths; (4) resolver lookup changes from `join("::")` to `join(".")` for key matching; (5) all CLI entry points (check, build, assurance, prove) updated to use qualified names. Three new integration tests, six new loader unit tests, spec 005 Requirement 1 and 3 updated, and ADR-0052 added.

## [0.238.0] - 2026-07-06

### Added

- **Go-model sibling method dispatch** (#1706) — Extension methods on the same type can now be split across sibling `.mvl` files in the same directory without cyclic `use` imports. Files in the same build unit share extension method declarations through the type system: `point_display.mvl` may call `self.sum()` (defined in `point_arith.mvl`) as long as both are compiled together via the entry module. Mirrors Go's package model: within a directory, type methods are ambient; between directories, explicit `use` imports are required and cycles are still rejected. Three changes: (1) sibling pre-check now includes all peer siblings in each sibling's prelude so cross-sibling method calls type-check correctly; (2) transpiler suppresses `use crate::mod::name` for extension method imports from sibling modules (they are Rust struct methods, not standalone functions); (3) the suppression is applied in all emission paths — both entry module and sibling module files. Includes a four-file runnable example (`examples/programs/sibling_dispatch/`) and corpus tests (`tests/corpus/18_modules/`).

## [0.237.4] - 2026-07-05

### Fixed

- **`test-runner`: module name collision in bundled tests** (#1707 phase 1) — `mvl test <dir>` concatenates transpiled code from multiple corpus files into a single Rust crate, wrapping each file in `#[cfg(test)] mod <name> { ... }`. The module name was derived from the filename stem only (e.g., `propagation.mvl` → `propagation`), so files with identical names in different directories collided (`07_effects/propagation.mvl` and `08_ifc/propagation.mvl` both became `mod propagation`). Rust rejected with E0428: duplicate module definition. Fix: qualify module names by full path segment — replace `/`, `\`, `-`, `.` with `_`, strip trailing `_test`, and prefix `_` if the first character is non-alphabetic. Measured: 221 rustc errors → 220 (E0428 eliminated; Phases 3–4 address remaining structural issues). Includes 5 new unit tests for path-to-module-name derivation.

## [0.237.3] - 2026-07-05

### Performance

- **`mvl test --backend=llvm`: parallelize test runner** (#1699) — `cmd_test_llvm_text` in `src/cli/llvm_text.rs` iterated fixtures sequentially, so each `lli` fork stacked back-to-back. Rewrite to run test cases across a worker pool sized by `std::thread::available_parallelism()`, mirroring the pattern in `src/cli/mutate.rs`. Each worker parses, lowers, and runs its assigned files under `lli` independently; output is collected and printed in deterministic file order after all workers finish, so `--verbose` PASS/FAIL reporting stays stable. Also hoists `lli::find_mvl_runtime_llvm_lib()` out of the per-file loop (was doing a filesystem probe for every test). Measured on macOS, 10-core M-series, warm cache: `make test-backend-llvm` 2.13-2.52s → 1.00-1.14s (~2.1x); `mvl test tests/corpus/ --backend=llvm` 2.11s → 0.16s (~13x). Ticket originally framed the LLVM path as "3 forks per file × 200 files" (llc + cc + binary); current backend actually uses `lli` — 1 fork per file, 38 executed fixtures across corpus + intrinsics + stdlib. Correction posted on the issue.

### Fixed

- **`rust_backend`: quantifier contracts panicked codegen** — `emit_ref_expr_for_assert` in `src/mvl/backends/rust/emit_types.rs` panicked with `unreachable!("quantifiers are ghost-only and must not appear in codegen")` on any function whose contract used `requires forall …` / `ensures exists …`. The comment was correct (quantifiers cannot be checked at runtime) but nothing was filtering them out at the four emit sites in `emit_functions.rs`. Consequence: `mvl test tests/corpus/` crashed on `tests/corpus/01_syntax/keywords.mvl` before running any test. Fix: add `is_runtime_checkable(&RefExpr)` which walks the predicate tree and returns `false` iff any subtree is a quantifier; filter `fd.requires` / `fd.ensures` through it at each `emit_ref_expr_for_assert` call. `has_ensures` gating on the `_result` binding uses the same predicate so pure-ghost `ensures` don't leave dead bindings. Verification uses of `forall` / `exists` remain intact — this only affects runtime assertion emission, not the checker or prover.

- **`rust_backend`: user-defined labels + relabel transitions crashed codegen (#990)** — Two hardcoded match blocks in `src/mvl/backends/rust/emit_exprs.rs::TirExprKind::Relabel` (audit + non-audit) enumerated only built-in transition names (`trust`, `release`, `classify`, `taint`, `db_url`, `config_path`, `api_endpoint`, `audit_target`, and their un-versions). Any user-declared `pub label Foo` / `pub relabel foo: A -> B` fell through to `_ => unreachable!("relabel '<name>': unknown transition — blocked by checker (#990)")`. Consequence: `tests/corpus/08_ifc/audit_relabel.mvl` (which declares `pub label Sensitive` and `pub relabel classify_audited: _ -> Sensitive audit`) panicked codegen even though the checker and standalone `mvl build` accepted it. Fix: add `RelabelKind` enum (`Wrap(String)` / `Unwrap` / `Transform(String)` / `Unknown`) and `relabel_kind(name)` on `RustEmitter` which classifies built-ins first, then falls back to a new `user_relabels: HashMap<String, (Option<String>, Option<String>)>` populated from ALL `tir.relabel_decls` (both entry and prelude, not just audit-flagged). Both audit and non-audit arms now consume the same `RelabelKind`, collapsing 60+ lines of duplicated matches. Additionally emit `#[repr(transparent)] pub struct <Name><T>(pub T);` for each user `LabelDecl` alongside `emit_tir_type_decl` — gated by `is_builtin_label()` so we don't collide with the runtime's `Secret` / `Tainted` / `DbUrl` / etc. re-exports.

### Follow-up

- **#1705** — `test-corpus` fast CI gate only runs `mvl check` (no codegen). Both `unreachable!`s fixed above were invisible to CI; they only surfaced when running `mvl test tests/corpus/` locally. Filed with 4 remediation options.

## [0.237.2] - 2026-07-05

### Fixed

- **`rust_backend`: cross-module capability param inference** (#1695) — Read-only borrow inference (`capability_params_for_tir_fn`) was module-local: `build_capability_params_map_tir` only saw the entry TIR + preludes, never sibling module TIRs. Consequence: a sibling fn like `use_map(m: Map[K, V])` that MVL's inference decided should be borrowed emitted `fn use_map(m: &HashMap<K, V>)` in the sibling's Rust file, but the entry emitter passed `use_map(mb)` (owned) — Rust rejected with `E0308: expected \`&HashMap<K, V>\`, found \`HashMap<K, V>\``. Fix threads sibling TIRs through `emit_program_with_mods_and_siblings` into a new `build_capability_params_map_tir_with_siblings`, which gives sibling fns the same "explicit + inferred" treatment as the entry TIR (entry wins on name collisions). Sibling TIRs are lowered upfront in `pipeline.rs::transpile_project_with_options` and reused by both the entry emitter and the per-sibling emit loop — zero extra monomorphization cost. Also sweeps sibling fn-type aliases into the alias set so cross-module Copy fn pointers aren't incorrectly borrow-inferred. New regression test `cross_module_map_arg_uses_borrow_at_call_site` in `tests/transpiler.rs`.

### Unblocked

- **#1693** — with #1692 (fixed in 0.235.2) and #1695 (this) both landed, the two transpiler-side prerequisites for splitting `backend_llvm.mvl` (2983 LOC monolith) into modular files are complete.

## [0.237.1] - 2026-07-05

### Fixed

- **`packages`: `mvl audit --license` misses transitive `[native]`/`[c-native]`** (#1701) — Follow-up to #1698. The license audit previously walked only the project's own `[c-native]` and the top-level `license` of each MVL dep (from `mvl.lock`). It did NOT roll up the `[native]` / `[c-native]` sections of each transitive MVL dep. Consequence: a project depending on `pkg-sqlite` never saw `sqlite3 (blessing)` in the audit even though pkg-sqlite v0.2.2 declares it. Fix: `cmd_audit_license` now walks each cached dep's `mvl.toml` and emits entries tagged with `introduced_by` (the MVL package that pulled them in). Direct entries win over transitive when the same name is declared in both (no double-reporting). Also enforces project-direct `[native]` licenses stored via the new `native_licenses` map introduced in #1698. Report format gains `(via pkg-name)` on the transitive rows. Refactored: `cmd_audit_license` now delegates to a pure `audit_licenses(&manifest, &lockfile, load_fn)` for testability without touching the global package cache — 5 new tests cover the rollup, unknown transitive entries, and direct-wins-over-transitive dedup.

## [0.237.0] - 2026-07-05

### Added

- **`rust_backend`: MVL-hosted LLVM strings + println** (#1118, Phase A1m) — `String` type end-to-end: literals, params, returns, plus the `println(String)` builtin lowered to libc printf. String → `{i64, ptr}` in LLVM (length + data ptr).  Each distinct literal → private `@.str.N` global with C-string bytes, deduped via first-seen registry. `void` returns emit `ret void` (no operand); `Unit` main skips the terminal printf. `EmitCtx` gains `strings: Map[String, String]`, threaded through all construction sites. 3 new spike tests. Corpus 35 → 38.
- **`rust_backend`: MVL-hosted LLVM structs** (#1118, Phase A1n) — `type T = struct { … }` declaration → `%<Name> = type { … }`, construction via chained `insertvalue`, field access via `extractvalue`.  New struct registry + `tir_ty_to_llvm_ctx` (struct-aware). 2 new spike tests. Corpus 38 → 40.
- **`rust_backend`: MVL-hosted LLVM enum payloads** (#1118, Phase A1o) — user enums extend to data variants: `type Value = enum { Zero, Num(Int) }` lowers to `{i8, i64}` when any variant carries payload. `TupleStruct` patterns dispatch on extracted tag and bind payload via extractvalue. New `enum_types` + `variant_payloads` registries. 2 new spike tests. Corpus 40 → 42.
- **`rust_backend`: MVL-hosted LLVM list ops + method dispatch** (#1118, Phase A1p) — new `MethodCall` branch. `xs.len()` → extractvalue index 0. `xs.get(i)` → GEP+load+Some (no bounds check in this phase — see A1t). `opt.unwrap_or(d)` → `select i1` on tag. Unlocks `xs.get(i).unwrap_or(default)`. 2 new spike tests. Corpus 42 → 44.
- **`rust_backend`: MVL-hosted LLVM struct-variant enums + multi-payload** (#1118, Phase A1q) — `type Session = enum { Open, Locked { attempts: Int, window: Int } }`. Enum LLVM shape widens to `{i8, i64, i64, …}` sized to the widest variant. `Struct` patterns dispatch on tag + extract each named field per slot registry lookup. `EmitCtx.variant_slots: Map[String, Map[String, Int]]`. `Session::Locked { … }` construction reuses shared `emit_variant_construction`. 1 new spike test. Corpus 44 → 45.
- **`rust_backend`: MVL-hosted LLVM Float payloads via bitcast** (#1118, Phase A1r) — `Float` → LLVM `double`, `f.to_int()` → `fptosi`. Enum payload slots stay `i64`; non-i64 payloads (currently `double`) bitcast to/from i64 at construction/extraction. `variant_slot_types` registry drives the bitcast decision. 1 new spike test. Corpus 45 → 46.
- **`rust_backend`: MVL-hosted LLVM multi-payload tuple variants** (#1118, Phase A1s) — `Rect(Int, Int)`, `Cube(Int, Float, Int)` work via the A1q infrastructure — no emitter changes, just coverage. 2 new spike tests. Corpus 46 → 48.
- **`rust_backend`: MVL-hosted LLVM bounds-checked .get()** (#1118, Phase A1t) — real Option semantics for OOB access: branch+phi in expression position. `icmp ult` for negative-index safety; None branch skips the load entirely. 1 new spike test. Corpus 48 → 49.
- **`rust_backend`: MVL-hosted LLVM nested list types** (#1118, Phase A1u) — `List[String]`, `List[Point]`, `List[<enum>]` — element type from `iter.ty.inner` drives ListLit storage + for-loop element load. Bonus fix: `emit_if_blocks` no longer emits `phi void` for Unit-typed arms. 2 new spike tests. Corpus 49 → **51**.

### Backend surface after A1m-A1u

Emitter: 1662 → 2983 LOC (+1321). Full spike corpus 51/51 pass end-to-end (TIR → IR → llc → cc → binary → assert stdout). `make test-mvl` clean throughout.

### Deferred (filed as follow-ups)

- **#1693** — split `backend_llvm.mvl` monolith into modular files (blocked on #1692 [fixed by #1694] + #1695).
- **#1695** — cross-module capability param inference for user-defined fns.
## [0.236.0] - 2026-07-05

### Added

- **`packages`: `mvl package check` + `license` on `[native]`** (#1698) — Closes the compliance gap where `mvl audit --supply-chain` sees external Rust crates (e.g. `rusqlite 0.31`) but `mvl audit --license` silently reports nothing. `[native]` entries now accept an inline-table form with a `license` field: `rusqlite = { version = "0.31", license = "MIT" }`; stored on the `Manifest` as a parallel `native_licenses` map so existing consumers stay unchanged. New `mvl package check` walks `[native]` and `[c-native]` — the same entries `--supply-chain` already knows about — and errors on missing `license`, unrecognized SPDX id, or a name declared in both sections. `mvl audit` with no flags now runs `--supply-chain` + `--license` + `--paradox` together (individual flags remain for CI granularity). SPDX list gains `blessing` (SQLite's public-domain-style license). Backfill for external `pkg-*` repos is a per-repo follow-up.

## [0.235.2] - 2026-07-05

### Fixed

- **`rust_backend`: cross-module Map/Set/List coercion** (#1692) — Two related transpiler surface defects that made splitting the LLVM emitter monolith (#1693) impossible.
  - **Variant 1 (`.get()` on labeled Map)**: `emit_method_call.rs::get` matched `Ty::Map(_, _)` directly, missing `Ty::Labeled(Ty::Map(...))` values (e.g. `Tainted<Map<K, V>>`). Those fell through to the List branch and emitted list-index code for a HashMap receiver — trips `E0308: expected String, found integer`. Fix: apply `.unlabeled()` before pattern matching, following the same pattern used by `filter`/`any`/`all` a few lines above.
  - **Variant 2 (`.into()` on Map/Set/List args)**: `emit_expr_as_value_arg` excluded `Option`/`Result` from the `.into()` coercion (no blanket `From<T> for Label<T>`) but Map/Set/List still went through it. Passing a `Map[String, StructInfo]` across a `use` boundary emitted `m.clone().into()` — `HashMap<String, StructInfo>: Into<_>` isn't satisfied. Fix: extend the exclusion list to include `Ty::Map`, `Ty::Set`, `Ty::List`. These clone in place instead of coercing.
- 3 new regression tests in `tests/transpiler.rs` covering Map/Set/List argument passing.

## [0.235.1] - 2026-07-05

### Fixed

- **`rust_backend`: method-call receiver parenthesization** (#1697) — MVL source `(b1 && b2).to_string()` was transpiling to Rust `b1 && b2.to_string()`, which parses as `b1 && (b2.to_string())` under Rust's operator precedence and trips `E0308: expected bool, found String`. Same failure mode for `.to_string()` on any Binary/Unary/If/Match/Lambda receiver. Fix: new `emit_method_receiver(receiver)` helper in `emit_exprs.rs` mirroring the existing `emit_operand_left` — wraps in parens iff `expr_own_prec(receiver) < Prec::Suffix`. All 57 `self.emit_expr(receiver)` call sites in `emit_method_call.rs` migrated to the wrapper. Precedence infrastructure (`Prec`, `expr_own_prec`) already existed, so this reuses the same table that governs binary-op sub-expression parenthesization. 4 new regression tests in `tests/transpiler.rs`.

### Changed

- **`build`: fold `test-integration` into `test-full` via `test-rust-integration`** — `test-integration` was a dev-convenience target running `cargo test --tests` but NOT included in `test-full`. Nine test binaries had zero coverage in the pre-merge gate: `transpiler`, `assurance`, `corpus_ir_parity`, `cross_backend_tir`, `linter_integration`, `manifest_rationale`, `meta_commands`, `module_resolver`, `toolchain`. `test-integration` deleted; equivalent `test-rust-integration` target added to `TEST_FULL_EXTRA_SUITES`. Overlap with `test-backend-rust`/`test-cross-backend` accepted (cargo caches).

- **`tests/integration/`: cleanup** — Delete stale `.gitkeep` in `error_messages/` (dir has 27 fixtures now); update `args.sh` docstring to reference `test-rust-integration` instead of the removed `test-integration`.

### Fixed (test infra)

- **`cross_backend_tir/common.rs`: drop stale `compiler.expr_types` refs** — `LlvmTextCompiler` no longer exposes an `expr_types` field (hoisted to pipeline-local state in an earlier refactor). The test helper still referenced it, causing `cargo test --tests` (and thus `make test-integration`) to fail with three `E0609` errors before any test ran. Bind `expr_types` as a local; thread through `mono` / `lower` directly. No behavioral change.
## [0.235.0] - 2026-07-04

### Added

- **`rust_backend`: MVL-hosted LLVM user enums + List[Int] iteration** (#1118, Phases A1k+A1l) — Two major features in one PR. A1l adds user-defined unit-variant enums: `type Color = enum { Red, Green, Blue }` lowers to `i8` (variant tag), variant references (`Color::Green`) emit i8 constants from a registry, variant patterns dispatch via `icmp eq i8`. A1k adds List[Int] literals and iteration: `[1, 2, 3]` lowers to stack-allocated `[N x i64]` wrapped in `{i64, ptr}` (len + data), `for x in xs` iterates by index via alloca'd counter with GEP+load per element. Changes: `EmitCtx` gains `enums: Map[String, Int]` (threaded through ~20 sites), `build_enum_registry` walks TIR types once per program, `tir_ty_to_llvm` extended (Named → i8, List → {i64, ptr}), new `emit_for_list` helper for list iteration, `emit_expr` ListLit case for literals, registry lookup in Var for enum construction, `emit_match` detects user-variant Ident patterns via registry key lookup and dispatches on i8 tag (distinct from binding fallbacks). `range(lo, hi)` preserved as special-case counter-only loop (avoids intermediate list alloc). 4 new spike tests: `enum_unit` (3-variant Color, match dispatch), `enum_direction` (4-variant with wildcard), `list_sum` (iterate [1..5], sum), `list_filter` (list iter + if-in-body). Full corpus 35/35 passing. `make test-mvl` clean (98 tests). Emitter grew 1351 → 1662 LOC (+311).

## [0.234.0] - 2026-07-04

### Added

- **`rust_backend`: MVL-hosted LLVM for-in-range loops** (#1118, Phase A1j) — Adds `for i in range(lo, hi) { body }` lowering. Range is inclusive-lower, exclusive-upper. Lowered to a while-loop shape with an alloca'd counter that reuses the ref-local infrastructure from A1d: loop var is alloca'd, body reads auto-emit `load`, increment stores back. New `"For"` case in `emit_stmt` detects `iter` shape (FnCall named `range` with 2 args), emits alloca+init-store, binds loop var as a ref-local in body ctx, and produces the head/body/exit block structure with `slt` comparison. Pivoted from A1j-guards (checker-only `RefExpr`, not in TIR runtime dispatch). 2 new spike tests (`for_sum` = `sum_range(1, 10)` → 45, `for_count` = count odd i in [0, 10) → 5). Full corpus 31/31 passing. `make test-mvl` clean (98 tests). Emitter grew 1230 → 1351 LOC (+121).

## [0.233.0] - 2026-07-04

### Added

- **`rust_backend`: MVL-hosted LLVM fn returning Option/Result** (#1118, Phase A1i) — Fns can now RETURN Option[Int] and Result[Int, Int], not just accept them as parameters. Completes the enum round-trip started in A1g/A1h. Changes: `emit_fn_def` uses `tir_ty_to_llvm(f.ret_ty)` for `define <ty> @` and `ret <ty>` (was hardcoded `i64`); `emit_fn_call` takes a `ret_ty` param derived from FnCall's TIR ty (was hardcoded `i64`); `StmtOut` gains `tail_ty` field, `MatchArmOut` gains `ty` field — both propagate LLVM type up so `emit_if_blocks` and `emit_match` emit `phi {i8, i64}` instead of `phi i64` when merging aggregate results. 2 new spike tests (`fn_ret_option` = fn returns Option; caller matches, `fn_ret_result` = fn returns Result; caller matches). Full corpus 29/29 passing. `make test-mvl` clean (98 tests). Emitter grew 1183 → 1230 LOC (+47).

## [0.232.0] - 2026-07-04

### Added

- **`rust_backend`: MVL-hosted LLVM Result[Int, Int] match + Ok/Err constructors** (#1118, Phase A1h) — Result[Int, Int] works end-to-end using the same `{i8, i64}` tagged-struct lowering as Option (tag 0 = Ok, 1 = Err). Extends `tir_ty_to_llvm` (Result → {i8, i64}), `emit_fn_call` (Ok/Err ctors alongside Some/None), and `emit_match` (Ok/Err variant tags). 2 new spike tests (`match_result_ok` = Ok payload binding, `match_result_err` = Err arm + binding). Full corpus 27/27 passing. `make test-mvl` clean (98 tests). Emitter grew 1152 → 1183 LOC (+31).

## [0.231.0] - 2026-07-04

### Added

- **`rust_backend`: MVL-hosted LLVM Option[Int] match + Some/None constructors** (#1118, Phase A1g) — Option[Int] now works end-to-end: lowered to `{i8, i64}` tagged struct (tag byte + payload word). `Some(x)` and `None` construct via `insertvalue` (tag-set + payload-set, or tag-set-only). Match dispatch extracts tag once, branches per arm, extracts payload for `Some(name)` pattern and binds in arm-locals. Enables fn params/args of type Option[Int]; arms can read outer scope + payload simultaneously. Changes: `tir_ty_to_llvm` (TIR ty → LLVM ty), `is_aggregate_ty` (enum discriminator), revised `emit_fn_call` (per-arg type tracking), revised `emit_expr` Var branch (bare `None` as FnCall-less constructor), revised `emit_match` (tag extraction, variant-tag dispatch, payload `extractvalue`). 3 new spike tests (`match_option_some` = payload binding, `match_option_none` = None arm, `match_option_flow` = Option + outer param). Full corpus 25/25 passing. `make test-mvl` clean (98 tests). Emitter grew 1004 → 1152 LOC (+148).

## [0.230.0] - 2026-07-04

### Added

- **`rust_backend`: MVL-hosted LLVM match arm-locals + Ident bindings** (#1118, Phase A1f) — Completes A1e: arm bodies now see the enclosing fn's `locals` (params + outer let-bindings) AND Ident patterns bind the scrutinee value in the arm's scope. Root cause of A1e limitation was the Rust transpiler's move analysis rejecting cross-iteration `ctx.locals` reuse. Workaround (spike-verified on 2026-07-04): capture locals in a `ref` variable BEFORE the loop, then call `clone_locals(ref_var)` per iteration — transpiler emits `alloca`-load + `.clone()`, giving each arm a fresh Map without consuming the original. 2 new spike tests (`match_bind` = Ident pattern binding, `match_scope` = reading fn params in arm exprs) demonstrate the fix. Full corpus 22/22 passing. `make test-mvl` clean (98 tests). Emitter 1004 LOC (net +29 from A1e: `clone_locals` helper + revised `emit_match` logic).

## [0.229.0] - 2026-07-04

### Added

- **`rust_backend`: MVL-hosted LLVM match on Int/Bool** (#1118, Phase A1e) — Emitter extends to support `match` scrutinee on Int/Bool types. Lowers to a linear chain of icmp+br tests followed by a phi merge at join block. Supports Literal(Integer|Bool), Wildcard, and Ident patterns (Ident currently treated as Wildcard due to transpiler cross-iteration move limitation). Emits `unreachable` terminator after final `match_next_N` block when no fallback arm seen (e.g. exhaustive Bool match on `true`/`false`). Known limitation: arm bodies see empty locals — can't reference fn params or outer lets. Passes A1e corpus (all arm bodies are literals). Full fix (arm-scope locals + Ident bindings) lands in A1f via `clone_locals` workaround. 3 new spike tests (`match_int`, `match_bool`, `match_expr`) all green. Full corpus 20/20 passing. Emitter grew 789 → 975 LOC.

## [0.228.0] - 2026-07-04

### Added

- **`rust_backend`: MVL-hosted LLVM while loops + ref-mutation + else-if chains + unary** (#1118, Phase A1d) — Emitter extends to support iterative programs. Ref-mutable locals (`let x: ref Int = 0;`) lower to `alloca` + `store` at declaration; subsequent reads auto-emit `load`. `Assign` statements write via `store`. While-loops emit the classic 3-block shape (`while_head_N` / `while_body_N` / `while_exit_N`) with back-edge; `decreases` clause (termination proof) is checker-only. Else-if chains work via synthetic block-wrapping of nested ifs. Unary `Neg` → `sub i64 0, <val>`; `Not` → `xor i1 <val>, 1`. Critical fix: `last_label` threading ensures phi predecessors are correct when if-arms contain nested control flow (else-if chains, while loops). 5 new spike tests (`ref_mut`, `while_counter`, `factorial`, `while_if`, `else_if`) all green. Full corpus 17/17 passing. Emitter grew 512 → 789 LOC.

## [0.227.0] - 2026-07-04

### Added

- **`rust_backend`: MVL-hosted LLVM if/else + comparisons + Bool** (#1118, Phase A1c) — Emitter extends to support all comparison operators (`==`, `!=`, `<`, `<=`, `>`, `>=`), Bool literals (`true`/`false`), and if/else in both expression and statement positions. If-expressions lower to `br i1 %cond` + `phi i64` at the join label; Bool-typed locals correctly thread their i1 type through uses to prevent type mismatches. 5 new spike test cases (`cmp_lt`, `if_expr`, `bool_lit`, `max`, `if_eq`) all green; full corpus 12/12 passing. `Emitted` struct grows `ty` field to track LLVM type (i64|i1); `locals` map becomes `Map[String, LocalRef]` for per-binding type tracking.

## [0.226.0] - 2026-07-04

### Added

- **`rust_backend`: MVL-hosted LLVM function calls + lets** (#1118, Phase A1b) — Rust transpiler now emits multi-function TIR programs with proper call sequences. Handles all call variants: zero-arg, polymorphic multi-arg (Int, Bool, String, List, Map), struct/enum field access, method calls on values. Let binding analysis (scope-aware, span-keyed) eliminates spurious `unused_mut` warnings on read-only `ref` bindings. All 98 self-hosted parser tests pass; MVL-hosted LLVM backend spike (A0 scaffold + A1a arith + A1b calls) fully operational with 7/7 arith corpus tests green.

### Fixed

- **`rust_backend`: suppress `unused_mut` via scope-aware read-only analysis** — MVL `let x: ref T = ...` unconditionally lowered to `let mut x` in Rust, producing 12 `unused_mut` warnings on bindings declared `ref` but never reassigned. New `mut_analysis.rs` pass walks TIR with a scope stack; each `let` registers a binding by its pattern span; `Assign` / method-call receivers flip the innermost matching binding to mutated. Span-keyed (not name-based) correctly handles shadowed bindings across different match arms. Lambda captures are conservatively escalated. Result: **12 warnings → 0** with all tests passing.

## [0.225.2] - 2026-07-03

### Fixed

- **`llvm_text`: resolve dominance violations in TIR walker** (#1645) — Four TIR-walker bugs blocked Phase 3b PR 2 (AST deletion). (1) Missing regex dispatch: `_mvl_regex_find` returns `Option[Match]` where `Match` is a value struct; C-ABI payload is `*mut MvlMatch` (heap ptr), but normalization wrapped it in `alloca ptr` (8 bytes), then match-arm `load %Match` tried to read 24 bytes from an 8-byte alloca (UB/garbage). Fixed: detect value-struct inner types (all primitive LLVM fields, e.g. `Match{String, Int, Int}`), skip the alloca-ptr indirection, pass `*mut T` directly to match arm. Also fixed: `fn_ret_types` short-name overwrite — extension methods now only register their qualified name, preventing `String::find` from clobbering regex `find`. (2) Ref-local dominance: ref-local allocas inside branches didn't dominate all uses; fixed: hoist to entry block (pre_allocas list), inject after "entry:" label in `finish_fn_body()`. (3) Loop-scoped heap drops: drops were function-wide; fixed: snapshot heap_locals before each loop body, drop at back-edge, truncate. (4) Opaque-handle payloads: `Result[Child, ...]` with nested Option fields should use alloca-ptr indirection to keep the opaque ptr, not dereference the struct. Rust runtime actors with `traps_exit` / `on_exit` / `on_down` hooks now generate spawn infrastructure even without public behaviors. Test results: 113/113 cross-backend, 179/179 corpus, 31/31 backend-rust all pass. Linker duplicates removed from Rust runtime (LLVM provides C-ABI wrappers).

## [0.225.1] - 2026-07-01

### Fixed

- **`llvm_text`: port `partition` + set-algebra + `Set::contains` dispatch to TIR walker** (#1612, PR 2 prep) — Three more method dispatches handled by the AST `emit_method_call` that were silently swallowed by TIR's catch-all: `List::partition` (filters list into two, used in sort implementations), `Set` algebra ops (`union`, `intersection`, `difference`), and `Set::contains` (dispatch was present but receiver type matched too broadly, firing for any generic). Each dispatch was missing 2–8 instructions in TIR, producing zero-instruction emissions instead of the AST's C-ABI calls. Fixes parity between walkers for stdlib set operations and list-splitting logic, critical before AST deletion in Phase 3b PR 2.

## [0.225.0] - 2026-07-01

### Changed

- **`llvm_text`: extract shared helpers from AST modules to `c_call.rs` + `emit_helpers.rs`** (#1612, PR 2 prep) — 40+ helper functions (C-ABI dispatch shapes, heap-drop tracking, type mapping, string globals, closure infrastructure, enum lookup, mangling, literal emission, string→numeric parse) move out of the soon-to-be-deleted AST `emit_*.rs` modules into two shared modules above the AST/TIR boundary. Pure relocation with zero behavior change; every helper is moved verbatim and the TIR walker already called each via `self.*` / `Self::*` paths. Sets the stage for the AST emitter deletion in a follow-up PR — every helper TIR depends on now lives outside the AST modules.

### Fixed

- **`llvm_text`: port `Box::new` to TIR walker + guard String::contains dispatch** (#1612) — Two latent TIR-walker bugs surfaced during PR 2 prep. (1) `Box::new(x)` fell through to the user-fn path and emitted invalid `call i64 @Box::new(...)` (LLVM rejects `::` in symbol names); ported the AST inline handler verbatim, supporting primitive payloads and the `{ i8, ptr }` tagged union. (2) The `("contains", "ptr")` arm had no receiver-type guard, so it fired for `List::contains` too, routing through `_mvl_str_contains` and passing the i64 needle as `ptr N` (invalid); added `matches!(unwrap_labels(&receiver.ty), Ty::String)` so List/Array/Set dispatch reaches its own arm. Fixes: `cross_backend_box_field_deref`, `cross_backend_linked_list` (partial for `collections_basic`, `core_types_demo` — surfaces `_mvl_array_contains` runtime symbol gap as follow-up).

## [0.224.1] - 2026-06-30

### Fixed

- **`llvm_text`: port `Map::insert` / `Map::remove` / `String::char_at` / `List::group_by` dispatch to TIR walker** (#1612, PR 2 prep) — Four method dispatches handled by the AST `emit_method_call` were silently swallowed by TIR's `_ => Ok(None)` catch-all, producing zero-instruction emissions instead of the AST's 3–5-instruction `_mvl_*` C-ABI calls. Most visible: `std/json.mvl::parse_object_step::r.insert(key, jv)` emitted 5 instructions through AST but nothing through TIR, shifting SSA register numbering downstream and causing the apparent "drop-ordering" symptom in `tests/corpus/13_stdlib/json_log_imports.mvl`. With the four arms ported, the TIR walker now produces byte-identical IR to AST for every corpus file that uses these methods. De-risks the upcoming AST-walker deletion (Phase 3b PR 2 of #1612): without this fix, deleting AST would have left `r.insert(...)` calls silently compiling to no-ops.

## [0.224.0] - 2026-06-30

### Added

- **`llvm_text`: Phase 3b — TIR-walking LLVM emitter alongside AST** (#1612, part 1 of 2) — Eight new `emit_*_tir.rs` modules under `src/mvl/backends/llvm_text/` parallel to the existing AST modules, consuming `TirProgram` directly so each node carries its fully-resolved `Ty` inline (per ADR-0038). Variant coverage matches AST 1:1 (Let/Assign/Return, If/While/For, Match over Option/Result/payload-enum/unit-enum, Propagate, Relabel, Select, struct/enum-variant construct, Lambda + closures, all method-call dispatches, Spawn + actor method-call). A new `emit_mono_tir.rs` mirrors the AST `MonoQueue` (per the ADR-0050 plan) and emits mangled symbols byte-identical to the AST path. `MVL_LLVM_BACKEND=tir` swaps `build`, `run`, and `test --backend=llvm` over to the TIR walker via a single `compile_ir` dispatcher in `src/cli/llvm_text.rs`; AST remains default until PR 2 of #1612 flips it. A new `tests/corpus_ir_parity.rs` harness walks every corpus `.mvl` file with `fn main(`, lowers it through both walkers in-process, and asserts byte-identical IR (70/70 passing, 4 documented allowlist entries for AST-only bugs that resolve when PR 2 deletes the AST walker). Unblocks #1118 (self-hosting backend port).

## [0.223.4] - 2026-06-28

### Fixed

- **`llvm`: drop branch/loop-local heap allocations before the join** (#1617) — The LLVM emitter pushed every `let s: String = ...` (and List / Map) onto a flat function-wide `heap_locals` list, then dropped the whole list at function-end. For lets inside a loop body or one branch of an if, the SSA was only defined when control passed through that block — when it didn't, `_mvl_string_drop` tried to use an undefined value (SSA dominance violation, lli rejection). Fix: snapshot `heap_locals.len()` before each scope (loop body / if-then / if-else), drop everything pushed since the snapshot at the scope's tail, truncate back to the snapshot length. For if-as-expression, the branch's return SSA is passed as `escape` — that one entry is removed without a drop (the merge phi becomes the new owner). Applied to `emit_for_list`, `emit_for_range`, `emit_while`, `emit_if_phi`, `emit_if_expr`, `emit_if_stmt_chain`, and `emit_if_stmt`. With this fix `use std.actors` finally compiles and runs end-to-end on LLVM — the final blocker after #1604, #1607, #1610, and #1615.

## [0.223.3] - 2026-06-28

### Fixed

- **`llvm`: dedupe actor decl emission across `emit_program` calls** (#1610) — The LLVM actor pass ran once per `emit_program` invocation (once per prelude program plus once for the user program). `actor_decls` is a HashMap that accumulates across calls, but the pass naively re-emitted every entry every time — producing 5× duplicate definitions of std.actors' Supervisor and DeadLetterHandler and fatal lli "invalid redefinition" errors on any program that did `use std.actors`. Fix: track emitted actor names in a new `Module.actor_emitted: HashSet<String>` field; the pass skips names already in the set. User-defined actors are unaffected (they only appear in one program).

## [0.223.2] - 2026-06-28

### Fixed

- **`runtime/llvm`: fire exit cascade before DISC_SHUTDOWN at process exit** (#1602) — The LLVM runtime's `_mvl_actor_join_all` cleared the link/monitor registry BEFORE dispatching DISC_SHUTDOWN to actors, so `process_actor_exit` ran against an empty registry and the `on_exit`/`on_down` handlers wired by #1597 were silently unreachable during normal program termination. Reorder: call `process_actor_exit` for every live actor first (injecting EXIT/DOWN signals into peer mailboxes while the registry is intact), wait for peers to dispatch their handlers, then queue DISC_SHUTDOWN to terminate. Also move `scheduled.store(false)` to AFTER `process_actor_exit` in the dispatch loop's DISC_SHUTDOWN and panic paths so the spin-wait observes the cell as busy through the cascade. Brings LLVM behavior to parity with the Rust runtime.

## [0.223.1] - 2026-06-28

### Fixed

- **`llvm`: heap-allocate struct-typed actor behavior arguments** (#1607) — The actor-message ABI flattens behavior args into a fixed `[8 x i64]` array. Primitives round-trip via integer coercion (ptrtoint/zext), but struct values cannot be coerced to i64 — the dispatch function passed a raw i64 to a function expecting `%Struct`, producing invalid IR ("type 'i64' but expected '%DeadLetter ...'"). Fix: sender heap-allocates the struct via `_mvl_alloc`, packs the pointer as i64; receiver inttoptr-loads-frees on the dispatch side before calling the behavior. Detects both named structs (`%Foo`) and anonymous struct literals (`{...}`, used for Option/Result). Uses the standard `getelementptr null, 1` idiom for sizeof.

## [0.223.0] - 2026-06-28

### Changed

- **`backends`: Phase 3a of TIR-first migration — remove dead AST code, fix import paths** (#1594) — Removed 377 lines of dead AST-walking code from the Rust backend (`compute_last_uses_ast` in `last_use.rs`, and `emit_type_decl`/`emit_struct`/`emit_enum`/`emit_alias`/`is_copy_primitive` in `emit_types.rs` — all dead since the TIR equivalents replaced them). Migrated 4 backend import lines (`BinaryOp`, `Capability`, `TypeExpr`, `RefExpr`, `GenericParam`) from `parser::ast` to their re-exports in `crate::mvl::ir`. Lowered `audit-backend-ast` budget from 18 → 14. Phase 3b (LLVM emitter functional migration, ~7,650 LOC) filed as separate ticket #1612.

## [0.222.5] - 2026-06-28

### Fixed

- **`actors`: cascade quiescence wait restores per-actor ping-pong rounds** (#1601 follow-up) — The previous shutdown fix nulled `_self_ref` on `_Shutdown` so channels could close naturally, but without a quiescence wait the `_Shutdown` poison pill raced past user messages: `actor_pingpong` would print one round and exit instead of all five. Restored cascade quiescence detection using *per-channel* `Arc<ChannelMeta>` counters (incremented on send, decremented after dispatch) — no global hot spot. `mvl_join_actors` polls every live channel until in-flight = 0 before sending `_Shutdown`. Each channel's counter only contends between its own producer/consumer threads, so unrelated actor pairs do not share cache lines.

### Changed

- **`runtime`: bump to v0.197.0** — `MvlSender`/`MvlReceiver`/`MvlWeakSender` carry `Arc<ChannelMeta>` for cascade quiescence accounting. Run `mvl self install` to refresh the on-disk runtime.

## [0.222.4] - 2026-06-27

### Fixed

- **`actors`: simplify shutdown via `_self_ref` nulling instead of `IN_FLIGHT`** (#1601) — The previous shutdown protocol used a global `IN_FLIGHT` atomic counter to detect cascade quiescence before sending `_Shutdown`. This introduced cache-line bouncing on every send/recv and a mandatory drain loop in each actor. Simplified: actors now null `_self_ref` (a strong sender clone) when processing `_Shutdown`, allowing channels to close naturally without global synchronization. Fixes actor_pingpong hang and eliminates the global interpreter lock pattern on messages.

## [0.222.3] - 2026-06-27

### Fixed

- **`llvm`: dispatch ExitSignal/DownSignal to on_exit/on_down handlers** (#1597) — The LLVM backend was correctly injecting `ExitSignal` (disc=-2) and `DownSignal` (disc=-3) into actor mailboxes as part of the link/monitor exit cascade, but then silently discarding them because the runtime filtered out negative discriminants and the dispatch switch only covered user behaviors. Extended the LLVM codegen to emit switch cases for system signals wired to `on_exit(from_id, reason)` and `on_down(from_id, reason, monitor_id)` private methods when defined; updated the runtime to route all non-shutdown messages through dispatch. Brings LLVM to parity with Rust backend on supervisor signal handling.

## [0.222.1] - 2026-06-27

### Fixed

- **`loader`: skip `*_test.mvl` files when loading package sources** (#1586) — `load_pkg_modules` and `load_pkg_modules_tagged` iterated every `.mvl` file under a package's `src/` and `src/internal/` directories, including `*_test.mvl` test helpers. When a package's test file redefined a helper from the production source (with a different signature, as is common in unit tests), both versions were emitted into the generated Rust crate, causing rustc E0428 "defined multiple times" errors. Now match the user-side `mvl_files` behavior and exclude `*_test.mvl` from package loading; package test files are only relevant to their own package's `mvl test` runs.
- **`std.runtime`: rename private `json_str` helper to avoid collision with `pkg.rest`** (#1586) — `std/runtime.mvl` defined `fn json_str` as a module-internal JSON-quoting helper. Because the Rust transpiler concatenates all prelude functions into a single Rust file, the name collided with `pkg.rest.json::json_str` whenever a program imported both `std.runtime` and `pkg.rest`. Renamed the private helper to `rt_json_quote`; no public API change.

## [0.222.0] - 2026-06-27

### Added

- **`llvm`: port audit-trail builtin so `relabel ... audit` emits records on LLVM** (#1554, ADR-0049) — The Rust backend emitted a real call to `mvl_runtime::stdlib::audit::emit_relabel_event(...)` for every relabel marked `audit`, but the LLVM backend ignored the flag and produced no audit records — the one runtime divergence identified by the IFC/refine/audit audit (#1547). New C-ABI wrapper `_mvl_audit_emit_relabel` in `runtime/llvm/src/stdlib/audit.rs` takes five `*const MvlString` args and delegates to the existing runtime emit; `ModuleCtx::audit_relabels` tracks declaration-level `audit` keywords; the LLVM `Expr::Relabel` arm now emits the C-ABI call before the transparent unwrap when either the expression or the declaration is audit-marked. Cross-backend test verifies both backends produce identical `MVL_AUDIT_SINK` JSONL output (modulo timestamp).

## [0.221.7] - 2026-06-27

### Fixed

- **`transpiler`: prefix pkg fn when shadowed by user fn** (#1587) — When a user-defined function shared its name with a function from an imported `pkg.*` module (e.g. user defines `serve(...)` while importing from `pkg.http` which also exports `serve(...)`), both were emitted as `pub fn <name>(...)` at the crate root, producing E0428 (defined multiple times) and binding call sites to the wrong overload. The cross-package dispatch table (`pkg_fn_dispatch`) only namespaced functions when 2+ packages collided (#1475); now it also prefixes the pkg variant whenever a user function shadows it. The call-site lookup keys on (name, ret_ty), so user calls keep the bare name and pkg-internal calls route to the prefixed name.

## [0.221.6] - 2026-06-27

### Fixed

- **`checker`: reject `?` in functions not returning Result/Option** (#1588) — A function whose return type was not `Result` or `Option` could still use `?` propagation inside its body; `mvl check` passed, but the transpiler emitted invalid Rust `fn f(...) -> ()` with `?` inside, which rustc rejected (E0277). Now emit `PropagateInNonResultFn` error to require the enclosing function to return a propagatable type, matching Rust's `FromResidual` rule.

## [0.221.5] - 2026-06-27

### Refactored

- **`checker`: extract per-Expr handlers from `IfcFlowAnalyzer::visit_expr`** (#1565) — The `visit_expr` arm of the IFC flow analyzer was 97 lines with effective nesting depth 8, with the `MethodCall` arm alone occupying ~35 lines. Extracted seven per-variant handler methods (`visit_fn_call_flow`, `visit_method_call_flow`, `visit_if_flow`, `visit_match_flow`, `visit_lambda_flow`, `visit_block_flow`, `visit_select_flow`); top-level `visit_expr` is now a 27-line flat dispatch. Max nesting depth in any helper is now 4. Pure refactor — no behavior changes.

## [0.221.4] - 2026-06-27

### Fixed

- **`audit`: deterministic ordering of supply-chain scan output** (#1564) — `audit::scan_all` iterated `manifest.native` and `manifest.c_native` (both `HashMap`s) in arbitrary order, so `mvl audit --supply-chain` produced findings in a different sequence across runs even for the same input.  Sorting helpers in the new `packages/render.rs` now enforce alphabetical iteration; SBOM emitters were already sorted and remain byte-identical.

### Refactored

- **`packages`: extract sorted-iter helpers to `render.rs`** (#1564) — Three small helpers (`iter_native_sorted`, `iter_c_native_sorted`, `iter_source_files_sorted`) replace four copies of `sort_by_key(|(k, _)| *k)` boilerplate across `audit.rs` and the two SBOM emitters.  Note: AC #2 (dedupe `json_escape` between audit and sbom) was already closed by #1567.  A `DepEntry` struct unifying lock + native + c-native was considered and rejected — audit and SBOM have genuinely different domain needs.

## [0.221.3] - 2026-06-27

### Refactored

- **`parser`: split `lexer.rs` (1511 lines) into focused submodules** (#1563) — The single 654-line `impl<'src> Lexer<'src>` block interleaved cursor primitives, string-literal handling, number parsing, and the dispatch loop. Split into `lexer/{mod,cursor,strings,numbers}.rs` with `pub(super)` on the cross-module method visibility. Public API (Lexer::new, tokenize, next_token, Span, Token, TokenKind, LexError) unchanged.
- **`checker`: split `contracts.rs` (2035 lines) into focused submodules** (#1561) — Two-way split into `contracts/{mod,loop_and_field}.rs`: top-level entry points (`check_contracts`, `check_return_refinements`), requires/ensures checking, and the shared pure predicate helpers stay in `mod.rs`; invariant checking + actor/struct field-refinement checks (which share helpers like `check_standalone_pred`, `apply_effects_to_pred`, `extract_simple_assignments`) live in `loop_and_field.rs`. Both files now under the epic's 1500-line advisory; finer four-way split deferred. Public API unchanged.
- **`packages`: split `manifest.rs` (2228 lines) into focused submodules** (#1562) — After the `packages.rs` god-object split (#1523/#1524), `manifest.rs` was the largest remaining file in `src/mvl/packages/` with the same shape of internal coupling. Split into `manifest/{mod,toml,sections}.rs`: data model + Manifest impl stay in `mod.rs`; hand-rolled TOML lexer/parser primitives in `toml.rs`; six per-section parsers in `sections.rs`. Public API unchanged.

## [0.221.2] - 2026-06-27

### Fixed

- **`transpiler`: emit bare ref param when forwarding to ref param** (#1569) — When passing a `ref T` parameter to another function expecting `ref T`, the transpiler emitted `&mut param` instead of bare `param`. The parameter is already `&mut T` in Rust, and the binding is not declared `mut`, causing E0596. Now emits the bare parameter name so Rust's implicit reborrow handles the forwarding transparently.
- **`sbom`: escape C0 control characters in JSON output** (#1567) — `packages::sbom::json_escape` only handled `"`, `\`, `\n`, `\r`, `\t`. Descriptions or names containing C0 control characters (`\x00`–`\x1F` minus the four named ones) produced invalid JSON. Now routed through the canonical helper which `\u00XX`-encodes all C0 controls plus `U+2028` / `U+2029`.

### Refactored

- **`compiler`: dedupe `json_escape` across the crate** (#1559 / #1567) — four near-identical copies lived in `src/cli.rs`, `src/mvl/passes/complexity.rs`, `src/mvl/packages/audit.rs`, and `src/mvl/packages/sbom.rs` — each with slightly different escape coverage (see Fixed above). Promoted a single canonical implementation to `src/mvl/json_util.rs` and routed all four call sites through it; `cli.rs` keeps a `pub(super) use` re-export so the sibling cli modules continue to work unchanged.
- **`checker`: migrate ifc/refinements walkers to ADR-0048 `Visit` trait** (#1560 / #1567) — `ifc.rs` and `refinements.rs` both hand-rolled exhaustive `Block`/`Stmt`/`Expr` recursion (the trio survived even after #1526 introduced `Visit`). Replaced with `IfcFlowAnalyzer` and `RefinementAnalyzer` structs implementing `Visit`; small scope helpers (`in_branch`, `in_pc`, `with_narrowed`) save/restore cloned env/pc/var_refs at branch points. Adding a new `Expr` or `Stmt` variant now fails to compile in `parser/visit.rs` first, forcing a deliberate decision in every walker. Behaviour byte-identical (174/174 corpus, 1440/1440 lib tests, 38/38 stdlib).

## [0.221.1] - 2026-06-26

### Fixed

- **`packages`: treat unknown licenses as audit violations** (#1536) — `LicenseAudit::has_violations()` flagged only rejected licenses, silently passing packages with no declared license at all. Under the default permissive policy a supply-chain attacker could bypass the gate by shipping a package without an `mvl.toml` license field. Unknown licenses now fail the audit unless policy mode is `any`, which explicitly disables enforcement.

### Hardened

- **`packages`: tighten `validate_tag` to a strict allowlist** (#1538) — `validate_tag` only rejected leading `-` and embedded null bytes; shell metacharacters (`$`, backtick, `;`, `|`, `&`, `>`, whitespace, quotes, backslash) were accepted. Inert today because `git` is invoked via `process::Command` without a shell, but the previous check gave a false sense of security and would fail if a future code path interpolated the tag into a log or generated script without quoting. Tightened to ASCII alphanumerics plus `.`, `-`, `_`, `+`, `/` — a subset of git's ref-name rules that covers every legitimate semver/branch tag.

### Refactored

- **`packages`: extract `load_cached_manifest` helper** (#1537) — After the `packages.rs` split (#1524) the "load `mvl.toml` from a cached package dir" pattern lived in five places, and `cmd_add` depended on `cmd_audit` for what was really a generic cache utility. Added `packages::manifest::load_cached_manifest(name, version) -> Option<Manifest>`; collapsed `cmd_audit::read_package_license` into the helper then deleted it; switched `cmd_sbom`'s inline read+parse to the helper; replaced `loader.rs`'s inline `read_to_string + Manifest::parse` with the existing `Manifest::load`. No behavior change.

## [0.221.0] - 2026-06-26

### Changed

- **`make audit-panics`: split metric into PROD vs TEST counts** (#1549) — The old gate counted every `unreachable!()`/`panic!()` site equally, conflating real compiler crash surface (production code) with test failure messages (`panic!("expected Struct body")` inside `#[cfg(test)]`). Replaced the inline `grep | wc -l` with `tools/audit_panics.py`, a brace-aware classifier that reports two counts against two budgets (`PANIC_BUDGET_PROD=30`, `PANIC_BUDGET_TEST=100`). Initial counts: 22 PROD / 75 TEST.

### Refactored

- **Remove 5 unreachable!()/panic!() sites in production code** (#1549) — Parser: collapsed inner `_ => unreachable!()` dead arms in `parser/ast.rs::expr_to_ref_expr_ext` (binary-op lowering) and `parser/externs.rs::parse_extern_decl` (ABI string consumption). Packages: replaced `LicensePolicyMode::Any => unreachable!()` in `manifest.rs::check` with defensive accept-and-break behaviour (still dominated by the function's early return for `Any`, but resilient if that branch is ever removed). Annotated 5 LLVM dispatch-drift detectors in `backends/llvm_text/emit_method_call.rs` with `// AUDIT: drift detector` markers for future audits.

## [0.220.9] - 2026-06-26

### Fixed

- **`mvl test`: clean stale native-dep build cache before each test run** (#1533) — `mvl test` reuses a per-project temp directory keyed by a content hash. If a previous build partially succeeded (C library compiled but Rust FFI bindings not generated), the stale `debug/build/` persisted and caused subsequent runs to fail with `couldn't read .../out/bindgen.rs`. Fix: remove `<test_target>/debug/build/` before `cargo test`, forcing build scripts to re-run cleanly while keeping compiled `.rlib`/`.rmeta` files intact for fast incremental rebuilds. Closes #1532.

## [0.220.8] - 2026-06-26

### Fixed

- **Prover: support `result.field` projection in `ensures` clauses on struct-returning functions** (#1540) — Postconditions like `ensures result.score == 0` and `ensures result.alive == true` on functions whose body is a struct literal were silently dropped (bool comparisons, which had no `RefExpr` counterpart) or deferred to runtime (int comparisons, which hit Layer 1's `_ => None` fallback for `Expr::Construct`). Added a `RefExpr::Bool` variant so bool-literal predicates survive `expr_to_ref_expr_ext`, and extended `try_trivial` with an `Expr::Construct` arm that resolves `self.field` against the struct literal's init expressions. Violations (e.g. `Game { score: 5 }` against `ensures result.score == 0`) are now reported at compile time.

## [0.220.7] - 2026-06-25

### Fixed

- **Rust transpiler: emit pkg-prefixed names for cross-package function collisions** (#1475) — When two packages exported functions with the same name (e.g. `status_reason` from both `pkg.http` and `pkg.health`), the Rust transpiler's prelude deduplication would drop one and leave the surviving definition with an incorrect type at call sites, causing Rust compilation to fail. Fixed by: adding `pkg_name` tracking to `TirFn`; threading package names from the loader through the build pipeline; building a `pkg_fn_dispatch` table to emit collision-avoiding Rust names (`http__status_reason`, `health__status_reason`); and resolving call sites using the checker-inferred return type. Functions without collisions keep their original names. Unblocks projects that import packages sharing function names (e.g. `crud_api` example).

## [0.220.6] - 2026-06-25

### Fixed

- **`mvl test`: load pkg.* modules imported by sibling library files** (#1521) — `load_pkg_modules` was only seeded with `*_test.mvl` files, so packages imported solely by sibling source files (e.g. `db.mvl` importing `pkg.sqlite` in a project whose `db_test.mvl` does not) failed to reach the test crate. Before #1520 this was masked by recursive `.mvl/pkg/` walks; with that path closed, calls like `execute(...)` / `query_scalar(...)` went unresolved in the generated test `lib.rs`. Added a second `load_pkg_modules` frontier pass seeded with discovered sibling programs (sharing `seen_pkgs` with the test-file pass), and included loaded pkg programs in the `load_mvl_native_stdlib_extras` seed so transitive pure-MVL stdlib imports inside packages (e.g. pkg-trace's `use std.crypto.{uuid_v4}`) also resolve.

## [0.220.5] - 2026-06-24

### Fixed

- **Loader: skip `.mvl/` directory in recursive file scans** (#1520) — `mvl_files` and `mvl_files_all` recursively entered `.mvl/pkg/`, the package install directory (analogous to `node_modules`), treating installed package files as user programs. Since `load_pkg_modules` simultaneously loaded the same files from the XDG cache into `stdlib_prelude`, this caused double-loading and false errors: REQ9 "actor shadows prelude" (pkg actor names appeared in both prelude and user programs) and REQ7 "missing effect" (stale `.mvl/pkg/` copies with old APIs broke effect subsumption). Fix: skip any directory named `.mvl` during recursive walks — packages reach the checker exclusively via `load_pkg_modules` (lockfile-pinned). All examples now pass `mvl check --stdlib=proven`.

## [0.220.4] - 2026-06-24

### Fixed

- **Rust codegen: clone `self.field` in `let` bindings inside actor methods** — `let x: ref T = self.field` in actor methods generated a bare move (`let mut x = self.field`) which Rust rejects (E0507 — cannot move out of `&mut self`). The existing `needs_clone` guard was extended to also fire when the field-access receiver is `self` inside an actor method body. Fixes `Pusher::add_worker` and `Publisher::subscribe` in `pkg.zmq`.

- **Clippy: rewrite `loop { match _ => break }` as `while let` in LLVM emit_types** — Two `loop { match }` patterns in `llvm_text/emit_types.rs` triggered `clippy::while_let_loop` on newer Clippy versions. Rewritten as `while let` loops.  No behaviour change.

## [0.220.3] - 2026-06-24

### Added

- **Transitive dependency resolution in `mvl update`** (#1511) — `mvl update` now resolves the full transitive closure of the dependency tree, not just direct dependencies. After resolving all packages in `mvl.toml`, a BFS phase reads each resolved package's own `mvl.toml` and locks its dependencies recursively. Diamond dependencies are handled naturally via a `queued` set. Documented in ADR-0047 (package management system), which consolidates and supersedes ADR-0012, ADR-0039, and ADR-0046.

## [0.220.2] - 2026-06-24

### Fixed

- **Emit `ActorNameConflict` when user actor shadows prelude** (#1497) — Silent shadowing of prelude actors could allow user programs to replace security-enforcing actors (e.g., IFC boundary supervisors) with no feedback. Now emits `CheckError::ActorNameConflict` at check time instead of silently overwriting the prelude definition. Includes timing-based guard (`prelude_actor_names` populated after prelude collection but before user-program checking) to prevent false positives during pre-pass and prelude re-registration.

## [0.220.1] - 2026-06-24

### Fixed

- **Rust backend: suppress `improper_ctypes` lint on generated extern blocks** (#1509) — MVL extern blocks use `String` in FFI signatures because the MVL runtime ships its own ABI (not C ABI). The `improper_ctypes` lint fired on every generated extern block, producing dozens of identical warnings. Now emit `#[allow(improper_ctypes)]` immediately before each extern block to suppress the noise while preserving real signal.


## [0.220.0] - 2026-06-24

### Added

- **`pub test fn` on actors — synchronous actor state assertions** (#1506) — Actor declarations now support `pub test fn` methods that run synchronously on the actor thread, return non-Unit values, and are stripped in production builds (emitted as `#[cfg(test)]`). Enables synchronous state reads in test contexts while preserving fire-and-forget semantics for regular `pub fn` behaviors. FIFO mailbox ordering guarantees all prior async sends are processed before the `pub test fn` call executes, enabling causal consistency without explicit mailbox flush. Implementation includes parser, AST, TIR, checker, and Rust backend changes; all test methods use `std::sync::mpsc` channels for request-reply over the mailbox.

### Changed

- **Spec 004-testing updated to v0.4.0** — Requirement 6 (effect annotations on test fn) updated to reference the new Req 8; Requirement 7 expanded to document `std/testing.mvl` (now live); new Requirement 8 added with full documentation of `pub test fn` syntax, FIFO guarantee, generated Rust pattern, and test scenarios.

### Testing

- Added transpiler tests: `actor_pub_test_fn_emits_cfg_test_infrastructure`, `actor_pub_test_fn_with_params_emits_fields_in_variant`
- Added checker test: `actor_pub_test_fn_non_unit_return_accepted`
- Added stdlib runtime tests: `pub_test_fn_initial_state_is_zero`, `pub_test_fn_sees_state_after_increments`, `pub_test_fn_sees_state_after_reset`, `pub_test_fn_multiple_reads_are_consistent`

## [0.219.0] - 2026-06-24

### Added

- **`std/testing.mvl` — test assertion helpers** — Five pure MVL functions supplement the three core builtins (`assert`, `assert_eq`, `assert_ne`) with common test patterns: `assert_contains` (String membership), `assert_len[T]` (List length), `assert_empty[T]` (List emptiness), `assert_some[T]` (Option is Some), `assert_none[T]` (Option is None). All are `total fn`, require explicit import, and delegate to existing primitives.

## [0.218.1] - 2026-06-24

### Fixed

- **Actor method coverage branches attributed to last prelude function name** (#1501) — `emit_actor_decl` emitted method bodies via `emit_block_stmts` without updating `self.current_fn`, so all coverage probes inside actor methods inherited the function name from the last non-actor function processed by `emit_fn_decl`. In pkg-metrics, `histogram_observe` match arms appeared as `union 0/2` in coverage reports. Fixed by setting `current_fn = m.name` at the start of each actor method body.

### Changed

- **Spec 004-testing updated with Req 6 (effect annotations on test fn) and Req 7 (std/testing stdlib)** — Documents the `test fn ! Spawn + Send` pattern for testing actors, the two-call technique for covering both None/Some arms, and introduces a placeholder for `std/testing.mvl` helpers.

## [0.218.0] - 2026-06-24

### Added

- **Effects annotations on `test fn` declarations** (#1500) — `test fn` now accepts `! Effect` syntax (e.g., `test fn foo() -> Unit ! Spawn + Send`). Effect annotations are parsed, type-checked, and emitted in doc comments; the test runner must satisfy them. Actors can be spawned and have behaviors called from test fn in the same process. Enables unit testing of actor-backed libraries like `pkg-metrics`.

### Fixed

- **`link`/`unlink` conflict with Rust's built-in `link` attribute** — calls like `link(a, b)` were emitted as bare function names, causing `E0423` in generated Rust code (Rust treats `link` as a reserved keyword in some contexts). Now emitted as `mvl_link`/`mvl_unlink` with explicit `as u64` casts to match the runtime's u64 signature (MVL passes i64).
- **`ExitReason` type shadowing in actor mailbox enums** — when an actor imported `std/actors.mvl`, the compiled `ExitReason` enum shadowed the runtime's `ExitReason = i64` alias, causing `E0308` mismatched-types errors in `register_actor_controls` closures. Now the mailbox uses the fully-qualified `mvl_runtime::actors::ExitReason` to disambiguate.
- **Supervisor's `new_order` missing `ref` annotation** — four instances of `let new_order: List[String] = []` in `std/actors.mvl` were marked immutable in the MVL source, generating non-`mut` Rust variables. Subsequent `.push()` calls failed to compile. Fixed by adding `ref` annotation: `let new_order: ref List[String] = []`.

## [0.217.2] - 2026-06-23

### Fixed

- **`mvl prove`/`mvl assurance` reports `~` for Req 10 when only `ensures`/`requires` contracts are present** (#1498) — `check_contracts` populated `by_layer`, `sites`, and `proof_log` in `RefinementCounts` but never set `fn_total` or `fully_verified_fns`. When `checker` merged contract counts, those fields stayed at 0, causing `RefinementsPass` to always emit "no refined types used in this file". Fixed by tracking `fn_total`/`fully_verified_fns` per-function in `check_contracts` via site-count snapshots, then merging them in `checker`.

## [0.217.1] - 2026-06-23

### Fixed

- **Prelude actors not emitted in test crate codegen** (#1496) — `emit_program_with_mods` collected `prelude_fns` and `prelude_types` from library files loaded as preludes but silently dropped `prelude_actors`. Any `pub actor` defined in a prelude would have its entire Rust infrastructure (State struct, Mailbox enum, handle struct, dispatch fn, `_start_*` fn) dropped from the generated code, causing `E0425`/`E0422` compile errors in entry-TIR functions that referenced the actor type. Fixed by collecting prelude actors, extending the actor runtime preamble guard, and emitting prelude actors before entry-TIR actors. Also fixed misleading comments and added comprehensive regression test.

## [0.217.0] - 2026-06-22

### Added

- **`access_control` example now has 97% branch coverage** — improved from 36% (31/85) to 97% (83/85) by adding 50 unit tests across `audit_test.mvl`, `auth_test.mvl`, `rbac_test.mvl`, and a new `main_test.mvl`. Tests cover all reachable branches in `audit.mvl`, `auth.mvl`, `rbac.mvl`, and the pure helpers in `main.mvl`. The remaining 2 uncovered branches (demo_auth `None`/`Err` paths) require the real Rust runtime backend to exercise.

## [0.216.3] - 2026-06-22

### Fixed

- **`mvl test --coverage` skipped sibling library files** (#1489) — only the test file's own body was getting branch probes, while paired library code (e.g. `json.mvl` next to `json_test.mvl`) was emitted as silent prelude. Coverage reports showed near-zero branches even when library functions had dozens of `if`/`match` arms. The transpiler now routes per-file coverage metadata for prelude entries and instruments each sibling library file exactly once across a test run — paired siblings in their matching test module, unpaired helpers in the first test module's transpile. Entry-point files (`fn main`) that import sibling modules also join the prelude so their helpers appear in the report — and their transitive pure-function dependencies are auto-loaded so the test crate still links. Standalone demos that re-declare project types stay excluded.

## [0.216.2] - 2026-06-22

### Changed

- **`String::from_bytes` now uses Latin-1 semantics, not UTF-8 lossy** (#1487) — bytes 128-255 are preserved as Unicode codepoints of the same numeric value instead of being collapsed to U+FFFD by `String::from_utf8_lossy`. This makes `s.byte_at(i)` a lossless round-trip for every byte 0..=255 and unblocks binary protocols (ZMTP greetings, HTTP bodies, hash digests) that need to carry raw bytes through `String`. **Breaking:** callers that were relying on UTF-8 decoding of `from_bytes` input must now decode externally; the previous behaviour was documented but is no longer reachable.

### Fixed

- **Annotated tag false positives in `mvl update`** (#1476) — `mvl update` was comparing the tag object hash against the commit hash stored in the lock file for annotated tags, producing spurious divergence warnings. The `ls_remote_tag_sha` helper now requests both `refs/tags/{tag}` and `refs/tags/{tag}^{}` patterns so git returns the peeled ref (commit hash) for annotated tags.
- **`mvl test` propagates `[native]` deps and `bridge.rs` from `pkg.*` packages** (#1481) — when a test pulls in source files that transitively depend on a package with `[native]` Cargo deps, those deps are now added to the generated test crate's `Cargo.toml` (mirroring `mvl build`). A package-provided `bridge.rs` is copied into the test crate with `mod bridge;` injected so extern "rust" symbols link.

## [0.216.1] - 2026-06-21

### Fixed

- **Transitive package dependencies not loaded** (#1477) — `check` and `test` commands now use a frontier loop to load all transitive package dependencies, matching the behaviour of `build`. Previously, if package A depended on package B, importing A in a `check` or `test` run would fail to load B's types, causing spurious "type not found" errors.

## [0.216.0] - 2026-06-20

### Added

- **`unused-function` linter rule** (#1373) — flags non-`pub`, non-`main`, non-test functions that are never called within the program. Configurable via `unused_functions = false` in `.mvllintrc`.
- **`silent-result-discard` linter rule** (#1465) — flags `Result` values silently discarded without inspecting the `Err` variant. Detects four patterns: `let _: Result = …`, statement-position calls, `if let Ok` with no else branch, and `.unwrap_or*`/`.ok()` on known-Result-returning calls. Configurable via `silent_result_discard = false`.
- **`relabel-tag-hygiene` linter rule** (#1466) — flags boilerplate audit tags (`"TODO"`, `"FIXME"`, `""`, single-char) and tags reused at 3+ call sites on `relabel trust`/`relabel classify` expressions. New `ifc` rules module. Configurable via `relabel_tag_hygiene = false`.
- **Per-site lint suppression** — any rule can be silenced with `// allow: <rule-id> <reason>` on the immediately preceding line (reason text required).

## [0.215.0] - 2026-06-20

### Added

- **Assurance: quantitative evidence for Req 4/5/6** — the assurance report now shows actual counts alongside violation counters, giving auditors a denominator for the "0 violations" claim:
  - Req 4 (Null elimination): Option type sites, Some/None pattern matches, `?` propagate sites.
  - Req 5 (Error visibility): Result type sites, Ok/Err pattern matches, `?` propagate sites.
  - Req 6 (Ownership): immutable bindings, ref bindings, reassignment statements.
  - Counts also surface in the JSON `verification_activity` block (`option_types`, `result_types`, `some_patterns`, `none_patterns`, `ok_patterns`, `err_patterns`, `propagate_sites`, `assign_sites`).

## [0.214.1] - 2026-06-20

### Added

- **`\u{NNNN}` Unicode escape sequences in string and char literals** (#1468) — the lexer now accepts `\u{NNNN}` (1–6 hex digits, case-insensitive) in regular strings, multiline strings, and char literals. Invalid codepoints (e.g. surrogates, out-of-range values) and missing braces produce a lex error and emit U+FFFD. This unblocks direct string-literal comparisons for non-ASCII expected values in tests.

### Fixed

- **fn-alias `val T` param spurious `.into()` (#1467)** — Calling a function pointer through a named `fn(val T) -> U` alias no longer emits `.into()` at the call site (`d(req.clone().into())`). Adds fn-alias resolution to the #960 HOF cap-propagation so the inner `val/ref` flags are visible through the alias, and treats Named fn-aliases as `Copy` so the alias param itself stays an owned fn pointer (no spurious `&Dispatcher`).
## [0.214.0] - 2026-06-20

### Added

- **`mvl update` hardening: timeouts, flags, cache cross-checks** (#1455–#1461) — comprehensive update to address stale cache references and network hangs:
  - Subprocess timeouts: All git operations (`ls-remote`, `clone`) now enforce timeouts (defaults: 30s, 120s) with `MVL_GIT_TIMEOUT` override and `FetchError::Timeout` on expiry (#1457).
  - Warn-and-continue: `mvl update` now catches per-dependency network failures and emits warnings instead of aborting; exits non-zero only when **every** dependency fails (#1458).
  - CLI flags: Add `--force` (re-clone cached packages), `--offline` (skip network, report cache vs. lock state), `--dry-run` (preview without writing), `--package <name>` (update single dep) to `mvl update` (#1456).
  - Last-checked timestamp: New optional `last_checked: Option<u64>` field in `mvl.lock` records when each package was last validated against the remote. Set by `mvl add` and `mvl update`. Backward-compatible parsing for older lockfiles (#1460).
  - Remote SHA cross-check: `fetch_package_opts(force)` allows forced re-clone on cache hit. New `fetch::ls_remote_tag_sha` helper cross-checks remote commit SHA even on the "up to date" path in `cmd_update`; mismatches warn and suggest `--force` (#1455, #1461).
  - Manifest sync: `mvl update` now rewrites `tag = "vX.Y.Z"` entries in `mvl.toml` in lockstep with `mvl.lock` bumps, stopping the manifest from lagging behind after updates (#1459).

## [0.213.1] - 2026-06-19

### Fixed

- **`into_inner()` / `as_inner()` on IFC label wrapper types** — the Rust backend incorrectly emitted `v.into_inner()` (where `v: Tainted[String]`) as a free function call `into_inner(v)` because labeled receiver types matched the UFCS fallthrough. Fixed by adding an early match arm in `emit_method_call.rs`; the type checker's `infer_method_call` now also resolves these methods to their inner type (`Tainted[T].into_inner()` → `T`). Regression test: `tests/corpus/08_ifc/label_into_inner.mvl`.
- **`tcp_read_request` now reads request body** — the runtime stopped reading at the blank line separator between headers and body, leaving POST/PUT/PATCH bodies empty (`body_json` returned "unexpected end of input"). Fixed by parsing the `Content-Length` header after reading headers and reading exactly that many additional bytes from the socket.

## [0.213.0] - 2026-06-18

### Added

- **`String::hex_char_value` and `String::is_hex_char`** (#1433) — two new pure MVL `total fn` string utilities: `hex_char_value(self) -> Option[Int]` maps a single ASCII hex digit (0–9, a–f, A–F) to its nibble value 0–15, returning `None` for non-hex input; `is_hex_char(self) -> Bool` is the corresponding predicate. `String::is_hex` is updated to delegate to `is_hex_char`, removing the previous lowercase-only restriction.

### Fixed

- **Pure MVL extension methods on builtin types no longer require 4-way sync** (#992) — the type checker now falls back to the `method_table` when static dispatch returns `Unknown` for builtin receiver types (`String`, `List`, `Int`, etc.). The Rust backend auto-detects builtin-receiver UFCS dispatch in the generic fallthrough. New pure MVL stdlib methods (`pub fn String::foo(self)`) need only a single entry in `std/*.mvl`; no changes to `method_types.rs` or `STDLIB_UFCS_METHODS` required. LLVM backend auto-dispatch is not yet implemented.

## [0.212.0] - 2026-06-18

### Added

- **Assurance: `(planned)` marker convention** (#1435) — `tools/assurance.py` now detects an optional `(planned)` annotation after the Implementation backtick on a requirement and excludes such requirements from the Completeness, Coverage, and Assurance metrics. Aspirational requirements (e.g. `007-toolchain` R1, `008-packages` R2–R9) are still listed but no longer distort the dashboard.
- **Grammar: `test fn`, session types, const generics, `Type[T]::method`** (#1436) — `docs/grammar.ebnf` now documents four parser features that lagged the canonical grammar: the `test` prefix on `fn_decl`, the session-type production family (`!T. S`, `?T. S`, `+{…}`, `&{…}`, `end`), the `const N: Int` alternative in `type_params`, and the receiver-type prefix `fn List[T]::flatten(…)` via a new `fn_name` non-terminal.
- **Test coverage backfill** (#1430, #1431, #1432) — 25 requirements across 8 specs gained `Tests:` evidence links. New tests: `tests/error_messages.rs::json_format_emits_structured_object_on_failure` (spec 025 R6 — JSON output mode does not use the source-context renderer) and a new `tests/meta_commands.rs` file with four CLI integration tests for `mvl init` and `mvl sbom --output`/`--help` (spec 024 R6, R7, R9).

### Fixed

- **Spec corpus references** (#1434) — five specs referenced `tests/corpus/` directories that had been renamed (`01_basics` → `01_syntax`, `04_effects` → `07_effects`, `05_ifc`/`06_ifc` → `08_ifc`, `09_concurrency` → `12_actors`, `10_verification` → `15_verification`, etc.). The files exist; only the spec paths were stale. The assurance dashboard now reports 22/22 corpus files present (was 3/22).

### Changed

Closes epic #1437. Final assurance metrics: Completeness 157/157 (100%), Coverage 157/157 (100%), Assurance 157/157 (100%), Corpus 22/22.

## [0.211.2] - 2026-06-18

### Fixed

- **Runtime resolution: XDG-based lookup instead of source-tree path** (#1422) — The `mvl` binary no longer hardcodes the absolute source-tree path at compile time. All `mvl run`, `mvl test`, `mvl fuzz`, `mvl mutate`, `mvl mcdc`, and tokio-target commands now resolve the runtime from `~/.local/share/mvl/runtime/{version}/`. The runtime is downloaded by `mvl self install` as a separate release artifact (`mvl-runtime-{version}.tar.gz`). CI jobs set up symlinks to the source-tree runtime for local development. `MVL_HOME` overrides the XDG base for testing and offline environments.

## [0.211.1] - 2026-06-18

### Fixed

- **Type checker: allow `for` loops in `partial fn`** (#1426) — `for` loops iterate over finite collections and always terminate, so they should be allowed in `partial fn` alongside `while`. Removed the inverted-logic guard that was incorrectly rejecting them.

## [0.211.0] - 2026-06-14

### Added

- **Parser: `Type[K, V]::method()` syntax for typed-receiver static calls** (#1417) — The parser now accepts explicit type parameters on the receiver in static method calls. `Map[String, Int]::new()` is now valid inline (no surrounding `let` annotation required), eliminating the need for type inference to determine map key/value types. Enables removal of sentinel-and-remove helper functions from stdlib that existed solely to work around empty map construction ambiguity.
- **Runtime: `purl` field on `PackageInfo`** (#1423) — `std.runtime.PackageInfo` now carries a `purl: String` field (`pkg:mvl/<name>@<version>`) baked in at compile time from `mvl.lock`. `manifest_to_logfmt` and `manifest_to_json` include PURLs in the package list, enabling direct comparison between startup logs and CycloneDX SBOM output.

### Changed

- **Stdlib: replaced sentinel empty-map helpers with `Map::new()` and `Map[K,V]::new()`** (#1417) — Removed five internal helpers (`empty_config_map`, `empty_str_map`, `empty_object`, `toml_empty_table`, `kv_empty_map`) from `std/config`, `std/http`, `std/json`, `std/toml`, `std/kv/file`. Call sites now use either `Map::new()` (when type inference applies) or `Map[K,V]::new()` (inline contexts), both clearer and more idiomatic.

## [0.210.0] - 2026-06-14

### Added

- **Package supply-chain security controls** (#1414, #1415, #1416)
  - **Lockout period (`min-age-days`)** — Project-level `[security]` table in `mvl.toml` and global `$XDG_CONFIG_HOME/mvl/config.toml` prevent `mvl update` from selecting versions published less than N days ago. Enabled by default (0 = no restriction). Bypassed when explicit version + hash pinned in `mvl.lock`.
  - **Semver range operators** — `^X.Y.Z` and `~X.Y.Z` added to `version.rs`. `^` locks to left-most non-zero digit (allows `1.x.x` changes when major ≥ 1); `~` locks to minor (allows patch-level changes). Complements existing `>=`, `>`, `<=`, `<`, `=` predicates.
  - **Version exclusion lists** — Per-dependency `exclude = ["1.2.3", "1.3.0"]` in `mvl.toml` and global `[exclusions]` table in XDG config block known-bad versions (CVE, broken releases). `mvl update` reports each skipped version with reason.
## [0.209.2] - 2026-06-14

### Fixed

- **Type checker: qualified variant name resolution in nested enum patterns** (#1410) — The `TupleStruct` pattern handler resolved variant field types by scanning all registered types for a matching short name (e.g. `ParseError`). Multiple stdlib types share variant names (`JsonError::ParseError(String)`, `TomlError::ParseError(String)`, `CsvError::ParseError(Int, String)`), and HashMap iteration is non-deterministic, so the wrong enum could be picked first — binding pattern variables to incorrect types and causing spurious `type mismatch` errors on explicit `let` annotations. Fix: when the pattern name is qualified (e.g. `CsvError::ParseError`), look up the named type directly in `env.types` first, mirroring the disambiguation logic already used for identifier resolution.

## [0.209.1] - 2026-06-14

### Fixed

- **Composition root exemption for `complexity-effect-width` lint** (#1408) — Functions reachable from `fn main` within `composition_root_depth` hops (configurable, default 2) are now exempt from the effect-width lint in binary crates. Eliminates false positives on legitimate composition roots like `main`, `serve`, and setup functions that aggregate orthogonal effects from subsystems. Library crates are unaffected.

## [0.209.0] - 2026-06-14

### Added

- **Self-hosted type checker: EffectHierarchy + TypeEnv foundation** (#1404, #1117) — Ports `src/mvl/checker/effects.rs` and `src/mvl/checker/context.rs` (Phase 4a of the self-hosting epic). Includes EffectHierarchy with three-pass construction (register names, validate parents, DFS cycle detection) and BFS subsumption queries; TypeEnv with three lookup tables (scopes, types, fns) and ~35 pre-registered stdlib builtins. Fixes critical bugs in cycle deduplication and shadowed-variable handling. Security audit: declassification transitions (`trust`, `release`, `unaudit_target`) now require audit trail annotation. All 12 compiler files pass type check (9/11 requirements proven); all 98 Rust unit tests and 162 corpus tests passing.

## [0.208.0] - 2026-06-13

### Added

- **Struct-returning list and map methods** (#1383) — `List::enumerate() → List[Indexed[T]]`, `List::zip(other) → List[Pair[T, U]]`, and `Map::entries() → List[Entry[K, V]]` replace anonymous tuple patterns with named struct types (ADR-0002, #1380). Implements the full 4-way sync: stdlib declarations, BUILTINS registry, Rust backend iterator emission, and LLVM Shape A CCall dispatch with C runtime functions.

## [0.207.0] - 2026-06-13

### Added

- **Self-hosted compiler: zero-alloc lexer tokens** (#1372) — Lexer now emits `Token { span: Span { start, end }, loc: SourceLoc }` instead of `Token { lexeme: String }`, eliminating N heap allocations per token (N = token count). The `span` field indexes into the source buffer; parser recovers token text on-demand via `tok.span.text(src)`. Type system update: renamed `tir.Span { line, col }` → `SourceLoc` to avoid collision with the new `std.text.Span` index type. Reduces memory pressure during parsing and enables the self-hosted parser to match the Rust backend's allocation footprint.

## [0.206.0] - 2026-06-13

### Added

- **`mvl sbom snapshot`** (#636) — Saves the current SBOM as a baseline to `.mvl/sbom.baseline.json` (full CycloneDX) and `.mvl/sbom.baseline.meta` (lightweight dep list + timestamp). Enables audit trail preservation in version control.
- **`mvl sbom diff [--baseline] [--format=json]`** (#636) — Compares the current manifest/lock state against a stored baseline, reporting added/removed/updated dependencies and source-file count changes. Computes a time-decaying trust score (default 90-day half-life) for supply-chain freshness assessment. New c-native deps reduce trust by 0.5; native by 0.3; mvl by 0.1. Exits with code 1 on regression > 0.5 points, enabling CI gates. Human-readable output by default; `--format=json` for machine parsing.

## [0.205.0] - 2026-06-13

### Added

- **`mvl prove` caller/callee display** (#836) — Each proof site now shows `caller → callee(param)` instead of just `callee(param)`, making it clear which function contains the call. Layer format changed from `Layer N (name)` to `(N:name)`. All columns (counter, line, caller, callee, verdict) are aligned using char-count widths to handle the multi-byte `→` correctly.
- **`mvl prove --verbose` wrapping** (#836) — In verbose mode, the predicate is fit on the same line when it fits within the terminal width (respects `COLUMNS`); otherwise it wraps to a second indented line. The callee column width is computed from the arrow only, so long predicates no longer inflate padding for every other line.
- **`mvl prove --callee <fn>`** (#1374) — Filter proof sites to a specific callee function. Shows only sites where the named function is called. Prints a clear message when no sites match. Exits with an error if `--callee` is given without an argument.
- **Proof site recording for return-type refinements, loop invariants, and struct/actor field-init checks** (#836) — Previously only call-site parameter checks appeared in `mvl prove` output; return-type refinements (`-> T where ...`), `invariant` checks, and field-init refinements are now included. The summary is counted from sites (not the internal solver counters) so it always matches the printed lines.

### Fixed

- **`std.audit`: `AuditEvent::with_details` and `AuditEvent::fail` extension methods** — Added method-call forms so handlers can write `event.with_details({...})` and `event.fail(reason)` without violating ADR-0031 (no UFCS). The free-function forms `with_details` and `fail(String, String, String, String)` remain for backward compatibility.
- **Rust backend: prelude extension-method shadowing** — A name-based dedup in `emitter.rs` used bare method names to exclude prelude functions that clashed with user-defined names. This caused `AuditEvent::fail` to be silently dropped whenever the free function `fail` existed in scope. The filter now uses qualified keys (`Type::method`) for extension methods on user-defined types, so distinct symbols are no longer conflated.
- **Rust backend: examples removed from repo** — `examples/crud_api` moved to the standalone `mvl-lang/examples` repository.

## [0.204.0] - 2026-06-13

### Added

- **Tuple expression literals** (#1366) — Parser, type checker, and backends now support tuple construction syntax `(e1, e2, ...)` as first-class expressions. Type annotations `(Int, String)` and patterns `(a, b)` already worked; this completes the pipeline. Enables multi-return functions without per-shape struct wrappers, supporting self-hosted checker implementation.

### Fixed

- **Data race: iso aliasing via tuple packing** — Detection now catches `let t = (iso_x, other)` and `t = (iso_x, other)` which create hidden aliases, violating the single-reference isolation invariant.
- **Data race: ref escape via tuple in spawn field** — Detection now catches `Spawn { field: (ref_x, other) }` which allows mutable refs to escape into actor initial state.
- **IFC: tuple match scrutinee** — Tuple-valued match scrutinees now properly raise the program counter label for implicit flow analysis, preventing secret information leakage through observable side effects.
- **Linear binding: shadow-drop detection** — Now correctly identifies references within tuple expressions when checking linear (iso) binding shadows.
- **LLVM backend: tuple type** — Fixed `type_of_expr` to return correct type for tuple expressions instead of falling through to `i64`.
- **Parser: single-element tuple grammar** — Trailing comma `(e,)` is now normalized to grouping syntax to enforce the two-or-more-elements invariant for `Expr::Tuple`.

## [0.203.0] - 2026-06-13

### Added

- **Type checker foundations (C1/C2/C3 phases)** (#1117) — Ported three foundational MVL files for the self-hosted type checker (issue #1117), establishing the data model and enum-dispatch architecture for verification passes:
  - `compiler/verify_types.mvl` (C1) — Full implementations of `Ty` and `SessionTy` extension methods (`display`, `base`, `unlabeled`, `is_*` predicates, `propagate_inner`), `types_compatible()` structural matching for all type variants including the new `Ty::Ptr` arm, and `effects_name_eq()` effect list comparison. Uses OR patterns (#1355) for efficient multi-case matching.
  - `compiler/verify_errors.mvl` (C2) — `CheckError` enum with 80+ variants in named-field struct form, mapping all 11 requirements with tagged variants. Accessor method implementations deferred pending Rust backend fixes for Copy-type field extraction in `&self` context.
  - `compiler/verify_passes.mvl` (C3) — `Verdict` enum (tuple variants), `PassId` enum (11 requirements), `PassEntry` and `AssuranceReport` structs. Establishes enum-dispatch over 11 passes, replacing Rust trait objects (`Box<dyn VerificationPass>`) with explicit MVL enum matching.
  - Discovered three MVL language constraints (orthogonal to #1355-#1359): tuple match scrutinees unsupported, tuple value construction as expressions unsupported (workaround: pass-through in branches), and `for` loops forbidden in `partial fn` (workaround: use `while` with manual indexing).

### Fixed

- **`AuditLogger::emit` ownership semantics** — Updated to `val self: AuditLogger` parameter to reflect correct non-consuming borrow after #1359 self receiver fix. Multi-emit tests were failing with "use of moved value" due to `plain self` now being consuming per ownership semantics. Applies same pattern as `std/log.mvl` `Logger` methods in #1359.

## [0.202.2] - 2026-06-13

### Fixed

- **Extension method self receiver semantics** (#1359) — Fixed Rust backend hardcoding `&self` for all extension methods on user-defined types, ignoring MVL ownership semantics. Now correctly derives receiver from capability analysis: consuming methods without inferred borrow get `self`, read-only methods inferred as `&self`, and explicit `val`/`ref` capabilities control the receiver kind. Added `*self` dereference detection in capability analysis to mark methods like `Box[T]::unwrap` as consuming. Annotated `Logger` methods with `val self: Logger` for correct multi-call semantics.

## [0.202.1] - 2026-06-13

### Fixed

- **Cross-file extension method type conflict** (#1358) — Fixed false `UndefinedType` errors when a prelude file defines an extension method whose receiver type is declared only in the current file under type-check. The root cause was incorrect ordering of declaration passes in `check_with_two_preludes_mode`; now type declarations are pre-registered from all files (prelude + prog) before extension method validation occurs.

## [0.202.0] - 2026-06-12

### Added

- **Struct pattern wildcards** (#1356) — MVL now supports `Foo { x, .. }` syntax in match patterns to ignore remaining fields. Eliminates brittle exhaustive field lists when matching large structs like `TirExpr` and `FnDecl` during self-hosting work. Adds `DotDot` token to lexer, `rest: bool` to `Pattern::Struct` AST node, and emits `..` in the Rust backend. EBNF grammar updated.
## [0.201.0] - 2026-06-12

### Added

- **Named-field enum variant construction** (#1357) — MVL now supports `Enum::Variant { field: value }` syntax for constructing enum variants with named fields. Parser already handled the syntax; enhanced the type checker to infer generic type parameters from provided field values and return the correctly parameterized enum type. LLVM backend tracks struct-variant field names and reorders them to declaration order. Supports both non-generic and generic enum variants.

## [0.200.0] - 2026-06-12

### Added

- **Expression & statement parser in MVL** (#1116) — Phase 3 of the self-hosting epic. Implements recursive-descent expression parser (Pratt binary, if/match, while/for, struct/list literals, method chains, pattern matching), statement parser (let/assign/return/while/for), block parser, and pattern parser. Wired into `parse_fn_decl` replacing `skip_body`. 15 new tests.
- **OR patterns in match arms** (#1355) — MVL now supports `A | B => body` syntax in match expressions. All alternatives bind identically-named variables (standard OR-pattern semantics). Self-hosting checker code can now match multiple error/AST variants in a single arm without repetition.
- **ADR-0045** — Documents Phase 3 technical decisions: `List[T]` as heap indirection for recursive fields, struct literal disambiguation by name case, `AstLiteral` vs `Literal`, and tuple-variant-only constraint.

### Fixed

- `compiler/mono.mvl`: update `substitute_decl` to use `body: fd.body` (renamed from `has_body: Bool` in Phase 3)
## [0.199.0] - 2026-06-12

### Added

- **Resolver in MVL** (#1115) — port the three-pass module resolver (`src/mvl/resolver.rs`) to `compiler/resolver.mvl`. Collects public exports (Pass 1), validates use declarations (Pass 2), and detects circular imports via recursive DFS (Pass 3). Operates entirely at declaration scope, feasible with simplified types.
- **Mono infrastructure in MVL** (#1115) — port monomorphization machinery (`src/mvl/passes/mono.rs`) to `compiler/mono.mvl`. Implements `MonoSubs`/`MonoFn`/`MonoProgram` types, `substitute_type` (restructured to avoid Rust backend move-then-capture issue), `mangle_name`, `ty_to_type_expr` bridge, and entry-point seeding. Transitive call-site discovery deferred to Phase 3 (#1116).

## [0.198.0] - 2026-06-11

### Added

- **TIR Lower in MVL** (#1115) — port the TIR lowering pass (`src/mvl/ir/lower.rs`) to `compiler/tir_lower.mvl`, the first self-hosted pipeline stage. Includes `typeexpr_to_ty`, `substitute_ty`, and declaration lowering (fn, type, extern, impl). Expression bodies deferred to Phase 3 (#1116).
- **Ptr type support** — add missing `Ptr(Box[Ty])` variant to the TIR type system for C FFI pointers

### Fixed

- **Named-field enum variant construction** — fix checker gap where `EnumType::Variant { field: val }` was silently rejected; now properly type-checked against variant field declarations

## [0.197.1] - 2026-06-10

### Fixed

- **LLVM type substitution** (#1333) — `substitute_type` was silently dropping 5 type variants (`Refined`, `Tainted`, `Secret`, `Actor`, `Infer`) in the LLVM backend; fix clippy PI errors
- **anthropic_chat example** — add missing `mvl.toml`, `mvl.lock`, and `LICENSE` to example package; update `checked_div`/`rem` expectations

### Changed

- **Checker** — extract `walk.rs` and replace 3 AST traversal triples with shared walker (#1338); remove `TirProgram::span_types` round-trip (#1337)
- **Backends** — drop AST twin helpers in `capability_params.rs` (#1335); gate `openapi.rs` behind `self-host` feature flag (#1336)
- **Parser** — split `functions.rs` into `declarations/`, `externs/`, `actors/` modules (#1339); remove dead cli stubs, pre-TIR helpers, and solver façade (#1334)

## [0.197.0] - 2026-06-10

### Added

- **Runtime manifest phases 5-7** (#1244) — complete the 7-phase `std.runtime.manifest()` embedding system:
  - Phase 5: `AssuranceInfo` fields (`extern_count`, `total_functions`, `extern_ratio`, `requirements_proven`) now populated at compile time by counting function declarations and extern blocks
  - Phase 6: `licenses` list populated from `mvl.lock` package metadata — deduplicated and sorted SPDX license identifiers
  - Phase 7: `log_manifest() -> Unit ! Log` stdlib function for startup logging of full build provenance to the default logger
  - All assurance metrics previously hardcoded to zero/empty are now real compile-time values

## [0.196.1] - 2026-06-10

### Fixed

- **Runtime naming and versioning** — rename `mvl_runtime` → `mvl_runtime_rust` and `mvl_runtime_c` → `mvl_runtime_llvm`; align all runtime versions to `0.196.0`; update CI, Makefile, and all generated Cargo.toml code to use new names (#1330)
- **make test-mvl** — fix `String::char_at()` calls in `compiler/lexer.mvl` to handle `Option[String]` return type; fix `len(curr.tokens)` → `curr.tokens.len()` in `compiler/parser.mvl`
- **LLVM numeric methods** — implement `Int`/`Float` methods (`abs`, `min`, `max`, `clamp`, `pow`, `ceil`, `floor`, `round`, `sqrt`, etc.) via LLVM intrinsics; fix `type_of_expr` so chained `.to_string()` calls work correctly (fixes #1252)

## [0.196.0] - 2026-06-10

### Added

- **Dependency rationale enforcement** (#637) — require audit justification for external dependencies to enforce conscious dependency decisions:
  - `[dependency-policy]` manifest section with `complexity-threshold` and `rationale-required` flags
  - `rationale` field on each dependency in `[dependencies]` section
  - `audit_dep_rationale()` API validates all dependencies have rationale
  - Applied to all examples with external packages: `actor_webserver`, `sqlite_basic`, `zmq_hello`, `crud_api`

- **License validation** (#635 extension) — validate SPDX license ID in mvl.toml matches LICENSE file content
  - `validate_license()` API checks LICENSE file exists and matches declared SPDX id
  - Applied to all examples with external packages

- **SBOM application type detection** — distinguish libraries from applications in generated SBOM/CycloneDX output
  - Scans for `fn main()` in package root to classify as application vs library
  - Applied to examples with entry points

- **Refined type alias coercion at call sites** — improved Port type handling in zmq_hello example
  - Demonstrated L1, L4, L5 refinement solver proofs in server_pull.mvl

- **Syntax highlighting fixes**
  - Tree-sitter: wrap `string_literal` and `raw_string_literal` in `token()` to prevent `//` inside strings being parsed as line comments
  - TextMate: reorder patterns to check strings before comments
  - nvim-mvl: remove invalid `"transparent" @keyword.modifier` node type reference

## [0.195.2] - 2026-06-10

### Fixed

- **Refined alias From trait implementation** (#1328) — generate `impl From<Port> for i64` alongside refined alias struct so `.into()` correctly unwraps the newtype at all call sites (method calls, stdlib functions, etc.):
  - Previously, `port.into()` failed with "trait `From<Port>` not implemented for `i64`"
  - Now generates automatic unwrapping via `From` impl to enable transparent coercion at argument emission

## [0.195.1] - 2026-06-10

### Fixed

- **Rust backend newtype coercion for refined type aliases** (#1326) — emit correct `Type::new(expr)` wrapping and `.0` unwrapping when coercing between refined type aliases and their base types:
  - Let bindings: `let port: Port = 5558` now emits `Port::new(5558)`
  - Function call arguments: automatic wrapping/unwrapping at call sites
  - Return expressions: correct wrapping in return context
  - `as` cast expressions: type-aware coercion in cast operations
  - Maintains distinct refined alias types in Rust-generated code while handling seamless MVL-level conversions

## [0.195.0] - 2026-06-10

### Added

- **Checked coercion and explicit `as` cast for refined type aliases** (#1324) — enable seamless type-safe migration between refined type aliases and their base types:
  - Automatic coercion when compiler proves refinement statically (e.g., `let port: Port = 5558` where `type Port = Int where self >= 1 && self <= 65535`)
  - Explicit `n as Port` cast with runtime check when refinement cannot be proven statically
  - Bidirectional support: works for both `Int → Port` and `Port → Int` conversions
  - Updated EBNF grammar, tree-sitter grammar, and self-hosted MVL compiler to track `as` keyword

## [0.194.0] - 2026-06-09

### Added

- **License policy enforcement at resolve time** (#635) — build license checking into the MVL package resolver to reject incompatible licenses at `mvl add` time:
  - `LicensePolicy` type with modes: `permissive`, `copyleft-ok`, `any`, `custom` (default: `permissive`)
  - `[license-policy]` manifest section with `allow` and `deny` lists for fine-grained control
  - SPDX OR expression handling — if any alternative in a dual-licensed package is compatible, the whole expression passes
  - `[c-native]` inline table syntax: `{ version = "...", license = "..." }` for declaring C dependency licenses
  - `--allow-license "reason"` flag on `mvl add` to override policy rejections with audit trail stored in `mvl.lock`
  - `mvl audit --license` command to scan all dependencies against project policy, warn on unknown licenses, fail on rejected
  - `LicenseAudit` report with `Compatible`/`Rejected`/`Overridden`/`Unknown` statuses per dependency
  - TOML parser extended to support string arrays for `allow`/`deny` lists in `[license-policy]`

### Fixed

- **pbt_operations test** — fixed `fn_bytes_len_nonneg` signature to use `List[Byte]` instead of `List[Int]` to match `fuzz_check_bytes` callback contract

## [0.193.1] - 2026-06-09

### Fixed

- **LLVM backend struct field types** (#1320) — resolve enum and nested struct field types correctly in LLVM IR struct type definitions and function call arguments. Fixes 3 example test failures: `access_control`, `flight_clearance`, `medical_triage`.

## [0.193.0] - 2026-06-09

### Added

- **Dependency Paradox policy layer** (#637) — make dependency decisions explicit and auditable:
  - `DepSpec::Git` now carries optional `rationale` field for justifying dependencies
  - `[dependency-policy]` manifest section with configurable `complexity-threshold` (default 1000 LOC) and `rationale-required` (default true)
  - `mvl add --rationale "..."` flag to attach justification when adding dependencies
  - `mvl audit --paradox` command that counts source LOC per cached dependency, flags deps below threshold without rationale, and exits 1 as CI gate
  - TOML parser support for boolean and integer literals in inline tables

## [0.192.0] - 2026-06-09

### Changed

- **BREAKING: `String::char_at` returns `Option[String]`** instead of sentinel `""` — callers must handle `None` (#1263)
- **BREAKING: `String::byte_at` returns `Option[Byte]`** instead of sentinel `from_int(0)` — callers must handle `None` (#1263)
- **BREAKING: `random.bytes()` returns `List[Byte]`** instead of `List[Int]` — fuzz callbacks updated (#1266)
- **`String::to_upper` / `to_lower` promoted to builtins** with runtime backing, removed from UFCS transpile path (#1263)

### Added

- **`float_checked_to_int` builtin** — safe Float→Int conversion returning `Option[Int]`, with NaN/Infinity/range checks (#1264)
- **Refinement constraints on `int_pow`, `int_shift_left`, `int_shift_right`** — `exp`/`amount` params require `self >= 0` (#1261)
- **Refinement constraints on `List::windows` and `List::chunks`** — size param requires `self >= 1` (#1262)
- **Checked division and remainder** in Rust backend — `BinaryOp::Div` and `BinaryOp::Rem` now use checked arithmetic (#1265)
- **`random.int()` overflow fix** — uses `i128` arithmetic to prevent panic on full-range `[Int.min, Int.max]` (#1267)

### Removed

- **`pkg/zmq/` local package** — removed in-repo copy; relies solely on external `pkg-zmq` registry package (#1268)

### Fixed

- **82 call-site migrations** across std/, tests/, and examples/ for `char_at`/`byte_at` Option API (#1263)
- **`pkg-zmq` updated to v0.2.0** in lock file to match new `byte_at` API (#1268)

## [0.191.0] - 2026-06-09

### Added

- **Rust transpiler expect-test runner**: `mvl test --expect` discovers `.mvl` files with `// expect:` annotations and runs them through the Rust backend (build → run → assert output), mirroring the LLVM backend's expect-test infrastructure (#1247)
- **Backend test parity**: Updated `make test-backend-rust` to run expect-annotated corpus/intrinsics/stdlib tests alongside compile_and_run tests, now matching `make test-backend-llvm`'s test directory structure
- **Corpus annotations**: Added `rust-expect-skip:` annotation type for known Rust transpiler limitations (e.g. closure capture → fn pointer; #1313)

## [0.190.1] - 2026-06-09

### Fixed

- **Rust backend**: skip type stub generation for types imported from sibling modules in multi-file programs (#1311)
- **Rust backend**: properly handle both dot-path imports (`use game::Direction`) and brace-group imports (`use models::{User, Req}`)
- **MVL examples**: corrected `val` annotation position from type-position to capability-position in function parameters across access_control, bzip, and crud_api examples
- **Test infrastructure**: removed example tests from compile_and_run.rs (covered by make test-examples) and split example testing by backend (Rust vs LLVM)

## [0.190.0] - 2026-06-09

### Added

- **LLVM backend**: full dispatch arms for five Category-D list builtins — `sort`, `partition`, `group_by`, `windows`, `chunks` (#1290, ADR-0041 Phase 1)
- **C-ABI runtime**: `_mvl_list_sort`, `_mvl_list_partition`, `_mvl_list_group_by`, `_mvl_list_windows`, `_mvl_list_chunks` implementations
- **LLVM backend**: `Pattern::Tuple` let-binding destructuring for partition results
- **LLVM backend**: `type_of_block_tail` helper for correct lambda return-type inference from block/if/match tails

### Fixed

- **LLVM backend**: `Map::get` now supports integer keys (stack-allocated) and returns `{ i8, ptr }` Option struct compatible with `unwrap_or`
- **LLVM backend**: `Map::contains_key` now supports integer keys (same key_ty branching as `get`)
- **std/lists.mvl**: `windows` and `chunks` promoted from `pub fn` to `pub builtin fn` to avoid LLVM SSA-dominance issues (#992)

## [0.189.3] - 2026-06-09

### Fixed

- **std/lists.mvl**: corrected stale comment on `List::sort` that incorrectly claimed MVL lacks `PartialOrd` where-bounds; actual reason is LLVM SSA-dominance (#992, ADR-0041 Phase 2) (#1309)

## [0.189.2] - 2026-06-06

### Fixed

- **C-ABI boundary u64/i64 mismatch** (#1292)
  - `_mvl_array_len`: return type `u64` → `i64` (cast at C-ABI boundary)
  - `_mvl_array_get`: parameter type `usize` → `i64` (with negative index guard)
  - `_mvl_string_len`: return type `u64` → `i64` (cast at C-ABI boundary)
  - Fixed double-underscore naming bugs in `crypto.rs` (`__mvl_*` → `_mvl_*`)
  - Removed dead uuid C-ABI wrappers (now pure MVL implementations)
  - MVL's `Int` type is `i64`, so C-ABI boundary now matches the language type system

## [0.189.1] - 2026-06-05

### Fixed

- **Stdlib cache: detect stale extracted files** (#1294)
  - `needs_extraction()` now compares on-disk file content against embedded copy
  - Catches stale cache from other branches or manual edits that previously went undetected when version stamp matched
  - Regression test: `modified_file_triggers_reextraction_despite_valid_stamp`

## [0.189.0] - 2026-06-05

### Added

- **`uuid_v4()` and `uuid_from_bytes()` in `std/crypto`** (#1279)
  - `uuid_v4() -> String ! CryptoRandom` — generates random UUID v4 (RFC 4122) using the OS CSPRNG
  - `uuid_from_bytes(bytes: List[Int]) -> String` — formats 16 bytes as a UUID string (pure, deterministic)
  - Both set version 4 bits and RFC 4122 variant bits
  - Implemented as runtime builtins with Rust and C-ABI (LLVM) backends
  - 9 Rust unit tests, 4 LLVM unit tests, 7 MVL stdlib tests, 1 corpus test

## [0.188.2] - 2026-06-05

### Fixed

- **`min()`, `max()`, `join()` transpiler intercepts** (#1222)
  - Restored emitter intercepts: `min()` → `min_by(partial_cmp)`, `max()` → `max_by(partial_cmp)`, `join()` → `slice::join`
  - Added MVL fallback implementation for `join()` in LLVM backend
  - Validation: all 1175 unit tests, 18 examples, 51 cross-backend tests passing

- **Stale issue references in `std/collections.mvl`** (#1222)
  - Removed 3 references to closed issue #436 from stdlib comments

## [0.188.1] - 2026-06-05

### Fixed

- **Test runner cross-module imports** (#96)
  - Sibling pure-function modules (no types/extern blocks) now loaded into prelude when explicitly imported via `use` declarations
  - All test-file transpile configs now call `.for_test_crate()` to properly suppress `use crate::X` imports in test crates
  - Validation: `examples/bzip/imports_test.mvl` uses clean imports without inline re-declarations

## [0.188.0] - 2026-06-05

### Added

- **`source_digest` field in `std.runtime.Manifest`** (#1246)
  - `Manifest.source_digest: String` — SHA-256 tree digest of all `.mvl` source files, computed at compile time
  - `manifest_to_logfmt`, `manifest_to_json`, `manifest_to_block` updated to include `source_digest`
  - `load_and_generate()` computes digest from the source file's own project root (not the invoking cwd)
  - Corpus test: `tests/corpus/13_stdlib/runtime_manifest_source_digest.mvl`

### Fixed

- **`app_name`/`app_version` now read from the entry file's `mvl.toml`** (#1246)
  - Added `manifest_root` parameter to `load_and_generate()` separate from `project_root`
  - `project_root` (from cwd) used for package lock resolution; `manifest_root` (from entry file dir) used for app identity
  - Fixes `mvl run examples/crud_api/main.mvl` showing `app=mvl_language` instead of `app=crud_api`

### Changed

- **`crud_api` example startup logging restructured** (#1246)
  - Three focused log lines: `application` (app, version, built), `versions` (mvl, runtime, stdlib), `source` (digest)
  - Followed by `settings` (host, port, log_level, log_format, db_path), optional seeding, then `listening`
  - `examples/crud_api/mvl.toml` bumped to `v0.2.0`

## [0.187.0] - 2026-06-05

### Added

- **Source file hashes in SBOM generation** (#185)
  - Extracted pure-Rust FIPS 180-4 SHA-256 into shared `packages/hash.rs` module; zero new Cargo dependencies
  - `SourceFile { rel_path, digest }` struct for including `.mvl` files in SBOMs
  - CycloneDX: source files emitted as `type=file` components with `SHA-256` hash entries
  - SPDX 2.3: source files emitted as `FileName/FileChecksum/CONTAINS` entries
  - `cmd_sbom()` now walks project root, hashes all `.mvl` files with `hash::sha256_file()`, passes list to `generate()`
  - 7 new unit tests in sbom.rs; 6 NIST-vector tests in hash.rs; 1212 total tests passing

## [0.186.0] - 2026-06-05

### Added

- **Error message exposure pattern with IFC enforcement** (#823)
  - New `user_message()` and `debug_message()` extension methods on all stdlib error types
  - `user_message()` returns a safe generic string for end users (e.g., "resource not found")
  - `debug_message()` returns a `Secret[String]` with full diagnostic details, enforced at compile time via IFC
  - Pattern documented in `.openspec/patterns/003-error-exposure.md` and applied to all 10 error types
  - IFC corpus test: `tests/corpus/08_ifc/error_exposure.mvl` validates label transitions

- **Per-package LLVM backend convention** (#811)
  - New ADR-0042: `llvm.rs` + `extern "c"` ABI for opt-in LLVM support in packages
  - LLVM emitter now handles `Decl::Extern` with `extern "c"` ABI, emitting LLVM `declare` instructions
  - CLI discovers `llvm.rs` files via `find_pkg_llvm_bridge()` and compiles them to cdylib
  - Package `ffi.mvl` supports dual `extern "rust"` / `extern "c"` blocks; backend selects appropriate path
  - Build flow: discover → compile → emit declarations → execute with `lli --load=libpkg_llvm_bridge.{dylib,so}`
  - Enables packages like `pkg.sqlite` to provide LLVM-compatible implementations without Rust-backend overhead

## [0.185.0] - 2026-06-05

### Added

- **std.runtime Phase 4 — BuildInfo fully populated at compile time** (#1241 #803)
  - `rustc_version`: extracted from `rustc --version`; `None` if rustc unavailable
  - `llvm_version`: extracted from `llvm-config --version`; `None` if llvm-config unavailable
  - `target`: from Cargo `TARGET` env var (e.g. `"aarch64-apple-darwin"`)
  - `profile`: from Cargo `PROFILE` env var (`"debug"` or `"release"`)
  - `date`: UTC timestamp at build time via Hinnant's civil calendar algorithm (no external deps)
  - All fields embedded into `manifest()` override during compilation
  - New corpus test: `runtime_manifest_phase4.mvl` validates all BuildInfo field types

## [0.184.0] - 2026-06-04

### Added

- **LLVM actor scheduler — Phase 2: work-stealing** (#1226)
  - Replace 1-thread-per-actor model with N work-stealing worker threads using `crossbeam-deque`
  - Each actor is now a lightweight `ActorCell` (mailbox + state + scheduling flag) instead of an OS thread
  - Enables ~100K actors with no thread-stack overhead per actor
  - Worker threads use batch-steal pattern: local queue → injector → sibling steal
  - Producer-race-window guard ensures messages are never lost during re-schedule
  - `mvl_yield_check()` now works with the scheduler (reduction budget consumed, work-stealing handles fairness)

### Fixed

- **Work-stealing scheduler safety fixes** (#1227)
  - Fix use-after-free window in `mvl_actor_drop` by holding registry lock through Box::from_raw
  - Add guard against self-links in `mvl_link` (prevents infinite death cascade)
  - Upgrade `handle_ptr` load from Relaxed to Acquire (correct memory ordering)
  - Replace spin-wait `yield_now()` with `sleep(1ms)` in `join_all` (avoid CPU burn)
  - Add explicit negative argc guard in `mvl_actor_send` (buffer safety)
  - Document ExitSignal/DownSignal handling and bounded-mailbox loss limitation

## [0.183.0] - 2026-06-04

### Added

- **LLVM actor scheduler — Phase 1: reduction counting** (#1181)
  - Compiler: Insert `call void @mvl_yield_check()` at loop back-edges in `emit_for_range()` and `emit_while()`
  - Runtime: Add `mvl_yield_check()` C-ABI function with per-thread reduction counter (4000 reductions, Erlang default)
  - Cooperative yield infrastructure in place for Phase 2 work-stealing scheduler

## [0.182.2] - 2026-06-04

### Fixed

- **Stdlib test suite** — fix all test files to pass individually
  - Added missing `use` imports (actors, db, pbt types)
  - Fixed match arm assignments with braces syntax
  - Removed phantom `CheckResult` and invalid `Supervisor` tests
  - Changed Makefile to run test files individually (avoids cross-module name collisions)
  - 34/34 stdlib tests now passing (was 0/34)

- **Grammar coverage** — document known intentional divergences
  - Added `label_ref` to `EBNF_KNOWN_ABSENT` (inlined into relabel_decl as $.identifier)
  - Grammar coverage now passing

- **LLVM backend use-after-free bug** — exclude returned heap locals from drop emission
  - Added `exclude_returned_value()` to prevent premature drop of values being returned
  - Fixed `range_pipeline.mvl` test (expected "5", got garbage "44032"/"58368")
  - Applied to both tail-expression and explicit return statements
  - All 19 LLVM backend tests passing

## [0.182.1] - 2026-06-04

### Fixed

- **List stub methods** (#1214) — replace broken stub MVL bodies with proper implementations
  - `sort`, `partition`, `group_by`: changed to `pub builtin fn` (can't express PartialOrd/tuple construction/Map operations in pure MVL yet)
  - `windows`, `chunks`: replaced recursive stubs with real MVL implementations using while loops + slice builtin
  - Both Rust and LLVM backends now work correctly (previously LLVM would crash on recursive stubs)
  - Fixed pre-existing `group_by` emit bug: inline lambdas weren't parenthesized in both Rust emitters
  - 23 new runtime tests, 1 corpus type-check test

## [0.182.0] - 2026-06-04

### Added

- **Map/Set higher-order functions** (#1213) — add `map_values`, `filter`, `fold`, `any`, `all` to Map and Set collections
  - Map HOFs operate on **values only** (single-arg closures), keeping keys unchanged
  - Set HOFs mirror List patterns exactly, collecting into `HashSet` instead of `Vec`
  - Closes HOF surface gap: `map.keys().filter(...)` and `set.to_list().map(...)` now have direct methods
  - Full 4-way sync: stdlib declarations, checker types, both Rust backends, BUILTINS registry
  - LLVM backend deferred to #436 (existing pattern for new HOF methods)
  - 27 new runtime tests, 1 corpus type-check test

## [0.181.0] - 2026-06-04

### Added

- **IFC audit keyword** (#896) — adds `audit` contextual keyword to `relabel` declarations and expressions
  - Declaration-level: `pub relabel release: Secret -> _ audit` — ALL call sites emit a `RelabelEvent` to the runtime audit trail
  - Expression-level: `relabel trust(input, "XSS-001") audit` — this specific call site emits an event
  - Events written as JSONL to `$MVL_AUDIT_SINK` (file path env var) or stderr if unset
  - Connects compile-time IFC enforcement to the runtime audit trail infrastructure (#808)
  - `RelabelEvent` type in `std.audit` carries: transition name, from/to labels, audit tag, location
  - New runtime module: `runtime/rust/src/stdlib/audit.rs` with `emit_relabel_event` Rust implementation

### Changed

- Checker: relabel map extended to carry `(from, to, audit)` tuple
- Parser: `relabel` declarations now support optional `audit` keyword: `pub relabel X: A -> B audit`
- Parser: `relabel` expressions now support optional `audit` keyword: `relabel X(expr, "tag") audit`
- Assurance report: displays count of audit-marked relabel transitions separately
- EBNF grammar: fixed `relabel_decl` to support `pub` and `_` wildcard sides; added `relabel_expr` to `expr` production

## [0.180.0] - 2026-06-03

### Added
- `std.actors`: dead-letter handling — `DeadLetterReason`, `DeadLetter`, `DeadLetterHandler` actor for capturing and logging undeliverable messages (#1180)

### Changed
- **Rust backend**: enforce TIR as sole backend input boundary (#1195) — backends now accept `TirProgram` instead of raw AST + `HashMap<Span, Ty>`; all monomorphization and type lowering happens before backend entry
- Extend `TirProgram` with all declaration types (functions, types, externs, actors, impls, consts, uses, effects, labels, relabels)
- Dual emitter paths: TIR for user code, AST for prelude; `emit_tir_*` functions parallel existing `emit_*` for each expression/statement kind
- TIR borrow inference (`is_read_only_param_tir`) aligned with AST path: lambda captures, relabel/consume/propagate operands, and sibling module functions now correctly handled

## [0.178.1] - 2026-06-03

### Fixed
- `llvm_text` backend: lower non-unit enum payloads in match arms (#1200) — variants like `Some(v)` now correctly project payload fields in `match` arms instead of emitting unit-typed loads
- `tests/corpus/06_ifc/declassification.mvl`: refresh stale comment that referenced the retired `Public`/`Clean` lattice terminology; updated to reflect the current model where `relabel trust` lowers `Tainted[T]→T` and `relabel release` lowers `Secret[T]→T` (#1201, closes #893)

## [0.178.0] - 2026-06-03

### Added
- `llvm_text` backend: **stdlib C-ABI dispatch parity** (#1202) — wire 11 previously soft-skipped stdlib functions into direct C-ABI dispatch: `sha256`, `sha512`, `crypto_random_bytes`, `format_datetime`, `format_instant`, `find_all`, `replace`, `Float::to_string`, `choice`, `path`; all 11 `run_llvm_text_or_skip` tests migrated to strict `run_llvm_text`
- `_mvl_time_format_datetime` / `_mvl_time_format_instant` C-ABI exports in `runtime/llvm` (MvlString ABI)
- `_mvl_float_to_string` C-ABI export for `Float::to_string()` over the LLVM boundary
- `emit_choice_call` emitter method: SSA-correct `Option[T]` codegen for `choice[T](list)` via `_mvl_random_choice_index`
- `STDLIB_REPLACED_BY_DISPATCH` constant: named list of MVL prelude bodies stripped in favour of C-ABI dispatch to prevent SSA dominance violations

### Fixed
- `emit_propagate` / `emit_result_match` / `emit_result_constructor`: guard `load`/`alloca`/`store` against `void` for `Result[Unit, E]` in `?`, match arms, and `Ok(())` constructor
- `heap_kind`: skip heap tracking for `List[T]` with complex element types (e.g. `List[Match]`) to prevent SSA dominance violations from out-of-scope drops
- `type_of_expr` for `FnCall`: return correct `ptr` type for dispatched functions so `Ok(expr)` wrapping uses correct LLVM type
- stdlib dispatch block in `emit_fn_call` now runs before `generic_fns` check, fixing `choice` being intercepted by generic monomorphization

## [0.177.1] - 2026-06-02

### Fixed
- `llvm_text` backend: **Set algebra dispatch** (#1198) — added emitter dispatch for `Set[Int].intersection`, `.difference`, `.union`; the C-ABI runtime exports already existed but no method-call lowering routed to them. `cross_backend_set_algebra` test migrated from `#[ignore]` to strict parity.
- `llvm_text` backend: review-findings cleanup from PR #1196 — `Box::new` aggregate fallback hardened to a codegen error; slice/take/skip dispatch consolidated via `emit_list_slice_call` / `is_list_array_set` helpers; `run_llvm_text` / `run_llvm_text_or_skip` refactored to share `run_llvm_text_inner` + `strip_progress_lines`; 10 soft-skip parity tests upgraded to strict, 11 annotated with `// TODO(llvm_text): <reason>`.

## [0.177.0] - 2026-06-02

### Added
- `std.actors`: `Supervisor` actor with `OneForOne` restart strategy — monitors children via links and restarts them on failure with configurable `max_restarts` per child (#1128)
- `std.actors`: `RestartStrategy` enum (`OneForOne`, `OneForAll`, `RestForOne`) — `OneForAll`/`RestForOne` declared, not yet implemented (see #1179)
- `actor_id()` accessor on all actor handles — pure sync read of the handle's unique ID, no `Send` effect required
- `link`/`unlink`/`monitor`/`demonitor` upgraded from MVL stub bodies to `builtin fn` declarations backed by a Rust bridge

### Fixed
- Actor handle self-ref construction (`self` as tag argument) now correctly populates `_id` field — previously missing, causing build failures in examples using the self-ref pattern (e.g. `actor_pingpong`)
- `Supervisor.remove_child` now cleans the `live` map (keyed by actor ID) to stay consistent with name-keyed maps
- `Supervisor.on_exit`: budget-exhausted path now removes all tracking for the dead child

## [0.176.0] - 2026-06-02

### Added
- `std.audit`: compliance audit trail module per #808 — `AuditEvent` struct, `AuditOutcome` enum (Success/Failure/Denied), `AuditLogger` for JSONL append-only records, pure constructors (`access`, `modify`, `deny`, `fail`), enrichment helpers (`with_correlation`, `with_details`)
- `Audit` effect (subsumes `FileWrite + Clock`) — distinct from `Log` effect; audit records may contain sensitive data since they ARE the compliance artifact
- Parser support for wildcard relabel syntax: `relabel X -> _` and `relabel _ -> Y` for erasing/restoring labels
- `json_escape` exported from `std.json` for shared JSON serialization across stdlib
- `llvm_text` backend: **Set.contains dispatch** (#1154) — new C-ABI export `_mvl_set_contains_i64` and emitter dispatch for `Set[Int].contains`
- `llvm_text` backend: **Box[T] primitive payload codegen** (#1154) — `Box::new` heap-allocates and stores primitive (i64/ptr/double/i32/i8/i1) payloads; `*box` deref emits typed load via `box_inner_llvm_ty` resolution
- `llvm_text` backend: **List/Array/Set slice/take/skip dispatch** (#1154) — emitter routes to the existing `_mvl_list_slice` runtime
- `tests/cross_backend.rs`: **strict parity infrastructure** (#1154) — `assert_backends_agree` / `assert_parity` now fail on mismatch instead of logging via `eprintln!`; `run_llvm_text` (panic on backend failure) split from `run_llvm_text_or_skip` (legacy soft skip with reason comments)

### Fixed
- `AuditLogger::emit()` now returns `Result[Unit, IoError] ! Audit` instead of silently discarding write errors — callers must handle I/O failures to ensure compliance records aren't lost
- `llvm_text` backend: **String drop double-free** (#1154) — dedupe `heap_locals` SSA tracking when consume/move reuses the source register, preventing underflow abort
- `llvm_text` backend: **Box::new aggregate guard** (#1154) — non-primitive payload types now produce a hard codegen error instead of silently allocating 8 bytes for a wider struct (heap buffer overflow)

## [0.175.1] - 2026-06-02

### Fixed
- Actor thread deadlock: clear link/monitor registry before joining actor threads — Phase 9 link/monitor infrastructure (#1177) held cloned senders that prevented channels from closing, causing `rx.recv()` to block forever in both Rust and LLVM runtimes

## [0.175.0] - 2026-06-02

### Added
- `llvm_text` backend: **Map literal emission** (#1184) — `Expr::Map` emits `mvl_map_new` + `mvl_map_insert` calls; Map method dispatch for get, insert, len, keys, values, contains_key, remove
- `llvm_text` backend: **HeapKind drop tracking** (#1185) — automatic cleanup for String, List, Map locals via `mvl_*_drop` calls at function exit; tracks both immutable bindings and mutable `ref` locals
- `llvm_text` backend: **String builtin kernel methods** (#1186) — 12 new string methods: chars, byte_at, find, split, substring, contains, starts_with, ends_with, trim, to_lower, to_upper, replace

### Fixed
- `llvm_text` backend: **Map::get null guard** — null-check before dereferencing returned pointer; returns 0 on missing key instead of undefined behavior
- `llvm_text` backend: **Double-drop on shadowed locals** — retain-remove old SSA from heap_locals when a binding name is shadowed, preventing double-free
- `llvm_text` backend: **Mutable ref heap tracking** — ref locals now properly tracked for drop; emit load before drop call since ref holds stack alloca, not heap pointer directly
- `llvm_text` backend: **Propagate error path drops** — emit heap drops before `ret` in `?` operator error branch (was previously skipped)
- `llvm_text` backend: **String method receiver guards** — all 9 previously unguarded String method arms now check receiver type to prevent dispatch to List/Map values
- `llvm_text` backend: **Consolidated return heap drops** — hoist single `emit_heap_drops()` call to start of `Stmt::Return`, after expression evaluation but before any `ret` instruction

## [0.174.0] - 2026-06-02

### Added
- `--target=tokio`: actor runtime now uses M:N scheduled tokio tasks instead of OS threads, enabling 1M+ concurrent actors on fixed-size thread pool (#751)
- End-to-end tests for `--target=tokio` actor output parity with default backend

### Fixed
- Tokio actor runtime: safer sender.send() from any calling context (uses `runtime().block_on()` instead of `Handle::current()`); logs failures instead of silent drop
- Mutex poisoning: prefer explicit `.unwrap()` panic over silent recovery in actor handle registry
- Unit tests: direct task joining eliminates parallel-test race conditions on `MVL_ACTOR_HANDLES`
- Pre-existing clippy warnings: `missing_const_for_thread_local`, `suspicious_open_options`, `unused_unit` (#1183)

## [0.173.2] - 2026-06-02

### Fixed
- `llvm_text` backend: address PR #1176 security and correctness review findings: prevent wildcard arm duplication in option match; replace `.unwrap()` panic with proper error; sanitize LLVM IR identifiers to prevent name injection; use consistent PHI type selection; guard merge block terminator; cap monomorphization loop at 10,000 iterations (#1155, #1156)

## [0.173.1] - 2026-06-01

### Fixed
- `llvm_text` backend: save/restore `current_fn_is_main` in nested emit (actors, lambdas) to prevent invalid IR when main's state corrupts nested function generation; extract magic strings and add `wrap_result_pair()` helper for Result wrapping; apply PHI completeness fix to `emit_result_match` (#1169)
- `llvm_text` backend: net_basic.mvl now properly declares `! Console + Net + Spawn + Send` effects (#1169)
- Rust transpiler: use weak sender for actor `_self_ref` to prevent channel hang when `mvl_join_actors()` waits for actor threads; weak ref doesn't keep mailbox channel alive, allowing `rx.recv()` to return `None` when external handles drop (#1169)
- Error message tests: fix REQ tag case sensitivity and println arity (variadic println removed) (#1169)

## [0.173.0] - 2026-06-01

### Added
- `llvm_text` backend: `Result[T,E]` lowering (Ok/Err allocation, `is_ok`/`is_err`/`unwrap`/`unwrap_err`), `parse_int`/`parse_float` builtins, `List::push`, else-if chains, and builtin fn dispatch via C-ABI symbol map (`builtin_syms` field in `TextEmitter`); `collect_llvm_text_builtins` and `derive_builtin_c_symbol` added to `loader.rs` (#1160)
- CI: release-only commits (touching only `Cargo.toml`/`Cargo.lock`/`CHANGELOG.md`) now skip all heavy CI jobs (#1159)

## [0.172.0] - 2026-06-01

### Added
- `llvm_text` backend Phase 3B: actor declaration lowering — state structs, behavior functions, dispatch functions, spawn expressions, actor method calls, and `@mvl_actor_join_all` injection in `main`; implemented in `emit_actors.rs` as a child module of `emitter` (#1149)
- `examples/anthropic_chat`: `assurance` target in `Makefile` (#1167)

## [0.171.1] - 2026-06-01

### Fixed
- `examples/log_to_file`: annotate all five functions with `total fn` keyword to explicitly declare totality. Assurance report now shows 5/5 implemented fns are total (5 explicit, 0 implicit), eliminating the `total*` (inferred) asterisk from the Totality column (#1166)

## [0.171.0] - 2026-06-01

### Added
- Standalone package repositories published for all remaining `pkg/` packages:
  - [`github.com/mvl-lang/pkg-rest`](https://github.com/mvl-lang/pkg-rest) v0.1.0 -- typed REST client (JSON POST/GET over TLS)
  - [`github.com/mvl-lang/pkg-tls`](https://github.com/mvl-lang/pkg-tls) v0.1.0 -- TLS 1.3 client via rustls, HTTPS convenience layer
  - [`github.com/mvl-lang/pkg-tui`](https://github.com/mvl-lang/pkg-tui) v0.1.0 -- terminal UI (raw mode, ANSI styles, keyboard input)
  - [`github.com/mvl-lang/pkg-zmq`](https://github.com/mvl-lang/pkg-zmq) v0.1.0 -- ZeroMQ-style messaging (REQ/REP, PUB/SUB, PUSH/PULL)
- `pkg/` in-repo directory removed; all packages now live in standalone repos under `github.com/mvl-lang/`
- Package resolution now uses XDG cache directly (`$XDG_DATA_HOME/mvl/pkg/`); no project-local `.mvl/pkg/` symlinks needed
- `find_project_root()` walks up from cwd to find `mvl.lock`, enabling `mvl check` from any subdirectory

## [0.170.0] - 2026-06-01

### Added
- Layer 1 refinement solver now statically proves `len(string_literal)` predicates via `eval_pred_str_len` / `eval_bool_str_len` / `eval_num_str_len` helpers; enables `validate_log_path("app.log")` to be proven directly instead of deferred to runtime (#1152)
- `examples/log_to_file/` unit tests (`main_test.mvl`) covering `validate_log_path` and `resolve_path`, demonstrating IFC boundary via `relabel taint/trust` (#1152)

### Changed
- `examples/log_to_file`: bumped from 10/11 → **11/11 requirements proven**; `validate_log_path("app.log")` now statically verified at Layer 1 instead of runtime-checked (#1152)
## [0.169.0] - 2026-05-31

### Added
- `std/log`: file sink — `Logger` now carries an `fd: Fd` field, allowing callers to direct log output to any file descriptor (file, stdout, stderr) instead of always writing to stderr. `default_logger()` defaults to `stderr()` for backward compatibility; `file_logger(fd, format, min_level)` convenience constructor added. New example `examples/log_to_file/` demonstrates file logging (#1152)

## [0.168.0] - 2026-05-31

### Added
- `llvm_text` backend now supports lambda expressions and closures: inline lambdas compile to top-level LLVM functions with environment pointer parameters; captures are collected via AST walk and stored in heap-allocated structs; named functions can be wrapped in closures via generated trampolines (#1148)
- Higher-order function (HOF) support for `llvm_text`: filter, map, find, fold, any, all, take_while, skip_while methods on List types now emit runtime function calls accepting closure pointers (#1148)

### Fixed
- Named-function closures in `llvm_text`: trampoline wrappers now emit properly-typed forwarding calls instead of calling the original function with zero arguments; param types are now stored per-function enabling correct ABI (#1148)
- Capture variable analysis: ref-local (mutable) captures are now correctly identified and loaded before storing into the closure environment struct; statement variants (Assign, Return, While, For, If, Match) and expression variants (List, Map, Set, Borrow, Spawn, Select) are now walked for capture detection (#1148)
- State restore-on-error in lambda emission: saved emitter context is now restored before propagating any error from body expression emission, preventing state corruption on compilation failure (#1148)

## [0.167.1] - 2026-05-31

### Added
- ZMTP protocol test coverage expansion: 19 new tests for `parse_ready_body`, `parse_socket_type_property`, and `zmq_error_msg` achieving 100% branch coverage (80/80 branches) (#1058)

### Fixed
- `mvl build` now fails with an error message when the type checker detects violations (refinement, IFC, type errors, etc.); previously all checker errors were silently discarded and the build succeeded regardless

### Changed
- Rust backend decoupled from checker: `transpile()` and `transpile_project*()` no longer call the checker internally; callers supply pre-built `expr_types` map; new `Pipeline::assemble_expr_types()` centralises prelude + program type assembly (#1110)

## [0.167.0] - 2026-05-31

### Added
- Package extraction: `pkg/anthropic` extracted to standalone repository at [github.com/mvl-lang/pkg-anthropic](https://github.com/mvl-lang/pkg-anthropic) v0.1.0 (#1020)

### Changed
- Model ID enum: `Opus4` → `Opus4_6`, `Sonnet4` → `Sonnet4_6`, `Haiku4` → `Haiku4_5` (#1020)
- API strings: `claude-opus-4-6`, `claude-sonnet-4-6`, `claude-haiku-4-5-20251001` (#1020)
## [0.166.0] - 2026-05-30

### Added
- Actor runtime interface decoupling: Rust backend emitter now calls named symbols (`mvl_channel`, `mvl_spawn`, `mvl_register_actor`, `mvl_join_actors`) instead of inlining `std::thread` and `std::sync::mpsc` glue. Swapping `--target` (Phase 9) will replace the runtime crate without changing emitter output (ADR-0027, #1014)
- `runtime/rust/src/actors.rs`: default `std::thread` + `mpsc` implementation of the actor runtime interface, with `MvlSender<M>`, `MvlReceiver<M>`, and policy-aware message sending
## [0.165.0] - 2026-05-29

### Added
- Actor mailbox configuration: `with mailbox(capacity)`, `with mailbox(capacity, block|drop_newest)`, `with mailbox(unbounded)` syntax on actor declarations (#1127)
- Configurable backpressure policies: `Block` (sender waits) vs `DropNewest` (fire-and-forget, default) for bounded mailboxes (#1127)
- Unbounded mailbox option for audit/compliance actors that must never lose messages (#1127)

## [0.164.0] - 2026-05-29

### Added
- Package distribution infrastructure: `mvl install` now links cached packages to `.mvl/pkg/<short_name>/` for compiler resolution (#1139)
- SBOM license support: `mvl sbom` now reads cached package manifests to populate dependency license fields in CycloneDX and SPDX output (#1139)
- Package manifest parser enhancements: support for TOML arrays and table-format native dependencies (e.g., `rusqlite = { version = "...", features = [...] }`) (#1139)
- End-to-end package distribution example: `examples/crud_api` now uses `mvl add` to depend on `pkg-http` and `pkg-sqlite` as proper git dependencies with version tags (#1139)
## [0.163.0] - 2026-05-29

### Added
- LLVM text emitter Phase 2: string literals, `println`/`assert`/`format` builtins, struct construction and field access, unit enum variants with `match`/`switch`, `for`-range loops, method calls (`to_string`, `len`, `concat`), list literals (#1136)
- Bool comparison in LLVM IR now correctly uses `icmp eq i1` instead of `icmp eq i64` (#1136)

## [0.162.1] - 2026-05-29

### Fixed
- `mvl sbom` now detects application vs library component type by checking for `main.mvl` or `src/main.mvl` in the project root; CycloneDX `type` and SPDX `PrimaryPackagePurpose` reflect the result (#252)

## [0.162.0] - 2026-05-29

### Added
- Text-based LLVM IR backend (`LlvmTextCompiler`) — pure-string IR generation without inkwell/C FFI, Phase 1 supports Int/Float/Bool/Byte/Unit, arithmetic, comparisons, if/else (phi nodes), while loops, and fn declarations/calls (#1111)
- `--backend=llvm` now invokes the text emitter; `--backend=llvm-inkwell` invokes the inkwell backend (#1111)

### Changed
- `mvl init [<name>]` now scaffolds a new project (`mvl.toml` + `src/main.mvl`) in the
  current directory; name defaults to the current directory name when omitted (#1129)
- `mvl self init` replaces `mvl init` for stdlib extraction (#1129)

## [0.161.2] - 2026-05-27

### Fixed
- Actor self-ref shutdown protocol: replaced channel-closure shutdown with AtomicBool
  flags so actors that pass `self` as a `tag` argument no longer panic at runtime (#1087)

## [0.161.1] - 2026-05-27

### Fixed
- #1068 — LinearTypeBareBind check replaced with move semantics per ADR-0029:
  - `let b: T = a` for non-iso linear types is now a valid move (marks `a` unavailable)
  - `consume()` is only required for `iso` capability transfers
  - Bzip example smoke test failures (`bwt.mvl`, `huffman.mvl`, `bitstream.mvl`) fixed
  - LinearShadowDrop false positives eliminated for builder/accumulator patterns

## [0.161.0] - 2026-05-27

### Added
- #1023 — `mvl openapi` subcommand to generate OpenAPI 3.0.3 JSON specs from route tables:
  - Extracts routes from `route()` calls in MVL programs
  - Maps MVL types to JSON Schema: structs → properties, refinements → validation keywords
  - Refinement mapping: `self > 0` → `minimum: 1`, `len(self) > 0` → `minLength: 1`, compound predicates → min/max
  - Effects → OpenAPI tags, IFC labels → x-security-label extension
  - Result[Ok, Err] return types → success + error response schemas
  - Json[T] request bodies and path parameters supported
  - Output is valid OpenAPI 3.0.3 JSON to stdout

## [0.160.0] - 2026-05-27

### Added
- #1065 — Quantified evidence in assurance reports:
  - Refinement proof detail table in verbose mode showing per-proof layer, file:line, callee, and predicate
  - Contract proof counting (ensures/requires) integrated into layer breakdown
  - Implicit totality warning for functions defined without explicit `total`/`partial` keyword
  - Gap surfacing for Req 10/11: shows which refinements and labels are not exercised by internal callers
- `examples/access_control` — Added refinement types and contracts for Req 10 verification:
  - SecurityConfig struct with integer refinements (`max_attempts: Int where self > 0 && self <= 10`)
  - Refined functions: `clamp_attempts`, `next_attempt`, `total_timeout` (L1/L4 proofs)
  - Username type alias with length refinement for Req 11 (L5 Z3 proofs)
  - Explicit `total` keyword on all functions; 15 proven refinements across 8 layers

### Improved
- Assurance verbose output now includes file:line information for each proof, enabling fast navigation to proof sites

## [0.159.0] - 2026-05-27

### Added
- #1067 — Closed 6 Req 10 refinement prover gaps:
  - Gap 1: Struct field `where` refinement violation checking at construction sites
  - Gap 2: Struct `with invariant` violation checking at construction sites
  - Gap 3: Return type refinement checking on explicit returns and tail expressions
  - Gap 4: Let binding initialiser refinement checking against declared type aliases
  - Gap 5: Method call argument refinement checking against parameter predicates
  - Gap 6: Enum variant struct field `where` refinement violation checking at construction sites
- Test suite: 6 new requirement tests validating compile-time violation detection for each gap

## [0.158.1] - 2026-05-27

### Added
- #1059 — `pkg/zmq/tests/zmtp_handshake_integration.mvl` — ZMTP 3.x handshake integration tests (4/4 passing) with actor-based TCP loopback on ephemeral ports. Tests REQ/REP, PUB/SUB, PUSH/PULL socket type detection and full message exchange.
- `pkg/zmq/Makefile` — `make test-integration` target with progress output, dependency on `.mvl/pkg/zmq` symlink, and timeout handling.
- `pkg/zmq/tests/.gitignore` — Exclude `.mvl/` symlink directory.

### Fixed
- #1048 fallout — `tests/stdlib/net_basic.mvl` — Remove `concurrently {}` keyword after #1048 language change. Actor spawn and `tcp_accept` work without explicit concurrency scoping; runtime handles actor draining at process exit.

### Improved
- #1062 — `pkg/zmq/tools/check-sync.sh` — Bash script that detects signature drift between re-declared test functions and their source implementations. Integrated into `make sync-check` and `make assurance`. Catches 19 pub/non-pub function re-declarations; allow-list for intentional variants (e.g. `Tainted` stripping).
- `zmq_test.mvl` — Replaced `decode_frame_str`, `sub_topic_str`, `sub_body_str` variants with real `Tainted`-aware functions from source. Tests now use `relabel taint/trust` at call sites, matching production code. Coverage: 65/65 branches (100%).

### Closed
- #1060 — Mock TcpStream not feasible in MVL (opaque types, no traits, no monadic builders).
- #1061 — Reopened with corrected analysis. Coverage instrumentation works correctly; issue is visibility-driven re-declarations (non-pub helpers must be copied locally).

## [0.158.0] - 2026-05-26

### Changed
- #1048 — Remove `concurrently {}` keyword from the language (ADR-0037). `fn main()` is now
  implicitly a one-shot actor: the Rust backend injects `_mvl_join_actors()` at process exit,
  draining all spawned child actors before the program terminates. No explicit scoping keyword
  is required. Corpus test updated; actor examples (actor_pingpong, actor_trading) migrated.
- `examples/anthropic_chat/Makefile` — Improve `make smoke` to run the binary without an API
  key and verify graceful error output; fix `guard-mvl` to validate binary presence rather than
  rebuilding (matches all other example Makefiles and avoids CI z3-sys path issues).
- `compiler/lexer.mvl`, `compiler/ast.mvl` — Remove `KwConcurrently` from self-hosted bootstrap
  compiler to keep keyword consistency check passing.
- ADR-0037 — Document the main-as-actor design decision.

## [0.157.0] - 2026-05-25

### Added
- #1000 — `pkg/http/src/rest.mvl` — REST response builders, JSON helpers, Router/MatchedRoute types, and dispatch logic.
- #999, #1000 — `examples/crud_api` — Full CRUD REST API over SQLite with layered config (defaults → TOML → env → CLI), CSV seeding, structured logging, and refinement types.
- #1042 — `std/io` — TempFile and TempDir with linear type safety, temp_path/temp_dir_path builtins returning Tainted[String].
- `tests/corpus/05_effects/temp_files.mvl` — Termination proof for TempFile cleanup loop.

### Changed
- `std/json.mvl` — Upgrade encode, json_escape, encode_array, encode_object to total fn with decreases annotations.
- `pkg/http/src/rest.mvl` — Upgrade json_error to total fn.
- `examples/crud_api/main.mvl` — Replace tail-recursive request_loop with while-true serve() loop.

### Fixed
- `std/http.mvl` — Fix shorthand struct patterns (`{msg}` → `{msg: msg}`) that caused silent parse failures and empty transpiled else branches; restructure parse_request to single-match to avoid use-after-move. Fixes 709 stdlib tests.
- `pkg/http/src/http.mvl` — Add std.collections import for Map::new(); fix dispatch to use early return instead of ref Option (linear type); fix body string concat.
- `src/cli/build.rs` — Set binary runtime CWD to source file's parent directory so config.toml resolves.
- `src/mvl/checker/passes.rs` — Restore N/M coverage format in RefinementsPass verdicts.
- `examples/crud_api` — Call db_clear_users before seeding to prevent duplicates on restart; fix total→partial fn for handler/db/config functions.

## [0.156.3] - 2026-05-25

### Fixed
- `examples/zmq_hello`: client now calls `SHUT_WR` after send so `tcp_read` on the server sees EOF
- `examples/zmq_hello`: add Makefile with server startup, polling, client execution, and teardown

## [0.156.2] - 2026-05-25

### Fixed
- CI: skip refinement solver benchmarks on PRs (no baseline to compare against); restrict to push-to-main only
- CI: drop Z3 from example smoke test builds; examples don't exercise the Z3 solver layer
- CI: add benchmark regression tracking via `benchmark-data` branch using `github-action-benchmark`

## [0.156.1] - 2026-05-25

### Fixed
- Transitive package loading: `mvl build` now follows multi-hop pkg dependencies (e.g. main → pkg.anthropic → pkg.tls) via a frontier loop.
- Infinite loop in `load_pkg_modules` when a package's own sources import itself (e.g. `use pkg.http` inside pkg.http files).
- Bridge symlink resolution: `find_pkg_bridge` no longer rejects symlinks that resolve outside `.mvl/pkg/`.
- Static method emission: type-attached functions without `self` (e.g. `Claude::new(key)`) no longer emit `&self`.
- Relabel unwrap codegen: `trust`/`release`/etc. now emit `.0.clone()` to avoid E0507 on shared references.
- Match scrutinee for capability params (`val`/`ref` → `&T` in Rust) now clones to prevent reference binding errors.
- Added `default_endpoint`, `load_endpoint`, `endpoint_connect`, `endpoint_listen` to Rust runtime net module.

## [0.156.0] - 2026-05-25

### Added
- #1020 — `pkg/anthropic` — Typed Anthropic Messages API client SDK with full IFC: API key as `Secret[String]`, responses as `Tainted[String]`. Zero builtins, pure MVL implementation of request/response serialization and HTTPS calls via `pkg/tls`.
- #1020 — `pkg/rest` — Typed REST client layer (JSON in/out) built on `pkg/tls.https` with `rest_post_json` / `rest_get_json` convenience functions.
- `examples/anthropic_chat` — Runnable example demonstrating SDK usage with full IFC threat model.

### Fixed
- Security: Split premature declassification in `Claude::messages()` to use distinct audit tags for JSON parse path vs error display path.
- Security: Error body truncation (512 byte cap) in `AnthropicError` and `RestError` to prevent unbounded allocation and verbatim display of hostile response bodies.
- Correctness: Multiple `Role::System` messages now error (API supports one) instead of silently dropping all but first.
- Build: Remove dead `pkg/http` symlink rule and Makefile dependency from `examples/anthropic_chat`.

### Docs
- Added `.gitignore` to `examples/anthropic_chat` to exclude `.mvl/` symlink directories.
- Clarified `multi_turn()` example in `main.mvl` is illustrative, not called from `main()`.

### Chore
- Refactored `pkg/rest` header-merge logic into reusable `merge_headers()` helper (eliminates 10-line duplication).
- Added 4 new unit tests to `pkg/anthropic` (missing usage field, wrong type, empty array, no messages).
- Added 2 new unit tests to `pkg/rest` (InvalidUrl, InvalidResponse error variants).

## [0.155.0] - 2026-05-25

### Added
- #1017 — `pkg/tls` — TLS 1.3 client layer using rustls with full Rust/LLVM backend parity. Enables HTTPS for both client and server via `https_get/post/put/delete` convenience layer.
- `make check-pkg` — Root Makefile target that type-checks all packages (pkg/*)

### Fixed
- Security: Port range validation (reject 0, negative, >65535) in HTTPS URL parsing
- Security: Error message sanitization (strip hostname/OS details from TLS error reporting)
- Correctness: Add 1 MiB size cap to `tls_read` (prevents OOM on attacker-controlled responses)
- Correctness: Handle flush errors in `tls_write` instead of silent discard
- Testing: Add 12 new HTTPS tests for CRLF injection validation and port bounds

## [0.154.2] - 2026-05-25

### Fixed
- #980 — LLVM backend now heap-allocates Option/Result payloads to prevent dangling pointer SIGSEGV
- #987 — Rust codegen now inlines pkg-defined actors from prelude programs into standalone binaries
- #991 — Audited all 98 unreachable!/panic! sites; added CI check to prevent new unvetted sites

### Docs
- #926 — Fixed stale operator precedence documentation and ADR-0022 intrinsic mapping examples

### Chore
- #913 — Updated config_server example to use `get_secret()` for API key management instead of hardcoded config
- #992 — Documented 4-phase desugaring plan for eliminating 4-way method dispatch synchronization

## [0.154.1] - 2026-05-25

### Fixed
- #1027 — Label-to-bare `TypeMismatch` now emits `LabelMismatch` (Req 11/IFC) instead of polluting Req 1
- #1028 — `MissingConstraint` mapped to Req 1 (Type Safety) instead of Req 9 (Data Race)
- #1029 — Removed false-positive `ForLoopInPartialFn` — `for` loops are always bounded

### Added
- `suggest-decreases` lint rule — hints when a `while` loop has an obvious decrementing variable (#1037)
- `suggest-total-upgrade` lint rule — hints when a `partial fn` could be `total fn` (#1038)

### Changed
- Split `linter/rules.rs` into 5 submodules: `style`, `ast_style`, `semantic`, `reading_quality`, `complexity`
- Deduplicated cyclomatic complexity between linter and passes (#1040)

## [0.153.0] - 2026-05-25

### Added
- Requirement verdict tests — 15 new test cases covering contracts, decreases, relabel, and implicit flows
- `OptionIgnored` error check (Requirement 5) — enforce handling of Option return values
- Corpus tests for method-call predicates in requires clauses and decreases measures

### Fixed
- #968 regression test — verify `decreases` on method-call measures
- #983 regression test — verify `requires` predicates with method calls
- Requirement 5 gap — unhandled Option values now caught at compile time

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.152.0] - 2026-05-24

### Added

- **std/json: JSONL encode/decode** (#998): Adds `jsonl_encode(values: List[Value]) -> String` and `jsonl_decode(s: String) -> Result[List[Value], JsonError]` as pure-MVL functions with no effects. `jsonl_encode` serialises one JSON value per line with a trailing newline; `jsonl_decode` splits on `\n`, skips blank lines, and fails-fast on the first malformed line. 13 new tests covering encode, decode, blank-line skipping, error propagation, and roundtrip verification.

## [0.151.0] - 2026-05-24

### Added

- **linter: while-to-for-range lint rule** (#1004): New linter rule detecting `while VAR < END { ...; VAR = VAR + N }` counter patterns and suggesting conversion to `for VAR in range(START, END)`. Rule id: `while-to-for-range`, severity: warning, default: on. Heuristic matches when all three hold: while loop with no `decreases` clause, condition is `VAR < END`, and last statement in body is `VAR = VAR + N`. Loops with explicit `decreases` clause are silently skipped. Complements existing `for-iter-antipattern` rule (#705) which catches list iteration patterns.

## [0.150.0] - 2026-05-24

### Added

- **cli: mvl fmt — source code formatter** (#1008): New `mvl fmt <file|dir>` command with full comment preservation via two-pass printing (extract comments from source, re-inject during AST emission). Supports `--check` (exit 1 if any file needs formatting), `--stdout` (print to stdout instead of modifying files), and `--stdin` (read from stdin, write to stdout). Directory mode recursively processes all `.mvl` files. Comment preservation includes both `//` line comments and comment-only lines; blank lines separate declarations. Idempotent: `fmt(fmt(src)) == fmt(src)`. Verifies zero type errors lost: roundtrip tests confirm `check(fmt(src))` has identical error counts and per-requirement verdicts as `check(src)`.
- **cli: mvl check --stdin** (#1008): Extended `mvl check` to support `--stdin` for reading MVL source from standard input. Useful for pipe-friendly workflows (e.g., `mvl fmt | mvl check`). Supports all checker options: `--error-limit`, `--format=json`, `--verbose`, `--req N`, `--refinement-solver`, `--refinement-stats`. Cross-module imports cannot be resolved without file system context; documented limitation.

### Testing

- **roundtrip tests** (`tests/fmt_roundtrip.rs`): 16 integration tests verifying formatter semantic preservation across 5 corpus categories (basics, types, ownership, effects, termination, contracts). Each test verifies `check(fmt(src)) == check(src)` (identical error counts and per-requirement verdicts) and idempotency (`fmt(fmt(src)) == fmt(src)`).

## [0.149.0] - 2026-05-24

### Added

- **stdlib: args::parse_args() --version/-v flag support** (#996): Extended `FieldSpec` enum with `Version(String)` variant to enable schema-driven version priming. `parse_args()` pre-scans arguments for `--version` or `-v` flags before processing other options, and exits with version string if found. Defaults to "0.0.0" if not specified. Includes unit tests in `tests/stdlib/args_test.mvl` and integration tests in `tests/integration/compile_and_run/args.sh`.
- **testing: example smoke tests + CI integration** (#997): Added `smoke` target to all 14 example Makefiles (build binary without terminal/network/specific arguments). Created `examples/test-all.sh --full` infrastructure to validate all examples compile successfully. Integrated into CI with new `examples` job that runs on stdlib/examples changes, proving compilation integrity without full runtime execution. All 14 examples now pass smoke tests: access_control, actor_pingpong, actor_trading, actor_webserver, bzip, config_server, csv_transactions, flight_clearance, log_analyzer, medical_triage, programs, snake_game, sqlite_basic, task_pipeline.

### Fixed

- **examples/bzip: eliminated all .unwrap() calls** — Migrated from non-existent `.unwrap()` method to `.unwrap_or(default)` across all modules: rle.mvl (8 calls), bwt.mvl (9 calls), mtf.mvl (8 calls), huffman.mvl (15+ calls mixed types), bitstream.mvl (1 call), main.mvl (2 calls). Defaults selected per type: `0` for Option[Int], `[]` for Option[List[Int]], `HuffmanTree::Leaf(0, 0)` for HuffmanTree variants.
- **examples/bzip/main.mvl: fixed totality violations** — Marked `huffman_encode_stream`, `huffman_decode_stream`, `compress_bytes`, `decompress_bytes`, and `main()` as `partial fn` due to recursive call chains. Added `main.mvl` to bzip Makefile `check` and `test-solver` targets. Effect annotations: `compress_bytes` and `decompress_bytes` marked `pub partial fn`; `main()` retains `partial fn main() -> Unit ! Console`.
- **examples/sqlite_basic: fixed smoke target path resolution** — Changed smoke target to run from REPO_ROOT to ensure SQLite context is available: `cd $(REPO_ROOT) && $(abspath $(MVL)) build $(DIR)main.mvl`.
- **examples/actor_webserver: config.mvl relabel migration** (#882) — Replaced `.into_inner().concat("")` with `relabel trust(raw, "CONFIG-FILE")` for PR #882 compatibility (IFC label normalization).
- **examples/task_pipeline: added Env effect** — Added `+ Env` effect to `run()` and `main()` signatures since `parse_args()` requires `! Env` effect.
- **examples/actor_trading: added test-solver target** — Added `test-solver: check ## Show per-file solver statistics (alias for check)` for consistency with other examples.

## [0.148.0] - 2026-05-24

### Added

- **tooling: Phase 1 LSP server** (`tools/lsp_server.py`) — tree-sitter-based language server providing real-time syntax diagnostics for `.mvl` files in any LSP-capable editor. No compiler binary required; uses the `tree-sitter-mvl` Python binding bundled in `etc/tree-sitter-mvl/`. Includes VS Code client (`etc/vscode-mvl/extension.js`) and Neovim helper (`etc/nvim-mvl/lua/mvl/lsp.lua`). Install with `cd tools && make install`. Full type/effect diagnostics tracked in #1003 (Phase 2).

## [0.147.2] - 2026-05-24

### Fixed

- **compiler: resolve post-#993 regressions** — Fixed 12 regressions introduced by PR #993 (format 2-arg migration, declarative sink keyword, UnknownMethod enforcement). `sink` keyword collision silently broke 11 IFC propagation tests (parser dropped unparseable functions). `format(...)` collided with Rust's `format!` macro — added `mvl_format` runtime function for both Rust and LLVM backends. Missing method table entries (`char_at`, `byte_at`, `concat`, `reverse`, `Bool.to_string()`) caused stdlib and corpus failures. Unary `!` operator lacked parens in transpiled Rust, breaking `(!b).to_string()` chains. Migrated all remaining old-style variadic `format` calls across 23 files. `write(path(...))` → `write_file(path(...))` in `std/kv/file.mvl` after Fd unification (#982). Added LLVM `emit_bool_to_string` with `select` on `"true"`/`"false"` globals for backend parity. Result: all 13 test suites pass except LLVM/cross-backend (now also fixed).

## [0.147.1] - 2026-05-23

### Fixed

- **compiler: eliminate todo!/panic stubs, audit unreachables, 4-way sync docs** (#990, #991, #992): Added `check_impl_decl()` validation to prevent 7 unimplemented todo!/unimplemented!() stubs in Rust backend from being reached at runtime — trait impl methods now fail at compile time if bodies are missing (#990). All production unreachable!() sites annotated with GitHub issue numbers and layer-specific context to clarify legitimacy and support regression audits (#991). Added `make audit-panics` Makefile target that counts unreachable!/panic! calls across codebase with budget of 100, establishing baseline at 98 and failing CI if exceeded. Comprehensive 4-way sync documentation added to all method-definition and emission points (std/*.mvl, method_types.rs, emit_exprs.rs, llvm/exprs.rs) explaining the requirement to keep Type → Checker → Rust backend → LLVM backend in lockstep when adding builtin methods (#992). Full architectural fix (method desugaring) deferred to Phase 9.

## [0.147.0] - 2026-05-23

### Added

- **pkg.http Phase 3: HttpServer + ConnectionHandler actors** (#800): `HttpServer` owns the `TcpListener` and spawns a `ConnectionHandler` per accepted connection. `ConnectionHandler` reads one HTTP request, dispatches via `Router`, writes the response, and closes the stream — no shared mutable state. `Dispatcher = fn(Request, MatchedRoute) -> Response` is the public type alias for custom handler tables. `serve()` is a convenience wrapper for one-call server setup. 6 new routing tests; `examples/http_server.mvl` demonstrates the full API.

## [0.146.1] - 2026-05-23

### Fixed

- **parser: requires/ensures/invariant no longer silently drop complex expressions** (#983): Contract clauses containing method calls (e.g. `requires items.len() > 0`) or other constructs not supported by `RefExpr` were silently discarded. Fix mirrors #968 (`decreases` fix): AST widened to `Vec<Expr>`, new `parse_contract_expr()` uses `parse_expr()` for general expressions and wraps `forall`/`exists` in a new `Expr::Quantifier` variant. Extended `expr_to_ref_expr_ext` handles comparisons, logical ops, field access, and `x.len()` calls for static verification. Unsupported shapes degrade to `RuntimeCheck` rather than being dropped. 3 regression tests added.
- **loader: restore `format_error_with_source` accidentally removed in #982** (#988): Function was called but not defined, breaking compilation of the LLVM backend and benchmarks.

## [0.146.0] - 2026-05-23

### Added

- **std.kv: file-based human-readable key-value store** (#963): Pure MVL KV implementation with cat-able `key : type = value` format. Zero external dependencies; suitable for config files and embedded/ESP32 use cases. Supported types: Null, Bool, Int, Float, Text, Blob. Public API prefixed `kv_` to avoid prelude namespace collisions: `kv_new`, `kv_open`, `kv_save`, `kv_get`, `kv_get_text`, `kv_get_int`, `kv_get_bool`, `kv_get_float`, `kv_get_blob`, `kv_set`, `kv_delete`, `kv_keys`, `kv_len`. Infrastructure: recursive std/ directory scan in `build.rs`, subdirectory creation in `stdlib.rs`, multi-component path support in `loader.rs`. 25 tests covering all value types, edge cases, and effectful round-trip.

## [0.145.0] - 2026-05-23

### Added

- **std.csv: RFC 4180 CSV parser with IFC-aware encode/decode** (#978): Pure MVL CSV implementation with cell-level taint tracking. Includes `parse_rows`, `parse_rows_with`, `parse_with_headers` functions returning `Tainted[String]` cells (external input is untrusted). Encode counterparts (`encode_rows`, `encode_with_headers`) transform clean structs to CSV strings. Supports quoted fields, embedded commas/newlines, escaped quotes, custom delimiters (TSV), CRLF/LF line endings. Decode functions validate tainted cells and call `relabel trust()` at trust boundaries — explicit audit points. CsvError enum with variants for IO, parse, column count, and field validation errors. Example demonstrates end-to-end pipeline: read CSV file, parse with headers, show all rows, re-encode to stdout.

## [0.144.0] - 2026-05-23

### Added

- **std.log: explicit Logger value replaces global log state** (#973): Removes process-global `log_set_format` / `log_set_min_level` in favour of a `Logger` struct that carries `format` and `min_level` as plain values. Callers construct a `Logger` (or use `default_logger()`) and thread it explicitly through the call graph — no hidden global state, no thread-safety concerns. `log_debug/info/warn/error` free functions replaced by `Logger::debug/info/warn/error` methods. `runtime/rust` and `runtime/llvm` log shims deleted (~770 lines). IFC implicit-flow enforcement extended to `Expr::MethodCall` nodes so Secret-conditional `logger.info(...)` branches are rejected at compile time. `effect Log > Clock + Console` reflects that logging writes to stderr; `log_write` made module-private; `sanitize_log`/`json_escape`/`pad_right` added to `std/strings.mvl`.

## [0.143.1] - 2026-05-23

### Fixed

- **Parser: decreases clause accepts method calls, fixes silent loop body drop** (#968): Extended `decreases` measure syntax from restricted `RefExpr` to full `Expr`, allowing method calls like `result.len()` in termination measures. Fixed critical parser bug where `decreases` parse failures were silently swallowed, causing the entire loop body to be discarded with no diagnostic (token stream misalignment). Now propagates hard parse errors and converts valid expressions to `RefExpr` for static termination checks; unconvertible expressions (method calls) fall back to `RuntimeCheck` with loop body preserved. Added tests for method-call and arithmetic-expression decreases clauses.

## [0.143.0] - 2026-05-23

### Added

- **pkg/http query string parsing** (#957): `Request` now carries `query: Map[String, List[Tainted[String]]]` populated by `parse_request`. Implements FastAPI/Starlette-style multi-value semantics with values kept `Tainted` as they originate from user-supplied URL input. Adds `percent_decode` (lenient WHATWG — `+` → space, multi-byte UTF-8 via byte-accumulation, malformed `%XX` passed through as literal `%`) and `parse_query` (splits on `&`, decodes both sides, skips empty-key pairs, re-wraps values with `relabel taint`). Convenience accessors `query_first` / `query_all` mirror FastAPI's `query_params[key]` / `getlist(key)`. 21 new tests covering ASCII, multi-byte UTF-8 (`café`), `+`, malformed escapes, repeated keys, fragment stripping, and `query_first`/`query_all`.

## [0.142.0] - 2026-05-23

### Added

- **pkg.http testing utilities** (#951): Adds `pkg/http/src/testing.mvl` with response parsing and BDD-style assertion helpers. Includes `test_request(method, path)` for building raw HTTP/1.0 request strings, `parse_response(raw)` for parsing status line/headers/body, and `expect_status` / `expect_body_contains` / `expect_header` assertion helpers. 20 unit tests covering happy path and edge cases. Pure MVL, no extern blocks or runtime dependencies.

## [0.141.1] - 2026-05-23

### Fixed

- **examples/actor_webserver accept-error handling** (#952): Distinguish transient `tcp_accept` errors (`ConnectionReset`, `Timeout`) from fatal listener-level errors. Transient errors log a warning and tail-recurse; fatal errors propagate to `main` for lifecycle handling. Replaced `while true` with tail recursion (idiomatic for MVL's expression-based syntax and `partial fn` semantics). Fixed empty `{}` map literals (parsed as `Unit`, not `Map`) by supplying `reason` fields in all `log_warn` calls. Enum variants in match patterns now fully qualified (`NetError::ConnectionReset`).

## [0.141.0] - 2026-05-23

### Added

- **pkg.http HTTP package** (#783, #799, #800, #913): Extract HTTP types and functions into a standalone pure MVL package. Includes Status enum with 20 HTTP status codes (Http200Ok → Http504GatewayTimeout), HttpError struct for FastAPI-style error responses, Request/Response types, and helper functions (parse_request, serialize_response, ok, not_found, error_response). No extern blocks or native dependencies required. Enables examples/actor_webserver to use pkg.http instead of stdlib utilities.
- **HTTP status code classification helpers**: is_success() and is_error() predicates for status code ranges (2xx, 4xx+).

### Changed

- **examples/actor_webserver refactored for pkg.http** (#783, #799, #800, #913): Route function now returns Result[Response, HttpError]. Removed Status parameter from config layers. RequestHandler actor unchanged (iso stream ownership preserved). Main function flattened: config loading + logging setup + server startup in sequence, with explicit exit(1) on config error instead of fallback defaults. Layered config system (defaults → TOML → env → CLI) harmonized: single default source in config.mvl, no duplicate in main. Package resolution via .mvl/pkg/http local symlink (preference over global pkg/).

### Fixed

- **Config load error handling** (#913): Removed duplicate config defaults that shadowed config.toml and environment overrides. Configuration now fails explicitly with exit(1) on load error, preventing silent fallback to incorrect defaults.
- **Local package resolution** (#800): Created .mvl/pkg/http symlink pattern to prefer local packages over global pkg/ directory resolution.

## [0.139.1] - 2026-05-22

### Fixed

- **Rust backend capability label support** (#931): Add `DbUrl`, `ConfigPath`, `ApiEndpoint`, `AuditTarget` newtype wrappers to `mvl_runtime::capability` and implement relabel codegen for all 8 capability transitions (4 wrap + 4 unwrap). Fixes compilation errors in examples using `std.db` or `std.audit` capability-labeled functions.

## [0.139.0] - 2026-05-22

### Added

- **Capability labels as IFC tokens** (#931, #932): Four new `label` types (`ConfigPath`, `DbUrl`, `ApiEndpoint`, `AuditTarget`) reuse existing IFC machinery to provide provenance tracking for resource identifiers. Type system enforces label compatibility at call boundaries — bare `String` or mismatched labels are rejected where a capability label is expected. Capability-aware wrapper functions in `std/io.mvl` and `std/net.mvl` accept labeled types; raw builtins remain available for backward compatibility. Parser and checker pre-seed all 4 labels and 8 relabel transitions. ADR-0001 and specs 002-003 updated: capability security absorbed into Req 11 (IFC labels), not Req 7 (effects).

### Changed

- **Req 13 absorption clarification** (#932): Capability-based security absorbed into Req 11 (IFC labels as capability tokens) + std/audit (runtime policy), not Req 7. Effects (`! FileRead`) tell you the *class* of action; capability labels tell you *which* resource.

## [0.138.0] - 2026-05-22

### Added

- **Assurance gap closure — specs 015/016 fully covered** (#943): All 14 requirements across Spec 015 (Actors) and Spec 016 (Session Types) now have complete test evidence and corpus. Spec 015 Reqs 7-9 have new corpus files (ActorRef tag semantics, structured concurrency scope, select with timeout). Spec 016 Reqs 4-5 have scenario definitions, test links, and negative corpus for duplicate branch labels. Assurance dashboard improved: Corpus 12/14 → 20/22, Coverage 94 → 96, Assurance 88 → 90.
- **Actor design evaluation** (#854): Comprehensive analysis of five open design questions (reduction budget, bidirectional links, supervisor scope, scheduling/session interaction, failure model completeness). All questions resolved with no blocking issues for Phase 8.

## [0.140.0] - 2026-05-22

### Added

- **Guard patterns in match** (#938): Parser now accepts `pattern if expr => body` syntax for conditional match arms. Guard expressions use the refinement expression language (comparisons, logic ops). Guarded arms don't count toward exhaustiveness checking — a wildcard catch-all is still required. All backends (Rust, LLVM) and MC/DC analysis already supported guards; only the parser was missing.

### Fixed

- **Post-consume iso ownership tracking (L5)** (#938): After `let y = consume(x)`, `y` is now tracked as the new iso owner. Subsequent aliasing `let z = y` correctly emits `IsoAliasingViolation`. The consumed variable `x` is removed from tracking. Branch-scoped iso tracking uses snapshot semantics (conservative). Resolves spec 014 Known Limitation L5.

### Changed

- **Req 6 fully proven — reclassify `LinearTypeBareBind` under Ownership**: `LinearTypeBareBind` now maps to requirement 6 (Ownership / linearity) instead of requirement 2 (Memory Safety). Linear resource consumption (must use `consume()`) is an ownership/linearity concern. Negative corpus tests `bare_linear_assignment.mvl` and `linear_assignment_without_consume.mvl` moved from `tests/negative/req02/` to `tests/negative/req06/`. Req 6 `BasicCheckPass` evidence updated. ADR-0001 Req 6 status updated from "partial" to fully proven at Phase 1.

- **Complete stdlib extension method migration** (#928): Migrated ~300+ call sites across ~35 MVL files from old-style free function calls (`map_get(m, k)`) to extension method syntax (`m.get(k)`). Fixed codegen issues: `join`/`to_string` name collision with io module, set operation use-after-move, LLVM use-after-free on function parameter drops, LLVM HOF/set method dispatch for mangled extension names. Fixed tree-sitter unnecessary grammar conflicts and spike parser type annotation typos.

## [0.137.0] — 2026-05-22

### Added

- **Guard patterns in match expressions** (#938): Added `pattern if expr => body` syntax for match guards. Parser extends `parse_match_arm()` to accept optional `if` followed by a predicate expression. Exhaustiveness checker updated: guarded arms do not satisfy pattern coverage (a guard may fail). LLVM backend emits conditional branch after pattern binding: guard succeeds → arm body, guard fails → next arm or fallback. Supported guard shapes: comparisons, boolean operators, logical operators, field accesses, arithmetic. Comprehensive corpus test covers basic guards and error cases (non-exhaustive with guarded wildcard).

## [0.136.0] — 2026-05-22

### Added

- **If-let-else syntax** (#891): Added expression and statement forms of if-let-else for concise single-pattern matching. Supports `if let Pattern(v) = expr { ... } else { ... }` syntax. Parser desugars to exhaustive match at parse time. Modernized `config_server` and `task_pipeline` examples to use if-let instead of verbose match expressions.

## [0.135.2] — 2026-05-22

### Fixed

- **Reject linear type assignment without consume()** (#934): `check_assignment()` now enforces the same linear-type rule as `let` bindings — assignment of linear types (String, List, Map, Set) requires explicit `consume()`. Added checks in Stmt::Assign mirroring stmts.rs:297-310 logic. Fixed 3 bare linear assignments caught in `std/json.mvl`.
- **Verify BorrowState transitions** (#935): Investigated claim that transitions were not implemented. Confirmed all 6 acceptance criteria met by existing code (stmts.rs:331-392, infer.rs:145-164, context.rs:755-772 with comprehensive test coverage). Closed as already implemented.

### Changed

- **Update Spec 009 borrow inference phase status**: Documented Phase B (borrow parameter inference) as implemented per #660. Phase B algorithms (parameter analysis, disqualifying uses, borrow kinds) now explicitly described with implementation and test links. Corrected stale "Phase B deferred" / "Phase C target" references.

## [0.135.1] — 2026-05-21

### Fixed

- **Support extension method syntax throughout compiler pipeline** (#928): Commit 86df6e7c migrated stdlib declarations to `fn Type::method(self)` syntax but did not update parser, checker, or backends. Fixed parser to handle receiver type params (`fn Type[T]::method`), checker to accept builtin types (String/List/Map/etc.) as receivers and resolve static `Type::method()` calls via method_table, Rust backend to emit correct standalone functions, and LLVM backend to compute correct bridge names and emit UFCS dispatch for extension methods. Updated `std/strings.mvl`, `std/log.mvl`, `std/args.mvl`, `std/json.mvl` to use method syntax.

## [0.135.0] — 2026-05-21

### Added

- **Convert `env_var` to pure MVL** (#900): Wrap `_env_read` + `relabel taint` instead of being a builtin alias. Removes redundant Rust runtime implementation.
- **Convert `regex::replace` to pure MVL** (#900): Implement using `find_all` + `str_concat`/`str_substring`. Removes LLVM backend builtin revert introduced in #900 fix commit.

### Changed

- **Revert LLVM pass-ordering hack** (#900): Move builtin emission back to pass 4 (last), pure-MVL bodies to pass 2. Remove `count_basic_blocks() > 0` early-return guards from `emit_fn` and `emit_extern_rust_fn_body`. Last-definition-wins semantics now restored naturally via `load_rust_backed_stdlib_fns` appending hybrid-module bodies after implicit prelude.
- **Update `trusted.mvl` profile manifest**: Note that `replace` joins `find_all` as pure MVL since #903.

### Fixed

- **Fix `relabel taint` syntax in `env_var`**: Requires 2-arg form `relabel taint(v, "TAG")`, not 1-arg. This parse error cascaded, preventing resolution of `getuid`, `getgid`, `signal_on`, and other `std.env` functions, causing 5 corpus test failures.
- **Add `relabel_expr` to grammar coverage tool** (`TS_KNOWN_EXTENSIONS`): Tree-sitter grammar extension now documented.
- **Fix `&i64` pattern bindings in checked arithmetic** (#920): Pattern-bound variables in match arms on `&Enum` are `&i64`, not `i64`. The `as i64` cast fails on references. Use `<i64>::clone(&(expr))` which handles both types via auto-deref. Fixes huffman example build failure.

## [0.134.1] — 2026-05-21

### Fixed

- **Docs §19.5 corrected** (#919): section "No Bitwise Operators" was wrong — `&`, `|`, `^`, `~`, `<<`, `>>` are first-class operators implemented in the parser, AST, and both backends. Section rewritten with precedence table and examples.
- **Rust backend: Int arithmetic traps on overflow** (#920): `+`, `-`, `*` on `Int` now emit `.checked_add/sub/mul().expect("integer overflow")` instead of bare operators, matching the LLVM backend's overflow-trap behaviour.
- **LLVM backend: `&&`/`||` now short-circuit** (#921): previously emitted as bitwise `and`/`or` instructions (eager evaluation). Now uses conditional branch + phi-node pattern; rhs is only evaluated when lhs does not determine the result.

## [0.134.0] — 2026-05-21

### Added

- **Declare 30 hidden backend methods in stdlib** (#905): `pub fn` / `pub builtin fn` declarations for methods that already existed in the Rust/LLVM backends but were invisible to MVL programmers. Int: `int_bit_and/or/xor/not`, `int_shift_left/right`, `int_wrapping_add/sub/mul`, `int_checked_add/sub/mul/div`. Bool: `bool_to_string` (pure MVL). Byte: `from_int` (builtin), `byte_to_int`, `byte_to_string`, `byte_bit_and/or/xor/not`, `byte_shift_left/right`, `byte_wrapping_add/sub/mul`, `byte_checked_add/sub/mul`. List: `group_by`, `windows`, `chunks`. Option: `and_then` (pure MVL). Backend: auto-bound scan now includes return types (fixes `K: Hash+Eq` for `group_by`); `windows`/`chunks` cast size argument to `usize`.


## [0.133.0] — 2026-05-21

### Added

- **UFCS dispatch table for string/list method parity** (#906): Unified Function Call Syntax for method calls in LLVM backend, matching Rust transpiler's MethodCall-to-dispatch-table approach. Organizes method call dispatch into six groups (A–F) by C runtime function signature (ptr→ptr, ptr×ptr→ptr, etc.). Includes string methods (trim, to_lower, to_upper, starts_with, ends_with, contains, replace, substring, concat, split) and list methods (slice, take, skip). Eliminates 30+ explicit match arms, reducing duplication and improving maintainability. Both backends now produce identical output for UFCS method calls via identical cross-backend corpus tests.


## [0.132.1] — 2026-05-21

### Fixed

- **LLVM backend correctly handles hybrid stdlib modules** (#900): regex and time modules contain both Rust-backed `pub builtin fn` declarations and pure-MVL helper functions. The LLVM backend now emits builtin bodies first (before pure-MVL), preventing same-named wrappers from overwriting C-ABI dispatches. Also marks `regex::replace` as a builtin to avoid collision with `strings::replace`. Fixes cross-backend tests: `cross_backend_regex_find_all`, `cross_backend_regex_replace`, `cross_backend_time_format_datetime`.

## [0.132.0] — 2026-05-20

### Added

- **Cross-function implicit flows — PC label across call boundaries** (#832): the IFC implicit flow checker now detects public sinks reachable from callees invoked under a high-PC branch condition. `if secret { log_access("x") }` is now a compile error when `log_access` transitively calls `println`. Adds `CrossFunctionImplicitFlowViolation` (Req 11) with `pc_label`, `caller`, `callee`, and `sink` fields, and a BFS-based sink reachability analysis over user-defined function call edges.

## [0.131.1] — 2026-05-20

### Fixed

- **LLVM backend `.clone()` for heap types creates independent copy** (#904): replaced no-op identity return with true deep-clone functions (`mvl_array_deep_clone`, `mvl_string_deep_clone`, `mvl_map_deep_clone`). Mutations on cloned collections no longer affect originals. Type-dispatched via receiver type lookup, matching `.len()` pattern. Also removed `tests/corpus/05_effects/parametrized.mvl` (unimplemented syntax from #290).

## [0.131.0] — 2026-05-20

### Added

- **Convert 12 reducible builtins to pure MVL** (#903): `str_contains`, `str_starts_with`, `str_ends_with`, `str_trim`, `str_to_upper`, `str_to_lower`, `str_replace` (strings.mvl); `env_var` (env.mvl); `path` (io.mvl); `format_datetime` (time.mvl); `find_all`, `replace` (regex.mvl). Shrinks the Rust stdlib surface and enables in-language testing of stdlib functions.

## [0.130.1] — 2026-05-20

### Fixed

- **Eliminate `is_variadic_builtin` bypass for 6 stdlib functions** (#902): Removed type-safety escape hatch from checker. `assert_eq`, `assert_ne`, `parse_int`, `float`, `choice`, and `shuffle` now properly enforce arity and type checking. Only `format` remains in the bypass pending #901 redesign. Fixes hardcoded function registrations in `register_builtins()` by marking generic functions with `type_params` and correcting param counts for non-generic ones.

## [0.130.0] — 2026-05-18

### Added

- **`map_new[K, V]() -> Map[K, V]` builtin for empty map creation** (#860): new stdlib function to create empty maps without the sentinel-and-remove workaround. `{}` parses as an empty block, not a map literal; `map_new()` provides a clean alternative. Inline codegen in both backends: Rust → `HashMap::new()`, LLVM → `mvl_map_new(8)`. Removes four workaround helpers from `std/args.mvl` that existed solely for this limitation.

## [0.129.0] — 2026-05-18

### Added

- **std/io: Stdout/Stderr I/O handles** (#839): new `Stdout` and `Stderr` types with builtin entry points `stdout()` and `stderr()`. Raw write primitives `stdout_write()` and `stderr_write()` enable pure MVL implementations of console output functions. Pattern mirrors existing `Stdin` for symmetric I/O design.
- **Pure MVL print functions** (#839): `print`, `println`, `eprint`, `eprintln` now implemented as pure MVL wrappers over stdout/stderr writes instead of Rust builtins. Reduces builtin footprint while maintaining full functionality.
- **Pure MVL log functions** (#839): `log_debug`, `log_info`, `log_warn`, `log_error` converted to pure MVL implementations. Four minimal builtins (`log_get_format_int`, `log_get_level_int`, `log_timestamp`, `log_write`) provide runtime state access and stderr writes. All format logic (plain/logfmt/json) and sanitization implemented in pure MVL.

### Changed

- **ADR-0024: Universal IFC label propagation** (#839): all functions now propagate security labels by default. **Before:** `format("{}", secret)` silently dropped `Secret` labels. **After:** `format("{}", secret)` returns `Secret[String]`; passing it to `println` is now a compile-time IFC error. Excess-label approach prevents double-counting — only label exceeding declared parameter type propagates. Fixes fundamental security gap in information-flow control.
- **Type-attached methods** (#868): `fn Type::method(self, ...)` syntax for methods bound to types. Methods resolve via dot-call syntax (`x.method()`). No implicit UFCS; method resolution is unambiguous.

### Builtin Reduction

Consolidated 9 builtins → 4 builtins in I/O and logging subsystems:

| Function | Before | After |
|----------|--------|-------|
| print | builtin | pure MVL |
| println | builtin | pure MVL |
| eprint | builtin | pure MVL |
| eprintln | builtin | pure MVL |
| log_debug | builtin | pure MVL |
| log_info | builtin | pure MVL |
| log_warn | builtin | pure MVL |
| log_error | builtin | pure MVL |
| log_format_entry | builtin | pure MVL (formatters) |
| stdout | — | new builtin |
| stderr | — | new builtin |
| stdout_write | — | new builtin |
| stderr_write | — | new builtin |

## [0.128.1] — 2026-05-18

### Fixed

- **Refinement subsumption: Ty::Refined now stores RefExpr AST, not Debug string** (#880): predicates are stored as `Box<RefExpr>` instead of `format!("{pred:?}")` strings. Structural `PartialEq` on `RefExpr` allows two refined types with syntactically different but semantically equivalent predicates (e.g. `x > 0 && x < 10` vs `x < 10 && x > 0`) to be correctly recognized as equal. This fixes subsumption checks that were falling back to `RuntimeCheck` due to string inequality. The string field was dead code (all match arms discarded it with `_`).

## [0.128.0] — 2026-05-18

### Added

- **`env::get_secret()` — Secret[String] for API keys and credentials** (#872): new stdlib function `pub fn get_secret(name: Clean[String]) -> Option[Secret[String]] ! Env` implemented as a pure MVL wrapper over `env::get()`. Upward flow (Tainted → Secret) is free in the IFC lattice — zero Rust runtime changes needed. Secrets loaded via this function cannot be passed to `println`, `log_*`, or any public sink without explicit `declassify()`. Corpus tests, runtime roundtrip tests, and spike validation suite included.

## [0.127.2] — 2026-05-18

### Fixed

- **IFC soundness: Clean[String] label preserved through parse_args tokenizer** (#873): `raw_named` and `positionals` now typed as `Map[String, Clean[String]]` / `List[Clean[String]]`; `coerce_arg` receives `Clean[String]` and returns `ArgValue::Str` directly without re-sanitizing via String. Closes the trust-erosion gap from PR #859 review (Critical 1 & 2). Also fixes two pre-existing transpiler test regressions from commit 9a513f5b (`labeled_param_transpiles`, `corpus_args_transpiles`).

## [0.127.1] — 2026-05-18

### Fixed

- **IFC soundness: For-loop iterator taint tracking** (#858): `Stmt::For` pattern variables now correctly receive iterator security labels; nested patterns like `for (a, b) in tainted_pairs()` now propagate taint to all bound names
- **IFC soundness: Nested destructuring taint preservation** (#858): `Stmt::Let` with nested patterns like `let (Some(x), y) = source()` now correctly propagates taint to all identifiers in the full pattern tree (recursive `bind_pattern_labels` helper)
- **IFC soundness: Lambda return type annotation visibility** (#858): `Expr::Lambda` with declared return types like `|| -> Tainted[String] { ... }` now correctly propagate taint at the call site; `let f = || -> Tainted[T]; f()` now marks the result as tainted
- **IFC false positive: FnCall env lookup shadowing** (#871): local variables no longer shadow unannotated functions of the same name in taint label inference; guarded env lookup with `!inferred.contains_key(name)`
- **Implicit-flow gap: For-loop taint propagation in ifc.rs** (#858): `check_implicit_flows` now handles for-loops over tainted iterators by extracting shared `bind_pattern_labels` helper to `ifc.rs`

## [0.127.0] — 2026-05-18

### Added
- **Monomorphization pass** (ADR-0034): compile-time polymorphism elimination (#838)
  - Generic-to-monomorphic transformation: rewrite generic functions and actors into specialized versions for each type parameter binding
  - `MonoProgram` structure carrying monomorphized functions, actors, and a `FnMonoIndex` for call-site type argument tracking
  - Integration with LLVM and Rust backends: backend receives pre-monomorphized program, eliminating runtime polymorphic dispatch
  - Full test coverage: generic function instantiation, actor specialization, type argument resolution, nested generics, standard library interaction

## [0.126.1] — 2026-05-17

### Fixed
- Grammar keyword divergence: add missing `effect` keyword to `docs/grammar.ebnf`, `compiler/lexer.mvl`, `compiler/ast.mvl`, and `etc/tree-sitter-mvl/grammar.js` to match Rust lexer ground truth (#852)
- Grammar: add `effect_decl` production rule to EBNF and tree-sitter grammar
- Pre-existing breakage in `compiler/main.mvl` from #844 args schema-driven refactor: migrate `get_arg()` (removed from std.args) to `std.env.{args}`; fix IoError formatting (it's an enum, not a struct)

## [0.126.0] — 2026-05-17

### Added
- **std.args: schema-driven CLI argument parsing** (#844): replace struct-based `ParseFromArgs` with a `List[FieldSpec]`-driven `parse_args` — the schema IS the argument spec, no codegen required
  - `ArgType` enum (`Str`, `Int`, `Float`), `FieldSpec` variants (`Required`, `Optional`, `Flag`, `Positional`, `OptPositional`), `ArgValue` enum carrying `Clean[String]` for `Str` (IFC-safe)
  - Schema-aware tokenizer: pre-builds flag set so value fields consume the next token regardless of `-` prefix (enables `--threshold -0.5`)
  - Typed result accessors: `get_str`, `get_str_opt`, `get_int`, `get_float`, `get_float_opt`, `get_flag`
  - Auto-generated `--help` / `-h` usage string from schema; exits 0 on `--help`, 1 on error
  - IFC: `ArgValue::Str` carries `Clean[String]` — CLI input sanitized inside `parse_args`, callers receive clean values directly

## [0.125.0] — 2026-05-17

### Added
- **Effect system upgrade** (ADR-0035): user-defined effects with subsumption-based hierarchies (#846, #852, #853, #855, #856, #857)
  - Effect declarations and hierarchy resolution: dual-pass compilation with cycle detection (#853)
  - Effect subsumption (`> ` operator) and transitive satisfaction checking for effect compatibility
  - Standard library effects: `IO`, `Log`, `Clock`, `Console`, `FileRead`, `FileWrite`, `Network`, `Actor`, `Spawn`, `Send`, `Recv`, `Terminal` (#856)
  - Type checker integration: replace hardcoded `VALID_EFFECT_NAMES` with dynamic hierarchy queries (#855)
  - Grammar and parser support for effect declarations in modules (#852)
  - Corpus tests for effect propagation across concurrency, I/O, and user-defined effect declarations (#857)

### Fixed
- Effect system tests: removed parametrized effect syntax tests (feature dropped as out-of-scope for #846)
- Cycle detection in `EffectHierarchy`: guard against panics with `.expect()` and trim cycle chains to contain only cycle members
- Effect validation error messages: clarify that valid effects are declared in `std/effects.mvl`

## [0.124.1] — 2026-05-17

### Fixed
- IFC `Stmt::Let` now consults declared type annotation before falling back to inferred init label, preventing false positives for validated bindings like `let clean: Clean[String] = validate(tainted)?` (#849)
- IFC `collect_violations_in_stmt` now handles `Pattern::Tuple`, `TupleStruct`, `Struct`, `Some`, `Ok`, `Err` destructuring patterns — previously only tracked `Pattern::Ident` bindings (#850)
- IFC `infer_label_extended` and `collect_violations_in_expr` now insert lambda parameters into the lambda-local env before recursing into the body, making parameter labels visible inside lambda expressions (#851)

## [0.124.0] — 2026-05-17

### Added
- **Interprocedural IFC analysis**: whole-program taint tracking across function call chains (#825)
  - Call graph construction: `CallGraph` struct for whole-program function call topology (#829)
  - Label propagation: fixed-point inference over call graphs with external taint source registry (#830, #833)
  - Violation detection: interprocedural information flow violations with call-chain error reporting (#831)
- **7 new unit tests** for IFC analysis: 3-hop SQL injection chain, mutual recursion termination, violation field assertions, Tainted→Public violations, let-binding taint tracking

### Fixed
- Call graph `reachable()` BFS infinite loop on cyclic calls — now correctly terminates
- IFC return-label inference now handles `MatchBody::Expr` arms (was returning `None`)
- IFC if-expression label inference no longer conflates implicit flow (condition) with explicit flow (value label)
- IFC `extract_chain` now threads caller's env to capture variable-routed taint in error messages
- Propagation and violation detection now cover `Decl::Impl` and `Decl::Actor` method bodies (previously only `Decl::Fn`)

### Changed
- `label_of_type_expr` moved to `ifc.rs` as `pub(crate)` to eliminate duplication
- `TAINT_SOURCES` extended to include `env_var`, `read_file`, `recv`, `recv_line` (note: method-call forms deferred to #838)

## [0.123.0] — 2026-05-16

### Added
- **Refinement solver benchmarks**: Criterion benchmark suite (`benches/refinement_solver.rs`) measuring all three solver modes across micro-programs and corpus files; layered solver is **127x faster** than Z3-only on typical refinement programs; CI job uploads results as artifact (#595)
- **Refinement performance docs**: `docs/refinement-performance.md` with real benchmark numbers and regression tracking guide

## [0.122.0] — 2026-05-16

### Added
- **Layered configuration pattern**: defaults → TOML → environment variables → CLI arguments with `config::{load_config, ServerConfig}` and reference pattern doc in `.openspec/patterns/001-config.md` (#828)
- **`std.log` level filtering**: `LogLevel` enum (Debug/Info/Warn/Error) and `log_set_min_level` to control runtime log verbosity; parse helpers `parse_log_level`/`parse_log_format` for config-driven log setup (#828)
- **Actor-per-request concurrency**: `RequestHandler` actor in `examples/actor_webserver` demonstrates fire-and-forget pattern with `iso` capability for exclusive socket ownership (#828)

### Fixed
- **Map literal codegen**: emit `.clone().into()` instead of `.into()` for map values to preserve MVL value semantics — fixes E0382 when a variable is used in a map literal and later in the same scope (#828)

## [0.121.0] — 2026-05-16

### Added
- **`pkg.sqlite`**: embedded SQLite driver with `std.db` types, `Open`/`Query`/`Execute` effects, refinement-typed API, and `examples/sqlite_basic` (#785)
- **Cross-module refinement checking**: `check_refinements` now scans prelude programs so calls to package functions with `where` clauses are fully verified
- **Cross-module IFC boundary detection**: IFC pass recognises prelude functions with labeled params called from user code, enabling 11/11 assurance for sqlite example
- **`RefinementCounts.fn_total`/`fully_verified_fns`**: accurate per-function verification statistics

### Fixed
- Assurance Req 9/10/11 summary rows now use prover verdict detail strings, eliminating mismatch between summary table and Prover Verdicts section
- `mvl assurance` loads `pkg.*` modules to resolve types (mirrors `mvl check`)
- `mvl test` uses stable `CARGO_TARGET_DIR` per source path to avoid recompilation on every run
- `cross_backend_net_basic` marked `#[ignore]` pending actor concurrency fix (#826)

## [0.120.0] — 2026-05-16

### Added
- **`std.toml`**: pure MVL TOML parser — `toml_encode`/`toml_decode`, `TomlValue` enum (TStr, TInt, TFloat, TBool, TDateTime, TArray, TTable), 36 tests (#819)

## [0.119.1] — 2026-05-16

### Fixed
- Add missing doc comments to `IoError`, `NetError`, `ProcessError`, and `RegexError` variants in `mvl_runtime` — silences `missing_docs` warnings that polluted stderr and caused `log_output_formats_correctly` to fail (#813)

## [0.119.0] — 2026-05-16

### Added
- Builtin rewrite rules for Layer 3 symbolic execution: 17 rules for String `.len()`/`.is_empty()`, List `.len()`, Option `.is_some()`/`.is_none()`, and Result `.is_ok()`/`.is_err()` — enables Layers 1/2 to prove predicates previously requiring runtime checks (#596, #791)

## [0.118.0] — 2026-05-16

### Added
- **Rust 2018 sibling-file module style**: directory module entry points now use `foo.mvl` (sibling file) instead of `foo/mod.mvl` — improved editor UX and consistency with Rust 2018 convention (#794)
- Two-step module resolution: prefer sibling file, fall back to `mod.mvl` with deprecation warning for one release cycle
- `loader::find_module_file()` function implementing new resolution order with fallback logic
- `loader::stem()` correctly derives module names from directory for legacy `foo/mod.mvl` paths
- ADR-0033: Rust 2018 sibling-file module style decision and deprecation plan
- Updated spec 005 with new module resolution order and three scenarios (single-file, sibling preferred, legacy deprecated)

## [0.117.0] — 2026-05-16

### Added
- Builtin SMT axioms for Z3 Layer 5 fallback: `len(self)` axioms for string/list length predicates, non-negativity axioms, and string literal grounding (#597, #792)
- Layered refinement solver dispatch with Z3 fallback and CLI flags `--refinement-solver` and `--refinement-stats` (#594, #796)

## [0.116.0] — 2026-05-16

### Added
- Stdlib structured error enums: `NetError`, `IoError`, `RegexError`, `JsonError`, `ProcessError` replacing `Result[T, String]` across all stdlib modules (#782)
- `LlvmEnumError` ABI struct for LLVM runtime enum error encoding
- ADR-0032: Stdlib structured error enums

## [0.115.0] — 2026-05-16

### Added

- **std.net TCP stdlib** (#779) — TcpListener and TcpStream types with tcp_listen, tcp_connect, tcp_accept, tcp_read, tcp_write, tcp_listener_port, and close functions; implemented for both Rust transpiler and LLVM backends via C-ABI FFI; includes error handling for bind failures, connection refusal, and invalid addresses; cross-backend integration test using actor spawn.

---

## [0.114.0] — 2026-05-15

### Added

- **Phase 8 compiler architecture refactor** (#774) — complete restructuring of the monolithic 4000-line main.rs into layered, composable modules:
  - `Loader` module (#766) — unified file loading with 10 extracted functions (parse, stdlib, packages).
  - `Pipeline` abstraction (#767) — orchestrator for Loader → Checker → Transpiler phases with composable instrumentation.
  - `TranspileConfig` builder (#768) — consolidates 20+ transpile_* variants into single `transpile(prog, config)`.
  - CLI command extraction (#770) — split monolithic main.rs into 13 focused modules (check, build, test, mcdc, mutate, etc.).
  - Main.rs dispatch (#771) — reduced from 4000 to 55 lines; version resolution chain (ADR-0009).
  - Documentation updates (#772) — module structure, public API docs, tests passing (890 unit + 366 integration).

### Fixed

- **Library design** — `parse_or_exit` moved from library to CLI layer; library now exposes pure `parse_file() -> Result<…>`.
- **Symlink escape** — `collect_mvl_files_recursive` now uses `entry.file_type()` (lstat) instead of `path.is_dir()` (follows symlinks).
- **Error handling** — `copy_dir_recursive` skips symlinks; build.rs uses structured error output instead of `panic!`.
- **JSON escaping** — `json_escape` now handles U+2028 and U+2029 (Unicode line terminators).
- **Type encapsulation** — `TranspileConfig` fields now `pub(crate)` to enforce builder-only construction.

### Changed

- `CoverageVisitor::branch_count()` renamed to `next_counter_id()` — clearer semantics (returns `start_id + allocated`, not count).
- `Pipeline::build()` documented as single-file-only; for multi-file coverage, use `TranspileConfig::with_coverage(offset)` directly.

---

## [0.113.0] — 2026-05-15

### Added

- **Counterexample infrastructure** (#627) — `RefResult::Failed` now carries `Option<String>` counterexample propagated through all 5 solver layers and all error types (`RefinementViolated`, `PreconditionViolated`, `PostconditionViolated`, `InvariantViolated`).
- **LLVM requires-clause runtime guards** (#627) — LLVM backend emits `llvm.trap` (Always/DebugOnly) or `llvm.assume` (Assume mode) for `requires` predicates at function entry, matching the Rust backend's `assert!` guards.
- **Session type model checker** (#134) — duplicate branch label detection (`SessionDuplicateLabel`) and mutual-blocking deadlock detection (`SessionDeadlock`) for session type declarations.
- **Actor protocol bounded model checker** (#37) — field refinement checking at `spawn` sites and full refinement/contract checking inside actor behavior bodies.

### Fixed

- `check_actor_field_refinements`: seed `var_refs` per-body from function parameters so parameter where-refinements are available as solver hypotheses.
- `count_fully_verified_fns`: actor behavior methods now included in assurance coverage reports.
- `layer5.rs`: removed spurious `get_model()` call in the Sat branch.

---

## [0.112.0] — 2026-05-15

### Added

- **Spec 018 — Layered Refinement Solver** — documents the 5-layer proof architecture (trivial → intervals → symbolic → Cooper's QE → Z3) for `where` predicate verification; links all 9 sub-tickets to epic #545.

---

## [0.111.0] — 2026-05-15

### Added

- **Mutation testing for actor checker and codegen** (#703) — cargo-mutants integrated into CI and `make setup`; actor checker and LLVM actor IR covered by mutation test suites.

### Fixed

- CI: fetch base SHA before PR diff to fix "Detect changed paths" (#703).
- Move LLVM actor IR tests to transpiler suite; drop `cross_backend` from mutants-actors (#703).
- Install `cargo-mutants` in `make setup`, drop manual guards (#703).

---

## [0.110.1] — 2026-05-15

### Fixed

- Remove stale Phase 6 annotations from Req 10/11 assurance messages.


## [0.110.0] — 2026-05-15

### Added

- **Closure lowering on LLVM backend** (#588) — lambdas can now capture variables from enclosing scopes and be passed as first-class values to higher-order functions (`filter`, `map`, `fold`, etc.). Universal closure struct representation `{ fn_ptr, env_ptr }` with trampoline calling convention; non-capturing lambdas use null `env_ptr`; capturing lambdas use stack-allocated environment structs. All three HOF scenarios (filter/map/fold) verified via cross-backend test parity.

### Fixed

- Closure capture analysis: let-bound names in lambda body now properly shadow outer bindings (C2).
- Closure capture analysis: else-if chains arbitrarily deep now properly walked for captures (C1).
- Closure capture analysis: function-typed variables used as callees now included in captures (C3).
- Wrapper function generation: type mismatch now fails loudly (unreachable) instead of silently returning undefined (W1).

## [0.109.0] — 2026-05-15

### Added

- **`std.args.parse[T]()`** — struct-driven CLI argument parsing. The struct IS the argument spec: `Positional[T]` fields parse leading argv tokens, `Bool` fields become presence flags, `Option[T]` fields are optional named flags, all other fields are required named flags. Auto-generates `-h/--help` usage. Defaults via `Option[T]` + `.unwrap_or(default)`. (`#752`)
- `unwrap_or_exit<T>()` in the args runtime — prints error to stderr and exits 1 on `Err`, providing uniform CLI error handling.

## [0.108.0] — 2026-05-15

### Added

- **Actor pingpong example** — End-to-end Phase 8 actor model demonstration: two actors (`Ping`, `Pong`) exchanging messages for a configurable number of rounds. Demonstrates `actor` keyword, `pub fn` behaviors, `tag`/`val` capabilities, `concurrently {}` structured concurrency, and `Tainted[String]` sanitization for CLI args. Achieves 11/11 assurance requirements (#580).
- Rust codegen fixes for actor creation expressions and `concurrently {}` blocks so `make run` works end-to-end.
- Transpiler unit tests for actor state `_self_ref` field, spawn init, helper call prefix, and self-as-tag-handle.

## [0.107.1] — 2026-05-15

### Fixed

- Missing `DuplicateActorField`, `DuplicateActorMethod`, and `NonUnitBehaviorReturn` variants in `CheckError` enum that were emitted by actor checker but not defined, causing compile error after session types merge (#745).

## [0.107.0] — 2026-05-15

### Added

- **Phase 8 Session Types (Honda 1993)** — First-class typed communication protocols. Session types (`!T.S`, `?T.S`, `+{l:S,...}`, `&{l:S,...}`, `end`) describe the exact sequence of messages exchanged on a channel. Compiler verifies both sides follow the declared protocol; missing/wrong/out-of-order messages are compile errors. Full duality support: `dual(S)` flips `!`↔`?` and `+`↔`&`. Includes well-formedness checking, error reporting, tree-sitter grammar, comprehensive tests, and specification (#260).

## [0.106.0] — 2026-05-15

### Added

- **Req 9 Data Race Freedom upgrade to Proven** — Phase 3 ref-escape-to-spawn check closes final concurrent escape path for `ref` parameters. Three interlocking layers now guarantee data race freedom: (1) type checker rejects `channel.send(ref)`, (2) type checker rejects actor `pub fn(ref param)`, (3) new check rejects `actor ActorType { field: ref_var }`. When all three layers pass, the pass returns `Proven` instead of `Unchecked` (#723).

## [0.105.0] — 2026-05-14

### Added

- **Phase 8 Actor Runtime (Rust backend)** — Full actor infrastructure: `{Name}State` struct, `{Name}Msg` enum, dispatch loop, fire-and-forget method wrappers, thread spawning via `std::sync::mpsc::sync_channel(256)` (#695).
- **Phase 8 Actor Runtime (LLVM backend)** — C-ABI runtime functions (`mvl_actor_spawn`, `mvl_actor_send`, `mvl_actor_drop`) for standalone LLVM IR execution; behavior functions with dispatch switch (#696).
- **Actor sendability enforcement** — Type checker validates that `pub fn` behavior parameters carry only sendable capabilities (`iso`, `val`, `tag`, or unannotated); rejects `ref` at declaration time (#506).
- **Actor grammar & tree-sitter** — Full actor syntax in EBNF and tree-sitter: actor declarations with fields and methods, `pub fn` async behaviors, `fn` private helpers, `actor Expr` creation expressions (#63, #706).
- **Select expression and concurrently block** — AST nodes and parsing for structured concurrency: `select { arm => { } timeout(dur) => { } }` and `concurrently { }` scope blocks (#69).
- **ADR-0029** — Documented architectural decisions behind Pony's reference capability adaptation for MVL: capability set, iso recovery, Capability/TypeExpr split, cross-backend applicability, Phase 3/8 boundary.
- **Spec 015** — Complete actor model specification covering 9 requirements: declaration syntax, behavior semantics, spawn/lifecycle, iso ownership transfer, sendability rules, actor isolation, ActorRef tag semantics, structured concurrency scope lifetimes, select with timeout.
- **Safety hardening** — Null/negative-size guards in LLVM runtime (`mvl_actor_spawn`, `mvl_actor_send`, `mvl_actor_drop`); codegen-time MAX_ARGS enforcement; iso aliasing checks extended to actor method bodies.

### Fixed

- **Select type inference** — Returns `Ty::Unit` (not `Ty::Unknown`), aligning with spec 015 §8.
- **Tag capability sendability** — Aligned `check_send_capability` with ADR-0029: `tag` is sendable (identity-only reference); only `ref` is rejected.
- **LLVM dispatch function preamble** — Added missing `local_mvl_types.clear()` to prevent stale type bindings from leaking between behaviors.
- **State size casting** — Fixed double-cast `usize→i64→u64` to direct `usize→u64` in `emit_actor_spawn`.

### Known Gaps (Tracked)

See issues #742–#745 for remaining Phase 8 work:
- Actor body type-checking (method bodies never inferred) (#742)
- Select/concurrently codegen (AST only, no executable output) (#743)
- Actor type registration in type env (spawn returns unparameterized `ActorRef`) (#744)
- Actor checker completeness (duplicate names, non-Unit behavior return) (#745)

## [0.104.0] — 2026-05-14

### Added

- `examples/snake_game` — Complete Snake game example demonstrating MVL's core thesis: pure game logic in `game.mvl` (zero effects, fully testable) with an effectful I/O shell (`main.mvl`, `render.mvl`). Demonstrates R1 (ADTs), R3 (Totality), R4 (Null), R7 (Effects), and R10 (Refinements) with 31 unit tests (#175).
- 3-life system for snake_game with retry on death, accumulated score tracking, and "game over" screen.
- `make assurance` target in examples/snake_game Makefile — runs `mvl assurance game.mvl` to verify pure game logic meets 8/11 requirements.

### Fixed

- Effect annotation syntax: `! A, B` → `! A + B` (comma was never valid; use `+` to combine multiple effects).

## [0.103.0] — 2026-05-14

### Added

- **MC/DC EXEMPT tier** — Automatically classify decisions in effectful functions as `! effects` exempt from unit-test coverage requirements; reporting distinguishes pure obligations (unit-testable) from exempt obligations (integration-testable only) (#737).
- `is_effectful: bool` field in MC/DC `DecisionInfo` struct to track whether a decision occurs in a function with `! Effect` annotations (#737).
- Per-file error handler refactoring pattern in `examples/log_analyzer/main.mvl`: pure `run_error_message()` function mapping error variants to strings, separate from effectful `handle_run_error()` with tight `! Log` effect boundary (#737).
- Help flag (`-h`, `--help`) to `examples/test-all.sh` script for improved usability (#737).

### Changed

- **MC/DC reporting** — Header line now shows: `Found X test file(s), Y compound decisions (N pure, M exempt), Z pure obligations` instead of total decision count; coverage summary shows `MC/DC coverage: Z/Z pure obligations met (100%)` (#737).
- **MC/DC verbose output** — New EXEMPT section displays decisions in effectful functions with `[— —]` markers and `IO-BOUNDARY` label (#737).

## [0.102.0] — 2026-05-14

### Added

- `docs/style.md`: `.mvl` file documentation convention guide covering module headers (`//!`), item docs (`///`), requirement references, and inline comments (#727)
- Early `--help` / `-h` check in CLI: `mvl check --help` now prints usage and exits 0 instead of treating `--help` as a path (#728)
- Verbose output for `mvl check --verbose`: per-requirement ✓/✗/~ verdict breakdown per file, plus stdlib-profile line (#728)

### Changed

- `path_arg_index()`: now correctly skips leading `--flag` arguments when locating the positional path argument, enabling `mvl check --verbose compiler/` and similar usage patterns across all subcommands (#728)
- `cmd_check()` signature: added `verbose: bool` parameter to thread verbose flag through from CLI (#728)
- All 15 stdlib `.mvl` files: module headers converted from `// MVL standard library —` to `//! std.X —` format with canonical Import and Effects fields (#727)

## [0.101.0] — 2026-05-14

### Added

- `RefinementsPass` now returns `Proven` when all functions with refinements are fully verified, with per-function coverage evidence (#733)
- `invariants: Vec<RefExpr>` field on `Stmt::For` AST node; parser handles `invariant pred*` clauses in for-loops (#733)
- `count_fully_verified_fns(prog)` helper for aggregating SMT verdicts by function (#733)

### Changed

- `RefinementsPass::run()` verdict: `Proven` when all functions fully verified, `Unchecked` with per-function coverage otherwise (#733)

## [0.100.0] — 2026-05-14

### Added

- `missing-totality` lint rule flags functions with no explicit `total`/`partial` keyword; enabled via `require_explicit_totality = true` in `.mvllintrc` (#729)
- `make assure-compiler` target runs the assurance report for the self-hosted compiler in verbose mode
- EBNF named productions for `contract_clause`, `ghost_let_stmt`, `decreases_expr`, `forall_expr`, `exists_expr` matching tree-sitter grammar rules

### Changed

- `mvl assurance` now uses cross-file user prelude for multi-file projects, matching `mvl check` behaviour (#732)
- Assurance report shows correct verdict categories (proven ✓ / not proven – / violated ✗), split explicit vs implicit total fn count, and files-found vs files-checked (#729–#731)
- `mvl lint` reports lex/parse errors as diagnostics instead of aborting
- `make check-compiler` now also runs `mvl lint compiler/`

### Fixed

- `mvl assurance` false positives on multi-file projects due to missing cross-file prelude (#732)
- `make test-grammar-coverage` failure caused by 5 undocumented tree-sitter rules added by decreases/proof commits

## [0.99.0] — 2026-05-14

### Changed

- **Req 2 Memory Safety Phase 3 completion** — upgrade from `Unchecked` to `Proven` verdict when all borrow scope, aliasing, and use-after-move checks pass. All underlying checks (Phase C scope-depth analysis, `AliasingMutableBorrow`, `DoubleMutableBorrow`, `UseAfterMove`) were already implemented; only the pass verdict needed updating (#722).

## [0.98.1] — 2026-05-13

### Fixed

- **MC/DC coupling detection false positives** — interprocedural field-sensitivity analysis now resolves bare-variable call-site arguments to the actual field paths each callee reads, so clauses like `f(p) || g(p)` where `f` reads `p.x` and `g` reads `p.y` are no longer incorrectly coupled (#562).

## [0.98.0] — 2026-05-13

### Added

- **`if let` syntax** — `if let Pat = expr { body }` desugars to `Stmt::Match` at parse time, enabling single-arm Option/Result binding without full match expressions (#704).
- **Linter rule L042: for-iter-antipattern** — error-level diagnostic when code uses `while`/`.get(i)`/`match`/`None ⇒ ()` instead of `for x in list`; escape hatch when the `None` arm contains real logic (#705).
- **Keyword validation tooling** — `tools/validate_keywords.py` cross-checks keyword lists across EBNF grammar, tree-sitter grammar, `compiler/lexer.mvl`, and the Rust lexer; `make validate-keywords` target and CI step added (#706).
- **Tuple destructuring in for-in loops** — `for (a, b) in pairs` now emits LLVM GEP field extraction via `emit_for_list_tuple()`; supports wildcard patterns (#710).
- **Corpus tests** — `tests/corpus/01_basics/if_let.mvl`, `for_tuple_pattern.mvl`, `tests/corpus/03_linting/for_iter_antipattern.mvl`.

### Changed

- **`if_stmt` grammar** — `docs/grammar.ebnf` and `etc/tree-sitter-mvl/grammar.js` updated to include `if let` variant.
- **Self-hosted compiler** — `compiler/ast.mvl` and `compiler/lexer.mvl` gain missing `KwWith`, `KwGhost`, `KwDecreases`, `KwForall`, `KwExists` token variants.
- **Makefile targets** — `test-backend-mvl` renamed to `test-mvl`; `test-llvm` renamed to `test-backend-llvm`; pre-commit hook updated accordingly.

## [0.97.7] — 2026-05-13

### Added

- **Spike tests README** — `tests/spikes/README.md` documents spike test status, manual invocation, and guidance for adding new spikes (#683).

## [0.97.6] — 2026-05-13

### Added

- **Solver layer test corpus** — 34 new `.mvl` test files across `tests/solver/layer1`–`layer5` and `tests/solver/cross_layer`, expanding dedicated solver coverage from 19 to 53 tests. Each layer exercises distinct patterns (equality hypotheses, interval arithmetic, symbolic paths, Fourier-Motzkin, Z3 chains, and runtime fallback) (#684).
- **LLM-generated corpus infrastructure** — `tests/corpus/llm_generated/` directory with YAML schema, README, and analysis templates for collecting and categorising LLM-authored programs and self-healing attempt records (#685).
- **Spike tests README** — `tests/spikes/README.md` documents spike exclusion from CI and provides manual invocation instructions (#683).

### Fixed

- **Effect-list parser accepts `+` separator** — `compiler/parser.mvl` now accepts `! Eff1 + Eff2` in addition to comma-separated effects; fixes `parser::tests::fn_with_multiple_effects`.
- **Pre-commit hook target name** — `.githooks/pre-commit` referenced `make test-mvl` which does not exist; corrected to `make test-backend-mvl`.

## [0.97.5] — 2026-05-13

### Fixed

- **Higher-order function effect propagation** — Caller must now declare all effects of higher-order function parameters, enforcing Req 7/8. Validates parameter effect lists before call site inference (#676).
- **Linear type enforcement for `consume()` parameters** — Enforce destructive-read semantics for `iso` and `val` parameters, rejecting non-consume operations on linear types in function arguments. Closes linear-type gap tracked in #691.
- **Const-generic `N` type resolution** — Const-generic `N` now resolves to `UNKNOWN` instead of `Named("N")` to allow polymorphic instantiation across generic call sites. Type::Fn now expands effects list for concrete call-site validation (#687).
- **Cargo `publish` unsafe warning** — `cargo-gen` emits `PUBLISH-UNSAFE` comment for path and unversioned dependencies, signaling unsafe publish attempts (#679).

## [0.97.4] — 2026-05-13

### Fixed

- **nvim-mvl install** — Global XDG pack install (`~/.local/share/nvim/site/pack/`), sentinel-based idempotent `init.lua` wiring, backup before edits, `nvim` presence check moved before any filesystem writes, XDG path validation (#669).
- **Tree-sitter highlights** — Removed stale `mut`, `move`, `bitxor_op`, `module_decl` nodes; added `impl`, `extern`, `builtin`, `transparent`, `with`, `invariant` keywords; scoped `!` operator highlight to `unary_expr` to avoid false-matching effect-list separator (#669).
- **Tree-sitter grammar** — Added `word` property, `unary_expr` named node, optional `;` in `use_decl`/`reexport_decl`, `::` path separator in `module_path` (#669).
- **Pre-commit hook** — Upgraded to `set -euo pipefail`; added `make test-tree-sitter` trigger for grammar/query file changes (#669).
- **Compiler lexer** — Removed stale `mut` and `move` keyword entries from `keyword_kind()` (#669).
- **Effect-list grammar ambiguity** — Switched effect separator from `,` to `+` to restore LL(1) parsing. The comma had created a local LL(k>1) ambiguity in fn-type expressions where the parser couldn't determine at `,` whether the next identifier was another effect name or a function parameter. Using `+` (`! Effect1 + Effect2`) eliminates the ambiguity with zero lookahead since `,` remains the sole parameter/tuple separator everywhere. Grammar documentation (EBNF, Tree-sitter) and all test/example files updated (#712, closes #711).
## [0.97.3] — 2026-05-13

### Added

- **Test coverage matrix and gap analysis** — `tests/COVERAGE.md` maps all 102 corpus files to 11 ADR-0001 requirements with coverage statistics and recommendations for closing gaps (#677).
- **20 negative corpus programs** — Comprehensive negative test suite for Requirements 1–10 in `tests/corpus/13_negative/req{01-10}/`, validated by `make test-corpus` via `corpus:expect-fail` annotation (#680).

### Changed

- **Test directory reorganization** — Separated concerns: `tests/corpus/03_stdlib/*.mvl` → `tests/stdlib/`, `tests/corpus/11_programs/*` → `examples/programs/`, corpus directory renumbering (04_linting→03_linting, 12_bdd→11_bdd, 13_contracts→12_contracts, 14_negative→13_negative) (#694).
- **Makefile** — Renamed test suites to clarify backends: `test-transpiler` → `test-backend-rust`, `test-mvl` → `test-backend-mvl`; added `examples/programs/Makefile` for showcase program validation.
- **Spec cross-references** — Added Design Principles 4–10 cross-references to existing requirements in specs 001, 002, 003 for traceability (#427).
- **Type checker** — Deleted 6 redundant stdlib smoke tests (now covered natively by `make test-corpus`); updated 48 test file paths for directory reorg.

### Fixed

- **`make test-corpus` on macOS** — Replaced bash globstar `**/*.mvl` (unsupported in macOS `/bin/bash` 3.2) with `find` + process substitution; also caught 3 previously-missed nested test files in corpus subdirectories.

## [0.97.2] — 2026-05-13

### Fixed

- **Stale Rust/`mut` references in specs** — Replaced `let mut x`, `mut self`, `mut field` with Pony-style capability equivalents (`let x: ref T`, `ref self`, `ref field`) in type-system and parser specs; fixed language.md statement syntax table; corrected `mvl_rationale.md` framing from "Pony + Rust's ownership" to "Pony's deny capabilities" (#692, part of #669).

## [0.97.1] — 2026-05-13

### Fixed

- **LLVM backend silently ignores `with invariant`** — `register_type_decl` now stores invariants and `emit_construct` emits a conditional branch to `llvm.trap` on violation. Enables cross-backend parity with the Rust backend (#670).
- **`assert_eq` covert channel for Secret/Tainted arguments** — Added `assert_eq` and `assert_ne` to the IFC label guard; assertion failures expose their arguments to stderr (#671).
- **Split enforcement model for `requires`/`ensures`** — Promoted from `debug_assert!` to `assert!`, matching the `assert!` enforcement already used for struct `with invariant` and field refinements since v0.97.0 (#672).

## [0.97.0] — 2026-05-12

### Added

- **Struct-level invariants (`with invariant`)** — SPARK-style cross-field predicates for structs. Syntax: `type Stack = struct { size: Int, capacity: Int } with invariant self.size <= self.capacity`. Checked at construction via `assert!` in the Rust backend; LLVM support planned (#662). Closes #654.

### Fixed

- **ParseFromArgs bypass of struct invariants** — CLI argument parsing now routes through `Self::new()`, ensuring invariants are always enforced.
- **Missing identifier validation on FieldAccess predicates** — Added `assert_safe_identifier()` guard before code generation interpolation.
- **EBNF `ref_atom` documentation** — Updated to document the new `IDENT { "." IDENT }` field-access form.

### Changed

- **Refinement and invariant checks upgraded from `debug_assert!` to `assert!`** — Ensures enforcement in release builds. See #662 for planned `AssertMode` (configurable Rust/LLVM enforcement levels).

## [0.96.0] — 2026-05-12

### Changed

- **Phase D capability state machine now driven by implicit borrows** — The `CapabilityState` state machine in the type checker now enforces reference aliasing rules on implicit borrow assignments (`let v: val T = x` / `let r: ref T = x`), not just explicit borrow expressions (`let v: val T = val x` / `let r: ref T = ref x`). Improves error detection for capability violations in real-world code. Closes #660.

## [0.95.0] — 2026-05-12

### Changed

- **Removed `mut` and `move` keywords** — Mutability and ownership transfer are now encoded exclusively through Pony-style capabilities (`iso`, `val`, `ref`, `tag`). Bindings use `let x: ref T` for mutability instead of `let mut x: T`; function parameters use `ref param: T` instead of `mut param: T`; expressions use `consume(x)` for ownership transfer instead of `move(x)`. All three backends (Rust, LLVM, Cranelift) and type checker updated. Closes #653.

### Technical Details

- **Type-level `ref` marker**: `ref T` in type annotations encodes mutability at the type system level
- **Environment type stripping**: Bindings store stripped inner type in environment for simplicity; type checking uses transparent `Ty::Ref` case for compatibility
- **Ownership transfer via `consume()`**: Replaced `Expr::Move` with `Expr::Consume` using mark-moved semantics
- **Lexer/AST cleanup**: Removed `TokenKind::Mut`, `TokenKind::Move`, `mutable: bool` field from AST nodes, `LetKind::Regular { mutable }` simplified to `LetKind::Regular`
- **Parser updates**: All keyword parsing for `mut`/`move` removed; parameter/field/let declarations now use only capability annotations
- **Type checker**: Added mutability derivation from `Ty::Ref(true, _)` or capability (`Capability::Ref`/`Iso`); binding type stripping ensures correct type lookup
- **All tests updated**: 1582 tests passing; corpus files, stdlib, and transpiler tests refactored to new syntax

## [0.94.0] — 2026-05-12

### Added

- **Function contracts Phase 5: loop verification** — `while` loops now accept `invariant` and `decreases` clauses; the checker verifies invariant preservation and termination (decreasing metric). Closes #628.
- **Quantifiers in refinements (`forall`/`exists`)** — New `RefExpr::Forall` and `RefExpr::Exists` AST nodes; Z3 solver encodes universal and existential quantifiers for contract verification.
- **Hard-reserved contract keywords** — `requires`, `ensures`, `ghost`, `invariant`, `decreases`, `forall`, `exists` are now reserved identifiers; stdlib `io.exists` renamed to `io.path_exists` to avoid conflict.
- **Grammar EBNF updated** — `docs/grammar.ebnf` extended with all Phase 3–5 productions and a reserved-keyword reference section.
- **ADR-0025 updated** — Hard-keyword decision documented with rationale and migration example.
- **ADR-0004 keyword count updated** — Target revised from ~25 to ~45 keywords; growth justified by verification-density policy.

## [0.93.0] — 2026-05-11

### Added

- **Function contracts Phase 4: cross-backend runtime assertion emission** — Rust and LLVM backends now emit `debug_assert!` for `requires` clauses at function entry and `ensures` clauses at return points, catching RuntimeCheck violations at runtime. Closes #627.
- **Ghost bindings (`ghost let`)** — Specification-only declarations that are type-checked at compile time but erased before transpilation/codegen. Complement explicit refinements with informal documentation.
- **Entry-time value capture in postconditions (`old(e)`)** — New `RefExpr::Old` syntax in `ensures` predicates captures parameter values at function entry (currently uses conservative current-value emission; full register allocation deferred to future phase).
- **LetKind enum for unrepresentable invalid states** — Replaced `mutable: bool, ghost: bool` pair on `Stmt::Let` with `kind: LetKind { Regular { mutable }, Ghost }`, making the invalid state `ghost + mutable` unrepresentable at the type level (#651).

### Fixed

- **LLVM backend ghost erasure** — Added missing `Stmt::Let { kind: LetKind::Ghost, .. }` guard to prevent ghost bindings from being emitted as real LLVM locals.
- **Labeled return types with ensures clauses** — `emit_expr_tail_with_return_type` now called in `has_ensures` branch to preserve security-label wrapping for functions with postconditions.
- **Format string injection risk in debug_assert messages** — Predicate strings in `debug_assert!` messages now escape `{` and `}` to prevent malformed Rust format strings if future predicate forms emit braces.

## [0.92.1] — 2026-05-11

### Fixed

- **Security: validate `MVL_MEMORY_LIB` and `MVL_RUNTIME_C_LIB` paths** — Environment variable overrides for cdylib paths now reject any path that doesn't end in `.dylib` or `.so`, preventing accidental or malicious loading of arbitrary files into the `lli` interpreter process. Closes #454.

## [0.83.0] — 2026-05-08

### Added

- **Property-based testing stdlib module** — `std/pbt.mvl` implements Phase A (generators, combinators, property_check) and Phase B (mutation operators, targeted property checking) of #40 and #425. Five concrete generator types (IntGen, FloatGen, BoolGen, StringGen, ListIntGen) encode generation strategies as data. All function types are pure MVL atop `std.random.*` (Tier 3, no new C-ABI). Closes #555.

### Changed

- **Function pointer parameters emit as bare `fn` instead of `impl Fn`** — Matches enum variant field emission and ensures `Copy+Clone` compatibility for function-typed values stored in enum variants. Fixes type mismatch when user-defined functions with `List[T]` parameters are passed as callbacks to higher-order functions.
- **Prelude programs scanned for Rust-backed stdlib imports** — `emitter.rs` now includes stdlib imports from both user and prelude programs, enabling `std/pbt.mvl`'s `use std.random.*` to auto-generate `use mvl_runtime::stdlib::random::*` in transpiled output.


## [0.92.0] — 2026-05-10

### Added

- **Function contracts — Phase 1: requires/ensures** — `fn` declarations now accept `requires` (precondition) and `ensures` (postcondition) clauses. Preconditions validated at call sites via the 5-layer refinement solver (Layer 1 literal eval + tautology; Layer 2 interval arithmetic). Postconditions checked at return points with predicate normalization (`result → self`). Deferred: multi-parameter `requires` checking at call sites, parameter-aware `ensures` analysis. Closes #621 (Phases 1–3).

- **Function contracts — Phase 2: multi-param requires + parameter-aware ensures** — `requires` predicates with 2+ free variables now trigger `RuntimeCheck` (runtime assertion at call sites). Parameter-aware `ensures` clauses normalize to `self` and check parameter-ref constraints via the solver, with remaining multi-param predicates deferred to runtime. Enables precondition checking for range guards (`lo <= hi`) and postcondition checking tied to input values (`result == n`).

- **Loop invariants on while statements** — `while cond { invariant pred1; invariant pred2; ... body }` syntax now supported. Invariants are checked at loop entry using the 5-layer solver (constant predicates via Layer 1, single-variable predicates via Layer 2 with normalization to `self`). Multi-variable invariants trigger `RuntimeCheck`. Parameter-aware `where` refinements on loop variables are threaded into the solver context, enabling proofs like "invariant holds because input was constrained". Deferred: invariant preservation (loop condition + body must prove invariant maintained), loop termination checking (`decreases`), quantified invariants (`forall`/`exists`).

### Fixed

- **FnDecl constructor in lambda lowering** — Added missing `requires: vec![]` and `ensures: vec![]` fields when constructing `FnDecl` for lowered lambdas in `codegen/exprs.rs`. Fixes type mismatch after Phase 1 AST expansion.

## [0.91.1] — 2026-05-10

### Fixed

- **Stdlib dead-code stubs cleaned up** — Removed duplicate `pub fn print { }` and `pub fn eprint { }` in `std/core.mvl` (the real `pub builtin fn` versions already existed). Fixed `int_to_float` in `std/math.mvl` from dead stub `{ 0.0 }` to correct implementation `{ n.to_float() }`. Added clarifying comment to `digit_of` in `std/json.mvl`. Closes #547.

## [0.91.0] — 2026-05-10

### Added

- **`--stdlib=proven` wired into `build`, `run`, and `test`** — the proven-profile pre-flight check (`check_proven_stdlib`) now runs before all four commands (`check`, `build`, `run`, `test`). Previously it was only active for `mvl check`; the other three silently discarded the flag. Closes #533.

## [0.90.1] — 2026-05-10

### Fixed

- **CI z3-sys build on Linux** — `.cargo/config.toml` sets `Z3_SYS_Z3_HEADER=/opt/homebrew/include/z3.h` (macOS path) with `force=false`. Despite the name, `force=false` still applies the value when the variable is unset — which is always the case on Linux CI runners. Fix: CI now explicitly sets `Z3_SYS_Z3_HEADER=/usr/include/z3.h` after installing `libz3-dev`, so Cargo's guard correctly leaves it alone.

## [0.90.0] — 2026-05-10

### Added

- **Lambda lowering for LLVM backend (#421)** — Non-capturing lambdas (`|params| body`) are now emitted as top-level LLVM functions returning function pointers, enabling higher-order functions on the LLVM backend. Return type inferred from body's checker-inferred `Ty` when no explicit annotation present.
- **HOF method dispatch on LLVM backend (#421)** — `xs.filter(f)`, `xs.map(f)`, `xs.fold(init, f)`, `xs.any(f)`, `xs.all(f)`, `xs.find(f)`, `xs.take_while(f)`, `xs.skip_while(f)` now work via stdlib function monomorphization. Rewrites method calls to free-function calls with receiver prepended.
- **For-list iteration on LLVM backend** — `for x in <list>` implemented via `mvl_array_len` + `mvl_array_get` loop, supporting iteration over `MvlArray*` pointers.
- **Named function references as HOF arguments** — `emit_ident` falls back to `module.get_function(name)` to return function pointers for named functions passed as callbacks, enabling `xs.filter(is_even)` patterns.
- **`cross_backend_hof_lambdas` test** — New corpus test verifying filter, map, fold, any with both named functions and inline lambdas achieve output parity between Rust and LLVM backends. All 44 cross-backend tests pass.

### Fixed

- **`emit_fn_named` fallback return value** — Was always emitting `ret void` regardless of declared return type, causing LLVM IR verification errors for non-void monomorphized functions whose body emits no value. Now uses type-based zeroed return matching declared return type.

## [0.89.0] — 2026-05-09

### Added

- **Whole-program checking (#609)** — Cross-file function resolution: each source file is now checked with all other user modules as a prelude, enabling correct type checking of cross-file function calls. O(n²) AST cloning eliminated via `check_with_two_preludes`. Closes #609.
- **Cooper's algorithm refinement solver Layer 4 (#593)** — Presburger arithmetic: Fourier-Motzkin elimination + divisibility checks for linear inequality and divisibility predicates. Enables proofs like `n > 0 → n % 2 = 0 ∨ n % 2 = 1` without SMT. Closes #593.
- **Z3 SMT solver refinement Layer 5 (#543)** — Final dispatch layer using the `z3` crate for theorem proving with 1s timeout. Unique capability: cross-variable hypothesis chains (e.g., `x > 10, y > x` implies `y > 5`). Always on when built with `--features z3`; CI updated to install `libz3-dev`. Closes #543.
- **Example instrumentation** — All 7 example Makefiles now have `make test-solver` target showing per-file solver statistics with ✓/✗ status and summary pass/fail counts.

### Fixed

- **Transpiler spurious `.clone()` on rvalue arguments** — Removed unnecessary clones in `emit_expr_as_arg` fallback case; rvalue temporaries (function results, struct literals) that Rust moves into callees no longer generate redundant `.clone()`, eliminating 6 `unused_allocation` warnings in bzip example.
- **bzip example type mismatches** — Added `val` keyword to `encode_symbol` and `build_tree` calls to properly pass borrowed parameters, fixing parameter type mismatches introduced by recent transpiler changes.

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
