// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Phase 2 semantic rules — structural and flow analysis.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{
    BinaryOp, Block, Decl, ElseBranch, Expr, LValue, Literal, MatchArm, MatchBody, Pattern,
    Program, Stmt, Totality, TypeBody, TypeExpr, VariantFields,
};
use std::collections::HashMap;

// ── Phase 2: Semantic rules ────────────────────────────────────────────────

/// Flag statements that are unreachable because they follow a `return` in the
/// same block.
///
/// Rule id: `unreachable-code`
///
/// Only the **first** unreachable statement in each block is flagged to avoid
/// noise from cascading violations.
pub fn unreachable_code(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.unreachable_code {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            check_block_unreachable(&f.body, out);
        }
    }
}

fn check_block_unreachable(block: &Block, out: &mut Vec<LintDiag>) {
    let mut returned = false;
    for stmt in &block.stmts {
        if returned {
            let s = stmt.span();
            out.push(LintDiag::warning(
                "unreachable-code",
                "unreachable statement after `return`",
                s.line,
                s.col,
            ));
            // Only flag the first unreachable statement per block.
            break;
        }
        match stmt {
            Stmt::Return { .. } => {
                returned = true;
            }
            Stmt::If {
                then, else_: None, ..
            } => {
                check_block_unreachable(then, out);
            }
            Stmt::If {
                then,
                else_: Some(ElseBranch::Block(eb)),
                ..
            } => {
                check_block_unreachable(then, out);
                check_block_unreachable(eb, out);
            }
            Stmt::If {
                then,
                else_: Some(ElseBranch::If(inner)),
                ..
            } => {
                check_block_unreachable(then, out);
                check_block_unreachable(
                    // The inner `if` is a Stmt — wrap it in a synthetic block
                    // by recursing into its then-branch directly.
                    &Block {
                        stmts: vec![*inner.clone()],
                        span: inner.span(),
                    },
                    out,
                );
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    if let MatchBody::Block(b) = &arm.body {
                        check_block_unreachable(b, out);
                    }
                }
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                check_block_unreachable(body, out);
            }
            Stmt::Expr {
                expr: Expr::Block(b),
                ..
            } => {
                check_block_unreachable(b, out);
            }
            _ => {}
        }
    }
}

/// Flag `match` expressions/statements with exactly one irrefutable arm.
///
/// Rule id: `redundant-match`
///
/// A single-arm `match` where the pattern is a wildcard (`_`) or a plain
/// binding (`x`) will always match — the `match` is then equivalent to a
/// `let` binding or a bare expression.  This is a common code-smell when
/// developers use `match` for destructuring a non-enum type.
pub fn redundant_match(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.redundant_match {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            check_block_redundant_match(&f.body, out);
        }
    }
}

fn check_block_redundant_match(block: &Block, out: &mut Vec<LintDiag>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                emit_if_redundant_match(scrutinee, arms, span.line, span.col, out);
                // Recurse into arm bodies
                for arm in arms {
                    if let MatchBody::Block(b) = &arm.body {
                        check_block_redundant_match(b, out);
                    }
                }
            }
            Stmt::Expr {
                expr:
                    Expr::Match {
                        scrutinee,
                        arms,
                        span,
                    },
                ..
            } => {
                emit_if_redundant_match(scrutinee, arms, span.line, span.col, out);
            }
            Stmt::If { then, else_, .. } => {
                check_block_redundant_match(then, out);
                match else_ {
                    Some(ElseBranch::Block(eb)) => check_block_redundant_match(eb, out),
                    Some(ElseBranch::If(inner)) => check_block_redundant_match(
                        &Block {
                            stmts: vec![*inner.clone()],
                            span: inner.span(),
                        },
                        out,
                    ),
                    None => {}
                }
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                check_block_redundant_match(body, out);
            }
            _ => {}
        }
    }
}

/// Emit a `redundant-match` diagnostic if `arms` has a single irrefutable arm.
fn emit_if_redundant_match(
    scrutinee: &Expr,
    arms: &[MatchArm],
    line: u32,
    col: u32,
    out: &mut Vec<LintDiag>,
) {
    if arms.len() == 1 && is_irrefutable(&arms[0].pattern) {
        out.push(LintDiag::warning(
            "redundant-match",
            format!(
                "single-arm `match` with irrefutable pattern — use `let` instead: \
                 `let {} = {}`",
                pattern_display(&arms[0].pattern),
                expr_display(scrutinee),
            ),
            line,
            col,
        ));
    }
}

/// A pattern is irrefutable if it always matches: wildcard (`_`) or plain binding.
fn is_irrefutable(pat: &Pattern) -> bool {
    matches!(pat, Pattern::Wildcard(_) | Pattern::Ident(..))
}

fn pattern_display(pat: &Pattern) -> &str {
    match pat {
        Pattern::Wildcard(_) => "_",
        Pattern::Ident(name, _) => name.as_str(),
        _ => "_",
    }
}

fn expr_display(expr: &Expr) -> &str {
    match expr {
        Expr::Ident(name, _) => name.as_str(),
        _ => "<expr>",
    }
}

/// Flag functions that declare effects but whose body contains no function
/// or method calls at all.
///
/// Rule id: `redundant-effects`
///
/// If a function body is entirely free of calls, no effect can ever be
/// triggered — any declared effects are therefore dead annotations.
///
/// Note: this is a conservative check.  A function that only calls local
/// pure helpers is not flagged even if those helpers are call-free.
pub fn redundant_effects(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.redundant_effects {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            if f.effects.is_empty() {
                continue;
            }
            if !block_has_calls(&f.body) {
                out.push(LintDiag::warning(
                    "redundant-effects",
                    format!(
                        "function `{}` declares effect(s) [{}] but contains no calls — \
                         remove the effect declaration",
                        f.name,
                        f.effects
                            .iter()
                            .map(|e| e.to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    f.span.line,
                    f.span.col,
                ));
            }
        }
    }
}

/// Warn when a function has calls in its body but no declared effects.
///
/// Rule id: `missing-annotation`
///
/// This is the inverse of `redundant-effects`: where that rule flags declared
/// effects with no calls, this rule flags calls with no declared effects.
/// The rule is **opt-in** (disabled by default) because the linter cannot
/// distinguish calls to pure MVL helpers from calls to effectful stdlib
/// functions without a full symbol table.  Enable with
/// `missing_annotations = true` in `.mvllintrc` for code that should
/// be explicit-everywhere.
///
/// `test fn` declarations are excluded — test bodies call the system under
/// test and do not need to declare effects themselves.
///
/// See: Spec 011 Req 4, ADR-0017 amendment.
pub fn missing_annotations(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.missing_annotations {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            if f.is_test || !f.effects.is_empty() {
                continue;
            }
            if block_has_calls(&f.body) {
                out.push(LintDiag::warning(
                    "missing-annotation",
                    format!(
                        "function `{}` has calls but no effect declaration \
                         — add `! Effects` or verify all calls are pure",
                        f.name,
                    ),
                    f.span.line,
                    f.span.col,
                ));
            }
        }
    }
}

/// Warn on unannotated `pub fn` — every public function must explicitly declare
/// `total` or `partial` to make the termination contract visible.
///
/// Rule id: `missing-totality`
///
/// On by default (`require_explicit_totality = true`).  The rule checks only
/// `pub fn` declarations; private helpers, test functions, and built-in
/// functions are excluded.  The diagnostic includes a suggestion based on
/// body analysis: `partial` when the body contains a `while` loop without a
/// `decreases` variant or a direct recursive self-call; `total` otherwise.
pub fn missing_totality(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.require_explicit_totality {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            if !f.visible || f.is_test || f.is_builtin || f.totality.is_some() {
                continue;
            }
            let suggestion =
                if block_has_while_no_decreases(&f.body) || block_calls_fn(&f.body, &f.name) {
                    "partial"
                } else {
                    "total"
                };
            out.push(LintDiag::warning(
                "missing-totality",
                format!(
                    "pub fn `{}` has no explicit `total` or `partial` keyword \
                     — add `{}` to document the termination contract",
                    f.name, suggestion,
                ),
                f.span.line,
                f.span.col,
            ));
        }
    }
}

/// Return `true` if `block` (or any nested block/expression) contains a
/// `while` loop that has no `decreases` variant.
fn block_has_while_no_decreases(block: &Block) -> bool {
    block.stmts.iter().any(stmt_has_while_no_decreases)
}

fn stmt_has_while_no_decreases(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::While {
            decreases, body, ..
        } => decreases.is_none() || block_has_while_no_decreases(body),
        Stmt::Let { init, .. } => expr_has_while_no_decreases(init),
        Stmt::Assign { value, .. } => expr_has_while_no_decreases(value),
        Stmt::Return { value: Some(e), .. } => expr_has_while_no_decreases(e),
        Stmt::Return { value: None, .. } => false,
        Stmt::Expr { expr, .. } => expr_has_while_no_decreases(expr),
        Stmt::If {
            cond, then, else_, ..
        } => {
            expr_has_while_no_decreases(cond)
                || block_has_while_no_decreases(then)
                || else_
                    .as_ref()
                    .map(|e| match e {
                        ElseBranch::Block(b) => block_has_while_no_decreases(b),
                        ElseBranch::If(s) => stmt_has_while_no_decreases(s),
                    })
                    .unwrap_or(false)
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            expr_has_while_no_decreases(scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_has_while_no_decreases(e),
                    MatchBody::Block(b) => block_has_while_no_decreases(b),
                })
        }
        Stmt::For { iter, body, .. } => {
            expr_has_while_no_decreases(iter) || block_has_while_no_decreases(body)
        }
    }
}

fn expr_has_while_no_decreases(expr: &Expr) -> bool {
    match expr {
        Expr::Block(b) => block_has_while_no_decreases(b),
        Expr::If {
            cond, then, else_, ..
        } => {
            expr_has_while_no_decreases(cond)
                || block_has_while_no_decreases(then)
                || else_
                    .as_ref()
                    .map(|e| expr_has_while_no_decreases(e))
                    .unwrap_or(false)
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_has_while_no_decreases(scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_has_while_no_decreases(e),
                    MatchBody::Block(b) => block_has_while_no_decreases(b),
                })
        }
        Expr::Lambda { body, .. } => expr_has_while_no_decreases(body),
        Expr::Binary { left, right, .. } => {
            expr_has_while_no_decreases(left) || expr_has_while_no_decreases(right)
        }
        Expr::Unary { expr: e, .. }
        | Expr::FieldAccess { expr: e, .. }
        | Expr::Propagate { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Relabel { expr: e, .. }
        | Expr::Borrow { expr: e, .. } => expr_has_while_no_decreases(e),
        Expr::FnCall { args, .. } => args.iter().any(expr_has_while_no_decreases),
        Expr::MethodCall { receiver, args, .. } => {
            expr_has_while_no_decreases(receiver) || args.iter().any(expr_has_while_no_decreases)
        }
        Expr::Construct { fields, .. } => {
            fields.iter().any(|(_, e)| expr_has_while_no_decreases(e))
        }
        Expr::Spawn { fields, .. } => fields.iter().any(|(_, e)| expr_has_while_no_decreases(e)),
        Expr::Select { arms, .. } => arms
            .iter()
            .any(|a| expr_has_while_no_decreases(&a.expr) || block_has_while_no_decreases(&a.body)),
        Expr::Concurrently { body, .. } => block_has_while_no_decreases(body),
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            elems.iter().any(expr_has_while_no_decreases)
        }
        Expr::Map { pairs, .. } => pairs
            .iter()
            .any(|(k, v)| expr_has_while_no_decreases(k) || expr_has_while_no_decreases(v)),
        Expr::Literal(..) | Expr::Ident(..) | Expr::Quantifier(..) => false,
    }
}

/// Return `true` if `block` (or any nested block/expression) contains a
/// direct call to the function named `name` (used for recursion detection).
fn block_calls_fn(block: &Block, name: &str) -> bool {
    block.stmts.iter().any(|s| stmt_calls_fn(s, name))
}

fn stmt_calls_fn(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::Let { init, .. } => expr_calls_fn(init, name),
        Stmt::Assign { value, .. } => expr_calls_fn(value, name),
        Stmt::Return { value: Some(e), .. } => expr_calls_fn(e, name),
        Stmt::Return { value: None, .. } => false,
        Stmt::Expr { expr, .. } => expr_calls_fn(expr, name),
        Stmt::If {
            cond, then, else_, ..
        } => {
            expr_calls_fn(cond, name)
                || block_calls_fn(then, name)
                || else_
                    .as_ref()
                    .map(|e| match e {
                        ElseBranch::Block(b) => block_calls_fn(b, name),
                        ElseBranch::If(s) => stmt_calls_fn(s, name),
                    })
                    .unwrap_or(false)
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            expr_calls_fn(scrutinee, name)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_calls_fn(e, name),
                    MatchBody::Block(b) => block_calls_fn(b, name),
                })
        }
        Stmt::For { iter, body, .. } => expr_calls_fn(iter, name) || block_calls_fn(body, name),
        Stmt::While { cond, body, .. } => expr_calls_fn(cond, name) || block_calls_fn(body, name),
    }
}

fn expr_calls_fn(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::FnCall { name: n, args, .. } => {
            n == name || args.iter().any(|a| expr_calls_fn(a, name))
        }
        Expr::MethodCall { receiver, args, .. } => {
            expr_calls_fn(receiver, name) || args.iter().any(|a| expr_calls_fn(a, name))
        }
        Expr::Binary { left, right, .. } => expr_calls_fn(left, name) || expr_calls_fn(right, name),
        Expr::Unary { expr: e, .. }
        | Expr::FieldAccess { expr: e, .. }
        | Expr::Propagate { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Relabel { expr: e, .. }
        | Expr::Borrow { expr: e, .. } => expr_calls_fn(e, name),
        Expr::Block(b) => block_calls_fn(b, name),
        Expr::If {
            cond, then, else_, ..
        } => {
            expr_calls_fn(cond, name)
                || block_calls_fn(then, name)
                || else_
                    .as_ref()
                    .map(|e| expr_calls_fn(e, name))
                    .unwrap_or(false)
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_calls_fn(scrutinee, name)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_calls_fn(e, name),
                    MatchBody::Block(b) => block_calls_fn(b, name),
                })
        }
        Expr::Lambda { body, .. } => expr_calls_fn(body, name),
        Expr::Construct { fields, .. } => fields.iter().any(|(_, e)| expr_calls_fn(e, name)),
        Expr::Spawn { fields, .. } => fields.iter().any(|(_, e)| expr_calls_fn(e, name)),
        Expr::Select { arms, .. } => arms
            .iter()
            .any(|a| expr_calls_fn(&a.expr, name) || block_calls_fn(&a.body, name)),
        Expr::Concurrently { body, .. } => block_calls_fn(body, name),
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            elems.iter().any(|e| expr_calls_fn(e, name))
        }
        Expr::Map { pairs, .. } => pairs
            .iter()
            .any(|(k, v)| expr_calls_fn(k, name) || expr_calls_fn(v, name)),
        Expr::Literal(..) | Expr::Ident(..) | Expr::Quantifier(..) => false,
    }
}

/// Return `true` if `block` (or any nested block/expression) contains at
/// least one function or method call.
fn block_has_calls(block: &Block) -> bool {
    block.stmts.iter().any(stmt_has_calls)
}

fn stmt_has_calls(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let { init, .. } => expr_has_calls(init),
        Stmt::Assign { value, .. } => expr_has_calls(value),
        Stmt::Return { value: Some(e), .. } => expr_has_calls(e),
        Stmt::Return { value: None, .. } => false,
        Stmt::Expr { expr, .. } => expr_has_calls(expr),
        Stmt::If {
            cond, then, else_, ..
        } => {
            expr_has_calls(cond)
                || block_has_calls(then)
                || else_
                    .as_ref()
                    .map(|e| match e {
                        crate::mvl::parser::ast::ElseBranch::Block(b) => block_has_calls(b),
                        crate::mvl::parser::ast::ElseBranch::If(s) => stmt_has_calls(s),
                    })
                    .unwrap_or(false)
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            expr_has_calls(scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_has_calls(e),
                    MatchBody::Block(b) => block_has_calls(b),
                })
        }
        Stmt::For { iter, body, .. } => expr_has_calls(iter) || block_has_calls(body),
        Stmt::While { cond, body, .. } => expr_has_calls(cond) || block_has_calls(body),
    }
}

fn expr_has_calls(expr: &Expr) -> bool {
    match expr {
        Expr::FnCall { .. } | Expr::MethodCall { .. } => true,
        Expr::Literal(..) | Expr::Ident(..) | Expr::Quantifier(..) => false,
        Expr::FieldAccess { expr: e, .. } => expr_has_calls(e),
        Expr::Unary { expr: e, .. } => expr_has_calls(e),
        Expr::Binary { left, right, .. } => expr_has_calls(left) || expr_has_calls(right),
        Expr::If {
            cond, then, else_, ..
        } => {
            expr_has_calls(cond)
                || block_has_calls(then)
                || else_.as_ref().map(|e| expr_has_calls(e)).unwrap_or(false)
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_has_calls(scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_has_calls(e),
                    MatchBody::Block(b) => block_has_calls(b),
                })
        }
        Expr::Block(b) => block_has_calls(b),
        Expr::Lambda { body, .. } => expr_has_calls(body),
        Expr::Propagate { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Relabel { expr: e, .. }
        | Expr::Borrow { expr: e, .. } => expr_has_calls(e),
        Expr::Construct { fields, .. } => fields.iter().any(|(_, e)| expr_has_calls(e)),
        Expr::Spawn { fields, .. } => fields.iter().any(|(_, e)| expr_has_calls(e)),
        Expr::Select { arms, .. } => arms
            .iter()
            .any(|a| expr_has_calls(&a.expr) || block_has_calls(&a.body)),
        Expr::Concurrently { body, .. } => block_has_calls(body),
        Expr::List { elems, .. } | Expr::Set { elems, .. } => elems.iter().any(expr_has_calls),
        Expr::Map { pairs, .. } => pairs
            .iter()
            .any(|(k, v)| expr_has_calls(k) || expr_has_calls(v)),
    }
}

/// Flag `Public<T>` type annotations.
///
/// Rule id: `redundant-ifc-label`
///
/// `Public` is the base level of the MVL IFC lattice — all un-annotated types
/// are implicitly public.  Writing `Public<T>` is therefore always redundant
/// and should be simplified to `T`.
pub fn redundant_ifc_labels(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.redundant_ifc_labels {
        return;
    }
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(f) => {
                for param in &f.params {
                    check_type_expr_ifc(&param.ty, out);
                }
                check_type_expr_ifc(&f.return_type, out);
            }
            Decl::Const(c) => check_type_expr_ifc(&c.ty, out),
            Decl::Type(t) => match &t.body {
                TypeBody::Struct { fields, .. } => {
                    for field in fields {
                        check_type_expr_ifc(&field.ty, out);
                    }
                }
                TypeBody::Enum(variants) => {
                    for variant in variants {
                        match &variant.fields {
                            VariantFields::Struct(fields) => {
                                for field in fields {
                                    check_type_expr_ifc(&field.ty, out);
                                }
                            }
                            VariantFields::Tuple(types) => {
                                for ty in types {
                                    check_type_expr_ifc(ty, out);
                                }
                            }
                            VariantFields::Unit => {}
                        }
                    }
                }
                TypeBody::Alias(ty) => check_type_expr_ifc(ty, out),
            },
            _ => {}
        }
    }
}

#[allow(clippy::only_used_in_recursion)]
fn check_type_expr_ifc(ty: &TypeExpr, out: &mut Vec<LintDiag>) {
    match ty {
        TypeExpr::Labeled { inner, .. } => check_type_expr_ifc(inner, out),
        TypeExpr::Option { inner, .. }
        | TypeExpr::Ref { inner, .. }
        | TypeExpr::Refined { inner, .. } => check_type_expr_ifc(inner, out),
        TypeExpr::Result { ok, err, .. } => {
            check_type_expr_ifc(ok, out);
            check_type_expr_ifc(err, out);
        }
        TypeExpr::Base { args, .. } => {
            for arg in args {
                check_type_expr_ifc(arg, out);
            }
        }
        TypeExpr::Fn { params, ret, .. } => {
            for p in params {
                check_type_expr_ifc(p, out);
            }
            check_type_expr_ifc(ret, out);
        }
        TypeExpr::Tuple { elems, .. } => {
            for e in elems {
                check_type_expr_ifc(e, out);
            }
        }
        TypeExpr::IntConst { .. } => {}
        // Session types have no IFC label — no check needed.
        TypeExpr::Session { .. } => {}
    }
}

#[allow(dead_code, clippy::only_used_in_recursion)]
fn type_expr_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Base { name, args, .. } if args.is_empty() => name.clone(),
        TypeExpr::Base { name, args, .. } => {
            format!(
                "{}<{}>",
                name,
                args.iter()
                    .map(type_expr_name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        TypeExpr::Option { inner, .. } => format!("Option<{}>", type_expr_name(inner)),
        TypeExpr::Result { ok, err, .. } => {
            format!("Result<{}, {}>", type_expr_name(ok), type_expr_name(err))
        }
        TypeExpr::Labeled { label, inner, .. } => {
            format!("{}<{}>", label, type_expr_name(inner))
        }
        _ => "<type>".to_string(),
    }
}

// ── Phase 2 (continued): for-iter-antipattern ──────────────────────────────

/// Error on the `while / .get(i) / match / None => ()` anti-pattern (#705).
///
/// Rule id: `for-iter-antipattern`
///
/// The pattern:
///
/// ```mvl
/// let i: ref Int = 0;
/// while i < xs.len() {
///     match xs.get(i) {
///         None    => (),
///         Some(x) => { ... }
///     }
///     i = i + 1
/// }
/// ```
///
/// is never correct when iterating a `List[T]`.  `for x in xs { ... }` is
/// always equivalent, safer (no off-by-one risk), and more readable.
/// The `None => ()` arm is a false branch that only satisfies exhaustiveness.
///
/// **Detection:** a `while` whose direct body contains a `match` on
/// `<expr>.get(<args>)` where any arm is `None => ()`.
///
/// **Escape hatch:** if the `None` arm contains real logic (not just `()`),
/// the rule is silent — the user is deliberately handling a missing element.
pub fn for_iter_antipattern(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.for_iter_antipattern {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            check_block_for_iter_antipattern(&f.body, out);
        }
    }
}

fn check_block_for_iter_antipattern(block: &Block, out: &mut Vec<LintDiag>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::While { body, span, .. } => {
                // Check direct children of the while body for the anti-pattern.
                for inner in &body.stmts {
                    if let Stmt::Match {
                        scrutinee,
                        arms,
                        span: match_span,
                    } = inner
                    {
                        if is_get_call(scrutinee) && has_none_unit_arm(arms) {
                            out.push(LintDiag::error(
                                "for-iter-antipattern",
                                "use `for x in list { }` for List[T] iteration; \
                                 `while/.get(i)/match/None=>()` is not allowed",
                                span.line,
                                span.col,
                            ));
                            let _ = match_span; // span reported at while header
                            break;
                        }
                    }
                }
                // Recurse to catch nested whiles.
                check_block_for_iter_antipattern(body, out);
            }
            Stmt::For { body, .. } | Stmt::If { then: body, .. } => {
                check_block_for_iter_antipattern(body, out);
            }
            _ => {}
        }
    }
}

/// Returns `true` if `expr` is a method call to `.get(...)`.
fn is_get_call(expr: &Expr) -> bool {
    matches!(expr, Expr::MethodCall { method, .. } if method == "get")
}

/// Returns `true` if any arm of the match has pattern `None` and body `()`.
fn has_none_unit_arm(arms: &[MatchArm]) -> bool {
    arms.iter().any(|arm| {
        matches!(arm.pattern, Pattern::None(_))
            && matches!(&arm.body, MatchBody::Expr(Expr::Literal(Literal::Unit, _)))
    })
}

// ── Phase 2 (continued): while-to-for-range ─────────────────────────────────

/// Warn on counter-increment while loops that can be rewritten as `for i in range()`.
///
/// Rule id: `while-to-for-range`
///
/// Detects the pattern:
/// ```mvl
/// let i: ref Int = 0;
/// while i < n {
///     // ...
///     i = i + 1
/// }
/// ```
/// and suggests: `for i in range(0, n)` which uses `range`'s `decreases` clause
/// and is therefore provably total.
///
/// **Escape hatch:** loops with an explicit `decreases` clause are already
/// annotated and are silently skipped.
pub fn while_to_for_range(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.while_to_for_range {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            check_block_for_while_range(&f.body, out);
        }
    }
}

fn check_block_for_while_range(block: &Block, out: &mut Vec<LintDiag>) {
    // Track let-bound variable initial values seen so far in this block.
    let mut let_inits: HashMap<String, String> = HashMap::new();

    for stmt in &block.stmts {
        match stmt {
            Stmt::Let {
                pattern: Pattern::Ident(name, _),
                init,
                ..
            } => {
                let_inits.insert(name.clone(), simple_expr_str(init));
            }
            Stmt::While {
                cond,
                decreases,
                body,
                span,
                ..
            } => {
                if decreases.is_none() {
                    if let Some((var, end)) = counter_lt_cond(cond) {
                        if is_counter_increment(body, &var) {
                            let start = let_inits.get(&var).map(String::as_str).unwrap_or("0");
                            out.push(LintDiag::warning(
                                "while-to-for-range",
                                format!(
                                    "`while {var} < {end}` counter loop — use \
                                     `for {var} in range({start}, {end})` for a \
                                     provably-terminating loop",
                                ),
                                span.line,
                                span.col,
                            ));
                        }
                    }
                }
                // Recurse to catch nested patterns.
                check_block_for_while_range(body, out);
            }
            Stmt::For { body, .. } | Stmt::If { then: body, .. } => {
                check_block_for_while_range(body, out);
            }
            _ => {}
        }
    }
}

/// If `expr` is `VAR < END`, return `(var_name, end_repr)`.
fn counter_lt_cond(expr: &Expr) -> Option<(String, String)> {
    if let Expr::Binary {
        op: BinaryOp::Lt,
        left,
        right,
        ..
    } = expr
    {
        if let Expr::Ident(name, _) = left.as_ref() {
            return Some((name.clone(), simple_expr_str(right)));
        }
    }
    None
}

/// Return `true` if the last statement in `block` is `var = var + N`.
fn is_counter_increment(block: &Block, var: &str) -> bool {
    match block.stmts.last() {
        Some(Stmt::Assign { target, value, .. }) => {
            if let LValue::Ident(name, _) = target {
                if name == var {
                    if let Expr::Binary {
                        op: BinaryOp::Add,
                        left,
                        ..
                    } = value
                    {
                        if let Expr::Ident(n, _) = left.as_ref() {
                            return n == var;
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// Format simple expressions for diagnostic messages.
fn simple_expr_str(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::Literal(Literal::Integer(n), _) => n.to_string(),
        _ => "_".to_string(),
    }
}

// ── Phase 2: suggest-decreases (#1037) ─────────────────────────────────────

/// Suggest a `decreases` clause for `while` loops in total functions that have
/// an obvious decrementing variable (#1037, #1029).
///
/// Rule id: `suggest-decreases`
///
/// Heuristic: if the loop body contains `VAR = VAR - N` (or `VAR = VAR + N`
/// with a `VAR > 0` / `VAR >= 1` condition), suggest `decreases VAR`.
/// Also detects method-call measures like `rest.len()` when the condition is
/// `rest.len() > 0`.
pub fn suggest_decreases(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.suggest_decreases {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            // Only fire in total functions (explicit or implicit)
            if matches!(f.totality, Some(Totality::Partial)) {
                continue;
            }
            check_block_for_decreases(&f.body, out);
        }
    }
}

fn check_block_for_decreases(block: &Block, out: &mut Vec<LintDiag>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::While {
                cond,
                decreases: None,
                body,
                span,
                ..
            } => {
                if let Some(var) = find_decrement_var(cond, body) {
                    out.push(LintDiag::hint(
                        "suggest-decreases",
                        format!(
                            "`while` loop has no termination proof — \
                             add `decreases {var}` to prove bounded iteration",
                        ),
                        span.line,
                        span.col,
                    ));
                }
                check_block_for_decreases(body, out);
            }
            Stmt::For { body, .. } => check_block_for_decreases(body, out),
            Stmt::If { then, else_, .. } => {
                check_block_for_decreases(then, out);
                if let Some(ElseBranch::Block(b)) = else_ {
                    check_block_for_decreases(b, out);
                }
            }
            _ => {}
        }
    }
}

/// Detect a decrementing variable: condition references `var` (e.g. `var > 0`)
/// and body decrements it (e.g. `var = var - 1`).
fn find_decrement_var(cond: &Expr, body: &Block) -> Option<String> {
    let cond_var = extract_cond_var(cond)?;
    // Check if body contains `cond_var = cond_var - N`
    if body_decrements(&cond_var, body) {
        return Some(cond_var);
    }
    None
}

/// Extract variable name from conditions like `var > 0`, `var >= 1`, `var != 0`.
fn extract_cond_var(expr: &Expr) -> Option<String> {
    if let Expr::Binary {
        op, left, right, ..
    } = expr
    {
        match op {
            BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Ne => {
                if let Expr::Ident(name, _) = left.as_ref() {
                    return Some(name.clone());
                }
            }
            BinaryOp::Lt | BinaryOp::Le => {
                // `0 < var` form
                if let Expr::Ident(name, _) = right.as_ref() {
                    return Some(name.clone());
                }
            }
            _ => {}
        }
    }
    None
}

/// Check if the block body contains `var = var - N` for the given variable.
fn body_decrements(var: &str, block: &Block) -> bool {
    block.stmts.iter().any(|stmt| {
        if let Stmt::Assign {
            target: LValue::Ident(name, _),
            value,
            ..
        } = stmt
        {
            if name == var {
                if let Expr::Binary {
                    op: BinaryOp::Sub,
                    left,
                    ..
                } = value
                {
                    if let Expr::Ident(n, _) = left.as_ref() {
                        return n == var;
                    }
                }
            }
        }
        false
    })
}

// ── Phase 2: suggest-total-upgrade (#1038) ─────────────────────────────────

/// Suggest upgrading `partial fn` to `total fn` when all constructs in the
/// body are provably bounded (#1038, #1029).
///
/// Rule id: `suggest-total-upgrade`
///
/// A `partial fn` is eligible for `total` when:
/// - All `for` loops iterate finite collections (always true by definition)
/// - All `while` loops have a `decreases` clause
/// - No direct recursive self-calls
/// - No calls to other `partial` functions (not checked here — would require
///   cross-function analysis; we check structural properties only)
pub fn suggest_total_upgrade(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.suggest_total_upgrade {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            if !matches!(f.totality, Some(Totality::Partial)) {
                continue;
            }
            // Check if the body contains only bounded constructs
            if !block_has_while_no_decreases(&f.body) && !block_calls_fn(&f.body, &f.name) {
                out.push(LintDiag::hint(
                    "suggest-total-upgrade",
                    format!(
                        "`partial fn {}` contains only bounded constructs — \
                         consider upgrading to `total fn` for stronger termination guarantees",
                        f.name,
                    ),
                    f.span.line,
                    f.span.col,
                ));
            }
        }
    }
}
