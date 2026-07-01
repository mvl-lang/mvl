// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Lambda and closure lowering for the `llvm_text` backend.

use crate::mvl::parser::ast::{Block, ElseBranch, Expr, MatchBody, Stmt, TypeExpr};

use super::TextEmitter;

impl TextEmitter {
    // ── Closure / lambda lowering (#1148) ────────────────────────────────
    //
    // `ensure_closure_type` and `make_named_fn_closure_hof` live in
    // `emit_helpers.rs` (#1612 PR 2 prep — both are AST-shape-agnostic).

    /// Collect free variables referenced in `body` that exist in `self.fn_ctx.locals`
    /// and are not in `exclude` (the lambda's own parameters).
    /// Returns `(name, TypeExpr)` pairs in stable order.
    pub(super) fn collect_lambda_captures(
        &self,
        body: &Expr,
        exclude: &std::collections::HashSet<String>,
    ) -> Vec<(String, TypeExpr)> {
        let mut seen = std::collections::HashSet::new();
        let mut caps = Vec::new();
        self.walk_expr_for_captures(body, exclude, &mut seen, &mut caps);
        caps
    }

    pub(super) fn walk_expr_for_captures(
        &self,
        expr: &Expr,
        exclude: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        caps: &mut Vec<(String, TypeExpr)>,
    ) {
        match expr {
            Expr::Ident(name, _)
                if !exclude.contains(name)
                    && !seen.contains(name)
                    && (self.fn_ctx.locals.contains_key(name)
                        || self.fn_ctx.ref_locals.contains_key(name)) =>
            {
                let ty_opt = self.fn_ctx.local_mvl_types.get(name).cloned().or_else(|| {
                    self.fn_ctx
                        .ref_locals
                        .get(name)
                        .map(|rl| rl.elem_ty.clone())
                });
                if let Some(ty) = ty_opt {
                    seen.insert(name.clone());
                    caps.push((name.clone(), ty));
                }
            }
            Expr::Lambda { params, body, .. } => {
                let mut inner_excl = exclude.clone();
                for p in params {
                    inner_excl.insert(p.name.clone());
                }
                self.walk_expr_for_captures(body, &inner_excl, seen, caps);
            }
            Expr::Binary { left, right, .. } => {
                self.walk_expr_for_captures(left, exclude, seen, caps);
                self.walk_expr_for_captures(right, exclude, seen, caps);
            }
            Expr::Unary { expr, .. } => {
                self.walk_expr_for_captures(expr, exclude, seen, caps);
            }
            Expr::FnCall { name, args, .. } => {
                // If the callee is a local closure binding, capture it too.
                if !exclude.contains(name)
                    && !seen.contains(name)
                    && (self.fn_ctx.locals.contains_key(name)
                        || self.fn_ctx.ref_locals.contains_key(name))
                {
                    if let Some(ty) = self.fn_ctx.local_mvl_types.get(name).cloned().or_else(|| {
                        self.fn_ctx
                            .ref_locals
                            .get(name)
                            .map(|rl| rl.elem_ty.clone())
                    }) {
                        seen.insert(name.clone());
                        caps.push((name.clone(), ty));
                    }
                }
                for a in args {
                    self.walk_expr_for_captures(a, exclude, seen, caps);
                }
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.walk_expr_for_captures(receiver, exclude, seen, caps);
                for a in args {
                    self.walk_expr_for_captures(a, exclude, seen, caps);
                }
            }
            Expr::FieldAccess { expr, .. } => {
                self.walk_expr_for_captures(expr, exclude, seen, caps);
            }
            Expr::If {
                cond, then, else_, ..
            } => {
                self.walk_expr_for_captures(cond, exclude, seen, caps);
                self.walk_block_for_captures(then, exclude, seen, caps);
                if let Some(e) = else_ {
                    self.walk_expr_for_captures(e, exclude, seen, caps);
                }
            }
            Expr::Block(b) => self.walk_block_for_captures(b, exclude, seen, caps),
            Expr::Construct { fields, .. } => {
                for (_, v) in fields {
                    self.walk_expr_for_captures(v, exclude, seen, caps);
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.walk_expr_for_captures(scrutinee, exclude, seen, caps);
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(e) => self.walk_expr_for_captures(e, exclude, seen, caps),
                        MatchBody::Block(b) => self.walk_block_for_captures(b, exclude, seen, caps),
                    }
                }
            }
            Expr::Consume { expr, .. }
            | Expr::Relabel { expr, .. }
            | Expr::Propagate { expr, .. }
            | Expr::Borrow { expr, .. } => {
                self.walk_expr_for_captures(expr, exclude, seen, caps);
            }
            Expr::List { elems, .. } | Expr::Set { elems, .. } => {
                for e in elems {
                    self.walk_expr_for_captures(e, exclude, seen, caps);
                }
            }
            Expr::Map { pairs, .. } => {
                for (k, v) in pairs {
                    self.walk_expr_for_captures(k, exclude, seen, caps);
                    self.walk_expr_for_captures(v, exclude, seen, caps);
                }
            }
            Expr::Spawn { fields, .. } => {
                for (_, v) in fields {
                    self.walk_expr_for_captures(v, exclude, seen, caps);
                }
            }
            Expr::Select { arms, .. } => {
                for arm in arms {
                    self.walk_expr_for_captures(&arm.expr, exclude, seen, caps);
                    self.walk_block_for_captures(&arm.body, exclude, seen, caps);
                }
            }
            _ => {}
        }
    }

    pub(super) fn walk_block_for_captures(
        &self,
        block: &Block,
        exclude: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        caps: &mut Vec<(String, TypeExpr)>,
    ) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Expr { expr, .. } => self.walk_expr_for_captures(expr, exclude, seen, caps),
                Stmt::Let { init, .. } => {
                    self.walk_expr_for_captures(init, exclude, seen, caps);
                }
                Stmt::Assign { value, .. } => {
                    self.walk_expr_for_captures(value, exclude, seen, caps);
                }
                Stmt::Return { value: Some(e), .. } => {
                    self.walk_expr_for_captures(e, exclude, seen, caps);
                }
                Stmt::While { cond, body, .. } => {
                    self.walk_expr_for_captures(cond, exclude, seen, caps);
                    self.walk_block_for_captures(body, exclude, seen, caps);
                }
                Stmt::For { iter, body, .. } => {
                    self.walk_expr_for_captures(iter, exclude, seen, caps);
                    self.walk_block_for_captures(body, exclude, seen, caps);
                }
                Stmt::If {
                    cond, then, else_, ..
                } => {
                    self.walk_expr_for_captures(cond, exclude, seen, caps);
                    self.walk_block_for_captures(then, exclude, seen, caps);
                    match else_ {
                        Some(ElseBranch::Block(b)) => {
                            self.walk_block_for_captures(b, exclude, seen, caps);
                        }
                        Some(ElseBranch::If(s)) => {
                            // Recurse into else-if as a single statement.
                            let tmp_block = Block {
                                stmts: vec![*s.clone()],
                                span: s.span(),
                            };
                            self.walk_block_for_captures(&tmp_block, exclude, seen, caps);
                        }
                        None => {}
                    }
                }
                Stmt::Match {
                    scrutinee, arms, ..
                } => {
                    self.walk_expr_for_captures(scrutinee, exclude, seen, caps);
                    for arm in arms {
                        match &arm.body {
                            MatchBody::Expr(e) => {
                                self.walk_expr_for_captures(e, exclude, seen, caps);
                            }
                            MatchBody::Block(b) => {
                                self.walk_block_for_captures(b, exclude, seen, caps);
                            }
                        }
                    }
                }
                Stmt::Return { value: None, .. } => {}
            }
        }
    }

    /// Emit a lambda expression as a top-level LLVM function and return a
    /// pointer to a stack-allocated `%__closure_type { fn_ptr, env_ptr }`.
    pub(super) fn emit_lambda(
        &mut self,
        params: &[crate::mvl::parser::ast::Param],
        ret_type: Option<&TypeExpr>,
        body: &Expr,
    ) -> Result<Option<String>, String> {
        self.emit_lambda_inner(params, ret_type, body, &[])
    }

    /// Emit a lambda for use by HOF runtime functions (filter/map/fold/any/all).
    ///
    /// `ptr_param_indices` lists parameter indices (0-based, within user params)
    /// that the runtime passes as raw pointers to array elements.  The lambda
    /// receives `ptr` for those params and emits a `load` to recover the real type.
    pub(super) fn emit_hof_lambda(
        &mut self,
        params: &[crate::mvl::parser::ast::Param],
        ret_type: Option<&TypeExpr>,
        body: &Expr,
        ptr_param_indices: &[usize],
    ) -> Result<Option<String>, String> {
        self.emit_lambda_inner(params, ret_type, body, ptr_param_indices)
    }

    fn emit_lambda_inner(
        &mut self,
        params: &[crate::mvl::parser::ast::Param],
        ret_type: Option<&TypeExpr>,
        body: &Expr,
        ptr_param_indices: &[usize],
    ) -> Result<Option<String>, String> {
        let lambda_name = format!("__lambda_{}", self.module.lambda_counter);
        self.module.lambda_counter += 1;

        let ret_ty = match ret_type {
            Some(t) => t.clone(),
            None => {
                // Infer from the body's LLVM type when no annotation is present.
                let inferred = self.type_of_expr(body);
                let base_name = match inferred.as_str() {
                    "i1" => "Bool",
                    "double" => "Float",
                    "ptr" => "String",
                    "void" => "Unit",
                    _ => "Int",
                };
                TypeExpr::Base {
                    name: base_name.into(),
                    args: vec![],
                    span: Default::default(),
                }
            }
        };

        // Capture analysis — must happen before we clear locals.
        let param_names: std::collections::HashSet<String> =
            params.iter().map(|p| p.name.clone()).collect();
        let captures = self.collect_lambda_captures(body, &param_names);

        self.ensure_closure_type();

        // ── Build env struct and alloca in the OUTER function ────────────
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
                // Ref locals: load current value from the alloca before capturing.
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
                    continue; // not in scope (shouldn't happen after collect_lambda_captures)
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

        // ── Emit lambda as a separate top-level function with fresh FnCtx ─
        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        let is_void = Self::is_void(&ret_ty);

        let mut param_parts = vec!["ptr %__env".to_string()];
        for (i, p) in params.iter().enumerate() {
            let ty_str = self.llvm_ty_ctx(&p.ty);
            if ty_str != "void" {
                if ptr_param_indices.contains(&i) {
                    // Runtime passes a raw pointer to the array element.
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
            for (i, p) in params.iter().enumerate() {
                let ty_str = this.llvm_ty_ctx(&p.ty);
                if ty_str != "void" {
                    if ptr_param_indices.contains(&i) {
                        // Load the real type from the pointer the runtime passed us.
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
                        .insert(p.name.clone(), p.ty.clone());
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

            let body_val = this.emit_expr(body)?;

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

        // ── Build closure struct in outer function ────────────────────────
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

    // `make_named_fn_closure_hof` lives in `emit_helpers.rs` (#1612 PR 2 prep).

    /// Emit `expr` as a closure for HOF runtime functions.
    ///
    /// `ptr_param_indices` specifies which lambda params are passed by pointer
    /// (the runtime passes a raw pointer to the array element).  The generated
    /// lambda loads the real type from that pointer.  Pass `&[]` for non-HOF use.
    pub(super) fn emit_as_hof_closure(
        &mut self,
        expr: &Expr,
        ptr_param_indices: &[usize],
    ) -> Result<Option<String>, String> {
        self.emit_as_closure_inner(expr, ptr_param_indices)
    }

    fn emit_as_closure_inner(
        &mut self,
        expr: &Expr,
        ptr_param_indices: &[usize],
    ) -> Result<Option<String>, String> {
        match expr {
            Expr::Lambda {
                params,
                ret_type,
                body,
                ..
            } => self.emit_hof_lambda(params, ret_type.as_deref(), body, ptr_param_indices),
            Expr::Ident(name, _) => {
                // Module-level function reference (not in locals).
                if !self.fn_ctx.locals.contains_key(name.as_str())
                    && self.module.fn_ret_types.contains_key(name.as_str())
                {
                    self.make_named_fn_closure_hof(name, ptr_param_indices)
                } else {
                    // Already a closure-typed local — just return its SSA value.
                    self.emit_expr(expr)
                }
            }
            _ => self.emit_expr(expr),
        }
    }

    /// Return `true` if `expr` is a closure-like argument (Lambda or a
    /// module-level function reference).  Used to guard HOF method arms so
    /// they don't accidentally match String kernel methods like `find`.
    pub(super) fn is_closure_arg(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Lambda { .. } => true,
            Expr::Ident(name, _) => {
                !self.fn_ctx.locals.contains_key(name.as_str())
                    && self.module.fn_ret_types.contains_key(name.as_str())
            }
            _ => false,
        }
    }

    // ── List literal ──────────────────────────────────────────────────────
}
