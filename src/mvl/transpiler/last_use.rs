//! Last-use analysis for clone elision (Phase A, Spec 009 Req 2).
//!
//! Computes the set of [`Span`]s that represent the *final* use of each local
//! variable within a function body.  The transpiler can emit a Rust move (no
//! `.clone()`) at these positions instead of copying the value, because the
//! caller will never reference the binding again.
//!
//! # Algorithm
//!
//! A single recursive walk visits every [`Expr::Ident`] in textual order,
//! overwriting the recorded span for each name on each encounter.  After the
//! walk, the map holds exactly one span per name — the last occurrence —
//! which becomes the returned set.
//!
//! # Conservative cases
//!
//! - **Loops** (`for`/`while`): identifiers inside loop bodies are excluded.
//!   A binding used inside a loop may be accessed on every iteration, so
//!   eliding the clone on the "last textual occurrence" would be unsound.
//! - **Lambdas**: not recursed into.  A capture may be called multiple times
//!   after the point of definition.
//!
//! # Interaction with if/match branches
//!
//! Branch bodies are walked in textual order.  The last occurrence of a name
//! across all branches wins.  This is sound: at most one branch executes per
//! call, so moving in the "last textual branch" is always safe — sibling
//! branches still clone.

use std::collections::{HashMap, HashSet};

use crate::mvl::parser::ast::{Block, ElseBranch, Expr, MatchBody, Stmt};
use crate::mvl::parser::lexer::Span;

/// Return the set of spans that are last uses of their respective variables.
///
/// Store the result in [`Codegen::last_uses`] before emitting a function body.
/// [`emit_expr_as_arg`] will suppress `.clone()` for `Expr::Ident` nodes
/// whose span appears in this set.
pub fn compute_last_uses(body: &Block) -> HashSet<Span> {
    let mut tracker = LastUseTracker::default();
    tracker.visit_block(body, false);
    // Variables that appear in loop bodies must always be cloned: they may be
    // accessed on every iteration, so even an "earlier" outside-loop use cannot
    // be moved.  Exclude them from the result entirely.
    let looped = tracker.looped_vars;
    tracker
        .last
        .into_iter()
        .filter(|(name, _)| !looped.contains(name))
        .map(|(_, span)| span)
        .collect()
}

// ── Internal tracker ─────────────────────────────────────────────────────────

#[derive(Default)]
struct LastUseTracker {
    /// Maps variable name → the most-recently-seen span of that name.
    last: HashMap<String, Span>,
    /// Variables that appear anywhere inside a loop body.  These cannot be moved
    /// even if their outside-loop use appears textually "after" the loop.
    looped_vars: HashSet<String>,
}

impl LastUseTracker {
    fn record(&mut self, name: &str, span: Span, in_loop: bool) {
        if in_loop {
            self.looped_vars.insert(name.to_string());
        } else {
            self.last.insert(name.to_string(), span);
        }
    }

    fn visit_block(&mut self, block: &Block, in_loop: bool) {
        for stmt in &block.stmts {
            self.visit_stmt(stmt, in_loop);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt, in_loop: bool) {
        match stmt {
            Stmt::Let { init, .. } => self.visit_expr(init, in_loop),
            Stmt::Assign { value, .. } => {
                // LValue target is intentionally not visited: assignment is a write,
                // not a read.  Last-use analysis tracks read uses only — the value
                // currently bound to the name is consumed by the RHS expression, not
                // by being written to.
                self.visit_expr(value, in_loop);
            }
            Stmt::Return { value, .. } => {
                if let Some(e) = value {
                    self.visit_expr(e, in_loop);
                }
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                self.visit_expr(cond, in_loop);
                self.visit_block(then, in_loop);
                if let Some(else_branch) = else_ {
                    match else_branch {
                        ElseBranch::Block(b) => self.visit_block(b, in_loop),
                        ElseBranch::If(s) => self.visit_stmt(s, in_loop),
                    }
                }
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee, in_loop);
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(e) => self.visit_expr(e, in_loop),
                        MatchBody::Block(b) => self.visit_block(b, in_loop),
                    }
                }
            }
            Stmt::For { iter, body, .. } => {
                // The iterable expression is evaluated once outside the loop.
                self.visit_expr(iter, in_loop);
                // Loop body executes 0..N times — conservatively exclude from last-use.
                self.visit_block(body, true);
            }
            Stmt::While { cond, body, .. } => {
                // Both condition and body execute repeatedly.
                self.visit_expr(cond, true);
                self.visit_block(body, true);
            }
            Stmt::Expr { expr, .. } => self.visit_expr(expr, in_loop),
        }
    }

    fn visit_expr(&mut self, expr: &Expr, in_loop: bool) {
        match expr {
            Expr::Ident(name, span) => {
                self.record(name, *span, in_loop);
            }
            Expr::Literal(_, _) => {}
            Expr::FieldAccess { expr, .. } => self.visit_expr(expr, in_loop),
            Expr::MethodCall { receiver, args, .. } => {
                self.visit_expr(receiver, in_loop);
                for arg in args {
                    self.visit_expr(arg, in_loop);
                }
            }
            Expr::FnCall { args, .. } => {
                for arg in args {
                    self.visit_expr(arg, in_loop);
                }
            }
            Expr::Unary { expr, .. }
            | Expr::Propagate { expr, .. }
            | Expr::Move { expr, .. }
            | Expr::Consume { expr, .. }
            | Expr::Declassify { expr, .. }
            | Expr::Sanitize { expr, .. } => self.visit_expr(expr, in_loop),
            Expr::Binary { left, right, .. } => {
                self.visit_expr(left, in_loop);
                self.visit_expr(right, in_loop);
            }
            Expr::If {
                cond, then, else_, ..
            } => {
                self.visit_expr(cond, in_loop);
                self.visit_block(then, in_loop);
                if let Some(else_expr) = else_ {
                    self.visit_expr(else_expr, in_loop);
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee, in_loop);
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(e) => self.visit_expr(e, in_loop),
                        MatchBody::Block(b) => self.visit_block(b, in_loop),
                    }
                }
            }
            Expr::Block(b) => self.visit_block(b, in_loop),
            // Lambdas capture variables — the body may be called multiple times.
            Expr::Lambda { .. } => {}
            Expr::Construct { fields, .. } => {
                for (_, e) in fields {
                    self.visit_expr(e, in_loop);
                }
            }
            Expr::List { elems, .. } | Expr::Set { elems, .. } => {
                for e in elems {
                    self.visit_expr(e, in_loop);
                }
            }
            Expr::Map { pairs, .. } => {
                for (k, v) in pairs {
                    self.visit_expr(k, in_loop);
                    self.visit_expr(v, in_loop);
                }
            }
        }
    }
}
