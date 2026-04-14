//! Data race freedom checker for Req 9 (partial — Phase 3).
//!
//! **Spec:** `docs/specs/008-data-race-freedom.md`
//!
//! Phase 3 checks:
//! 1. **Isolation verification** — `iso` values must not be aliased without
//!    `consume()`.  Binding `let y = iso_x` creates two live references to the
//!    same isolated object, violating the single-reference invariant.
//! 2. **Function race-freedom classification** — functions whose parameters
//!    carry only `iso`, `val`, or no capability annotation cannot participate
//!    in data races at the capability level and are proven race-free.
//!
//! **Precondition:** `TypeChecker::check_program` MUST have run first so that
//! basic `channel.send()` capability violations (Phase 1) are already flagged.
//!
//! **Phase 6** will extend this with the full actor model: structured
//! concurrency lifetimes, message-passing across actor boundaries, and an
//! architectural proof that no shared mutable state exists.

use std::collections::HashSet;

use crate::mvl::checker::errors::CheckError;
use crate::mvl::parser::ast::{
    Block, Capability, Decl, ElseBranch, Expr, FnDecl, MatchBody, Program, Stmt,
};

// ── Public entry points ───────────────────────────────────────────────────────

/// Walk every function in `prog` and emit [`CheckError::IsoAliasingViolation`]
/// for any `iso` variable that is bound to a new `let` binding without
/// `consume()`.
///
/// An `iso` parameter represents an isolated reference — only ONE live
/// reference may exist at a time.  Writing `let y = iso_x` (without wrapping
/// `iso_x` in `consume()`) would create two simultaneous references to the
/// same object, breaking the isolation guarantee that makes `iso` sendable.
///
/// The canonical pattern for transferring ownership is `consume(iso_x)`, which
/// consumes the original binding.  `channel.send(consume(item))` is the
/// standard actor-boundary transfer idiom.
pub fn check_iso_aliasing(prog: &Program, errors: &mut Vec<CheckError>) {
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            check_fn_iso(fd, errors);
        }
    }
}

/// Count functions that are provably race-free under Phase 3 capability
/// analysis.
///
/// A function is provably race-free if **none** of its parameters carry `ref`
/// capability.  Functions with only `iso`, `val`, or unannotated parameters
/// cannot participate in data races at the capability level:
/// - `iso` parameters are isolated — no other live reference exists.
/// - `val` parameters are deeply immutable — no mutation is possible.
/// - Unannotated parameters are treated as locally scoped (no cross-boundary
///   sharing).
///
/// Returns `(race_free_count, total_fn_count)`.  Extern declarations and
/// impl blocks are excluded (they are trust-boundary items checked separately).
pub fn count_race_free_fns(prog: &Program) -> (usize, usize) {
    let mut total = 0usize;
    let mut race_free = 0usize;
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            total += 1;
            let has_ref_param = fd
                .params
                .iter()
                .any(|p| matches!(p.capability, Some(Capability::Ref)));
            if !has_ref_param {
                race_free += 1;
            }
        }
    }
    (race_free, total)
}

// ── Per-function iso aliasing check ──────────────────────────────────────────

fn check_fn_iso(fd: &FnDecl, errors: &mut Vec<CheckError>) {
    let iso_params: HashSet<&str> = fd
        .params
        .iter()
        .filter(|p| matches!(p.capability, Some(Capability::Iso)))
        .map(|p| p.name.as_str())
        .collect();

    if iso_params.is_empty() {
        return; // no iso params — nothing to alias-check
    }

    check_block_iso(&fd.body, &iso_params, errors);
}

// ── Block / statement walker ──────────────────────────────────────────────────

fn check_block_iso(block: &Block, iso_vars: &HashSet<&str>, errors: &mut Vec<CheckError>) {
    for stmt in &block.stmts {
        check_stmt_iso(stmt, iso_vars, errors);
    }
}

fn check_stmt_iso(stmt: &Stmt, iso_vars: &HashSet<&str>, errors: &mut Vec<CheckError>) {
    match stmt {
        Stmt::Let { init, span, .. } => {
            // `let y = iso_x` — bare ident binding without consume() creates an alias.
            if let Expr::Ident(src, _) = init {
                if iso_vars.contains(src.as_str()) {
                    errors.push(CheckError::IsoAliasingViolation {
                        name: src.clone(),
                        span: *span,
                    });
                    // Don't recurse further — the ident is the aliasing site.
                    return;
                }
            }
            check_expr_iso(init, iso_vars, errors);
        }
        Stmt::Assign { value, .. } => check_expr_iso(value, iso_vars, errors),
        Stmt::Expr { expr, .. } => check_expr_iso(expr, iso_vars, errors),
        Stmt::Return { value: Some(e), .. } => check_expr_iso(e, iso_vars, errors),
        Stmt::Return { value: None, .. } => {}
        Stmt::If {
            cond, then, else_, ..
        } => {
            check_expr_iso(cond, iso_vars, errors);
            check_block_iso(then, iso_vars, errors);
            if let Some(eb) = else_ {
                check_else_iso(eb, iso_vars, errors);
            }
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            check_expr_iso(scrutinee, iso_vars, errors);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => check_expr_iso(e, iso_vars, errors),
                    MatchBody::Block(b) => check_block_iso(b, iso_vars, errors),
                }
            }
        }
        Stmt::For { iter, body, .. } => {
            check_expr_iso(iter, iso_vars, errors);
            check_block_iso(body, iso_vars, errors);
        }
        Stmt::While { cond, body, .. } => {
            check_expr_iso(cond, iso_vars, errors);
            check_block_iso(body, iso_vars, errors);
        }
    }
}

fn check_else_iso(eb: &ElseBranch, iso_vars: &HashSet<&str>, errors: &mut Vec<CheckError>) {
    match eb {
        ElseBranch::Block(b) => check_block_iso(b, iso_vars, errors),
        ElseBranch::If(stmt) => check_stmt_iso(stmt, iso_vars, errors),
    }
}

// ── Expression walker ─────────────────────────────────────────────────────────

fn check_expr_iso(expr: &Expr, iso_vars: &HashSet<&str>, errors: &mut Vec<CheckError>) {
    match expr {
        // `consume()` and `move` are ownership-transfer operations — not aliases.
        // Do NOT recurse: the inner ident is being consumed, not aliased.
        Expr::Consume { .. } | Expr::Move { .. } => {}

        Expr::FnCall { args, .. } => {
            for arg in args {
                check_expr_iso(arg, iso_vars, errors);
            }
        }

        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            check_expr_iso(receiver, iso_vars, errors);
            // `channel.send(iso_x)` — iso as direct send arg is sendable per
            // the capability model; no aliasing occurs at this site.
            // All other method calls: recurse normally.
            if method != "send" {
                for arg in args {
                    check_expr_iso(arg, iso_vars, errors);
                }
            }
            // For `.send()` we skip arg aliasing — sendability is already
            // verified by check_send_capability in the type checker.
        }

        Expr::Unary { expr: inner, .. }
        | Expr::Propagate { expr: inner, .. }
        | Expr::FieldAccess { expr: inner, .. }
        | Expr::Declassify { expr: inner, .. }
        | Expr::Sanitize { expr: inner, .. } => check_expr_iso(inner, iso_vars, errors),

        Expr::Binary { left, right, .. } => {
            check_expr_iso(left, iso_vars, errors);
            check_expr_iso(right, iso_vars, errors);
        }

        Expr::If {
            cond, then, else_, ..
        } => {
            check_expr_iso(cond, iso_vars, errors);
            check_block_iso(then, iso_vars, errors);
            if let Some(e) = else_ {
                check_expr_iso(e, iso_vars, errors);
            }
        }

        Expr::Match {
            scrutinee, arms, ..
        } => {
            check_expr_iso(scrutinee, iso_vars, errors);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => check_expr_iso(e, iso_vars, errors),
                    MatchBody::Block(b) => check_block_iso(b, iso_vars, errors),
                }
            }
        }

        Expr::Block(b) => check_block_iso(b, iso_vars, errors),

        Expr::Construct { fields, .. } => {
            for (_, v) in fields {
                check_expr_iso(v, iso_vars, errors);
            }
        }

        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                check_expr_iso(e, iso_vars, errors);
            }
        }

        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                check_expr_iso(k, iso_vars, errors);
                check_expr_iso(v, iso_vars, errors);
            }
        }

        // Lambdas capture by value — they don't alias the outer iso param.
        // Phase 6 will track closure captures that cross actor boundaries.
        Expr::Lambda { .. } => {}

        // Leaves — no aliasing possible.
        Expr::Literal(..) | Expr::Ident(..) => {}
    }
}
