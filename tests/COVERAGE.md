# Test Coverage — Corpus vs 11 Requirements

Maps every corpus test to the ADR-0001 requirements it exercises.
Requirements: 1=Type Safety, 2=Memory Safety, 3=Exhaustive Match, 4=Null Elimination,
5=Error Visibility, 6=Ownership, 7=Effects, 8=Termination, 9=Data Race, 10=Refinements, 11=IFC

| Test | Req 1 | Req 2 | Req 3 | Req 4 | Req 5 | Req 6 | Req 7 | Req 8 | Req 9 | Req 10 | Req 11 | Notes |
|------|:-----:|:-----:|:-----:|:-----:|:-----:|:-----:|:-----:|:-----:|:-----:|:------:|:------:|-------|
| **01_basics/** |||||||||||||
| 01_basics/args.mvl | ✓ | | | | | | | | | | | CLI args, fn signatures |
| 01_basics/bitwise.mvl | ✓ | | | | | | | | | | | Bitwise ops on Int/Bool |
| 01_basics/env_identity_llvm.mvl | ✓ | | | | | | ✓ | | | | | Env effect, identity fn |
| 01_basics/eprint_stderr.mvl | ✓ | | | | | | ✓ | | | | | Console effect, stderr |
| 01_basics/expressions.mvl | ✓ | | | | | | | | | | | Arithmetic expressions |
| 01_basics/functions.mvl | ✓ | | | | | | | ✓ | | | | Total fn declarations |
| 01_basics/keywords.mvl | ✓ | | | | | | | | | | | Reserved keywords parse |
| 01_basics/literals.mvl | ✓ | | | | | | | | | | | Int/Float/String/Bool |
| 01_basics/statements.mvl | ✓ | | | | | | | | | | | let, let mut, return |
| 01_basics/unix.mvl | ✓ | | | | | | ✓ | | | | | Process/Env effects |
| **02_types/** |||||||||||||
| 02_types/adt_checking.mvl | ✓ | | | | | | | | | | | Struct/enum ADT validity |
| 02_types/basic_types.mvl | ✓ | | | | | | | | | | | Int, Float, String, Bool |
| 02_types/bit_operators.mvl | ✓ | | | | | | | | | | | Bitwise type rules |
| 02_types/core_types.mvl | ✓ | | | | | | | | | | | Primitive types |
| 02_types/enum_match_llvm.mvl | ✓ | | ✓ | | | | | | | | | LLVM enum + exhaustive match |
| 02_types/enum_string_match_llvm.mvl | ✓ | | ✓ | | | | | | | | | String-tagged enum match |
| 02_types/enums.mvl | ✓ | | | | | | | | | | | Enum declaration + variants |
| 02_types/exhaustive_match.mvl | ✓ | | ✓ | ✓ | ✓ | | | | | | | All-variant match, Option, Result |
| 02_types/fn_takes_string_llvm.mvl | ✓ | | | | | | | | | | | String param LLVM path |
| 02_types/for_loop_llvm.mvl | ✓ | | | | | | | ✓ | | | | Bounded for-loop totality |
| 02_types/immutability.mvl | ✓ | | | | | ✓ | | | | | | let vs let mut |
| 02_types/move_string_llvm.mvl | | ✓ | | | | ✓ | | | | | | Move semantics, LLVM |
| 02_types/option_result.mvl | ✓ | | | ✓ | ✓ | | | | | | | Option/Result types |
| 02_types/overflow_checking.mvl | ✓ | | | | | | | | | ✓ | | Overflow as refinement |
| 02_types/parse_int_float_llvm.mvl | ✓ | | | | ✓ | | | | | | | Result from parse |
| 02_types/refinements.mvl | ✓ | | | | | | | | | ✓ | | Inline refinement predicates |
| 02_types/result_propagate_llvm.mvl | ✓ | | | | ✓ | | | | | | | ? propagation |
| 02_types/string_heap_llvm.mvl | ✓ | | | | | ✓ | | | | | | String ownership/LLVM |
| 02_types/struct_fields_llvm.mvl | ✓ | | | | | | | | | | | Struct field access LLVM |
| 02_types/structs.mvl | ✓ | | | | | | | | | | | Struct declaration |
| 02_types/unsigned_types.mvl | ✓ | | | | | | | | | ✓ | | u8/u16/u32/u64 bounds |
| 02_types/while_loop_llvm.mvl | ✓ | | | | | | | ✓ | | | | while in partial fn |
| **stdlib/** |||||||||||||
| stdlib/collections.mvl | ✓ | | | | | | | | | | | List, Map, Set |
| stdlib/crypto_operations.mvl | ✓ | | | | | | ✓ | | | | ✓ | Crypto effect + IFC labels |
| stdlib/json_operations.mvl | ✓ | | | | | | | | | | ✓ | JSON + taint tracking |
| stdlib/map_set_literals.mvl | ✓ | | | | | | | | | | | Map/Set construction |
| stdlib/pbt_operations.mvl | ✓ | | | | | | | | | | | Property-based testing stdlib |
| stdlib/random_operations.mvl | ✓ | | | | | | ✓ | | | | | Random effect |
| stdlib/range_pipeline.mvl | ✓ | | | | | | | | | | | Range + pipeline functions |
| stdlib/regex_find_all.mvl | ✓ | | | | | | | | | | ✓ | Regex + label safety |
| stdlib/regex_find.mvl | ✓ | | | | | | | | | | | Regex Option[Match] |
| stdlib/regex_operations.mvl | ✓ | | | | | | | | | | | Regex match/replace |
| stdlib/regex_replace.mvl | ✓ | | | | | | | | | | | Regex replace |
| stdlib/set_algebra.mvl | ✓ | | | | | | | | | | | Set union/intersection |
| stdlib/time_operations.mvl | ✓ | | | | | | ✓ | | | | | Time effect |
| **03_linting/** |||||||||||||
| 03_linting/complexity_demo.mvl | ✓ | | | | | | | | | | | Linter complexity rule |
| **04_ownership/** |||||||||||||
| 04_ownership/ownership.mvl | | ✓ | | | | ✓ | | | | | | consume(), move semantics |
| **05_effects/** |||||||||||||
| 05_effects/crypto_random_bytes_shape.mvl | ✓ | | | | | | ✓ | | | | | Crypto shape |
| 05_effects/crypto_random_bytes_zero.mvl | ✓ | | | | | | ✓ | | | | | Crypto zeros |
| 05_effects/crypto_sha256.mvl | ✓ | | | | | | ✓ | | | | | SHA-256 effect |
| 05_effects/declarations.mvl | ✓ | | | | | | ✓ | | | | | Effect declarations |
| 05_effects/env_basic.mvl | ✓ | | | | | | ✓ | | | | | Env effect |
| 05_effects/env_signal_ignore.mvl | ✓ | | | | | | ✓ | | | | | Signal handling |
| 05_effects/env_signal_on.mvl | ✓ | | | | | | ✓ | | | | | Signal callback |
| 05_effects/file_io.mvl | ✓ | | | | | | ✓ | | | | | File effect |
| 05_effects/io_basic.mvl | ✓ | | | | | | ✓ | | | | | Console + File |
| 05_effects/log_output.mvl | ✓ | | | | | | ✓ | | | | ✓ | Log effect + IFC label |
| 05_effects/logging.mvl | ✓ | | | | | | ✓ | | | | ✓ | Logging with labels |
| 05_effects/parametrized.mvl | ✓ | | | | | | ✓ | | | | | Parametrized effects |
| 05_effects/propagation.mvl | ✓ | | | | | | ✓ | | | | | Effect propagation to callers |
| 05_effects/pure_vs_effectful.mvl | ✓ | | | | | | ✓ | | | | | Pure fn vs effectful fn |
| 05_effects/random_choice.mvl | ✓ | | | | | | ✓ | | | | | Random effect |
| 05_effects/random_int.mvl | ✓ | | | | | | ✓ | | | | | Random int |
| 05_effects/random_shuffle.mvl | ✓ | | | | | | ✓ | | | | | Random shuffle |
| 05_effects/time_format_datetime.mvl | ✓ | | | | | | ✓ | | | | | Time format |
| 05_effects/time_format_instant.mvl | ✓ | | | | | | ✓ | | | | | Instant format |
| 05_effects/time_sleep.mvl | ✓ | | | | | | ✓ | | | | | Time sleep effect |
| **06_ifc/** |||||||||||||
| 06_ifc/declassification.mvl | ✓ | | | | | | | | | | ✓ | declassify() usage |
| 06_ifc/implicit_flow.mvl | ✓ | | | | | | ✓ | | | | ✓ | **Negative**: PC label violations |
| 06_ifc/label_types.mvl | ✓ | | | | | | | | | | ✓ | Public/Tainted/Secret types |
| 06_ifc/labels.mvl | ✓ | | | | | | | | | | ✓ | Label syntax + sanitize |
| 06_ifc/lattice.mvl | ✓ | | | | | | | | | | ✓ | Label lattice ordering |
| 06_ifc/propagation.mvl | ✓ | | | | | | | | | | ✓ | Label propagation through fns |
| **07_refinements/** |||||||||||||
| 07_refinements/refinements_valid.mvl | ✓ | | | | | | | | | ✓ | | Valid refinement predicates |
| 07_refinements/refinements_violations.mvl | ✓ | | | | | | | | | ✓ | | Unproven refinement (no `!`) |
| **08_termination/** |||||||||||||
| 08_termination/total_vs_partial.mvl | ✓ | | | | | | | ✓ | | | | total vs partial fn semantics |
| **09_concurrency/** |||||||||||||
| 09_concurrency/capabilities.mvl | ✓ | | | | | | | | ✓ | | | iso/val/ref capabilities |
| **10_verification/** |||||||||||||
| 10_verification/effect_ifc_interaction/main.mvl | ✓ | | | | | | ✓ | | | | ✓ | Effect × IFC cross-check |
| 10_verification/ownership_effects_interaction.mvl | ✓ | | | | | ✓ | ✓ | | | | | Ownership × effects |
| 10_verification/refinement_totality_interaction.mvl | ✓ | | | | | | | ✓ | | ✓ | | Refinements × totality |
| **examples/programs/** |||||||||||||
| examples/programs/auth_handler.mvl | ✓ | | | | ✓ | | | | | | ✓ | Auth: Result + IFC labels |
| examples/programs/box_field_deref.mvl | ✓ | | | | | ✓ | | | | | | Box field deref |
| examples/programs/bridge_ok/main.mvl | ✓ | | | | | | ✓ | | | | | Bridge + extern Rust |
| examples/programs/calculator.mvl | ✓ | | ✓ | | | | | ✓ | | | | Total fns, match |
| examples/programs/collections_basic.mvl | ✓ | | | | | | | | | | | List/Map/Set usage |
| examples/programs/core_types_demo.mvl | ✓ | | | | | | | | | | | Core type showcase |
| examples/programs/else_if_chain.mvl | ✓ | | | | | | | | | | | if/else-if/else |
| examples/programs/generic_fns.mvl | ✓ | | | | | | | | ✓ | | | Generic fn constraints |
| examples/programs/hello_mvl.mvl | ✓ | | ✓ | | | | | | | | | Enum match, total fn |
| examples/programs/hello_world.mvl | ✓ | | | | | | ✓ | | | | | Minimal: fn main + println |
| examples/programs/hof_lambdas.mvl | ✓ | | | | | | | | | | | Higher-order functions |
| examples/programs/linked_list.mvl | ✓ | | | | | ✓ | | ✓ | | | | Recursive enum, Box, total |
| examples/programs/password_checker.mvl | ✓ | | | | | | | | | | ✓ | IFC: password taint flow |
| examples/programs/println_non_string_first_arg.mvl | ✓ | | | | | | ✓ | | | | | println type safety |
| examples/programs/random_dice/main.mvl | ✓ | | | | | | ✓ | | | | | Random effect program |
| examples/programs/safe_division.mvl | ✓ | | | | ✓ | | | | | ✓ | ✓ | Result + refinement + IFC |
| examples/programs/shapes.mvl | ✓ | | ✓ | | | | | | | | | ADTs, multi-enum match |
| examples/programs/struct_value_semantics.mvl | ✓ | | | | | ✓ | | | | | | Clone-on-pass |
| **11_bdd/** |||||||||||||
| 11_bdd/calculator_bdd_test.mvl | ✓ | | ✓ | | | | | ✓ | | | | BDD tests, total fns |
| **12_contracts/** |||||||||||||
| 12_contracts/basic_contracts.mvl | ✓ | | | | | | | ✓ | | ✓ | | requires/ensures contracts |
| 12_contracts/ghost_old_contracts.mvl | ✓ | | | | | | | ✓ | | ✓ | | ghost variables, old() |
| 12_contracts/loop_verification.mvl | ✓ | | | | | | | ✓ | | ✓ | | Loop invariants |

---

## Coverage Summary

| Req | Name | Test Count | Files |
|-----|------|:----------:|-------|
| 1 | Type Safety | 102 | All files (type checking is universal) |
| 2 | Memory Safety | 3 | move_string_llvm, ownership, implicit_flow (via move) |
| 3 | Exhaustive Match | 9 | enum_match_llvm, enum_string_match_llvm, exhaustive_match, calculator, hello_mvl, shapes, calculator_bdd_test, 2× integration |
| 4 | Null Elimination | 2 | exhaustive_match, option_result |
| 5 | Error Visibility | 6 | exhaustive_match, option_result, parse_int_float_llvm, result_propagate_llvm, auth_handler, safe_division |
| 6 | Ownership | 8 | immutability, move_string_llvm, ownership, string_heap_llvm, box_field_deref, linked_list, struct_value_semantics, ownership_effects_interaction |
| 7 | Effect Tracking | 28 | All 05_effects/ + several integration files |
| 8 | Termination | 10 | functions, for_loop_llvm, while_loop_llvm, total_vs_partial, calculator, linked_list, refinement_totality_interaction, basic_contracts, ghost_old_contracts, loop_verification |
| 9 | Data Race | 2 | capabilities, generic_fns |
| 10 | Refinement Types | 8 | overflow_checking, refinements, unsigned_types, refinements_valid, refinements_violations, refinement_totality_interaction, safe_division, basic_contracts + 2× |
| 11 | IFC | 13 | All 06_ifc/ + crypto_operations, json_operations, logging, log_output, regex_find_all, auth_handler, password_checker, safe_division |

---

## Gap Analysis

### Critical gaps (≤3 tests)

| Req | Gap | Impact |
|-----|-----|--------|
| **2 — Memory Safety** | 3 tests, 0 negative tests | Use-after-move, double-free, dangling ref not exercised as full programs |
| **4 — Null Elimination** | 2 tests, 0 negative tests | Option misuse not exercised |
| **9 — Data Race** | 2 tests, 0 negative tests | iso/val/ref violations not exercised |

### Low coverage (4–6 tests)

| Req | Gap |
|-----|-----|
| **5 — Error Visibility** | No negative test for unchecked Result; ? propagation only tested via transpiler |
| **3 — Exhaustive Match** | No negative test (missing-arm programs) |
| **6 — Ownership** | Linear resource consumption not tested; `capture` mutability gap |

### No negative tests at all

Requirements 2, 3, 4, 5, 6, 8, 9, 10 had **zero negative corpus programs** before this audit. The initial batch of 20 is now in `tests/corpus/13_negative/` and is validated by `make test-corpus` via the `corpus:expect-fail` annotation. `tests/integration/error_messages/` covers individual error message quality but is not full-program corpus coverage.

---

## Recommendations for Next 20 Tests

Priority order — fill the largest gaps first:

| # | File | Req | Type | Rationale |
|---|------|-----|------|-----------|
| 1 | `13_negative/req02/use_after_move.mvl` | 2 | negative | First Req 2 negative program |
| 2 | `13_negative/req02/double_consume.mvl` | 2 | negative | iso consumed twice |
| 3 | `13_negative/req03/missing_arm.mvl` | 3 | negative | Non-exhaustive enum match |
| 4 | `13_negative/req04/option_unwrap.mvl` | 4 | negative | Field access on Option |
| 5 | `13_negative/req05/result_ignored.mvl` | 5 | negative | Unchecked Result |
| 6 | `13_negative/req06/reassign_immutable.mvl` | 6 | negative | Assign to let binding |
| 7 | `13_negative/req07/undeclared_effect.mvl` | 7 | negative | println without ! Console |
| 8 | `13_negative/req07/missing_propagation.mvl` | 7 | negative | Caller missing callee effect |
| 9 | `13_negative/req08/while_in_total.mvl` | 8 | negative | while in total fn |
| 10 | `13_negative/req08/partial_call.mvl` | 8 | negative | partial fn called from total |
| 11 | `13_negative/req09/send_ref.mvl` | 9 | negative | ref sent across actor |
| 12 | `13_negative/req09/iso_alias.mvl` | 9 | negative | iso aliased |
| 13 | `13_negative/req10/division_by_zero.mvl` | 10 | negative | Literal 0 to NonZero param |
| 14 | `13_negative/req10/predicate_false.mvl` | 10 | negative | where clause violated |
| 15 | `13_negative/req11/tainted_to_console.mvl` | 11 | negative | Tainted string to println |
| 16 | `13_negative/req11/secret_leak.mvl` | 11 | negative | Secret to public output |
| 17 | `02_types/move_sequence.mvl` | 2 | positive | Correct consume chain |
| 18 | `02_types/option_match.mvl` | 4 | positive | Full Option match pattern |
| 19 | `09_concurrency/iso_transfer.mvl` | 9 | positive | iso consumed and sent |
| 20 | `07_refinements/proven_refinement.mvl` | 10 | positive | Matching refinement proven |

---

## Cross-Backend Parity — `llvm_text` (#1154)

`tests/cross_backend.rs` enforces stdout parity between the Rust transpiler
and the `llvm_text` backend (post-ADR-0040, `--backend=llvm` resolves to
`llvm_text`). Helpers `assert_backends_agree` and `assert_parity` are
**strict**: a divergence is a test failure, not a logged warning.

### Helpers

| Helper | On `lli` missing | On llvm_text failure | Use case |
|--------|:---------------:|:--------------------:|----------|
| `run_llvm_text(file)` | `None` (skip) | **panic** | Default for new tests |
| `run_llvm_text_or_skip(file)` | `None` (skip) | `None` (logged skip) | Legacy callers, pre-existing known-broken paths |
| `assert_backends_agree(name)` | skip | **panic** | Compare full stdout against transpiler |
| `assert_parity(file, expected)` | skip | **panic** | Compare against pinned `expected` |
| `assert_llvm_output(file, expected)` | skip | **panic** | LLVM-only expected output |

### Known divergences (`#[ignore]`'d, surfaced by #1154)

| Test | Status | Notes |
|------|--------|-------|
| `cross_backend_collections_basic` | ✅ fixed | Added `_mvl_set_contains_i64` runtime + dispatch |
| `cross_backend_box_field_deref` | ✅ fixed | `Box::new`/`*box` codegen for primitive payloads |
| `cross_backend_list_ufcs_methods` | ✅ fixed | Added `slice`/`take`/`skip` dispatch via `_mvl_list_slice` |
| `llvm_move_string` | ✅ fixed | Dedupe heap_locals on consume (SSA already tracked) |
| `cross_backend_linked_list` | ❌ ignored | Requires non-unit enum payload lowering (`Cons(Int, Box[LL])` — match arms drop payload); separate epic |

Each ignored test carries a `reason` string identifying the symptom. New
divergences MUST be triaged the same way (ignored with reason, follow-up
issue filed) — never downgraded back to a soft `eprintln!` skip.
