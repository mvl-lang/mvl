// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR lowering pass: `MonoProgram + expr_types → TirProgram`.
//!
//! For each [`MonoFn`] in a [`MonoProgram`], walks the body AST and:
//! 1. Looks up the checker-resolved type for every expression via `expr_types`.
//! 2. Applies the function's type-parameter substitution to replace remaining
//!    generic placeholders (e.g. `Ty::Named("T", [])` → `Ty::Int`).
//! 3. Embeds the concrete [`Ty`] into every [`TirExpr`] node.
//!
//! # Usage
//!
//! ```rust,ignore
//! let tir = lower(&prog, &mono_program, &check_result.expr_types);
//! ```

use std::collections::HashMap;

use crate::mvl::checker::types::{resolve_session_op, Ty};
use crate::mvl::parser::ast::{
    expr_to_ref_expr_ext, ActorDecl, ActorMethod, Block, ConstDecl, Decl, ElseBranch, Expr,
    ExternDecl, ExternFnDecl, FieldDecl, ImplDecl, MatchArm, MatchBody, Param, Program, SelectArm,
    Stmt, TypeBody, TypeDecl, TypeExpr, Variant, VariantFields,
};
use crate::mvl::parser::lexer::Span;
use crate::mvl::passes::mono::MonoProgram;

use super::{
    TirActorDecl, TirActorMethod, TirBlock, TirConstDecl, TirElseBranch, TirExpr, TirExprKind,
    TirExternDecl, TirExternFn, TirFieldDecl, TirFn, TirImplDecl, TirMatchArm, TirMatchBody,
    TirParam, TirProgram, TirSelectArm, TirStmt, TirTypeBody, TirTypeDecl, TirVariant,
    TirVariantFields,
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Lower a parsed [`Program`] + [`MonoProgram`] to a [`TirProgram`].
///
/// - Functions come from `mono` (monomorphized, one copy per generic instantiation).
/// - All other declarations (types, externs, actors, impls, consts) come from `prog`.
/// - `expr_types` is [`CheckResult::expr_types`] — the `Span → Ty` map from the checker.
pub fn lower(prog: &Program, _mono: &MonoProgram, expr_types: &HashMap<Span, Ty>) -> TirProgram {
    let empty_subs: HashMap<String, Ty> = HashMap::new();

    // Lower functions directly from AST, preserving generics.
    // The Rust backend relies on native Rust generics rather than monomorphization
    // (ADR-0034). MonoProgram is computed for pipeline parity with LLVM but not
    // applied to emission here.
    let mut fns = Vec::new();
    let mut types = Vec::new();
    let mut externs = Vec::new();
    let mut actors = Vec::new();
    let mut impls = Vec::new();
    let mut consts = Vec::new();
    let mut uses = Vec::new();
    let mut effect_decls = Vec::new();
    let mut label_decls = Vec::new();
    let mut relabel_decls = Vec::new();

    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => fns.push(lower_fn_decl(fd, expr_types, &empty_subs)),
            Decl::Type(td) => types.push(lower_type_decl(td)),
            Decl::Extern(ed) => externs.push(lower_extern_decl(ed, &empty_subs)),
            Decl::Actor(ad) => actors.push(lower_actor_decl(ad, expr_types, &empty_subs)),
            Decl::Impl(id) => impls.push(lower_impl_decl(id, expr_types, &empty_subs)),
            Decl::Const(cd) => consts.push(lower_const_decl(cd, expr_types, &empty_subs)),
            Decl::Use(ud) => uses.push(ud.clone()),
            Decl::EffectDecl(ed) => effect_decls.push(ed.clone()),
            Decl::Label(ld) => label_decls.push(ld.clone()),
            Decl::Relabel(rd) => relabel_decls.push(rd.clone()),
        }
    }

    TirProgram {
        fns,
        types,
        externs,
        actors,
        impls,
        consts,
        uses,
        effect_decls,
        label_decls,
        relabel_decls,
    }
}

// ── Function lowering ─────────────────────────────────────────────────────────

/// Lower an AST `FnDecl` directly to `TirFn`, preserving generics.
fn lower_fn_decl(
    fd: &crate::mvl::parser::ast::FnDecl,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirFn {
    let params = fd.params.iter().map(|p| lower_param(p, ty_subs)).collect();
    let ret_ty = typeexpr_to_ty_sub(&fd.return_type, ty_subs);
    let body = lower_block(&fd.body, expr_types, ty_subs);

    let requires = fd
        .requires
        .iter()
        .filter_map(|e| expr_to_ref_expr_ext(e, fd.span))
        .collect();
    let ensures = fd
        .ensures
        .iter()
        .filter_map(|e| expr_to_ref_expr_ext(e, fd.span))
        .collect();

    TirFn {
        name: fd.name.clone(),
        original_name: fd.name.clone(),
        visible: fd.visible,
        is_test: fd.is_test,
        is_builtin: fd.is_builtin,
        receiver_type: fd.receiver_type.clone(),
        type_params: fd.type_params.clone(),
        constraints: fd.constraints.clone(),
        totality: fd.totality.clone(),
        params,
        ret_ty,
        return_refinement: fd.return_refinement.clone(),
        effects: fd.effects.clone(),
        requires,
        ensures,
        body,
        span: fd.span,
    }
}

fn lower_param(p: &Param, ty_subs: &HashMap<String, Ty>) -> TirParam {
    TirParam {
        name: p.name.clone(),
        ty: typeexpr_to_ty_sub(&p.ty, ty_subs),
        capability: p.capability.clone(),
        span: p.span,
    }
}

// ── Block / statement lowering ────────────────────────────────────────────────

fn lower_block(
    block: &Block,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirBlock {
    let stmts = block
        .stmts
        .iter()
        .map(|s| lower_stmt(s, expr_types, ty_subs))
        .collect();
    TirBlock {
        stmts,
        span: block.span,
    }
}

fn lower_stmt(
    stmt: &Stmt,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirStmt {
    match stmt {
        Stmt::Let {
            kind,
            pattern,
            ty,
            init,
            span,
        } => TirStmt::Let {
            kind: kind.clone(),
            pattern: pattern.clone(),
            ty: typeexpr_to_ty_sub(ty, ty_subs),
            init: lower_expr(init, expr_types, ty_subs),
            span: *span,
        },
        Stmt::Assign {
            target,
            value,
            span,
        } => TirStmt::Assign {
            target: target.clone(),
            value: lower_expr(value, expr_types, ty_subs),
            span: *span,
        },
        Stmt::Return { value, span } => TirStmt::Return {
            value: value.as_ref().map(|e| lower_expr(e, expr_types, ty_subs)),
            span: *span,
        },
        Stmt::If {
            cond,
            then,
            else_,
            span,
        } => TirStmt::If {
            cond: lower_expr(cond, expr_types, ty_subs),
            then: lower_block(then, expr_types, ty_subs),
            else_: else_.as_ref().map(|b| lower_else(b, expr_types, ty_subs)),
            span: *span,
        },
        Stmt::Match {
            scrutinee,
            arms,
            span,
        } => TirStmt::Match {
            scrutinee: lower_expr(scrutinee, expr_types, ty_subs),
            arms: lower_match_arms(arms, expr_types, ty_subs),
            span: *span,
        },
        Stmt::For {
            pattern,
            iter,
            invariants,
            body,
            span,
        } => TirStmt::For {
            pattern: pattern.clone(),
            iter: lower_expr(iter, expr_types, ty_subs),
            invariants: lower_exprs(invariants, expr_types, ty_subs),
            body: lower_block(body, expr_types, ty_subs),
            span: *span,
        },
        Stmt::While {
            cond,
            invariants,
            decreases,
            body,
            span,
        } => TirStmt::While {
            cond: lower_expr(cond, expr_types, ty_subs),
            invariants: lower_exprs(invariants, expr_types, ty_subs),
            decreases: decreases
                .as_ref()
                .map(|d| Box::new(lower_expr(d, expr_types, ty_subs))),
            body: lower_block(body, expr_types, ty_subs),
            span: *span,
        },
        Stmt::Expr { expr, span } => TirStmt::Expr {
            expr: lower_expr(expr, expr_types, ty_subs),
            span: *span,
        },
    }
}

fn lower_else(
    branch: &ElseBranch,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirElseBranch {
    match branch {
        ElseBranch::Block(b) => TirElseBranch::Block(lower_block(b, expr_types, ty_subs)),
        ElseBranch::If(s) => TirElseBranch::If(Box::new(lower_stmt(s, expr_types, ty_subs))),
    }
}

fn lower_match_arms(
    arms: &[MatchArm],
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> Vec<TirMatchArm> {
    arms.iter()
        .map(|arm| TirMatchArm {
            pattern: arm.pattern.clone(),
            guard: arm.guard.clone(),
            body: match &arm.body {
                MatchBody::Expr(e) => TirMatchBody::Expr(lower_expr(e, expr_types, ty_subs)),
                MatchBody::Block(b) => TirMatchBody::Block(lower_block(b, expr_types, ty_subs)),
            },
            span: arm.span,
        })
        .collect()
}

// ── Expression lowering ───────────────────────────────────────────────────────

fn lower_expr(
    expr: &Expr,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirExpr {
    let span = expr.span();
    // Known legitimate miss cases that fall back to Ty::Unknown:
    //   1. Expr::Quantifier — the checker never calls infer_expr on quantifiers
    //      (spec-only, unreachable in inference position).
    //   2. Session-typed parameter annotations resolved via typeexpr_to_ty in lower_param
    //      hit Ty::Unknown there; body expressions with session types ARE in expr_types.
    // Any other miss is unexpected and should be investigated.
    debug_assert!(
        expr_types.contains_key(&span) || matches!(expr, Expr::Quantifier(..)),
        "TIR lower: no type for span {span:?} in {expr:?}"
    );
    let raw_ty = expr_types.get(&span).cloned().unwrap_or(Ty::Unknown);
    let ty = substitute_ty(&raw_ty, ty_subs);

    let kind = match expr {
        Expr::Literal(lit, _) => TirExprKind::Literal(lit.clone()),

        Expr::Ident(name, _) => TirExprKind::Var(name.clone()),

        Expr::FieldAccess { expr, field, .. } => TirExprKind::FieldAccess {
            expr: Box::new(lower_expr(expr, expr_types, ty_subs)),
            field: field.clone(),
        },

        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => TirExprKind::MethodCall {
            receiver: Box::new(lower_expr(receiver, expr_types, ty_subs)),
            method: method.clone(),
            args: lower_exprs(args, expr_types, ty_subs),
        },

        Expr::FnCall {
            name,
            args,
            type_args,
            ..
        } => TirExprKind::FnCall {
            name: name.clone(),
            args: lower_exprs(args, expr_types, ty_subs),
            type_args: type_args.clone(),
        },

        Expr::Unary { op, expr, .. } => TirExprKind::Unary {
            op: *op,
            expr: Box::new(lower_expr(expr, expr_types, ty_subs)),
        },

        Expr::Binary {
            op, left, right, ..
        } => TirExprKind::Binary {
            op: *op,
            left: Box::new(lower_expr(left, expr_types, ty_subs)),
            right: Box::new(lower_expr(right, expr_types, ty_subs)),
        },

        Expr::If {
            cond, then, else_, ..
        } => TirExprKind::If {
            cond: Box::new(lower_expr(cond, expr_types, ty_subs)),
            then: lower_block(then, expr_types, ty_subs),
            else_: else_
                .as_ref()
                .map(|e| Box::new(lower_expr(e, expr_types, ty_subs))),
        },

        Expr::Match {
            scrutinee, arms, ..
        } => TirExprKind::Match {
            scrutinee: Box::new(lower_expr(scrutinee, expr_types, ty_subs)),
            arms: lower_match_arms(arms, expr_types, ty_subs),
        },

        Expr::Lambda { params, body, .. } => TirExprKind::Lambda {
            params: params.iter().map(|p| lower_param(p, ty_subs)).collect(),
            body: Box::new(lower_expr(body, expr_types, ty_subs)),
        },

        Expr::Block(block) => TirExprKind::Block(lower_block(block, expr_types, ty_subs)),

        Expr::Propagate { expr, .. } => {
            TirExprKind::Propagate(Box::new(lower_expr(expr, expr_types, ty_subs)))
        }

        Expr::Construct { name, fields, .. } => TirExprKind::Construct {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(f, e)| (f.clone(), lower_expr(e, expr_types, ty_subs)))
                .collect(),
        },

        Expr::List { elems, .. } => TirExprKind::List {
            elems: lower_exprs(elems, expr_types, ty_subs),
        },

        Expr::Map { pairs, .. } => TirExprKind::Map {
            pairs: pairs
                .iter()
                .map(|(k, v)| {
                    (
                        lower_expr(k, expr_types, ty_subs),
                        lower_expr(v, expr_types, ty_subs),
                    )
                })
                .collect(),
        },

        Expr::Set { elems, .. } => TirExprKind::Set {
            elems: lower_exprs(elems, expr_types, ty_subs),
        },

        Expr::Consume { expr, .. } => {
            TirExprKind::Consume(Box::new(lower_expr(expr, expr_types, ty_subs)))
        }

        Expr::Relabel {
            name,
            expr,
            tag,
            audit,
            ..
        } => TirExprKind::Relabel {
            name: name.clone(),
            expr: Box::new(lower_expr(expr, expr_types, ty_subs)),
            tag: tag.clone(),
            audit: *audit,
        },

        Expr::Borrow { mutable, expr, .. } => TirExprKind::Borrow {
            mutable: *mutable,
            expr: Box::new(lower_expr(expr, expr_types, ty_subs)),
        },

        Expr::Spawn {
            actor_type, fields, ..
        } => TirExprKind::Spawn {
            actor_type: actor_type.clone(),
            fields: fields
                .iter()
                .map(|(f, e)| (f.clone(), lower_expr(e, expr_types, ty_subs)))
                .collect(),
        },

        Expr::Select { arms, .. } => TirExprKind::Select {
            arms: arms
                .iter()
                .map(|arm| lower_select_arm(arm, expr_types, ty_subs))
                .collect(),
        },

        // `as` cast is transparent at runtime — the inner expression has the same
        // representation as the target refined type (#1324).
        Expr::As { expr, .. } => return lower_expr(expr, expr_types, ty_subs),

        Expr::Quantifier(ref_expr, _) => TirExprKind::Quantifier(ref_expr.clone()),
    };

    TirExpr { kind, ty, span }
}

fn lower_exprs(
    exprs: &[Expr],
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> Vec<TirExpr> {
    exprs
        .iter()
        .map(|e| lower_expr(e, expr_types, ty_subs))
        .collect()
}

fn lower_select_arm(
    arm: &SelectArm,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirSelectArm {
    TirSelectArm {
        binding: arm.binding.clone(),
        expr: Box::new(lower_expr(&arm.expr, expr_types, ty_subs)),
        is_timeout: arm.is_timeout,
        body: lower_block(&arm.body, expr_types, ty_subs),
        span: arm.span,
    }
}

// ── Declaration lowering ──────────────────────────────────────────────────────

fn lower_field_decl(f: &FieldDecl, ty_subs: &HashMap<String, Ty>) -> TirFieldDecl {
    TirFieldDecl {
        name: f.name.clone(),
        ty: typeexpr_to_ty_sub(&f.ty, ty_subs),
        refinement: f.refinement.clone(),
        span: f.span,
    }
}

fn lower_variant(v: &Variant, ty_subs: &HashMap<String, Ty>) -> TirVariant {
    let fields = match &v.fields {
        VariantFields::Unit => TirVariantFields::Unit,
        VariantFields::Tuple(types) => TirVariantFields::Tuple(
            types
                .iter()
                .map(|t| typeexpr_to_ty_sub(t, ty_subs))
                .collect(),
        ),
        VariantFields::Struct(fields) => TirVariantFields::Struct(
            fields
                .iter()
                .map(|f| lower_field_decl(f, ty_subs))
                .collect(),
        ),
    };
    TirVariant {
        name: v.name.clone(),
        fields,
        span: v.span,
    }
}

fn lower_type_decl(td: &TypeDecl) -> TirTypeDecl {
    let empty_subs: HashMap<String, Ty> = HashMap::new();
    let body = match &td.body {
        TypeBody::Struct { fields, invariant } => TirTypeBody::Struct {
            fields: fields
                .iter()
                .map(|f| lower_field_decl(f, &empty_subs))
                .collect(),
            invariant: invariant.clone(),
        },
        TypeBody::Enum(variants) => TirTypeBody::Enum(
            variants
                .iter()
                .map(|v| lower_variant(v, &empty_subs))
                .collect(),
        ),
        TypeBody::Alias(te) => TirTypeBody::Alias(typeexpr_to_ty(te)),
    };
    TirTypeDecl {
        visible: td.visible,
        name: td.name.clone(),
        params: td.params.clone(),
        body,
        span: td.span,
    }
}

fn lower_extern_fn(ef: &ExternFnDecl, ty_subs: &HashMap<String, Ty>) -> TirExternFn {
    TirExternFn {
        name: ef.name.clone(),
        params: ef.params.iter().map(|p| lower_param(p, ty_subs)).collect(),
        ret_ty: typeexpr_to_ty_sub(&ef.return_type, ty_subs),
        effects: ef.effects.clone(),
        totality: ef.totality.clone(),
        span: ef.span,
    }
}

fn lower_extern_decl(ed: &ExternDecl, ty_subs: &HashMap<String, Ty>) -> TirExternDecl {
    TirExternDecl {
        abi: ed.abi.clone(),
        fns: ed.fns.iter().map(|f| lower_extern_fn(f, ty_subs)).collect(),
        span: ed.span,
    }
}

fn lower_actor_method(
    m: &ActorMethod,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirActorMethod {
    TirActorMethod {
        is_public: m.is_public,
        name: m.name.clone(),
        params: m.params.iter().map(|p| lower_param(p, ty_subs)).collect(),
        ret_ty: typeexpr_to_ty_sub(&m.return_type, ty_subs),
        effects: m.effects.clone(),
        body: lower_block(&m.body, expr_types, ty_subs),
        span: m.span,
    }
}

fn lower_actor_decl(
    ad: &ActorDecl,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirActorDecl {
    TirActorDecl {
        visible: ad.visible,
        name: ad.name.clone(),
        type_params: ad.type_params.clone(),
        fields: ad
            .fields
            .iter()
            .map(|f| lower_field_decl(f, ty_subs))
            .collect(),
        methods: ad
            .methods
            .iter()
            .map(|m| lower_actor_method(m, expr_types, ty_subs))
            .collect(),
        mailbox: ad.mailbox.clone(),
        traps_exit: ad.traps_exit,
        span: ad.span,
    }
}

fn lower_impl_method(
    fd: &crate::mvl::parser::ast::FnDecl,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirFn {
    TirFn {
        name: fd.name.clone(),
        original_name: fd.name.clone(),
        visible: fd.visible,
        is_test: fd.is_test,
        is_builtin: fd.is_builtin,
        receiver_type: fd.receiver_type.clone(),
        type_params: fd.type_params.clone(),
        constraints: fd.constraints.clone(),
        totality: fd.totality.clone(),
        params: fd.params.iter().map(|p| lower_param(p, ty_subs)).collect(),
        ret_ty: typeexpr_to_ty_sub(&fd.return_type, ty_subs),
        return_refinement: None, // impl methods don't have return refinement
        effects: fd.effects.clone(),
        requires: Vec::new(), // impl methods don't have requires clauses here
        ensures: Vec::new(),  // impl methods don't have ensures clauses here
        body: lower_block(&fd.body, expr_types, ty_subs),
        span: fd.span,
    }
}

fn lower_impl_decl(
    id: &ImplDecl,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirImplDecl {
    TirImplDecl {
        trait_name: id.trait_name.clone(),
        trait_type_args: id
            .trait_type_args
            .iter()
            .map(|t| typeexpr_to_ty_sub(t, ty_subs))
            .collect(),
        type_name: id.type_name.clone(),
        methods: id
            .methods
            .iter()
            .map(|m| lower_impl_method(m, expr_types, ty_subs))
            .collect(),
        span: id.span,
    }
}

fn lower_const_decl(
    cd: &ConstDecl,
    expr_types: &HashMap<Span, Ty>,
    ty_subs: &HashMap<String, Ty>,
) -> TirConstDecl {
    TirConstDecl {
        visible: cd.visible,
        name: cd.name.clone(),
        ty: typeexpr_to_ty_sub(&cd.ty, ty_subs),
        value: lower_expr(&cd.value, expr_types, ty_subs),
        span: cd.span,
    }
}

// ── Type utilities ────────────────────────────────────────────────────────────

/// Convert a `TypeExpr` to `Ty`, then apply `ty_subs` to substitute type parameters.
fn typeexpr_to_ty_sub(te: &TypeExpr, ty_subs: &HashMap<String, Ty>) -> Ty {
    substitute_ty(&typeexpr_to_ty(te), ty_subs)
}

/// Shallow conversion from syntactic [`TypeExpr`] to semantic [`Ty`].
///
/// Named type parameters (e.g. `Base { name: "T", args: [] }`) become
/// `Ty::Named("T", [])` — call [`substitute_ty`] afterwards to resolve them.
fn typeexpr_to_ty(te: &TypeExpr) -> Ty {
    match te {
        TypeExpr::Base { name, args, .. } => match name.as_str() {
            "Int" if args.is_empty() => Ty::Int,
            "Float" if args.is_empty() => Ty::Float,
            "String" if args.is_empty() => Ty::String,
            "Bool" if args.is_empty() => Ty::Bool,
            "Char" if args.is_empty() => Ty::Char,
            "Byte" if args.is_empty() => Ty::Byte,
            "UByte" if args.is_empty() => Ty::UByte,
            "UInt" if args.is_empty() => Ty::UInt,
            "Unit" if args.is_empty() => Ty::Unit,
            "Never" if args.is_empty() => Ty::Never,
            "List" if args.len() == 1 => Ty::List(Box::new(typeexpr_to_ty(&args[0]))),
            "Set" if args.len() == 1 => Ty::Set(Box::new(typeexpr_to_ty(&args[0]))),
            "Map" if args.len() == 2 => Ty::Map(
                Box::new(typeexpr_to_ty(&args[0])),
                Box::new(typeexpr_to_ty(&args[1])),
            ),
            "Array" if args.len() == 2 => {
                let inner = typeexpr_to_ty(&args[0]);
                let size = match &args[1] {
                    TypeExpr::IntConst { value, .. } => *value as u64,
                    _ => crate::mvl::checker::types::ARRAY_SIZE_UNKNOWN,
                };
                Ty::Array(Box::new(inner), size)
            }
            _ => Ty::Named(name.clone(), args.iter().map(typeexpr_to_ty).collect()),
        },
        TypeExpr::Option { inner, .. } => Ty::Option(Box::new(typeexpr_to_ty(inner))),
        TypeExpr::Result { ok, err, .. } => {
            Ty::Result(Box::new(typeexpr_to_ty(ok)), Box::new(typeexpr_to_ty(err)))
        }
        TypeExpr::Ref { mutable, inner, .. } => Ty::Ref(*mutable, Box::new(typeexpr_to_ty(inner))),
        TypeExpr::Labeled { label, inner, .. } => {
            Ty::Labeled(label.clone(), Box::new(typeexpr_to_ty(inner)))
        }
        TypeExpr::Refined { inner, pred, .. } => {
            Ty::Refined(Box::new(typeexpr_to_ty(inner)), Box::new(pred.clone()))
        }
        TypeExpr::Fn {
            params,
            ret,
            effects,
            ..
        } => Ty::Fn(
            params.iter().map(typeexpr_to_ty).collect(),
            Box::new(typeexpr_to_ty(ret)),
            effects.clone(),
            None,
        ),
        TypeExpr::Tuple { elems, .. } => Ty::Tuple(elems.iter().map(typeexpr_to_ty).collect()),
        // Session types: convert the AST SessionOp tree to a checker SessionTy.
        // Note: this path is taken for session-typed *parameter annotations* (via lower_param).
        // Session-typed body *expressions* always carry a resolved Ty::Session in expr_types.
        TypeExpr::Session { op, .. } => Ty::Session(Box::new(resolve_session_op(op))),
        // IntConst only appears as args[1] in Array[T, N] and is consumed by the Array arm
        // above. A standalone IntConst here is unexpected — return Unknown to match the
        // checker's behaviour (checker/types.rs maps standalone IntConst to Ty::Unknown).
        TypeExpr::IntConst { .. } => Ty::Unknown,
    }
}

/// Recursively substitute type-parameter placeholders in a [`Ty`].
///
/// `subs` maps type-parameter names to their concrete [`Ty`] (e.g. `"T" → Ty::Int`).
/// `Ty::Named(name, [])` is treated as a type parameter reference when `name` is in `subs`.
fn substitute_ty(ty: &Ty, subs: &HashMap<String, Ty>) -> Ty {
    if subs.is_empty() {
        return ty.clone();
    }
    match ty {
        // Bare (no-arg) Named types are type-parameter references — substitute if in subs.
        // MVL does not currently support higher-kinded type parameters, so a Named type
        // with non-empty args is always a concrete generic (e.g. `Result[T, E]`), not a
        // type-param application; we only recurse into its args, never substitute the name.
        Ty::Named(name, args) if args.is_empty() => {
            subs.get(name).cloned().unwrap_or_else(|| ty.clone())
        }
        Ty::Named(name, args) => Ty::Named(
            name.clone(),
            args.iter().map(|a| substitute_ty(a, subs)).collect(),
        ),
        Ty::List(inner) => Ty::List(Box::new(substitute_ty(inner, subs))),
        Ty::Set(inner) => Ty::Set(Box::new(substitute_ty(inner, subs))),
        Ty::Map(k, v) => Ty::Map(
            Box::new(substitute_ty(k, subs)),
            Box::new(substitute_ty(v, subs)),
        ),
        Ty::Array(inner, size) => Ty::Array(Box::new(substitute_ty(inner, subs)), *size),
        Ty::Option(inner) => Ty::Option(Box::new(substitute_ty(inner, subs))),
        Ty::Result(ok, err) => Ty::Result(
            Box::new(substitute_ty(ok, subs)),
            Box::new(substitute_ty(err, subs)),
        ),
        Ty::Ref(mutable, inner) => Ty::Ref(*mutable, Box::new(substitute_ty(inner, subs))),
        Ty::Fn(params, ret, effects, totality) => Ty::Fn(
            params.iter().map(|p| substitute_ty(p, subs)).collect(),
            Box::new(substitute_ty(ret, subs)),
            effects.clone(),
            totality.clone(),
        ),
        Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(|e| substitute_ty(e, subs)).collect()),
        Ty::Labeled(label, inner) => {
            Ty::Labeled(label.clone(), Box::new(substitute_ty(inner, subs)))
        }
        Ty::Refined(inner, pred) => Ty::Refined(Box::new(substitute_ty(inner, subs)), pred.clone()),
        // Primitives and terminal types — no substitution needed.
        Ty::Int
        | Ty::Float
        | Ty::String
        | Ty::Bool
        | Ty::Char
        | Ty::Byte
        | Ty::UByte
        | Ty::UInt
        | Ty::Unit
        | Ty::Never
        | Ty::Unknown
        | Ty::Session(_) => ty.clone(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::checker::types::Ty;
    use crate::mvl::parser::ast::TypeExpr;
    use crate::mvl::parser::lexer::Span;

    fn sp() -> Span {
        Span::default()
    }

    fn base_te(name: &str) -> TypeExpr {
        TypeExpr::Base {
            name: name.into(),
            args: vec![],
            span: sp(),
        }
    }

    // ── typeexpr_to_ty ────────────────────────────────────────────────────────

    #[test]
    fn converts_primitives() {
        assert_eq!(typeexpr_to_ty(&base_te("Int")), Ty::Int);
        assert_eq!(typeexpr_to_ty(&base_te("Bool")), Ty::Bool);
        assert_eq!(typeexpr_to_ty(&base_te("String")), Ty::String);
        assert_eq!(typeexpr_to_ty(&base_te("Unit")), Ty::Unit);
        assert_eq!(typeexpr_to_ty(&base_te("Float")), Ty::Float);
    }

    #[test]
    fn converts_list() {
        let te = TypeExpr::Base {
            name: "List".into(),
            args: vec![base_te("Int")],
            span: sp(),
        };
        assert_eq!(typeexpr_to_ty(&te), Ty::List(Box::new(Ty::Int)));
    }

    #[test]
    fn converts_map() {
        let te = TypeExpr::Base {
            name: "Map".into(),
            args: vec![base_te("String"), base_te("Int")],
            span: sp(),
        };
        assert_eq!(
            typeexpr_to_ty(&te),
            Ty::Map(Box::new(Ty::String), Box::new(Ty::Int))
        );
    }

    #[test]
    fn converts_option() {
        let te = TypeExpr::Option {
            inner: Box::new(base_te("Bool")),
            span: sp(),
        };
        assert_eq!(typeexpr_to_ty(&te), Ty::Option(Box::new(Ty::Bool)));
    }

    #[test]
    fn unknown_name_becomes_named() {
        let te = base_te("T");
        assert_eq!(typeexpr_to_ty(&te), Ty::Named("T".into(), vec![]));
    }

    // ── substitute_ty ─────────────────────────────────────────────────────────

    #[test]
    fn substitutes_type_param() {
        let mut subs = HashMap::new();
        subs.insert("T".into(), Ty::Int);
        assert_eq!(
            substitute_ty(&Ty::Named("T".into(), vec![]), &subs),
            Ty::Int
        );
    }

    #[test]
    fn substitutes_inside_list() {
        let mut subs = HashMap::new();
        subs.insert("T".into(), Ty::Bool);
        let ty = Ty::List(Box::new(Ty::Named("T".into(), vec![])));
        assert_eq!(substitute_ty(&ty, &subs), Ty::List(Box::new(Ty::Bool)));
    }

    #[test]
    fn substitutes_inside_option() {
        let mut subs = HashMap::new();
        subs.insert("T".into(), Ty::String);
        let ty = Ty::Option(Box::new(Ty::Named("T".into(), vec![])));
        assert_eq!(substitute_ty(&ty, &subs), Ty::Option(Box::new(Ty::String)));
    }

    #[test]
    fn non_param_named_unchanged() {
        let subs = HashMap::new();
        let ty = Ty::Named("MyStruct".into(), vec![]);
        assert_eq!(substitute_ty(&ty, &subs), ty);
    }

    #[test]
    fn empty_subs_returns_clone() {
        let subs = HashMap::new();
        assert_eq!(substitute_ty(&Ty::Int, &subs), Ty::Int);
        assert_eq!(
            substitute_ty(&Ty::List(Box::new(Ty::String)), &subs),
            Ty::List(Box::new(Ty::String))
        );
    }

    // ── lower (integration) ───────────────────────────────────────────────────

    fn parse_and_check(
        src: &str,
    ) -> (
        crate::mvl::parser::ast::Program,
        crate::mvl::checker::CheckResult,
    ) {
        let (mut p, _) = crate::mvl::parser::Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        let check = crate::mvl::checker::check(&prog);
        (prog, check)
    }

    #[test]
    fn lower_simple_function() {
        let (prog, check) = parse_and_check(
            r#"
fn add(x: Int, y: Int) -> Int { x + y }
fn main() -> Unit { let r: Int = add(1, 2); }
"#,
        );
        let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &check.expr_types);
        let tir = lower(&prog, &mono, &check.expr_types);

        assert!(!tir.fns.is_empty());
        let add_fn = tir
            .fns
            .iter()
            .find(|f| f.name == "add")
            .expect("add must be in TIR");
        assert_eq!(add_fn.ret_ty, Ty::Int);
        assert_eq!(add_fn.params.len(), 2);
        assert_eq!(add_fn.params[0].ty, Ty::Int);
    }

    #[test]
    fn lower_generic_function_preserves_generics() {
        let (prog, check) = parse_and_check(
            r#"
fn identity[T](x: T) -> T { x }
fn main() -> Unit {
    let n: Int    = identity(42);
    let s: String = identity("hello");
}
"#,
        );
        let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &check.expr_types);
        let tir = lower(&prog, &mono, &check.expr_types);

        // Rust backend preserves generics — one generic function, not monomorphized copies.
        let identity = tir
            .fns
            .iter()
            .find(|f| f.name == "identity")
            .expect("identity must be in TIR");
        assert_eq!(identity.type_params.len(), 1);
        assert_eq!(identity.name, "identity");
        assert_eq!(identity.original_name, "identity");
    }

    // ── typeexpr_to_ty — additional variant coverage ───────────────────────────

    #[test]
    fn converts_set() {
        let te = TypeExpr::Base {
            name: "Set".into(),
            args: vec![base_te("Int")],
            span: sp(),
        };
        assert_eq!(typeexpr_to_ty(&te), Ty::Set(Box::new(Ty::Int)));
    }

    #[test]
    fn converts_result() {
        let te = TypeExpr::Result {
            ok: Box::new(base_te("Int")),
            err: Box::new(base_te("String")),
            span: sp(),
        };
        assert_eq!(
            typeexpr_to_ty(&te),
            Ty::Result(Box::new(Ty::Int), Box::new(Ty::String))
        );
    }

    #[test]
    fn converts_ref_immutable() {
        let te = TypeExpr::Ref {
            mutable: false,
            inner: Box::new(base_te("Int")),
            span: sp(),
        };
        assert_eq!(typeexpr_to_ty(&te), Ty::Ref(false, Box::new(Ty::Int)));
    }

    #[test]
    fn converts_ref_mutable() {
        let te = TypeExpr::Ref {
            mutable: true,
            inner: Box::new(base_te("Bool")),
            span: sp(),
        };
        assert_eq!(typeexpr_to_ty(&te), Ty::Ref(true, Box::new(Ty::Bool)));
    }

    #[test]
    fn converts_labeled() {
        let te = TypeExpr::Labeled {
            label: "Tainted".into(),
            inner: Box::new(base_te("String")),
            span: sp(),
        };
        assert_eq!(
            typeexpr_to_ty(&te),
            Ty::Labeled("Tainted".into(), Box::new(Ty::String))
        );
    }

    #[test]
    fn converts_fn_type() {
        let te = TypeExpr::Fn {
            params: vec![base_te("Int"), base_te("Bool")],
            ret: Box::new(base_te("String")),
            effects: vec![],
            span: sp(),
        };
        assert_eq!(
            typeexpr_to_ty(&te),
            Ty::Fn(vec![Ty::Int, Ty::Bool], Box::new(Ty::String), vec![], None)
        );
    }

    #[test]
    fn converts_tuple() {
        let te = TypeExpr::Tuple {
            elems: vec![base_te("Int"), base_te("Bool"), base_te("String")],
            span: sp(),
        };
        assert_eq!(
            typeexpr_to_ty(&te),
            Ty::Tuple(vec![Ty::Int, Ty::Bool, Ty::String])
        );
    }

    #[test]
    fn converts_array_with_concrete_size() {
        let te = TypeExpr::Base {
            name: "Array".into(),
            args: vec![
                base_te("Int"),
                TypeExpr::IntConst {
                    value: 8,
                    span: sp(),
                },
            ],
            span: sp(),
        };
        assert_eq!(typeexpr_to_ty(&te), Ty::Array(Box::new(Ty::Int), 8));
    }

    #[test]
    fn converts_array_with_unknown_size() {
        use crate::mvl::checker::types::ARRAY_SIZE_UNKNOWN;
        let te = TypeExpr::Base {
            name: "Array".into(),
            args: vec![base_te("Bool"), base_te("N")],
            span: sp(),
        };
        assert_eq!(
            typeexpr_to_ty(&te),
            Ty::Array(Box::new(Ty::Bool), ARRAY_SIZE_UNKNOWN)
        );
    }

    #[test]
    fn converts_intconst_standalone_to_unknown() {
        let te = TypeExpr::IntConst {
            value: 42,
            span: sp(),
        };
        // Standalone IntConst is a degenerate case — expect Ty::Unknown.
        assert_eq!(typeexpr_to_ty(&te), Ty::Unknown);
    }

    #[test]
    fn converts_remaining_primitives() {
        assert_eq!(typeexpr_to_ty(&base_te("Char")), Ty::Char);
        assert_eq!(typeexpr_to_ty(&base_te("Byte")), Ty::Byte);
        assert_eq!(typeexpr_to_ty(&base_te("UByte")), Ty::UByte);
        assert_eq!(typeexpr_to_ty(&base_te("UInt")), Ty::UInt);
        assert_eq!(typeexpr_to_ty(&base_te("Never")), Ty::Never);
    }

    // ── substitute_ty — compound Ty coverage ──────────────────────────────────

    #[test]
    fn substitutes_inside_map() {
        let mut subs = HashMap::new();
        subs.insert("K".into(), Ty::String);
        subs.insert("V".into(), Ty::Int);
        let ty = Ty::Map(
            Box::new(Ty::Named("K".into(), vec![])),
            Box::new(Ty::Named("V".into(), vec![])),
        );
        assert_eq!(
            substitute_ty(&ty, &subs),
            Ty::Map(Box::new(Ty::String), Box::new(Ty::Int))
        );
    }

    #[test]
    fn substitutes_inside_result() {
        let mut subs = HashMap::new();
        subs.insert("T".into(), Ty::Int);
        subs.insert("E".into(), Ty::String);
        let ty = Ty::Result(
            Box::new(Ty::Named("T".into(), vec![])),
            Box::new(Ty::Named("E".into(), vec![])),
        );
        assert_eq!(
            substitute_ty(&ty, &subs),
            Ty::Result(Box::new(Ty::Int), Box::new(Ty::String))
        );
    }

    #[test]
    fn substitutes_inside_array_preserves_size() {
        let mut subs = HashMap::new();
        subs.insert("T".into(), Ty::Bool);
        let ty = Ty::Array(Box::new(Ty::Named("T".into(), vec![])), 16);
        assert_eq!(substitute_ty(&ty, &subs), Ty::Array(Box::new(Ty::Bool), 16));
    }

    #[test]
    fn substitutes_inside_ref_preserves_mutability() {
        let mut subs = HashMap::new();
        subs.insert("T".into(), Ty::Int);
        let ty = Ty::Ref(true, Box::new(Ty::Named("T".into(), vec![])));
        assert_eq!(substitute_ty(&ty, &subs), Ty::Ref(true, Box::new(Ty::Int)));
    }

    #[test]
    fn substitutes_inside_fn_params_and_ret() {
        let mut subs = HashMap::new();
        subs.insert("A".into(), Ty::Int);
        subs.insert("B".into(), Ty::Bool);
        let ty = Ty::Fn(
            vec![Ty::Named("A".into(), vec![])],
            Box::new(Ty::Named("B".into(), vec![])),
            vec![],
            None,
        );
        assert_eq!(
            substitute_ty(&ty, &subs),
            Ty::Fn(vec![Ty::Int], Box::new(Ty::Bool), vec![], None)
        );
    }

    #[test]
    fn substitutes_inside_tuple() {
        let mut subs = HashMap::new();
        subs.insert("T".into(), Ty::String);
        let ty = Ty::Tuple(vec![Ty::Named("T".into(), vec![]), Ty::Int]);
        assert_eq!(
            substitute_ty(&ty, &subs),
            Ty::Tuple(vec![Ty::String, Ty::Int])
        );
    }

    #[test]
    fn substitutes_inside_labeled() {
        let mut subs = HashMap::new();
        subs.insert("T".into(), Ty::Int);
        let ty = Ty::Labeled("Secret".into(), Box::new(Ty::Named("T".into(), vec![])));
        assert_eq!(
            substitute_ty(&ty, &subs),
            Ty::Labeled("Secret".into(), Box::new(Ty::Int))
        );
    }

    #[test]
    fn substitutes_inside_named_with_args() {
        let mut subs = HashMap::new();
        subs.insert("T".into(), Ty::Int);
        subs.insert("E".into(), Ty::String);
        // Ty::Named("Either", [Named("T",[]), Named("E",[])])
        let ty = Ty::Named(
            "Either".into(),
            vec![Ty::Named("T".into(), vec![]), Ty::Named("E".into(), vec![])],
        );
        assert_eq!(
            substitute_ty(&ty, &subs),
            Ty::Named("Either".into(), vec![Ty::Int, Ty::String])
        );
    }

    #[test]
    fn substitutes_nested_two_levels_deep() {
        let mut subs = HashMap::new();
        subs.insert("T".into(), Ty::Int);
        // List[Option[T]]
        let ty = Ty::List(Box::new(Ty::Option(Box::new(Ty::Named(
            "T".into(),
            vec![],
        )))));
        assert_eq!(
            substitute_ty(&ty, &subs),
            Ty::List(Box::new(Ty::Option(Box::new(Ty::Int))))
        );
    }

    // ── span_types integration ────────────────────────────────────────────────

    #[test]
    fn span_types_empty_program() {
        use crate::mvl::ir::TirProgram;
        let prog = TirProgram::default();
        assert!(prog.span_types().is_empty());
    }

    #[test]
    fn span_types_contains_body_expressions() {
        let (prog, check) = parse_and_check(
            r#"
fn add(x: Int, y: Int) -> Int { x + y }
fn main() -> Unit { let r: Int = add(1, 2); }
"#,
        );
        let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &check.expr_types);
        let tir = lower(&prog, &mono, &check.expr_types);

        let span_map = tir.span_types();
        assert!(
            !span_map.is_empty(),
            "span_types must not be empty for a non-trivial program"
        );
        // Every Ty in the map must be a concrete, non-generic type.
        for ty in span_map.values() {
            assert_ne!(
                *ty,
                Ty::Unknown,
                "span_types should not contain Unknown types"
            );
        }
    }

    #[test]
    fn tir_expr_carries_type() {
        let (prog, check) = parse_and_check(
            r#"
fn add(x: Int, y: Int) -> Int { x + y }
fn main() -> Unit { let r: Int = add(1, 2); }
"#,
        );
        let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &check.expr_types);
        let tir = lower(&prog, &mono, &check.expr_types);

        // The body of `add` is `x + y` — a Binary expr whose type should be Int.
        let add_fn = tir.fns.iter().find(|f| f.name == "add").unwrap();
        // The block has one implicit-return expr which is the Binary.
        // Block stmts should be empty; the return expression is in the trailing Expr stmt.
        let body_ty = match add_fn.body.stmts.last() {
            Some(TirStmt::Expr { expr, .. }) => expr.ty.clone(),
            _ => panic!("expected trailing Expr stmt in add body"),
        };
        assert_eq!(body_ty, Ty::Int);
    }
}
