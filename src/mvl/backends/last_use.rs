// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Last-use analysis for clone elision (Phase A, Spec 009 Req 2).
//!
//! Computes the set of [`Span`]s that represent the *final* use of each local
//! variable within a function body.  The transpiler can emit a Rust move (no
//! `.clone()`) at these positions instead of copying the value, because the
//! caller will never reference the binding again.
//!
//! # Algorithm
//!
//! A single recursive walk visits every [`TirExprKind::Var`] in textual order.
//! Reads are keyed by **binding identity**, not by name: a lexical scope stack
//! maps each in-scope name to a synthetic id, and a `let`/`for`/select-receive
//! binding or match-arm pattern pushes a fresh id that shadows (without
//! disturbing) any outer binding of the same name. A name with no local
//! binding — e.g. a function parameter — resolves to a stable id created on
//! its first read and reused for the rest of the walk. Recording overwrites
//! the span for the *resolved id* on each encounter, so the map ends up with
//! exactly one span per binding — its last occurrence — which becomes the
//! returned set.
//!
//! This distinction matters under shadowing:
//!
//! ```mvl
//! let s: String = a();
//! {
//!     let s: String = b();
//!     f(s);   // last use of the INNER binding
//! }
//! g(s);       // last use of the OUTER binding — a different id, not textual
//!             // "after f(s)" for the same name
//! ```
//!
//! Name-only keying would let `g(s)`'s span win as "the" last use of `s`,
//! even though `f(s)` is genuinely the inner binding's own last use — an
//! unsound move on the LLVM backend (double-free/use-after-free), masked on
//! the Rust backend only because rustc's own borrow checker rejects the bad
//! move at compile time.
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
//! Branch bodies are walked in textual order.  The last occurrence of a
//! binding across all branches wins.  This is sound: at most one branch
//! executes per call, so moving in the "last textual branch" is always safe
//! — sibling branches still clone.

use std::collections::{HashMap, HashSet};

use crate::mvl::ir::{
    TirBlock, TirElseBranch, TirExpr, TirExprKind, TirMatchBody, TirSelectArm, TirStmt,
};
use crate::mvl::parser::ast::Pattern;
use crate::mvl::parser::lexer::Span;

/// Return the set of spans that are last uses of their respective variables.
///
/// Store the result in [`Codegen::last_uses`] before emitting a function body.
/// [`emit_expr_as_arg`] will suppress `.clone()` for `TirExprKind::Var` nodes
/// whose span appears in this set.
pub fn compute_last_uses(body: &TirBlock) -> HashSet<Span> {
    let mut tracker = LastUseTracker::default();
    tracker.visit_block(body, false);
    // Bindings that are ever read inside a loop body must always be cloned:
    // they may be accessed on every iteration, so even an "earlier" outside-loop
    // use cannot be moved.  Exclude them from the result entirely.
    let looped = tracker.looped;
    tracker
        .last
        .into_iter()
        .filter(|(id, _)| !looped.contains(id))
        .map(|(_, span)| span)
        .collect()
}

/// Collect every name a pattern binds, in the order they appear.
///
/// `Or` alternatives are required (by the checker) to bind identical names,
/// so only the first alternative is inspected.
fn pattern_names(pattern: &Pattern, out: &mut Vec<String>) {
    match pattern {
        Pattern::Wildcard(_) | Pattern::Literal(_, _) | Pattern::None(_) => {}
        Pattern::Ident(name, _) => out.push(name.clone()),
        Pattern::TupleStruct { fields, .. } => {
            for f in fields {
                pattern_names(f, out);
            }
        }
        Pattern::Struct { fields, .. } => {
            for (_, p) in fields {
                pattern_names(p, out);
            }
        }
        Pattern::Some { inner, .. } | Pattern::Ok { inner, .. } | Pattern::Err { inner, .. } => {
            pattern_names(inner, out);
        }
        Pattern::Or { patterns, .. } => {
            if let Some(first) = patterns.first() {
                pattern_names(first, out);
            }
        }
    }
}

// ── Internal tracker ─────────────────────────────────────────────────────────

#[derive(Default)]
struct LastUseTracker {
    /// binding id → the most-recently-seen span of a read that resolved to it.
    last: HashMap<u64, Span>,
    /// binding ids read anywhere inside a loop body.
    looped: HashSet<u64>,
    /// Lexical scope stack.  Each frame maps a name to the binding id currently
    /// in effect for it; pushing a shadowing name onto a new frame leaves the
    /// outer frame's entry untouched.
    scopes: Vec<HashMap<String, u64>>,
    next_id: u64,
}

impl LastUseTracker {
    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Introduce a new binding for `name` in the innermost scope, shadowing
    /// (without mutating) any binding of the same name in an outer scope.
    fn bind(&mut self, name: &str) {
        let id = self.fresh_id();
        self.scopes
            .last_mut()
            .expect("bind called outside any scope")
            .insert(name.to_string(), id);
    }

    fn bind_pattern(&mut self, pattern: &Pattern) {
        let mut names = Vec::new();
        pattern_names(pattern, &mut names);
        for name in names {
            self.bind(&name);
        }
    }

    /// Resolve `name` to the binding id currently in effect, searching from
    /// the innermost scope outward.  A name with no active local binding
    /// (e.g. a function parameter) is assigned a stable id at the root scope
    /// the first time it is read, and that id is reused for every later read.
    fn resolve(&mut self, name: &str) -> u64 {
        for scope in self.scopes.iter().rev() {
            if let Some(&id) = scope.get(name) {
                return id;
            }
        }
        let id = self.fresh_id();
        self.scopes[0].insert(name.to_string(), id);
        id
    }

    fn record(&mut self, name: &str, span: Span, in_loop: bool) {
        let id = self.resolve(name);
        if in_loop {
            self.looped.insert(id);
        } else {
            self.last.insert(id, span);
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn visit_block(&mut self, block: &TirBlock, in_loop: bool) {
        self.push_scope();
        for stmt in &block.stmts {
            self.visit_stmt(stmt, in_loop);
        }
        self.pop_scope();
    }

    fn visit_stmt(&mut self, stmt: &TirStmt, in_loop: bool) {
        match stmt {
            TirStmt::Let { pattern, init, .. } => {
                // `init` is evaluated in the OUTER scope — the new binding is
                // not yet in effect while its own initializer runs.
                self.visit_expr(init, in_loop);
                self.bind_pattern(pattern);
            }
            TirStmt::Assign { value, .. } => {
                // LValue target is intentionally not visited: assignment is a write,
                // not a read.  Last-use analysis tracks read uses only — the value
                // currently bound to the name is consumed by the RHS expression, not
                // by being written to.
                self.visit_expr(value, in_loop);
            }
            TirStmt::Return { value, .. } => {
                if let Some(e) = value {
                    self.visit_expr(e, in_loop);
                }
            }
            TirStmt::If {
                cond, then, else_, ..
            } => {
                self.visit_expr(cond, in_loop);
                self.visit_block(then, in_loop);
                if let Some(else_branch) = else_ {
                    match else_branch {
                        TirElseBranch::Block(b) => self.visit_block(b, in_loop),
                        TirElseBranch::If(s) => self.visit_stmt(s, in_loop),
                    }
                }
            }
            TirStmt::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee, in_loop);
                for arm in arms {
                    self.push_scope();
                    self.bind_pattern(&arm.pattern);
                    match &arm.body {
                        TirMatchBody::Expr(e) => self.visit_expr(e, in_loop),
                        TirMatchBody::Block(b) => self.visit_block(b, in_loop),
                    }
                    self.pop_scope();
                }
            }
            TirStmt::For {
                pattern,
                iter,
                body,
                ..
            } => {
                // The iterable expression is evaluated once outside the loop,
                // in the outer scope — the loop variable isn't bound yet.
                self.visit_expr(iter, in_loop);
                // Loop body executes 0..N times — conservatively exclude from last-use.
                self.push_scope();
                self.bind_pattern(pattern);
                for stmt in &body.stmts {
                    self.visit_stmt(stmt, true);
                }
                self.pop_scope();
            }
            TirStmt::While { cond, body, .. } => {
                // Both condition and body execute repeatedly.
                self.visit_expr(cond, true);
                self.visit_block(body, true);
            }
            TirStmt::Expr { expr, .. } => self.visit_expr(expr, in_loop),
        }
    }

    fn visit_select_arm(&mut self, arm: &TirSelectArm, in_loop: bool) {
        // The receive/timeout expression is evaluated in the outer scope —
        // `binding` (the received value) isn't in effect for it.
        self.visit_expr(&arm.expr, in_loop);
        self.push_scope();
        if let Some(name) = &arm.binding {
            self.bind(name);
        }
        self.visit_block(&arm.body, in_loop);
        self.pop_scope();
    }

    fn visit_expr(&mut self, expr: &TirExpr, in_loop: bool) {
        match &expr.kind {
            TirExprKind::Var(name) => {
                self.record(name, expr.span, in_loop);
            }
            TirExprKind::Literal(_) => {}
            TirExprKind::FieldAccess { expr: inner, .. } => self.visit_expr(inner, in_loop),
            TirExprKind::MethodCall { receiver, args, .. } => {
                self.visit_expr(receiver, in_loop);
                for arg in args {
                    self.visit_expr(arg, in_loop);
                }
            }
            TirExprKind::FnCall { args, .. } => {
                for arg in args {
                    self.visit_expr(arg, in_loop);
                }
            }
            TirExprKind::Unary { expr: inner, .. }
            | TirExprKind::Propagate(inner)
            | TirExprKind::Consume(inner)
            | TirExprKind::Relabel { expr: inner, .. }
            | TirExprKind::Borrow { expr: inner, .. } => self.visit_expr(inner, in_loop),
            TirExprKind::Binary { left, right, .. } => {
                self.visit_expr(left, in_loop);
                self.visit_expr(right, in_loop);
            }
            TirExprKind::If {
                cond, then, else_, ..
            } => {
                self.visit_expr(cond, in_loop);
                self.visit_block(then, in_loop);
                if let Some(else_expr) = else_ {
                    self.visit_expr(else_expr, in_loop);
                }
            }
            TirExprKind::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee, in_loop);
                for arm in arms {
                    self.push_scope();
                    self.bind_pattern(&arm.pattern);
                    match &arm.body {
                        TirMatchBody::Expr(e) => self.visit_expr(e, in_loop),
                        TirMatchBody::Block(b) => self.visit_block(b, in_loop),
                    }
                    self.pop_scope();
                }
            }
            TirExprKind::Block(b) => self.visit_block(b, in_loop),
            // Lambdas capture variables — the body may be called multiple times.
            TirExprKind::Lambda { .. } => {}
            TirExprKind::Construct { fields, .. } | TirExprKind::Spawn { fields, .. } => {
                for (_, e) in fields {
                    self.visit_expr(e, in_loop);
                }
            }
            TirExprKind::Select { arms, .. } => {
                for arm in arms {
                    self.visit_select_arm(arm, in_loop);
                }
            }
            TirExprKind::List { elems } | TirExprKind::Set { elems } => {
                for e in elems {
                    self.visit_expr(e, in_loop);
                }
            }
            TirExprKind::Map { pairs } => {
                for (k, v) in pairs {
                    self.visit_expr(k, in_loop);
                    self.visit_expr(v, in_loop);
                }
            }
            // Quantifier predicates are contract-only — no identifiers to track.
            TirExprKind::Quantifier(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::checker::types::Ty;
    use crate::mvl::ir::TirMatchArm;
    use crate::mvl::parser::ast::RefExpr;

    fn sp(line: u32) -> Span {
        Span::new(line, 1, line as u64 as u32, 1)
    }

    fn var(name: &str, line: u32) -> TirExpr {
        TirExpr {
            kind: TirExprKind::Var(name.to_string()),
            ty: Ty::String,
            span: sp(line),
        }
    }

    fn call(name: &str, args: Vec<TirExpr>, line: u32) -> TirExpr {
        TirExpr {
            kind: TirExprKind::FnCall {
                name: name.to_string(),
                args,
                type_args: vec![],
            },
            ty: Ty::Unit,
            span: sp(line),
        }
    }

    fn let_stmt(name: &str, init: TirExpr, line: u32) -> TirStmt {
        TirStmt::Let {
            kind: crate::mvl::parser::ast::LetKind::Regular,
            pattern: Pattern::Ident(name.to_string(), sp(line)),
            ty: Ty::String,
            init,
            span: sp(line),
        }
    }

    fn expr_stmt(expr: TirExpr, line: u32) -> TirStmt {
        TirStmt::Expr {
            expr,
            span: sp(line),
        }
    }

    fn block(stmts: Vec<TirStmt>) -> TirBlock {
        TirBlock { stmts, span: sp(0) }
    }

    /// Reproduces the shadowing shape from #2021:
    ///
    /// ```mvl
    /// let s = a();
    /// { let s = b(); f(s); }
    /// g(s);
    /// ```
    ///
    /// The inner `f(s)` is the inner binding's own last use and must survive
    /// even though `g(s)` — a different binding — appears later in the walk.
    #[test]
    fn shadowed_binding_keeps_its_own_last_use() {
        let inner_call = call("f", vec![var("s", 3)], 3);
        let inner_use_span = match &inner_call.kind {
            TirExprKind::FnCall { args, .. } => args[0].span,
            _ => unreachable!(),
        };

        let outer_call = call("g", vec![var("s", 5)], 5);
        let outer_use_span = match &outer_call.kind {
            TirExprKind::FnCall { args, .. } => args[0].span,
            _ => unreachable!(),
        };

        let body = block(vec![
            let_stmt("s", call("a", vec![], 1), 1),
            TirStmt::Expr {
                expr: TirExpr {
                    kind: TirExprKind::Block(block(vec![
                        let_stmt("s", call("b", vec![], 2), 2),
                        expr_stmt(inner_call, 3),
                    ])),
                    ty: Ty::Unit,
                    span: sp(2),
                },
                span: sp(2),
            },
            expr_stmt(outer_call, 5),
        ]);

        let last_uses = compute_last_uses(&body);

        assert!(
            last_uses.contains(&inner_use_span),
            "inner shadowed binding's last use (f(s) at line 3) must be in the set"
        );
        assert!(
            last_uses.contains(&outer_use_span),
            "outer binding's last use (g(s) at line 5) must be in the set"
        );
    }

    /// A match arm's pattern-bound name must not be conflated with an
    /// unrelated outer variable of the same name.
    #[test]
    fn match_arm_binding_does_not_leak_into_outer_scope() {
        let outer_read = var("v", 1);
        let outer_span = outer_read.span;

        let arm_read = call("h", vec![var("v", 3)], 3);
        let arm_use_span = match &arm_read.kind {
            TirExprKind::FnCall { args, .. } => args[0].span,
            _ => unreachable!(),
        };

        let body = block(vec![
            let_stmt("v", call("a", vec![], 0), 0),
            expr_stmt(outer_read, 1),
            TirStmt::Match {
                scrutinee: call("opt", vec![], 2),
                arms: vec![TirMatchArm {
                    pattern: Pattern::Some {
                        inner: Box::new(Pattern::Ident("v".to_string(), sp(3))),
                        span: sp(3),
                    },
                    guard: None::<RefExpr>,
                    body: TirMatchBody::Expr(arm_read),
                    span: sp(3),
                }],
                span: sp(2),
            },
        ]);

        let last_uses = compute_last_uses(&body);

        assert!(
            last_uses.contains(&outer_span),
            "outer `v`'s only read is its own last use"
        );
        assert!(
            last_uses.contains(&arm_use_span),
            "arm-bound `v`'s read inside the arm is its own last use"
        );
    }
}
