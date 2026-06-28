// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_types.rs` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/types.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

#[test]
fn struct_type_emits_type_def() {
    let ir = compile(
        "type Point = struct { x: Int, y: Int }\n\
         fn get_x(p: Point) -> Int { p.x }",
    );
    assert!(ir.contains("%Point = type { i64, i64 }"), "{ir}");
    assert!(ir.contains("define i64 @get_x(%Point %p)"), "{ir}");
    assert!(ir.contains("extractvalue %Point"), "{ir}");
}

#[test]
fn enum_variant_emits_discriminant() {
    let ir = compile(
        "type Shape = enum { Circle, Square }\n\
         fn circle() -> Shape { Shape::Circle }",
    );
    assert!(ir.contains("ret i64 0"), "{ir}");
}
