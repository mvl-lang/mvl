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

use crate::mvl::ir::{TirExpr, TirExprKind};

use super::TextEmitter;

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

            // Unimplemented variants — build out leaf-first in subsequent commits (#1612).
            _ => Err(format!(
                "emit_expr_tir: variant not yet implemented: {:?}",
                std::mem::discriminant(&expr.kind)
            )),
        }
    }
}
