// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Phase-2 semantic rules — traverse the AST to detect code smells.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{
    Block, Decl, ElseBranch, Expr, MatchArm, MatchBody, Pattern, Program, Stmt, TypeBody, TypeExpr,
    VariantFields,
};

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
        | Expr::Borrow { expr: e, .. }
        | Expr::As { expr: e, .. } => expr_has_while_no_decreases(e),
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
        | Expr::Borrow { expr: e, .. }
        | Expr::As { expr: e, .. } => expr_calls_fn(e, name),
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
        | Expr::Borrow { expr: e, .. }
        | Expr::As { expr: e, .. } => expr_has_calls(e),
        Expr::Construct { fields, .. } => fields.iter().any(|(_, e)| expr_has_calls(e)),
        Expr::Spawn { fields, .. } => fields.iter().any(|(_, e)| expr_has_calls(e)),
        Expr::Select { arms, .. } => arms
            .iter()
            .any(|a| expr_has_calls(&a.expr) || block_has_calls(&a.body)),
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

/// Warn on `extern "rust"` blocks — deprecated in favour of `extern "C"` (#561).
///
/// Rule id: `deprecated-extern-rust`
///
/// `extern "rust"` only works with the Rust transpiler backend. Use `extern "C"` with
/// `#[no_mangle] pub extern "C"` on the Rust side for backend-symmetric FFI.
pub fn deprecated_extern_rust(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.deprecated_extern_rust {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Extern(ed) = decl {
            if ed.abi == "rust" {
                let s = ed.span;
                out.push(LintDiag::warning(
                    "deprecated-extern-rust",
                    "extern \"rust\" is deprecated; use extern \"C\" with #[no_mangle] pub extern \"C\" on the Rust implementation",
                    s.line,
                    s.col,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::linter::config::LintConfig;
    use crate::mvl::parser::Parser;

    fn cfg() -> LintConfig {
        let mut c = LintConfig::default();
        c.line_length = 120;
        c.trailing_ws = true;
        c.indentation = true;
        c.final_newline = true;
        c.consistent_comment_style = true;
        c
    }

    fn parse(src: &str) -> crate::mvl::parser::ast::Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    // -- unreachable_code --

    #[test]
    fn unreachable_after_return_detected() {
        let src = "fn f() -> Int { return 1; let x: Int = 2; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unreachable_code(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1, "expected 1 unreachable diagnostic");
        assert_eq!(diags[0].rule, "unreachable-code");
    }

    #[test]
    fn unreachable_code_disabled() {
        let src = "fn f() -> Int { return 1; let x: Int = 2; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.unreachable_code = false;
        unreachable_code(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn reachable_code_clean() {
        let src = "fn f() -> Int { let x: Int = 1; return x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unreachable_code(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_match --

    #[test]
    fn single_arm_wildcard_detected() {
        let src = "fn f(x: Int) -> Int { match x { _ => x } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_match(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-match");
    }

    #[test]
    fn single_arm_binding_detected() {
        let src = "fn f(x: Int) -> Int { match x { v => v } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_match(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-match");
    }

    #[test]
    fn multi_arm_match_clean() {
        let src = "type Color = enum { Red, Green, Blue }\nfn f(c: Color) -> Int { match c { Color::Red => 1 Color::Green => 2 Color::Blue => 3 } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_match(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_effects --

    #[test]
    fn effects_on_call_free_fn_detected() {
        let src = "fn f() -> Int ! Console { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_effects(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-effects");
        assert!(diags[0].message.contains("Console"));
    }

    #[test]
    fn effects_on_fn_with_call_clean() {
        let src = "fn f() -> Unit ! Console {\n    println(\"hi\")\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_effects(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn no_effects_declared_clean() {
        let src = "fn f() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_effects(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_ifc_labels --

    #[test]
    fn public_label_on_param_no_longer_detected(/* #894: Public is not a label anymore */) {
        // Post-#894: `Public` is a plain identifier, not a label keyword.
        // `Public[Int]` parses as a generic base type, not TypeExpr::Labeled.
        // The redundant-ifc-label rule no longer fires for it.
        let src = "fn f(x: Public[Int]) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty(), "Public is no longer a label (#894)");
    }

    #[test]
    fn secret_label_on_param_clean() {
        let src = "fn f(x: Secret[Int]) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_on_return_type_no_longer_detected() {
        // Post-#894: `Public[String]` is just a generic type — no lint fires.
        let src = "fn f() -> Public[String] { \"hi\" }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn redundant_ifc_disabled() {
        let src = "fn f(x: Tainted[Int]) -> Int { relabel trust(x, \"V-01\") }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.redundant_ifc_labels = false;
        redundant_ifc_labels(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_on_struct_field_no_longer_detected() {
        // Post-#894: `Public[Int]` in a struct is a generic type, not a labeled type.
        let src = "type Wrapper = struct { data: Public[Int] }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_in_type_alias_no_longer_detected() {
        // Post-#894: `Public[Int]` as type alias — not a labeled type.
        let src = "type MyInt = Public[Int]\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_match: config-disable test --

    #[test]
    fn redundant_match_disabled() {
        let src = "fn f(x: Int) -> Int { match x { _ => x } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.redundant_match = false;
        redundant_match(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_effects: config-disable test --

    #[test]
    fn redundant_effects_disabled() {
        let src = "fn f() -> Int ! Console { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.redundant_effects = false;
        redundant_effects(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- missing_annotations --

    fn cfg_missing_annotations_on() -> LintConfig {
        LintConfig {
            missing_annotations: true,
            ..LintConfig::default()
        }
    }

    #[test]
    fn missing_annotations_fires_on_call_without_effects() {
        // fn has a call but no declared effects — must warn when rule is enabled
        let src = "fn foo() -> Unit {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-annotation" && d.message.contains("foo")),
            "expected missing-annotation for `foo`; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_off_by_default() {
        // default config has missing_annotations = false — rule must be silent
        let src = "fn foo() -> Unit {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "missing-annotation must not fire with default config; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_no_fire_on_effects_declared() {
        // fn has calls AND declared effects — must not warn
        let src = "fn foo() -> Unit ! Console {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "must not warn when effects are declared; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_no_fire_on_callless_fn() {
        // pure arithmetic fn — no calls, no effects — must not warn
        let src = "fn add(x: Int, y: Int) -> Int {\n    x + y\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "must not warn on call-free function; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_no_fire_on_test_fn() {
        // test fn is excluded from the rule
        let src = "test fn check_add() -> Unit {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "test fn must be excluded; got: {diags:?}"
        );
    }

    // -- missing_totality --

    #[test]
    fn missing_totality_fires_on_unannotated_pub_fn() {
        // unannotated pub fn must warn with default config (rule is ON by default)
        let src = "pub fn add(x: Int, y: Int) -> Int { x + y }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("add")),
            "expected missing-totality for `add`; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_silent_on_private_fn() {
        // private fn must not warn even when unannotated
        let src = "fn helper(x: Int) -> Int { x + 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-totality"),
            "private fn must not trigger missing-totality; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_silent_when_annotated() {
        // explicit `total` suppresses the warning
        let src = "pub total fn add(x: Int, y: Int) -> Int { x + y }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-totality"),
            "annotated pub fn must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_suggests_total_for_simple_fn() {
        // no while, no recursion → suggest `total`
        let src = "pub fn double(x: Int) -> Int { x * 2 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("`total`")),
            "should suggest `total` for simple fn; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_suggests_partial_for_while_no_decreases() {
        // while without decreases → suggest `partial`
        let src = concat!(
            "pub fn count(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    while i < n {\n",
            "        i = i + 1\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("`partial`")),
            "should suggest `partial` for while without decreases; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_suggests_partial_for_recursive_fn() {
        // direct recursion → suggest `partial`
        let src = concat!(
            "pub fn factorial(n: Int) -> Int {\n",
            "    if n <= 1 { 1 } else { n * factorial(n - 1) }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("`partial`")),
            "should suggest `partial` for recursive fn; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_off_when_disabled() {
        // rule can be opted out via config
        let cfg_off = LintConfig {
            require_explicit_totality: false,
            ..LintConfig::default()
        };
        let src = "pub fn add(x: Int, y: Int) -> Int { x + y }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg_off, &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-totality"),
            "rule must be silent when disabled; got: {diags:?}"
        );
    }

    // -- deprecated_extern_rust --

    #[test]
    fn deprecated_extern_rust_fires_on_extern_rust() {
        let src = "extern \"rust\" {\n    fn hash(data: String) -> String;\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        deprecated_extern_rust(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "deprecated-extern-rust"),
            "extern \"rust\" should trigger deprecated-extern-rust; got: {diags:?}"
        );
    }

    #[test]
    fn deprecated_extern_rust_silent_for_extern_c() {
        let src = "extern \"c\" {\n    fn sqrt(x: Float) -> Float;\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        deprecated_extern_rust(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "deprecated-extern-rust"),
            "extern \"c\" must not trigger the rule; got: {diags:?}"
        );
    }

    #[test]
    fn deprecated_extern_rust_off_when_disabled() {
        let cfg_off = LintConfig {
            deprecated_extern_rust: false,
            ..LintConfig::default()
        };
        let src = "extern \"rust\" {\n    fn hash(data: String) -> String;\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        deprecated_extern_rust(&prog, &cfg_off, &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "deprecated-extern-rust"),
            "rule must be silent when disabled; got: {diags:?}"
        );
    }
}
