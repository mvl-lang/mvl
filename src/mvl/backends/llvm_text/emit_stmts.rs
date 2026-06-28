// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Statement emission for the `llvm_text` backend.

use crate::mvl::parser::ast::{
    Block, ElseBranch, Expr, LValue, LetKind, MatchArm, Pattern, Stmt, TypeExpr,
};
use crate::mvl::parser::lexer::Span;

use super::{RefLocal, TextEmitter, MAIN_RET};

/// Synthesize a `TypeExpr` from a checker-resolved `Ty` so loop variables
/// land in `local_mvl_types` and method dispatch can find the right kind.
///
/// Returns `None` for compound types we don't yet need to round-trip
/// (e.g. `Ty::Fn`, `Ty::Session`).
fn ty_to_type_expr(ty: &crate::mvl::checker::types::Ty) -> Option<TypeExpr> {
    use crate::mvl::checker::types::Ty;
    let base = |name: &str, args: Vec<TypeExpr>| TypeExpr::Base {
        name: name.into(),
        args,
        span: Span::default(),
    };
    Some(match ty {
        Ty::Int => base("Int", vec![]),
        Ty::UInt => base("UInt", vec![]),
        Ty::Float => base("Float", vec![]),
        Ty::Bool => base("Bool", vec![]),
        Ty::Byte => base("Byte", vec![]),
        Ty::UByte => base("UByte", vec![]),
        Ty::Char => base("Char", vec![]),
        Ty::Unit => base("Unit", vec![]),
        Ty::String => base("String", vec![]),
        Ty::List(inner) => base("List", vec![ty_to_type_expr(inner)?]),
        Ty::Array(inner, _) => base("Array", vec![ty_to_type_expr(inner)?]),
        Ty::Set(inner) => base("Set", vec![ty_to_type_expr(inner)?]),
        Ty::Map(k, v) => base("Map", vec![ty_to_type_expr(k)?, ty_to_type_expr(v)?]),
        Ty::Option(inner) => TypeExpr::Option {
            inner: Box::new(ty_to_type_expr(inner)?),
            span: Span::default(),
        },
        Ty::Result(ok, err) => TypeExpr::Result {
            ok: Box::new(ty_to_type_expr(ok)?),
            err: Box::new(ty_to_type_expr(err)?),
            span: Span::default(),
        },
        Ty::Ref(mutable, inner) => TypeExpr::Ref {
            mutable: *mutable,
            inner: Box::new(ty_to_type_expr(inner)?),
            span: Span::default(),
        },
        Ty::Named(name, args) => base(name, args.iter().filter_map(ty_to_type_expr).collect()),
        _ => return None,
    })
}

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
                    let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

                    self.start_bb(&then_bb);
                    let then_val = self.emit_block(then)?;
                    let then_end = self.fn_ctx.current_bb.clone();
                    if !self.fn_ctx.terminated {
                        self.drop_scope_locals(heap_locals_snapshot, then_val.as_deref());
                        self.push_instr(&format!("br label %{merge_bb}"));
                    } else {
                        self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
                    }

                    self.start_bb(&else_bb);
                    let else_val = self.emit_if_stmt_chain(ncond, nthen, nelse.as_ref())?;
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
                    // Only set reg_types if the register doesn't already have
                    // an entry.  The emitter that produced the value (e.g.
                    // emit_propagate for `?`) already recorded the correct
                    // LLVM-level type; the MVL-derived type from `llvm_ty_ctx`
                    // may disagree (e.g. `%Child` vs the actual `ptr` from a
                    // C-ABI Result extraction).
                    if !self.fn_ctx.reg_types.contains_key(&v) {
                        let ty_str = self.llvm_ty_ctx(&elem_ty);
                        self.fn_ctx.reg_types.insert(v.clone(), ty_str);
                    }
                    // If this name shadows a previous heap-allocated binding,
                    // remove the old SSA from heap_locals to prevent double-drop.
                    if let Some(old_ssa) = self.fn_ctx.locals.get(name) {
                        let old_ssa = old_ssa.clone();
                        self.fn_ctx.heap_locals.retain(|(s, _, _)| *s != old_ssa);
                    }
                    self.fn_ctx.locals.insert(name.clone(), v.clone());
                    // Track heap-allocated locals for automatic drop at function exit.
                    // Skip if this SSA is already tracked (consume/move reuses the
                    // source's SSA — adding it again would double-drop).
                    if let Some(hk) = Self::heap_kind(&elem_ty) {
                        if !self.fn_ctx.heap_locals.iter().any(|(s, _, _)| s == &v) {
                            self.fn_ctx.heap_locals.push((v, hk, false));
                        }
                    }
                    self.fn_ctx.local_mvl_types.insert(name.clone(), elem_ty);
                }
                Ok(())
            }

            Stmt::Assign { target, value, .. } => {
                let val = self.emit_expr(value)?;
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

            Stmt::Return { value, .. } => {
                let ret_ty = self.fn_ctx.current_ret_ty.clone();
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

    // ── For loop (range and List) ─────────────────────────────────────────

    pub(super) fn emit_for_stmt(
        &mut self,
        pattern: &Pattern,
        iter: &Expr,
        body: &Block,
    ) -> Result<(), String> {
        // `for var in range(lo, hi)` — integer range loop.
        if let Expr::FnCall { name, args, .. } = iter {
            if name == "range" && args.len() == 2 {
                let var_name = match pattern {
                    Pattern::Ident(n, _) => n.clone(),
                    _ => "_".into(),
                };
                return self.emit_for_range(&var_name, &args[0], &args[1], body);
            }
        }
        // `for var in <list-expr>` — list / array / set iteration (#1546).
        let var_name = match pattern {
            Pattern::Ident(n, _) => n.clone(),
            _ => "_".into(),
        };
        self.emit_for_list(&var_name, iter, body)
    }

    /// Emit a `for x in <list-expr> { body }` loop.
    ///
    /// The iterable must lower to a `ptr` to a runtime `MvlArray`; this covers
    /// `List[T]`, `Array[T, N]`, and `Set[T]`. Element type is taken from the
    /// checker-resolved `expr_types` map and defaults to `i64` when unavailable.
    pub(super) fn emit_for_list(
        &mut self,
        var_name: &str,
        iter: &Expr,
        body: &Block,
    ) -> Result<(), String> {
        use crate::mvl::checker::types::Ty;

        let arr = match self.emit_expr(iter)? {
            Some(v) => v,
            None => return Ok(()),
        };

        // Resolve the element type — Ty::List | Ty::Array | Ty::Set | Ty::Ref(_, inner).
        let (elem_ty_opt, elem_llvm_ty): (Option<Ty>, String) =
            match self.module.expr_types.get(&iter.span()) {
                Some(ty) => {
                    let inner = match ty {
                        Ty::Ref(_, t) => t.as_ref(),
                        other => other,
                    };
                    match inner {
                        Ty::List(e) | Ty::Array(e, _) | Ty::Set(e) => {
                            (Some((**e).clone()), self.ty_to_llvm_ctx(e))
                        }
                        _ => (None, "i64".into()),
                    }
                }
                None => (None, "i64".into()),
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

        // Load element at index cur_i.
        let elem_ptr = self.next_reg();
        self.push_instr(&format!(
            "{elem_ptr} = call ptr @_mvl_array_get(ptr {arr}, i64 {cur_i})"
        ));
        let elem_val = self.next_reg();
        self.push_instr(&format!("{elem_val} = load {elem_llvm_ty}, ptr {elem_ptr}"));
        self.fn_ctx
            .reg_types
            .insert(elem_val.clone(), elem_llvm_ty.clone());

        // Bind loop variable (immutable).
        let old_local = self
            .fn_ctx
            .locals
            .insert(var_name.to_string(), elem_val.clone());
        let old_mvl_ty = elem_ty_opt
            .as_ref()
            .and_then(ty_to_type_expr)
            .and_then(|te| self.fn_ctx.local_mvl_types.insert(var_name.to_string(), te));

        // Snapshot heap_locals: anything pushed during the body lives only
        // for one iteration. Drop them at the loop tail and truncate back to
        // the snapshot so the function-end drop pass doesn't try to drop
        // SSAs from a block that may not have executed (#1617).
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

        self.emit_block(body)?;

        // Restore locals.
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
            // Body terminated (e.g. early return) — its emit_heap_drops will
            // already have emitted drops; just discard the entries so the
            // outer function-end pass doesn't double-drop.
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&end_bb);
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
        let old = self
            .fn_ctx
            .locals
            .insert(var_name.to_string(), cur_i.clone());
        self.fn_ctx.reg_types.insert(cur_i.clone(), "i64".into());
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();
        self.emit_block(body)?;

        // Restore locals
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
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();
        self.emit_block(body)?;
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

        // Snapshot heap_locals so branch-local lets get dropped at the end of
        // their branch instead of leaking into the function-end drop pass —
        // where they would be dropped from blocks that may not have executed
        // (#1617).
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

        self.start_bb(&then_bb);
        self.emit_block(then)?;
        if !self.fn_ctx.terminated {
            self.drop_scope_locals(heap_locals_snapshot, None);
            self.push_instr(&format!("br label %{merge_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
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
        if !self.fn_ctx.terminated {
            self.drop_scope_locals(heap_locals_snapshot, None);
            self.push_instr(&format!("br label %{merge_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
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
