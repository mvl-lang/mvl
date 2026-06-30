// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR walker parity tests — mirrors `emit_exprs.rs`.
//!
//! Each test re-uses the same `compile(...)` input as the corresponding
//! AST substring test in `emitter_tests/exprs.rs`, asserting that the
//! TIR walker emits byte-identical IR. When parity is achieved across the
//! corpus, PR 2 of #1612 flips the CLI to use the TIR path and deletes the
//! AST walker — at which point these tests become the primary coverage.

use super::common::assert_tir_parity;

#[test]
fn simple_add_function() {
    assert_tir_parity("fn add(a: Int, b: Int) -> Int { a + b }");
}

#[test]
fn integer_literal_returned() {
    assert_tir_parity("fn answer() -> Int { 42 }");
}

#[test]
fn bool_literal_returned() {
    assert_tir_parity("fn always_true() -> Bool { true }");
}

#[test]
fn arithmetic_operators() {
    assert_tir_parity("fn f(a: Int, b: Int) -> Int { a - b }");
    assert_tir_parity("fn f(a: Int, b: Int) -> Int { a * b }");
    assert_tir_parity("fn f(a: Int, b: Int) -> Int { a / b }");
    assert_tir_parity("fn f(a: Int, b: Int) -> Int { a % b }");
}

#[test]
fn comparison_operators_emit_icmp() {
    assert_tir_parity("fn lt(a: Int, b: Int) -> Bool { a < b }");
    assert_tir_parity("fn gt(a: Int, b: Int) -> Bool { a > b }");
    assert_tir_parity("fn eq(a: Int, b: Int) -> Bool { a == b }");
}

#[test]
fn if_else_emits_phi() {
    assert_tir_parity("fn max(a: Int, b: Int) -> Int { if a > b { a } else { b } }");
}

#[test]
fn else_if_chain_emits_phi_for_all_branches() {
    assert_tir_parity(
        "fn classify(n: Int) -> Int {\n\
             if n > 0 { 1 }\n\
             else if n < 0 { -1 }\n\
             else { 0 }\n\
         }",
    );
}

#[test]
fn logical_not_emits_xor() {
    assert_tir_parity("fn f(b: Bool) -> Bool { !b }");
}

#[test]
fn multiple_functions_and_call() {
    assert_tir_parity(
        "fn add(a: Int, b: Int) -> Int { a + b }\n\
         fn double(n: Int) -> Int { add(n, n) }",
    );
}

#[test]
fn negation_emits_sub_from_zero() {
    assert_tir_parity("fn neg(x: Int) -> Int { -x }");
}

#[test]
fn short_circuit_and_emits_phi() {
    assert_tir_parity("fn f(a: Bool, b: Bool) -> Bool { a && b }");
}

#[test]
fn short_circuit_or_emits_phi() {
    assert_tir_parity("fn f(a: Bool, b: Bool) -> Bool { a || b }");
}

#[test]
fn string_literal_emits_global_and_string_new() {
    assert_tir_parity("fn main() -> Unit ! Console { println(\"hello\") }");
}

#[test]
fn assert_emits_conditional_trap() {
    assert_tir_parity("fn main() -> Unit { assert(1 == 1) }");
}

#[test]
fn propagate_in_result_returning_fn() {
    assert_tir_parity(
        "fn div(a: Int, b: Int) -> Result[Int, String] {\n\
         if b == 0 { Err(\"zero\") } else { Ok(a / b) }\n\
         }\n\
         fn caller(x: Int) -> Result[Int, String] {\n\
         let v: Int = div(x, 2)?;\n\
         Ok(v + 1)\n\
         }",
    );
}

#[test]
fn relabel_trust_unwraps_tainted() {
    assert_tir_parity(
        "fn sanitize(raw: Tainted[String]) -> String {\n\
         relabel trust(raw, \"TEST-001\")\n\
         }",
    );
}
