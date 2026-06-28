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

            // Unimplemented variants — build out leaf-first in subsequent commits (#1612).
            _ => Err(format!(
                "emit_expr_tir: variant not yet implemented: {:?}",
                std::mem::discriminant(&expr.kind)
            )),
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

