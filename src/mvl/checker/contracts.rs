// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Function contract checker — verifies `requires` and `ensures` clauses (Issue #621).
//!
//! # Approach
//!
//! Contracts are checked statically using the same 5-layer solver as refinement types.
//! Three outcomes per contract clause:
//!
//! | Outcome      | Meaning                                                           |
//! |--------------|-------------------------------------------------------------------|
//! | Proven       | Clause statically satisfied — no error or runtime code emitted   |
//! | RuntimeCheck | Cannot prove statically — deferred to runtime (no error yet)     |
//! | Failed       | Clause statically violated — compile-time error                  |
//!
//! # `requires` checking
//!
//! For each call site `f(a1, a2, …)`:
//! - Find which single parameter name the `requires` predicate references.
//! - Normalise that parameter name to `"self"` and run the solver on the corresponding arg.
//! - If the predicate references zero or multiple parameters, emit `RuntimeCheck`.
//!
//! # `ensures` checking
//!
//! At each return point (explicit `return e` or implicit tail expression):
//! - Normalise `"result"` to `"self"` in the predicate.
//! - If the predicate contains references to parameter names after normalisation,
//!   emit `RuntimeCheck` (conservative; those are tracked in Phase 2+).
//! - Otherwise run the solver on the returned expression.
//!
//! # `invariant` checking (Phase 3)
//!
//! At each `while` loop entry point:
//! - If the invariant references exactly one variable name, normalise it to `"self"`.
//! - Run the solver on `Expr::Ident(var_name)` with the caller's `var_refs`.
//! - If the predicate references zero names (constant), evaluate it directly.
//! - If the predicate references multiple names: `RuntimeCheck` (Phase 4).
//! - `RefResult::Failed` → `CheckError::InvariantViolated`.

use std::collections::HashMap;

use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::refinements::check_arg_against_pred;
use crate::mvl::checker::solver::RefResult;
use crate::mvl::parser::ast::{
    ArithOp, Block, CmpOp, Decl, ElseBranch, Expr, FnDecl, Literal, LogicOp, MatchBody, Param,
    Program, RefExpr, Stmt,
};
use crate::mvl::parser::lexer::Span;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Check all `requires`/`ensures` clauses for every function and method in `prog`.
pub fn check_contracts(prog: &Program, errors: &mut Vec<CheckError>) {
    let fn_map = build_fn_contract_map(prog);
    let fn_decls = build_fn_decls_for_solver(prog);

    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                // Phase 3: seed var_refs with parameter where-refinements so that
                // `requires` checks on variable arguments (e.g. `f(x)` where
                // `x: Int where self > 0`) can be resolved by the solver.
                let var_refs = build_param_var_refs(&fd.params);
                check_requires_in_block(&fd.body, &fn_map, &var_refs, &fn_decls, errors);
                if !fd.ensures.is_empty() {
                    check_ensures_in_block(
                        &fd.body,
                        &fd.name,
                        &fd.ensures,
                        &fd.params,
                        &fn_decls,
                        errors,
                    );
                }
                // Phase 3: check loop invariants.
                check_invariants_in_block(&fd.body, &fd.name, &var_refs, &fn_decls, errors);
            }
            Decl::Impl(impl_d) => {
                for method in &impl_d.methods {
                    let var_refs = build_param_var_refs(&method.params);
                    check_requires_in_block(&method.body, &fn_map, &var_refs, &fn_decls, errors);
                    if !method.ensures.is_empty() {
                        check_ensures_in_block(
                            &method.body,
                            &method.name,
                            &method.ensures,
                            &method.params,
                            &fn_decls,
                            errors,
                        );
                    }
                    // Phase 3: check loop invariants.
                    check_invariants_in_block(
                        &method.body,
                        &method.name,
                        &var_refs,
                        &fn_decls,
                        errors,
                    );
                }
            }
            _ => {}
        }
    }
}

// ── Lookup table builders ─────────────────────────────────────────────────────

/// Per-function contract information used when checking call sites.
struct FnContracts {
    params: Vec<Param>,
    requires: Vec<RefExpr>,
}

fn build_fn_contract_map(prog: &Program) -> HashMap<String, FnContracts> {
    let mut map = HashMap::new();
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) if !fd.requires.is_empty() => {
                map.insert(
                    fd.name.clone(),
                    FnContracts {
                        params: fd.params.clone(),
                        requires: fd.requires.clone(),
                    },
                );
            }
            Decl::Impl(impl_d) => {
                for method in &impl_d.methods {
                    if !method.requires.is_empty() {
                        map.insert(
                            method.name.clone(),
                            FnContracts {
                                params: method.params.clone(),
                                requires: method.requires.clone(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    map
}

/// Pure function map forwarded to the solver for constant folding.
fn build_fn_decls_for_solver(prog: &Program) -> HashMap<String, FnDecl> {
    let mut map = HashMap::new();
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            if fd.effects.is_empty() {
                map.insert(fd.name.clone(), fd.clone());
            }
        }
    }
    map
}

// ── requires: call-site checker ───────────────────────────────────────────────

fn check_requires_in_block(
    block: &Block,
    fn_map: &HashMap<String, FnContracts>,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    for stmt in &block.stmts {
        check_requires_in_stmt(stmt, fn_map, var_refs, fn_decls, errors);
    }
}

fn check_requires_in_stmt(
    stmt: &Stmt,
    fn_map: &HashMap<String, FnContracts>,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    match stmt {
        Stmt::Let { init, .. } => {
            check_requires_in_expr(init, fn_map, var_refs, fn_decls, errors);
        }
        Stmt::Assign { value, .. } => {
            check_requires_in_expr(value, fn_map, var_refs, fn_decls, errors);
        }
        Stmt::Return { value: Some(e), .. } => {
            check_requires_in_expr(e, fn_map, var_refs, fn_decls, errors);
        }
        Stmt::Return { value: None, .. } => {}
        Stmt::Expr { expr, .. } => {
            check_requires_in_expr(expr, fn_map, var_refs, fn_decls, errors);
        }
        Stmt::If {
            cond, then, else_, ..
        } => {
            check_requires_in_expr(cond, fn_map, var_refs, fn_decls, errors);
            check_requires_in_block(then, fn_map, var_refs, fn_decls, errors);
            if let Some(eb) = else_ {
                match eb {
                    ElseBranch::Block(b) => {
                        check_requires_in_block(b, fn_map, var_refs, fn_decls, errors)
                    }
                    ElseBranch::If(s) => {
                        check_requires_in_stmt(s, fn_map, var_refs, fn_decls, errors)
                    }
                }
            }
        }
        Stmt::While { cond, body, .. } => {
            check_requires_in_expr(cond, fn_map, var_refs, fn_decls, errors);
            check_requires_in_block(body, fn_map, var_refs, fn_decls, errors);
        }
        Stmt::For { iter, body, .. } => {
            check_requires_in_expr(iter, fn_map, var_refs, fn_decls, errors);
            check_requires_in_block(body, fn_map, var_refs, fn_decls, errors);
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            check_requires_in_expr(scrutinee, fn_map, var_refs, fn_decls, errors);
            for arm in arms {
                check_requires_in_match_body(&arm.body, fn_map, var_refs, fn_decls, errors);
            }
        }
    }
}

fn check_requires_in_expr(
    expr: &Expr,
    fn_map: &HashMap<String, FnContracts>,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    match expr {
        Expr::FnCall {
            name, args, span, ..
        } => {
            // Recurse into arguments first.
            for arg in args.iter() {
                check_requires_in_expr(arg, fn_map, var_refs, fn_decls, errors);
            }
            // Then check requires clauses for this callee.
            if let Some(contracts) = fn_map.get(name) {
                check_requires_at_call(name, args, *span, contracts, var_refs, fn_decls, errors);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            check_requires_in_expr(receiver, fn_map, var_refs, fn_decls, errors);
            for arg in args {
                check_requires_in_expr(arg, fn_map, var_refs, fn_decls, errors);
            }
        }
        Expr::Block(b) => {
            check_requires_in_block(b, fn_map, var_refs, fn_decls, errors);
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            check_requires_in_expr(cond, fn_map, var_refs, fn_decls, errors);
            check_requires_in_block(then, fn_map, var_refs, fn_decls, errors);
            if let Some(e) = else_ {
                check_requires_in_expr(e, fn_map, var_refs, fn_decls, errors);
            }
        }
        Expr::Borrow { expr, .. }
        | Expr::Unary { expr, .. }
        | Expr::Move { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Declassify { expr, .. }
        | Expr::Sanitize { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::FieldAccess { expr, .. } => {
            check_requires_in_expr(expr, fn_map, var_refs, fn_decls, errors);
        }
        Expr::Binary { left, right, .. } => {
            check_requires_in_expr(left, fn_map, var_refs, fn_decls, errors);
            check_requires_in_expr(right, fn_map, var_refs, fn_decls, errors);
        }
        Expr::Construct { fields, .. } => {
            for (_, e) in fields {
                check_requires_in_expr(e, fn_map, var_refs, fn_decls, errors);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                check_requires_in_expr(e, fn_map, var_refs, fn_decls, errors);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                check_requires_in_expr(k, fn_map, var_refs, fn_decls, errors);
                check_requires_in_expr(v, fn_map, var_refs, fn_decls, errors);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            check_requires_in_expr(scrutinee, fn_map, var_refs, fn_decls, errors);
            for arm in arms {
                check_requires_in_match_body(&arm.body, fn_map, var_refs, fn_decls, errors);
            }
        }
        Expr::Lambda { body, .. } => {
            check_requires_in_expr(body, fn_map, var_refs, fn_decls, errors);
        }
        // Leaves: Literal, Ident — no sub-expressions.
        Expr::Literal(_, _) | Expr::Ident(_, _) => {}
    }
}

fn check_requires_in_match_body(
    body: &MatchBody,
    fn_map: &HashMap<String, FnContracts>,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    match body {
        MatchBody::Expr(e) => check_requires_in_expr(e, fn_map, var_refs, fn_decls, errors),
        MatchBody::Block(b) => check_requires_in_block(b, fn_map, var_refs, fn_decls, errors),
    }
}

/// Check all `requires` clauses for a single call site.
fn check_requires_at_call(
    fn_name: &str,
    args: &[Expr],
    call_span: Span,
    contracts: &FnContracts,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    let params = &contracts.params;

    for req_pred in &contracts.requires {
        // Find which single parameter name this predicate references.
        match single_param_ref(req_pred, params) {
            Some((param_idx, param_name)) if param_idx < args.len() => {
                let normalized = normalize_pred(req_pred, &param_name);
                let arg = &args[param_idx];
                let outcome = check_arg_against_pred(arg, &normalized, var_refs, fn_decls);
                if outcome == RefResult::Failed {
                    errors.push(CheckError::PreconditionViolated {
                        fn_name: fn_name.to_string(),
                        pred: display_pred(req_pred),
                        span: call_span,
                        counterexample: None,
                    });
                }
                // Proven or RuntimeCheck: silent at compile time.
            }
            _ => {
                // Phase 2: try multi-param substitution when all referenced args are literals.
                check_multi_param_requires_literal(
                    fn_name, req_pred, params, args, var_refs, fn_decls, errors, call_span,
                );
            }
        }
    }
}

// ── ensures: return-point checker ────────────────────────────────────────────

fn check_ensures_in_block(
    block: &Block,
    fn_name: &str,
    ensures: &[RefExpr],
    params: &[Param],
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    for (i, stmt) in block.stmts.iter().enumerate() {
        match stmt {
            Stmt::Return {
                value: Some(ret_expr),
                span,
            } => {
                check_ensures_for_return(
                    ret_expr, *span, fn_name, ensures, params, fn_decls, errors,
                );
            }
            Stmt::Return { value: None, .. } => {
                // `return;` returns Unit — nothing to check against ensures.
            }
            Stmt::If { then, else_, .. } => {
                check_ensures_in_block(then, fn_name, ensures, params, fn_decls, errors);
                if let Some(eb) = else_ {
                    match eb {
                        ElseBranch::Block(b) => {
                            check_ensures_in_block(b, fn_name, ensures, params, fn_decls, errors)
                        }
                        ElseBranch::If(s) => {
                            check_ensures_in_stmt(s, fn_name, ensures, params, fn_decls, errors)
                        }
                    }
                }
            }
            Stmt::While { body, .. } => {
                check_ensures_in_block(body, fn_name, ensures, params, fn_decls, errors);
            }
            Stmt::For { body, .. } => {
                check_ensures_in_block(body, fn_name, ensures, params, fn_decls, errors);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    check_ensures_in_match_body(
                        &arm.body, fn_name, ensures, params, fn_decls, errors,
                    );
                }
            }
            // Tail expression (implicit return) — last Stmt::Expr in the block.
            Stmt::Expr { expr, span } if i + 1 == block.stmts.len() => {
                check_ensures_for_return(expr, *span, fn_name, ensures, params, fn_decls, errors);
            }
            _ => {}
        }
    }
}

fn check_ensures_in_stmt(
    stmt: &Stmt,
    fn_name: &str,
    ensures: &[RefExpr],
    params: &[Param],
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    // Recursion helper for else-if chains.
    if let Stmt::If { then, else_, .. } = stmt {
        check_ensures_in_block(then, fn_name, ensures, params, fn_decls, errors);
        if let Some(eb) = else_ {
            match eb {
                ElseBranch::Block(b) => {
                    check_ensures_in_block(b, fn_name, ensures, params, fn_decls, errors)
                }
                ElseBranch::If(s) => {
                    check_ensures_in_stmt(s, fn_name, ensures, params, fn_decls, errors)
                }
            }
        }
    }
}

fn check_ensures_in_match_body(
    body: &MatchBody,
    fn_name: &str,
    ensures: &[RefExpr],
    params: &[Param],
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    match body {
        MatchBody::Block(b) => {
            check_ensures_in_block(b, fn_name, ensures, params, fn_decls, errors)
        }
        MatchBody::Expr(e) => {
            // MatchBody::Expr is a tail expression — treated as a return point.
            let span = e.span();
            check_ensures_for_return(e, span, fn_name, ensures, params, fn_decls, errors);
        }
    }
}

/// Check all `ensures` clauses against a single return expression.
///
/// Phase 2: builds `var_refs` from the function's own parameter `where`-refinements
/// so that the solver can reason about parameter values symbolically.  The
/// `has_param_ref` guard from Phase 1 is removed — the solver (Layer 4 Cooper)
/// already handles linear multi-variable arithmetic like `n + 1 >= n`.
fn check_ensures_for_return(
    ret_expr: &Expr,
    ret_span: Span,
    fn_name: &str,
    ensures: &[RefExpr],
    params: &[Param],
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    // Phase 2: populate var_refs with each parameter's inline where-predicate
    // (normalised so the param name becomes "self").  This lets Layer 2 and
    // Layer 4 prove postconditions like `ensures result >= 0` when the function
    // parameter is annotated `n: Int where self >= 0`.
    let var_refs = build_param_var_refs(params);

    for ens_pred in ensures {
        // Normalise "result" → "self" so the solver recognises the return value.
        let normalized = normalize_pred(ens_pred, "result");

        // Let the solver decide: Proven (silent), Failed (emit error),
        // or RuntimeCheck (silent — deferred to runtime).
        let outcome = check_arg_against_pred(ret_expr, &normalized, &var_refs, fn_decls);
        if outcome == RefResult::Failed {
            errors.push(CheckError::PostconditionViolated {
                fn_name: fn_name.to_string(),
                pred: display_pred(ens_pred),
                span: ret_span,
                counterexample: None,
            });
        }
    }
}

// ── Predicate helpers ─────────────────────────────────────────────────────────

/// Find which single parameter the predicate references.
/// Returns `Some((param_index, param_name))` if exactly one distinct param is referenced,
/// `None` if zero or multiple different params are referenced.
fn single_param_ref(pred: &RefExpr, params: &[Param]) -> Option<(usize, String)> {
    let idents = collect_ident_names(pred);
    let mut found: Option<(usize, String)> = None;
    for ident in &idents {
        if let Some(idx) = params.iter().position(|p| &p.name == ident) {
            match &found {
                None => found = Some((idx, ident.clone())),
                Some((prev_idx, _)) if *prev_idx == idx => {} // same param, fine
                Some(_) => return None,                       // multiple different params
            }
        }
    }
    found
}

/// Replace every occurrence of `old_name` with `"self"` in a predicate.
fn normalize_pred(pred: &RefExpr, old_name: &str) -> RefExpr {
    match pred {
        RefExpr::Ident { name, span } => RefExpr::Ident {
            name: if name == old_name {
                "self".to_string()
            } else {
                name.clone()
            },
            span: *span,
        },
        RefExpr::Compare {
            op,
            left,
            right,
            span,
        } => RefExpr::Compare {
            op: *op,
            left: Box::new(normalize_pred(left, old_name)),
            right: Box::new(normalize_pred(right, old_name)),
            span: *span,
        },
        RefExpr::LogicOp {
            op,
            left,
            right,
            span,
        } => RefExpr::LogicOp {
            op: *op,
            left: Box::new(normalize_pred(left, old_name)),
            right: Box::new(normalize_pred(right, old_name)),
            span: *span,
        },
        RefExpr::ArithOp {
            op,
            left,
            right,
            span,
        } => RefExpr::ArithOp {
            op: *op,
            left: Box::new(normalize_pred(left, old_name)),
            right: Box::new(normalize_pred(right, old_name)),
            span: *span,
        },
        RefExpr::Not { inner, span } => RefExpr::Not {
            inner: Box::new(normalize_pred(inner, old_name)),
            span: *span,
        },
        RefExpr::Grouped { inner, span } => RefExpr::Grouped {
            inner: Box::new(normalize_pred(inner, old_name)),
            span: *span,
        },
        RefExpr::Old { inner, span } => RefExpr::Old {
            inner: Box::new(normalize_pred(inner, old_name)),
            span: *span,
        },
        // Leaves unchanged.
        RefExpr::Integer { .. } | RefExpr::Float { .. } | RefExpr::Len { .. } => pred.clone(),
    }
}

// ── Phase 2 helpers ───────────────────────────────────────────────────────────

/// Build a `var_refs` map from a function's parameter inline `where`-refinements.
///
/// Each predicate is normalised so the parameter name becomes `"self"`,
/// matching the form expected by the 5-layer solver.
fn build_param_var_refs(params: &[Param]) -> HashMap<String, Option<RefExpr>> {
    params
        .iter()
        .map(|p| {
            let pred = p.refinement.as_ref().map(|r| normalize_pred(r, &p.name));
            (p.name.clone(), pred)
        })
        .collect()
}

/// Substitute every `RefExpr::Ident { name == old_name }` with `new_val`.
///
/// Used to replace non-primary parameter names with their literal argument
/// values before dispatching to the single-variable solver.
fn subst_pred_ident(pred: &RefExpr, old_name: &str, new_val: &RefExpr) -> RefExpr {
    match pred {
        RefExpr::Ident { name, .. } if name == old_name => new_val.clone(),
        RefExpr::Ident { .. } => pred.clone(),
        RefExpr::Compare {
            op,
            left,
            right,
            span,
        } => RefExpr::Compare {
            op: *op,
            left: Box::new(subst_pred_ident(left, old_name, new_val)),
            right: Box::new(subst_pred_ident(right, old_name, new_val)),
            span: *span,
        },
        RefExpr::LogicOp {
            op,
            left,
            right,
            span,
        } => RefExpr::LogicOp {
            op: *op,
            left: Box::new(subst_pred_ident(left, old_name, new_val)),
            right: Box::new(subst_pred_ident(right, old_name, new_val)),
            span: *span,
        },
        RefExpr::ArithOp {
            op,
            left,
            right,
            span,
        } => RefExpr::ArithOp {
            op: *op,
            left: Box::new(subst_pred_ident(left, old_name, new_val)),
            right: Box::new(subst_pred_ident(right, old_name, new_val)),
            span: *span,
        },
        RefExpr::Not { inner, span } => RefExpr::Not {
            inner: Box::new(subst_pred_ident(inner, old_name, new_val)),
            span: *span,
        },
        RefExpr::Grouped { inner, span } => RefExpr::Grouped {
            inner: Box::new(subst_pred_ident(inner, old_name, new_val)),
            span: *span,
        },
        // Substituting inside old(e) is correct here: all current callers supply a
        // call-site literal as new_val, which represents the value at call time — the same
        // moment old(e) refers to. If this function is ever used with post-call values,
        // re-evaluate whether substituting inside Old remains sound.
        RefExpr::Old { inner, span } => RefExpr::Old {
            inner: Box::new(subst_pred_ident(inner, old_name, new_val)),
            span: *span,
        },
        RefExpr::Integer { .. } | RefExpr::Float { .. } | RefExpr::Len { .. } => pred.clone(),
    }
}

/// Convert a simple `Expr` to a `RefExpr` literal for predicate substitution.
///
/// Only integer and float literals are converted; returns `None` for anything
/// more complex, causing the multi-param check to fall back to `RuntimeCheck`.
fn expr_to_ref_expr(expr: &Expr) -> Option<RefExpr> {
    match expr {
        Expr::Literal(Literal::Integer(n), span) => Some(RefExpr::Integer {
            value: *n,
            span: *span,
        }),
        Expr::Literal(Literal::Float(f), span) => Some(RefExpr::Float {
            value: *f,
            span: *span,
        }),
        _ => None,
    }
}

/// Phase 2: check a multi-parameter `requires` predicate when all non-primary
/// argument expressions are integer or float literals.
///
/// Algorithm:
/// 1. Collect the distinct parameters referenced in the predicate.
/// 2. Pick the lowest-indexed one as "primary" (mapped to `"self"`).
/// 3. For each remaining parameter, convert its argument to a `RefExpr` literal.
///    If any argument is not a literal, bail out silently (`RuntimeCheck`).
/// 4. Substitute all non-primary names in the predicate with their literal values.
/// 5. Call the standard single-variable solver on the primary argument.
#[allow(clippy::too_many_arguments)]
fn check_multi_param_requires_literal(
    fn_name: &str,
    pred: &RefExpr,
    params: &[Param],
    args: &[Expr],
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
    call_span: Span,
) {
    // Collect distinct param indices in order of first appearance.
    let idents = collect_ident_names(pred);
    let mut param_refs: Vec<(usize, String)> = Vec::new();
    for ident in &idents {
        if let Some(idx) = params.iter().position(|p| &p.name == ident) {
            if !param_refs.iter().any(|(i, _)| *i == idx) {
                param_refs.push((idx, ident.clone()));
            }
        }
    }

    // Need at least two distinct params for multi-param checking.
    if param_refs.len() < 2 {
        return;
    }

    // Sort by param index so the primary is the lowest-indexed param.
    param_refs.sort_by_key(|(idx, _)| *idx);
    let (primary_idx, primary_name) = &param_refs[0];

    // Normalise primary name → "self".
    let mut modified_pred = normalize_pred(pred, primary_name);

    // Substitute each non-primary param with its literal arg value.
    for (other_idx, other_name) in &param_refs[1..] {
        if *other_idx >= args.len() {
            return; // Arg count mismatch — bail.
        }
        match expr_to_ref_expr(&args[*other_idx]) {
            Some(ref_val) => {
                modified_pred = subst_pred_ident(&modified_pred, other_name, &ref_val);
            }
            None => return, // Non-literal arg — stay RuntimeCheck.
        }
    }

    if *primary_idx >= args.len() {
        return;
    }

    let outcome = check_arg_against_pred(&args[*primary_idx], &modified_pred, var_refs, fn_decls);
    if outcome == RefResult::Failed {
        errors.push(CheckError::PreconditionViolated {
            fn_name: fn_name.to_string(),
            pred: display_pred(pred),
            span: call_span,
            counterexample: None,
        });
    }
}

// ── Phase 3: invariant checker ────────────────────────────────────────────────

/// Walk a block and check every `while` loop's invariants at loop entry.
fn check_invariants_in_block(
    block: &Block,
    fn_name: &str,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    for stmt in &block.stmts {
        check_invariants_in_stmt(stmt, fn_name, var_refs, fn_decls, errors);
    }
}

fn check_invariants_in_stmt(
    stmt: &Stmt,
    fn_name: &str,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
) {
    match stmt {
        Stmt::While {
            invariants,
            body,
            span,
            ..
        } => {
            for inv_pred in invariants {
                check_invariant_at_entry(fn_name, inv_pred, var_refs, fn_decls, errors, *span);
            }
            // Recurse into the body for nested loops.
            check_invariants_in_block(body, fn_name, var_refs, fn_decls, errors);
        }
        Stmt::If { then, else_, .. } => {
            check_invariants_in_block(then, fn_name, var_refs, fn_decls, errors);
            if let Some(eb) = else_ {
                match eb {
                    ElseBranch::Block(b) => {
                        check_invariants_in_block(b, fn_name, var_refs, fn_decls, errors)
                    }
                    ElseBranch::If(s) => {
                        check_invariants_in_stmt(s, fn_name, var_refs, fn_decls, errors)
                    }
                }
            }
        }
        Stmt::For { body, .. } => {
            check_invariants_in_block(body, fn_name, var_refs, fn_decls, errors);
        }
        Stmt::Match { arms, .. } => {
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => {
                        check_invariants_in_block(b, fn_name, var_refs, fn_decls, errors)
                    }
                    MatchBody::Expr(_) => {}
                }
            }
        }
        _ => {}
    }
}

/// Check a single `invariant` predicate at loop entry.
///
/// Strategy:
/// - 0 free variable names: constant predicate — check directly with a dummy literal argument.
/// - 1 free variable name: normalise it to `"self"` and run the solver on `Ident(name)`.
/// - 2+ free variable names: `RuntimeCheck` (multi-variable reasoning is Phase 4).
fn check_invariant_at_entry(
    fn_name: &str,
    inv_pred: &RefExpr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
    loop_span: Span,
) {
    let idents = collect_ident_names(inv_pred);

    // Deduplicate while preserving order.
    let mut distinct: Vec<String> = Vec::new();
    for id in &idents {
        if !distinct.contains(id) {
            distinct.push(id.clone());
        }
    }

    match distinct.as_slice() {
        [] => {
            // Constant predicate (e.g., `invariant 0 >= 0` or `invariant 1 < 0`).
            // The predicate has no `self` reference; pass a dummy literal as the argument.
            // Layer 1 will const-fold the comparison directly.
            let dummy = Expr::Literal(Literal::Integer(0), loop_span);
            let outcome = check_arg_against_pred(&dummy, inv_pred, var_refs, fn_decls);
            if outcome == RefResult::Failed {
                errors.push(CheckError::InvariantViolated {
                    fn_name: fn_name.to_string(),
                    pred: display_pred(inv_pred),
                    span: loop_span,
                    counterexample: None,
                });
            }
        }
        [var_name] => {
            // Single free variable — normalise it to "self" and check via Ident lookup.
            let normalized = normalize_pred(inv_pred, var_name);
            let ident_expr = Expr::Ident(var_name.clone(), loop_span);
            let outcome = check_arg_against_pred(&ident_expr, &normalized, var_refs, fn_decls);
            if outcome == RefResult::Failed {
                errors.push(CheckError::InvariantViolated {
                    fn_name: fn_name.to_string(),
                    pred: display_pred(inv_pred),
                    span: loop_span,
                    counterexample: None,
                });
            }
            // Proven or RuntimeCheck: silent at compile time.
        }
        _ => {
            // Multiple variables: defer to Phase 4 (RuntimeCheck, no error).
        }
    }
}

/// Collect all identifier names referenced in a predicate (may contain duplicates).
fn collect_ident_names(pred: &RefExpr) -> Vec<String> {
    let mut names = Vec::new();
    collect_idents_inner(pred, &mut names);
    names
}

fn collect_idents_inner(pred: &RefExpr, names: &mut Vec<String>) {
    match pred {
        RefExpr::Ident { name, .. } => names.push(name.clone()),
        RefExpr::Compare { left, right, .. }
        | RefExpr::LogicOp { left, right, .. }
        | RefExpr::ArithOp { left, right, .. } => {
            collect_idents_inner(left, names);
            collect_idents_inner(right, names);
        }
        RefExpr::Not { inner, .. }
        | RefExpr::Grouped { inner, .. }
        | RefExpr::Old { inner, .. } => {
            collect_idents_inner(inner, names);
        }
        RefExpr::Len { ident, .. } => names.push(ident.clone()),
        RefExpr::Integer { .. } | RefExpr::Float { .. } => {}
    }
}

/// Format a predicate for error messages.
fn display_pred(pred: &RefExpr) -> String {
    match pred {
        RefExpr::Ident { name, .. } => name.clone(),
        RefExpr::Integer { value, .. } => value.to_string(),
        RefExpr::Float { value, .. } => value.to_string(),
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let op_str = match op {
                CmpOp::Eq => "==",
                CmpOp::Ne => "!=",
                CmpOp::Lt => "<",
                CmpOp::Gt => ">",
                CmpOp::Le => "<=",
                CmpOp::Ge => ">=",
            };
            format!("{} {op_str} {}", display_pred(left), display_pred(right))
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => {
            let op_str = match op {
                LogicOp::And => "&&",
                LogicOp::Or => "||",
            };
            format!("{} {op_str} {}", display_pred(left), display_pred(right))
        }
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let op_str = match op {
                ArithOp::Add => "+",
                ArithOp::Sub => "-",
                ArithOp::Mul => "*",
                ArithOp::Div => "/",
                ArithOp::Rem => "%",
            };
            format!("{} {op_str} {}", display_pred(left), display_pred(right))
        }
        RefExpr::Not { inner, .. } => format!("!{}", display_pred(inner)),
        RefExpr::Grouped { inner, .. } => format!("({})", display_pred(inner)),
        RefExpr::Old { inner, .. } => format!("old({})", display_pred(inner)),
        RefExpr::Len { ident, .. } => format!("len({ident})"),
    }
}
