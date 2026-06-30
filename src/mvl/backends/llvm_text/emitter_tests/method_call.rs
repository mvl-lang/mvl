// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_method_call.rs` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/method_call.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

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
