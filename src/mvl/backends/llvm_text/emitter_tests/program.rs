// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emitter.rs::emit_program (top-level walk + extern)` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/program.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::{compile, compile_with_sibling};

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

/// Sibling functions appear in the flat IR module (#1879).
#[test]
fn sibling_fn_emitted_in_flat_module() {
    let sibling = "pub fn add(a: Int, b: Int) -> Int { a + b }";
    let entry = "fn main() -> Unit { }";
    let ir = compile_with_sibling(entry, sibling);
    assert!(
        ir.contains("define i64 @add(i64 %a, i64 %b)"),
        "sibling @add not in IR: {ir}"
    );
    assert!(ir.contains("define i32 @main()"), "main not in IR: {ir}");
}

/// `extern "rust"` block emits an opaque `declare` stub so callers produce valid IR.
/// lli validates the whole module statically; without the stub, functions that call
/// rust-extern symbols produce type errors (i64 default vs actual return type) that
/// reject the whole IR file even when the function being run never calls the extern.
#[test]
fn extern_rust_emits_declare_stub() {
    let ir = compile(
        "extern \"rust\" {\n\
         fn bridge_fn(x: Int) -> Int\n\
         }",
    );
    assert!(
        ir.contains("declare") && ir.contains("bridge_fn"),
        "extern rust should emit an opaque declare stub: {ir}"
    );
}
