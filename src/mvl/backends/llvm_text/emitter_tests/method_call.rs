// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_method_call.rs` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/method_call.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::{compile, compile_test_crate};

#[test]
fn map_len_emits_mvl_map_len() {
    let ir = compile(
        "fn main() -> Int {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         m.len()\n\
         }",
    );
    assert!(ir.contains("declare i64 @_mvl_map_len(ptr)"), "{ir}");
    assert!(ir.contains("call i64 @_mvl_map_len(ptr"), "{ir}");
}

#[test]
fn map_keys_emits_mvl_map_keys() {
    let ir = compile(
        "fn main() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         let _k: List[String] = m.keys();\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_map_keys(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_map_keys(ptr"), "{ir}");
}

#[test]
fn map_contains_key_emits_null_check() {
    let ir = compile(
        "fn main() -> Bool {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         m.contains_key(\"a\")\n\
         }",
    );
    assert!(ir.contains("call ptr @_mvl_map_get(ptr"), "{ir}");
    assert!(ir.contains("icmp ne ptr"), "{ir}");
}

#[test]
fn map_get_emits_null_guard_before_load() {
    let ir = compile(
        "fn f(m: Map[String, Int]) -> Int {\n\
         m.get(\"key\")\n\
         }",
    );
    assert!(ir.contains("call ptr @_mvl_map_get(ptr"), "{ir}");
    // Must null-check before building Option struct
    assert!(ir.contains("icmp eq ptr"), "{ir}");
    assert!(ir.contains("insertvalue { i8, ptr }"), "{ir}");
    assert!(ir.contains("phi { i8, ptr }"), "{ir}");
}

// `Map[String, Int]`'s value slot holds the i64 by value, not a shared
// reference — nothing to clone. Confirms the scalar path is unchanged by
// the String-specific fix below.
#[test]
fn map_get_scalar_value_does_not_clone() {
    let ir = compile(
        "fn f(m: Map[String, Int]) -> Int {\n\
         m.get(\"key\")\n\
         }",
    );
    assert!(!ir.contains("@_mvl_string_clone"), "{ir}");
}

// `Map[String, String]`'s value slot stores the string's pointer by value
// (aliasing the same heap object the map owns — see the "Transfer
// ownership" comment in `emit_map_literal_tir`). `.get()` must clone the
// loaded pointer into a fresh Option payload slot rather than handing back
// a pointer into the map's own storage — otherwise the caller's eventual
// drop (e.g. `unwrap_or`'s cleanup) frees memory the map still references,
// and the map's own later drop double-frees it (#2047, mirrors the
// equivalent WASM backend fix).
#[test]
fn map_get_string_value_clones_before_wrapping_in_option() {
    let ir = compile(
        "fn f(m: Map[String, String]) -> String {\n\
         m.get(\"key\").unwrap_or(\"default\")\n\
         }",
    );
    assert!(ir.contains("call ptr @_mvl_map_get(ptr"), "{ir}");
    assert!(ir.contains("declare ptr @_mvl_string_clone(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_string_clone(ptr"), "{ir}");
    // The cloned pointer must land in a fresh slot, not the map's own —
    // i.e. a second `alloca ptr` distinct from the map-get call's result.
    assert!(ir.contains("insertvalue { i8, ptr }"), "{ir}");
}

#[test]
fn string_chars_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Unit {\n\
         let _cs: List[String] = s.chars();\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_string_chars(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_string_chars(ptr"), "{ir}");
}

#[test]
fn string_byte_at_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Option[Byte] {\n\
         s.byte_at(0)\n\
         }",
    );
    assert!(
        ir.contains("declare i8 @_mvl_str_byte_at(ptr, i64, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("call i8 @_mvl_str_byte_at(ptr"), "{ir}");
}

#[test]
fn string_find_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Int {\n\
         s.find(\"x\")\n\
         }",
    );
    assert!(ir.contains("declare i64 @_mvl_str_find(ptr, ptr)"), "{ir}");
    assert!(ir.contains("call i64 @_mvl_str_find(ptr"), "{ir}");
}

#[test]
fn string_split_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Unit {\n\
         let _parts: List[String] = s.split(\",\");\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_str_split(ptr, ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_str_split(ptr"), "{ir}");
}

#[test]
fn string_substring_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> String {\n\
         s.substring(0, 3)\n\
         }",
    );
    assert!(
        ir.contains("declare ptr @_mvl_str_substring(ptr, i64, i64)"),
        "{ir}"
    );
    assert!(ir.contains("call ptr @_mvl_str_substring(ptr"), "{ir}");
}

#[test]
fn string_contains_emits_i64_to_bool() {
    let ir = compile(
        "fn f(s: String) -> Bool {\n\
         s.contains(\"x\")\n\
         }",
    );
    assert!(
        ir.contains("declare i64 @_mvl_str_contains(ptr, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("icmp ne i64"), "{ir}");
}

#[test]
fn string_starts_with_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Bool {\n\
         s.starts_with(\"http\")\n\
         }",
    );
    assert!(
        ir.contains("declare i64 @_mvl_str_starts_with(ptr, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("call i64 @_mvl_str_starts_with(ptr"), "{ir}");
}

#[test]
fn string_ends_with_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Bool {\n\
         s.ends_with(\".mvl\")\n\
         }",
    );
    assert!(
        ir.contains("declare i64 @_mvl_str_ends_with(ptr, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("call i64 @_mvl_str_ends_with(ptr"), "{ir}");
}

#[test]
fn string_trim_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> String {\n\
         s.trim()\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_str_trim(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_str_trim(ptr"), "{ir}");
}

#[test]
fn string_to_lower_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> String {\n\
         s.to_lower()\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_str_to_lower(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_str_to_lower(ptr"), "{ir}");
}

#[test]
fn string_to_upper_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> String {\n\
         s.to_upper()\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_str_to_upper(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_str_to_upper(ptr"), "{ir}");
}

#[test]
fn string_replace_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> String {\n\
         s.replace(\"old\", \"new\")\n\
         }",
    );
    assert!(
        ir.contains("declare ptr @_mvl_str_replace(ptr, ptr, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("call ptr @_mvl_str_replace(ptr"), "{ir}");
}

/// `assert_eq` on two `Option[T]` values used to be a hard codegen error
/// ("unsupported LLVM type `{ i8, ptr }`"), which failed the *whole file's*
/// compilation — so `assert_eq(row.get(1), Some("x"))`, the natural way to
/// assert on any `.get()`/`.first()`/`.last()` result, was unusable.
/// Compares discriminants, then payloads only when both are `Some`.
#[test]
fn assert_eq_on_option_string_compares_disc_then_payload() {
    let ir = compile_test_crate(
        "test fn t() -> Unit {\n\
         let row: List[String] = [\"a\", \"bb\"];\n\
         assert_eq(row.get(1), Some(\"bb\"));\n\
         }",
    );
    assert!(
        ir.contains("extractvalue { i8, ptr }") && ir.contains("call i1 @_mvl_string_eq"),
        "Option[String] equality must compare discriminants and then payloads \
         via _mvl_string_eq:\n{ir}"
    );
}

/// The payload comparison must be reached only when both sides are `Some` —
/// a `None`'s payload pointer is null, so an unconditional load would fault.
#[test]
fn assert_eq_on_option_int_guards_payload_load() {
    let ir = compile_test_crate(
        "test fn t() -> Unit {\n\
         let xs: List[Int] = [1, 2];\n\
         assert_eq(xs.get(0), Some(1));\n\
         }",
    );
    assert!(
        ir.contains("opt_eq_payload") && ir.contains("opt_eq_skip"),
        "payload load must sit behind a both-Some guard:\n{ir}"
    );
}
