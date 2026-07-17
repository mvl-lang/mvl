// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Shared helpers for the segmented emitter tests (#1612 prep).
//!
//! Re-exports the parent `emitter` module's items so per-concern
//! submodules can `use super::common::*` instead of restating long
//! crate paths.

pub use super::super::*;
pub use crate::mvl::parser::Parser;

/// Compile `entry_src` with `sibling_src` as a sibling module into one flat IR module.
pub fn compile_with_sibling(entry_src: &str, sibling_src: &str) -> String {
    let parse = |src: &str| {
        let (mut p, errs) = Parser::new(src);
        assert!(errs.is_empty(), "lex errors: {errs:?}");
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    };

    let entry_prog = parse(entry_src);
    let sib_prog = parse(sibling_src);

    let mut expr_types = crate::mvl::checker::collect_prelude_expr_types(&[]);
    let cr = crate::mvl::checker::check(&entry_prog);
    expr_types.extend(cr.expr_types);

    let mut sib_types = crate::mvl::checker::collect_prelude_expr_types(&[]);
    sib_types.extend(crate::mvl::checker::check(&sib_prog).expr_types);

    let tir_entry = {
        let all_fns = crate::mvl::passes::mono::collect_fns([&entry_prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&entry_prog, &all_fns, &expr_types);
        crate::mvl::ir::lower::lower(&entry_prog, &mono, &expr_types)
    };
    let tir_sib = {
        let all_fns = crate::mvl::passes::mono::collect_fns([&sib_prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&sib_prog, &all_fns, &sib_types);
        crate::mvl::ir::lower::lower(&sib_prog, &mono, &sib_types)
    };

    let compiler = LlvmTextCompiler::with_context(std::collections::HashMap::new());
    compiler
        .compile_to_ir_with_siblings_tir(&[], &[tir_sib], &tir_entry, "test")
        .expect("compile_to_ir_with_siblings_tir failed")
}

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
