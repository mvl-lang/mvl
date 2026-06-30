// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR walker parity tests — mirrors `emit_actors.rs`.
//!
//! Each test re-uses the same `compile(...)` input as the corresponding
//! AST substring test in `emitter_tests/actors.rs`, asserting that the
//! TIR walker emits byte-identical IR. When parity is achieved across the
//! corpus, PR 2 of #1612 flips the CLI to use the TIR path and deletes the
//! AST walker — at which point these tests become the primary coverage.

use super::common::assert_tir_parity;

#[test]
fn actor_emits_state_struct_and_behavior_fn() {
    assert_tir_parity(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }",
    );
}

#[test]
fn actor_emits_dispatch_function_with_switch() {
    assert_tir_parity(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
           pub fn reset() { }\n\
         }",
    );
}

#[test]
fn actor_runtime_externs_emitted() {
    assert_tir_parity(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }",
    );
}

#[test]
fn spawn_emits_alloca_and_actor_spawn_call() {
    assert_tir_parity(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }\n\
         fn main() -> Int {\n\
           let c: Counter = actor Counter { count: 0 };\n\
           0\n\
         }",
    );
}

#[test]
fn actor_method_call_emits_send() {
    assert_tir_parity(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }\n\
         fn main() -> Int {\n\
           let c: Counter = actor Counter { count: 0 };\n\
           c.increment(1);\n\
           0\n\
         }",
    );
}

#[test]
fn join_all_emitted_in_main_when_actors_present() {
    assert_tir_parity(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }\n\
         fn main() -> Int { 0 }",
    );
}
