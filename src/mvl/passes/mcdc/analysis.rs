// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Static MC/DC obligation analysis.
//!
//! Walks the AST to identify all decisions (if/while conditions and match arms)
//! and counts their atomic boolean clauses. Results feed the obligation table
//! used by `cmd_mcdc` to verify MC/DC coverage.
//!
//! MC/DC (Modified Condition/Decision Coverage) requires that each atomic
//! boolean clause in a compound condition independently affects the decision
//! outcome. Required by DO-178C at DAL-A and ISO 26262 at ASIL-D.
//!
//! ## Decision types tracked
//!
//! | Kind         | Label     | Obligation per unit        | Notes                          |
//! |--------------|-----------|----------------------------|--------------------------------|
//! | `if` body    | `return`  | One per boolean clause     | Function whose body IS the cond|
//! | `if` cond    | `if`      | One per boolean clause     | Top-level and `else if` chains |
//! | `while` cond | `while`   | One per boolean clause     | Loop guard                     |
//! | `match` arms | `match`   | One per arm                | Each arm must be taken once    |
//! | match guard  | `guard`   | One per boolean clause     | `pat if a && b =>`              |
//!
//! ## What is NOT tracked
//!
//! - Single-clause conditions (no `&&`/`||`) — they have trivially one obligation
//! - Conditions inside macro calls or `extern "rust"` blocks
//! - LLVM backend — no MC/DC infrastructure in `src/mvl/codegen/`

use crate::mvl::parser::ast::{
    BinaryOp, Block, Decl, ElseBranch, Expr, LogicOp, MatchBody, Program, RefExpr, Stmt,
};
use crate::mvl::parser::visit::{walk_block, walk_stmt, Visit};

/// Identifies the kind of decision point.
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionKind {
    If,
    While,
    /// A `match` expression/statement — each arm is an independent outcome.
    Match,
    /// A compound guard condition on a match arm (`if cond` with `&&`/`||`).
    MatchGuard,
}

/// A single decision point in the source with its obligation metadata.
#[derive(Debug, Clone)]
pub struct DecisionInfo {
    /// Sequential ID matching the runtime instrumentation index.
    pub id: usize,
    /// Enclosing function name.
    pub fn_name: String,
    /// Source file stem.
    pub file: String,
    /// Source line (1-based).
    pub line: u32,
    /// Kind of decision point.
    pub kind: DecisionKind,
    /// Number of atomic boolean clauses (leaf nodes of the &&/|| tree).
    pub clause_count: usize,
    /// True when this decision is inside a function with `! effect` annotations.
    /// Shown in the EXEMPT tier and excluded from the coverage percentage denominator.
    pub is_effectful: bool,
}

impl DecisionInfo {
    /// Minimum number of test cases required for full MC/DC under optimal
    /// (independent-effect) test design: N clauses + 1 base case.
    ///
    /// Note: used in tests and reserved for future report output.
    /// This lower bound assumes optimal test design; coupled conditions may
    /// require more test cases in practice.
    pub fn min_tests(&self) -> usize {
        self.clause_count + 1
    }

    /// True when the condition has more than one atomic clause.
    pub fn is_compound(&self) -> bool {
        self.clause_count > 1
    }
}

/// Walk a program and collect all compound-condition decision points.
///
/// `file_stem` is the source file name without extension (for reporting).
/// `start_id` is the first decision ID to assign; use `0` for the first file
/// and `existing_decisions.len()` for subsequent files in a multi-file run.
///
/// Test functions are excluded — only production code decisions are tracked.
/// Single-clause conditions (no `&&`/`||`) are excluded — they carry no MC/DC
/// obligations and do not receive IDs from the transpiler.
pub fn analyze_mcdc(prog: &Program, file_stem: &str, start_id: usize) -> Vec<DecisionInfo> {
    let mut decisions = Vec::new();
    let mut next_id = start_id;
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            if fd.is_test {
                continue;
            }
            let before = decisions.len();
            collect_from_block(&fd.body, &fd.name, file_stem, &mut decisions, &mut next_id);
            if !fd.effects.is_empty() {
                for d in &mut decisions[before..] {
                    d.is_effectful = true;
                }
            }
        }
    }
    decisions
}

fn collect_from_block(
    block: &Block,
    fn_name: &str,
    file: &str,
    decisions: &mut Vec<DecisionInfo>,
    next_id: &mut usize,
) {
    let mut v = DecisionCollector {
        decisions,
        next_id,
        fn_name,
        file,
    };
    walk_block(&mut v, block);
}

struct DecisionCollector<'out, 'ctx> {
    decisions: &'out mut Vec<DecisionInfo>,
    next_id: &'out mut usize,
    fn_name: &'ctx str,
    file: &'ctx str,
}

impl<'out, 'ctx> DecisionCollector<'out, 'ctx> {
    fn push_decision(&mut self, line: u32, kind: DecisionKind, clause_count: usize) {
        self.decisions.push(DecisionInfo {
            id: *self.next_id,
            fn_name: self.fn_name.to_string(),
            file: self.file.to_string(),
            line,
            kind,
            clause_count,
            is_effectful: false,
        });
        *self.next_id += 1;
    }
}

impl<'ast> Visit<'ast> for DecisionCollector<'_, '_> {
    fn visit_stmt(&mut self, s: &'ast Stmt) {
        match s {
            Stmt::If {
                cond,
                then,
                else_,
                span,
            } => {
                let clause_count = count_clauses(cond);
                if clause_count > 1 {
                    self.push_decision(span.line, DecisionKind::If, clause_count);
                }
                self.visit_block(then);
                if let Some(else_branch) = else_ {
                    match else_branch {
                        ElseBranch::Block(b) => self.visit_block(b),
                        ElseBranch::If(s) => self.visit_stmt(s),
                    }
                }
            }
            Stmt::While {
                cond, body, span, ..
            } => {
                let clause_count = count_clauses(cond);
                if clause_count > 1 {
                    self.push_decision(span.line, DecisionKind::While, clause_count);
                }
                self.visit_block(body);
            }
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                // Ordering mirrors transpiler emission: scrutinee → match decision →
                // all guard decisions → arm bodies.
                self.visit_expr(scrutinee);
                if arms.len() >= 2 {
                    self.push_decision(span.line, DecisionKind::Match, arms.len());
                }
                for arm in arms.iter() {
                    if let Some(guard) = &arm.guard {
                        let n = count_clauses_ref(guard);
                        if n >= 2 {
                            self.push_decision(arm.span.line, DecisionKind::MatchGuard, n);
                        }
                    }
                }
                for arm in arms {
                    match &arm.body {
                        MatchBody::Block(b) => self.visit_block(b),
                        MatchBody::Expr(e) => self.visit_expr(e),
                    }
                }
            }
            Stmt::For { body, .. } => self.visit_block(body),
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &'ast Expr) {
        match e {
            Expr::If {
                cond,
                then,
                else_,
                span,
            } => {
                let clause_count = count_clauses(cond);
                if clause_count > 1 {
                    self.push_decision(span.line, DecisionKind::If, clause_count);
                }
                self.visit_block(then);
                if let Some(e) = else_ {
                    self.visit_expr(e);
                }
            }
            Expr::Match {
                scrutinee,
                arms,
                span,
                ..
            } => {
                self.visit_expr(scrutinee);
                if arms.len() >= 2 {
                    self.push_decision(span.line, DecisionKind::Match, arms.len());
                }
                for arm in arms.iter() {
                    if let Some(guard) = &arm.guard {
                        let n = count_clauses_ref(guard);
                        if n >= 2 {
                            self.push_decision(arm.span.line, DecisionKind::MatchGuard, n);
                        }
                    }
                }
                for arm in arms {
                    match &arm.body {
                        MatchBody::Block(b) => self.visit_block(b),
                        MatchBody::Expr(e) => self.visit_expr(e),
                    }
                }
            }
            Expr::Binary { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            Expr::Unary { expr, .. } | Expr::Borrow { expr, .. } => self.visit_expr(expr),
            Expr::FnCall { args, .. } => {
                for a in args {
                    self.visit_expr(a);
                }
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.visit_expr(receiver);
                for a in args {
                    self.visit_expr(a);
                }
            }
            Expr::FieldAccess { expr, .. } => self.visit_expr(expr),
            Expr::Literal(..) | Expr::Ident(..) => {}
            _ => {} // Intentionally: MC/DC only needs if/while/match decisions
        }
    }
}

/// Count the number of atomic boolean clauses in an expression.
///
/// For `A && B` → 2. For `(A || B) && C` → 3. For a simple `x > 0` → 1.
pub fn count_clauses(expr: &Expr) -> usize {
    match expr {
        Expr::Binary {
            op: BinaryOp::And | BinaryOp::Or,
            left,
            right,
            ..
        } => count_clauses(left) + count_clauses(right),
        _ => 1,
    }
}

/// Collect atomic leaf expressions from a compound boolean condition in
/// left-to-right order.
pub fn collect_clauses<'a>(expr: &'a Expr, clauses: &mut Vec<&'a Expr>) {
    match expr {
        Expr::Binary {
            op: BinaryOp::And | BinaryOp::Or,
            left,
            right,
            ..
        } => {
            collect_clauses(left, clauses);
            collect_clauses(right, clauses);
        }
        _ => clauses.push(expr),
    }
}

/// Count atomic boolean clauses in a `RefExpr` (match guard language).
///
/// For `a && b` → 2. For `(x > 0) || (y < 10 && z == 5)` → 3.
/// Single comparisons or identifiers → 1.
pub fn count_clauses_ref(expr: &RefExpr) -> usize {
    match expr {
        RefExpr::LogicOp {
            op: LogicOp::And | LogicOp::Or,
            left,
            right,
            ..
        } => count_clauses_ref(left) + count_clauses_ref(right),
        RefExpr::Grouped { inner, .. } => count_clauses_ref(inner),
        _ => 1,
    }
}

/// MC/DC summary statistics for a set of decisions.
#[derive(Debug, Default)]
pub struct MCDCStats {
    pub total_decisions: usize,
    pub compound_decisions: usize,
    pub total_clauses: usize,
    /// One independence obligation per clause across all compound decisions.
    pub total_obligations: usize,
    /// Decisions inside effectful functions (excluded from the coverage denominator).
    pub exempt_decisions: usize,
    /// Obligations inside effectful functions (excluded from the coverage denominator).
    pub exempt_obligations: usize,
}

impl MCDCStats {
    pub fn from_decisions(decisions: &[DecisionInfo]) -> Self {
        let mut s = MCDCStats::default();
        for d in decisions {
            s.total_decisions += 1;
            s.total_clauses += d.clause_count;
            if d.is_compound() {
                s.compound_decisions += 1;
                s.total_obligations += d.clause_count;
            }
            if d.is_effectful {
                s.exempt_decisions += 1;
                if d.is_compound() {
                    s.exempt_obligations += d.clause_count;
                }
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn decisions_for(src: &str) -> Vec<DecisionInfo> {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        analyze_mcdc(&prog, "test", 0)
    }

    #[test]
    fn simple_if_excluded_no_compound() {
        // Single-clause conditions carry no MC/DC obligations — not tracked.
        let decisions = decisions_for("fn f(x: Int) -> Int { if x > 0 { 1 } else { 0 } }");
        assert_eq!(decisions.len(), 0);
    }

    #[test]
    fn compound_and_has_two_clauses() {
        let decisions =
            decisions_for("fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }");
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].clause_count, 2);
        assert!(decisions[0].is_compound());
        assert_eq!(decisions[0].min_tests(), 3);
    }

    #[test]
    fn triple_or_has_three_clauses() {
        let decisions = decisions_for(
            "fn f(a: Bool, b: Bool, c: Bool) -> Int { if a || b || c { 1 } else { 0 } }",
        );
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].clause_count, 3);
    }

    #[test]
    fn nested_decisions_get_sequential_ids() {
        // Inner `if c` is single-clause — excluded. Only `if a && b` is tracked.
        let decisions = decisions_for(
            "fn f(a: Bool, b: Bool, c: Bool) -> Int { if a && b { if c { 1 } else { 2 } } else { 0 } }"
        );
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].id, 0);
        assert_eq!(decisions[0].clause_count, 2);
    }

    #[test]
    fn two_compound_decisions_get_sequential_ids() {
        let decisions = decisions_for(
            "fn f(a: Bool, b: Bool, c: Bool) -> Int { if a && b { 1 } else { if b || c { 2 } else { 0 } } }"
        );
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].id, 0);
        assert_eq!(decisions[1].id, 1);
    }

    #[test]
    fn test_fns_excluded() {
        let decisions =
            decisions_for("test fn t() -> Bool { if true && false { true } else { false } }");
        assert_eq!(decisions.len(), 0);
    }

    #[test]
    fn while_compound_condition_tracked() {
        let decisions = decisions_for(
            "partial fn f(a: Bool, b: Bool) -> Int { let x: ref Int = 0; while a && b { x = x + 1; } x }"
        );
        let while_decisions: Vec<_> = decisions
            .iter()
            .filter(|d| d.kind == DecisionKind::While)
            .collect();
        assert_eq!(while_decisions.len(), 1);
        assert_eq!(while_decisions[0].clause_count, 2);
    }

    #[test]
    fn stats_counts_compound_only() {
        // Single-clause `if x > 0` is not tracked; compound `if a && b` is.
        let decisions = decisions_for(
            "fn f(a: Bool, b: Bool, x: Int) -> Int { if x > 0 { 1 } else { if a && b { 2 } else { 0 } } }"
        );
        let stats = MCDCStats::from_decisions(&decisions);
        assert_eq!(stats.total_decisions, 1);
        assert_eq!(stats.compound_decisions, 1);
        assert_eq!(stats.total_obligations, 2); // 2 clauses in the compound decision
    }

    #[test]
    fn start_id_offsets_decision_ids() {
        let (mut p, _) =
            Parser::new("fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }");
        let prog = p.parse_program();
        let decisions = analyze_mcdc(&prog, "test", 5);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].id, 5);
    }

    #[test]
    fn correct_line_numbers_reported() {
        // The if statement is on line 3 of this snippet; verify span.line is captured correctly.
        let src = "fn f(a: Bool, b: Bool) -> Bool {\n    let x: Bool = a;\n    if a && b {\n        return true\n    }\n    return false\n}";
        let decisions = decisions_for(src);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].line, 3, "if statement should report line 3");
    }

    // ── Match coverage tests ──────────────────────────────────────────────

    #[test]
    fn match_with_two_arms_tracked() {
        let decisions = decisions_for("fn f(x: Bool) -> Int { match x { true => 1, false => 0 } }");
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].kind, DecisionKind::Match);
        assert_eq!(decisions[0].clause_count, 2);
    }

    #[test]
    fn match_with_three_arms_tracked() {
        // Use Bool/Int match (no enum needed) to avoid parse_program() limitations.
        let decisions =
            decisions_for("fn f(x: Int) -> Int { match x { 1 => 10, 2 => 20, _ => 30 } }");
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].kind, DecisionKind::Match);
        assert_eq!(decisions[0].clause_count, 3);
    }

    #[test]
    fn match_single_arm_not_tracked() {
        // A match with only one arm has no meaningful decision — excluded.
        let decisions = decisions_for("fn f(x: Int) -> Int { match x { n => n + 1 } }");
        assert_eq!(decisions.len(), 0);
    }

    #[test]
    fn match_compound_condition_in_arm_body_tracked_as_if() {
        // A compound `if` inside a match arm body is still tracked as `If`.
        let decisions = decisions_for(
            "fn f(a: Bool, b: Bool, x: Int) -> Int { match x { 1 => if a && b { 1 } else { 0 }, _ => 2 } }",
        );
        // Match (2 arms) + If (a&&b in arm body)
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].kind, DecisionKind::Match);
        assert_eq!(decisions[0].clause_count, 2);
        assert_eq!(decisions[1].kind, DecisionKind::If);
        assert_eq!(decisions[1].clause_count, 2);
    }

    // ── Match guard coverage tests ─────────────────────────────────────

    #[test]
    fn match_guard_compound_tracked() {
        // A compound guard (`if a && b`) produces a MatchGuard decision.
        let decisions = decisions_for(
            "fn f(a: Bool, b: Bool, x: Int) -> Int { match x { n if a && b => n, _ => 0 } }",
        );
        let guards: Vec<_> = decisions
            .iter()
            .filter(|d| d.kind == DecisionKind::MatchGuard)
            .collect();
        assert_eq!(guards.len(), 1);
        assert_eq!(guards[0].clause_count, 2);
    }

    #[test]
    fn match_guard_single_clause_not_tracked() {
        // A single-clause guard (`if a`) has no MC/DC obligation — excluded.
        let decisions =
            decisions_for("fn f(a: Bool, x: Int) -> Int { match x { n if a => n, _ => 0 } }");
        let guards: Vec<_> = decisions
            .iter()
            .filter(|d| d.kind == DecisionKind::MatchGuard)
            .collect();
        assert_eq!(guards.len(), 0);
    }

    #[test]
    fn match_guard_and_match_both_tracked() {
        // Match itself (2 arms) + compound guard on first arm.
        // Order: Match decision is registered first, then MatchGuard decisions.
        let decisions = decisions_for(
            "fn f(a: Bool, b: Bool, x: Int) -> Int { match x { n if a || b => n, _ => 0 } }",
        );
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].kind, DecisionKind::Match);
        assert_eq!(decisions[0].clause_count, 2);
        assert_eq!(decisions[1].kind, DecisionKind::MatchGuard);
        assert_eq!(decisions[1].clause_count, 2);
    }

    #[test]
    fn match_guard_triple_clause() {
        // Three-clause guard: `if a && b || c`.
        let decisions = decisions_for(
            "fn f(a: Bool, b: Bool, c: Bool, x: Int) -> Int { match x { n if a && b || c => n, _ => 0 } }",
        );
        let guards: Vec<_> = decisions
            .iter()
            .filter(|d| d.kind == DecisionKind::MatchGuard)
            .collect();
        assert_eq!(guards.len(), 1);
        assert_eq!(guards[0].clause_count, 3);
    }

    // ── Expr::If (inline expression form) ────────────────────────────────

    #[test]
    fn inline_if_expr_compound_condition_tracked() {
        // `if` used as an expression (not a statement) with a compound condition.
        let decisions = decisions_for(
            "fn f(a: Bool, b: Bool) -> Int { let x: Int = if a && b { 1 } else { 0 }; x }",
        );
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].kind, DecisionKind::If);
        assert_eq!(decisions[0].clause_count, 2);
    }

    // ── count_clauses_ref unit tests ────────────────────────────────────

    #[test]
    fn count_clauses_ref_basic() {
        use crate::mvl::parser::ast::{LogicOp, RefExpr};
        use crate::mvl::parser::lexer::Span;
        let span = Span {
            line: 1,
            col: 1,
            offset: 0,
            len: 1,
        };
        let a = RefExpr::Ident {
            name: "a".into(),
            span,
        };
        let b = RefExpr::Ident {
            name: "b".into(),
            span,
        };
        let and_expr = RefExpr::LogicOp {
            op: LogicOp::And,
            left: Box::new(a),
            right: Box::new(b),
            span,
        };
        assert_eq!(count_clauses_ref(&and_expr), 2);
    }
}
