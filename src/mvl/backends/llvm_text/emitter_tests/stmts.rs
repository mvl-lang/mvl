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

// ── Type-aware element drops for nested collections (#1991) ──────────────

#[test]
fn list_of_string_uses_string_ptr_array_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let xs: List[String] = [\"a\", \"b\"];\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_string_ptr_array_drop(ptr"),
        "{ir}"
    );
    assert!(
        ir.contains("declare void @_mvl_string_ptr_array_drop(ptr)"),
        "{ir}"
    );
    assert!(!ir.contains("call void @_mvl_array_drop(ptr"), "{ir}");
}

#[test]
fn list_of_list_uses_array_drop_mvlarray() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let xs: List[List[Int]] = [[1, 2], [3]];\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_array_drop_mvlarray(ptr") && ir.contains("@_mvl_array_drop)"),
        "{ir}"
    );
    assert!(
        ir.contains("declare void @_mvl_array_drop_mvlarray(ptr, ptr)"),
        "{ir}"
    );
}

#[test]
fn list_of_list_of_string_picks_string_inner_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let xs: List[List[String]] = [[\"a\"]];\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_array_drop_mvlarray(ptr")
            && ir.contains("@_mvl_string_ptr_array_drop)"),
        "{ir}"
    );
}

#[test]
fn list_of_option_int_uses_array_drop_option_with_null_payload_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let ys: List[Option[Int]] = [Some(1), None];\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_array_drop_option(ptr") && ir.contains("i64 8, ptr null)"),
        "{ir}"
    );
    assert!(
        ir.contains("declare void @_mvl_array_drop_option(ptr, i64, ptr)"),
        "{ir}"
    );
}

#[test]
fn list_of_option_string_uses_string_drop_payload() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let ys: List[Option[String]] = [Some(\"a\"), None];\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_array_drop_option(ptr")
            && ir.contains("@_mvl_string_drop)"),
        "{ir}"
    );
}

#[test]
fn list_of_result_uses_array_drop_result() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let rs: List[Result[Int, Bool]] = [Ok(1), Err(true)];\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_array_drop_result(ptr"), "{ir}");
    assert!(
        ir.contains("declare void @_mvl_array_drop_result(ptr, i64, ptr, i64, ptr)"),
        "{ir}"
    );
}

#[test]
fn map_string_value_uses_map_drop_ptr_values() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let m: Map[String, String] = {\"a\": \"1\"};\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_map_drop_ptr_values(ptr")
            && ir.contains("@_mvl_string_drop)"),
        "{ir}"
    );
    assert!(
        ir.contains("declare void @_mvl_map_drop_ptr_values(ptr, ptr)"),
        "{ir}"
    );
    assert!(!ir.contains("call void @_mvl_map_drop(ptr"), "{ir}");
}

#[test]
fn map_scalar_value_still_uses_plain_map_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_map_drop(ptr"), "{ir}");
    assert!(!ir.contains("_mvl_map_drop_ptr_values"), "{ir}");
}

#[test]
fn map_insert_string_value_excludes_from_heap_locals() {
    // The inserted `v` must not also be dropped at scope exit — otherwise
    // it would be double-freed once the map's own drop follows the value
    // pointer (#1991).
    let ir = compile(
        "fn f() -> Unit {\n\
         let m: ref Map[String, String] = {\"seed\": \"0\"};\n\
         let v: String = \"hello\";\n\
         m.insert(\"k\", v);\n\
         }",
    );
    let string_drop_count = ir.matches("call void @_mvl_string_drop(ptr").count();
    assert_eq!(
        string_drop_count, 0,
        "expected 0 direct _mvl_string_drop calls on `v` (owned by the map now), got {string_drop_count}\n{ir}"
    );
}
