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

// KNOWN MIGRATION GAP (#1612): the TIR walker's lower() pipeline doesn't
// produce monomorphized TirFn copies in the standalone-test environment
// (no prelude, no full pipeline). The walker succeeds but emits a single
// generic-typed definition + mismatched call sites, instead of the two
// mangled copies the AST emit_mono path produces on-the-fly via its
// legacy mono_queue. Tracked as part of the broader emit_mono.rs deletion
// in #1612 — the AST path runs the same logic inline during emission;
// the TIR path will get its monomorphization from the upstream pipeline
// once that's wired through prepare_llvm_text_tir.
#[test]
#[ignore = "#1612: TIR walker mono pipeline not yet wired in standalone tests"]
fn generic_fn_monomorphized_per_concrete_type() {
    assert_tir_parity("fn identity[T](x: T) -> T { x }\n\
         fn main() -> Unit {\n\
           let n: Int = identity(42);\n\
           let s: String = identity(\"hi\");\n\
         }");
}
