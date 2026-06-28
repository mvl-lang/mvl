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

use crate::mvl::ir::{BinaryOp, TirExpr, TirExprKind, Ty, UnaryOp};

use super::TextEmitter;

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

            // Consume is a move marker — lowers to the inner value unchanged.
            TirExprKind::Consume(inner) => self.emit_expr_tir(inner),

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

        // Reject the special cases we haven't ported yet — keeps the TIR walker
        // safe to invoke against any program (returns Err instead of producing
        // divergent IR that would silently fail at lli runtime).
        match name {
            "assert" | "println" | "print" | "eprintln" | "format" | "Ok" | "Err" | "Some"
            | "None" | "Box::new" | "path" | "format_datetime" | "format_instant" | "find_all"
            | "replace" | "choice" | "List::filled" | "float_checked_to_int" => {
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
}

