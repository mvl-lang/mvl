//! Emit Rust statements from MVL [`Stmt`] nodes.
//!
//! Covers statement forms defined in 000-parser/Req 5 ("Parse Statements"):
//! `let`/`let mut`, assignment, `if`/`else`, `match`, `for`, `while`, `return`, `?`.
//!
//! The emitted Rust preserves MVL's semantic guarantees:
//! - `while` only appears in `partial fn` bodies (enforced by the type checker per Req 8)
//! - `for` iterates over labeled collections, preserving security labels per Req 11
//! - Assignments carry the label of the source expression (IFC is static, no runtime cost)
//!
//! Part of the ADR-0003 transpilation pipeline.  Spec link: 000-parser Req 1 (statement grammar).
//!
//! See ADR-0003 for the overall compilation strategy.

use crate::mvl::checker::mcdc::collect_clauses;
use crate::mvl::parser::ast::{BinaryOp, ElseBranch, Expr, LValue, MatchBody, Stmt};
use crate::mvl::transpiler::codegen::Codegen;
use crate::mvl::transpiler::coverage::BranchKind;
use crate::mvl::transpiler::emit_exprs::{
    arms_have_str_pattern, emit_block_as_value, emit_block_stmts, emit_expr, emit_pattern,
};
use crate::mvl::transpiler::emit_types::{emit_ref_expr_for_assert, emit_type_expr};

/// Emit a single statement (with indentation and trailing newline).
pub fn emit_stmt(cg: &mut Codegen, stmt: &Stmt) {
    match stmt {
        Stmt::Let {
            mutable,
            pattern,
            ty,
            init,
            ..
        } => {
            cg.indent();
            if *mutable {
                cg.push("let mut ");
            } else {
                cg.push("let ");
            }
            emit_pattern(cg, pattern);
            if let Some(t) = ty {
                cg.push(": ");
                cg.push(&emit_type_expr(t));
            }
            cg.push(" = ");
            emit_expr(cg, init);
            cg.push(";");
            cg.nl();
        }

        Stmt::Assign { target, value, .. } => {
            cg.indent();
            emit_lvalue(cg, target);
            cg.push(" = ");
            emit_expr(cg, value);
            cg.push(";");
            cg.nl();
        }

        Stmt::Return { value, .. } => {
            cg.indent();
            if let Some(v) = value {
                cg.push("return ");
                emit_expr(cg, v);
                cg.push(";");
            } else {
                cg.push("return;");
            }
            cg.nl();
        }

        Stmt::If {
            cond,
            then,
            else_,
            span,
            ..
        } => {
            let true_id = cg.alloc_branch(span.line, BranchKind::IfTrue);
            let false_id = else_
                .as_ref()
                .and(cg.alloc_branch(span.line, BranchKind::IfFalse));
            if emit_mcdc_if(cg, cond, then, else_, span.line, true_id, false_id) {
                // MC/DC instrumented emission handled the full if-statement.
            } else {
                cg.indent();
                cg.push("if ");
                emit_expr(cg, cond);
                cg.push(" {");
                cg.nl();
                cg.push_indent();
                if let Some(id) = true_id {
                    cg.emit_cov_hit(id);
                }
                emit_block_as_value(cg, &then.stmts);
                cg.pop_indent();
                cg.indent();
                cg.push("}");
                if let Some(else_branch) = else_ {
                    cg.push(" else ");
                    emit_else_branch(cg, else_branch, false_id);
                }
                cg.nl();
            }
        }

        Stmt::Match {
            scrutinee,
            arms,
            span,
            ..
        } => {
            // Allocate coverage IDs for each arm up-front (avoids borrow conflict).
            let arm_ids: Vec<Option<usize>> = (0..arms.len())
                .map(|i| cg.alloc_branch(span.line, BranchKind::MatchArm(i)))
                .collect();
            let has_str_arm = arms_have_str_pattern(arms);
            cg.indent();
            cg.push("match ");
            emit_expr(cg, scrutinee);
            if has_str_arm {
                cg.push(".as_str()");
            }
            cg.push(" {");
            cg.nl();
            cg.push_indent();
            for (arm, cov_id) in arms.iter().zip(arm_ids.iter()) {
                cg.indent();
                emit_pattern(cg, &arm.pattern);
                if let Some(guard) = &arm.guard {
                    cg.push(" if ");
                    cg.push(&emit_ref_expr_for_assert(guard, "_"));
                }
                cg.push(" => ");
                match &arm.body {
                    MatchBody::Expr(e) => {
                        if let Some(id) = cov_id {
                            // Wrap expr arm in a block to inject hit statement.
                            cg.push("{ ");
                            cg.push(&format!("#[cfg(test)] crate::__mvl_cov::hit({id}); "));
                            emit_expr(cg, e);
                            cg.push(" }");
                        } else {
                            emit_expr(cg, e);
                        }
                        cg.push(",");
                        cg.nl();
                    }
                    MatchBody::Block(block) => {
                        cg.push("{");
                        cg.nl();
                        cg.push_indent();
                        if let Some(id) = cov_id {
                            cg.emit_cov_hit(*id);
                        }
                        // Use emit_block_as_value so the final Stmt::Expr is a tail
                        // expression (no semicolon) and becomes the arm's return value.
                        emit_block_as_value(cg, &block.stmts);
                        cg.pop_indent();
                        cg.indent();
                        cg.push("}");
                        cg.nl();
                    }
                }
            }
            cg.pop_indent();
            cg.indent();
            cg.push("}");
            cg.nl();
        }

        Stmt::For {
            pattern,
            iter,
            body,
            span,
            ..
        } => {
            let for_id = cg.alloc_branch(span.line, BranchKind::ForBody);
            cg.indent();
            cg.push("for ");
            emit_pattern(cg, pattern);
            // MVL value semantics: the iterable is conceptually copied, not consumed.
            // Wrap the entire expression in parens before `.clone()` so the pattern
            // works for all expression forms (ident, field access, function call, etc.).
            // Spec 009 Req 7.
            cg.push(" in (");
            emit_expr(cg, iter);
            cg.push(").clone() {");
            cg.nl();
            cg.push_indent();
            if let Some(id) = for_id {
                cg.emit_cov_hit(id);
            }
            emit_block_stmts(cg, &body.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
            cg.nl();
        }

        Stmt::While {
            cond, body, span, ..
        } => {
            let while_id = cg.alloc_branch(span.line, BranchKind::WhileBody);
            if emit_mcdc_while(cg, cond, body, span.line, while_id) {
                // MC/DC instrumented emission handled the full while-loop.
            } else {
                cg.indent();
                cg.push("while ");
                emit_expr(cg, cond);
                cg.push(" {");
                cg.nl();
                cg.push_indent();
                if let Some(id) = while_id {
                    cg.emit_cov_hit(id);
                }
                emit_block_stmts(cg, &body.stmts);
                cg.pop_indent();
                cg.indent();
                cg.push("}");
                cg.nl();
            }
        }

        Stmt::Expr { expr, .. } => {
            cg.indent();
            emit_expr(cg, expr);
            // Determine if this needs a semicolon: add one for non-block expressions
            // that are used as statements (not the implicit tail expression).
            // Phase 1: always add semicolon for safety; tail expressions in Rust
            // blocks without semicolons are handled by emit_fn_decl's body emitter.
            cg.push(";");
            cg.nl();
        }
    }
}

fn emit_lvalue(cg: &mut Codegen, lv: &LValue) {
    match lv {
        LValue::Ident(name, _) => cg.push(name),
        LValue::Field { base, field, .. } => {
            emit_lvalue(cg, base);
            cg.push(".");
            cg.push(field);
        }
    }
}

fn emit_else_branch(cg: &mut Codegen, branch: &ElseBranch, cov_id: Option<usize>) {
    match branch {
        ElseBranch::Block(block) => {
            cg.push("{");
            cg.nl();
            cg.push_indent();
            if let Some(id) = cov_id {
                cg.emit_cov_hit(id);
            }
            emit_block_as_value(cg, &block.stmts);
            cg.pop_indent();
            cg.indent();
            cg.push("}");
        }
        ElseBranch::If(stmt) => {
            // Emit the `if` inline (no leading indent, no trailing newline) so
            // the caller's `} else ` and this `if` land on the same line.
            // The false-branch coverage hit for the outer if is injected here
            // before the inner condition is tested.
            match stmt.as_ref() {
                Stmt::If {
                    cond,
                    then,
                    else_,
                    span,
                    ..
                } => {
                    // Allocate IDs for the inner else-if's own branches.
                    let inner_true_id = cg.alloc_branch(span.line, BranchKind::IfTrue);
                    let inner_false_id = else_
                        .as_ref()
                        .and(cg.alloc_branch(span.line, BranchKind::IfFalse));
                    cg.push("if ");
                    emit_expr(cg, cond);
                    cg.push(" {");
                    cg.nl();
                    cg.push_indent();
                    if let Some(id) = inner_true_id {
                        cg.emit_cov_hit(id);
                    }
                    emit_block_as_value(cg, &then.stmts);
                    cg.pop_indent();
                    cg.indent();
                    cg.push("}");
                    if let Some(inner_else) = else_ {
                        cg.push(" else ");
                        emit_else_branch(cg, inner_else, inner_false_id);
                    }
                }
                other => unreachable!("ElseBranch::If must always wrap Stmt::If; got {:?}", other),
            }
        }
    }
}

// ── MC/DC instrumentation helpers ────────────────────────────────────────

/// Emit an if-statement with MC/DC clause tracking for compound conditions.
///
/// Returns `true` when MC/DC instrumentation was applied (compound condition
/// with an active MC/DC map).  Returns `false` when the caller should fall
/// back to normal emission (simple condition or MC/DC inactive).
fn emit_mcdc_if(
    cg: &mut Codegen,
    cond: &Expr,
    then: &crate::mvl::parser::ast::Block,
    else_: &Option<ElseBranch>,
    line: u32,
    true_id: Option<usize>,
    false_id: Option<usize>,
) -> bool {
    let mut clauses = Vec::new();
    collect_clauses(cond, &mut clauses);
    if clauses.len() <= 1 || cg.mcdc.is_none() {
        return false;
    }
    let n = clauses.len();
    let Some(decision_id) = cg.alloc_mcdc_decision(line, n) else {
        return false;
    };

    // Emit clause locals (eager evaluation — MVL expressions are pure).
    for (i, clause_expr) in clauses.iter().enumerate() {
        cg.indent();
        cg.push(&format!("let __d{decision_id}_c{i}: bool = "));
        emit_expr(cg, clause_expr);
        cg.push(";");
        cg.nl();
    }

    // Emit outcome via recomposed boolean expression using clause vars.
    cg.indent();
    cg.push(&format!("let __d{decision_id}_outcome: bool = "));
    emit_mcdc_recomposed(cg, cond, decision_id, &mut 0);
    cg.push(";");
    cg.nl();

    // Emit observation record.
    emit_mcdc_record(cg, decision_id, n);

    // Emit: if __d{id}_outcome { ... }
    cg.indent();
    cg.push(&format!("if __d{decision_id}_outcome {{"));
    cg.nl();
    cg.push_indent();
    if let Some(id) = true_id {
        cg.emit_cov_hit(id);
    }
    emit_block_as_value(cg, &then.stmts);
    cg.pop_indent();
    cg.indent();
    cg.push("}");
    if let Some(else_branch) = else_ {
        cg.push(" else ");
        emit_else_branch(cg, else_branch, false_id);
    }
    cg.nl();
    true
}

/// Emit a while-loop with MC/DC clause tracking, restructured as `loop { … }`.
///
/// Returns `true` when MC/DC instrumentation was applied, `false` to fall back.
fn emit_mcdc_while(
    cg: &mut Codegen,
    cond: &Expr,
    body: &crate::mvl::parser::ast::Block,
    line: u32,
    while_id: Option<usize>,
) -> bool {
    let mut clauses = Vec::new();
    collect_clauses(cond, &mut clauses);
    if clauses.len() <= 1 || cg.mcdc.is_none() {
        return false;
    }
    let n = clauses.len();
    let Some(decision_id) = cg.alloc_mcdc_decision(line, n) else {
        return false;
    };

    // Restructure as `loop` so clause locals can be re-evaluated each iteration.
    cg.indent();
    cg.push("loop {");
    cg.nl();
    cg.push_indent();

    // Clause locals.
    for (i, clause_expr) in clauses.iter().enumerate() {
        cg.indent();
        cg.push(&format!("let __d{decision_id}_c{i}: bool = "));
        emit_expr(cg, clause_expr);
        cg.push(";");
        cg.nl();
    }

    // Outcome.
    cg.indent();
    cg.push(&format!("let __d{decision_id}_outcome: bool = "));
    emit_mcdc_recomposed(cg, cond, decision_id, &mut 0);
    cg.push(";");
    cg.nl();

    // Record observation.
    emit_mcdc_record(cg, decision_id, n);

    // Break when condition is false.
    cg.line(&format!("if !__d{decision_id}_outcome {{ break; }}"));

    // Coverage hit (body entry).
    if let Some(id) = while_id {
        cg.emit_cov_hit(id);
    }

    emit_block_stmts(cg, &body.stmts);
    cg.pop_indent();
    cg.indent();
    cg.push("}");
    cg.nl();
    true
}

/// Recompose a compound boolean expression using pre-evaluated clause variables.
///
/// Leaf nodes are replaced with `__d{decision_id}_c{i}` in left-to-right order.
fn emit_mcdc_recomposed(cg: &mut Codegen, expr: &Expr, decision_id: usize, clause_idx: &mut usize) {
    if let Expr::Binary {
        op, left, right, ..
    } = expr
    {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            cg.push("(");
            emit_mcdc_recomposed(cg, left, decision_id, clause_idx);
            cg.push(if *op == BinaryOp::And { " && " } else { " || " });
            emit_mcdc_recomposed(cg, right, decision_id, clause_idx);
            cg.push(")");
            return;
        }
    }
    let i = *clause_idx;
    *clause_idx += 1;
    cg.push(&format!("__d{decision_id}_c{i}"));
}

/// Emit the `__mvl_mcdc::record(…)` call for a decision with `n` clauses.
fn emit_mcdc_record(cg: &mut Codegen, decision_id: usize, n: usize) {
    // Encode: bits 0..n-1 = clause vals, bit n = outcome.
    let vals: Vec<String> = (0..n)
        .map(|i| format!("((__d{decision_id}_c{i} as u16) << {i}u16)"))
        .collect();
    let outcome = format!("((__d{decision_id}_outcome as u16) << {n}u16)");
    let encoded = vals
        .into_iter()
        .chain(std::iter::once(outcome))
        .collect::<Vec<_>>()
        .join(" | ");
    cg.line(&format!(
        "#[cfg(test)] crate::__mvl_mcdc::record({decision_id}usize, {encoded});"
    ));
}
