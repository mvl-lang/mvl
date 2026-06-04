// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Statement emission for the `llvm_text` backend.

use crate::mvl::parser::ast::{Block, ElseBranch, Expr, LValue, LetKind, MatchArm, Pattern, Stmt};

use super::{RefLocal, TextEmitter, MAIN_RET};

impl TextEmitter {
    // ── Block emission ────────────────────────────────────────────────────

    pub(super) fn emit_block(&mut self, block: &Block) -> Result<Option<String>, String> {
        let stmts = &block.stmts;
        if stmts.is_empty() {
            return Ok(None);
        }
        let (head, tail) = stmts.split_at(stmts.len() - 1);
        for s in head {
            self.emit_stmt(s)?;
        }
        match &tail[0] {
            Stmt::Expr { expr, .. } => self.emit_expr(expr),
            Stmt::If {
                cond, then, else_, ..
            } => self.emit_if_stmt_chain(cond, then, else_.as_ref()),
            Stmt::Match {
                scrutinee, arms, ..
            } => self.emit_match_expr(scrutinee, arms),
            other => {
                self.emit_stmt(other)?;
                Ok(None)
            }
        }
    }

    /// Emit a `Stmt::If` as an expression, correctly handling `else if` chains.
    ///
    /// Unlike `emit_if_phi` (which only handles `else { block }`), this recursively
    /// follows `ElseBranch::If` chains so that deeply nested `else if` trees produce
    /// correct IR instead of dropping the tail branches.
    pub(super) fn emit_if_stmt_chain(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&ElseBranch>,
    ) -> Result<Option<String>, String> {
        match else_ {
            None => self.emit_if_phi(cond, then, None),
            Some(ElseBranch::Block(b)) => self.emit_if_phi(cond, then, Some(b)),
            Some(ElseBranch::If(nested)) => {
                if let Stmt::If {
                    cond: ncond,
                    then: nthen,
                    else_: nelse,
                    ..
                } = nested.as_ref()
                {
                    let cond_val = match self.emit_expr(cond)? {
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
                    let then_val = self.emit_block(then)?;
                    let then_end = self.current_bb.clone();
                    if !self.terminated {
                        self.push_instr(&format!("br label %{merge_bb}"));
                    }

                    self.start_bb(&else_bb);
                    let else_val = self.emit_if_stmt_chain(ncond, nthen, nelse.as_ref())?;
                    let else_end = self.current_bb.clone();
                    if !self.terminated {
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
                            self.reg_types.insert(result.clone(), phi_ty);
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

    // ── Statement emission ────────────────────────────────────────────────

    pub(super) fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Let {
                kind,
                pattern,
                ty,
                init,
                ..
            } => {
                if *kind == LetKind::Ghost {
                    return Ok(());
                }
                let val = self.emit_expr(init)?;
                let elem_ty = Self::deref_ty(ty).clone();

                if Self::is_mutable_ref(ty) {
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
                        // Track heap-allocated ref locals for drop at function exit.
                        if let Some(hk) = Self::heap_kind(&elem_ty) {
                            self.heap_locals.push((ptr.clone(), hk, true));
                        }
                        self.ref_locals.insert(
                            name.clone(),
                            RefLocal {
                                ptr,
                                elem_ty: elem_ty.clone(),
                            },
                        );
                    }
                } else if let (Some(v), Pattern::Ident(name, _)) = (val, pattern) {
                    let ty_str = self.llvm_ty_ctx(&elem_ty);
                    self.reg_types.insert(v.clone(), ty_str);
                    // If this name shadows a previous heap-allocated binding,
                    // remove the old SSA from heap_locals to prevent double-drop.
                    if let Some(old_ssa) = self.locals.get(name) {
                        let old_ssa = old_ssa.clone();
                        self.heap_locals.retain(|(s, _, _)| *s != old_ssa);
                    }
                    self.locals.insert(name.clone(), v.clone());
                    // Track heap-allocated locals for automatic drop at function exit.
                    // Skip if this SSA is already tracked (consume/move reuses the
                    // source's SSA — adding it again would double-drop).
                    if let Some(hk) = Self::heap_kind(&elem_ty) {
                        if !self.heap_locals.iter().any(|(s, _, _)| s == &v) {
                            self.heap_locals.push((v, hk, false));
                        }
                    }
                    self.local_mvl_types.insert(name.clone(), elem_ty);
                }
                Ok(())
            }

            Stmt::Assign { target, value, .. } => {
                let val = self.emit_expr(value)?;
                if let LValue::Ident(name, _) = target {
                    if let Some(loc) = self.ref_locals.get(name).cloned() {
                        if let Some(v) = val {
                            let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                            self.push_instr(&format!("store {ty_str} {v}, ptr {}", loc.ptr));
                        }
                    }
                }
                Ok(())
            }

            Stmt::Return { value, .. } => {
                let ret_ty = self.current_ret_ty.clone();
                // Evaluate return expression first (if any), then drop once.
                let ret_val = if let Some(expr) = value {
                    self.emit_expr(expr)?
                } else {
                    None
                };
                // Exclude the returned value from drops (move semantics).
                if let Some(expr) = value {
                    self.exclude_returned_value(expr);
                }
                self.emit_heap_drops();
                if Self::is_void(&ret_ty) {
                    if self.current_fn_is_main {
                        self.push_instr(MAIN_RET);
                    } else {
                        self.push_instr("ret void");
                    }
                } else if let Some(v) = ret_val {
                    let ty = self.llvm_ty_ctx(&ret_ty);
                    self.push_instr(&format!("ret {ty} {v}"));
                } else if self.current_fn_is_main {
                    self.push_instr(MAIN_RET);
                } else {
                    self.push_instr("ret void");
                }
                self.terminated = true;
                Ok(())
            }

            Stmt::While { cond, body, .. } => self.emit_while(cond, body),

            Stmt::If {
                cond, then, else_, ..
            } => self.emit_if_stmt(cond, then, else_.as_ref()),

            Stmt::For {
                pattern,
                iter,
                body,
                ..
            } => self.emit_for_stmt(pattern, iter, body),

            Stmt::Match {
                scrutinee, arms, ..
            } => self.emit_match_stmt(scrutinee, arms),

            Stmt::Expr { expr, .. } => {
                self.emit_expr(expr)?;
                Ok(())
            }
        }
    }

    // ── For loop (range only) ─────────────────────────────────────────────

    pub(super) fn emit_for_stmt(
        &mut self,
        pattern: &Pattern,
        iter: &Expr,
        body: &Block,
    ) -> Result<(), String> {
        // Only handle `for var in range(lo, hi)` for Phase 2.
        if let Expr::FnCall { name, args, .. } = iter {
            if name == "range" && args.len() == 2 {
                let var_name = match pattern {
                    Pattern::Ident(n, _) => n.clone(),
                    _ => "_".into(),
                };
                return self.emit_for_range(&var_name, &args[0], &args[1], body);
            }
        }
        Ok(())
    }

    pub(super) fn emit_for_range(
        &mut self,
        var_name: &str,
        lo: &Expr,
        hi: &Expr,
        body: &Block,
    ) -> Result<(), String> {
        let lo_val = match self.emit_expr(lo)? {
            Some(v) => v,
            None => return Ok(()),
        };
        let hi_val = match self.emit_expr(hi)? {
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

        // Bind loop variable (immutable, read-only inside body)
        let old = self.locals.insert(var_name.to_string(), cur_i.clone());
        self.reg_types.insert(cur_i.clone(), "i64".into());
        self.emit_block(body)?;

        // Restore locals
        if let Some(prev) = old {
            self.locals.insert(var_name.to_string(), prev);
        } else {
            self.locals.remove(var_name);
        }

        if !self.terminated {
            let next_i = self.next_reg();
            self.push_instr(&format!("{next_i} = add i64 {cur_i}, 1"));
            self.push_instr(&format!("store i64 {next_i}, ptr {i_ptr}"));
            self.push_instr(&format!("br label %{cond_bb}"));
        }

        self.start_bb(&end_bb);
        Ok(())
    }

    // ── While loop ────────────────────────────────────────────────────────

    pub(super) fn emit_while(&mut self, cond: &Expr, body: &Block) -> Result<(), String> {
        let loop_bb = self.next_bb("loop");
        let body_bb = self.next_bb("loop_body");
        let end_bb = self.next_bb("loop_end");

        self.push_instr(&format!("br label %{loop_bb}"));
        self.start_bb(&loop_bb);

        let cond_val = self.emit_expr(cond)?;
        if let Some(cv) = cond_val {
            self.push_instr(&format!("br i1 {cv}, label %{body_bb}, label %{end_bb}"));
        } else {
            self.push_instr(&format!("br label %{end_bb}"));
        }

        self.start_bb(&body_bb);
        self.emit_block(body)?;
        if !self.terminated {
            self.push_instr(&format!("br label %{loop_bb}"));
        }

        self.start_bb(&end_bb);
        Ok(())
    }

    // ── If-statement (void, no phi) ───────────────────────────────────────

    pub(super) fn emit_if_stmt(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&ElseBranch>,
    ) -> Result<(), String> {
        let then_bb = self.next_bb("then");
        let else_bb = self.next_bb("else");
        let merge_bb = self.next_bb("merge");

        let cond_val = match self.emit_expr(cond)? {
            Some(v) => v,
            None => return Ok(()),
        };
        self.push_instr(&format!(
            "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
        ));

        self.start_bb(&then_bb);
        self.emit_block(then)?;
        if !self.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&else_bb);
        if let Some(e) = else_ {
            match e {
                ElseBranch::Block(b) => {
                    self.emit_block(b)?;
                }
                ElseBranch::If(stmt) => {
                    self.emit_stmt(stmt)?;
                }
            }
        }
        if !self.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&merge_bb);
        Ok(())
    }

    // ── Match (statement, void) ───────────────────────────────────────────

    pub(super) fn emit_match_stmt(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
    ) -> Result<(), String> {
        self.emit_match_expr(scrutinee, arms)?;
        Ok(())
    }

    // ── Match (expression, produces value) ───────────────────────────────
}
