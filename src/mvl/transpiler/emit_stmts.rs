// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

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

use crate::mvl::parser::ast::{
    BinaryOp, ElseBranch, Expr, LValue, LogicOp, MatchBody, RefExpr, Stmt, TypeExpr,
};
use crate::mvl::passes::coverage::BranchKind;
use crate::mvl::passes::mcdc::analysis::{collect_clauses, count_clauses_ref};
use crate::mvl::transpiler::emit_exprs::{
    arms_have_str_pattern, emit_block_as_value, emit_block_stmts, emit_expr, emit_pattern,
};
use crate::mvl::transpiler::emit_types::{emit_ref_expr_for_assert, emit_type_expr};
use crate::mvl::transpiler::emitter::RustEmitter;
use crate::mvl::transpiler::mcdc_instr::{detect_coupled_pairs, DecisionKind};

/// Emit a single statement (with indentation and trailing newline).
pub fn emit_stmt(cg: &mut RustEmitter, stmt: &Stmt) {
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
            cg.push(": ");
            cg.push(&emit_type_expr(ty));
            cg.push(" = ");
            emit_expr(cg, init);
            // When the declared type is a security label (e.g. Tainted<String>) and the
            // init is a plain string/value, `.into()` converts it to the labeled type.
            // The explicit type annotation makes this unambiguous (unlike assert_eq! context).
            let needs_into = matches!(ty, TypeExpr::Labeled { .. })
                && matches!(
                    init,
                    Expr::Literal(crate::mvl::parser::ast::Literal::Str(_), _)
                );
            if needs_into {
                cg.push(".into()");
            }
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
            // Allocate branch coverage IDs for each arm up-front (avoids borrow conflict).
            let arm_ids: Vec<Option<usize>> = (0..arms.len())
                .map(|i| cg.alloc_branch(span.line, BranchKind::MatchArm(i)))
                .collect();
            let has_str_arm = arms_have_str_pattern(arms);

            // Emit scrutinee first so any compound conditions inside it allocate
            // MC/DC IDs before the match-level decisions (mirrors analysis order).
            cg.indent();
            cg.push("match ");
            emit_expr(cg, scrutinee);

            // Allocate MC/DC arm-coverage decision (Match kind, one "clause" per arm).
            let match_mcdc_id: Option<usize> = if arms.len() >= 2 {
                cg.alloc_mcdc_decision(span.line, arms.len(), DecisionKind::Match, vec![])
            } else {
                None
            };
            // Pre-allocate MatchGuard decision IDs for compound guards — all arms in
            // order before any body emission (mirrors analysis pre-allocation order).
            let guard_mcdc_ids: Vec<Option<usize>> = arms
                .iter()
                .map(|arm| {
                    arm.guard.as_ref().and_then(|g| {
                        let n = count_clauses_ref(g);
                        if n >= 2 {
                            cg.alloc_mcdc_decision(
                                arm.span.line,
                                n,
                                DecisionKind::MatchGuard,
                                vec![],
                            )
                        } else {
                            None
                        }
                    })
                })
                .collect();

            if has_str_arm {
                cg.push(".as_str()");
            }
            cg.push(" {");
            cg.nl();
            cg.push_indent();
            for ((arm_idx, arm), (cov_id, guard_mcdc_id)) in arms
                .iter()
                .enumerate()
                .zip(arm_ids.iter().zip(guard_mcdc_ids.iter()))
            {
                cg.indent();
                emit_pattern(cg, &arm.pattern);
                if let Some(guard) = &arm.guard {
                    cg.push(" if ");
                    if let Some(&gid) = guard_mcdc_id.as_ref() {
                        let n = count_clauses_ref(guard);
                        cg.push(&emit_mcdc_guard_block(guard, gid, n));
                    } else {
                        cg.push(&emit_ref_expr_for_assert(guard, "_"));
                    }
                }
                cg.push(" => ");
                match &arm.body {
                    MatchBody::Expr(e) => {
                        // Wrap in a block to inject coverage and MC/DC hits.
                        cg.push("{ ");
                        if let Some(&id) = cov_id.as_ref() {
                            cg.push(&format!("#[cfg(test)] crate::__mvl_cov::hit({id}); "));
                        }
                        if let Some(mid) = match_mcdc_id {
                            cg.push(&format!(
                                "#[cfg(test)] crate::__mvl_mcdc::record({mid}usize, {arm_idx}u32); "
                            ));
                        }
                        emit_expr(cg, e);
                        cg.push(" }");
                        cg.push(",");
                        cg.nl();
                    }
                    MatchBody::Block(block) => {
                        cg.push("{");
                        cg.nl();
                        cg.push_indent();
                        if let Some(&id) = cov_id.as_ref() {
                            cg.emit_cov_hit(id);
                        }
                        if let Some(mid) = match_mcdc_id {
                            cg.line(&format!(
                                "#[cfg(test)] crate::__mvl_mcdc::record({mid}usize, {arm_idx}u32);"
                            ));
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

fn emit_lvalue(cg: &mut RustEmitter, lv: &LValue) {
    match lv {
        LValue::Ident(name, _) => cg.push(name),
        LValue::Field { base, field, .. } => {
            emit_lvalue(cg, base);
            cg.push(".");
            cg.push(field);
        }
    }
}

fn emit_else_branch(cg: &mut RustEmitter, branch: &ElseBranch, cov_id: Option<usize>) {
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
                    // Instrument compound else-if conditions with MC/DC (same as
                    // top-level if — mirrors analysis order so decision IDs align).
                    // Must wrap in `{ }` because `else` requires a block in Rust.
                    let mut check_clauses = Vec::new();
                    collect_clauses(cond, &mut check_clauses);
                    if check_clauses.len() >= 2 && cg.mcdc.is_some() {
                        cg.push("{");
                        cg.nl();
                        cg.push_indent();
                        emit_mcdc_if(
                            cg,
                            cond,
                            then,
                            else_,
                            span.line,
                            inner_true_id,
                            inner_false_id,
                        );
                        cg.pop_indent();
                        cg.indent();
                        cg.push("}");
                        return;
                    }
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

// ── MC/DC guard instrumentation ──────────────────────────────────────────

/// Build a Rust block expression (usable as a match arm guard) that tracks
/// MC/DC clause values for a compound `RefExpr` guard.
///
/// The generated block evaluates the guard with short-circuit semantics,
/// records an observation, and evaluates to the boolean guard outcome.
/// Returned as a `String` because `RefExpr` uses string-building throughout.
pub(crate) fn emit_mcdc_guard_block(guard: &RefExpr, decision_id: usize, n: usize) -> String {
    let decls = format!(
        "let mut __d{decision_id}_c = [false; {n}]; \
         let mut __d{decision_id}_e = [false; {n}];"
    );
    let mut idx = 0usize;
    let sc = sc_ref_outcome_str(guard, decision_id, &mut idx);

    // Build 2N+1 bit observation encoding identical to emit_mcdc_record.
    let vals: Vec<String> = (0..n)
        .map(|i| format!("((__d{decision_id}_c[{i}] as u32) << {i}u32)"))
        .collect();
    let evals: Vec<String> = (0..n)
        .map(|i| format!("((__d{decision_id}_e[{i}] as u32) << {}u32)", n + i))
        .collect();
    let outcome_bit = format!("((__d{decision_id}_outcome as u32) << {}u32)", 2 * n);
    let encoding = vals
        .into_iter()
        .chain(evals)
        .chain(std::iter::once(outcome_bit))
        .collect::<Vec<_>>()
        .join(" | ");

    format!(
        "{{ {decls} \
         let __d{decision_id}_outcome: bool = {sc}; \
         #[cfg(test)] crate::__mvl_mcdc::record({decision_id}usize, {encoding}); \
         __d{decision_id}_outcome }}"
    )
}

/// Recursively build the short-circuit evaluation tree for a `RefExpr`.
///
/// Returns a Rust expression string that evaluates the guard with short-circuit
/// semantics while setting `__d{id}_c[i]` and `__d{id}_e[i]` for each leaf.
fn sc_ref_outcome_str(expr: &RefExpr, id: usize, idx: &mut usize) -> String {
    match expr {
        RefExpr::LogicOp {
            op: LogicOp::And,
            left,
            right,
            ..
        } => {
            let l = sc_ref_outcome_str(left, id, idx);
            let r = sc_ref_outcome_str(right, id, idx);
            format!("(if {{ {l} }} {{ {r} }} else {{ false }})")
        }
        RefExpr::LogicOp {
            op: LogicOp::Or,
            left,
            right,
            ..
        } => {
            let l = sc_ref_outcome_str(left, id, idx);
            let r = sc_ref_outcome_str(right, id, idx);
            format!("(if {{ {l} }} {{ true }} else {{ {r} }})")
        }
        RefExpr::Grouped { inner, .. } => sc_ref_outcome_str(inner, id, idx),
        _ => {
            let i = *idx;
            *idx += 1;
            let val = emit_ref_expr_for_assert(expr, "_");
            format!("{{ __d{id}_e[{i}] = true; __d{id}_c[{i}] = {val}; __d{id}_c[{i}] }}")
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
    cg: &mut RustEmitter,
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
    let coupled = detect_coupled_pairs(&clauses);
    let Some(decision_id) = cg.alloc_mcdc_decision(line, n, DecisionKind::If, coupled) else {
        return false;
    };

    // Emit clause value and eval-flag arrays (short-circuit — only observed clauses are set).
    cg.line(&format!("let mut __d{decision_id}_c = [false; {n}];"));
    cg.line(&format!("let mut __d{decision_id}_e = [false; {n}];"));

    // Emit outcome via short-circuit tree that sets c[i]/e[i] for each leaf when reached.
    cg.indent();
    cg.push(&format!("let __d{decision_id}_outcome: bool = "));
    emit_mcdc_sc_outcome(cg, cond, decision_id, &mut 0, &mut 0);
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
    cg: &mut RustEmitter,
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
    let coupled = detect_coupled_pairs(&clauses);
    let Some(decision_id) = cg.alloc_mcdc_decision(line, n, DecisionKind::While, coupled) else {
        return false;
    };

    // Restructure as `loop` so clause locals can be re-evaluated each iteration.
    cg.indent();
    cg.push("loop {");
    cg.nl();
    cg.push_indent();

    // Clause value and eval-flag arrays.
    cg.line(&format!("let mut __d{decision_id}_c = [false; {n}];"));
    cg.line(&format!("let mut __d{decision_id}_e = [false; {n}];"));

    // Outcome via short-circuit tree.
    cg.indent();
    cg.push(&format!("let __d{decision_id}_outcome: bool = "));
    emit_mcdc_sc_outcome(cg, cond, decision_id, &mut 0, &mut 0);
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

/// Emit MC/DC instrumentation for a compound boolean function-return expression.
///
/// When a production function returns `Bool` and its body ends with a compound
/// `&&`/`||` expression, this wraps the expression with clause arrays, the
/// short-circuit evaluation tree, and an observation record — identical in
/// structure to `emit_mcdc_if` but without any control-flow branching.
///
/// Returns `true` when instrumentation was applied; `false` to fall back to
/// normal emission (simple expression, non-Bool return, or MC/DC inactive).
pub fn emit_mcdc_return_expr(
    cg: &mut RustEmitter,
    expr: &Expr,
    return_type: &TypeExpr,
    line: u32,
) -> bool {
    let is_bool = matches!(return_type, TypeExpr::Base { name, .. } if name == "Bool");
    if !is_bool {
        return false;
    }
    let mut clauses = Vec::new();
    collect_clauses(expr, &mut clauses);
    if clauses.len() <= 1 || cg.mcdc.is_none() {
        return false;
    }
    let n = clauses.len();
    let coupled = detect_coupled_pairs(&clauses);
    let Some(decision_id) = cg.alloc_mcdc_decision(line, n, DecisionKind::Return, coupled) else {
        return false;
    };

    cg.line(&format!("let mut __d{decision_id}_c = [false; {n}];"));
    cg.line(&format!("let mut __d{decision_id}_e = [false; {n}];"));
    cg.indent();
    cg.push(&format!("let __d{decision_id}_outcome: bool = "));
    emit_mcdc_sc_outcome(cg, expr, decision_id, &mut 0, &mut 0);
    cg.push(";");
    cg.nl();
    emit_mcdc_record(cg, decision_id, n);
    cg.indent();
    cg.push(&format!("__d{decision_id}_outcome"));
    cg.nl();
    true
}

/// Emit the short-circuit evaluation tree for a compound boolean condition.
///
/// Sets `__d{id}_c[i]` (clause value) and `__d{id}_e[i]` (evaluated flag) for
/// each leaf clause *only when it is actually reached* by short-circuit execution.
/// Emits a Rust block expression whose value is the overall boolean outcome.
///
/// `clause_idx` counts leaf clauses (left-to-right); `tmp_idx` numbers internal
/// temporaries so nested `&&`/`||` nodes use distinct names.
fn emit_mcdc_sc_outcome(
    cg: &mut RustEmitter,
    expr: &Expr,
    decision_id: usize,
    clause_idx: &mut usize,
    tmp_idx: &mut usize,
) {
    if let Expr::Binary {
        op, left, right, ..
    } = expr
    {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            let t = *tmp_idx;
            *tmp_idx += 1;
            cg.push("{");
            cg.push(&format!(" let __d{decision_id}_t{t} = "));
            emit_mcdc_sc_outcome(cg, left, decision_id, clause_idx, tmp_idx);
            cg.push(";");
            if *op == BinaryOp::And {
                cg.push(&format!(" if __d{decision_id}_t{t} {{ "));
                emit_mcdc_sc_outcome(cg, right, decision_id, clause_idx, tmp_idx);
                cg.push(" } else { false }");
            } else {
                cg.push(&format!(" if __d{decision_id}_t{t} {{ true }} else {{ "));
                emit_mcdc_sc_outcome(cg, right, decision_id, clause_idx, tmp_idx);
                cg.push(" }");
            }
            cg.push(" }");
            return;
        }
    }
    // Leaf: set eval flag, record value, return value.
    let i = *clause_idx;
    *clause_idx += 1;
    cg.push(&format!(
        "{{ __d{decision_id}_e[{i}] = true; __d{decision_id}_c[{i}] = "
    ));
    emit_expr(cg, expr);
    cg.push(&format!("; __d{decision_id}_c[{i}] }}"));
}

/// Emit the `__mvl_mcdc::record(…)` call for a decision with `n` clauses.
///
/// Encoding (u32): bits 0..n-1 = clause vals, bits n..2n-1 = eval flags, bit 2n = outcome.
fn emit_mcdc_record(cg: &mut RustEmitter, decision_id: usize, n: usize) {
    let vals: Vec<String> = (0..n)
        .map(|i| format!("((__d{decision_id}_c[{i}] as u32) << {i}u32)"))
        .collect();
    let evals: Vec<String> = (0..n)
        .map(|i| format!("((__d{decision_id}_e[{i}] as u32) << {}u32)", n + i))
        .collect();
    let outcome = format!("((__d{decision_id}_outcome as u32) << {}u32)", 2 * n);
    let encoded = vals
        .into_iter()
        .chain(evals)
        .chain(std::iter::once(outcome))
        .collect::<Vec<_>>()
        .join(" | ");
    cg.line(&format!(
        "#[cfg(test)] crate::__mvl_mcdc::record({decision_id}usize, {encoded});"
    ));
}
