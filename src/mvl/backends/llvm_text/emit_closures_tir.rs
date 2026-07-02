// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Lambda and closure lowering for the TIR-walking path (#1612, Phase 3b PR 1).
//!
//! Parallel to `emit_closures.rs`. Splits into two concerns:
//!
//! 1. **Capture analysis** (`collect_lambda_captures_tir` /
//!    `walk_tir_expr_for_captures` / `walk_tir_block_for_captures`) —
//!    recursively walks a [`TirExpr`] / [`TirBlock`] to find free
//!    variables that live in the outer function's `locals`/`ref_locals`
//!    and aren't shadowed by inner lambda parameters.
//!
//! 2. **Lambda emission** (`emit_lambda_tir` / `emit_lambda_inner_tir`)
//!    — synthesises a top-level LLVM function for the lambda body,
//!    builds the env struct in the outer function, and assembles the
//!    `%__closure_type` struct holding the function pointer and env ptr.
//!
//! Mirrors `emit_closures.rs` line-for-line except:
//! - Captures store `(name, Ty)` initially and the [`TypeExpr`] used by
//!   the existing `llvm_ty_ctx` helpers is reconstructed via
//!   `ty_to_type_expr` at the emission boundary.
//! - The TIR walker reads `expr.ty` directly for the body's return type
//!   when no annotation is present (which is always the case in TIR —
//!   `TirExprKind::Lambda` doesn't carry a separate `ret_type`).

use std::collections::HashSet;

use crate::mvl::ir::{
    TirBlock, TirElseBranch, TirExpr, TirExprKind, TirMatchBody, TirParam, TirStmt, TypeExpr,
};
use crate::mvl::parser::lexer::Span;

use super::emit_helpers::ty_to_type_expr;
use super::TextEmitter;

impl TextEmitter {
    // ── Capture analysis ──────────────────────────────────────────────────

    /// Collect free variables referenced in `body` that live in the outer
    /// function's `locals`/`ref_locals` and aren't shadowed by `exclude`.
    pub(super) fn collect_lambda_captures_tir(
        &self,
        body: &TirExpr,
        exclude: &HashSet<String>,
    ) -> Vec<(String, TypeExpr)> {
        let mut seen = HashSet::new();
        let mut caps = Vec::new();
        self.walk_tir_expr_for_captures(body, exclude, &mut seen, &mut caps);
        caps
    }

    fn capture_var_if_local(
        &self,
        name: &str,
        exclude: &HashSet<String>,
        seen: &mut HashSet<String>,
        caps: &mut Vec<(String, TypeExpr)>,
    ) {
        if exclude.contains(name) || seen.contains(name) {
            return;
        }
        if !self.fn_ctx.locals.contains_key(name) && !self.fn_ctx.ref_locals.contains_key(name) {
            return;
        }
        let ty_opt = self.fn_ctx.local_mvl_types.get(name).cloned().or_else(|| {
            self.fn_ctx
                .ref_locals
                .get(name)
                .map(|rl| rl.elem_ty.clone())
        });
        if let Some(ty) = ty_opt {
            seen.insert(name.to_string());
            caps.push((name.to_string(), ty));
        }
    }

    fn walk_tir_expr_for_captures(
        &self,
        expr: &TirExpr,
        exclude: &HashSet<String>,
        seen: &mut HashSet<String>,
        caps: &mut Vec<(String, TypeExpr)>,
    ) {
        match &expr.kind {
            TirExprKind::Var(name) => {
                self.capture_var_if_local(name, exclude, seen, caps);
            }
            TirExprKind::Lambda { params, body } => {
                let mut inner_excl = exclude.clone();
                for p in params {
                    inner_excl.insert(p.name.clone());
                }
                self.walk_tir_expr_for_captures(body, &inner_excl, seen, caps);
            }
            TirExprKind::Binary { left, right, .. } => {
                self.walk_tir_expr_for_captures(left, exclude, seen, caps);
                self.walk_tir_expr_for_captures(right, exclude, seen, caps);
            }
            TirExprKind::Unary { expr, .. } => {
                self.walk_tir_expr_for_captures(expr, exclude, seen, caps);
            }
            TirExprKind::FnCall { name, args, .. } => {
                // If the callee is a local closure binding, capture it too.
                self.capture_var_if_local(name, exclude, seen, caps);
                for a in args {
                    self.walk_tir_expr_for_captures(a, exclude, seen, caps);
                }
            }
            TirExprKind::MethodCall { receiver, args, .. } => {
                self.walk_tir_expr_for_captures(receiver, exclude, seen, caps);
                for a in args {
                    self.walk_tir_expr_for_captures(a, exclude, seen, caps);
                }
            }
            TirExprKind::FieldAccess { expr, .. } => {
                self.walk_tir_expr_for_captures(expr, exclude, seen, caps);
            }
            TirExprKind::If { cond, then, else_ } => {
                self.walk_tir_expr_for_captures(cond, exclude, seen, caps);
                self.walk_tir_block_for_captures(then, exclude, seen, caps);
                if let Some(e) = else_ {
                    self.walk_tir_expr_for_captures(e, exclude, seen, caps);
                }
            }
            TirExprKind::Block(b) => self.walk_tir_block_for_captures(b, exclude, seen, caps),
            TirExprKind::Construct { fields, .. } => {
                for (_, v) in fields {
                    self.walk_tir_expr_for_captures(v, exclude, seen, caps);
                }
            }
            TirExprKind::Match { scrutinee, arms } => {
                self.walk_tir_expr_for_captures(scrutinee, exclude, seen, caps);
                for arm in arms {
                    match &arm.body {
                        TirMatchBody::Expr(e) => {
                            self.walk_tir_expr_for_captures(e, exclude, seen, caps);
                        }
                        TirMatchBody::Block(b) => {
                            self.walk_tir_block_for_captures(b, exclude, seen, caps);
                        }
                    }
                }
            }
            TirExprKind::Consume(inner)
            | TirExprKind::Propagate(inner)
            | TirExprKind::Borrow { expr: inner, .. } => {
                self.walk_tir_expr_for_captures(inner, exclude, seen, caps);
            }
            TirExprKind::Relabel { expr, .. } => {
                self.walk_tir_expr_for_captures(expr, exclude, seen, caps);
            }
            TirExprKind::List { elems } | TirExprKind::Set { elems } => {
                for e in elems {
                    self.walk_tir_expr_for_captures(e, exclude, seen, caps);
                }
            }
            TirExprKind::Map { pairs } => {
                for (k, v) in pairs {
                    self.walk_tir_expr_for_captures(k, exclude, seen, caps);
                    self.walk_tir_expr_for_captures(v, exclude, seen, caps);
                }
            }
            TirExprKind::Spawn { fields, .. } => {
                for (_, v) in fields {
                    self.walk_tir_expr_for_captures(v, exclude, seen, caps);
                }
            }
            TirExprKind::Select { arms } => {
                for arm in arms {
                    self.walk_tir_expr_for_captures(&arm.expr, exclude, seen, caps);
                    self.walk_tir_block_for_captures(&arm.body, exclude, seen, caps);
                }
            }
            // Leaf / spec-only variants have no children to walk.
            TirExprKind::Literal(_) | TirExprKind::Quantifier(_) => {}
        }
    }

    fn walk_tir_block_for_captures(
        &self,
        block: &TirBlock,
        exclude: &HashSet<String>,
        seen: &mut HashSet<String>,
        caps: &mut Vec<(String, TypeExpr)>,
    ) {
        for stmt in &block.stmts {
            match stmt {
                TirStmt::Expr { expr, .. } => {
                    self.walk_tir_expr_for_captures(expr, exclude, seen, caps);
                }
                TirStmt::Let { init, .. } => {
                    self.walk_tir_expr_for_captures(init, exclude, seen, caps);
                }
                TirStmt::Assign { value, .. } => {
                    self.walk_tir_expr_for_captures(value, exclude, seen, caps);
                }
                TirStmt::Return { value: Some(e), .. } => {
                    self.walk_tir_expr_for_captures(e, exclude, seen, caps);
                }
                TirStmt::Return { value: None, .. } => {}
                TirStmt::While { cond, body, .. } => {
                    self.walk_tir_expr_for_captures(cond, exclude, seen, caps);
                    self.walk_tir_block_for_captures(body, exclude, seen, caps);
                }
                TirStmt::For { iter, body, .. } => {
                    self.walk_tir_expr_for_captures(iter, exclude, seen, caps);
                    self.walk_tir_block_for_captures(body, exclude, seen, caps);
                }
                TirStmt::If {
                    cond, then, else_, ..
                } => {
                    self.walk_tir_expr_for_captures(cond, exclude, seen, caps);
                    self.walk_tir_block_for_captures(then, exclude, seen, caps);
                    match else_ {
                        Some(TirElseBranch::Block(b)) => {
                            self.walk_tir_block_for_captures(b, exclude, seen, caps);
                        }
                        Some(TirElseBranch::If(s)) => {
                            // Recurse into else-if as a single statement.
                            let tmp_block = TirBlock {
                                stmts: vec![(**s).clone()],
                                span: Span::default(),
                            };
                            self.walk_tir_block_for_captures(&tmp_block, exclude, seen, caps);
                        }
                        None => {}
                    }
                }
                TirStmt::Match {
                    scrutinee, arms, ..
                } => {
                    self.walk_tir_expr_for_captures(scrutinee, exclude, seen, caps);
                    for arm in arms {
                        match &arm.body {
                            TirMatchBody::Expr(e) => {
                                self.walk_tir_expr_for_captures(e, exclude, seen, caps);
                            }
                            TirMatchBody::Block(b) => {
                                self.walk_tir_block_for_captures(b, exclude, seen, caps);
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Lambda emission ───────────────────────────────────────────────────

    /// Top-level entry: emit a TIR lambda. Body's `.ty` is the return type.
    pub(super) fn emit_lambda_tir(
        &mut self,
        params: &[TirParam],
        body: &TirExpr,
    ) -> Result<Option<String>, String> {
        self.emit_lambda_inner_tir(params, body, &[])
    }

    /// Emit a lambda for use by HOF runtime functions (filter/map/fold/any/all).
    ///
    /// `ptr_param_indices` lists parameter indices that the runtime passes as
    /// raw pointers to array elements. The lambda receives `ptr` for those
    /// params and emits a `load` to recover the real type.
    /// TIR variant of [`Self::emit_as_hof_closure`].
    ///
    /// Wraps a closure-like argument (Lambda literal or module-level fn
    /// reference) into the runtime `%__closure_type` representation, with
    /// `ptr_param_indices` marking which params the runtime passes as raw
    /// pointers to array elements.
    pub(super) fn emit_as_hof_closure_tir(
        &mut self,
        expr: &TirExpr,
        ptr_param_indices: &[usize],
    ) -> Result<Option<String>, String> {
        match &expr.kind {
            TirExprKind::Lambda { params, body } => {
                self.emit_hof_lambda_tir(params, body, ptr_param_indices)
            }
            TirExprKind::Var(name) => {
                if !self.fn_ctx.locals.contains_key(name.as_str())
                    && self.module.fn_ret_types.contains_key(name.as_str())
                {
                    self.make_named_fn_closure_hof(name, ptr_param_indices)
                } else {
                    self.emit_expr_tir(expr)
                }
            }
            _ => self.emit_expr_tir(expr),
        }
    }

    /// Return `true` if `expr` is a closure-like argument (Lambda or a
    /// module-level function reference). Used to guard HOF method arms so
    /// they don't accidentally match String kernel methods like `find`.
    pub(super) fn is_closure_arg_tir(&self, expr: &TirExpr) -> bool {
        match &expr.kind {
            TirExprKind::Lambda { .. } => true,
            TirExprKind::Var(name) => {
                !self.fn_ctx.locals.contains_key(name.as_str())
                    && self.module.fn_ret_types.contains_key(name.as_str())
            }
            _ => false,
        }
    }

    pub(super) fn emit_hof_lambda_tir(
        &mut self,
        params: &[TirParam],
        body: &TirExpr,
        ptr_param_indices: &[usize],
    ) -> Result<Option<String>, String> {
        self.emit_lambda_inner_tir(params, body, ptr_param_indices)
    }

    fn emit_lambda_inner_tir(
        &mut self,
        params: &[TirParam],
        body: &TirExpr,
        ptr_param_indices: &[usize],
    ) -> Result<Option<String>, String> {
        let lambda_name = format!("__lambda_{}", self.module.lambda_counter);
        self.module.lambda_counter += 1;

        // Return type from body's resolved .ty — no need to infer from LLVM type
        // as the AST path does (the AST has Option<TypeExpr> annotation; TIR
        // lambdas don't carry a separate ret_type).
        let ret_ty = ty_to_type_expr(&body.ty).unwrap_or(TypeExpr::Base {
            name: "Unit".into(),
            args: vec![],
            span: Span::default(),
        });

        // Convert param types once for the AST-shaped helpers below.
        let param_tes: Vec<TypeExpr> = params
            .iter()
            .map(|p| {
                ty_to_type_expr(&p.ty).unwrap_or(TypeExpr::Base {
                    name: "Unit".into(),
                    args: vec![],
                    span: Span::default(),
                })
            })
            .collect();

        // Capture analysis — must happen before we clear locals.
        let param_names: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();
        let captures = self.collect_lambda_captures_tir(body, &param_names);

        self.ensure_closure_type();

        // ── Build env struct + alloca in OUTER function ────────────────────
        let env_ty_name = format!("__env_{lambda_name}");
        let env_ptr: String = if captures.is_empty() {
            "null".into()
        } else {
            let field_types: Vec<String> = captures
                .iter()
                .map(|(_, ty)| self.llvm_ty_ctx(ty))
                .collect();
            self.module.type_defs.push(format!(
                "%{env_ty_name} = type {{ {} }}",
                field_types.join(", ")
            ));

            let env_alloca = self.next_reg();
            self.push_instr(&format!("{env_alloca} = alloca %{env_ty_name}"));
            self.fn_ctx
                .reg_types
                .insert(env_alloca.clone(), "ptr".into());

            for (i, (cap_name, cap_ty)) in captures.iter().enumerate() {
                let store_val = if let Some(ref_loc) = self.fn_ctx.ref_locals.get(cap_name).cloned()
                {
                    let ty_str = self.llvm_ty_ctx(&ref_loc.elem_ty);
                    let loaded = self.next_reg();
                    self.push_instr(&format!("{loaded} = load {ty_str}, ptr {}", ref_loc.ptr));
                    self.fn_ctx.reg_types.insert(loaded.clone(), ty_str);
                    loaded
                } else if let Some(cap_val) = self.fn_ctx.locals.get(cap_name).cloned() {
                    cap_val
                } else {
                    continue;
                };
                let field_llvm_ty = self.llvm_ty_ctx(cap_ty);
                let field_ptr = self.next_reg();
                self.push_instr(&format!(
                    "{field_ptr} = getelementptr %{env_ty_name}, ptr {env_alloca}, i32 0, i32 {i}"
                ));
                self.push_instr(&format!(
                    "store {field_llvm_ty} {store_val}, ptr {field_ptr}"
                ));
            }
            env_alloca
        };

        // ── Emit lambda as a separate top-level fn with fresh FnCtx ────────
        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        let is_void = Self::is_void(&ret_ty);

        let mut param_parts = vec!["ptr %__env".to_string()];
        for (i, (p, p_te)) in params.iter().zip(param_tes.iter()).enumerate() {
            let ty_str = self.llvm_ty_ctx(p_te);
            if ty_str != "void" {
                if ptr_param_indices.contains(&i) {
                    param_parts.push(format!("ptr %__raw_{}", p.name));
                } else {
                    param_parts.push(format!("{ty_str} %{}", p.name));
                }
            }
        }
        let params_str = param_parts.join(", ");

        let define_ret = if is_void {
            "void".into()
        } else {
            llvm_ret.clone()
        };

        self.with_fresh_fn_ctx(ret_ty.clone(), |this| -> Result<(), String> {
            this.fn_ctx
                .fn_buf
                .push(format!("define {define_ret} @{lambda_name}({params_str})"));
            this.fn_ctx.fn_buf.push("{".into());
            this.fn_ctx.fn_buf.push("entry:".into());

            // Bind user parameters as locals.
            for (i, (p, p_te)) in params.iter().zip(param_tes.iter()).enumerate() {
                let ty_str = this.llvm_ty_ctx(p_te);
                if ty_str != "void" {
                    if ptr_param_indices.contains(&i) {
                        let loaded = this.next_reg();
                        this.push_instr(&format!(
                            "{loaded} = load {ty_str}, ptr %__raw_{}",
                            p.name
                        ));
                        this.fn_ctx.locals.insert(p.name.clone(), loaded.clone());
                        this.fn_ctx.reg_types.insert(loaded, ty_str);
                    } else {
                        let ssa = format!("%{}", p.name);
                        this.fn_ctx.locals.insert(p.name.clone(), ssa.clone());
                        this.fn_ctx.reg_types.insert(ssa, ty_str);
                    }
                    this.fn_ctx
                        .local_mvl_types
                        .insert(p.name.clone(), p_te.clone());
                }
            }

            // Load captures from env ptr.
            if !captures.is_empty() {
                for (i, (cap_name, cap_ty)) in captures.iter().enumerate() {
                    let field_llvm_ty = this.llvm_ty_ctx(cap_ty);
                    let field_ptr = this.next_reg();
                    this.push_instr(&format!(
                        "{field_ptr} = getelementptr %{env_ty_name}, ptr %__env, i32 0, i32 {i}"
                    ));
                    let val = this.next_reg();
                    this.push_instr(&format!("{val} = load {field_llvm_ty}, ptr {field_ptr}"));
                    this.fn_ctx.reg_types.insert(val.clone(), field_llvm_ty);
                    this.fn_ctx.locals.insert(cap_name.clone(), val.clone());
                    this.fn_ctx
                        .local_mvl_types
                        .insert(cap_name.clone(), cap_ty.clone());
                }
            }

            let body_val = this.emit_expr_tir(body)?;

            if !this.fn_ctx.terminated {
                if is_void {
                    this.push_instr("ret void");
                } else if let Some(v) = body_val {
                    this.push_instr(&format!("ret {llvm_ret} {v}"));
                } else {
                    this.push_instr(&format!("ret {llvm_ret} undef"));
                }
            }

            this.fn_ctx.fn_buf.push("}".into());
            let lambda_body = this.fn_ctx.fn_buf.join("\n");
            this.module.fn_bodies.push(lambda_body);
            Ok(())
        })?;

        // ── Build closure struct in outer function ─────────────────────────
        let closure_alloca = self.next_reg();
        self.push_instr(&format!("{closure_alloca} = alloca %__closure_type"));
        self.fn_ctx
            .reg_types
            .insert(closure_alloca.clone(), "ptr".into());

        let fn_field = self.next_reg();
        self.push_instr(&format!(
            "{fn_field} = getelementptr %__closure_type, ptr {closure_alloca}, i32 0, i32 0"
        ));
        self.push_instr(&format!("store ptr @{lambda_name}, ptr {fn_field}"));

        let env_field = self.next_reg();
        self.push_instr(&format!(
            "{env_field} = getelementptr %__closure_type, ptr {closure_alloca}, i32 0, i32 1"
        ));
        if captures.is_empty() {
            self.push_instr(&format!("store ptr null, ptr {env_field}"));
        } else {
            self.push_instr(&format!("store ptr {env_ptr}, ptr {env_field}"));
        }

        Ok(Some(closure_alloca))
    }
}
