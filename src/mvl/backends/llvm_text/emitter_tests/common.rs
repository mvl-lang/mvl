// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Shared helpers for the segmented emitter tests (#1612 prep).
//!
//! Re-exports the parent `emitter` module's items so per-concern
//! submodules can `use super::common::*` instead of restating long
//! crate paths.

pub use super::super::*;
pub use crate::mvl::parser::Parser;

pub fn compile(src: &str) -> String {
    let (mut p, errs) = Parser::new(src);
    assert!(errs.is_empty(), "lex errors: {errs:?}");
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    let mut expr_types = crate::mvl::checker::collect_prelude_expr_types(&[]);
    let cr = crate::mvl::checker::check(&prog);
    expr_types.extend(cr.expr_types);
    let compiler = LlvmTextCompiler::with_context(std::collections::HashMap::new());
    let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
    let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
    let tir = crate::mvl::ir::lower::lower(&prog, &mono, &expr_types);
    compiler
        .compile_to_ir_tir(&tir, "test")
        .expect("compile_to_ir_tir failed")
}
