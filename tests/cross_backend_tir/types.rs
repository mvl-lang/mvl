// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR walker parity tests — mirrors `emit_types.rs`.
//!
//! Each test re-uses the same `compile(...)` input as the corresponding
//! AST substring test in `emitter_tests/types.rs`, asserting that the
//! TIR walker emits byte-identical IR. When parity is achieved across the
//! corpus, PR 2 of #1612 flips the CLI to use the TIR path and deletes the
//! AST walker — at which point these tests become the primary coverage.

use super::common::assert_tir_parity;

#[test]
fn struct_type_emits_type_def() {
    assert_tir_parity("type Point = struct { x: Int, y: Int }\n\
         fn get_x(p: Point) -> Int { p.x }");
}

#[test]
fn enum_variant_emits_discriminant() {
    assert_tir_parity("type Shape = enum { Circle, Square }\n\
         fn circle() -> Shape { Shape::Circle }");
}
