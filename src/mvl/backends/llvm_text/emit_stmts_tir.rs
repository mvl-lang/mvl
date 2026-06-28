// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Statement and block emission for the TIR-walking path (#1612, Phase 3b PR 1).
//!
//! Parallel to `emit_stmts.rs`. Walks [`TirBlock`] and [`TirStmt`].
//!
//! TIR statements always carry their `span` inline; some variants (e.g. `Let`)
//! also carry the fully-resolved declared `Ty` so the emitter doesn't need to
//! re-infer types from initializers.

use crate::mvl::ir::{LetKind, LValue, Pattern, TirBlock, TirElseBranch, TirExpr, TirStmt};

use super::emit_stmts::ty_to_type_expr;
use super::{RefLocal, TextEmitter, MAIN_RET};

impl TextEmitter {
    /// Walk a [`TirBlock`] and emit the trailing expression's SSA register
    /// (mirrors `emit_block(&Block)` semantics).
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
            TirStmt::If {
                cond, then, else_, ..
            } => self.emit_if_stmt_chain_tir(cond, then, else_.as_ref()),
            // Match at tail: ported in a subsequent commit.
            other => {
                self.emit_stmt_tir(other)?;
                Ok(None)
            }
        }
    }

    /// TIR variant of [`Self::emit_if_stmt_chain`].
    ///
    /// Emits an `if`-statement that, at block-tail position, returns a phi value.
    /// Recursively follows `TirElseBranch::If` chains so deep `else if` trees
    /// emit correct IR.
    fn emit_if_stmt_chain_tir(
        &mut self,
        cond: &TirExpr,
        then: &TirBlock,
        else_: Option<&TirElseBranch>,
    ) -> Result<Option<String>, String> {
        match else_ {
            None => self.emit_if_phi_tir_from_blocks(cond, then, None),
            Some(TirElseBranch::Block(b)) => {
                self.emit_if_phi_tir_from_blocks(cond, then, Some(b))
            }
            Some(TirElseBranch::If(nested)) => {
                if let TirStmt::If {
                    cond: ncond,
                    then: nthen,
                    else_: nelse,
                    ..
                } = nested.as_ref()
                {
                    let cond_val = match self.emit_expr_tir(cond)? {
                        Some(v) => v,
                        None => return Ok(None),
                    };
                    let then_bb = self.next_bb("then");
                    let else_bb = self.next_bb("else");
                    let merge_bb = self.next_bb("merge");
                    self.push_instr(&format!(
                        "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
                    ));

                    self.start_bb(&then_bb);
                    let then_val = self.emit_block_tir(then)?;
                    let then_end = self.fn_ctx.current_bb.clone();
                    if !self.fn_ctx.terminated {
                        self.push_instr(&format!("br label %{merge_bb}"));
                    }

                    self.start_bb(&else_bb);
                    let else_val =
                        self.emit_if_stmt_chain_tir(ncond, nthen, nelse.as_ref())?;
                    let else_end = self.fn_ctx.current_bb.clone();
                    if !self.fn_ctx.terminated {
                        self.push_instr(&format!("br label %{merge_bb}"));
                    }

                    self.start_bb(&merge_bb);
                    match (then_val, else_val) {
                        (Some(tv), Some(ev)) => {
                            let phi_ty = self.infer_val_type(&tv);
                            let result = self.next_reg();
                            self.push_instr(&format!(
                                "{result} = phi {phi_ty} [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                            ));
                            self.fn_ctx.reg_types.insert(result.clone(), phi_ty);
                            Ok(Some(result))
                        }
                        _ => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Shared helper: emit if/else with phi merging when both branches yield a
    /// value. Used by both block-tail If statements and If expressions.
    pub(super) fn emit_if_phi_tir_from_blocks(
        &mut self,
        cond: &TirExpr,
        then: &TirBlock,
        else_: Option<&TirBlock>,
    ) -> Result<Option<String>, String> {
        let cond_val = match self.emit_expr_tir(cond)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let then_bb = self.next_bb("then");
        let else_bb = self.next_bb("else");
        let merge_bb = self.next_bb("merge");

        self.push_instr(&format!(
            "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
        ));

        self.start_bb(&then_bb);
        let then_val = self.emit_block_tir(then)?;
        let then_end = self.fn_ctx.current_bb.clone();
        if !self.fn_ctx.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&else_bb);
        let else_val = if let Some(b) = else_ {
            self.emit_block_tir(b)?
        } else {
            None
        };
        let else_end = self.fn_ctx.current_bb.clone();
        if !self.fn_ctx.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&merge_bb);

        match (then_val, else_val) {
            (Some(tv), Some(ev)) => {
                let phi_ty = self.infer_val_type(&tv).clone();
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi {phi_ty} [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                ));
                self.fn_ctx.reg_types.insert(result.clone(), phi_ty);
                Ok(Some(result))
            }
            _ => Ok(None),
        }
    }

    /// Walk a [`TirStmt`] for side effects (no value returned).
    ///
    /// Mirror of `emit_stmt(&Stmt)`. Unimplemented variants return an error;
    /// the `cross_backend_tir` test target tolerates these while the walker is
    /// being built out.
    pub(super) fn emit_stmt_tir(&mut self, stmt: &TirStmt) -> Result<(), String> {
        match stmt {
            TirStmt::Expr { expr, .. } => {
                self.emit_expr_tir(expr)?;
                Ok(())
            }

            TirStmt::Let {
                kind,
                pattern,
                ty,
                init,
                ..
            } => {
                if *kind == LetKind::Ghost {
                    return Ok(());
                }
                let val = self.emit_expr_tir(init)?;
                // Convert TIR `Ty` once at the boundary; the rest reuses the
                // existing AST-shaped helpers (deref_ty, is_mutable_ref, …).
                let ty_te = ty_to_type_expr(ty).unwrap_or_else(|| {
                    // Fallback — shouldn't happen for any user-facing Ty variants.
                    crate::mvl::ir::TypeExpr::Base {
                        name: "Unit".into(),
                        args: Vec::new(),
                        span: crate::mvl::parser::lexer::Span::default(),
                    }
                });
                let elem_ty = Self::deref_ty(&ty_te).clone();

                if Self::is_mutable_ref(&ty_te) {
                    let ty_str = self.llvm_ty_ctx(&elem_ty);
                    if ty_str == "void" {
                        return Ok(());
                    }
                    let ptr = self.next_reg();
                    self.push_instr(&format!("{ptr} = alloca {ty_str}"));
                    if let Some(v) = val {
                        self.push_instr(&format!("store {ty_str} {v}, ptr {ptr}"));
                    }
                    if let Pattern::Ident(name, _) = pattern {
                        if let Some(hk) = Self::heap_kind(&elem_ty) {
                            self.fn_ctx.heap_locals.push((ptr.clone(), hk, true));
                        }
                        self.fn_ctx.ref_locals.insert(
                            name.clone(),
                            RefLocal {
                                ptr,
                                elem_ty: elem_ty.clone(),
                            },
                        );
                    }
                } else if let (Some(v), Pattern::Ident(name, _)) = (val, pattern) {
                    if !self.fn_ctx.reg_types.contains_key(&v) {
                        let ty_str = self.llvm_ty_ctx(&elem_ty);
                        self.fn_ctx.reg_types.insert(v.clone(), ty_str);
                    }
                    if let Some(old_ssa) = self.fn_ctx.locals.get(name) {
                        let old_ssa = old_ssa.clone();
                        self.fn_ctx.heap_locals.retain(|(s, _, _)| *s != old_ssa);
                    }
                    self.fn_ctx.locals.insert(name.clone(), v.clone());
                    if let Some(hk) = Self::heap_kind(&elem_ty) {
                        if !self.fn_ctx.heap_locals.iter().any(|(s, _, _)| s == &v) {
                            self.fn_ctx.heap_locals.push((v, hk, false));
                        }
                    }
                    self.fn_ctx.local_mvl_types.insert(name.clone(), elem_ty);
                }
                Ok(())
            }

            TirStmt::Assign { target, value, .. } => {
                let val = self.emit_expr_tir(value)?;
                if let LValue::Ident(name, _) = target {
                    if let Some(loc) = self.fn_ctx.ref_locals.get(name).cloned() {
                        if let Some(v) = val {
                            let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                            self.push_instr(&format!(
                                "store {ty_str} {v}, ptr {}",
                                loc.ptr
                            ));
                        }
                    }
                }
                Ok(())
            }

            TirStmt::Return { value, .. } => {
                let ret_ty = self.fn_ctx.current_ret_ty.clone();
                let ret_val = if let Some(expr) = value {
                    self.emit_expr_tir(expr)?
                } else {
                    None
                };
                // Heap-drop exclusion needs an AST `Expr`; TIR-side equivalent
                // is built out alongside the heap-drop tracker. Until then, skip
                // exclusion — this is conservative (extra drops are runtime no-ops
                // on already-dropped slots in the AST path's drop ordering).
                self.emit_heap_drops();
                if Self::is_void(&ret_ty) {
                    if self.fn_ctx.current_fn_is_main {
                        self.push_instr(MAIN_RET);
                    } else {
                        self.push_instr("ret void");
                    }
                } else if let Some(v) = ret_val {
                    let ty = self.llvm_ty_ctx(&ret_ty);
                    self.push_instr(&format!("ret {ty} {v}"));
                } else if self.fn_ctx.current_fn_is_main {
                    self.push_instr(MAIN_RET);
                } else {
                    self.push_instr("ret void");
                }
                self.fn_ctx.terminated = true;
                Ok(())
            }

            TirStmt::If {
                cond, then, else_, ..
            } => {
                self.emit_if_stmt_void_tir(cond, then, else_.as_ref())?;
                Ok(())
            }

            TirStmt::While {
                cond, body, ..
            } => self.emit_while_tir(cond, body),

            // Unimplemented: Match, For — built out in subsequent commits.
            _ => Err(format!(
                "emit_stmt_tir: variant not yet implemented: {:?}",
                std::mem::discriminant(stmt)
            )),
        }
    }

    /// TIR variant of [`Self::emit_if_stmt`] — if-as-statement at non-tail
    /// position (no value returned, no phi).
    fn emit_if_stmt_void_tir(
        &mut self,
        cond: &TirExpr,
        then: &TirBlock,
        else_: Option<&TirElseBranch>,
    ) -> Result<(), String> {
        let then_bb = self.next_bb("then");
        let else_bb = self.next_bb("else");
        let merge_bb = self.next_bb("merge");

        let cond_val = match self.emit_expr_tir(cond)? {
            Some(v) => v,
            None => return Ok(()),
        };
        self.push_instr(&format!(
            "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
        ));

        self.start_bb(&then_bb);
        self.emit_block_tir(then)?;
        if !self.fn_ctx.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&else_bb);
        if let Some(e) = else_ {
            match e {
                TirElseBranch::Block(b) => {
                    self.emit_block_tir(b)?;
                }
                TirElseBranch::If(nested) => {
                    self.emit_stmt_tir(nested)?;
                }
            }
        }
        if !self.fn_ctx.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&merge_bb);
        Ok(())
    }

    /// TIR variant of [`Self::emit_while`].
    fn emit_while_tir(&mut self, cond: &TirExpr, body: &TirBlock) -> Result<(), String> {
        let loop_bb = self.next_bb("loop");
        let body_bb = self.next_bb("loop_body");
        let end_bb = self.next_bb("loop_end");

        self.push_instr(&format!("br label %{loop_bb}"));
        self.start_bb(&loop_bb);

        let cond_val = self.emit_expr_tir(cond)?;
        if let Some(cv) = cond_val {
            self.push_instr(&format!("br i1 {cv}, label %{body_bb}, label %{end_bb}"));
        } else {
            self.push_instr(&format!("br label %{end_bb}"));
        }

        self.start_bb(&body_bb);
        self.emit_block_tir(body)?;
        if !self.fn_ctx.terminated {
            self.ensure_yield_check_extern();
            self.push_instr("call void @_mvl_yield_check()");
            self.push_instr(&format!("br label %{loop_bb}"));
        }

        self.start_bb(&end_bb);
        Ok(())
    }
}
