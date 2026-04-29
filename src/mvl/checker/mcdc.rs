//! Static MC/DC obligation analysis.
//!
//! Walks the AST to identify all decisions (if/while conditions) and counts
//! their atomic boolean clauses. Results feed the obligation table used by
//! `cmd_mcdc` to verify MC/DC coverage.
//!
//! MC/DC (Modified Condition/Decision Coverage) requires that each atomic
//! boolean clause in a compound condition independently affects the decision
//! outcome. Required by DO-178C at DAL-A and ISO 26262 at ASIL-D.

use crate::mvl::parser::ast::{BinaryOp, Block, Decl, ElseBranch, Expr, MatchBody, Program, Stmt};

/// Identifies the kind of decision point.
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionKind {
    If,
    While,
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
            collect_from_block(&fd.body, &fd.name, file_stem, &mut decisions, &mut next_id);
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
    for stmt in &block.stmts {
        collect_from_stmt(stmt, fn_name, file, decisions, next_id);
    }
}

fn collect_from_stmt(
    stmt: &Stmt,
    fn_name: &str,
    file: &str,
    decisions: &mut Vec<DecisionInfo>,
    next_id: &mut usize,
) {
    match stmt {
        Stmt::If {
            cond,
            then,
            else_,
            span,
        } => {
            let clause_count = count_clauses(cond);
            if clause_count > 1 {
                decisions.push(DecisionInfo {
                    id: *next_id,
                    fn_name: fn_name.to_string(),
                    file: file.to_string(),
                    line: span.line,
                    kind: DecisionKind::If,
                    clause_count,
                });
                *next_id += 1;
            }
            collect_from_block(then, fn_name, file, decisions, next_id);
            if let Some(else_branch) = else_ {
                match else_branch {
                    ElseBranch::Block(b) => {
                        collect_from_block(b, fn_name, file, decisions, next_id);
                    }
                    ElseBranch::If(s) => {
                        collect_from_stmt(s, fn_name, file, decisions, next_id);
                    }
                }
            }
        }
        Stmt::While { cond, body, span } => {
            let clause_count = count_clauses(cond);
            if clause_count > 1 {
                decisions.push(DecisionInfo {
                    id: *next_id,
                    fn_name: fn_name.to_string(),
                    file: file.to_string(),
                    line: span.line,
                    kind: DecisionKind::While,
                    clause_count,
                });
                *next_id += 1;
            }
            collect_from_block(body, fn_name, file, decisions, next_id);
        }
        Stmt::Match { arms, .. } => {
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => {
                        collect_from_block(b, fn_name, file, decisions, next_id);
                    }
                    MatchBody::Expr(e) => {
                        collect_from_expr(e, fn_name, file, decisions, next_id);
                    }
                }
            }
        }
        Stmt::For { body, .. } => {
            collect_from_block(body, fn_name, file, decisions, next_id);
        }
        Stmt::Let { init, .. } => {
            collect_from_expr(init, fn_name, file, decisions, next_id);
        }
        Stmt::Assign { value, .. } => {
            collect_from_expr(value, fn_name, file, decisions, next_id);
        }
        Stmt::Expr { expr, .. } => {
            collect_from_expr(expr, fn_name, file, decisions, next_id);
        }
        Stmt::Return { value: Some(e), .. } => {
            collect_from_expr(e, fn_name, file, decisions, next_id);
        }
        Stmt::Return { value: None, .. } => {}
    }
}

fn collect_from_expr(
    expr: &Expr,
    fn_name: &str,
    file: &str,
    decisions: &mut Vec<DecisionInfo>,
    next_id: &mut usize,
) {
    match expr {
        Expr::If {
            cond,
            then,
            else_,
            span,
        } => {
            let clause_count = count_clauses(cond);
            if clause_count > 1 {
                decisions.push(DecisionInfo {
                    id: *next_id,
                    fn_name: fn_name.to_string(),
                    file: file.to_string(),
                    line: span.line,
                    kind: DecisionKind::If,
                    clause_count,
                });
                *next_id += 1;
            }
            collect_from_block(then, fn_name, file, decisions, next_id);
            if let Some(e) = else_ {
                collect_from_expr(e, fn_name, file, decisions, next_id);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_from_expr(left, fn_name, file, decisions, next_id);
            collect_from_expr(right, fn_name, file, decisions, next_id);
        }
        Expr::Unary { expr: e, .. } => {
            collect_from_expr(e, fn_name, file, decisions, next_id);
        }
        Expr::FnCall { args, .. } => {
            for arg in args {
                collect_from_expr(arg, fn_name, file, decisions, next_id);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_from_expr(receiver, fn_name, file, decisions, next_id);
            for arg in args {
                collect_from_expr(arg, fn_name, file, decisions, next_id);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_from_expr(scrutinee, fn_name, file, decisions, next_id);
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => {
                        collect_from_block(b, fn_name, file, decisions, next_id);
                    }
                    MatchBody::Expr(e) => {
                        collect_from_expr(e, fn_name, file, decisions, next_id);
                    }
                }
            }
        }
        Expr::FieldAccess { expr: e, .. } => {
            collect_from_expr(e, fn_name, file, decisions, next_id);
        }
        Expr::Literal(..) | Expr::Ident(..) => {}
        // Catch-all for any future expression variants
        #[allow(unreachable_patterns)]
        _ => {}
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

/// MC/DC summary statistics for a set of decisions.
#[derive(Debug, Default)]
pub struct MCDCStats {
    pub total_decisions: usize,
    pub compound_decisions: usize,
    pub total_clauses: usize,
    /// One independence obligation per clause across all compound decisions.
    pub total_obligations: usize,
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
            "partial fn f(a: Bool, b: Bool) -> Int { let mut x: Int = 0; while a && b { x = x + 1; } x }"
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
}
