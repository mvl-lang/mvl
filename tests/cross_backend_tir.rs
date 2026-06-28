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
fn tir_walker_is_wired_up_minimal() {
    // Smallest program that lowers to TIR cleanly. Verifies the CLI plumbing
    // (mono → monomorphize → lower → compile_to_ir_tir) doesn't panic.
    assert_tir_unimplemented("fn main() -> Int { 42 }");
}

#[test]
fn tir_walker_is_wired_up_empty_prog() {
    assert_tir_unimplemented("");
}
