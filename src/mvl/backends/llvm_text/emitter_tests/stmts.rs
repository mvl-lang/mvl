// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_stmts.rs (+ heap-drop tracking in emit_types.rs)` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/stmts.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

#[test]
fn let_binding_aliases_ssa_value() {
    let ir = compile("fn f(x: Int) -> Int { let y: Int = x; y }");
    assert!(ir.contains("ret i64"), "{ir}");
}

#[test]
fn mutable_ref_uses_alloca_store_load() {
    let ir = compile(
        "partial fn counter(n: Int) -> Int {\
         let c: ref Int = 0;\
         while c < n {\
           c = c + 1;\
         }\
         c\
         }",
    );
    assert!(ir.contains("alloca i64"), "{ir}");
    assert!(ir.contains("store i64"), "{ir}");
    assert!(ir.contains("load i64"), "{ir}");
    assert!(ir.contains("br i1"), "{ir}");
}

#[test]
fn string_local_emits_drop_before_ret() {
    let ir = compile(
        "fn greet() -> Unit {\n\
         let s: String = \"hello\";\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_string_drop(ptr"), "{ir}");
    assert!(ir.contains("declare void @_mvl_string_drop(ptr)"), "{ir}");
}

#[test]
fn list_local_emits_drop_before_ret() {
    let ir = compile(
        "fn nums() -> Unit {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_array_drop(ptr"), "{ir}");
    assert!(ir.contains("declare void @_mvl_array_drop(ptr)"), "{ir}");
}

#[test]
fn map_local_emits_drop_before_ret() {
    let ir = compile(
        "fn maps() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_map_drop(ptr"), "{ir}");
    assert!(ir.contains("declare void @_mvl_map_drop(ptr)"), "{ir}");
}

#[test]
fn multiple_heap_locals_all_dropped() {
    let ir = compile(
        "fn multi() -> Unit {\n\
         let s: String = \"hello\";\n\
         let xs: List[Int] = [1, 2];\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_string_drop(ptr"), "{ir}");
    assert!(ir.contains("call void @_mvl_array_drop(ptr"), "{ir}");
}

#[test]
fn primitive_locals_no_drop() {
    let ir = compile(
        "fn prims() -> Unit {\n\
         let x: Int = 42;\n\
         let b: Bool = true;\n\
         }",
    );
    assert!(!ir.contains("_drop"), "{ir}");
}

#[test]
fn explicit_return_emits_drops() {
    let ir = compile(
        "fn early() -> Int {\n\
         let s: String = \"hello\";\n\
         return 42;\n\
         }",
    );
    // The drop should appear before the ret instruction.
    assert!(ir.contains("call void @_mvl_string_drop(ptr"), "{ir}");
}

#[test]
fn shadowed_string_local_no_double_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let s: String = \"first\";\n\
         let s: String = \"second\";\n\
         }",
    );
    // Should have exactly 1 drop call (for the second binding only;
    // the first is removed from tracking when shadowed).
    let drop_count = ir.matches("call void @_mvl_string_drop(ptr").count();
    assert_eq!(drop_count, 1, "expected 1 drop, got {drop_count}\n{ir}");
}

#[test]
fn ref_string_local_emits_load_then_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let s: ref String = \"hello\";\n\
         }",
    );
    // ref local: must load from alloca, then drop the loaded value.
    assert!(ir.contains("call void @_mvl_string_drop(ptr"), "{ir}");
    // Verify the load-before-drop pattern exists.
    assert!(ir.contains("load ptr, ptr"), "{ir}");
}
