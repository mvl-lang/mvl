// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_construct.rs` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/construct.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

/// `Some(val)` must emit a `{ i8, ptr }` tagged union with disc=0.
#[test]
fn some_constructor_emits_tagged_union() {
    let ir = compile("fn wrap(n: Int) -> Option[Int] { Some(n) }");
    assert!(
        ir.contains("insertvalue { i8, ptr } zeroinitializer, i8 0, 0"),
        "{ir}"
    );
    assert!(ir.contains("insertvalue { i8, ptr }"), "{ir}");
    assert!(ir.contains("define { i8, ptr } @wrap"), "{ir}");
}

/// `None` must emit a `{ i8, ptr }` tagged union with disc=1.
#[test]
fn none_constructor_emits_tagged_union() {
    let ir = compile("fn empty() -> Option[Int] { None }");
    assert!(
        ir.contains("insertvalue { i8, ptr } zeroinitializer, i8 1, 0"),
        "{ir}"
    );
}

/// Match on `Option[Int]` must emit a switch on the discriminant byte.
#[test]
fn option_match_emits_switch_on_discriminant() {
    let ir = compile(
        "fn unwrap_or(opt: Option[Int], default: Int) -> Int {\n\
             match opt {\n\
                 Some(v) => v,\n\
                 None => default,\n\
             }\n\
         }",
    );
    assert!(ir.contains("switch i8"), "{ir}");
    assert!(ir.contains("i8 0, label"), "{ir}"); // Some arm
    assert!(ir.contains("i8 1, label"), "{ir}"); // None arm
    assert!(ir.contains("phi i64"), "{ir}");
}

#[test]
fn map_literal_emits_map_new_and_insert() {
    let ir = compile(
        "fn main() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1, \"b\": 2};\n\
         }",
    );
    assert!(ir.contains("call ptr @_mvl_map_new(i64"), "{ir}");
    assert!(ir.contains("call void @_mvl_map_insert(ptr"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_string_ptr(ptr"), "{ir}");
    assert!(ir.contains("call i64 @_mvl_str_len(ptr"), "{ir}");
}

#[test]
fn empty_map_emits_map_new_only() {
    let ir = compile(
        "fn main() -> Unit {\n\
         let m: Map[String, Int] = Map::new();\n\
         }",
    );
    // Map::new() goes through FnCall, not Map literal — just verify no crash.
    assert!(ir.contains("define i32 @main()"), "{ir}");
}
