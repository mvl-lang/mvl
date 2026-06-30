// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR walker parity tests — mirrors `emit_construct.rs`.
//!
//! Each test re-uses the same `compile(...)` input as the corresponding
//! AST substring test in `emitter_tests/construct.rs`, asserting that the
//! TIR walker emits byte-identical IR. When parity is achieved across the
//! corpus, PR 2 of #1612 flips the CLI to use the TIR path and deletes the
//! AST walker — at which point these tests become the primary coverage.

use super::common::assert_tir_parity;

#[test]
fn some_constructor_emits_tagged_union() {
    assert_tir_parity("fn wrap(n: Int) -> Option[Int] { Some(n) }");
}

#[test]
fn none_constructor_emits_tagged_union() {
    assert_tir_parity("fn empty() -> Option[Int] { None }");
}

#[test]
fn option_match_emits_switch_on_discriminant() {
    assert_tir_parity(
        "fn unwrap_or(opt: Option[Int], default: Int) -> Int {\n\
             match opt {\n\
                 Some(v) => v,\n\
                 None => default,\n\
             }\n\
         }",
    );
}

#[test]
fn map_literal_emits_map_new_and_insert() {
    assert_tir_parity(
        "fn main() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1, \"b\": 2};\n\
         }",
    );
}

#[test]
fn empty_map_emits_map_new_only() {
    assert_tir_parity(
        "fn main() -> Unit {\n\
         let m: Map[String, Int] = Map::new();\n\
         }",
    );
}
