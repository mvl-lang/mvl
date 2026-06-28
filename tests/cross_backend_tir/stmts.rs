// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR walker parity tests — mirrors `emit_stmts.rs (+ heap-drop tracking in emit_types.rs)`.
//!
//! Each test re-uses the same `compile(...)` input as the corresponding
//! AST substring test in `emitter_tests/stmts.rs`, asserting that the
//! TIR walker emits byte-identical IR. When parity is achieved across the
//! corpus, PR 2 of #1612 flips the CLI to use the TIR path and deletes the
//! AST walker — at which point these tests become the primary coverage.

use super::common::{assert_tir_parity, assert_tir_unimplemented};

#[test]
fn let_binding_aliases_ssa_value() {
    assert_tir_parity("fn f(x: Int) -> Int { let y: Int = x; y }");
}

#[test]
fn mutable_ref_uses_alloca_store_load() {
    assert_tir_parity("partial fn counter(n: Int) -> Int {\
         let c: ref Int = 0;\
         while c < n {\
           c = c + 1;\
         }\
         c\
         }");
}

#[test]
fn string_local_emits_drop_before_ret() {
    assert_tir_parity("fn greet() -> Unit {\n\
         let s: String = \"hello\";\n\
         }");
}

#[test]
fn list_local_emits_drop_before_ret() {
    assert_tir_parity("fn nums() -> Unit {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         }");
}

#[test]
fn map_local_emits_drop_before_ret() {
    assert_tir_parity("fn maps() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         }");
}

#[test]
fn multiple_heap_locals_all_dropped() {
    assert_tir_parity("fn multi() -> Unit {\n\
         let s: String = \"hello\";\n\
         let xs: List[Int] = [1, 2];\n\
         }");
}

#[test]
fn primitive_locals_no_drop() {
    assert_tir_parity("fn prims() -> Unit {\n\
         let x: Int = 42;\n\
         let b: Bool = true;\n\
         }");
}

#[test]
fn explicit_return_emits_drops() {
    assert_tir_parity("fn early() -> Int {\n\
         let s: String = \"hello\";\n\
         return 42;\n\
         }");
}

#[test]
fn shadowed_string_local_no_double_drop() {
    assert_tir_parity("fn f() -> Unit {\n\
         let s: String = \"first\";\n\
         let s: String = \"second\";\n\
         }");
}

#[test]
fn ref_string_local_emits_load_then_drop() {
    assert_tir_parity("fn f() -> Unit {\n\
         let s: ref String = \"hello\";\n\
         }");
}
