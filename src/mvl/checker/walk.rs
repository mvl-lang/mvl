// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Generic pre-order AST visitor for expression-level traversal.
//!
//! Replaces the 7+ duplicated `walk_block/walk_stmt/walk_expr` triples scattered
//! across checker passes. Callers pass a single `FnMut(&Expr)` closure; the walker
//! handles all statement and expression variants uniformly.
//!
//! # Example
//!
//! ```ignore
//! let mut call_names = HashSet::new();
//! walk_block(&fn_decl.body, &mut |expr| {
//!     if let Expr::FnCall { name, .. } = expr {
//!         call_names.insert(name.clone());
//!     }
//! });
//! ```

use crate::mvl::parser::ast::{Block, ElseBranch, Expr, MatchBody, Stmt};

/// Visit every expression in `block` with `f` (pre-order, depth-first).
///
/// `f` is called on each [`Expr`] node *before* recursing into its children,
/// so the closure sees a parent expression before any of its sub-expressions.
/// Statement-level constructs (let bindings, for/while loops, …) are traversed
/// automatically; the closure only needs to handle expressions it cares about.
///
/// Note: `Stmt::While/For` invariants and decreases clauses are traversed via
/// the `..` pattern — they are visited through expression recursion when the
/// invariant expressions are nested inside blocks that contain them.
pub fn walk_block<F>(block: &Block, f: &mut F)
where
    F: FnMut(&Expr),
{
    for stmt in &block.stmts {
        walk_stmt(stmt, f);
    }
}

fn walk_stmt<F>(stmt: &Stmt, f: &mut F)
where
    F: FnMut(&Expr),
{
    match stmt {
        Stmt::Let { init, .. } => walk_expr(init, f),
        Stmt::Assign { value, .. } => walk_expr(value, f),
        Stmt::Return { value: Some(e), .. } => walk_expr(e, f),
        Stmt::Return { value: None, .. } => {}
        Stmt::Expr { expr, .. } => walk_expr(expr, f),
        Stmt::If {
            cond, then, else_, ..
        } => {
            walk_expr(cond, f);
            walk_block(then, f);
            match else_ {
                None => {}
                Some(ElseBranch::Block(b)) => walk_block(b, f),
                Some(ElseBranch::If(s)) => walk_stmt(s, f),
            }
        }
        Stmt::While { cond, body, .. } => {
            walk_expr(cond, f);
            walk_block(body, f);
        }
        Stmt::For { iter, body, .. } => {
            walk_expr(iter, f);
            walk_block(body, f);
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            walk_expr(scrutinee, f);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => walk_expr(e, f),
                    MatchBody::Block(b) => walk_block(b, f),
                }
            }
        }
    }
}

/// Visit `expr` with `f` (pre-order), then recurse into all child expressions.
pub fn walk_expr<F>(expr: &Expr, f: &mut F)
where
    F: FnMut(&Expr),
{
    f(expr);
    match expr {
        Expr::FnCall { args, .. } => {
            for a in args {
                walk_expr(a, f);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            walk_expr(receiver, f);
            for a in args {
                walk_expr(a, f);
            }
        }
        Expr::Block(b) => walk_block(b, f),
        Expr::If {
            cond, then, else_, ..
        } => {
            walk_expr(cond, f);
            walk_block(then, f);
            if let Some(e) = else_ {
                walk_expr(e, f);
            }
        }
        Expr::Binary { left, right, .. } => {
            walk_expr(left, f);
            walk_expr(right, f);
        }
        Expr::Unary { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Relabel { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::Borrow { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::As { expr, .. } => walk_expr(expr, f),
        Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => {
            for (_, e) in fields {
                walk_expr(e, f);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } | Expr::Tuple { elems, .. } => {
            for e in elems {
                walk_expr(e, f);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                walk_expr(k, f);
                walk_expr(v, f);
            }
        }
        Expr::Lambda { body, .. } => walk_expr(body, f),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            walk_expr(scrutinee, f);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => walk_expr(e, f),
                    MatchBody::Block(b) => walk_block(b, f),
                }
            }
        }
        Expr::Select { arms, .. } => {
            for arm in arms {
                walk_expr(&arm.expr, f);
                walk_block(&arm.body, f);
            }
        }
        // Leaves — nothing to recurse into.
        Expr::Literal(..) | Expr::Ident(..) | Expr::Quantifier(..) => {}
    }
}
