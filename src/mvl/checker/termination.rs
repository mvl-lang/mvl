//! Structural recursion checker for Req 8 (Termination).
//!
//! For every `total fn` (explicit or implicit), this pass verifies that all
//! self-recursive calls operate on provably smaller arguments.  Two decrease
//! measures are currently recognized:
//!
//! - **Integer decrement** — the recursive argument is `param - N` where `N`
//!   is a positive integer literal.
//! - **Structural subterm** — the recursive argument is a variable that was
//!   pattern-bound from a *sub-pattern* of a function parameter (e.g. the
//!   `tail` in `match list { Cons(_, tail) => … }`).
//!
//! Mutual recursion and `while`-loop decreasing-measure annotations are not
//! yet analysed (tracked in #142).

use std::collections::HashSet;

use crate::mvl::checker::errors::CheckError;
use crate::mvl::parser::ast::{
    BinaryOp, Block, Decl, ElseBranch, Expr, FnDecl, Literal, MatchBody, Pattern, Program, Stmt,
    Totality,
};

// ── Public entry point ────────────────────────────────────────────────────────

/// Walk every total (or implicitly total) function in `prog` and emit
/// [`CheckError::UnprovenRecursion`] for any recursive call that cannot be
/// proven terminating.  Errors are appended to `errors`.
pub fn check_structural_recursion(prog: &Program, errors: &mut Vec<CheckError>) {
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            if !matches!(fd.totality, Some(Totality::Partial)) {
                check_fn(fd, errors);
            }
        }
    }
}

// ── Per-function analysis ─────────────────────────────────────────────────────

fn check_fn(fd: &FnDecl, errors: &mut Vec<CheckError>) {
    let params: Vec<&str> = fd.params.iter().map(|p| p.name.as_str()).collect();
    let ctx = TermCtx {
        fn_name: &fd.name,
        params: &params,
        smaller: HashSet::new(),
    };
    check_block(&fd.body, &ctx, errors);
}

// ── Walking context ───────────────────────────────────────────────────────────

/// Context threaded through the AST walk.
///
/// `smaller` is the set of local variable names that are known to be
/// structurally smaller than at least one function parameter because they
/// were bound from a sub-pattern of that parameter in a surrounding `match`.
#[derive(Clone)]
struct TermCtx<'a> {
    fn_name: &'a str,
    params: &'a [&'a str],
    smaller: HashSet<String>,
}

impl<'a> TermCtx<'a> {
    /// Return a new context extended with `more_smaller` variables.
    fn with_smaller(&self, more_smaller: impl IntoIterator<Item = String>) -> Self {
        let mut next = self.clone();
        next.smaller.extend(more_smaller);
        next
    }
}

// ── Block / statement walker ──────────────────────────────────────────────────

fn check_block(block: &Block, ctx: &TermCtx<'_>, errors: &mut Vec<CheckError>) {
    for stmt in &block.stmts {
        check_stmt(stmt, ctx, errors);
    }
}

fn check_stmt(stmt: &Stmt, ctx: &TermCtx<'_>, errors: &mut Vec<CheckError>) {
    match stmt {
        Stmt::Expr { expr, .. } => check_expr(expr, ctx, errors),
        Stmt::Return { value: Some(e), .. } => check_expr(e, ctx, errors),
        Stmt::Return { value: None, .. } => {}
        Stmt::Let { init, .. } => check_expr(init, ctx, errors),
        Stmt::Assign { value, .. } => check_expr(value, ctx, errors),
        Stmt::If {
            cond, then, else_, ..
        } => {
            check_expr(cond, ctx, errors);
            check_block(then, ctx, errors);
            if let Some(eb) = else_ {
                check_else(eb, ctx, errors);
            }
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            check_expr(scrutinee, ctx, errors);
            let param_matched = as_param(scrutinee, ctx.params);
            for arm in arms {
                let arm_ctx = if param_matched.is_some() {
                    ctx.with_smaller(subterm_vars(&arm.pattern))
                } else {
                    ctx.clone()
                };
                match &arm.body {
                    MatchBody::Expr(e) => check_expr(e, &arm_ctx, errors),
                    MatchBody::Block(b) => check_block(b, &arm_ctx, errors),
                }
            }
        }
        Stmt::For { iter, body, .. } => {
            // `for` loops over finite iterators are trivially terminating.
            check_expr(iter, ctx, errors);
            check_block(body, ctx, errors);
        }
        Stmt::While { .. } => {
            // `while` in total functions is already rejected by the type
            // checker as UnboundedLoopInTotal — nothing to do here.
        }
    }
}

fn check_else(eb: &ElseBranch, ctx: &TermCtx<'_>, errors: &mut Vec<CheckError>) {
    match eb {
        ElseBranch::Block(b) => check_block(b, ctx, errors),
        ElseBranch::If(stmt) => check_stmt(stmt, ctx, errors),
    }
}

// ── Expression walker ─────────────────────────────────────────────────────────

fn check_expr(expr: &Expr, ctx: &TermCtx<'_>, errors: &mut Vec<CheckError>) {
    match expr {
        // ── Recursive call site ───────────────────────────────────────────
        Expr::FnCall {
            name, args, span, ..
        } if name == ctx.fn_name => {
            if !is_decreasing(args, ctx) {
                errors.push(CheckError::UnprovenRecursion {
                    fn_name: ctx.fn_name.to_string(),
                    span: *span,
                });
            }
            // Still descend into args so nested recursive calls are caught.
            for arg in args {
                check_expr(arg, ctx, errors);
            }
        }

        // ── Expressions that don't introduce new scope ────────────────────
        Expr::FnCall { args, .. } => {
            for arg in args {
                check_expr(arg, ctx, errors);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            check_expr(receiver, ctx, errors);
            for arg in args {
                check_expr(arg, ctx, errors);
            }
        }
        Expr::Unary { expr: inner, .. }
        | Expr::Propagate { expr: inner, .. }
        | Expr::FieldAccess { expr: inner, .. }
        | Expr::Move { expr: inner, .. }
        | Expr::Consume { expr: inner, .. }
        | Expr::Declassify { expr: inner, .. }
        | Expr::Sanitize { expr: inner, .. } => check_expr(inner, ctx, errors),

        Expr::Binary { left, right, .. } => {
            check_expr(left, ctx, errors);
            check_expr(right, ctx, errors);
        }

        Expr::If {
            cond, then, else_, ..
        } => {
            check_expr(cond, ctx, errors);
            check_block(then, ctx, errors);
            if let Some(e) = else_ {
                check_expr(e, ctx, errors);
            }
        }

        Expr::Match {
            scrutinee, arms, ..
        } => {
            check_expr(scrutinee, ctx, errors);
            let param_matched = as_param(scrutinee, ctx.params);
            for arm in arms {
                let arm_ctx = if param_matched.is_some() {
                    ctx.with_smaller(subterm_vars(&arm.pattern))
                } else {
                    ctx.clone()
                };
                match &arm.body {
                    MatchBody::Expr(e) => check_expr(e, &arm_ctx, errors),
                    MatchBody::Block(b) => check_block(b, &arm_ctx, errors),
                }
            }
        }

        Expr::Block(b) => check_block(b, ctx, errors),

        Expr::Construct { fields, .. } => {
            for (_, v) in fields {
                check_expr(v, ctx, errors);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                check_expr(e, ctx, errors);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                check_expr(k, ctx, errors);
                check_expr(v, ctx, errors);
            }
        }
        Expr::Lambda { .. } => {
            // Don't recurse into lambdas: they have their own scope and are
            // not self-recursive with respect to the enclosing function.
        }

        // Leaves — nothing to recurse into.
        Expr::Literal(..) | Expr::Ident(..) => {}
    }
}

// ── Decrease analysis ─────────────────────────────────────────────────────────

/// Return `true` if at least one argument at the recursive call site is
/// provably smaller than the corresponding (or any) parameter.
fn is_decreasing(args: &[Expr], ctx: &TermCtx<'_>) -> bool {
    for (i, arg) in args.iter().enumerate() {
        let param = ctx.params.get(i).copied();
        if arg_decreases(arg, param, &ctx.smaller) {
            return true;
        }
    }
    false
}

/// Return `true` if `arg` is provably smaller than `param` (by name) or any
/// known-smaller variable.
fn arg_decreases(arg: &Expr, param: Option<&str>, smaller: &HashSet<String>) -> bool {
    match arg {
        // A variable known to be a structural subterm.
        Expr::Ident(name, _) if smaller.contains(name.as_str()) => true,

        // `param - N` where N is a positive integer literal.
        Expr::Binary {
            op: BinaryOp::Sub,
            left,
            right,
            ..
        } => {
            if let (Expr::Ident(lname, _), Expr::Literal(Literal::Integer(n), _)) =
                (left.as_ref(), right.as_ref())
            {
                param == Some(lname.as_str()) && *n > 0
            } else {
                false
            }
        }

        _ => false,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// If `expr` is a bare identifier that names one of the function parameters,
/// return that parameter name; otherwise `None`.
fn as_param<'a>(expr: &Expr, params: &[&'a str]) -> Option<&'a str> {
    if let Expr::Ident(name, _) = expr {
        params.iter().copied().find(|&p| p == name.as_str())
    } else {
        None
    }
}

/// Collect all variable names introduced by `pattern` at the *immediate*
/// sub-level (one level down from the matched value).  These are structurally
/// smaller than whatever was matched.
fn subterm_vars(pattern: &Pattern) -> Vec<String> {
    match pattern {
        // Binding the whole value is not smaller — skip.
        Pattern::Ident(..) | Pattern::Wildcard(..) | Pattern::Literal(..) | Pattern::None(..) => {
            vec![]
        }

        // Every field binding is a subterm of the constructor.
        Pattern::TupleStruct { fields, .. } => fields.iter().flat_map(leaf_idents).collect(),
        Pattern::Tuple { elems, .. } => elems.iter().flat_map(leaf_idents).collect(),
        Pattern::Struct { fields, .. } => fields.iter().flat_map(|(_, p)| leaf_idents(p)).collect(),
        Pattern::Some { inner, .. } | Pattern::Ok { inner, .. } | Pattern::Err { inner, .. } => {
            leaf_idents(inner).collect()
        }
    }
}

/// Yield the identifier names at the *leaf* of a pattern (not recursive —
/// one level only, to avoid over-approximation).
fn leaf_idents(pattern: &Pattern) -> impl Iterator<Item = String> + '_ {
    let mut out = Vec::new();
    if let Pattern::Ident(name, _) = pattern {
        out.push(name.clone());
    }
    out.into_iter()
}
