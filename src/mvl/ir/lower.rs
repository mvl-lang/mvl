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
//! let tir = lower(&mono_program, &check_result.expr_types);
//! ```

use std::collections::HashMap;

use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{
    Block, ElseBranch, Expr, MatchArm, MatchBody, Param, SelectArm, Stmt, TypeExpr,
};
use crate::mvl::parser::lexer::Span;
use crate::mvl::passes::mono::{MonoProgram, TypeSubst};

use super::{
    TirBlock, TirElseBranch, TirExpr, TirExprKind, TirFn, TirMatchArm, TirMatchBody, TirParam,
    TirProgram, TirSelectArm, TirStmt,
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Lower a [`MonoProgram`] to a [`TirProgram`] using the checker's expression type map.
///
/// `expr_types` is [`CheckResult::expr_types`] — the `Span → Ty` map produced by the
/// type checker.  Types for generic-body expressions may still reference type parameters
/// (e.g. `Ty::Named("T", [])`); the lowering resolves these using each function's
/// type-parameter substitution.
pub fn lower(mono: &MonoProgram, expr_types: &HashMap<Span, Ty>) -> TirProgram {
    let fns = mono.fns.iter().map(|mf| lower_fn(mf, expr_types)).collect();
    TirProgram { fns }
}

// ── Function lowering ─────────────────────────────────────────────────────────

fn lower_fn(mf: &crate::mvl::passes::mono::MonoFn, expr_types: &HashMap<Span, Ty>) -> TirFn {
    let ty_subs = build_ty_subs(&mf.type_subs);

    let params = mf
        .decl
        .params
        .iter()
        .map(|p| lower_param(p, &ty_subs))
        .collect();

    let ret_ty = typeexpr_to_ty_sub(&mf.decl.return_type, &ty_subs);
    let body = lower_block(&mf.decl.body, expr_types, &ty_subs);

    TirFn {
        name: mf.mangled_name.clone(),
        original_name: mf.original_name.clone(),
        totality: mf.decl.totality.clone(),
        params,
        ret_ty,
        effects: mf.decl.effects.clone(),
        body,
        span: mf.decl.span,
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
            body,
            span,
            ..
        } => TirStmt::For {
            pattern: pattern.clone(),
            iter: lower_expr(iter, expr_types, ty_subs),
            body: lower_block(body, expr_types, ty_subs),
            span: *span,
        },
        Stmt::While {
            cond, body, span, ..
        } => TirStmt::While {
            cond: lower_expr(cond, expr_types, ty_subs),
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

        Expr::FnCall { name, args, .. } => TirExprKind::FnCall {
            name: name.clone(),
            args: lower_exprs(args, expr_types, ty_subs),
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
            name, expr, tag, ..
        } => TirExprKind::Relabel {
            name: name.clone(),
            expr: Box::new(lower_expr(expr, expr_types, ty_subs)),
            tag: tag.clone(),
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

// ── Type utilities ────────────────────────────────────────────────────────────

/// Build a `Ty`-level substitution map from a mono pass `TypeSubst` (`String → TypeExpr`).
fn build_ty_subs(type_subs: &TypeSubst) -> HashMap<String, Ty> {
    type_subs
        .iter()
        .map(|(name, te)| (name.clone(), typeexpr_to_ty(te)))
        .collect()
}

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
        // Session types: complex dual structure — fall back to Unknown for now.
        // Checker-resolved Ty::Session values come via expr_types lookups, not this path.
        TypeExpr::Session { .. } => Ty::Unknown,
        TypeExpr::IntConst { .. } => Ty::Int,
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
        let tir = lower(&mono, &check.expr_types);

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
    fn lower_generic_function_resolves_type_param() {
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
        let tir = lower(&mono, &check.expr_types);

        let int_inst = tir
            .fns
            .iter()
            .find(|f| f.name == "identity_Int")
            .expect("identity_Int must be in TIR");
        assert_eq!(int_inst.ret_ty, Ty::Int);
        assert_eq!(int_inst.params[0].ty, Ty::Int);

        let str_inst = tir
            .fns
            .iter()
            .find(|f| f.name == "identity_String")
            .expect("identity_String must be in TIR");
        assert_eq!(str_inst.ret_ty, Ty::String);
        assert_eq!(str_inst.params[0].ty, Ty::String);
    }

    #[test]
    fn lower_preserves_original_name() {
        let (prog, check) = parse_and_check(
            r#"
fn identity[T](x: T) -> T { x }
fn main() -> Unit { let n: Int = identity(1); }
"#,
        );
        let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &check.expr_types);
        let tir = lower(&mono, &check.expr_types);

        let inst = tir.fns.iter().find(|f| f.name == "identity_Int").unwrap();
        assert_eq!(inst.original_name, "identity");
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
        let tir = lower(&mono, &check.expr_types);

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
