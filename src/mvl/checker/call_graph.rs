// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Whole-program call graph — topology of function calls across all modules.
//!
//! # Design
//!
//! The call graph is a simple directed graph:
//! - **Nodes**: every function name known to the type environment.
//! - **Edges**: `(caller, callee, call_span)` — one edge per `FnCall` expression
//!   found in a function body.
//!
//! MVL has no virtual dispatch, no function pointers, and full monomorphization
//! (currently implicit in codegen, tracked for explicit pass in #837/#838).
//! After type checking, every `Expr::FnCall { name, .. }` resolves to exactly
//! one callee — the call graph is a precise, syntactic AST walk.  No
//! points-to analysis or CHA needed.
//!
//! `MethodCall` (receiver.method syntax) edges are not yet recorded because
//! resolving the callee requires receiver-type lookup; that will be added once
//! monomorphization (#838) is a distinct pass.
//!
//! # Usage by downstream passes
//!
//! ```text
//! // IFC forward propagation (#830)
//! for edge in graph.callees("build_query") { ... }
//!
//! // Termination / mutual-recursion detection
//! if graph.reachable("f", "f") { /* f is recursive */ }
//! ```
//!
//! # References
//! - #829 — this feature
//! - #825 — interprocedural IFC epic (primary consumer)
//! - #837/#838 — ADR + impl for explicit monomorphization pass

use std::collections::{HashMap, HashSet, VecDeque};

use crate::mvl::checker::context::TypeEnv;
use crate::mvl::parser::ast::{Block, Decl, ElseBranch, Expr, MatchBody, Program, Stmt};
use crate::mvl::parser::lexer::Span;

// ── Public types ──────────────────────────────────────────────────────────────

/// A single call-site edge in the call graph.
#[derive(Debug, Clone)]
pub struct CallEdge {
    /// Name of the function being called.
    pub callee: String,
    /// Source location of the call expression.
    pub call_span: Span,
}

/// Whole-program call graph.
///
/// Built once after type checking; stored in [`CheckResult`] for use by
/// verification passes.
#[derive(Debug, Clone, Default)]
pub struct CallGraph {
    /// All function names known to the type environment (nodes).
    nodes: HashSet<String>,
    /// Outgoing edges per caller: `edges[caller] = [(callee, span), ...]`.
    edges: HashMap<String, Vec<CallEdge>>,
}

impl CallGraph {
    // ── Queries ───────────────────────────────────────────────────────────────

    /// Returns the call edges outgoing from `fn_name` (i.e. functions it calls).
    pub fn callees(&self, fn_name: &str) -> &[CallEdge] {
        self.edges.get(fn_name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Returns all function names that directly call `fn_name`.
    pub fn callers(&self, fn_name: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|(_, edges)| edges.iter().any(|e| e.callee == fn_name))
            .map(|(caller, _)| caller.as_str())
            .collect()
    }

    /// Returns `true` if `to` is reachable from `from` via call edges (BFS).
    ///
    /// `reachable("f", "f")` returns `true` when `f` is directly or mutually
    /// recursive.
    pub fn reachable(&self, from: &str, to: &str) -> bool {
        let mut visited: HashSet<&str> = HashSet::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        visited.insert(from);
        queue.push_back(from);
        while let Some(current) = queue.pop_front() {
            for edge in self.callees(current) {
                let callee = edge.callee.as_str();
                if callee == to {
                    return true;
                }
                if visited.insert(callee) {
                    queue.push_back(callee);
                }
            }
        }
        false
    }

    /// All function names that are nodes in the graph.
    pub fn nodes(&self) -> impl Iterator<Item = &str> {
        self.nodes.iter().map(String::as_str)
    }

    /// Whether `fn_name` is a known node in the graph.
    pub fn contains(&self, fn_name: &str) -> bool {
        self.nodes.contains(fn_name)
    }
}

// ── Construction ──────────────────────────────────────────────────────────────

/// Build the call graph from all parsed programs and the resolved type environment.
///
/// `programs` should include every program visible to the checker (stdlib prelude
/// slices + user modules), so that cross-module call chains are captured.
///
/// Called from `checker::check_with_two_preludes_mode` after type checking
/// completes, while both the type environment and the parsed ASTs are available.
pub fn build(programs: &[&Program], type_env: &TypeEnv) -> CallGraph {
    let mut graph = CallGraph::default();

    // Seed nodes from every known function in the type environment.
    for name in type_env.fns.keys() {
        graph.nodes.insert(name.clone());
    }

    // Walk every function body in every program to collect call edges.
    for prog in programs {
        collect_program_edges(prog, &mut graph);
    }

    graph
}

// ── AST walk ─────────────────────────────────────────────────────────────────

fn collect_program_edges(prog: &Program, graph: &mut CallGraph) {
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                graph.nodes.insert(fd.name.clone());
                collect_block_edges(&fd.body, &fd.name, graph);
            }
            Decl::Impl(id) => {
                for method in &id.methods {
                    graph.nodes.insert(method.name.clone());
                    collect_block_edges(&method.body, &method.name, graph);
                }
            }
            Decl::Actor(ad) => {
                for method in &ad.methods {
                    graph.nodes.insert(method.name.clone());
                    collect_block_edges(&method.body, &method.name, graph);
                }
            }
            _ => {}
        }
    }
}

fn collect_block_edges(block: &Block, caller: &str, graph: &mut CallGraph) {
    for stmt in &block.stmts {
        collect_stmt_edges(stmt, caller, graph);
    }
}

fn collect_stmt_edges(stmt: &Stmt, caller: &str, graph: &mut CallGraph) {
    match stmt {
        Stmt::Let { init, .. } => collect_expr_edges(init, caller, graph),
        Stmt::Assign { value, .. } => collect_expr_edges(value, caller, graph),
        Stmt::Return { value: Some(e), .. } => collect_expr_edges(e, caller, graph),
        Stmt::Return { value: None, .. } => {}
        Stmt::Expr { expr, .. } => collect_expr_edges(expr, caller, graph),
        Stmt::If {
            cond, then, else_, ..
        } => {
            collect_expr_edges(cond, caller, graph);
            collect_block_edges(then, caller, graph);
            match else_ {
                Some(ElseBranch::Block(b)) => collect_block_edges(b, caller, graph),
                Some(ElseBranch::If(s)) => collect_stmt_edges(s, caller, graph),
                None => {}
            }
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_edges(scrutinee, caller, graph);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => collect_expr_edges(e, caller, graph),
                    MatchBody::Block(b) => collect_block_edges(b, caller, graph),
                }
            }
        }
        Stmt::For { iter, body, .. } => {
            collect_expr_edges(iter, caller, graph);
            collect_block_edges(body, caller, graph);
        }
        Stmt::While { cond, body, .. } => {
            collect_expr_edges(cond, caller, graph);
            collect_block_edges(body, caller, graph);
        }
    }
}

fn collect_expr_edges(expr: &Expr, caller: &str, graph: &mut CallGraph) {
    match expr {
        Expr::FnCall {
            name, args, span, ..
        } => {
            // Record the call edge.
            graph
                .edges
                .entry(caller.to_string())
                .or_default()
                .push(CallEdge {
                    callee: name.clone(),
                    call_span: *span,
                });
            // Also ensure the callee is a node (may not be in type_env if the
            // call is to an unresolved or prelude name not yet registered).
            graph.nodes.insert(name.clone());
            for arg in args {
                collect_expr_edges(arg, caller, graph);
            }
        }
        // MethodCall: callee cannot be statically determined without receiver
        // type resolution — tracked for post-monomorphization (#838).
        Expr::MethodCall { receiver, args, .. } => {
            collect_expr_edges(receiver, caller, graph);
            for arg in args {
                collect_expr_edges(arg, caller, graph);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_edges(scrutinee, caller, graph);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => collect_expr_edges(e, caller, graph),
                    MatchBody::Block(b) => collect_block_edges(b, caller, graph),
                }
            }
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            collect_expr_edges(cond, caller, graph);
            collect_block_edges(then, caller, graph);
            if let Some(e) = else_ {
                collect_expr_edges(e, caller, graph);
            }
        }
        Expr::Block(b) => collect_block_edges(b, caller, graph),
        Expr::Binary { left, right, .. } => {
            collect_expr_edges(left, caller, graph);
            collect_expr_edges(right, caller, graph);
        }
        Expr::Unary { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Declassify { expr, .. }
        | Expr::Sanitize { expr, .. }
        | Expr::Borrow { expr, .. } => collect_expr_edges(expr, caller, graph),
        Expr::FieldAccess { expr, .. } => collect_expr_edges(expr, caller, graph),
        Expr::Construct { fields, .. } => {
            for (_, e) in fields {
                collect_expr_edges(e, caller, graph);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                collect_expr_edges(e, caller, graph);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                collect_expr_edges(k, caller, graph);
                collect_expr_edges(v, caller, graph);
            }
        }
        Expr::Lambda { body, .. } => collect_expr_edges(body, caller, graph),
        Expr::Spawn { fields, .. } => {
            for (_, e) in fields {
                collect_expr_edges(e, caller, graph);
            }
        }
        Expr::Select { arms, .. } => {
            for arm in arms {
                collect_expr_edges(&arm.expr, caller, graph);
                collect_block_edges(&arm.body, caller, graph);
            }
        }
        Expr::Concurrently { body, .. } => collect_block_edges(body, caller, graph),
        // Leaf expressions — no sub-expressions.
        Expr::Literal(..) | Expr::Ident(..) => {}
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn graph_from(src: &str) -> CallGraph {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let env = TypeEnv::default();
        build(&[&prog], &env)
    }

    #[test]
    fn empty_program_produces_empty_graph() {
        let g = graph_from("");
        assert!(g.callees("anything").is_empty());
        assert!(!g.reachable("a", "b"));
    }

    #[test]
    fn direct_call_edge() {
        let g = graph_from("fn a() -> Unit { b() } fn b() -> Unit { }");
        let callees: Vec<&str> = g.callees("a").iter().map(|e| e.callee.as_str()).collect();
        assert!(callees.contains(&"b"), "a should call b, got {callees:?}");
    }

    #[test]
    fn chain_a_calls_b_calls_c() {
        let g = graph_from("fn a() -> Unit { b() } fn b() -> Unit { c() } fn c() -> Unit { }");
        assert!(g.reachable("a", "c"), "a should reach c via b");
        assert!(!g.reachable("c", "a"), "c should not reach a");
    }

    #[test]
    fn direct_recursion() {
        let g = graph_from("fn f() -> Unit { f() }");
        assert!(g.reachable("f", "f"), "f should be reachable from itself");
    }

    #[test]
    fn mutual_recursion() {
        let g = graph_from("fn a() -> Unit { b() } fn b() -> Unit { a() }");
        assert!(g.reachable("a", "a"), "a should be reachable from a via b");
        assert!(g.reachable("b", "b"), "b should be reachable from b via a");
    }

    #[test]
    fn callers_lookup() {
        let g = graph_from(
            "fn main() -> Unit { foo() bar() } fn foo() -> Unit { } fn bar() -> Unit { }",
        );
        let callers = g.callers("foo");
        assert!(callers.contains(&"main"), "main should call foo");
    }

    #[test]
    fn nodes_include_called_functions() {
        let g = graph_from("fn caller() -> Unit { callee() }");
        assert!(g.contains("caller"));
        assert!(
            g.contains("callee"),
            "callee should be a node even if not declared"
        );
    }

    #[test]
    fn reachable_cycle_unreachable_target_does_not_loop() {
        // a→b→c→a cycle; x is not in the graph — must terminate and return false.
        let g = graph_from("fn a() -> Unit { b() } fn b() -> Unit { c() } fn c() -> Unit { a() }");
        assert!(!g.reachable("a", "x"), "x is not reachable from a");
        // Reachability within the cycle still works.
        assert!(g.reachable("a", "c"), "a should reach c via b");
        assert!(g.reachable("c", "b"), "c should reach b via a");
    }

    #[test]
    fn callers_multiple() {
        let g = graph_from(
            "fn main() -> Unit { foo() } fn other() -> Unit { foo() } fn foo() -> Unit { }",
        );
        let callers = g.callers("foo");
        assert!(callers.contains(&"main"), "main should call foo");
        assert!(callers.contains(&"other"), "other should call foo");
    }
}
