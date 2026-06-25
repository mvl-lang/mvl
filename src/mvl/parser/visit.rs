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

// ── Walkers ───────────────────────────────────────────────────────────────────
//
// `..` is intentionally avoided: every struct-variant pattern explicitly binds
// every field (using `field: _` for fields the walker does not descend into).
// When the AST grows a new field, the match stops compiling, forcing a
// deliberate include/exclude decision at the walker level rather than a silent
// default-to-skip.  See #1527 for the original symptom (loop `invariants` /
// `decreases` silently skipped under `..`).

pub fn walk_block<'a, V: Visit<'a> + ?Sized>(v: &mut V, b: &'a Block) {
    let Block { stmts, span: _ } = b;
    for s in stmts {
        v.visit_stmt(s);
    }
}

pub fn walk_stmt<'a, V: Visit<'a> + ?Sized>(v: &mut V, s: &'a Stmt) {
    match s {
        Stmt::Let {
            kind: _,
            pattern: _,
            ty: _,
            init,
            span: _,
        } => v.visit_expr(init),
        Stmt::Assign {
            target: _,
            value,
            span: _,
        } => v.visit_expr(value),
        Stmt::Return {
            value: Some(e),
            span: _,
        } => v.visit_expr(e),
        Stmt::Return {
            value: None,
            span: _,
        } => {}
        Stmt::If {
            cond,
            then,
            else_,
            span: _,
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
            scrutinee,
            arms,
            span: _,
        } => {
            v.visit_expr(scrutinee);
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => v.visit_block(b),
                    MatchBody::Expr(e) => v.visit_expr(e),
                }
            }
        }
        Stmt::While {
            cond,
            invariants,
            decreases,
            body,
            span: _,
        } => {
            v.visit_expr(cond);
            for inv in invariants {
                v.visit_expr(inv);
            }
            if let Some(dec) = decreases {
                v.visit_expr(dec);
            }
            v.visit_block(body);
        }
        Stmt::For {
            pattern: _,
            iter,
            invariants,
            body,
            span: _,
        } => {
            v.visit_expr(iter);
            for inv in invariants {
                v.visit_expr(inv);
            }
            v.visit_block(body);
        }
        Stmt::Expr { expr, span: _ } => v.visit_expr(expr),
    }
}

pub fn walk_expr<'a, V: Visit<'a> + ?Sized>(v: &mut V, e: &'a Expr) {
    match e {
        Expr::Literal(_, _) | Expr::Ident(_, _) | Expr::Quantifier(_, _) => {}
        Expr::FieldAccess {
            expr,
            field: _,
            span: _,
        } => v.visit_expr(expr),
        Expr::Propagate { expr, span: _ } => v.visit_expr(expr),
        Expr::Consume { expr, span: _ } => v.visit_expr(expr),
        Expr::Relabel {
            name: _,
            expr,
            tag: _,
            audit: _,
            span: _,
        } => v.visit_expr(expr),
        Expr::Borrow {
            mutable: _,
            expr,
            span: _,
        } => v.visit_expr(expr),
        Expr::As {
            expr,
            target: _,
            span: _,
        } => v.visit_expr(expr),
        Expr::Unary {
            op: _,
            expr,
            span: _,
        } => v.visit_expr(expr),
        Expr::Binary {
            op: _,
            left,
            right,
            span: _,
        } => {
            v.visit_expr(left);
            v.visit_expr(right);
        }
        Expr::If {
            cond,
            then,
            else_,
            span: _,
        } => {
            v.visit_expr(cond);
            v.visit_block(then);
            if let Some(e) = else_ {
                v.visit_expr(e);
            }
        }
        Expr::Match {
            scrutinee,
            arms,
            span: _,
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
        Expr::Lambda {
            params: _,
            ret_type: _,
            body,
            span: _,
        } => v.visit_expr(body),
        Expr::FnCall {
            name: _,
            type_args: _,
            args,
            span: _,
        } => {
            for arg in args {
                v.visit_expr(arg);
            }
        }
        Expr::MethodCall {
            receiver,
            method: _,
            args,
            span: _,
        } => {
            v.visit_expr(receiver);
            for arg in args {
                v.visit_expr(arg);
            }
        }
        Expr::Construct {
            name: _,
            fields,
            span: _,
        } => {
            for (_, e) in fields {
                v.visit_expr(e);
            }
        }
        Expr::Spawn {
            actor_type: _,
            fields,
            span: _,
        } => {
            for (_, e) in fields {
                v.visit_expr(e);
            }
        }
        Expr::List { elems, span: _ } => {
            for e in elems {
                v.visit_expr(e);
            }
        }
        Expr::Set { elems, span: _ } => {
            for e in elems {
                v.visit_expr(e);
            }
        }
        Expr::Map { pairs, span: _ } => {
            for (k, val) in pairs {
                v.visit_expr(k);
                v.visit_expr(val);
            }
        }
        Expr::Select { arms, span: _ } => {
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

    /// Regression test for #1527: `walk_stmt` must descend into loop contract
    /// expressions (`while … invariant … decreases …`).  The original
    /// implementation silently skipped these via `..`, causing checker passes
    /// and linter rules to miss calls/decisions in contract clauses.
    #[test]
    fn walk_stmt_visits_while_invariants_and_decreases() {
        // Collect identifiers seen during traversal so we can assert which
        // contract sub-expressions were visited.
        #[derive(Default)]
        struct IdentCollector(Vec<String>);
        impl<'a> Visit<'a> for IdentCollector {
            fn visit_expr(&mut self, e: &'a Expr) {
                if let Expr::Ident(name, _) = e {
                    self.0.push(name.clone());
                }
                walk_expr(self, e);
            }
        }

        let block = parse_block(
            "let i: ref Int = 0;\n\
             while cond_var \n\
                 invariant inv_var \n\
                 decreases dec_var { \n\
                 i = i + 1;\n\
             }",
        );
        let mut v = IdentCollector::default();
        walk_block(&mut v, &block);
        assert!(v.0.contains(&"cond_var".to_string()), "cond visited");
        assert!(
            v.0.contains(&"inv_var".to_string()),
            "invariant visited — got {:?}",
            v.0
        );
        assert!(
            v.0.contains(&"dec_var".to_string()),
            "decreases visited — got {:?}",
            v.0
        );
    }

    /// Regression test for #1527: `walk_stmt` must descend into `for` loop
    /// invariants.
    #[test]
    fn walk_stmt_visits_for_invariants() {
        #[derive(Default)]
        struct IdentCollector(Vec<String>);
        impl<'a> Visit<'a> for IdentCollector {
            fn visit_expr(&mut self, e: &'a Expr) {
                if let Expr::Ident(name, _) = e {
                    self.0.push(name.clone());
                }
                walk_expr(self, e);
            }
        }

        let block = parse_block(
            "for x in iter_var \n\
                 invariant inv_var { \n\
                 let y: Int = x;\n\
             }",
        );
        let mut v = IdentCollector::default();
        walk_block(&mut v, &block);
        assert!(v.0.contains(&"iter_var".to_string()), "iter visited");
        assert!(
            v.0.contains(&"inv_var".to_string()),
            "invariant visited — got {:?}",
            v.0
        );
    }
}
