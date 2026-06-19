// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR visitor trait and default walk functions.
//!
//! Mirrors [`crate::mvl::parser::visit`] for the Typed Intermediate
//! Representation.  Implement [`Visit`] to traverse TIR without writing
//! your own recursive block/stmt/expr triples.
//!
//! # Short-circuiting
//!
//! Override `visit_tir_expr` **without** calling `walk_tir_expr` to stop
//! descent at that node (e.g. when a path has been captured).

use super::{TirBlock, TirElseBranch, TirExpr, TirExprKind, TirMatchBody, TirStmt};

pub trait Visit<'a> {
    fn visit_tir_block(&mut self, b: &'a TirBlock) {
        walk_tir_block(self, b);
    }
    fn visit_tir_stmt(&mut self, s: &'a TirStmt) {
        walk_tir_stmt(self, s);
    }
    fn visit_tir_expr(&mut self, e: &'a TirExpr) {
        walk_tir_expr(self, e);
    }
}

pub fn walk_tir_block<'a, V: Visit<'a> + ?Sized>(v: &mut V, b: &'a TirBlock) {
    for s in &b.stmts {
        v.visit_tir_stmt(s);
    }
}

pub fn walk_tir_stmt<'a, V: Visit<'a> + ?Sized>(v: &mut V, s: &'a TirStmt) {
    match s {
        TirStmt::Let { init, .. } => v.visit_tir_expr(init),
        TirStmt::Assign { value, .. } => v.visit_tir_expr(value),
        TirStmt::Return { value: Some(e), .. } => v.visit_tir_expr(e),
        TirStmt::Return { value: None, .. } => {}
        TirStmt::If { cond, then, else_, .. } => {
            v.visit_tir_expr(cond);
            v.visit_tir_block(then);
            match else_ {
                Some(TirElseBranch::Block(b)) => v.visit_tir_block(b),
                Some(TirElseBranch::If(s)) => v.visit_tir_stmt(s),
                None => {}
            }
        }
        TirStmt::Match { scrutinee, arms, .. } => {
            v.visit_tir_expr(scrutinee);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Block(b) => v.visit_tir_block(b),
                    TirMatchBody::Expr(e) => v.visit_tir_expr(e),
                }
            }
        }
        TirStmt::While { cond, body, .. } => {
            v.visit_tir_expr(cond);
            v.visit_tir_block(body);
        }
        TirStmt::For { iter, body, .. } => {
            v.visit_tir_expr(iter);
            v.visit_tir_block(body);
        }
        TirStmt::Expr { expr, .. } => v.visit_tir_expr(expr),
    }
}

pub fn walk_tir_expr<'a, V: Visit<'a> + ?Sized>(v: &mut V, e: &'a TirExpr) {
    match &e.kind {
        TirExprKind::Literal(_)
        | TirExprKind::Var(_)
        | TirExprKind::FieldAccess { .. }
        | TirExprKind::Quantifier(_) => {}
        TirExprKind::Unary { expr: inner, .. }
        | TirExprKind::Propagate(inner)
        | TirExprKind::Consume(inner)
        | TirExprKind::Relabel { expr: inner, .. }
        | TirExprKind::Borrow { expr: inner, .. } => v.visit_tir_expr(inner),
        TirExprKind::Binary { left, right, .. } => {
            v.visit_tir_expr(left);
            v.visit_tir_expr(right);
        }
        TirExprKind::If { cond, then, else_, .. } => {
            v.visit_tir_expr(cond);
            v.visit_tir_block(then);
            if let Some(e) = else_ {
                v.visit_tir_expr(e);
            }
        }
        TirExprKind::Match { scrutinee, arms, .. } => {
            v.visit_tir_expr(scrutinee);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Block(b) => v.visit_tir_block(b),
                    TirMatchBody::Expr(e) => v.visit_tir_expr(e),
                }
            }
        }
        TirExprKind::Block(b) => v.visit_tir_block(b),
        TirExprKind::Lambda { body, .. } => v.visit_tir_expr(body),
        TirExprKind::FnCall { args, .. } => {
            for a in args {
                v.visit_tir_expr(a);
            }
        }
        TirExprKind::MethodCall { receiver, args, .. } => {
            v.visit_tir_expr(receiver);
            for a in args {
                v.visit_tir_expr(a);
            }
        }
        TirExprKind::Construct { fields, .. } | TirExprKind::Spawn { fields, .. } => {
            for (_, e) in fields {
                v.visit_tir_expr(e);
            }
        }
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            for e in elems {
                v.visit_tir_expr(e);
            }
        }
        TirExprKind::Map { pairs } => {
            for (k, val) in pairs {
                v.visit_tir_expr(k);
                v.visit_tir_expr(val);
            }
        }
        TirExprKind::Select { arms } => {
            for arm in arms {
                v.visit_tir_expr(&arm.expr);
                v.visit_tir_block(&arm.body);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::ir::{TirBlock, TirStmt};

    fn empty_block() -> TirBlock {
        TirBlock {
            stmts: vec![],
            span: crate::mvl::parser::lexer::Span::default(),
        }
    }

    #[derive(Default)]
    struct NodeCounter {
        blocks: usize,
        stmts: usize,
        exprs: usize,
    }

    impl<'a> Visit<'a> for NodeCounter {
        fn visit_tir_block(&mut self, b: &'a TirBlock) {
            self.blocks += 1;
            walk_tir_block(self, b);
        }
        fn visit_tir_stmt(&mut self, s: &'a TirStmt) {
            self.stmts += 1;
            walk_tir_stmt(self, s);
        }
        fn visit_tir_expr(&mut self, e: &'a TirExpr) {
            self.exprs += 1;
            walk_tir_expr(self, e);
        }
    }

    #[test]
    fn visit_empty_block_counts_nothing() {
        let block = empty_block();
        let mut counter = NodeCounter::default();
        walk_tir_block(&mut counter, &block);
        assert_eq!(counter.blocks, 0);
        assert_eq!(counter.stmts, 0);
        assert_eq!(counter.exprs, 0);
    }

    #[test]
    fn visit_short_circuit_stops_descent() {
        use crate::mvl::ir::{BinaryOp, Literal, TirExpr, TirExprKind, Ty};
        use crate::mvl::parser::lexer::Span;

        let span = Span::default();
        let lit = TirExpr {
            kind: TirExprKind::Literal(Literal::Integer(1)),
            ty: Ty::Int,
            span,
        };
        let binary = TirExpr {
            kind: TirExprKind::Binary {
                op: BinaryOp::Add,
                left: Box::new(lit.clone()),
                right: Box::new(lit),
            },
            ty: Ty::Int,
            span,
        };
        let block = TirBlock {
            stmts: vec![TirStmt::Expr { expr: binary, span }],
            span,
        };

        struct TopOnly(usize);
        impl<'a> Visit<'a> for TopOnly {
            fn visit_tir_expr(&mut self, _e: &'a TirExpr) {
                self.0 += 1;
                // intentionally no walk_tir_expr — stops descent
            }
        }

        let mut v = TopOnly(0);
        walk_tir_block(&mut v, &block);
        assert_eq!(v.0, 1, "short-circuit must see only the top-level binary expr, not its children");
    }
}
