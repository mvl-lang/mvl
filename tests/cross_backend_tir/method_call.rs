// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR walker parity tests — mirrors `emit_method_call.rs`.
//!
//! Each test re-uses the same `compile(...)` input as the corresponding
//! AST substring test in `emitter_tests/method_call.rs`, asserting that the
//! TIR walker emits byte-identical IR. When parity is achieved across the
//! corpus, PR 2 of #1612 flips the CLI to use the TIR path and deletes the
//! AST walker — at which point these tests become the primary coverage.

use super::common::{assert_tir_parity, assert_tir_unimplemented};

#[test]
fn map_len_emits_mvl_map_len() {
    assert_tir_parity("fn main() -> Int {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         m.len()\n\
         }");
}

#[test]
fn map_keys_emits_mvl_map_keys() {
    assert_tir_parity("fn main() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         let _k: List[String] = m.keys();\n\
         }");
}

#[test]
fn map_contains_key_emits_null_check() {
    assert_tir_parity("fn main() -> Bool {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         m.contains_key(\"a\")\n\
         }");
}

#[test]
fn map_get_emits_null_guard_before_load() {
    assert_tir_parity("fn f(m: Map[String, Int]) -> Int {\n\
         m.get(\"key\")\n\
         }");
}

#[test]
fn string_chars_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> Unit {\n\
         let _cs: List[String] = s.chars();\n\
         }");
}

#[test]
fn string_byte_at_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> Option[Byte] {\n\
         s.byte_at(0)\n\
         }");
}

#[test]
fn string_find_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> Int {\n\
         s.find(\"x\")\n\
         }");
}

#[test]
fn string_split_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> Unit {\n\
         let _parts: List[String] = s.split(\",\");\n\
         }");
}

#[test]
fn string_substring_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> String {\n\
         s.substring(0, 3)\n\
         }");
}

#[test]
fn string_contains_emits_i64_to_bool() {
    assert_tir_parity("fn f(s: String) -> Bool {\n\
         s.contains(\"x\")\n\
         }");
}

#[test]
fn string_starts_with_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> Bool {\n\
         s.starts_with(\"http\")\n\
         }");
}

#[test]
fn string_ends_with_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> Bool {\n\
         s.ends_with(\".mvl\")\n\
         }");
}

#[test]
fn string_trim_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> String {\n\
         s.trim()\n\
         }");
}

#[test]
fn string_to_lower_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> String {\n\
         s.to_lower()\n\
         }");
}

#[test]
fn string_to_upper_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> String {\n\
         s.to_upper()\n\
         }");
}

#[test]
fn string_replace_emits_runtime_call() {
    assert_tir_parity("fn f(s: String) -> String {\n\
         s.replace(\"old\", \"new\")\n\
         }");
}
