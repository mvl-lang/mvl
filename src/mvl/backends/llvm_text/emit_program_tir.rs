// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Program-level emission for the TIR-walking path (#1612, Phase 3b PR 1).
//!
//! Parallel to the top of `emitter.rs::emit_program` (which iterates `Decl`s).
//! Walks a [`TirProgram`] directly: `tir.fns`, `tir.types`, `tir.actors`, etc.
//!
//! Built leaf-first — function-body emission is delegated to `emit_*_tir`
//! submodules. The TIR variants of helpers reuse the existing AST-side
//! helpers where the inputs are shared types (e.g. `TypeExpr`, `Literal`,
//! `Pattern`) re-exported via `crate::mvl::ir`.

use crate::mvl::ir::TirProgram;

use super::TextEmitter;

impl TextEmitter {
    /// Walk a [`TirProgram`] and emit the LLVM IR module body.
    ///
    /// Mirror of `emit_program(&Program)` but consumes already-lowered TIR.
    /// During the parallel-tree migration this returns
    /// `Err("TIR walker not yet implemented for X")` for unimplemented
    /// constructs; the `cross_backend_tir` test target tolerates these
    /// while the implementation is built out.
    pub(super) fn emit_program_tir(&mut self, _prog: &TirProgram) -> Result<(), String> {
        // Stub: full implementation lands in subsequent commits (#1612).
        // Subsequent commits will build out:
        //   - enum/struct/actor pre-pass (mirrors emitter.rs:340-497)
        //   - fn signature registration
        //   - actor decl emission
        //   - fn body emission via emit_fn_tir → emit_block_tir → emit_stmt_tir → emit_expr_tir
        Err("emit_program_tir: not yet implemented (#1612 in progress)".into())
    }
}
