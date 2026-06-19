// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! AST visitor trait and default walk functions.
//!
//! Implement [`Visit`] to traverse the AST without writing your own recursive
//! block/stmt/expr triples.  Override only the methods you need; the default
//! implementations call the corresponding `walk_*` function which recursively
//! visits all children.
//!
//! # Short-circuiting
//!
//! Override `visit_expr` (or `visit_stmt` / `visit_block`) **without** calling
//! the matching `walk_*` function to stop descent at that node.

use super::ast::{Block, ElseBranch, Expr, MatchBody, Stmt};

pub trait Visit<'a> {
    fn visit_block(&mut self, b: &'a Block) {
        walk_block(self, b);
    }
    fn visit_stmt(&mut self, s: &'a Stmt) {
        walk_stmt(self, s);
    }
    fn visit_expr(&mut self, e: &'a Expr) {
        walk_expr(self, e);
    }
}

pub fn walk_block<'a, V: Visit<'a> + ?Sized>(v: &mut V, b: &'a Block) {
    for s in &b.stmts {
        v.visit_stmt(s);
    }
}

pub fn walk_stmt<'a, V: Visit<'a> + ?Sized>(v: &mut V, s: &'a Stmt) {
    match s {
        Stmt::Let { init, .. } => v.visit_expr(init),
        Stmt::Assign { value, .. } => v.visit_expr(value),
        Stmt::Return { value: Some(e), .. } => v.visit_expr(e),
        Stmt::Return { value: None, .. } => {}
        Stmt::If {
            cond, then, else_, ..
        } => {
            v.visit_expr(cond);
            v.visit_block(then);
            match else_ {
                Some(ElseBranch::Block(b)) => v.visit_block(b),
                Some(ElseBranch::If(s)) => v.visit_stmt(s),
                None => {}
            }
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            v.visit_expr(scrutinee);
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => v.visit_block(b),
                    MatchBody::Expr(e) => v.visit_expr(e),
                }
            }
        }
        Stmt::While { cond, body, .. } => {
            v.visit_expr(cond);
            v.visit_block(body);
        }
        Stmt::For { iter, body, .. } => {
            v.visit_expr(iter);
            v.visit_block(body);
        }
        Stmt::Expr { expr, .. } => v.visit_expr(expr),
    }
}

pub fn walk_expr<'a, V: Visit<'a> + ?Sized>(v: &mut V, e: &'a Expr) {
    match e {
        Expr::Literal(..) | Expr::Ident(..) | Expr::Quantifier(..) => {}
        Expr::FieldAccess { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Relabel { expr, .. }
        | Expr::Borrow { expr, .. }
        | Expr::As { expr, .. }
        | Expr::Unary { expr, .. } => v.visit_expr(expr),
        Expr::Binary { left, right, .. } => {
            v.visit_expr(left);
            v.visit_expr(right);
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            v.visit_expr(cond);
            v.visit_block(then);
            if let Some(e) = else_ {
                v.visit_expr(e);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            v.visit_expr(scrutinee);
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => v.visit_block(b),
                    MatchBody::Expr(e) => v.visit_expr(e),
                }
            }
        }
        Expr::Block(b) => v.visit_block(b),
        Expr::Lambda { body, .. } => v.visit_expr(body),
        Expr::FnCall { args, .. } => {
            for arg in args {
                v.visit_expr(arg);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            v.visit_expr(receiver);
            for arg in args {
                v.visit_expr(arg);
            }
        }
        Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => {
            for (_, e) in fields {
                v.visit_expr(e);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                v.visit_expr(e);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, val) in pairs {
                v.visit_expr(k);
                v.visit_expr(val);
            }
        }
        Expr::Select { arms, .. } => {
            for arm in arms {
                v.visit_expr(&arm.expr);
                v.visit_block(&arm.body);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn parse_block(src: &str) -> Block {
        let wrapped = format!("fn f() -> Unit {{ {src} }}");
        let (mut p, _) = Parser::new(&wrapped);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        match prog.declarations.into_iter().next().unwrap() {
            crate::mvl::parser::ast::Decl::Fn(f) => f.body,
            _ => panic!("expected fn"),
        }
    }

    #[derive(Default)]
    struct NodeCounter {
        blocks: usize,
        stmts: usize,
        exprs: usize,
    }

    impl<'a> Visit<'a> for NodeCounter {
        fn visit_block(&mut self, b: &'a Block) {
            self.blocks += 1;
            walk_block(self, b);
        }
        fn visit_stmt(&mut self, s: &'a Stmt) {
            self.stmts += 1;
            walk_stmt(self, s);
        }
        fn visit_expr(&mut self, e: &'a Expr) {
            self.exprs += 1;
            walk_expr(self, e);
        }
    }

    #[test]
    fn visit_counts_nodes_in_simple_fn() {
        // fn body: `let x: Int = 1; x`
        let block = parse_block("let x: Int = 1; x");
        let mut counter = NodeCounter::default();
        walk_block(&mut counter, &block);
        assert_eq!(counter.stmts, 2); // let + expr
        assert!(counter.exprs >= 2); // 1 (literal) + x (ident)
    }

    #[test]
    fn visit_short_circuit_stops_descent() {
        // Override visit_expr to count top-level exprs only (no recursion)
        struct TopLevelExprs(usize);
        impl<'a> Visit<'a> for TopLevelExprs {
            fn visit_expr(&mut self, _e: &'a Expr) {
                self.0 += 1;
                // intentionally no walk_expr — stops descent
            }
        }

        let block = parse_block("let x: Int = 1 + 2;");
        let mut v = TopLevelExprs(0);
        walk_block(&mut v, &block);
        assert_eq!(
            v.0, 1,
            "should see only the top-level init expr, not sub-exprs"
        );
    }
}
