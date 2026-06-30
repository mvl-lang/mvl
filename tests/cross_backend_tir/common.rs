// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Shared helpers for the `cross_backend_tir` IR-diff oracle (#1612).
//!
//! Each per-concern submodule mirrors the AST-side `emitter_tests/<concern>.rs`:
//! for every AST substring test, there is a TIR parity twin using the same
//! `compile(...)` input. Coverage transfers when AST emitters are deleted.

use mvl::mvl::backends::llvm_text::LlvmTextCompiler;
use mvl::mvl::ir::lower;
use mvl::mvl::ir::TirProgram;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;
use mvl::mvl::passes::mono;
use mvl::mvl::pipeline::assemble_expr_types;

pub fn parse(src: &str) -> Program {
    let (mut p, errs) = Parser::new(src);
    assert!(errs.is_empty(), "lex errors: {errs:?}");
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    prog
}

pub fn lower_to_tir(prog: &Program) -> (TirProgram, LlvmTextCompiler) {
    let mut compiler = LlvmTextCompiler::new();
    compiler.expr_types = assemble_expr_types(prog, &[]);
    let all_fns = mono::collect_fns([prog]);
    let m = mono::monomorphize(prog, &all_fns, &compiler.expr_types);
    let tir = lower::lower(prog, &m, &compiler.expr_types);
    (tir, compiler)
}

/// Assert IR parity between AST and TIR walker paths for `src`.
///
/// **Strict** as of the completion of the variant-by-variant port (#1612):
/// the TIR walker must succeed AND emit byte-identical IR to the AST walker.
/// Any "not yet implemented" / "not yet ported" error is treated as a
/// regression (the walker has been ported variant-by-variant — see commits
/// ffabb145..76bfbbf0 — so unimplemented messages should no longer reach
/// the test target).
///
/// Tests that exercise the one known migration gap (mono pipeline in the
/// standalone test environment) are individually marked `#[ignore]`.
pub fn assert_tir_parity(src: &str) {
    let prog = parse(src);
    let (tir, compiler) = lower_to_tir(&prog);
    let ast_ir = compiler
        .compile_to_ir(&prog, "test")
        .expect("AST path failed");
    let tir_ir = compiler
        .compile_to_ir_tir(&tir, "test")
        .expect("TIR path failed");
    assert_eq!(ast_ir, tir_ir, "TIR walker output diverged from AST walker");
}
