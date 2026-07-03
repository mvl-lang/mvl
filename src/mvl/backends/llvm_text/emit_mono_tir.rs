// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Generic-function monomorphization for the TIR-walking emitter (#1612, Bug 4).
//!
//! `MonoQueue::tir_generic_fns` holds the generic TIR originals.
//! [`Self::emit_monomorphized_call_tir`] is the call-site enqueue, and the
//! drain loop lives in [`emit_program_tir`].
//!
//! Concretion at call sites is straightforward: every `TirExpr` carries its
//! fully resolved `Ty` inline, so type-parameter bindings come from `arg.ty`
//! directly.

use std::collections::HashMap;

use crate::mvl::checker::types::Ty;
use crate::mvl::ir::lower::substitute_ty;
use crate::mvl::ir::TypeExpr;
use crate::mvl::ir::{
    TirBlock, TirElseBranch, TirExpr, TirExprKind, TirFn, TirMatchArm, TirMatchBody, TirParam,
    TirStmt,
};

use super::emit_program_tir::ty_to_type_expr_or_unit;
use super::TextEmitter;

impl TextEmitter {
    /// Emit a call to a generic TIR function, enqueuing the monomorphized copy
    /// if it has not been seen yet.
    ///
    /// Mirror of [`Self::emit_monomorphized_call`] but operates on `TirExpr`
    /// arguments. The concrete type substitution map is built from each
    /// argument's resolved `Ty` rather than the AST `Expr` shape.
    pub(super) fn emit_monomorphized_call_tir(
        &mut self,
        name: &str,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let gf = self
            .mono
            .tir_generic_fns
            .get(name)
            .cloned()
            .ok_or_else(|| {
                format!("ICE: generic TIR fn '{name}' missing from monomorphization table")
            })?;

        // Infer concrete `Ty` for each type parameter by matching declared
        // parameter shape against the runtime arg type.
        let mut tp_map: HashMap<String, Ty> = HashMap::new();
        for (param, arg) in gf.params.iter().zip(args.iter()) {
            collect_tir_type_bindings(&param.ty, &arg.ty, &gf, &mut tp_map);
        }
        let concrete_tys: Vec<Ty> = gf
            .type_params
            .iter()
            .map(|tp| tp_map.get(tp.name()).cloned().unwrap_or(Ty::Int))
            .collect();

        // Mangling operates on `TypeExpr`. The Ty → TypeExpr conversion is
        // lossy for some edge cases but is consistent with the type-lowering
        // helper used elsewhere, so call-site symbols agree across the pipeline.
        let concrete_te: Vec<TypeExpr> = concrete_tys.iter().map(ty_to_type_expr_or_unit).collect();
        let mangled = Self::mangle_generic(name, &concrete_te);

        if self.mono.tir_mono_emitted.insert(mangled.clone()) {
            self.mono.tir_mono_queue.push((
                mangled.clone(),
                name.to_string(),
                concrete_tys.clone(),
            ));

            // Register substituted return + param signatures so subsequent
            // call sites (and the body emission of the mangled copy) see the
            // concrete types via the existing dispatch tables.
            let mut subs: HashMap<String, Ty> = HashMap::new();
            for (tp, ct) in gf.type_params.iter().zip(concrete_tys.iter()) {
                subs.insert(tp.name().to_string(), ct.clone());
            }
            let resolved_ret = substitute_ty(&gf.ret_ty, &subs);
            self.module
                .fn_ret_types
                .insert(mangled.clone(), ty_to_type_expr_or_unit(&resolved_ret));
            let resolved_params: Vec<TypeExpr> = gf
                .params
                .iter()
                .map(|p| ty_to_type_expr_or_unit(&substitute_ty(&p.ty, &subs)))
                .collect();
            self.module
                .fn_param_types
                .insert(mangled.clone(), resolved_params);
        }

        // Emit the call. Argument lowering uses the runtime arg types (already
        // concrete in TIR), not the generic param shapes.
        let mut arg_vals: Vec<(String, String)> = Vec::new();
        for arg in args {
            let ty_str = self.ty_to_llvm_ctx(&arg.ty);
            if let Some(v) = self.emit_expr_tir(arg)? {
                arg_vals.push((ty_str, v));
            }
        }
        let args_str = arg_vals
            .iter()
            .map(|(ty, v)| format!("{ty} {v}"))
            .collect::<Vec<_>>()
            .join(", ");

        let ret_te = self
            .module
            .fn_ret_types
            .get(&mangled)
            .cloned()
            .unwrap_or_else(|| TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: Default::default(),
            });
        let llvm_ret = Self::llvm_ty(&ret_te);
        let is_void = matches!(
            &ret_te,
            TypeExpr::Base { name, args, .. } if name == "Unit" && args.is_empty()
        );

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

    /// Clone a generic TIR fn and substitute every `Ty` occurrence in its
    /// signature and body with the concrete types in `subs`.
    ///
    /// Called from the drain loop in `emit_program_tir` once per mangled
    /// instantiation. The returned `TirFn` is then walked by
    /// [`Self::emit_fn_tir`] as if it were an ordinary non-generic function.
    pub(super) fn substitute_tir_fn(&self, gf: &TirFn, subs: &HashMap<String, Ty>) -> TirFn {
        let mut out = gf.clone();
        out.params = out
            .params
            .iter()
            .map(|p| TirParam {
                ty: substitute_ty(&p.ty, subs),
                ..p.clone()
            })
            .collect();
        out.ret_ty = substitute_ty(&out.ret_ty, subs);
        substitute_block(&mut out.body, subs);
        out
    }
}

/// Pattern-match a generic parameter type against the runtime arg type to
/// bind type variables (e.g. `param=Ty::Named("T", [])` + `arg=Ty::Int` yields
/// `T → Int`). Compound types recurse so `List[T]` vs `List[Int]` binds T.
fn collect_tir_type_bindings(
    param_ty: &Ty,
    arg_ty: &Ty,
    gf: &TirFn,
    map: &mut HashMap<String, Ty>,
) {
    // Bare `Ty::Named(name, [])` with `name` listed in the fn's type params is a
    // type-parameter reference — bind it. (`lower::substitute_ty` uses the same
    // criterion.)
    if let Ty::Named(name, args) = param_ty {
        if args.is_empty() && gf.type_params.iter().any(|tp| tp.name() == name) {
            map.insert(name.clone(), arg_ty.clone());
            return;
        }
    }
    match (param_ty, arg_ty) {
        (Ty::List(a), Ty::List(b))
        | (Ty::Set(a), Ty::Set(b))
        | (Ty::Option(a), Ty::Option(b))
        | (Ty::Ptr(a), Ty::Ptr(b)) => collect_tir_type_bindings(a, b, gf, map),
        (Ty::Map(ak, av), Ty::Map(bk, bv)) => {
            collect_tir_type_bindings(ak, bk, gf, map);
            collect_tir_type_bindings(av, bv, gf, map);
        }
        (Ty::Result(ao, ae), Ty::Result(bo, be)) => {
            collect_tir_type_bindings(ao, bo, gf, map);
            collect_tir_type_bindings(ae, be, gf, map);
        }
        (Ty::Ref(_, a), Ty::Ref(_, b)) => collect_tir_type_bindings(a, b, gf, map),
        (Ty::Labeled(_, a), Ty::Labeled(_, b)) => collect_tir_type_bindings(a, b, gf, map),
        (Ty::Array(a, _), Ty::Array(b, _)) => collect_tir_type_bindings(a, b, gf, map),
        (Ty::Named(_, args_a), Ty::Named(_, args_b)) if args_a.len() == args_b.len() => {
            for (a, b) in args_a.iter().zip(args_b.iter()) {
                collect_tir_type_bindings(a, b, gf, map);
            }
        }
        _ => {}
    }
}

fn substitute_block(b: &mut TirBlock, subs: &HashMap<String, Ty>) {
    for s in &mut b.stmts {
        substitute_stmt(s, subs);
    }
}

fn substitute_stmt(s: &mut TirStmt, subs: &HashMap<String, Ty>) {
    match s {
        TirStmt::Let { ty, init, .. } => {
            *ty = substitute_ty(ty, subs);
            substitute_expr(init, subs);
        }
        TirStmt::Assign { value, .. } => substitute_expr(value, subs),
        TirStmt::Return { value, .. } => {
            if let Some(v) = value {
                substitute_expr(v, subs);
            }
        }
        TirStmt::If {
            cond, then, else_, ..
        } => {
            substitute_expr(cond, subs);
            substitute_block(then, subs);
            if let Some(TirElseBranch::Block(b)) = else_ {
                substitute_block(b, subs);
            } else if let Some(TirElseBranch::If(s)) = else_ {
                substitute_stmt(s, subs);
            }
        }
        TirStmt::Match {
            scrutinee, arms, ..
        } => {
            substitute_expr(scrutinee, subs);
            for arm in arms {
                substitute_match_arm(arm, subs);
            }
        }
        TirStmt::For {
            iter,
            invariants,
            body,
            ..
        } => {
            substitute_expr(iter, subs);
            for inv in invariants {
                substitute_expr(inv, subs);
            }
            substitute_block(body, subs);
        }
        TirStmt::While {
            cond,
            invariants,
            decreases,
            body,
            ..
        } => {
            substitute_expr(cond, subs);
            for inv in invariants {
                substitute_expr(inv, subs);
            }
            if let Some(d) = decreases {
                substitute_expr(d, subs);
            }
            substitute_block(body, subs);
        }
        TirStmt::Expr { expr, .. } => substitute_expr(expr, subs),
    }
}

fn substitute_expr(e: &mut TirExpr, subs: &HashMap<String, Ty>) {
    e.ty = substitute_ty(&e.ty, subs);
    match &mut e.kind {
        TirExprKind::Literal(_) | TirExprKind::Var(_) | TirExprKind::Quantifier(_) => {}
        TirExprKind::FieldAccess { expr, .. } => substitute_expr(expr, subs),
        TirExprKind::Unary { expr, .. }
        | TirExprKind::Propagate(expr)
        | TirExprKind::Consume(expr)
        | TirExprKind::Borrow { expr, .. } => substitute_expr(expr, subs),
        TirExprKind::Relabel { expr, .. } => substitute_expr(expr, subs),
        TirExprKind::Binary { left, right, .. } => {
            substitute_expr(left, subs);
            substitute_expr(right, subs);
        }
        TirExprKind::If {
            cond, then, else_, ..
        } => {
            substitute_expr(cond, subs);
            substitute_block(then, subs);
            if let Some(e) = else_ {
                substitute_expr(e, subs);
            }
        }
        TirExprKind::Match { scrutinee, arms } => {
            substitute_expr(scrutinee, subs);
            for arm in arms {
                substitute_match_arm(arm, subs);
            }
        }
        TirExprKind::Block(b) => substitute_block(b, subs),
        TirExprKind::Lambda { params, body } => {
            for p in params {
                p.ty = substitute_ty(&p.ty, subs);
            }
            substitute_expr(body, subs);
        }
        TirExprKind::FnCall { args, .. } => {
            for a in args {
                substitute_expr(a, subs);
            }
        }
        TirExprKind::MethodCall { receiver, args, .. } => {
            substitute_expr(receiver, subs);
            for a in args {
                substitute_expr(a, subs);
            }
        }
        TirExprKind::Construct { fields, .. } | TirExprKind::Spawn { fields, .. } => {
            for (_, val) in fields {
                substitute_expr(val, subs);
            }
        }
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            for el in elems {
                substitute_expr(el, subs);
            }
        }
        TirExprKind::Map { pairs } => {
            for (k, v) in pairs {
                substitute_expr(k, subs);
                substitute_expr(v, subs);
            }
        }
        TirExprKind::Select { arms } => {
            for arm in arms {
                substitute_expr(&mut arm.expr, subs);
                substitute_block(&mut arm.body, subs);
            }
        }
    }
}

fn substitute_match_arm(arm: &mut TirMatchArm, subs: &HashMap<String, Ty>) {
    match &mut arm.body {
        TirMatchBody::Expr(e) => substitute_expr(e, subs),
        TirMatchBody::Block(b) => substitute_block(b, subs),
    }
}
