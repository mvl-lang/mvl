//! Structural recursion checker for Req 8 (Termination).
//!
//! **Spec:** `docs/specs/007-termination.md`
//!
//! For every `total fn` (explicit or implicit), this pass verifies that all
//! self-recursive calls operate on provably smaller arguments.  Two decrease
//! measures are currently recognized:
//!
//! - **Integer decrement** — the recursive argument is `param - N` where `N`
//!   is a positive integer literal and `param` is any function parameter.
//!   (spec 007 §Req 2)
//! - **Integer division** — the recursive argument is `param / N` where `N`
//!   is an integer literal greater than 1 and `param` is any function
//!   parameter.  Catches binary search, merge sort, and other logarithmic
//!   algorithms.  (spec 007 §Req 2)
//! - **Structural subterm** — the recursive argument is a variable that was
//!   pattern-bound from a *sub-pattern* of a function parameter (e.g. the
//!   `tail` in `match list { Cons(_, tail) => … }`), where the match
//!   scrutinee is a *bare parameter identifier*.  (spec 007 §Req 3)
//! - **Method accessor** — the recursive argument is `param.tail()` or
//!   `param.rest()` (or the same applied to a known structural subterm),
//!   which yields a strict substructure of the receiver.  (spec 007 §Req 3)
//! - **Subterm length** — the recursive argument is `subterm.len()` where
//!   `subterm` is a known structural subterm; its length is provably smaller
//!   than the original.  (spec 007 §Req 3)
//!
//! Mutual recursion and `while`-loop decreasing-measure annotations are not
//! yet analysed (tracked in #142; spec 007 §Known Limitations L1/L2).
//!
//! **Precondition:** `TypeChecker::check_program` MUST have run before this
//! pass so that `while` loops in total functions are already flagged as
//! `UnboundedLoopInTotal`.  (spec 007 §Req 5)

use std::collections::HashSet;

use crate::mvl::checker::errors::CheckError;
use crate::mvl::parser::ast::{
    BinaryOp, Block, Decl, ElseBranch, Expr, FnDecl, Literal, MatchArm, MatchBody, Pattern,
    Program, Stmt, Totality, UnaryOp,
};

// ── Public entry point ────────────────────────────────────────────────────────

/// Walk every total (or implicitly total) function in `prog` and emit
/// [`CheckError::UnprovenRecursion`] for any recursive call that cannot be
/// proven terminating.  Errors are appended to `errors`.
///
/// Functions with `Totality::None` are implicitly total — checked by default.
/// Only `Some(Totality::Partial)` is exempt.  (spec 007 §Scope and Defaults)
pub fn check_structural_recursion(prog: &Program, errors: &mut Vec<CheckError>) {
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            // Totality::None == implicitly total (default); only Partial is exempt.
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
/// (spec 007 §Req 3)
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
        } => check_match_arms(scrutinee, arms, ctx, errors),
        Stmt::For { iter, body, .. } => {
            // `for` loops over finite iterators are trivially terminating.
            // Recursive calls inside the body are still checked. (spec 007 §Req 5)
            check_expr(iter, ctx, errors);
            check_block(body, ctx, errors);
        }
        Stmt::While { .. } => {
            // `while` in total functions is already rejected by the type checker
            // as UnboundedLoopInTotal — nothing to do here. (spec 007 §Req 5)
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
        | Expr::Sanitize { expr: inner, .. }
        | Expr::Borrow { expr: inner, .. } => check_expr(inner, ctx, errors),

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
        } => check_match_arms(scrutinee, arms, ctx, errors),

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
            // (spec 007 §Req 4)
        }

        // Leaves — nothing to recurse into.
        Expr::Literal(..) | Expr::Ident(..) => {}
    }
}

// ── Match-arm walker (shared by Stmt::Match and Expr::Match) ─────────────────

/// Walk `scrutinee` and each arm body, extending the subterm context for arms
/// whose pattern binds sub-components of a direct function parameter.
/// (spec 007 §Req 3)
fn check_match_arms(
    scrutinee: &Expr,
    arms: &[MatchArm],
    ctx: &TermCtx<'_>,
    errors: &mut Vec<CheckError>,
) {
    check_expr(scrutinee, ctx, errors);
    let on_param = as_param(scrutinee, ctx.params).is_some();
    for arm in arms {
        // Only extend `smaller` when the scrutinee is a bare parameter;
        // matching on a local or expression does not establish the subterm
        // relation. (spec 007 §Req 3, Scenario: Match on non-parameter)
        let arm_ctx;
        let effective_ctx: &TermCtx<'_> = if on_param {
            arm_ctx = ctx.with_smaller(subterm_vars(&arm.pattern));
            &arm_ctx
        } else {
            ctx
        };
        check_match_body(&arm.body, effective_ctx, errors);
    }
}

fn check_match_body(body: &MatchBody, ctx: &TermCtx<'_>, errors: &mut Vec<CheckError>) {
    match body {
        MatchBody::Expr(e) => check_expr(e, ctx, errors),
        MatchBody::Block(b) => check_block(b, ctx, errors),
    }
}

// ── Decrease analysis ─────────────────────────────────────────────────────────

/// Return `true` if at least one argument at the recursive call site is
/// provably smaller than some function parameter.
fn is_decreasing(args: &[Expr], ctx: &TermCtx<'_>) -> bool {
    args.iter()
        .any(|arg| arg_decreases(arg, ctx.params, &ctx.smaller))
}

/// Return `true` if `arg` is provably smaller than any parameter or is a
/// known structural subterm.
///
/// Recognised measures (spec 007 §Req 2 and §Req 3):
/// - Structural subterm: `arg` is a variable in `smaller`.
/// - Integer decrement: `arg` is `param - N` where `param` names any
///   function parameter and `N > 0`.
/// - Integer division: `arg` is `param / N` where `N > 1`.
/// - Method accessor: `arg` is `param.tail()` or `param.rest()` (or the
///   same on a known subterm) — yields a strict structural subterm.
/// - Subterm length: `arg` is `subterm.len()` where `subterm` is a known
///   structural subterm — its length is strictly smaller.
fn arg_decreases(arg: &Expr, params: &[&str], smaller: &HashSet<String>) -> bool {
    match arg {
        // A variable known to be a structural subterm. (spec 007 §Req 3)
        Expr::Ident(name, _) if smaller.contains(name.as_str()) => true,

        // `*subterm` — dereferencing a Box<T> subterm yields T, which is also
        // structurally smaller (Box is a thin indirection layer for recursive ADTs).
        // Required for recursive enums where a match arm binds `tail: Box<T>`
        // and the recursive call passes `*tail`. (spec 007 §Req 3)
        Expr::Unary {
            op: UnaryOp::Deref,
            expr: inner,
            ..
        } => arg_decreases(inner, params, smaller),

        // `param - N` where N is a positive integer literal and `param` is
        // any function parameter (not restricted to positional match).
        // (spec 007 §Req 2)
        Expr::Binary {
            op: BinaryOp::Sub,
            left,
            right,
            ..
        } => {
            if let (Expr::Ident(lname, _), Expr::Literal(Literal::Integer(n), _)) =
                (left.as_ref(), right.as_ref())
            {
                *n > 0 && params.contains(&lname.as_str())
            } else {
                false
            }
        }

        // `param / N` where N > 1 and `param` is any function parameter.
        // Catches binary search, merge sort, and other logarithmic algorithms.
        // (spec 007 §Req 2)
        Expr::Binary {
            op: BinaryOp::Div,
            left,
            right,
            ..
        } => {
            if let (Expr::Ident(lname, _), Expr::Literal(Literal::Integer(n), _)) =
                (left.as_ref(), right.as_ref())
            {
                *n > 1 && params.contains(&lname.as_str())
            } else {
                false
            }
        }

        // `param.tail()` or `param.rest()` — zero-argument accessor methods
        // that return a strict substructure of their receiver.  Also accepted
        // when the receiver is already a known structural subterm.
        // (spec 007 §Req 3)
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } if args.is_empty() && matches!(method.as_str(), "tail" | "rest") => {
            if let Expr::Ident(rname, _) = receiver.as_ref() {
                params.contains(&rname.as_str()) || smaller.contains(rname.as_str())
            } else {
                false
            }
        }

        // `subterm.len()` — the length of a known structural subterm is
        // strictly smaller than the length of the original parameter.
        // NOTE: `param.len()` is NOT accepted here — the length of the original
        // parameter is not smaller than itself.  Only variables already in `smaller`
        // (i.e. bound as structural subterms via pattern match) qualify.
        // This is intentionally asymmetric with the `.tail()`/`.rest()` arm above,
        // which does accept bare parameters.  (spec 007 §Req 3)
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } if args.is_empty() && method == "len" => {
            if let Expr::Ident(rname, _) = receiver.as_ref() {
                smaller.contains(rname.as_str())
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
/// smaller than whatever was matched.  (spec 007 §Req 3)
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

/// Yield the identifier name at the leaf of a pattern, if it is a bare
/// `Pattern::Ident`.  One level only — avoids over-approximation.
fn leaf_idents(pattern: &Pattern) -> impl Iterator<Item = String> + '_ {
    match pattern {
        Pattern::Ident(name, _) => Some(name.clone()),
        _ => None,
    }
    .into_iter()
}
