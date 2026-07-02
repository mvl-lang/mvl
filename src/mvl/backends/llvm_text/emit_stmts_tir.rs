// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Statement and block emission for the TIR-walking path (#1612, Phase 3b PR 1).
//!
//! Parallel to `emit_stmts.rs`. Walks [`TirBlock`] and [`TirStmt`].
//!
//! TIR statements always carry their `span` inline; some variants (e.g. `Let`)
//! also carry the fully-resolved declared `Ty` so the emitter doesn't need to
//! re-infer types from initializers.

use crate::mvl::ir::{
    LValue, LetKind, Pattern, TirBlock, TirElseBranch, TirExpr, TirExprKind, TirStmt,
};

use super::emit_helpers::ty_to_type_expr;
use super::{RefLocal, TextEmitter, MAIN_RET};

impl TextEmitter {
    /// TIR variant of [`Self::exclude_returned_value`] — walks a `TirExpr`.
    ///
    /// Removes the heap-local entry for a value about to be returned (moved
    /// out of the function), so the subsequent `emit_heap_drops` does not
    /// free what now belongs to the caller. Matches the AST-side rules in
    /// `emit_types.rs::exclude_returned_value`: only `Var` is the canonical
    /// owning expression; `Consume` / `Relabel` are transparent wrappers.
    pub(super) fn exclude_returned_value_tir(&mut self, expr: &TirExpr) {
        match &expr.kind {
            TirExprKind::Var(name) => {
                if let Some(loc) = self.fn_ctx.ref_locals.get(name) {
                    let ptr = loc.ptr.clone();
                    self.fn_ctx.heap_locals.retain(|(s, _, _)| *s != ptr);
                    return;
                }
                if let Some(ssa) = self.fn_ctx.locals.get(name) {
                    let ssa = ssa.clone();
                    self.fn_ctx.heap_locals.retain(|(s, _, _)| *s != ssa);
                }
            }
            TirExprKind::Consume(inner) | TirExprKind::Relabel { expr: inner, .. } => {
                self.exclude_returned_value_tir(inner);
            }
            _ => {}
        }
    }

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
            TirStmt::Match {
                scrutinee, arms, ..
            } => self.emit_match_expr_tir(scrutinee, arms),
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
            Some(TirElseBranch::Block(b)) => self.emit_if_phi_tir_from_blocks(cond, then, Some(b)),
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
                    // Branch heap_locals must not leak past merge_bb (#1617).
                    let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

                    self.start_bb(&then_bb);
                    let then_val = self.emit_block_tir(then)?;
                    let then_end = self.fn_ctx.current_bb.clone();
                    if !self.fn_ctx.terminated {
                        self.drop_scope_locals(heap_locals_snapshot, then_val.as_deref());
                        self.push_instr(&format!("br label %{merge_bb}"));
                    } else {
                        self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
                    }

                    self.start_bb(&else_bb);
                    let else_val = self.emit_if_stmt_chain_tir(ncond, nthen, nelse.as_ref())?;
                    let else_end = self.fn_ctx.current_bb.clone();
                    if !self.fn_ctx.terminated {
                        self.drop_scope_locals(heap_locals_snapshot, else_val.as_deref());
                        self.push_instr(&format!("br label %{merge_bb}"));
                    } else {
                        self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
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

        // Branch heap_locals must not leak past merge_bb (#1617).
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

        self.start_bb(&then_bb);
        let then_val = self.emit_block_tir(then)?;
        let then_end = self.fn_ctx.current_bb.clone();
        if !self.fn_ctx.terminated {
            self.drop_scope_locals(heap_locals_snapshot, then_val.as_deref());
            self.push_instr(&format!("br label %{merge_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&else_bb);
        let else_val = if let Some(b) = else_ {
            self.emit_block_tir(b)?
        } else {
            None
        };
        let else_end = self.fn_ctx.current_bb.clone();
        if !self.fn_ctx.terminated {
            self.drop_scope_locals(heap_locals_snapshot, else_val.as_deref());
            self.push_instr(&format!("br label %{merge_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
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
                    // Hoist to entry block when the binding is inside a branch BB
                    // so the alloca dominates all uses including cross-arm drops (#1645).
                    // Loop bodies manage their own heap scope via heap_locals snapshots
                    // so their allocas don't need hoisting — emit inline instead.
                    let bb = &self.fn_ctx.current_bb;
                    let in_loop_body = bb.starts_with("loop_body")
                        || bb.starts_with("for_body")
                        || bb.starts_with("for_list_body");
                    if bb == "entry" || in_loop_body {
                        self.push_instr(&format!("{ptr} = alloca {ty_str}"));
                    } else {
                        self.fn_ctx
                            .pre_allocas
                            .push(format!("  {ptr} = alloca {ty_str}"));
                    }
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
                            self.push_instr(&format!("store {ty_str} {v}, ptr {}", loc.ptr));
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
                if let Some(expr) = value {
                    self.exclude_returned_value_tir(expr);
                }
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

            TirStmt::While { cond, body, .. } => self.emit_while_tir(cond, body),

            TirStmt::For {
                pattern,
                iter,
                body,
                ..
            } => self.emit_for_stmt_tir(pattern, iter, body),

            TirStmt::Match {
                scrutinee, arms, ..
            } => {
                self.emit_match_expr_tir(scrutinee, arms)?;
                Ok(())
            }
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

        // Branch heap_locals must not leak past merge_bb — see emit_stmts.rs
        // (`emit_if_stmt`) and #1617. Without the snapshot/drop discipline the
        // function-end drop pass would emit `_mvl_string_drop(%v)` against an
        // SSA value that is only defined in the then-block, violating LLVM
        // dominance when the else-branch reaches the merge.
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

        self.start_bb(&then_bb);
        self.emit_block_tir(then)?;
        if !self.fn_ctx.terminated {
            self.drop_scope_locals(heap_locals_snapshot, None);
            self.push_instr(&format!("br label %{merge_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
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
            self.drop_scope_locals(heap_locals_snapshot, None);
            self.push_instr(&format!("br label %{merge_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&merge_bb);
        Ok(())
    }

    /// TIR variant of [`Self::emit_for_stmt`].
    ///
    /// Dispatches to `emit_for_range_tir` when the iterator is a `range(lo, hi)`
    /// FnCall; otherwise delegates to `emit_for_list_tir`. Receiver/iter type
    /// reads use `iter.ty` directly — no expr_types lookup needed.
    fn emit_for_stmt_tir(
        &mut self,
        pattern: &Pattern,
        iter: &TirExpr,
        body: &TirBlock,
    ) -> Result<(), String> {
        // `for var in range(lo, hi)` — integer range loop.
        if let crate::mvl::ir::TirExprKind::FnCall { name, args, .. } = &iter.kind {
            if name == "range" && args.len() == 2 {
                let var_name = match pattern {
                    Pattern::Ident(n, _) => n.clone(),
                    _ => "_".into(),
                };
                return self.emit_for_range_tir(&var_name, &args[0], &args[1], body);
            }
        }
        // `for var in <list-expr>` — list / array / set iteration (#1546).
        let var_name = match pattern {
            Pattern::Ident(n, _) => n.clone(),
            _ => "_".into(),
        };
        self.emit_for_list_tir(&var_name, iter, body)
    }

    /// TIR variant of [`Self::emit_for_range`].
    fn emit_for_range_tir(
        &mut self,
        var_name: &str,
        lo: &TirExpr,
        hi: &TirExpr,
        body: &TirBlock,
    ) -> Result<(), String> {
        let lo_val = match self.emit_expr_tir(lo)? {
            Some(v) => v,
            None => return Ok(()),
        };
        let hi_val = match self.emit_expr_tir(hi)? {
            Some(v) => v,
            None => return Ok(()),
        };

        let i_ptr = self.next_reg();
        self.push_instr(&format!("{i_ptr} = alloca i64"));
        self.push_instr(&format!("store i64 {lo_val}, ptr {i_ptr}"));

        let cond_bb = self.next_bb("for_cond");
        let body_bb = self.next_bb("for_body");
        let end_bb = self.next_bb("for_end");

        self.push_instr(&format!("br label %{cond_bb}"));
        self.start_bb(&cond_bb);

        let cur_i = self.next_reg();
        self.push_instr(&format!("{cur_i} = load i64, ptr {i_ptr}"));

        let cond_reg = self.next_reg();
        self.push_instr(&format!("{cond_reg} = icmp slt i64 {cur_i}, {hi_val}"));
        self.push_instr(&format!(
            "br i1 {cond_reg}, label %{body_bb}, label %{end_bb}"
        ));

        self.start_bb(&body_bb);

        let old = self
            .fn_ctx
            .locals
            .insert(var_name.to_string(), cur_i.clone());
        self.fn_ctx.reg_types.insert(cur_i.clone(), "i64".into());
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();
        self.emit_block_tir(body)?;

        if let Some(prev) = old {
            self.fn_ctx.locals.insert(var_name.to_string(), prev);
        } else {
            self.fn_ctx.locals.remove(var_name);
        }

        if !self.fn_ctx.terminated {
            self.drop_loop_body_locals(heap_locals_snapshot);
            let next_i = self.next_reg();
            self.push_instr(&format!("{next_i} = add i64 {cur_i}, 1"));
            self.push_instr(&format!("store i64 {next_i}, ptr {i_ptr}"));
            self.ensure_yield_check_extern();
            self.push_instr("call void @_mvl_yield_check()");
            self.push_instr(&format!("br label %{cond_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&end_bb);
        Ok(())
    }

    /// TIR variant of [`Self::emit_for_list`].
    ///
    /// Element type comes from `iter.ty` (unwrapping `Ref` / `Labeled` /
    /// `Refined` then matching `Ty::List(e)` / `Array(e, _)` / `Set(e)`),
    /// instead of the AST `expr_types.get(iter.span())` lookup.
    fn emit_for_list_tir(
        &mut self,
        var_name: &str,
        iter: &TirExpr,
        body: &TirBlock,
    ) -> Result<(), String> {
        use crate::mvl::ir::Ty;

        let arr = match self.emit_expr_tir(iter)? {
            Some(v) => v,
            None => return Ok(()),
        };

        // Unwrap label/refinement/ref wrappers, then match List/Array/Set.
        let mut cur = &iter.ty;
        while let Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) = cur {
            cur = inner;
        }
        let (elem_ty_opt, elem_llvm_ty): (Option<Ty>, String) = match cur {
            Ty::List(e) | Ty::Array(e, _) | Ty::Set(e) => {
                ((**e).clone().into(), self.ty_to_llvm_ctx(e))
            }
            _ => (None, "i64".into()),
        };

        self.ensure_extern("declare i64 @_mvl_array_len(ptr)");
        self.ensure_extern("declare ptr @_mvl_array_get(ptr, i64)");

        let len_reg = self.next_reg();
        self.push_instr(&format!("{len_reg} = call i64 @_mvl_array_len(ptr {arr})"));

        let i_ptr = self.next_reg();
        self.push_instr(&format!("{i_ptr} = alloca i64"));
        self.push_instr(&format!("store i64 0, ptr {i_ptr}"));

        let cond_bb = self.next_bb("for_list_cond");
        let body_bb = self.next_bb("for_list_body");
        let end_bb = self.next_bb("for_list_end");

        self.push_instr(&format!("br label %{cond_bb}"));
        self.start_bb(&cond_bb);

        let cur_i = self.next_reg();
        self.push_instr(&format!("{cur_i} = load i64, ptr {i_ptr}"));
        let cond_reg = self.next_reg();
        self.push_instr(&format!("{cond_reg} = icmp slt i64 {cur_i}, {len_reg}"));
        self.push_instr(&format!(
            "br i1 {cond_reg}, label %{body_bb}, label %{end_bb}"
        ));

        self.start_bb(&body_bb);

        let elem_ptr = self.next_reg();
        self.push_instr(&format!(
            "{elem_ptr} = call ptr @_mvl_array_get(ptr {arr}, i64 {cur_i})"
        ));
        let elem_val = self.next_reg();
        self.push_instr(&format!("{elem_val} = load {elem_llvm_ty}, ptr {elem_ptr}"));
        self.fn_ctx
            .reg_types
            .insert(elem_val.clone(), elem_llvm_ty.clone());

        let old_local = self
            .fn_ctx
            .locals
            .insert(var_name.to_string(), elem_val.clone());
        let old_mvl_ty = elem_ty_opt
            .as_ref()
            .and_then(ty_to_type_expr)
            .and_then(|te| self.fn_ctx.local_mvl_types.insert(var_name.to_string(), te));

        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

        self.emit_block_tir(body)?;

        if let Some(prev) = old_local {
            self.fn_ctx.locals.insert(var_name.to_string(), prev);
        } else {
            self.fn_ctx.locals.remove(var_name);
        }
        if let Some(prev) = old_mvl_ty {
            self.fn_ctx
                .local_mvl_types
                .insert(var_name.to_string(), prev);
        } else {
            self.fn_ctx.local_mvl_types.remove(var_name);
        }

        if !self.fn_ctx.terminated {
            self.drop_loop_body_locals(heap_locals_snapshot);
            let next_i = self.next_reg();
            self.push_instr(&format!("{next_i} = add i64 {cur_i}, 1"));
            self.push_instr(&format!("store i64 {next_i}, ptr {i_ptr}"));
            self.ensure_yield_check_extern();
            self.push_instr("call void @_mvl_yield_check()");
            self.push_instr(&format!("br label %{cond_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&end_bb);
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

        // Snapshot heap_locals before the body so any lets inside the loop are
        // dropped at the back-edge, matching the AST fix for #1617 (#1645).
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();
        self.start_bb(&body_bb);
        self.emit_block_tir(body)?;
        if !self.fn_ctx.terminated {
            self.drop_loop_body_locals(heap_locals_snapshot);
            self.ensure_yield_check_extern();
            self.push_instr("call void @_mvl_yield_check()");
            self.push_instr(&format!("br label %{loop_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&end_bb);
        Ok(())
    }
}
