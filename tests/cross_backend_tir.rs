// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! IR-diff oracle for the LLVM-text backend's TIR walker (#1612, Phase 3b PR 1).
//!
//! Compares output from the in-progress TIR-walking emitter
//! ([`LlvmTextCompiler::compile_to_ir_tir`]) against the established AST path
//! ([`LlvmTextCompiler::compile_to_ir`]) across a curated set of MVL programs.
//!
//! While the TIR walker is being built leaf-first, most programs return an
//! `"emit_program_tir: not yet implemented"` error — this test only asserts
//! the TIR path is *wired up* and exists. As the walker grows, individual
//! programs are migrated from `assert_tir_unimplemented` to `assert_tir_parity`.
//!
//! When all corpus programs pass `assert_tir_parity`, PR 2 of #1612 will flip
//! the CLI to use the TIR path and delete the AST walker.

use mvl::mvl::backends::llvm_text::LlvmTextCompiler;
use mvl::mvl::ir::lower;
use mvl::mvl::ir::TirProgram;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;
use mvl::mvl::passes::mono;
use mvl::mvl::pipeline::assemble_expr_types;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse(src: &str) -> Program {
    let (mut p, errs) = Parser::new(src);
    assert!(errs.is_empty(), "lex errors: {errs:?}");
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    prog
}

fn lower_to_tir(prog: &Program) -> (TirProgram, LlvmTextCompiler) {
    let mut compiler = LlvmTextCompiler::new();
    compiler.expr_types = assemble_expr_types(prog, &[]);

    let all_fns = mono::collect_fns([prog]);
    let m = mono::monomorphize(prog, &all_fns, &compiler.expr_types);
    let tir = lower::lower(prog, &m, &compiler.expr_types);

    (tir, compiler)
}

/// Assert the TIR walker is *wired up* and reports its unimplemented state
/// (used while the walker is being built leaf-first).
#[allow(dead_code)] // call sites land as the walker is built out
fn assert_tir_unimplemented(src: &str) {
    let prog = parse(src);
    let (tir, compiler) = lower_to_tir(&prog);
    let result = compiler.compile_to_ir_tir(&tir, "test");
    match result {
        Err(msg) if msg.contains("not yet implemented") => { /* expected */ }
        Err(msg) => panic!("unexpected error from TIR walker: {msg}"),
        Ok(_) => panic!(
            "TIR walker unexpectedly succeeded — promote this case to assert_tir_parity"
        ),
    }
}

/// Assert IR parity between the AST and TIR walker paths for `src`.
///
/// Once all corpus cases reach this state, PR 2 of #1612 flips the CLI
/// default and deletes the AST walker.
#[allow(dead_code)] // first call sites land as the walker is built out
fn assert_tir_parity(src: &str) {
    let prog = parse(src);
    let (tir, compiler) = lower_to_tir(&prog);
    let ast_ir = compiler
        .compile_to_ir(&prog, "test")
        .expect("AST path failed");
    let tir_ir = compiler
        .compile_to_ir_tir(&tir, "test")
        .expect("TIR path failed");
    assert_eq!(
        ast_ir, tir_ir,
        "TIR walker output diverged from AST walker"
    );
}

// ── Wiring smoke tests ────────────────────────────────────────────────────────

#[test]
fn tir_walker_empty_program() {
    // Smallest program: empty top-level. TIR walker should produce identical
    // IR to the AST walker (just the module header).
    assert_tir_parity("");
}

#[test]
fn tir_walker_main_returns_int_literal() {
    assert_tir_parity("fn main() -> Int { 42 }");
}

#[test]
fn tir_walker_main_returns_bool_literal() {
    assert_tir_parity("fn main() -> Bool { true }");
}

#[test]
fn tir_walker_fn_with_int_param() {
    assert_tir_parity("fn id(x: Int) -> Int { x }");
}

#[test]
fn tir_walker_fn_with_two_params() {
    assert_tir_parity("fn add(a: Int, b: Int) -> Int { a + b }");
}

#[test]
fn tir_walker_unit_return() {
    assert_tir_parity("fn nothing() -> Unit { }");
}

#[test]
fn tir_walker_unary_neg() {
    assert_tir_parity("fn negate(x: Int) -> Int { -x }");
}

#[test]
fn tir_walker_unary_not() {
    assert_tir_parity("fn flip(x: Bool) -> Bool { !x }");
}

#[test]
fn tir_walker_float_arith() {
    assert_tir_parity("fn fadd(a: Float, b: Float) -> Float { a + b }");
}

#[test]
fn tir_walker_comparison() {
    assert_tir_parity("fn lt(a: Int, b: Int) -> Bool { a < b }");
}

#[test]
fn tir_walker_short_circuit_and() {
    assert_tir_parity("fn both(a: Bool, b: Bool) -> Bool { a && b }");
}

#[test]
fn tir_walker_short_circuit_or() {
    assert_tir_parity("fn either(a: Bool, b: Bool) -> Bool { a || b }");
}

#[test]
fn tir_walker_let_immutable() {
    assert_tir_parity(
        "fn add_one(x: Int) -> Int {
            let y: Int = x + 1;
            y
        }",
    );
}

#[test]
fn tir_walker_let_ref_mutable() {
    assert_tir_parity(
        "fn count() -> Int {
            let c: ref Int = 0;
            c = c + 1;
            c
        }",
    );
}

#[test]
fn tir_walker_explicit_return() {
    assert_tir_parity(
        "fn early(x: Int) -> Int {
            return x;
        }",
    );
}

#[test]
fn tir_walker_let_chain() {
    assert_tir_parity(
        "fn sum_three(a: Int, b: Int, c: Int) -> Int {
            let ab: Int = a + b;
            let sum: Int = ab + c;
            sum
        }",
    );
}

#[test]
fn tir_walker_if_expression() {
    assert_tir_parity(
        "fn max(a: Int, b: Int) -> Int {
            if a > b { a } else { b }
        }",
    );
}

#[test]
fn tir_walker_if_else_chain() {
    assert_tir_parity(
        "fn sign(x: Int) -> Int {
            if x > 0 { 1 } else if x < 0 { -1 } else { 0 }
        }",
    );
}

#[test]
fn tir_walker_user_fn_call() {
    assert_tir_parity(
        "fn double(x: Int) -> Int { x + x }
         fn quad(x: Int) -> Int { double(double(x)) }",
    );
}

#[test]
fn tir_walker_void_fn_call() {
    assert_tir_parity(
        "fn noop() -> Unit { }
         fn main() -> Int { noop(); 0 }",
    );
}

#[test]
fn tir_walker_nested_let_and_if() {
    assert_tir_parity(
        "fn classify(x: Int) -> Int {
            let doubled: Int = x + x;
            if doubled > 10 {
                doubled
            } else {
                0
            }
        }",
    );
}

#[test]
fn tir_walker_if_stmt_void() {
    assert_tir_parity(
        "fn maybe_set(flag: Bool) -> Int {
            let result: ref Int = 0;
            if flag {
                result = 42;
            }
            result
        }",
    );
}

#[test]
fn tir_walker_while_loop() {
    assert_tir_parity(
        "fn count_down(n: Int) -> Int {
            let i: ref Int = n;
            while i > 0 {
                i = i - 1;
            }
            i
        }",
    );
}

#[test]
fn tir_walker_recursive_fn() {
    // Note: still requires the user-fn call path, which works.
    assert_tir_parity(
        "fn fact(n: Int) -> Int {
            if n <= 1 { 1 } else { n * fact(n - 1) }
        }",
    );
}

#[test]
fn tir_walker_struct_construct_and_field() {
    assert_tir_parity(
        "type Point = struct { x: Int, y: Int }
         fn make() -> Point { Point { x: 3, y: 4 } }
         fn first(p: Point) -> Int { p.x }",
    );
}

#[test]
fn tir_walker_nested_struct_access() {
    assert_tir_parity(
        "type Inner = struct { v: Int }
         type Outer = struct { inner: Inner }
         fn extract(o: Outer) -> Int { o.inner.v }",
    );
}

#[test]
fn tir_walker_assert() {
    assert_tir_parity(
        "fn check(x: Int) -> Int {
            assert(x > 0);
            x
        }",
    );
}

#[test]
fn tir_walker_println_string_literal() {
    assert_tir_parity(
        r#"fn main() -> Int { println("hello"); 0 }"#,
    );
}

#[test]
fn tir_walker_print_empty() {
    assert_tir_parity(
        "fn main() -> Int { println(); 0 }",
    );
}
