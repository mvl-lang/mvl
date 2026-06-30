// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR walker parity tests — mirrors `emit_mono.rs`.
//!
//! Each test re-uses the same `compile(...)` input as the corresponding
//! AST substring test in `emitter_tests/mono.rs`, asserting that the
//! TIR walker emits byte-identical IR. When parity is achieved across the
//! corpus, PR 2 of #1612 flips the CLI to use the TIR path and deletes the
//! AST walker — at which point these tests become the primary coverage.

use super::common::assert_tir_parity;

// TIR walker has its own MonoQueue parallel to the AST emitter (#1612, Bug 4).
// Pre-pass collects generic TirFns into `MonoQueue::tir_generic_fns`; call
// sites mangle and enqueue; a drain pass at the tail of `emit_program_tir`
// substitutes `Ty` in cloned bodies and emits one mangled copy per concrete
// instantiation. Mangled symbols agree with the AST path because both routes
// share `Self::mangle_generic` operating on `TypeExpr`.
#[test]
fn generic_fn_monomorphized_per_concrete_type() {
    assert_tir_parity(
        "fn identity[T](x: T) -> T { x }\n\
         fn main() -> Unit {\n\
           let n: Int = identity(42);\n\
           let s: String = identity(\"hi\");\n\
         }",
    );
}
