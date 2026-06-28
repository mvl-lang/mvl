// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR walker parity tests — mirrors `emitter.rs::emit_program (top-level walk + extern)`.
//!
//! Each test re-uses the same `compile(...)` input as the corresponding
//! AST substring test in `emitter_tests/program.rs`, asserting that the
//! TIR walker emits byte-identical IR. When parity is achieved across the
//! corpus, PR 2 of #1612 flips the CLI to use the TIR path and deletes the
//! AST walker — at which point these tests become the primary coverage.

use super::common::{assert_tir_parity, assert_tir_unimplemented};

#[test]
fn unit_function_emits_ret_void() {
    assert_tir_parity("fn noop() -> Unit { }");
}

#[test]
fn main_emits_i32_return() {
    assert_tir_parity("fn main() -> Unit { }");
}

#[test]
fn main_explicit_return_emits_ret_i32_0() {
    assert_tir_parity("fn main() -> Unit { return; }");
}

#[test]
fn module_header_present() {
    assert_tir_parity("fn f() -> Int { 0 }");
}

#[test]
fn extern_c_emits_declare() {
    assert_tir_parity("extern \"c\" {\n\
         fn sqlite_open(path: String) -> Int\n\
         fn sqlite_close(db: Int) -> Unit\n\
         }");
}

#[test]
fn extern_rust_not_emitted_by_llvm() {
    assert_tir_parity("extern \"rust\" {\n\
         fn bridge_fn(x: Int) -> Int\n\
         }");
}
