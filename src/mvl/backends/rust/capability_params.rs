// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Capability-parameter analysis (Phase B, Spec 009 Req 2).
//!
//! For each function declaration, determines which parameters are passed by
//! reference (`&T`) rather than by value.  The result drives two
//! transformations:
//!
//! 1. **Signature**: inferred-borrow parameters are emitted as `&T` in the
//!    Rust function signature even if the MVL source declares them as `T`.
//!
//! 2. **Call sites**: arguments destined for a `&T` parameter are emitted
//!    as `&x` instead of `x.clone()`, eliminating the clone entirely.
//!
//! # Explicit vs inferred borrows
//!
//! * **Explicit**: the MVL programmer wrote `fn f(x: val T)`.  The parameter's
//!   [`TypeExpr`] is `TypeExpr::Ref { mutable: false }`.  Always a borrow.
//! * **Explicit mutable**: `fn f(x: ref T)`.  `TypeExpr::Ref { mutable: true }`.
//!   Also a borrow (call site emits `&mut x`).
//! * **Inferred immutable borrow**: the parameter is declared as owned (`T`)
//!   but analysis proves the function body never mutates it, never stores it
//!   in a struct, and never returns it.  Safe to pass as `&T`.
//!
//! # Conservative cases
//!
//! Parameters that are passed to *other* MVL functions are excluded from
//! inference: without a fixed-point analysis we cannot guarantee that the
//! callee also expects a reference.  Such parameters keep value semantics.
//!
//! Parameters of Copy types (Int, Float, Bool, Byte, Unit, and their labeled
//! or refined wrappers) are never inferred as borrows — cloning them is free
//! in Rust so there is no performance benefit.

use std::collections::HashMap;

use crate::mvl::parser::ast::{Capability, Param, TypeExpr};

// ── Public API ────────────────────────────────────────────────────────────────

// ── Explicit-annotation helpers ───────────────────────────────────────────────

/// Returns the borrow kind for an explicitly annotated reference type.
fn explicit_ref_mutability(ty: &TypeExpr) -> Option<bool> {
    match ty {
        TypeExpr::Ref { mutable, .. } => Some(*mutable),
        _ => None,
    }
}

/// Explicit borrow flags for a parameter list (no body analysis).
/// Used by the emitter to register borrow annotations from builtin/stdlib functions.
pub fn explicit_borrow_flags_pub(params: &[Param]) -> Vec<Option<bool>> {
    params
        .iter()
        .map(|p| explicit_ref_mutability(&p.ty))
        .collect()
}

// ── TIR-based capability analysis ────────────────────────────────────────────

pub fn build_capability_params_map_tir(
    tir: &crate::mvl::ir::TirProgram,
    prelude_tirs: &[crate::mvl::ir::TirProgram],
) -> HashMap<String, Vec<Option<bool>>> {
    let mut map = HashMap::new();

    // Prelude functions (stdlib) — explicit annotations only (no body analysis),
    // mirroring the old AST path which used explicit_borrow_flags(&fd.params).
    // Body analysis on stdlib is incorrect: it would mark `default` in
    // `unwrap_or(self, default: T)` as `&T` since the body only reads it.
    for pt in prelude_tirs {
        for f in &pt.fns {
            let flags = explicit_borrow_flags_tir(f);
            if flags.iter().any(|b| b.is_some()) {
                map.insert(f.name.clone(), flags);
            }
        }
    }

    // User functions from TIR — explicit + inferred.
    for f in &tir.fns {
        let flags = capability_params_for_tir_fn(f);
        if flags.iter().any(|b| b.is_some()) {
            map.insert(f.name.clone(), flags);
        }
    }

    map
}

/// Explicit-only borrow flags for a TIR function (no body analysis).
/// Used for prelude/stdlib functions where body analysis is incorrect.
fn explicit_borrow_flags_tir(fd: &crate::mvl::ir::TirFn) -> Vec<Option<bool>> {
    fd.params
        .iter()
        .map(|p| {
            if let crate::mvl::ir::Ty::Ref(mutable, _) = &p.ty {
                Some(*mutable)
            } else {
                None
            }
        })
        .collect()
}

/// Borrow kinds for a single TIR function.
pub fn capability_params_for_tir_fn(fd: &crate::mvl::ir::TirFn) -> Vec<Option<bool>> {
    fd.params
        .iter()
        .map(|p| {
            // Explicit Ty::Ref annotation (from `val T` or `ref T`).
            if let crate::mvl::ir::Ty::Ref(mutable, _) = &p.ty {
                return Some(*mutable);
            }
            // No benefit to borrowing Copy types.
            if is_copy_ty(&p.ty) {
                return None;
            }
            // `val` capability suppresses inferred borrow — keep owned.
            if matches!(p.capability, Some(Capability::Val)) {
                return None;
            }
            // Conservative read-only inference on the TIR body.
            if is_read_only_param_tir(&p.name, &fd.body) {
                Some(false)
            } else {
                None
            }
        })
        .collect()
}

fn is_copy_ty(ty: &crate::mvl::ir::Ty) -> bool {
    use crate::mvl::ir::Ty;
    match ty {
        Ty::Int
        | Ty::Float
        | Ty::Bool
        | Ty::Char
        | Ty::Byte
        | Ty::UByte
        | Ty::UInt
        | Ty::Unit
        | Ty::Ref(..)
        | Ty::Fn(..) => true,
        Ty::Labeled(_, inner) | Ty::Refined(inner, _) => is_copy_ty(inner),
        _ => false,
    }
}

fn is_read_only_param_tir(param: &str, body: &crate::mvl::ir::TirBlock) -> bool {
    !block_has_disqualifying_use_tir(param, body)
}

fn block_has_disqualifying_use_tir(param: &str, block: &crate::mvl::ir::TirBlock) -> bool {
    for (i, stmt) in block.stmts.iter().enumerate() {
        let is_last = i == block.stmts.len() - 1;
        if stmt_has_disqualifying_use_tir(param, stmt, is_last) {
            return true;
        }
    }
    false
}

fn stmt_has_disqualifying_use_tir(
    param: &str,
    stmt: &crate::mvl::ir::TirStmt,
    is_last: bool,
) -> bool {
    use crate::mvl::ir::{TirElseBranch, TirExprKind, TirMatchBody, TirStmt};
    match stmt {
        TirStmt::Assign { target, value, .. } => {
            lvalue_is_param_tir(target, param)
                || matches!(&value.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, value)
        }
        TirStmt::Return {
            value: Some(expr), ..
        } => {
            matches!(&expr.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, expr)
        }
        TirStmt::Return { value: None, .. } => false,
        TirStmt::Let { init, .. } => {
            matches!(&init.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, init)
        }
        TirStmt::Expr { expr, .. } => {
            (is_last && matches!(&expr.kind, TirExprKind::Var(n) if n == param))
                || expr_has_disqualifying_use_tir(param, expr)
        }
        TirStmt::If {
            cond, then, else_, ..
        } => {
            expr_has_disqualifying_use_tir(param, cond)
                || block_has_disqualifying_use_tir(param, then)
                || else_.as_ref().is_some_and(|e| match e {
                    TirElseBranch::Block(b) => block_has_disqualifying_use_tir(param, b),
                    TirElseBranch::If(s) => stmt_has_disqualifying_use_tir(param, s, false),
                })
        }
        TirStmt::Match {
            scrutinee, arms, ..
        } => {
            matches!(&scrutinee.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, scrutinee)
                || arms.iter().any(|a| match &a.body {
                    TirMatchBody::Block(b) => block_has_disqualifying_use_tir(param, b),
                    TirMatchBody::Expr(e) => expr_has_disqualifying_use_tir(param, e),
                })
        }
        TirStmt::For { iter, body, .. } => {
            matches!(&iter.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, iter)
                || block_has_disqualifying_use_tir(param, body)
        }
        TirStmt::While { cond, body, .. } => {
            expr_has_disqualifying_use_tir(param, cond)
                || block_has_disqualifying_use_tir(param, body)
        }
    }
}

fn expr_has_disqualifying_use_tir(param: &str, expr: &crate::mvl::ir::TirExpr) -> bool {
    use crate::mvl::ir::{TirExprKind, TirMatchBody};
    match &expr.kind {
        // Field access on the param itself is a read-only use — not disqualifying.
        TirExprKind::FieldAccess { expr: inner, .. } => {
            if matches!(&inner.kind, TirExprKind::Var(n) if n == param) {
                false
            } else {
                expr_has_disqualifying_use_tir(param, inner)
            }
        }
        // Method call receiver auto-derefs — not disqualifying.
        TirExprKind::MethodCall { receiver, args, .. } => {
            let recv_is_param = matches!(&receiver.kind, TirExprKind::Var(n) if n == param);
            if recv_is_param {
                // Receiver is fine; check args for bare param use (disqualifying).
                args.iter().any(|a| {
                    matches!(&a.kind, TirExprKind::Var(n) if n == param)
                        || expr_has_disqualifying_use_tir(param, a)
                })
            } else {
                expr_has_disqualifying_use_tir(param, receiver)
                    || args.iter().any(|a| {
                        matches!(&a.kind, TirExprKind::Var(n) if n == param)
                            || expr_has_disqualifying_use_tir(param, a)
                    })
            }
        }
        // Free function call: any bare param arg disqualifies.
        TirExprKind::FnCall { args, .. } => args.iter().any(|a| {
            matches!(&a.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, a)
        }),
        // Binary: direct bare param operand disqualifies.
        TirExprKind::Binary { left, right, .. } => {
            matches!(&left.kind, TirExprKind::Var(n) if n == param)
                || matches!(&right.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, left)
                || expr_has_disqualifying_use_tir(param, right)
        }
        TirExprKind::Unary {
            op, expr: inner, ..
        } => {
            // Direct `*param` dereference consumes the value (e.g., Box[T]::unwrap).
            if matches!(op, crate::mvl::parser::ast::UnaryOp::Deref)
                && matches!(&inner.kind, TirExprKind::Var(n) if n == param)
            {
                return true;
            }
            expr_has_disqualifying_use_tir(param, inner)
        }
        TirExprKind::Propagate(inner)
        | TirExprKind::Consume(inner)
        | TirExprKind::Relabel { expr: inner, .. } => {
            matches!(&inner.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, inner)
        }
        TirExprKind::Borrow { expr: inner, .. } => expr_has_disqualifying_use_tir(param, inner),
        TirExprKind::If {
            cond, then, else_, ..
        } => {
            expr_has_disqualifying_use_tir(param, cond)
                || block_has_disqualifying_use_tir(param, then)
                || else_
                    .as_ref()
                    .is_some_and(|e| expr_has_disqualifying_use_tir(param, e))
        }
        TirExprKind::Match {
            scrutinee, arms, ..
        } => {
            matches!(&scrutinee.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, scrutinee)
                || arms.iter().any(|a| match &a.body {
                    TirMatchBody::Block(b) => block_has_disqualifying_use_tir(param, b),
                    TirMatchBody::Expr(e) => expr_has_disqualifying_use_tir(param, e),
                })
        }
        TirExprKind::Block(b) => block_has_disqualifying_use_tir(param, b),
        TirExprKind::Lambda {
            params: lambda_params,
            body,
            ..
        } => {
            let shadowed = lambda_params.iter().any(|p| p.name == param);
            !shadowed && expr_mentions_param_tir(param, body)
        }
        TirExprKind::Construct { fields, .. } | TirExprKind::Spawn { fields, .. } => {
            fields.iter().any(|(_, e)| {
                matches!(&e.kind, TirExprKind::Var(n) if n == param)
                    || expr_has_disqualifying_use_tir(param, e)
            })
        }
        TirExprKind::Select { arms, .. } => arms.iter().any(|a| {
            expr_has_disqualifying_use_tir(param, &a.expr)
                || block_has_disqualifying_use_tir(param, &a.body)
        }),
        TirExprKind::List { elems } | TirExprKind::Set { elems } => elems.iter().any(|e| {
            matches!(&e.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, e)
        }),
        TirExprKind::Map { pairs } => pairs.iter().any(|(k, v)| {
            matches!(&k.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, k)
                || matches!(&v.kind, TirExprKind::Var(n) if n == param)
                || expr_has_disqualifying_use_tir(param, v)
        }),
        TirExprKind::Var(_) | TirExprKind::Literal(_) | TirExprKind::Quantifier(_) => false,
    }
}

/// Returns `true` if `param` appears anywhere inside the TIR expression —
/// used to conservatively disqualify params captured by lambdas.
fn expr_mentions_param_tir(param: &str, expr: &crate::mvl::ir::TirExpr) -> bool {
    use crate::mvl::ir::TirExprKind;
    match &expr.kind {
        TirExprKind::Var(n) => n == param,
        TirExprKind::FnCall { args, .. } => args.iter().any(|a| expr_mentions_param_tir(param, a)),
        TirExprKind::MethodCall { receiver, args, .. } => {
            expr_mentions_param_tir(param, receiver)
                || args.iter().any(|a| expr_mentions_param_tir(param, a))
        }
        TirExprKind::Binary { left, right, .. } => {
            expr_mentions_param_tir(param, left) || expr_mentions_param_tir(param, right)
        }
        TirExprKind::Unary { expr: inner, .. }
        | TirExprKind::FieldAccess { expr: inner, .. }
        | TirExprKind::Relabel { expr: inner, .. }
        | TirExprKind::Consume(inner)
        | TirExprKind::Propagate(inner)
        | TirExprKind::Borrow { expr: inner, .. } => expr_mentions_param_tir(param, inner),
        TirExprKind::Construct { fields, .. } | TirExprKind::Spawn { fields, .. } => fields
            .iter()
            .any(|(_, v)| expr_mentions_param_tir(param, v)),
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            elems.iter().any(|e| expr_mentions_param_tir(param, e))
        }
        TirExprKind::Map { pairs } => pairs
            .iter()
            .any(|(k, v)| expr_mentions_param_tir(param, k) || expr_mentions_param_tir(param, v)),
        TirExprKind::If {
            cond, then, else_, ..
        } => {
            expr_mentions_param_tir(param, cond)
                || block_mentions_param_tir(param, then)
                || else_
                    .as_ref()
                    .is_some_and(|e| expr_mentions_param_tir(param, e))
        }
        TirExprKind::Match {
            scrutinee, arms, ..
        } => {
            expr_mentions_param_tir(param, scrutinee)
                || arms.iter().any(|a| match &a.body {
                    crate::mvl::ir::TirMatchBody::Block(b) => block_mentions_param_tir(param, b),
                    crate::mvl::ir::TirMatchBody::Expr(e) => expr_mentions_param_tir(param, e),
                })
        }
        TirExprKind::Block(b) => block_mentions_param_tir(param, b),
        TirExprKind::Lambda { params, body, .. } => {
            let shadowed = params.iter().any(|p| p.name == param);
            !shadowed && expr_mentions_param_tir(param, body)
        }
        TirExprKind::Select { arms, .. } => arms.iter().any(|a| {
            expr_mentions_param_tir(param, &a.expr) || block_mentions_param_tir(param, &a.body)
        }),
        TirExprKind::Literal(_) | TirExprKind::Quantifier(_) => false,
    }
}

fn block_mentions_param_tir(param: &str, block: &crate::mvl::ir::TirBlock) -> bool {
    block
        .stmts
        .iter()
        .any(|s| stmt_mentions_param_tir(param, s))
}

fn stmt_mentions_param_tir(param: &str, stmt: &crate::mvl::ir::TirStmt) -> bool {
    use crate::mvl::ir::{TirElseBranch, TirStmt};
    match stmt {
        TirStmt::Assign { value, .. } => expr_mentions_param_tir(param, value),
        TirStmt::Return { value, .. } => value
            .as_ref()
            .is_some_and(|e| expr_mentions_param_tir(param, e)),
        TirStmt::Let { init, .. } => expr_mentions_param_tir(param, init),
        TirStmt::Expr { expr, .. } => expr_mentions_param_tir(param, expr),
        TirStmt::If {
            cond, then, else_, ..
        } => {
            expr_mentions_param_tir(param, cond)
                || block_mentions_param_tir(param, then)
                || else_.as_ref().is_some_and(|e| match e {
                    TirElseBranch::Block(b) => block_mentions_param_tir(param, b),
                    TirElseBranch::If(s) => stmt_mentions_param_tir(param, s),
                })
        }
        TirStmt::Match {
            scrutinee, arms, ..
        } => {
            expr_mentions_param_tir(param, scrutinee)
                || arms.iter().any(|a| match &a.body {
                    crate::mvl::ir::TirMatchBody::Block(b) => block_mentions_param_tir(param, b),
                    crate::mvl::ir::TirMatchBody::Expr(e) => expr_mentions_param_tir(param, e),
                })
        }
        TirStmt::For { iter, body, .. } => {
            expr_mentions_param_tir(param, iter) || block_mentions_param_tir(param, body)
        }
        TirStmt::While { cond, body, .. } => {
            expr_mentions_param_tir(param, cond) || block_mentions_param_tir(param, body)
        }
    }
}

fn lvalue_is_param_tir(lv: &crate::mvl::ir::LValue, param: &str) -> bool {
    use crate::mvl::ir::LValue;
    match lv {
        LValue::Ident(name, _) => name == param,
        LValue::Field { base, .. } => lvalue_is_param_tir(base, param),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::ir::{TirBlock, TirExpr, TirExprKind, TirFn, TirParam, TirStmt, Ty};
    use crate::mvl::parser::ast::Capability;
    use crate::mvl::parser::lexer::Span;

    fn sp() -> Span {
        Span::new(0, 0, 0, 0)
    }

    fn tir_param(name: &str, ty: Ty) -> TirParam {
        TirParam {
            name: name.to_string(),
            ty,
            capability: None,
            span: sp(),
        }
    }

    fn tir_param_cap(name: &str, ty: Ty, cap: Capability) -> TirParam {
        TirParam {
            name: name.to_string(),
            ty,
            capability: Some(cap),
            span: sp(),
        }
    }

    fn tir_var(name: &str, ty: Ty) -> TirExpr {
        TirExpr {
            kind: TirExprKind::Var(name.to_string()),
            ty,
            span: sp(),
        }
    }

    fn empty_fn(params: Vec<TirParam>) -> TirFn {
        TirFn {
            name: "f".to_string(),
            original_name: "f".to_string(),
            visible: false,
            is_test: false,
            is_builtin: false,
            receiver_type: None,
            type_params: vec![],
            constraints: vec![],
            totality: None,
            params,
            ret_ty: Ty::Unit,
            return_refinement: None,
            effects: vec![],
            requires: vec![],
            ensures: vec![],
            body: TirBlock {
                stmts: vec![],
                span: sp(),
            },
            span: sp(),
        }
    }

    fn fn_with_stmts(params: Vec<TirParam>, stmts: Vec<TirStmt>) -> TirFn {
        let mut f = empty_fn(params);
        f.body.stmts = stmts;
        f
    }

    #[test]
    fn explicit_ref_param_is_shared_borrow() {
        let fd = empty_fn(vec![tir_param("x", Ty::Ref(false, Box::new(Ty::Int)))]);
        assert_eq!(capability_params_for_tir_fn(&fd), vec![Some(false)]);
    }

    #[test]
    fn explicit_mut_ref_param_is_mutable_borrow() {
        let fd = empty_fn(vec![tir_param("x", Ty::Ref(true, Box::new(Ty::Int)))]);
        assert_eq!(capability_params_for_tir_fn(&fd), vec![Some(true)]);
    }

    #[test]
    fn copy_param_is_not_borrow() {
        let fd = empty_fn(vec![tir_param("x", Ty::Int)]);
        assert_eq!(capability_params_for_tir_fn(&fd), vec![None]);
    }

    #[test]
    fn non_copy_empty_body_inferred_as_borrow() {
        // String is non-Copy; empty body → no disqualifying uses → inferred &T.
        let fd = empty_fn(vec![tir_param("s", Ty::String)]);
        assert_eq!(capability_params_for_tir_fn(&fd), vec![Some(false)]);
    }

    #[test]
    fn val_capability_suppresses_inferred_borrow() {
        let fd = empty_fn(vec![tir_param_cap("s", Ty::String, Capability::Val)]);
        assert_eq!(capability_params_for_tir_fn(&fd), vec![None]);
    }

    #[test]
    fn param_returned_is_not_borrow() {
        let ret = TirStmt::Return {
            value: Some(tir_var("s", Ty::String)),
            span: sp(),
        };
        let fd = fn_with_stmts(vec![tir_param("s", Ty::String)], vec![ret]);
        assert_eq!(capability_params_for_tir_fn(&fd), vec![None]);
    }

    #[test]
    fn param_as_fn_arg_is_not_borrow() {
        use crate::mvl::ir::TirExprKind;
        let call = TirExpr {
            kind: TirExprKind::FnCall {
                name: "g".to_string(),
                args: vec![tir_var("s", Ty::String)],
                type_args: vec![],
            },
            ty: Ty::Unit,
            span: sp(),
        };
        let fd = fn_with_stmts(
            vec![tir_param("s", Ty::String)],
            vec![TirStmt::Expr {
                expr: call,
                span: sp(),
            }],
        );
        assert_eq!(capability_params_for_tir_fn(&fd), vec![None]);
    }

    #[test]
    fn no_params_returns_empty() {
        let fd = empty_fn(vec![]);
        assert!(capability_params_for_tir_fn(&fd).is_empty());
    }

    #[test]
    fn build_map_tir_registers_non_copy_fn() {
        use crate::mvl::ir::TirProgram;
        // A TirProgram with one function that has a String param.
        let f = empty_fn(vec![tir_param("s", Ty::String)]);
        let mut tir = TirProgram::default();
        tir.fns.push(f);
        let map = build_capability_params_map_tir(&tir, &[]);
        assert!(map.contains_key("f"), "expected 'f' in map");
        assert_eq!(map["f"], vec![Some(false)]);
    }
}
