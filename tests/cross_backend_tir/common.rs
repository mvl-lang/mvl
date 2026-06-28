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
/// During the migration period (#1612), this helper is *lenient* — it accepts
/// either:
/// - byte-identical IR (full parity, the goal), or
/// - a "not yet implemented" / "not yet ported" error from the TIR walker
///   (the variant is still on the TODO list).
///
/// It only *fails* on:
/// - TIR walker succeeds with diverged IR (a real bug like the #1612 extern
///   regression we caught), or
/// - TIR walker errors with an unexpected message (not a known placeholder).
///
/// As variants are ported, individual tests automatically transition from
/// "errored cleanly" to "real parity check" with no test edit needed.
/// When the TIR walker covers every variant, `assert_tir_strict_parity`
/// (without the unimplemented escape hatch) becomes the entry point —
/// at which point PR 2 of #1612 can flip the CLI default.
pub fn assert_tir_parity(src: &str) {
    let prog = parse(src);
    let (tir, compiler) = lower_to_tir(&prog);
    let ast_ir = compiler.compile_to_ir(&prog, "test").expect("AST path failed");
    match compiler.compile_to_ir_tir(&tir, "test") {
        Ok(tir_ir) => {
            assert_eq!(
                ast_ir, tir_ir,
                "TIR walker output diverged from AST walker"
            );
        }
        Err(msg)
            if msg.contains("not yet implemented") || msg.contains("not yet ported") =>
        {
            // Expected during migration; will tighten to strict parity in PR 2.
        }
        Err(msg) => panic!("unexpected error from TIR walker: {msg}"),
    }
}

/// Strict parity — used by tests that already pass full parity and want to
/// regress-guard against unimplemented errors creeping back in.
#[allow(dead_code)]
pub fn assert_tir_strict_parity(src: &str) {
    let prog = parse(src);
    let (tir, compiler) = lower_to_tir(&prog);
    let ast_ir = compiler.compile_to_ir(&prog, "test").expect("AST path failed");
    let tir_ir = compiler
        .compile_to_ir_tir(&tir, "test")
        .expect("TIR path failed");
    assert_eq!(ast_ir, tir_ir, "TIR walker output diverged from AST walker");
}

/// Assert the TIR walker reports unimplemented-variant error for `src`.
/// Used for inputs that exercise variants not yet ported.
#[allow(dead_code)]
pub fn assert_tir_unimplemented(src: &str) {
    let prog = parse(src);
    let (tir, compiler) = lower_to_tir(&prog);
    let result = compiler.compile_to_ir_tir(&tir, "test");
    match result {
        Err(msg) if msg.contains("not yet implemented") || msg.contains("not yet ported") => {}
        Err(msg) => panic!("unexpected error from TIR walker: {msg}"),
        Ok(_) => panic!("TIR walker unexpectedly succeeded — promote to assert_tir_parity"),
    }
}
