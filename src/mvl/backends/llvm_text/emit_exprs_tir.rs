// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Expression emission for the TIR-walking path (#1612, Phase 3b PR 1).
//!
//! Parallel to `emit_exprs.rs`. Built leaf-first:
//! 1. `Literal`, `Var` (this commit — minimal leaves)
//! 2. `Unary`, `Binary`, `FieldAccess`
//! 3. `If`, `Match`, `Block`, `FnCall`, `Lambda`, …
//! 4. Composite walkers (`MethodCall`, `Construct`, `Spawn`, …)
//!
//! TIR nodes embed `.ty: Ty` directly — no `module.expr_types.get(span)` lookup
//! is needed. The `Expr::As` variant has been erased by lowering; the inner
//! expression's `.ty` carries the cast destination type.

use crate::mvl::ir::{
    BinaryOp, Pattern, TirExpr, TirExprKind, TirMatchArm, TirMatchBody, Ty, UnaryOp,
};

use super::{TextEmitter, RESULT_LLVM_TY};

/// Recursively check if a TIR expression's static type is `Float`.
///
/// Mirrors `expr_is_float(&Expr)` on the AST side but uses the embedded
/// `.ty` on each `TirExpr` node — no walk needed past the root in the
/// common case.
fn tir_expr_is_float(expr: &TirExpr) -> bool {
    matches!(expr.ty, Ty::Float)
        || match &expr.kind {
            TirExprKind::Binary { left, .. } => tir_expr_is_float(left),
            _ => false,
        }
}

impl TextEmitter {
    /// Walk a [`TirExpr`] and emit LLVM IR for it, returning the SSA register
    /// holding the value (or `None` if the expression diverges / has no value).
    ///
    /// TIR-walking counterpart of `emit_expr(&Expr)`. Built incrementally —
    /// unimplemented variants return an error so the `cross_backend_tir` test
    /// target surfaces gaps.
    #[allow(dead_code)] // wired up via emit_program_tir once that's implemented
    pub(super) fn emit_expr_tir(&mut self, expr: &TirExpr) -> Result<Option<String>, String> {
        match &expr.kind {
            TirExprKind::Literal(lit) => self.emit_literal(lit),

            TirExprKind::Var(name) => {
                // `None` as a bare identifier → Option None constructor.
                if name == "None" {
                    return self.emit_none_constructor();
                }
                // Qualified enum variant: "Shape::Circle" → discriminant i64,
                // or "LinkedList::Nil" (payload enum, unit variant) → { i8, ptr }.
                if name.contains("::") {
                    if let Some(disc) = self.pattern_discriminant(name) {
                        if let Some((type_name, _)) = Self::split_qualified(name) {
                            if self.enum_has_payloads(type_name) {
                                return self.emit_enum_variant_constructor(name, disc, &[]);
                            }
                        }
                        return Ok(Some(format!("{disc}")));
                    }
                }
                if let Some(loc) = self.fn_ctx.ref_locals.get(name).cloned() {
                    let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = load {ty_str}, ptr {}", loc.ptr));
                    self.fn_ctx.reg_types.insert(reg.clone(), ty_str);
                    return Ok(Some(reg));
                }
                if let Some(val) = self.fn_ctx.locals.get(name).cloned() {
                    return Ok(Some(val));
                }
                // Bare reference to a top-level function (e.g. `my_handler`
                // passed as an argument) — emit as the function symbol so it
                // can be used as a function pointer.
                if self.module.fn_ret_types.contains_key(name) {
                    return Ok(Some(format!("@{name}")));
                }
                Ok(None)
            }

            TirExprKind::Binary { op, left, right } => self.emit_binary_tir(op, left, right),

            TirExprKind::Unary { op, expr: inner } => self.emit_unary_tir(op, inner),

            TirExprKind::Block(block) => self.emit_block_tir(block),

            TirExprKind::If { cond, then, else_ } => {
                self.emit_if_expr_tir(cond, then, else_.as_deref())
            }

            TirExprKind::FnCall { name, args, .. } => self.emit_fn_call_tir(name, args),

            TirExprKind::Construct { name, fields } => self.emit_construct_tir(name, fields),

            TirExprKind::FieldAccess { expr: inner, field } => {
                self.emit_field_access_tir(inner, field)
            }

            // Set lowers to a List in this backend (sets are arrays at the IR level;
            // dedup is the runtime's responsibility).
            TirExprKind::List { elems } | TirExprKind::Set { elems } => {
                self.emit_list_literal_tir(elems)
            }

            TirExprKind::Map { pairs } => self.emit_map_literal_tir(pairs),

            TirExprKind::Match { scrutinee, arms } => self.emit_match_expr_tir(scrutinee, arms),

            // Consume is a move marker — lowers to the inner value unchanged.
            TirExprKind::Consume(inner) => self.emit_expr_tir(inner),

            // Borrow is a capability marker — lowers to the inner value.
            TirExprKind::Borrow { expr: inner, .. } => self.emit_expr_tir(inner),

            // Unimplemented variants — build out leaf-first in subsequent commits (#1612).
            _ => Err(format!(
                "emit_expr_tir: variant not yet implemented: {:?}",
                std::mem::discriminant(&expr.kind)
            )),
        }
    }

    /// TIR variant of [`Self::emit_if_expr`].
    fn emit_if_expr_tir(
        &mut self,
        cond: &TirExpr,
        then: &crate::mvl::ir::TirBlock,
        else_: Option<&TirExpr>,
    ) -> Result<Option<String>, String> {
        match else_ {
            Some(e) => match &e.kind {
                TirExprKind::Block(b) => self.emit_if_phi_tir_from_blocks(cond, then, Some(b)),
                TirExprKind::If { .. } => {
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
                    let else_val = self.emit_expr_tir(e)?;
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
                }
                _ => self.emit_if_phi_tir_from_blocks(cond, then, None),
            },
            None => self.emit_if_phi_tir_from_blocks(cond, then, None),
        }
    }

    /// TIR variant of [`Self::emit_fn_call`] — minimum-viable port.
    ///
    /// Handles direct user-defined function calls (the most common case).
    /// Builtins (assert/println/format/Ok/Err/Some/None), enum variant
    /// constructors, Box::new, stdlib C-ABI dispatch, generics, fn-alias
    /// indirect calls, and local closure calls fall through to errors and
    /// will be ported in subsequent commits.
    fn emit_fn_call_tir(
        &mut self,
        name: &str,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        use crate::mvl::ir::TypeExpr;

        // Builtins ported so far.
        match name {
            "assert" => return self.emit_assert_builtin_tir(args),
            "println" | "print" | "eprintln" => {
                return self.emit_println_builtin_tir(name, args)
            }
            "Ok" | "Err" => return self.emit_result_constructor_tir(name, args),
            "Some" => return self.emit_option_constructor_tir(args),
            "None" => return self.emit_none_constructor(),
            _ => {}
        }

        // Enum variant constructors: "Shape::Circle" or "LinkedList::Cons(...)".
        if name.contains("::") {
            if let Some(disc) = self.pattern_discriminant(name) {
                let (type_name, _variant_name) = Self::split_qualified(name)
                    .ok_or_else(|| format!("malformed qualified name: {name}"))?;
                if self.enum_has_payloads(type_name) {
                    return self.emit_enum_variant_constructor_tir(name, disc, args);
                }
                return Ok(Some(format!("{disc}")));
            }
        }

        // Reject the special cases we haven't ported yet — keeps the TIR walker
        // safe to invoke against any program (returns Err instead of producing
        // divergent IR that would silently fail at lli runtime).
        match name {
            "format" | "Box::new" | "path" | "format_datetime" | "format_instant"
            | "find_all" | "replace" | "choice" | "List::filled" | "float_checked_to_int" => {
                return Err(format!(
                    "emit_fn_call_tir: builtin `{name}` not yet ported"
                ));
            }
            _ => {}
        }
        if name.contains("::") && self.pattern_discriminant(name).is_some() {
            return Err(format!(
                "emit_fn_call_tir: enum variant constructor `{name}` not yet ported"
            ));
        }
        if self.mono.generic_fns.contains_key(name) {
            return Err(format!(
                "emit_fn_call_tir: generic call `{name}` not yet ported"
            ));
        }

        // User-defined function call.
        let mut arg_vals: Vec<(String, String)> = Vec::new();
        for arg in args {
            let ty = self.ty_to_llvm_ctx(&arg.ty);
            if let Some(v) = self.emit_expr_tir(arg)? {
                arg_vals.push((ty, v));
            }
        }
        let ret_ty = self
            .module
            .fn_ret_types
            .get(name)
            .cloned()
            .unwrap_or_else(|| TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: Default::default(),
            });

        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        let is_void = Self::is_void(&ret_ty);

        // Builtin C-ABI dispatch (e.g. `mvl_str_split`) — same rewrite rules as
        // the AST path: opaque-handle struct args become `ptr`.
        let (effective_name, is_c_builtin, args_str): (String, bool, String) =
            if let Some(c_sym) = self.module.builtin_syms.get(name).cloned() {
                let c_abi_args: Vec<(String, &str)> = arg_vals
                    .iter()
                    .map(|(ty, v)| {
                        let actual_ty = self.fn_ctx.reg_types.get(v).cloned();
                        let abi_ty =
                            if ty.starts_with('%') && actual_ty.as_deref() == Some("ptr") {
                                "ptr".to_string()
                            } else {
                                ty.clone()
                            };
                        (abi_ty, v.as_str())
                    })
                    .collect();
                let param_tys = c_abi_args
                    .iter()
                    .map(|(ty, _)| ty.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.ensure_extern(&format!("declare {llvm_ret} @{c_sym}({param_tys})"));
                let abi_args_str = c_abi_args
                    .iter()
                    .map(|(ty, v)| format!("{ty} {v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                (c_sym, true, abi_args_str)
            } else {
                let args_str = arg_vals
                    .iter()
                    .map(|(ty, v)| format!("{ty} {v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                (name.to_string(), false, args_str)
            };

        if is_void {
            self.push_instr(&format!("call void @{effective_name}({args_str})"));
            Ok(None)
        } else {
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = call {llvm_ret} @{effective_name}({args_str})"
            ));
            self.fn_ctx.reg_types.insert(reg.clone(), llvm_ret.clone());

            if is_c_builtin && llvm_ret == super::RESULT_LLVM_TY {
                let disc = self.next_reg();
                self.push_instr(&format!(
                    "{disc} = extractvalue {} {reg}, 0",
                    super::RESULT_LLVM_TY
                ));
                self.fn_ctx.reg_types.insert(disc.clone(), "i8".into());
                let raw_payload = self.next_reg();
                self.push_instr(&format!(
                    "{raw_payload} = extractvalue {} {reg}, 1",
                    super::RESULT_LLVM_TY
                ));
                self.fn_ctx
                    .reg_types
                    .insert(raw_payload.clone(), "ptr".into());
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca ptr"));
                self.push_instr(&format!("store ptr {raw_payload}, ptr {slot}"));
                let r1 = self.wrap_result_pair(&disc, &slot);
                return Ok(Some(r1));
            }

            Ok(Some(reg))
        }
    }

    /// TIR variant of [`Self::emit_binary`].
    ///
    /// Uses each operand's embedded `.ty` to drive float/string dispatch
    /// instead of the AST `expr_is_float` walker.
    fn emit_binary_tir(
        &mut self,
        op: &BinaryOp,
        left: &TirExpr,
        right: &TirExpr,
    ) -> Result<Option<String>, String> {
        if op.is_short_circuit() {
            return match op {
                BinaryOp::And => self.emit_short_circuit_and_tir(left, right),
                BinaryOp::Or => self.emit_short_circuit_or_tir(left, right),
                _ => unreachable!("is_short_circuit but not And or Or"),
            };
        }

        let lv = match self.emit_expr_tir(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rv = match self.emit_expr_tir(right)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let lhs_ty = self.ty_to_llvm_ctx(&left.ty);
        let is_float = lhs_ty == "double" || tir_expr_is_float(left);

        // String equality/inequality: delegate to runtime via mvl_string_eq.
        if lhs_ty == "ptr" && matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            self.ensure_extern("declare i1 @_mvl_string_eq(ptr, ptr)");
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = call i1 @_mvl_string_eq(ptr {lv}, ptr {rv})"
            ));
            if matches!(op, BinaryOp::Ne) {
                let neg = self.next_reg();
                self.push_instr(&format!("{neg} = xor i1 {reg}, true"));
                self.fn_ctx.reg_types.insert(neg.clone(), "i1".into());
                return Ok(Some(neg));
            }
            self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
            return Ok(Some(reg));
        }

        let instr = Self::binary_instr(op, is_float, &lhs_ty, &lv, &rv);
        let reg = self.next_reg();
        self.push_instr(&format!("{reg} = {instr}"));

        let result_ty = if op.is_comparison() {
            "i1"
        } else if is_float {
            "double"
        } else {
            "i64"
        };
        self.fn_ctx.reg_types.insert(reg.clone(), result_ty.into());
        Ok(Some(reg))
    }

    fn emit_short_circuit_and_tir(
        &mut self,
        left: &TirExpr,
        right: &TirExpr,
    ) -> Result<Option<String>, String> {
        let lv = match self.emit_expr_tir(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_bb = self.next_bb("and_rhs");
        let merge_bb = self.next_bb("and_merge");
        let left_end = self.fn_ctx.current_bb.clone();

        self.push_instr(&format!("br i1 {lv}, label %{rhs_bb}, label %{merge_bb}"));
        self.start_bb(&rhs_bb);
        let rv = match self.emit_expr_tir(right)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_end = self.fn_ctx.current_bb.clone();
        self.push_instr(&format!("br label %{merge_bb}"));

        self.start_bb(&merge_bb);
        let result = self.next_reg();
        self.push_instr(&format!(
            "{result} = phi i1 [ false, %{left_end} ], [ {rv}, %{rhs_end} ]"
        ));
        self.fn_ctx.reg_types.insert(result.clone(), "i1".into());
        Ok(Some(result))
    }

    fn emit_short_circuit_or_tir(
        &mut self,
        left: &TirExpr,
        right: &TirExpr,
    ) -> Result<Option<String>, String> {
        let lv = match self.emit_expr_tir(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_bb = self.next_bb("or_rhs");
        let merge_bb = self.next_bb("or_merge");
        let left_end = self.fn_ctx.current_bb.clone();

        self.push_instr(&format!("br i1 {lv}, label %{merge_bb}, label %{rhs_bb}"));
        self.start_bb(&rhs_bb);
        let rv = match self.emit_expr_tir(right)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_end = self.fn_ctx.current_bb.clone();
        self.push_instr(&format!("br label %{merge_bb}"));

        self.start_bb(&merge_bb);
        let result = self.next_reg();
        self.push_instr(&format!(
            "{result} = phi i1 [ true, %{left_end} ], [ {rv}, %{rhs_end} ]"
        ));
        self.fn_ctx.reg_types.insert(result.clone(), "i1".into());
        Ok(Some(result))
    }

    /// TIR variant of [`Self::emit_unary`].
    fn emit_unary_tir(
        &mut self,
        op: &UnaryOp,
        expr: &TirExpr,
    ) -> Result<Option<String>, String> {
        let val = match self.emit_expr_tir(expr)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let is_float = tir_expr_is_float(expr);
        let reg = self.next_reg();
        match op {
            UnaryOp::Neg if is_float => {
                self.push_instr(&format!("{reg} = fneg double {val}"));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
            }
            UnaryOp::Neg => {
                self.push_instr(&format!("{reg} = sub i64 0, {val}"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
            }
            UnaryOp::Not => {
                self.push_instr(&format!("{reg} = xor i1 {val}, true"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
            }
            UnaryOp::BitNot => {
                self.push_instr(&format!("{reg} = xor i64 {val}, -1"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
            }
            UnaryOp::Deref => {
                // Box[T] deref: load T through the pointer. The inner type is
                // available directly from the TIR `.ty` (no AST walk needed).
                if let Ty::Named(name, args) = &expr.ty {
                    if name == "Box" {
                        if let Some(inner) = args.first() {
                            let load_ty = self.ty_to_llvm_ctx(inner);
                            let loaded = self.next_reg();
                            self.push_instr(&format!("{loaded} = load {load_ty}, ptr {val}"));
                            self.fn_ctx.reg_types.insert(loaded.clone(), load_ty);
                            return Ok(Some(loaded));
                        }
                    }
                }
                return Ok(Some(val));
            }
        }
        Ok(Some(reg))
    }

    /// TIR variant of [`Self::emit_match_expr`].
    ///
    /// Three delegate paths — Option, Result, payload-enum — handled by
    /// dedicated TIR helpers below. Falls through to the generic unit-enum +
    /// wildcard case for everything else.
    fn emit_match_expr_tir(
        &mut self,
        scrutinee: &TirExpr,
        arms: &[TirMatchArm],
    ) -> Result<Option<String>, String> {
        let scrut_val = match self.emit_expr_tir(scrutinee)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let has_ok_err = arms
            .iter()
            .any(|a| matches!(&a.pattern, Pattern::Ok { .. } | Pattern::Err { .. }));
        if has_ok_err {
            return self.emit_result_match_tir(scrutinee, &scrut_val, arms);
        }

        let has_some_none = arms
            .iter()
            .any(|a| matches!(&a.pattern, Pattern::Some { .. } | Pattern::None(_)));
        if has_some_none {
            return self.emit_option_match_tir(scrutinee, &scrut_val, arms);
        }

        if let Some(enum_name) = self.scrutinee_payload_enum_tir(scrutinee) {
            return self.emit_payload_enum_match_tir(&enum_name, &scrut_val, arms);
        }

        // Generic unit-enum / scalar match.
        let scrut_ty = self.ty_to_llvm_ctx(&scrutinee.ty);

        let n = self.fn_ctx.bb;
        self.fn_ctx.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);

        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        let mut switch_arms: Vec<(i64, usize)> = Vec::new();
        let mut wildcard_arm: Option<usize> = None;

        for (idx, arm) in arms.iter().enumerate() {
            if self.collect_or_discriminants_tir(
                &arm.pattern,
                idx,
                &mut switch_arms,
                &mut wildcard_arm,
            ) {
                continue;
            }
            wildcard_arm = Some(idx);
        }

        let use_switch = !switch_arms.is_empty();
        if use_switch {
            let mut switch_str = format!("switch {scrut_ty} {scrut_val}, label %{default_bb} [\n");
            for (disc, arm_idx) in &switch_arms {
                switch_str.push_str(&format!(
                    "    {scrut_ty} {disc}, label %{}\n",
                    arm_bbs[*arm_idx]
                ));
            }
            switch_str.push_str("  ]");
            self.push_instr(&switch_str);
        } else {
            self.push_instr(&format!("br label %{default_bb}"));
        }

        let mut phi_entries: Vec<(String, String, String)> = Vec::new();
        let mut no_val_arms: Vec<String> = Vec::new();

        for (idx, arm) in arms.iter().enumerate() {
            let arm_bb = &arm_bbs[idx];
            self.fn_ctx.fn_buf.push(format!("{arm_bb}:"));
            self.fn_ctx.current_bb = arm_bb.clone();
            self.fn_ctx.terminated = false;

            if let Pattern::Ident(name, _) = &arm.pattern {
                if !name.contains("::") {
                    self.fn_ctx.locals.insert(name.clone(), scrut_val.clone());
                }
            }

            let arm_val = self.emit_match_arm_body_tir(&arm.body)?;

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

            if let Pattern::Ident(name, _) = &arm.pattern {
                if !name.contains("::") {
                    self.fn_ctx.locals.remove(name);
                }
            }
        }

        self.fn_ctx.fn_buf.push(format!("{default_bb}:"));
        self.fn_ctx.current_bb = default_bb.clone();
        self.fn_ctx.terminated = false;

        if let Some(wild_idx) = wildcard_arm {
            self.push_instr(&format!("br label %{}", arm_bbs[wild_idx]));
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.fn_ctx.terminated = true;
        }

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

    /// Identify whether `scrutinee` is a payload-enum expression — TIR-side.
    ///
    /// Uses `scrutinee.ty` directly (no AST walk needed) — cleaner than the
    /// AST equivalent which threads through `local_mvl_types` / `fn_ret_types`.
    fn scrutinee_payload_enum_tir(&self, scrutinee: &TirExpr) -> Option<String> {
        let mut cur = &scrutinee.ty;
        loop {
            match cur {
                Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
                    cur = inner;
                }
                Ty::Named(name, _) => {
                    if self.module.enum_variants.contains_key(name)
                        && self.enum_has_payloads(name)
                    {
                        return Some(name.clone());
                    }
                    return None;
                }
                _ => return None,
            }
        }
    }

    /// Helper: emit a [`TirMatchBody`] and return its value.
    fn emit_match_arm_body_tir(
        &mut self,
        body: &TirMatchBody,
    ) -> Result<Option<String>, String> {
        match body {
            TirMatchBody::Expr(e) => self.emit_expr_tir(e),
            TirMatchBody::Block(b) => self.emit_block_tir(b),
        }
    }

    /// TIR variant of [`collect_or_discriminants`] — collects discriminant values
    /// from a (possibly Or-flattened) pattern. Returns `true` if all sub-patterns
    /// matched as concrete discriminants; `false` if any sub-pattern was a
    /// wildcard or unsupported variant.
    fn collect_or_discriminants_tir(
        &self,
        pattern: &Pattern,
        arm_idx: usize,
        switch_arms: &mut Vec<(i64, usize)>,
        wildcard_arm: &mut Option<usize>,
    ) -> bool {
        match pattern {
            Pattern::Ident(name, _) if name.contains("::") => {
                if let Some(disc) = self.pattern_discriminant(name) {
                    switch_arms.push((disc, arm_idx));
                    true
                } else {
                    false
                }
            }
            Pattern::Literal(crate::mvl::ir::Literal::Integer(n), _) => {
                switch_arms.push((*n, arm_idx));
                true
            }
            Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                *wildcard_arm = Some(arm_idx);
                true
            }
            Pattern::Or { patterns: alts, .. } => {
                for alt in alts {
                    if !self.collect_or_discriminants_tir(
                        alt,
                        arm_idx,
                        switch_arms,
                        wildcard_arm,
                    ) {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// TIR variant of [`Self::emit_option_match`].
    fn emit_option_match_tir(
        &mut self,
        scrutinee: &TirExpr,
        scrut_val: &str,
        arms: &[TirMatchArm],
    ) -> Result<Option<String>, String> {
        // Inner type of Option[T] — read directly from scrutinee.ty.
        let inner_ty: Option<Ty> = match unwrap_labels(&scrutinee.ty) {
            Ty::Option(inner) => Some((**inner).clone()),
            _ => None,
        };
        let inner_load_ty = inner_ty
            .as_ref()
            .map(|t| self.ty_to_llvm_ctx(t))
            .unwrap_or_else(|| "ptr".into());

        // Extract discriminant byte from { i8, ptr }.
        let disc_reg = self.next_reg();
        self.push_instr(&format!(
            "{disc_reg} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 0"
        ));
        self.fn_ctx.reg_types.insert(disc_reg.clone(), "i8".into());

        let n = self.fn_ctx.bb;
        self.fn_ctx.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);
        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        let mut switch_str = format!("switch i8 {disc_reg}, label %{default_bb} [\n");
        let mut wildcard_arm: Option<usize> = None;
        for (idx, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                Pattern::Some { .. } => {
                    switch_str.push_str(&format!("    i8 0, label %{}\n", arm_bbs[idx]));
                }
                Pattern::None(_) => {
                    switch_str.push_str(&format!("    i8 1, label %{}\n", arm_bbs[idx]));
                }
                _ => {
                    wildcard_arm = Some(idx);
                }
            }
        }
        switch_str.push_str("  ]");
        self.push_instr(&switch_str);

        let mut phi_entries: Vec<(String, String, String)> = Vec::new();
        let mut no_val_arms: Vec<String> = Vec::new();

        for (idx, arm) in arms.iter().enumerate() {
            if Some(idx) == wildcard_arm {
                continue;
            }
            let arm_bb = &arm_bbs[idx];
            self.fn_ctx.fn_buf.push(format!("{arm_bb}:"));
            self.fn_ctx.current_bb = arm_bb.clone();
            self.fn_ctx.terminated = false;

            let mut bound_var: Option<String> = None;
            if let Pattern::Some { inner, .. } = &arm.pattern {
                let pp = self.next_reg();
                self.push_instr(&format!(
                    "{pp} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 1"
                ));
                let some_val = self.next_reg();
                self.push_instr(&format!("{some_val} = load {inner_load_ty}, ptr {pp}"));
                self.fn_ctx
                    .reg_types
                    .insert(some_val.clone(), inner_load_ty.clone());
                if let Pattern::Ident(var_name, _) = inner.as_ref() {
                    if var_name != "_" {
                        self.fn_ctx
                            .locals
                            .insert(var_name.clone(), some_val.clone());
                        if let Some(ref imty) = inner_ty {
                            if let Some(te) = super::emit_stmts::ty_to_type_expr(imty) {
                                self.fn_ctx
                                    .local_mvl_types
                                    .insert(var_name.clone(), te);
                            }
                        }
                        bound_var = Some(var_name.clone());
                    }
                }
            }

            let arm_val = self.emit_match_arm_body_tir(&arm.body)?;
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
            if let Some(ref var_name) = bound_var {
                self.fn_ctx.locals.remove(var_name);
                self.fn_ctx.local_mvl_types.remove(var_name);
            }
        }

        self.fn_ctx.fn_buf.push(format!("{default_bb}:"));
        self.fn_ctx.current_bb = default_bb.clone();
        self.fn_ctx.terminated = false;
        if let Some(wild_idx) = wildcard_arm {
            let wild_arm = &arms[wild_idx];
            let mut bound_var: Option<String> = None;
            if let Pattern::Ident(name, _) = &wild_arm.pattern {
                self.fn_ctx
                    .locals
                    .insert(name.clone(), scrut_val.to_string());
                bound_var = Some(name.clone());
            }
            let arm_val = self.emit_match_arm_body_tir(&wild_arm.body)?;
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
            if let Some(ref var_name) = bound_var {
                self.fn_ctx.locals.remove(var_name);
                self.fn_ctx.local_mvl_types.remove(var_name);
            }
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.fn_ctx.terminated = true;
        }

        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming == 0 {
            self.fn_ctx.fn_buf.push(format!("{merge_bb}:"));
            self.fn_ctx.current_bb = merge_bb.clone();
            self.fn_ctx.terminated = false;
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.fn_ctx.terminated = true;
            return Ok(None);
        }
        self.fn_ctx.fn_buf.push(format!("{merge_bb}:"));
        self.fn_ctx.current_bb = merge_bb.clone();
        self.fn_ctx.terminated = false;
        if total_incoming >= 2 && !phi_entries.is_empty() {
            let phi_ty = phi_entries[0].1.clone();
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

    /// TIR variant of [`Self::emit_result_match`].
    fn emit_result_match_tir(
        &mut self,
        scrutinee: &TirExpr,
        scrut_val: &str,
        arms: &[TirMatchArm],
    ) -> Result<Option<String>, String> {
        let (ok_load_ty, err_load_ty) = match unwrap_labels(&scrutinee.ty) {
            Ty::Result(ok, err) => (self.ty_to_llvm_ctx(ok), self.ty_to_llvm_ctx(err)),
            _ => ("i64".to_string(), "ptr".to_string()),
        };

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

        let mut phi_entries: Vec<(String, String, String)> = Vec::new();
        let mut no_val_arms: Vec<String> = Vec::new();

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

            let arm_val = self.emit_match_arm_body_tir(&arm.body)?;
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

    /// TIR variant of [`Self::emit_payload_enum_match`]. Not yet ported.
    fn emit_payload_enum_match_tir(
        &mut self,
        _enum_name: &str,
        _scrut_val: &str,
        _arms: &[TirMatchArm],
    ) -> Result<Option<String>, String> {
        Err("emit_match_expr_tir: payload-enum match not yet ported".into())
    }

    /// TIR variant of [`Self::emit_list_literal`].
    fn emit_list_literal_tir(
        &mut self,
        elems: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let elem_ty = elems
            .first()
            .map(|e| self.ty_to_llvm_ctx(&e.ty))
            .unwrap_or_else(|| "ptr".into());

        let mut elem_vals: Vec<String> = Vec::new();
        for e in elems {
            if let Some(v) = self.emit_expr_tir(e)? {
                elem_vals.push(v);
            }
        }

        let n = elem_vals.len().max(4) as i64;
        self.ensure_extern("declare ptr @_mvl_array_new(i64, i64)");
        self.ensure_extern("declare void @_mvl_array_push(ptr, ptr)");

        let arr = self.next_reg();
        let elem_size = Self::llvm_type_size(&elem_ty);
        self.push_instr(&format!(
            "{arr} = call ptr @_mvl_array_new(i64 {elem_size}, i64 {n})"
        ));
        self.fn_ctx.reg_types.insert(arr.clone(), "ptr".into());

        for v in &elem_vals {
            let slot = self.next_reg();
            self.push_instr(&format!("{slot} = alloca {elem_ty}"));
            self.push_instr(&format!("store {elem_ty} {v}, ptr {slot}"));
            self.push_instr(&format!(
                "call void @_mvl_array_push(ptr {arr}, ptr {slot})"
            ));
        }

        Ok(Some(arr))
    }

    /// TIR variant of [`Self::emit_map_literal`].
    fn emit_map_literal_tir(
        &mut self,
        pairs: &[(TirExpr, TirExpr)],
    ) -> Result<Option<String>, String> {
        let n = pairs.len().max(4) as i64;
        self.ensure_extern("declare ptr @_mvl_map_new(i64)");
        self.ensure_extern("declare void @_mvl_map_insert(ptr, ptr, i64, ptr, i64)");
        self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
        self.ensure_extern("declare i64 @_mvl_str_len(ptr)");

        let map = self.next_reg();
        self.push_instr(&format!("{map} = call ptr @_mvl_map_new(i64 {n})"));
        self.fn_ctx.reg_types.insert(map.clone(), "ptr".into());

        for (key_expr, val_expr) in pairs {
            let key_val = match self.emit_expr_tir(key_expr)? {
                Some(v) => v,
                None => continue,
            };
            let key_ptr = self.next_reg();
            self.push_instr(&format!(
                "{key_ptr} = call ptr @_mvl_string_ptr(ptr {key_val})"
            ));
            let key_len = self.next_reg();
            self.push_instr(&format!(
                "{key_len} = call i64 @_mvl_str_len(ptr {key_val})"
            ));

            let val_val = match self.emit_expr_tir(val_expr)? {
                Some(v) => v,
                None => continue,
            };
            let val_ty = self.infer_val_type(&val_val);
            let val_slot = self.next_reg();
            self.push_instr(&format!("{val_slot} = alloca {val_ty}"));
            self.push_instr(&format!("store {val_ty} {val_val}, ptr {val_slot}"));

            self.push_instr(&format!(
                "call void @_mvl_map_insert(ptr {map}, ptr {key_ptr}, i64 {key_len}, ptr {val_slot}, i64 8)"
            ));
        }

        Ok(Some(map))
    }

    /// TIR variant of [`Self::emit_result_constructor`] (Ok/Err).
    fn emit_result_constructor_tir(
        &mut self,
        name: &str,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let disc: i64 = if name == "Ok" { 0 } else { 1 };
        let arg_ty;
        let slot;
        if let Some(arg) = args.first() {
            let inferred_ty = self.ty_to_llvm_ctx(&arg.ty);
            if inferred_ty == "void" {
                let _ = self.emit_expr_tir(arg)?;
                arg_ty = "i8".to_string();
                slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca i8"));
            } else {
                arg_ty = inferred_ty;
                let arg_val = match self.emit_expr_tir(arg)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca {arg_ty}"));
                self.push_instr(&format!("store {arg_ty} {arg_val}, ptr {slot}"));
            }
        } else {
            arg_ty = "i8".to_string();
            slot = self.next_reg();
            self.push_instr(&format!("{slot} = alloca i8"));
        };
        let r1 = self.wrap_result_pair(&disc.to_string(), &slot);
        let _ = arg_ty;
        Ok(Some(r1))
    }

    /// TIR variant of [`Self::emit_option_constructor`] (Some).
    fn emit_option_constructor_tir(
        &mut self,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let arg = match args.first() {
            Some(a) => a,
            None => return self.emit_none_constructor(),
        };
        let arg_ty = self.ty_to_llvm_ctx(&arg.ty);
        let arg_val = match self.emit_expr_tir(arg)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let slot = self.next_reg();
        self.push_instr(&format!("{slot} = alloca {arg_ty}"));
        self.push_instr(&format!("store {arg_ty} {arg_val}, ptr {slot}"));
        let r1 = self.wrap_result_pair("0", &slot);
        Ok(Some(r1))
    }

    /// TIR variant of [`Self::emit_enum_variant_constructor`] (payload enums).
    fn emit_enum_variant_constructor_tir(
        &mut self,
        qualified_name: &str,
        disc: i64,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let field_tys: Vec<crate::mvl::ir::TypeExpr> = self
            .variant_payload_types(qualified_name)
            .map(|s| s.to_vec())
            .unwrap_or_default();

        let payload_ptr: String = if field_tys.is_empty() {
            "null".to_string()
        } else {
            if args.len() != field_tys.len() {
                return Err(format!(
                    "variant {qualified_name}: expected {} fields, got {}",
                    field_tys.len(),
                    args.len()
                ));
            }
            let n = field_tys.len();
            let base = self.next_reg();
            self.push_instr(&format!("{base} = alloca [{n} x i64]"));
            for (i, (ty_expr, arg)) in field_tys.iter().zip(args.iter()).enumerate() {
                let field_llvm = self.llvm_ty_ctx(ty_expr);
                let arg_val = match self.emit_expr_tir(arg)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let slot = self.next_reg();
                self.push_instr(&format!(
                    "{slot} = getelementptr [{n} x i64], ptr {base}, i32 0, i32 {i}"
                ));
                self.push_instr(&format!("store {field_llvm} {arg_val}, ptr {slot}"));
            }
            base
        };
        let r1 = self.wrap_result_pair(&disc.to_string(), &payload_ptr);
        Ok(Some(r1))
    }

    /// TIR variant of [`Self::emit_assert_builtin`].
    fn emit_assert_builtin_tir(
        &mut self,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let cond = match args.first() {
            Some(a) => a,
            None => return Ok(None),
        };
        let cond_val = match self.emit_expr_tir(cond)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let ok_bb = self.next_bb("assert_ok");
        let fail_bb = self.next_bb("assert_fail");
        self.push_instr(&format!(
            "br i1 {cond_val}, label %{ok_bb}, label %{fail_bb}"
        ));
        self.fn_ctx.fn_buf.push(format!("{fail_bb}:"));
        self.fn_ctx.current_bb = fail_bb.clone();
        self.fn_ctx.terminated = false;
        self.ensure_extern("declare void @llvm.trap()");
        self.push_instr("call void @llvm.trap()");
        self.push_instr("unreachable");
        self.fn_ctx.terminated = true;
        self.fn_ctx.fn_buf.push(format!("{ok_bb}:"));
        self.fn_ctx.current_bb = ok_bb;
        self.fn_ctx.terminated = false;
        Ok(None)
    }

    /// TIR variant of [`Self::emit_println_builtin`].
    fn emit_println_builtin_tir(
        &mut self,
        name: &str,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let fd = if name == "eprintln" { 2i32 } else { 1i32 };
        if args.is_empty() {
            let fmt = self.ensure_println_fmt();
            self.ensure_extern("declare i32 @dprintf(i32, ptr, ...)");
            let empty_g = self.emit_str_global("");
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = call ptr @_mvl_string_new(ptr @{empty_g}, i64 0)"
            ));
            self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
            let raw = self.next_reg();
            self.push_instr(&format!("{raw} = call ptr @_mvl_string_ptr(ptr {reg})"));
            self.push_instr(&format!(
                "call i32 (i32, ptr, ...) @dprintf(i32 {fd}, ptr @{fmt}, ptr {raw})"
            ));
            return Ok(None);
        }
        let val = match self.emit_expr_tir(&args[0])? {
            Some(v) => v,
            None => return Ok(None),
        };
        let fmt = self.ensure_println_fmt();
        self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
        self.ensure_extern("declare i32 @dprintf(i32, ptr, ...)");
        let raw = self.next_reg();
        self.push_instr(&format!("{raw} = call ptr @_mvl_string_ptr(ptr {val})"));
        self.push_instr(&format!(
            "call i32 (i32, ptr, ...) @dprintf(i32 {fd}, ptr @{fmt}, ptr {raw})"
        ));
        Ok(None)
    }

    /// TIR variant of [`Self::emit_construct`].
    fn emit_construct_tir(
        &mut self,
        name: &str,
        fields: &[(String, TirExpr)],
    ) -> Result<Option<String>, String> {
        // Named-field enum variant construction handled in a subsequent commit
        // (needs emit_enum_variant_constructor_tir).
        if let Some((type_name, _)) = name.split_once("::") {
            if self.module.enum_variants.contains_key(type_name) {
                return Err(format!(
                    "emit_construct_tir: enum variant `{name}` not yet ported"
                ));
            }
        }

        let field_defs = match self.module.struct_fields.get(name).cloned() {
            Some(f) => f,
            None => return Ok(None),
        };

        let mut field_vals: Vec<(String, String)> = Vec::new();
        for (field_name, field_ty) in &field_defs {
            let llvm_t = self.llvm_ty_ctx(field_ty);
            let val = fields
                .iter()
                .find(|(n, _)| n == field_name)
                .and_then(|(_, e)| self.emit_expr_tir(e).ok().flatten())
                .unwrap_or_else(|| "undef".into());
            field_vals.push((llvm_t, val));
        }

        let struct_ty = format!("%{name}");
        let mut acc = "undef".to_string();
        for (i, (field_ty, val)) in field_vals.iter().enumerate() {
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = insertvalue {struct_ty} {acc}, {field_ty} {val}, {i}"
            ));
            self.fn_ctx.reg_types.insert(reg.clone(), struct_ty.clone());
            acc = reg;
        }
        Ok(Some(acc))
    }

    /// TIR variant of [`Self::emit_field_access`].
    ///
    /// Uses the receiver's `expr.ty` directly to find the struct name —
    /// no AST `struct_name_of_expr` walk needed.
    fn emit_field_access_tir(
        &mut self,
        expr: &TirExpr,
        field: &str,
    ) -> Result<Option<String>, String> {
        // In actor method bodies, `self.field` maps to a ref_local GEP pointer.
        if let TirExprKind::Var(name) = &expr.kind {
            if name == "self" {
                if let Some(loc) = self.fn_ctx.ref_locals.get(field).cloned() {
                    let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = load {ty_str}, ptr {}", loc.ptr));
                    self.fn_ctx.reg_types.insert(reg.clone(), ty_str);
                    return Ok(Some(reg));
                }
            }
        }

        // Determine the struct name from the receiver's resolved type.
        let struct_name = match &expr.ty {
            Ty::Named(n, _) => Some(n.clone()),
            Ty::Ref(_, inner) => match inner.as_ref() {
                Ty::Named(n, _) => Some(n.clone()),
                _ => None,
            },
            _ => None,
        };

        let base_val = match self.emit_expr_tir(expr)? {
            Some(v) => v,
            None => return Ok(None),
        };

        if let Some(sn) = struct_name {
            if let Some(fields) = self.module.struct_fields.get(&sn).cloned() {
                if let Some(idx) = fields.iter().position(|(f, _)| f == field) {
                    let field_ty = self.llvm_ty_ctx(&fields[idx].1.clone());
                    let reg = self.next_reg();
                    self.push_instr(&format!(
                        "{reg} = extractvalue %{sn} {base_val}, {idx}"
                    ));
                    self.fn_ctx.reg_types.insert(reg.clone(), field_ty);
                    return Ok(Some(reg));
                }
            }
        }
        Ok(None)
    }
}

fn unwrap_labels(ty: &Ty) -> &Ty {
    let mut cur = ty;
    while let Ty::Labeled(_, inner) | Ty::Refined(inner, _) | Ty::Ref(_, inner) = cur {
        cur = inner;
    }
    cur
}
