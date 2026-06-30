// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_mono.rs` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/mono.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

/// Generic `identity[T]` must produce separate monomorphized copies for
/// each concrete type argument used at call sites.
#[test]
fn generic_fn_monomorphized_per_concrete_type() {
    let ir = compile(
        "fn identity[T](x: T) -> T { x }\n\
         fn main() -> Unit {\n\
           let n: Int = identity(42);\n\
           let s: String = identity(\"hi\");\n\
         }",
    );
    // Two separate definitions with correct types.
    assert!(ir.contains("define i64 @identity__Int(i64 %x)"), "{ir}");
    assert!(ir.contains("define ptr @identity__String(ptr %x)"), "{ir}");
    // Call sites use mangled names.
    assert!(ir.contains("call i64 @identity__Int(i64 42)"), "{ir}");
    assert!(ir.contains("call ptr @identity__String("), "{ir}");
}
