// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Generic monomorphization, Result handling, and format builtin emission for the `llvm_text` backend.

use std::collections::HashMap;

use crate::mvl::parser::ast::{Expr, FnDecl, Literal, MatchArm, MatchBody, Pattern, TypeExpr};
use crate::mvl::passes::mono;

use super::TextEmitter;

impl TextEmitter {
    // ── Generic monomorphization (#1156) ──────────────────────────────────

    /// Infer the MVL type of an expression (best-effort, for monomorphization).
    pub(super) fn mvl_type_of_expr(&self, expr: &Expr) -> TypeExpr {
        let default_int = || TypeExpr::Base {
            name: "Int".into(),
            args: vec![],
            span: Default::default(),
        };
        match expr {
            Expr::Literal(lit, _) => match lit {
                Literal::Integer(_) => default_int(),
                Literal::Float(_) => TypeExpr::Base {
                    name: "Float".into(),
                    args: vec![],
                    span: Default::default(),
                },
                Literal::Bool(_) => TypeExpr::Base {
                    name: "Bool".into(),
                    args: vec![],
                    span: Default::default(),
                },
                Literal::Str(_) => TypeExpr::Base {
                    name: "String".into(),
                    args: vec![],
                    span: Default::default(),
                },
                _ => default_int(),
            },
            Expr::Ident(name, _) => self
                .fn_ctx
                .local_mvl_types
                .get(name.as_str())
                .cloned()
                .unwrap_or_else(default_int),
            Expr::FnCall { name, .. } => self
                .module
                .fn_ret_types
                .get(name.as_str())
                .cloned()
                .unwrap_or_else(default_int),
            Expr::Construct { name, .. } => TypeExpr::Base {
                name: name.clone(),
                args: vec![],
                span: Default::default(),
            },
            Expr::FieldAccess {
                expr: receiver,
                field,
                ..
            } => {
                let recv_ty = self.mvl_type_of_expr(receiver);
                if let TypeExpr::Base { name: tn, .. } = &recv_ty {
                    if let Some(fields) = self.module.struct_fields.get(tn) {
                        if let Some((_, fty)) = fields.iter().find(|(f, _)| f == field) {
                            return fty.clone();
                        }
                    }
                }
                default_int()
            }
            Expr::Consume { expr: inner, .. } | Expr::Relabel { expr: inner, .. } => {
                self.mvl_type_of_expr(inner)
            }
            _ => default_int(),
        }
    }

    /// Sanitize a string segment for use in LLVM IR identifiers.
    pub(super) fn mangle_segment(s: &str) -> String {
        s.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// Mangle a generic function name with concrete types: `identity` + [Int] → `identity__Int`.
    pub(super) fn mangle_generic(name: &str, concrete: &[TypeExpr]) -> String {
        let suffix: Vec<String> = concrete
            .iter()
            .map(|ty| match ty {
                TypeExpr::Base { name, .. } => Self::mangle_segment(name),
                TypeExpr::Option { inner, .. } => {
                    format!(
                        "Option_{}",
                        Self::mangle_segment(&Self::mangle_type_name(inner))
                    )
                }
                TypeExpr::Result { ok, err, .. } => {
                    format!(
                        "Result_{}_{}",
                        Self::mangle_segment(&Self::mangle_type_name(ok)),
                        Self::mangle_segment(&Self::mangle_type_name(err))
                    )
                }
                _ => "T".into(),
            })
            .collect();
        format!("{}__{}", Self::mangle_segment(name), suffix.join("_"))
    }

    /// Extract a human-readable type name for mangling purposes.
    pub(super) fn mangle_type_name(ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Base { name, .. } => name.clone(),
            TypeExpr::Option { inner, .. } => format!("Option_{}", Self::mangle_type_name(inner)),
            TypeExpr::Result { ok, err, .. } => {
                format!(
                    "Result_{}_{}",
                    Self::mangle_type_name(ok),
                    Self::mangle_type_name(err)
                )
            }
            _ => "T".into(),
        }
    }

    /// Emit a call to a generic function, enqueuing the monomorphized version.
    pub(super) fn emit_monomorphized_call(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        let gfd = self.mono.generic_fns.get(name).cloned().ok_or_else(|| {
            format!("ICE: generic fn '{name}' missing from monomorphization table")
        })?;

        // Infer concrete types for each type parameter from the argument types.
        let mut tp_map: HashMap<String, TypeExpr> = HashMap::new();
        for (param, arg) in gfd.params.iter().zip(args.iter()) {
            Self::collect_type_bindings(&param.ty, &self.mvl_type_of_expr(arg), &gfd, &mut tp_map);
        }
        let concrete_types: Vec<TypeExpr> = gfd
            .type_params
            .iter()
            .map(|tp| {
                tp_map
                    .get(tp.name())
                    .cloned()
                    .unwrap_or_else(|| TypeExpr::Base {
                        name: "Int".into(),
                        args: vec![],
                        span: Default::default(),
                    })
            })
            .collect();

        let mangled = Self::mangle_generic(name, &concrete_types);

        // Enqueue monomorphized copy if not already emitted.
        if !self.mono.mono_emitted.contains(&mangled) {
            self.mono.mono_emitted.insert(mangled.clone());
            self.mono
                .mono_queue
                .push((mangled.clone(), name.to_string(), concrete_types.clone()));

            // Register the return type for the mangled function.
            // Resolve any type params in the return type.
            let resolved_ret = mono::substitute_type(&gfd.return_type, &tp_map);
            self.module
                .fn_ret_types
                .insert(mangled.clone(), resolved_ret);
        }

        // Emit the call.
        let mut arg_vals: Vec<(String, String)> = Vec::new();
        for arg in args {
            let ty = self.type_of_expr(arg);
            if let Some(v) = self.emit_expr(arg)? {
                arg_vals.push((ty, v));
            }
        }
        let args_str = arg_vals
            .iter()
            .map(|(ty, v)| format!("{ty} {v}"))
            .collect::<Vec<_>>()
            .join(", ");

        let ret_ty = self
            .module
            .fn_ret_types
            .get(&mangled)
            .cloned()
            .unwrap_or_else(|| TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: Default::default(),
            });
        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        let is_void = Self::is_void(&ret_ty);

        if is_void {
            self.push_instr(&format!("call void @{mangled}({args_str})"));
            Ok(None)
        } else {
            let result = self.next_reg();
            self.push_instr(&format!(
                "{result} = call {llvm_ret} @{mangled}({args_str})"
            ));
            self.fn_ctx.reg_types.insert(result.clone(), llvm_ret);
            Ok(Some(result))
        }
    }

    /// Match a generic parameter type against a concrete argument type to bind type variables.
    pub(super) fn collect_type_bindings(
        param_ty: &TypeExpr,
        arg_ty: &TypeExpr,
        gfd: &FnDecl,
        map: &mut HashMap<String, TypeExpr>,
    ) {
        if let TypeExpr::Base { name, .. } = param_ty {
            if gfd.type_params.iter().any(|tp| tp.name() == name) {
                map.insert(name.clone(), arg_ty.clone());
            }
        }
    }

    /// Emit `s.parse_int()` or `s.parse_float()` — calls the C-ABI parser and
    /// wraps the result in a `{ i8, ptr }` Result.
    ///
    /// `ok_llvm_ty` is the LLVM type of the success value (`"i64"` or `"double"`).
    pub(super) fn emit_str_parse(
        &mut self,
        val: &str,
        ok_llvm_ty: &str,
        c_sym: &str,
    ) -> Result<Option<String>, String> {
        let ok_slot = self.next_reg();
        self.push_instr(&format!("{ok_slot} = alloca {ok_llvm_ty}"));
        let err_slot = self.next_reg();
        self.push_instr(&format!("{err_slot} = alloca ptr"));
        self.ensure_extern(&format!("declare i8 @{c_sym}(ptr, ptr, ptr)"));
        let disc = self.next_reg();
        self.push_instr(&format!(
            "{disc} = call i8 @{c_sym}(ptr {val}, ptr {ok_slot}, ptr {err_slot})"
        ));
        self.fn_ctx.reg_types.insert(disc.clone(), "i8".into());
        // Select the correct payload pointer based on discriminant.
        let disc_is_ok = self.next_reg();
        self.push_instr(&format!("{disc_is_ok} = icmp eq i8 {disc}, 0"));
        self.fn_ctx
            .reg_types
            .insert(disc_is_ok.clone(), "i1".into());
        let payload = self.next_reg();
        self.push_instr(&format!(
            "{payload} = select i1 {disc_is_ok}, ptr {ok_slot}, ptr {err_slot}"
        ));
        self.fn_ctx.reg_types.insert(payload.clone(), "ptr".into());
        let r1 = self.wrap_result_pair(&disc, &payload);
        Ok(Some(r1))
    }

    /// Emit a `match` where at least one arm has `Pattern::Ok` / `Pattern::Err`.
    pub(super) fn emit_result_match(
        &mut self,
        scrutinee: &Expr,
        scrut_val: &str,
        arms: &[MatchArm],
    ) -> Result<Option<String>, String> {
        // Determine Ok/Err payload LLVM types from the scrutinee's MVL type.
        let (ok_load_ty, err_load_ty) = {
            let mvl_ty = match scrutinee {
                Expr::Ident(name, _) => self.fn_ctx.local_mvl_types.get(name.as_str()).cloned(),
                Expr::FnCall { name, .. } => self.module.fn_ret_types.get(name.as_str()).cloned(),
                _ => None,
            };
            match mvl_ty {
                Some(TypeExpr::Result { ok, err, .. }) => (Self::llvm_ty(&ok), Self::llvm_ty(&err)),
                _ => ("i64".into(), "ptr".into()),
            }
        };

        // Extract discriminant byte from the { i8, ptr } struct.
        let disc_reg = self.next_reg();
        self.push_instr(&format!(
            "{disc_reg} = extractvalue {{ i8, ptr }} {scrut_val}, 0"
        ));
        self.fn_ctx.reg_types.insert(disc_reg.clone(), "i8".into());

        let n = self.fn_ctx.bb;
        self.fn_ctx.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);
        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        // Build switch on i8 discriminant.
        let mut switch_str = format!("switch i8 {disc_reg}, label %{default_bb} [\n");
        let mut wildcard_arm: Option<usize> = None;
        for (idx, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                Pattern::Ok { .. } => {
                    switch_str.push_str(&format!("    i8 0, label %{}\n", arm_bbs[idx]));
                }
                Pattern::Err { .. } => {
                    switch_str.push_str(&format!("    i8 1, label %{}\n", arm_bbs[idx]));
                }
                Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                    wildcard_arm = Some(idx);
                }
                _ => {
                    wildcard_arm = Some(idx);
                }
            }
        }
        switch_str.push_str("  ]");
        self.push_instr(&switch_str);

        // Emit arm blocks.
        let mut phi_entries: Vec<(String, String, String)> = Vec::new();
        // Arms that branch to merge_bb but produced no value (need undef phi entries).
        let mut no_val_arms: Vec<String> = Vec::new(); // from_bb

        for (idx, arm) in arms.iter().enumerate() {
            let arm_bb = &arm_bbs[idx];
            self.fn_ctx.fn_buf.push(format!("{arm_bb}:"));
            self.fn_ctx.current_bb = arm_bb.clone();
            self.fn_ctx.terminated = false;

            let mut bound_var: Option<String> = None;

            match &arm.pattern {
                Pattern::Ok { inner, .. } if ok_load_ty != "void" => {
                    let pp = self.next_reg();
                    self.push_instr(&format!("{pp} = extractvalue {{ i8, ptr }} {scrut_val}, 1"));
                    let ok_val = self.next_reg();
                    self.push_instr(&format!("{ok_val} = load {ok_load_ty}, ptr {pp}"));
                    self.fn_ctx
                        .reg_types
                        .insert(ok_val.clone(), ok_load_ty.clone());
                    if let Pattern::Ident(var_name, _) = inner.as_ref() {
                        if var_name != "_" {
                            self.fn_ctx.locals.insert(var_name.clone(), ok_val.clone());
                            bound_var = Some(var_name.clone());
                        }
                    }
                }
                Pattern::Ok { .. } => {}
                Pattern::Err { inner, .. } => {
                    let pp = self.next_reg();
                    self.push_instr(&format!("{pp} = extractvalue {{ i8, ptr }} {scrut_val}, 1"));
                    let err_val = self.next_reg();
                    self.push_instr(&format!("{err_val} = load {err_load_ty}, ptr {pp}"));
                    self.fn_ctx
                        .reg_types
                        .insert(err_val.clone(), err_load_ty.clone());
                    if let Pattern::Ident(var_name, _) = inner.as_ref() {
                        if var_name != "_" {
                            self.fn_ctx.locals.insert(var_name.clone(), err_val.clone());
                            bound_var = Some(var_name.clone());
                        }
                    }
                }
                Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                    if let Pattern::Ident(name, _) = &arm.pattern {
                        self.fn_ctx
                            .locals
                            .insert(name.clone(), scrut_val.to_string());
                        bound_var = Some(name.clone());
                    }
                }
                _ => {}
            }

            let arm_val = match &arm.body {
                MatchBody::Expr(e) => self.emit_expr(e)?,
                MatchBody::Block(b) => self.emit_block(b)?,
            };
            let end_bb = self.fn_ctx.current_bb.clone();
            if !self.fn_ctx.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }

            if let Some(var_name) = bound_var {
                self.fn_ctx.locals.remove(&var_name);
            }
        }

        // Default block.
        self.fn_ctx.fn_buf.push(format!("{default_bb}:"));
        self.fn_ctx.current_bb = default_bb.clone();
        self.fn_ctx.terminated = false;
        if let Some(wild_idx) = wildcard_arm {
            let arm_bb = &arm_bbs[wild_idx];
            self.push_instr(&format!("br label %{arm_bb}"));
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.fn_ctx.terminated = true;
        }

        // Merge block + phi.
        self.fn_ctx.fn_buf.push(format!("{merge_bb}:"));
        self.fn_ctx.current_bb = merge_bb.clone();
        self.fn_ctx.terminated = false;
        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming >= 2 && !phi_entries.is_empty() {
            let phi_ty = phi_entries
                .iter()
                .find(|(_, ty, _)| ty != "i64")
                .map(|(_, ty, _)| ty.clone())
                .unwrap_or_else(|| phi_entries[0].1.clone());
            let mut parts: Vec<String> = phi_entries
                .iter()
                .map(|(v, _, from)| format!("[ {v}, %{from} ]"))
                .collect();
            for from in &no_val_arms {
                parts.push(format!("[ undef, %{from} ]"));
            }
            let result = self.next_reg();
            self.push_instr(&format!("{result} = phi {phi_ty} {}", parts.join(", ")));
            self.fn_ctx.reg_types.insert(result.clone(), phi_ty);
            Ok(Some(result))
        } else if phi_entries.len() == 1 && no_val_arms.is_empty() {
            Ok(Some(phi_entries.remove(0).0))
        } else {
            Ok(None)
        }
    }

    /// Emit the `?` propagation operator on a `Result[T,E]` value.
    ///
    /// On Err: early-return the `{ i8, ptr }` value from the current function.
    /// On Ok:  extract the payload and load the inner `T` value.
    pub(super) fn emit_propagate(&mut self, inner: &Expr) -> Result<Option<String>, String> {
        let result_val = match self.emit_expr(inner)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let disc = self.next_reg();
        self.push_instr(&format!(
            "{disc} = extractvalue {{ i8, ptr }} {result_val}, 0"
        ));
        self.fn_ctx.reg_types.insert(disc.clone(), "i8".into());

        let is_ok = self.next_reg();
        self.push_instr(&format!("{is_ok} = icmp eq i8 {disc}, 0"));
        self.fn_ctx.reg_types.insert(is_ok.clone(), "i1".into());

        let ok_bb = self.next_bb("prop_ok");
        let err_bb = self.next_bb("prop_err");
        self.push_instr(&format!("br i1 {is_ok}, label %{ok_bb}, label %{err_bb}"));

        // Err path: propagate the result upwards.
        self.start_bb(&err_bb);
        self.emit_heap_drops();
        let ret_ty = self.fn_ctx.current_ret_ty.clone();
        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        self.push_instr(&format!("ret {llvm_ret} {result_val}"));
        self.fn_ctx.terminated = true;

        // Ok path: extract and load the success payload.
        self.start_bb(&ok_bb);
        let ok_load_ty = self.result_ok_llvm_ty(inner);
        if ok_load_ty == "void" {
            // Result[Unit, E] — no payload to load; the ? expression yields nothing.
            return Ok(None);
        }
        let payload_ptr = self.next_reg();
        self.push_instr(&format!(
            "{payload_ptr} = extractvalue {{ i8, ptr }} {result_val}, 1"
        ));
        let ok_val = self.next_reg();
        self.push_instr(&format!("{ok_val} = load {ok_load_ty}, ptr {payload_ptr}"));
        self.fn_ctx.reg_types.insert(ok_val.clone(), ok_load_ty);
        Ok(Some(ok_val))
    }

    /// Infer the LLVM type of the `Ok` payload from a Result-returning expression.
    pub(super) fn result_ok_llvm_ty(&self, expr: &Expr) -> String {
        match expr {
            Expr::FnCall { name, .. } => {
                if let Some(TypeExpr::Result { ok, .. }) =
                    self.module.fn_ret_types.get(name.as_str())
                {
                    return Self::llvm_ty(ok);
                }
                "i64".into()
            }
            Expr::MethodCall { method, .. } if method == "parse_int" => "i64".into(),
            Expr::MethodCall { method, .. } if method == "parse_float" => "double".into(),
            _ => "i64".into(),
        }
    }

    pub(super) fn emit_format_builtin(&mut self, args: &[Expr]) -> Result<Option<String>, String> {
        if args.len() < 2 {
            return Ok(None);
        }
        let template = match self.emit_expr(&args[0])? {
            Some(v) => v,
            None => return Ok(None),
        };
        let list = match self.emit_expr(&args[1])? {
            Some(v) => v,
            None => return Ok(None),
        };
        self.ensure_extern("declare ptr @_mvl_format(ptr, ptr)");
        let reg = self.next_reg();
        self.push_instr(&format!(
            "{reg} = call ptr @_mvl_format(ptr {template}, ptr {list})"
        ));
        self.fn_ctx.reg_types.insert(reg.clone(), "ptr".into());
        Ok(Some(reg))
    }
}
