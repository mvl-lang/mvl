// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Statement and block emission for the TIR-walking path (#1612, Phase 3b PR 1).
//!
//! Parallel to `emit_stmts.rs`. Walks [`TirBlock`] and [`TirStmt`].
//!
//! TIR statements always carry their `span` inline; some variants (e.g. `Let`)
//! also carry the fully-resolved declared `Ty` so the emitter doesn't need to
//! re-infer types from initializers.

use crate::mvl::ir::{TirBlock, TirStmt};

use super::TextEmitter;

impl TextEmitter {
    /// Walk a [`TirBlock`] and emit the trailing expression's SSA register
    /// (mirrors `emit_block(&Block)` semantics).
    #[allow(dead_code)] // wired up via emit_program_tir once that's implemented
    pub(super) fn emit_block_tir(&mut self, block: &TirBlock) -> Result<Option<String>, String> {
        let stmts = &block.stmts;
        if stmts.is_empty() {
            return Ok(None);
        }
        let (head, tail) = stmts.split_at(stmts.len() - 1);
        for s in head {
            self.emit_stmt_tir(s)?;
        }
        match &tail[0] {
            TirStmt::Expr { expr, .. } => self.emit_expr_tir(expr),
            // If/Match-as-statement at block tail: subsequent commits will
            // mirror `emit_if_stmt_chain` and `emit_match_expr` for TIR.
            other => {
                self.emit_stmt_tir(other)?;
                Ok(None)
            }
        }
    }

    /// Walk a [`TirStmt`] for side effects (no value returned).
    ///
    /// Mirror of `emit_stmt(&Stmt)`. Unimplemented variants return an error;
    /// the `cross_backend_tir` test target tolerates these while the walker is
    /// being built out.
    #[allow(dead_code)] // wired up via emit_program_tir once that's implemented
    pub(super) fn emit_stmt_tir(&mut self, stmt: &TirStmt) -> Result<(), String> {
        match stmt {
            TirStmt::Expr { expr, .. } => {
                self.emit_expr_tir(expr)?;
                Ok(())
            }
            // Unimplemented variants — built out in subsequent commits (#1612).
            _ => Err(format!(
                "emit_stmt_tir: variant not yet implemented: {:?}",
                std::mem::discriminant(stmt)
            )),
        }
    }
}
