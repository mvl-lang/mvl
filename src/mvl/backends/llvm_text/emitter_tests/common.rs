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
    LlvmTextCompiler::new()
        .compile_to_ir(&prog, "test")
        .expect("compile_to_ir failed")
}
