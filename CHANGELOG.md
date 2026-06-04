# Changelog

## [Unreleased]

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
