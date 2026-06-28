// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_exprs.rs` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/exprs.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

#[test]
fn simple_add_function() {
    let ir = compile("fn add(a: Int, b: Int) -> Int { a + b }");
    assert!(ir.contains("define i64 @add(i64 %a, i64 %b)"), "{ir}");
    assert!(ir.contains("add i64"), "{ir}");
    assert!(ir.contains("ret i64"), "{ir}");
}

#[test]
fn integer_literal_returned() {
    let ir = compile("fn answer() -> Int { 42 }");
    assert!(ir.contains("define i64 @answer()"), "{ir}");
    assert!(ir.contains("ret i64 42"), "{ir}");
}

#[test]
fn bool_literal_returned() {
    let ir = compile("fn always_true() -> Bool { true }");
    assert!(ir.contains("define i1 @always_true()"), "{ir}");
    assert!(ir.contains("ret i1 true"), "{ir}");
}

#[test]
fn arithmetic_operators() {
    let ir = compile("fn f(a: Int, b: Int) -> Int { a - b }");
    assert!(ir.contains("sub i64"), "{ir}");
    let ir = compile("fn f(a: Int, b: Int) -> Int { a * b }");
    assert!(ir.contains("mul i64"), "{ir}");
    let ir = compile("fn f(a: Int, b: Int) -> Int { a / b }");
    assert!(ir.contains("sdiv i64"), "{ir}");
    let ir = compile("fn f(a: Int, b: Int) -> Int { a % b }");
    assert!(ir.contains("srem i64"), "{ir}");
}

#[test]
fn comparison_operators_emit_icmp() {
    let ir = compile("fn lt(a: Int, b: Int) -> Bool { a < b }");
    assert!(ir.contains("icmp slt i64"), "{ir}");
    let ir = compile("fn gt(a: Int, b: Int) -> Bool { a > b }");
    assert!(ir.contains("icmp sgt i64"), "{ir}");
    let ir = compile("fn eq(a: Int, b: Int) -> Bool { a == b }");
    assert!(ir.contains("icmp eq i64"), "{ir}");
}

#[test]
fn if_else_emits_phi() {
    let ir = compile("fn max(a: Int, b: Int) -> Int { if a > b { a } else { b } }");
    assert!(ir.contains("icmp sgt"), "{ir}");
    assert!(ir.contains("br i1"), "{ir}");
    assert!(ir.contains("phi"), "{ir}");
    assert!(ir.contains("ret i64"), "{ir}");
}

/// Regression for #1155: a 3-way `else if` chain must emit PHI nodes for
/// every branch so the correct value is selected at runtime. Before the fix,
/// the `else if` condition was silently dropped and the merge block produced
/// `ret i64 undef`.
#[test]
fn else_if_chain_emits_phi_for_all_branches() {
    let ir = compile(
        "fn classify(n: Int) -> Int {\n\
             if n > 0 { 1 }\n\
             else if n < 0 { -1 }\n\
             else { 0 }\n\
         }",
    );
    // The `else if n < 0` condition must actually be evaluated.
    assert!(ir.contains("icmp slt"), "{ir}");
    // Two PHI nodes: inner selects between -1 and 0; outer selects between 1 and inner.
    let phi_count = ir.matches(" = phi ").count();
    assert!(
        phi_count >= 2,
        "else-if chain needs ≥2 phi nodes, got {phi_count}\n{ir}"
    );
    // Return must be a defined value, not undef.
    assert!(ir.contains("ret i64"), "{ir}");
    assert!(!ir.contains("ret i64 undef"), "{ir}");
}

#[test]
fn logical_not_emits_xor() {
    let ir = compile("fn f(b: Bool) -> Bool { !b }");
    assert!(ir.contains("xor i1"), "{ir}");
}

#[test]
fn multiple_functions_and_call() {
    let ir = compile(
        "fn add(a: Int, b: Int) -> Int { a + b }\n\
         fn double(n: Int) -> Int { add(n, n) }",
    );
    assert!(ir.contains("define i64 @add"), "{ir}");
    assert!(ir.contains("define i64 @double"), "{ir}");
    assert!(ir.contains("call i64 @add"), "{ir}");
}

#[test]
fn negation_emits_sub_from_zero() {
    let ir = compile("fn neg(x: Int) -> Int { -x }");
    assert!(ir.contains("sub i64 0,"), "{ir}");
}

#[test]
fn short_circuit_and_emits_phi() {
    let ir = compile("fn f(a: Bool, b: Bool) -> Bool { a && b }");
    assert!(ir.contains("phi i1"), "{ir}");
    assert!(ir.contains("false"), "{ir}");
}

#[test]
fn short_circuit_or_emits_phi() {
    let ir = compile("fn f(a: Bool, b: Bool) -> Bool { a || b }");
    assert!(ir.contains("phi i1"), "{ir}");
    assert!(ir.contains("true"), "{ir}");
}

#[test]
fn string_literal_emits_global_and_string_new() {
    let ir = compile("fn main() -> Unit ! Console { println(\"hello\") }");
    assert!(ir.contains("_mvl_string_new"), "{ir}");
    assert!(ir.contains("hello"), "{ir}");
    assert!(ir.contains("dprintf"), "{ir}");
}

#[test]
fn assert_emits_conditional_trap() {
    let ir = compile("fn main() -> Unit { assert(1 == 1) }");
    assert!(ir.contains("llvm.trap"), "{ir}");
    assert!(ir.contains("br i1"), "{ir}");
}
