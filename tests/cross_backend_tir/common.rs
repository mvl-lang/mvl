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
    // `expr_types` was hoisted off `LlvmTextCompiler` in an earlier refactor —
    // now a pipeline-local product, threaded through `mono` and `lower` only.
    let compiler = LlvmTextCompiler::new();
    let expr_types = assemble_expr_types(prog, &[]);
    let all_fns = mono::collect_fns([prog]);
    let m = mono::monomorphize(prog, &all_fns, &expr_types);
    let tir = lower::lower(prog, &m, &expr_types);
    (tir, compiler)
}

/// Compile `src` via the TIR walker and return the IR string.
/// The AST walker was deleted in #1612 Phase 3b — this function now just
/// verifies the TIR path succeeds and returns non-empty IR.
pub fn assert_tir_parity(src: &str) {
    let prog = parse(src);
    let (tir, compiler) = lower_to_tir(&prog);
    let ir = compiler
        .compile_to_ir_tir(&tir, "test")
        .expect("TIR path failed");
    assert!(!ir.is_empty(), "TIR walker emitted empty IR for:\n{src}");
}
