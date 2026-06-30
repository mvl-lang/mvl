// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emitter.rs::emit_program (top-level walk + extern)` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/program.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

#[test]
fn unit_function_emits_ret_void() {
    let ir = compile("fn noop() -> Unit { }");
    assert!(ir.contains("define void @noop()"), "{ir}");
    assert!(ir.contains("ret void"), "{ir}");
}

#[test]
fn main_emits_i32_return() {
    let ir = compile("fn main() -> Unit { }");
    assert!(ir.contains("define i32 @main()"), "{ir}");
    assert!(ir.contains("ret i32 0"), "{ir}");
}

#[test]
fn main_explicit_return_emits_ret_i32_0() {
    let ir = compile("fn main() -> Unit { return; }");
    assert!(ir.contains("define i32 @main()"), "{ir}");
    assert!(ir.contains("ret i32 0"), "{ir}");
    assert!(!ir.contains("ret void"), "{ir}");
}

#[test]
fn module_header_present() {
    let ir = compile("fn f() -> Int { 0 }");
    assert!(ir.contains("ModuleID = 'test'"), "{ir}");
    assert!(ir.contains("source_filename = \"test\""), "{ir}");
    assert!(ir.contains("target triple"), "{ir}");
}

/// `extern "c"` block emits LLVM `declare` instructions (#811).
#[test]
fn extern_c_emits_declare() {
    let ir = compile(
        "extern \"c\" {\n\
         fn sqlite_open(path: String) -> Int\n\
         fn sqlite_close(db: Int) -> Unit\n\
         }",
    );
    assert!(
        ir.contains("declare i64 @sqlite_open(ptr)"),
        "missing sqlite_open declare: {ir}"
    );
    assert!(
        ir.contains("declare void @sqlite_close(i64)"),
        "missing sqlite_close declare: {ir}"
    );
}

/// `extern "rust"` block is NOT emitted by LLVM backend (handled by Rust backend only).
#[test]
fn extern_rust_not_emitted_by_llvm() {
    let ir = compile(
        "extern \"rust\" {\n\
         fn bridge_fn(x: Int) -> Int\n\
         }",
    );
    assert!(
        !ir.contains("declare") || !ir.contains("bridge_fn"),
        "extern rust should not emit declare: {ir}"
    );
}
