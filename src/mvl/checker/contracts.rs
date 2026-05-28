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
use crate::mvl::checker::refinements::{
    check_arg_against_pred_counted, ProofEntry, RefinementCounts,
};
use crate::mvl::checker::solver::{RefResult, SolverMode};
use crate::mvl::parser::ast::{
    expr_to_ref_expr_ext, ActorDecl, ArithOp, Block, CmpOp, Decl, ElseBranch, Expr, FieldDecl,
    FnDecl, LValue, LetKind, Literal, LogicOp, MatchBody, Param, Program, RefExpr, Stmt, TypeBody,
    UnaryOp, VariantFields,
};
use crate::mvl::parser::lexer::Span;

// ── Shared checker context ────────────────────────────────────────────────────

/// Context passed to all internal contract-checking functions.
///
/// Bundles the three parameters that are shared across every checker call:
/// the pure-function map for the solver, the error accumulator, and the
/// proof-layer counts (replaces the old `thread_local! CONTRACT_COUNTS`).
/// Solver mode is embedded in `counts.mode`.
struct ContractCheckCtx<'a> {
    fn_decls: &'a HashMap<String, FnDecl>,
    errors: &'a mut Vec<CheckError>,
    counts: &'a mut RefinementCounts,
}

// ── Generic AST walkers ───────────────────────────────────────────────────────

/// Visit every `Expr` in `block`, calling `f` on each one.
///
/// Traversal is post-order: sub-expressions are visited before the expression
/// that contains them, so inner call sites are checked before outer ones.
/// This matches the previous per-walker behaviour.
fn walk_stmts<F>(block: &Block, ctx: &mut ContractCheckCtx<'_>, f: &mut F)
where
    F: FnMut(&Expr, &mut ContractCheckCtx<'_>),
{
    for stmt in &block.stmts {
        walk_stmt_exprs(stmt, ctx, f);
    }
}

fn walk_stmt_exprs<F>(stmt: &Stmt, ctx: &mut ContractCheckCtx<'_>, f: &mut F)
where
    F: FnMut(&Expr, &mut ContractCheckCtx<'_>),
{
    match stmt {
        Stmt::Let { init, .. } => walk_expr(init, ctx, f),
        Stmt::Assign { value, .. } => walk_expr(value, ctx, f),
        Stmt::Return { value: Some(e), .. } => walk_expr(e, ctx, f),
        Stmt::Return { value: None, .. } => {}
        Stmt::Expr { expr, .. } => walk_expr(expr, ctx, f),
        Stmt::If {
            cond, then, else_, ..
        } => {
            walk_expr(cond, ctx, f);
            walk_stmts(then, ctx, f);
            match else_ {
                None => {}
                Some(ElseBranch::Block(b)) => walk_stmts(b, ctx, f),
                Some(ElseBranch::If(s)) => walk_stmt_exprs(s, ctx, f),
            }
        }
        Stmt::While { cond, body, .. } => {
            walk_expr(cond, ctx, f);
            walk_stmts(body, ctx, f);
        }
        Stmt::For { iter, body, .. } => {
            walk_expr(iter, ctx, f);
            walk_stmts(body, ctx, f);
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            walk_expr(scrutinee, ctx, f);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => walk_expr(e, ctx, f),
                    MatchBody::Block(b) => walk_stmts(b, ctx, f),
                }
            }
        }
    }
}

fn walk_expr<F>(expr: &Expr, ctx: &mut ContractCheckCtx<'_>, f: &mut F)
where
    F: FnMut(&Expr, &mut ContractCheckCtx<'_>),
{
    // Post-order: recurse into sub-expressions first, then call the visitor.
    match expr {
        Expr::FnCall { args, .. } => {
            for a in args {
                walk_expr(a, ctx, f);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            walk_expr(receiver, ctx, f);
            for a in args {
                walk_expr(a, ctx, f);
            }
        }
        Expr::Block(b) => {
            walk_stmts(b, ctx, f);
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            walk_expr(cond, ctx, f);
            walk_stmts(then, ctx, f);
            if let Some(e) = else_ {
                walk_expr(e, ctx, f);
            }
        }
        Expr::Binary { left, right, .. } => {
            walk_expr(left, ctx, f);
            walk_expr(right, ctx, f);
        }
        Expr::Borrow { expr, .. }
        | Expr::Unary { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Relabel { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::FieldAccess { expr, .. } => {
            walk_expr(expr, ctx, f);
        }
        Expr::Construct { fields, .. } => {
            for (_, e) in fields {
                walk_expr(e, ctx, f);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                walk_expr(e, ctx, f);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                walk_expr(k, ctx, f);
                walk_expr(v, ctx, f);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            walk_expr(scrutinee, ctx, f);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => walk_expr(e, ctx, f),
                    MatchBody::Block(b) => walk_stmts(b, ctx, f),
                }
            }
        }
        Expr::Lambda { body, .. } => {
            walk_expr(body, ctx, f);
        }
        Expr::Spawn { fields, .. } => {
            for (_, v) in fields {
                walk_expr(v, ctx, f);
            }
        }
        Expr::Select { arms, .. } => {
            for arm in arms {
                walk_expr(&arm.expr, ctx, f);
                walk_stmts(&arm.body, ctx, f);
            }
        }
        // Leaves — no sub-expressions to walk.
        Expr::Literal(..) | Expr::Ident(..) | Expr::Quantifier(..) => {}
    }
    f(expr, ctx);
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Check all `requires`/`ensures` clauses for every function and method in `prog`.
/// Returns proof-layer counts for contract checks (ensures, requires, invariants).
pub fn check_contracts(
    prog: &Program,
    errors: &mut Vec<CheckError>,
    mode: SolverMode,
) -> RefinementCounts {
    let fn_map = build_fn_contract_map(prog);
    let fn_decls = build_fn_decls_for_solver(prog);
    let mut counts = RefinementCounts {
        mode,
        ..Default::default()
    };
    {
        let mut ctx = ContractCheckCtx {
            fn_decls: &fn_decls,
            errors,
            counts: &mut counts,
        };

        for decl in &prog.declarations {
            match decl {
                Decl::Fn(fd) => {
                    // Phase 3: seed var_refs with parameter where-refinements so that
                    // `requires` checks on variable arguments (e.g. `f(x)` where
                    // `x: Int where self > 0`) can be resolved by the solver.
                    let var_refs = build_param_var_refs(&fd.params);
                    walk_stmts(&fd.body, &mut ctx, &mut |expr, ctx| {
                        if let Expr::FnCall {
                            name, args, span, ..
                        } = expr
                        {
                            if let Some(contracts) = fn_map.get(name) {
                                check_requires_at_call(
                                    name, args, *span, contracts, &var_refs, ctx,
                                );
                            }
                        }
                    });
                    if !fd.ensures.is_empty() {
                        check_ensures_in_block(
                            &fd.body,
                            &fd.name,
                            &fd.ensures,
                            &fd.params,
                            &mut ctx,
                        );
                    }
                    // Phase 3: check loop invariants.
                    check_invariants_in_block(&fd.body, &fd.name, &var_refs, &mut ctx);
                }
                Decl::Impl(impl_d) => {
                    for method in &impl_d.methods {
                        let var_refs = build_param_var_refs(&method.params);
                        walk_stmts(&method.body, &mut ctx, &mut |expr, ctx| {
                            if let Expr::FnCall {
                                name, args, span, ..
                            } = expr
                            {
                                if let Some(contracts) = fn_map.get(name) {
                                    check_requires_at_call(
                                        name, args, *span, contracts, &var_refs, ctx,
                                    );
                                }
                            }
                        });
                        if !method.ensures.is_empty() {
                            check_ensures_in_block(
                                &method.body,
                                &method.name,
                                &method.ensures,
                                &method.params,
                                &mut ctx,
                            );
                        }
                        // Phase 3: check loop invariants.
                        check_invariants_in_block(&method.body, &method.name, &var_refs, &mut ctx);
                    }
                }
                // D2 (Phase 8, #37): Actor behavior bodies must satisfy the same contract
                // rules as regular functions — `requires` clauses on called functions are
                // checked, and loop invariants within behavior bodies are verified.
                Decl::Actor(ad) => {
                    check_actor_behavior_contracts(ad, &fn_map, &mut ctx);
                }
                _ => {}
            }
        }
    }
    // Return accumulated proof-layer counts from all contract checks.
    counts
}

// ── Return type refinement checking (#1067 Gap 3) ────────────────────────────

/// Check that every return point in functions with a `return_refinement`
/// (`-> T where self > 0`) satisfies the declared predicate.
///
/// Analogous to `check_ensures` but operates directly on `FnDecl.return_refinement`
/// (a `RefExpr` already normalised to "self"), rather than an `ensures` clause.
pub fn check_return_refinements(prog: &Program, errors: &mut Vec<CheckError>, mode: SolverMode) {
    let fn_decls = build_fn_decls_for_solver(prog);
    let mut counts = RefinementCounts {
        mode,
        ..Default::default()
    };
    let mut ctx = ContractCheckCtx {
        fn_decls: &fn_decls,
        errors,
        counts: &mut counts,
    };
    for decl in &prog.declarations {
        let fns: Vec<&FnDecl> = match decl {
            Decl::Fn(fd) => vec![fd],
            Decl::Impl(id) => id.methods.iter().collect(),
            _ => vec![],
        };
        for fd in fns {
            if let Some(ret_pred) = &fd.return_refinement {
                let var_refs = build_param_var_refs(&fd.params);
                check_return_pred_in_block(&fd.body, &fd.name, ret_pred, &var_refs, &mut ctx);
            }
        }
    }
}

fn check_return_pred_in_block(
    block: &Block,
    fn_name: &str,
    ret_pred: &RefExpr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    ctx: &mut ContractCheckCtx<'_>,
) {
    for (i, stmt) in block.stmts.iter().enumerate() {
        match stmt {
            Stmt::Return {
                value: Some(ret_expr),
                span,
            } => {
                check_return_pred_for_expr(ret_expr, *span, fn_name, ret_pred, var_refs, ctx);
            }
            Stmt::Return { value: None, .. } => {}
            Stmt::If { then, else_, .. } => {
                check_return_pred_in_block(then, fn_name, ret_pred, var_refs, ctx);
                if let Some(eb) = else_ {
                    match eb {
                        ElseBranch::Block(b) => {
                            check_return_pred_in_block(b, fn_name, ret_pred, var_refs, ctx)
                        }
                        ElseBranch::If(s) => {
                            check_return_pred_in_stmt(s, fn_name, ret_pred, var_refs, ctx)
                        }
                    }
                }
            }
            Stmt::While { body, .. } => {
                check_return_pred_in_block(body, fn_name, ret_pred, var_refs, ctx);
            }
            Stmt::For { body, .. } => {
                check_return_pred_in_block(body, fn_name, ret_pred, var_refs, ctx);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(e) => {
                            // Match arm expr as tail position (last stmt).
                            if i + 1 == block.stmts.len() {
                                let span = e.span();
                                check_return_pred_for_expr(
                                    e, span, fn_name, ret_pred, var_refs, ctx,
                                );
                            }
                        }
                        MatchBody::Block(b) => {
                            check_return_pred_in_block(b, fn_name, ret_pred, var_refs, ctx)
                        }
                    }
                }
            }
            // Tail expression (implicit return) — last Stmt::Expr in the block.
            Stmt::Expr { expr, span } if i + 1 == block.stmts.len() => {
                check_return_pred_for_expr(expr, *span, fn_name, ret_pred, var_refs, ctx);
            }
            _ => {}
        }
    }
}

fn check_return_pred_in_stmt(
    stmt: &Stmt,
    fn_name: &str,
    ret_pred: &RefExpr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    ctx: &mut ContractCheckCtx<'_>,
) {
    if let Stmt::If { then, else_, .. } = stmt {
        check_return_pred_in_block(then, fn_name, ret_pred, var_refs, ctx);
        if let Some(eb) = else_ {
            match eb {
                ElseBranch::Block(b) => {
                    check_return_pred_in_block(b, fn_name, ret_pred, var_refs, ctx)
                }
                ElseBranch::If(s) => check_return_pred_in_stmt(s, fn_name, ret_pred, var_refs, ctx),
            }
        }
    }
}

fn check_return_pred_for_expr(
    ret_expr: &Expr,
    ret_span: Span,
    fn_name: &str,
    ret_pred: &RefExpr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    ctx: &mut ContractCheckCtx<'_>,
) {
    let outcome =
        check_arg_against_pred_counted(ret_expr, ret_pred, var_refs, ctx.fn_decls, ctx.counts);
    if let RefResult::Failed { counterexample } = outcome {
        ctx.errors.push(CheckError::RefinementViolated {
            pred: format!(
                "return value of `{fn_name}` violates return refinement `{}`",
                display_pred(ret_pred)
            ),
            span: ret_span,
            counterexample,
        });
    }
}

// ── Lookup table builders ─────────────────────────────────────────────────────

/// Per-function contract information used when checking call sites.
struct FnContracts {
    params: Vec<Param>,
    requires: Vec<Expr>,
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

/// Check all `requires` clauses for a single call site.
fn check_requires_at_call(
    fn_name: &str,
    args: &[Expr],
    call_span: Span,
    contracts: &FnContracts,
    var_refs: &HashMap<String, Option<RefExpr>>,
    ctx: &mut ContractCheckCtx<'_>,
) {
    let params = &contracts.params;

    for req_expr in &contracts.requires {
        // Expressions the solver can't handle (e.g. method calls) fall back to
        // RuntimeCheck — no static verification, but the clause is never silently dropped.
        let Some(req_pred) = expr_to_ref_expr_ext(req_expr, call_span) else {
            continue;
        };
        // Find which single parameter name this predicate references.
        match single_param_ref(&req_pred, params) {
            Some((param_idx, param_name)) if param_idx < args.len() => {
                let normalized = normalize_pred(&req_pred, &param_name);
                let arg = &args[param_idx];
                let outcome = check_arg_against_pred_counted(
                    arg,
                    &normalized,
                    var_refs,
                    ctx.fn_decls,
                    ctx.counts,
                );
                if let RefResult::Failed { counterexample } = outcome {
                    ctx.errors.push(CheckError::PreconditionViolated {
                        fn_name: fn_name.to_string(),
                        pred: display_pred(&req_pred),
                        span: call_span,
                        counterexample,
                    });
                }
                // Proven or RuntimeCheck: silent at compile time.
            }
            _ => {
                // Phase 2: try multi-param substitution when all referenced args are literals.
                check_multi_param_requires_literal(
                    fn_name, &req_pred, params, args, var_refs, call_span, ctx,
                );
            }
        }
    }
}

// ── ensures: return-point checker ────────────────────────────────────────────

fn check_ensures_in_block(
    block: &Block,
    fn_name: &str,
    ensures: &[Expr],
    params: &[Param],
    ctx: &mut ContractCheckCtx<'_>,
) {
    for (i, stmt) in block.stmts.iter().enumerate() {
        match stmt {
            Stmt::Return {
                value: Some(ret_expr),
                span,
            } => {
                check_ensures_for_return(ret_expr, *span, fn_name, ensures, params, ctx);
            }
            Stmt::Return { value: None, .. } => {
                // `return;` returns Unit — nothing to check against ensures.
            }
            Stmt::If { then, else_, .. } => {
                check_ensures_in_block(then, fn_name, ensures, params, ctx);
                if let Some(eb) = else_ {
                    match eb {
                        ElseBranch::Block(b) => {
                            check_ensures_in_block(b, fn_name, ensures, params, ctx)
                        }
                        ElseBranch::If(s) => {
                            check_ensures_in_stmt(s, fn_name, ensures, params, ctx)
                        }
                    }
                }
            }
            Stmt::While { body, .. } => {
                check_ensures_in_block(body, fn_name, ensures, params, ctx);
            }
            Stmt::For { body, .. } => {
                check_ensures_in_block(body, fn_name, ensures, params, ctx);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    check_ensures_in_match_body(&arm.body, fn_name, ensures, params, ctx);
                }
            }
            // Tail expression (implicit return) — last Stmt::Expr in the block.
            Stmt::Expr { expr, span } if i + 1 == block.stmts.len() => {
                check_ensures_for_return(expr, *span, fn_name, ensures, params, ctx);
            }
            _ => {}
        }
    }
}

fn check_ensures_in_stmt(
    stmt: &Stmt,
    fn_name: &str,
    ensures: &[Expr],
    params: &[Param],
    ctx: &mut ContractCheckCtx<'_>,
) {
    // Recursion helper for else-if chains.
    if let Stmt::If { then, else_, .. } = stmt {
        check_ensures_in_block(then, fn_name, ensures, params, ctx);
        if let Some(eb) = else_ {
            match eb {
                ElseBranch::Block(b) => check_ensures_in_block(b, fn_name, ensures, params, ctx),
                ElseBranch::If(s) => check_ensures_in_stmt(s, fn_name, ensures, params, ctx),
            }
        }
    }
}

fn check_ensures_in_match_body(
    body: &MatchBody,
    fn_name: &str,
    ensures: &[Expr],
    params: &[Param],
    ctx: &mut ContractCheckCtx<'_>,
) {
    match body {
        MatchBody::Block(b) => check_ensures_in_block(b, fn_name, ensures, params, ctx),
        MatchBody::Expr(e) => {
            // MatchBody::Expr is a tail expression — treated as a return point.
            let span = e.span();
            check_ensures_for_return(e, span, fn_name, ensures, params, ctx);
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
    ensures: &[Expr],
    params: &[Param],
    ctx: &mut ContractCheckCtx<'_>,
) {
    // Phase 2: populate var_refs with each parameter's inline where-predicate
    // (normalised so the param name becomes "self").  This lets Layer 2 and
    // Layer 4 prove postconditions like `ensures result >= 0` when the function
    // parameter is annotated `n: Int where self >= 0`.
    let var_refs = build_param_var_refs(params);

    for ens_expr in ensures {
        // Expressions the solver can't handle (e.g. method calls) fall back to
        // RuntimeCheck — no static verification, but the clause is never silently dropped.
        let Some(ens_pred) = expr_to_ref_expr_ext(ens_expr, ret_span) else {
            continue;
        };
        // Normalise "result" → "self" so the solver recognises the return value.
        let normalized = normalize_pred(&ens_pred, "result");

        // Let the solver decide: Proven (silent), Failed (emit error),
        // or RuntimeCheck (silent — deferred to runtime).
        let layer_before = ctx.counts.by_layer;
        let outcome = check_arg_against_pred_counted(
            ret_expr,
            &normalized,
            &var_refs,
            ctx.fn_decls,
            ctx.counts,
        );
        if matches!(outcome, RefResult::Proven) {
            let layer = (1..6)
                .find(|&i| ctx.counts.by_layer[i] > layer_before[i])
                .unwrap_or(0);
            ctx.counts.proof_log.push(ProofEntry {
                file: String::new(),
                line: ret_span.line,
                caller: String::new(),
                callee: fn_name.to_string(),
                predicate: format!("ensures {}", display_pred(&ens_pred)),
                layer,
            });
        }
        if let RefResult::Failed { counterexample } = outcome {
            ctx.errors.push(CheckError::PostconditionViolated {
                fn_name: fn_name.to_string(),
                pred: display_pred(&ens_pred),
                span: ret_span,
                counterexample,
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
        // Quantifiers: recurse into body; the bound variable shadows the outer scope, so
        // only normalize free occurrences in the body that differ from the bound var.
        RefExpr::Forall {
            var,
            ty,
            body,
            span,
        } => RefExpr::Forall {
            var: var.clone(),
            ty: ty.clone(),
            body: Box::new(if var == old_name {
                // old_name is bound here — do not rename inside this scope.
                *body.clone()
            } else {
                normalize_pred(body, old_name)
            }),
            span: *span,
        },
        RefExpr::Exists {
            var,
            ty,
            body,
            span,
        } => RefExpr::Exists {
            var: var.clone(),
            ty: ty.clone(),
            body: Box::new(if var == old_name {
                *body.clone()
            } else {
                normalize_pred(body, old_name)
            }),
            span: *span,
        },
        // Field access: recurse into object, keep field unchanged.
        RefExpr::FieldAccess {
            object,
            field,
            span,
        } => RefExpr::FieldAccess {
            object: Box::new(normalize_pred(object, old_name)),
            field: field.clone(),
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
        // Quantifiers: substitute in the body unless old_name is the bound variable.
        RefExpr::Forall {
            var,
            ty,
            body,
            span,
        } => RefExpr::Forall {
            var: var.clone(),
            ty: ty.clone(),
            body: Box::new(if var == old_name {
                *body.clone()
            } else {
                subst_pred_ident(body, old_name, new_val)
            }),
            span: *span,
        },
        RefExpr::Exists {
            var,
            ty,
            body,
            span,
        } => RefExpr::Exists {
            var: var.clone(),
            ty: ty.clone(),
            body: Box::new(if var == old_name {
                *body.clone()
            } else {
                subst_pred_ident(body, old_name, new_val)
            }),
            span: *span,
        },
        // Field access: substitute inside the object expression.
        RefExpr::FieldAccess {
            object,
            field,
            span,
        } => RefExpr::FieldAccess {
            object: Box::new(subst_pred_ident(object, old_name, new_val)),
            field: field.clone(),
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
    call_span: Span,
    ctx: &mut ContractCheckCtx<'_>,
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

    let outcome = check_arg_against_pred_counted(
        &args[*primary_idx],
        &modified_pred,
        var_refs,
        ctx.fn_decls,
        ctx.counts,
    );
    if let RefResult::Failed { counterexample } = outcome {
        ctx.errors.push(CheckError::PreconditionViolated {
            fn_name: fn_name.to_string(),
            pred: display_pred(pred),
            span: call_span,
            counterexample,
        });
    }
}

// ── Phase 3: invariant checker ────────────────────────────────────────────────

/// Walk a block and check every `while` loop's invariants at loop entry.
fn check_invariants_in_block(
    block: &Block,
    fn_name: &str,
    var_refs: &HashMap<String, Option<RefExpr>>,
    ctx: &mut ContractCheckCtx<'_>,
) {
    for stmt in &block.stmts {
        check_invariants_in_stmt(stmt, fn_name, var_refs, ctx);
    }
}

fn check_invariants_in_stmt(
    stmt: &Stmt,
    fn_name: &str,
    var_refs: &HashMap<String, Option<RefExpr>>,
    ctx: &mut ContractCheckCtx<'_>,
) {
    match stmt {
        Stmt::While {
            invariants,
            decreases,
            body,
            span,
            ..
        } => {
            for inv_expr in invariants {
                // Expressions the solver can't handle fall back to RuntimeCheck.
                let Some(inv_pred) = expr_to_ref_expr_ext(inv_expr, *span) else {
                    continue;
                };
                check_invariant_at_entry(fn_name, &inv_pred, var_refs, *span, ctx);
                // Phase 5: also verify the invariant is preserved across iterations.
                check_invariant_preserved(fn_name, &inv_pred, body, var_refs, *span, ctx);
            }
            // Phase 5: verify the decreases measure.
            // Expressions that can't be converted to RefExpr (e.g. method calls) fall back
            // to RuntimeCheck — no static verification, but the loop body is never lost.
            if let Some(dec_expr) = decreases {
                if let Some(dec_ref) = expr_to_ref_expr_ext(dec_expr, *span) {
                    check_decreases_at_entry(fn_name, &dec_ref, var_refs, *span, ctx);
                    check_decreases_across_iteration(fn_name, &dec_ref, body, var_refs, *span, ctx);
                }
            }
            // Recurse into the body for nested loops.
            check_invariants_in_block(body, fn_name, var_refs, ctx);
        }
        Stmt::If { then, else_, .. } => {
            check_invariants_in_block(then, fn_name, var_refs, ctx);
            if let Some(eb) = else_ {
                match eb {
                    ElseBranch::Block(b) => check_invariants_in_block(b, fn_name, var_refs, ctx),
                    ElseBranch::If(s) => check_invariants_in_stmt(s, fn_name, var_refs, ctx),
                }
            }
        }
        Stmt::For {
            invariants,
            body,
            span,
            ..
        } => {
            for inv_expr in invariants {
                // Expressions the solver can't handle fall back to RuntimeCheck.
                let Some(inv_pred) = expr_to_ref_expr_ext(inv_expr, *span) else {
                    continue;
                };
                check_invariant_at_entry(fn_name, &inv_pred, var_refs, *span, ctx);
                // Phase 5: also verify the invariant is preserved across iterations.
                check_invariant_preserved(fn_name, &inv_pred, body, var_refs, *span, ctx);
            }
            // Recurse into the body for nested loops.
            check_invariants_in_block(body, fn_name, var_refs, ctx);
        }
        Stmt::Match { arms, .. } => {
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => check_invariants_in_block(b, fn_name, var_refs, ctx),
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
    loop_span: Span,
    ctx: &mut ContractCheckCtx<'_>,
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
            let outcome = check_arg_against_pred_counted(
                &dummy,
                inv_pred,
                var_refs,
                ctx.fn_decls,
                ctx.counts,
            );
            if let RefResult::Failed { counterexample } = outcome {
                ctx.errors.push(CheckError::InvariantViolated {
                    fn_name: fn_name.to_string(),
                    pred: display_pred(inv_pred),
                    span: loop_span,
                    counterexample,
                });
            }
        }
        [var_name] => {
            // Single free variable — normalise it to "self" and check via Ident lookup.
            let normalized = normalize_pred(inv_pred, var_name);
            let ident_expr = Expr::Ident(var_name.clone(), loop_span);
            let outcome = check_arg_against_pred_counted(
                &ident_expr,
                &normalized,
                var_refs,
                ctx.fn_decls,
                ctx.counts,
            );
            if let RefResult::Failed { counterexample } = outcome {
                ctx.errors.push(CheckError::InvariantViolated {
                    fn_name: fn_name.to_string(),
                    pred: display_pred(inv_pred),
                    span: loop_span,
                    counterexample,
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
        // Quantifiers: collect free identifiers from the body (bound var is treated as free
        // here since the caller uses this to count variables, and the bound var is in scope).
        RefExpr::Forall { body, .. } | RefExpr::Exists { body, .. } => {
            collect_idents_inner(body, names);
        }
        RefExpr::Len { ident, .. } => names.push(ident.clone()),
        RefExpr::Integer { .. } | RefExpr::Float { .. } => {}
        // Field access: collect idents from the object (e.g. `self` in `self.size`).
        RefExpr::FieldAccess { object, .. } => collect_idents_inner(object, names),
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
        RefExpr::Forall { var, body, .. } => format!("forall {var}, {}", display_pred(body)),
        RefExpr::Exists { var, body, .. } => format!("exists {var}, {}", display_pred(body)),
        RefExpr::FieldAccess { object, field, .. } => {
            format!("{}.{}", display_pred(object), field)
        }
    }
}

// ── Phase 5: loop body analysis helpers ───────────────────────────────────────

/// Extract simple variable assignments from a loop body.
///
/// Only handles top-level `x = expr` assignments where the target is a plain
/// identifier.  Returns `None` if the body contains any control-flow statement
/// (`if`, `while`, `for`, `match`) — indicating the effect map cannot be
/// determined statically and callers should fall back to `RuntimeCheck`.
fn extract_simple_assignments(body: &Block) -> Option<HashMap<String, Expr>> {
    let mut effects = HashMap::new();
    for stmt in &body.stmts {
        match stmt {
            Stmt::Assign {
                target: LValue::Ident(name, _),
                value,
                ..
            } => {
                effects.insert(name.clone(), value.clone());
            }
            // Ghost bindings and bare expression statements have no effect on
            // the variables tracked in invariants / decreases measures.
            Stmt::Let {
                kind: LetKind::Ghost,
                ..
            }
            | Stmt::Expr { .. } => {}
            // Any control flow makes static analysis too complex → RuntimeCheck.
            _ => return None,
        }
    }
    Some(effects)
}

/// Substitute all free identifiers in `pred` with their post-iteration values
/// from `effects` (variable name → new `RefExpr` value).
fn apply_effects_to_pred(pred: &RefExpr, effects: &HashMap<String, RefExpr>) -> RefExpr {
    match pred {
        RefExpr::Ident { name, .. } => {
            if let Some(new_val) = effects.get(name) {
                new_val.clone()
            } else {
                pred.clone()
            }
        }
        RefExpr::Compare {
            op,
            left,
            right,
            span,
        } => RefExpr::Compare {
            op: *op,
            left: Box::new(apply_effects_to_pred(left, effects)),
            right: Box::new(apply_effects_to_pred(right, effects)),
            span: *span,
        },
        RefExpr::LogicOp {
            op,
            left,
            right,
            span,
        } => RefExpr::LogicOp {
            op: *op,
            left: Box::new(apply_effects_to_pred(left, effects)),
            right: Box::new(apply_effects_to_pred(right, effects)),
            span: *span,
        },
        RefExpr::ArithOp {
            op,
            left,
            right,
            span,
        } => RefExpr::ArithOp {
            op: *op,
            left: Box::new(apply_effects_to_pred(left, effects)),
            right: Box::new(apply_effects_to_pred(right, effects)),
            span: *span,
        },
        RefExpr::Not { inner, span } => RefExpr::Not {
            inner: Box::new(apply_effects_to_pred(inner, effects)),
            span: *span,
        },
        RefExpr::Grouped { inner, span } => RefExpr::Grouped {
            inner: Box::new(apply_effects_to_pred(inner, effects)),
            span: *span,
        },
        // Old/Len/Integer/Float/Forall/Exists: leave unchanged.
        _ => pred.clone(),
    }
}

/// Build `var_refs` augmented with the invariant predicate as an induction
/// hypothesis for every variable the invariant mentions.
///
/// This lets the solver assume `inv_pred` holds at the start of the iteration
/// when checking whether it holds at the end (preservation proof).
fn augment_var_refs_with_invariant(
    var_refs: &HashMap<String, Option<RefExpr>>,
    inv_pred: &RefExpr,
) -> HashMap<String, Option<RefExpr>> {
    let mut augmented = var_refs.clone();
    let vars_in_inv = collect_ident_names(inv_pred);
    let mut distinct: Vec<String> = Vec::new();
    for v in &vars_in_inv {
        if !distinct.contains(v) {
            distinct.push(v.clone());
        }
    }
    for var in distinct {
        // Add invariant-derived hypothesis only if this variable has no finer
        // constraint already known to the solver.
        augmented.entry(var.clone()).or_insert_with(|| {
            // Normalise the invariant so the variable maps to "self".
            Some(normalize_pred(inv_pred, &var))
        });
    }
    augmented
}

/// Check a standalone `RefExpr` predicate (not parameterised by a "self" arg)
/// against the current hypotheses.
///
/// Uses a dummy integer-0 argument; the solver resolves all free identifiers
/// via `var_refs`.  Returns the solver outcome.
fn check_standalone_pred(
    pred: &RefExpr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    loop_span: Span,
    ctx: &mut ContractCheckCtx<'_>,
) -> RefResult {
    let idents = collect_ident_names(pred);
    let mut distinct: Vec<String> = Vec::new();
    for id in &idents {
        if !distinct.contains(id) {
            distinct.push(id.clone());
        }
    }

    match distinct.as_slice() {
        [] => {
            // Constant predicate — pass dummy literal.
            let dummy = Expr::Literal(Literal::Integer(0), loop_span);
            check_arg_against_pred_counted(&dummy, pred, var_refs, ctx.fn_decls, ctx.counts)
        }
        [var_name] => {
            // Single free variable — normalise to "self".
            let normalized = normalize_pred(pred, var_name);
            let ident_expr = Expr::Ident(var_name.clone(), loop_span);
            check_arg_against_pred_counted(
                &ident_expr,
                &normalized,
                var_refs,
                ctx.fn_decls,
                ctx.counts,
            )
        }
        _ => {
            // Multiple variables: pass dummy; the solver (Z3 / Cooper) will
            // resolve all identifiers from `var_refs`.
            let dummy = Expr::Literal(Literal::Integer(0), loop_span);
            check_arg_against_pred_counted(&dummy, pred, var_refs, ctx.fn_decls, ctx.counts)
        }
    }
}

// ── Phase 5: decreases checks ─────────────────────────────────────────────────

/// Check that the `decreases` measure is bounded below (≥ 0) at loop entry.
fn check_decreases_at_entry(
    fn_name: &str,
    decreases_expr: &RefExpr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    loop_span: Span,
    ctx: &mut ContractCheckCtx<'_>,
) {
    // Prove the *negation*: if `decreases_expr < 0` is Proven, the measure is
    // definitely not bounded below → emit error.
    // (Direct `Failed` is not reliably produced for variable predicates.)
    let lt_zero = RefExpr::Compare {
        op: CmpOp::Lt,
        left: Box::new(decreases_expr.clone()),
        right: Box::new(RefExpr::Integer {
            value: 0,
            span: loop_span,
        }),
        span: loop_span,
    };
    let outcome = check_standalone_pred(&lt_zero, var_refs, loop_span, ctx);
    if outcome == RefResult::Proven {
        ctx.errors.push(CheckError::DecreasesNotBounded {
            fn_name: fn_name.to_string(),
            measure: display_pred(decreases_expr),
            span: loop_span,
        });
    }
    // Proven of the positive check or RuntimeCheck: silent at compile time.
}

/// Check that the `decreases` measure strictly decreases across one iteration.
///
/// Strategy: extract the simple assignment effect map from the body, apply it
/// to the decreases expression to get `post_decreases`, then prove
/// `post_decreases < pre_decreases` under the current hypotheses.
/// Falls back to `RuntimeCheck` (silent) if the body is too complex to analyse.
fn check_decreases_across_iteration(
    fn_name: &str,
    decreases_expr: &RefExpr,
    body: &Block,
    var_refs: &HashMap<String, Option<RefExpr>>,
    loop_span: Span,
    ctx: &mut ContractCheckCtx<'_>,
) {
    let Some(effects_exprs) = extract_simple_assignments(body) else {
        return; // Too complex — RuntimeCheck (no error at compile time).
    };

    // Convert Expr effects to RefExpr effects.
    let mut effects_ref: HashMap<String, RefExpr> = HashMap::new();
    for (var, expr) in &effects_exprs {
        match expr_to_ref_expr_ext(expr, loop_span) {
            Some(ref_e) => {
                effects_ref.insert(var.clone(), ref_e);
            }
            None => return, // Can't convert — RuntimeCheck.
        }
    }

    // post_decreases = decreases_expr with effect map applied.
    let post_decreases = apply_effects_to_pred(decreases_expr, &effects_ref);

    // Prove the *negation*: if `post_decreases >= pre_decreases` is Proven, the
    // measure is definitely not decreasing → emit error.
    // This is equivalent to: ¬(post < pre) = (post >= pre).
    let not_decreasing = RefExpr::Compare {
        op: CmpOp::Ge,
        left: Box::new(post_decreases),
        right: Box::new(decreases_expr.clone()),
        span: loop_span,
    };

    let outcome = check_standalone_pred(&not_decreasing, var_refs, loop_span, ctx);
    if outcome == RefResult::Proven {
        ctx.errors.push(CheckError::DecreasesNotDecreasing {
            fn_name: fn_name.to_string(),
            measure: display_pred(decreases_expr),
            span: loop_span,
        });
    }
    // RuntimeCheck (can't determine) or Failed (positive case, measure IS decreasing): silent.
}

// ── Phase 5: invariant preservation ───────────────────────────────────────────

/// Check that a loop invariant is preserved across one iteration.
///
/// Strategy:
/// 1. Extract the simple assignment effect map from the body.
/// 2. Apply the effect map to the invariant predicate to get `post_inv`.
/// 3. Augment `var_refs` with the invariant itself as an induction hypothesis.
/// 4. Check `post_inv` holds under the augmented hypotheses.
///
/// Falls back to `RuntimeCheck` (silent) if the body cannot be statically analysed.
fn check_invariant_preserved(
    fn_name: &str,
    inv_pred: &RefExpr,
    body: &Block,
    var_refs: &HashMap<String, Option<RefExpr>>,
    loop_span: Span,
    ctx: &mut ContractCheckCtx<'_>,
) {
    let Some(effects_exprs) = extract_simple_assignments(body) else {
        return; // Too complex — RuntimeCheck.
    };

    let mut effects_ref: HashMap<String, RefExpr> = HashMap::new();
    for (var, expr) in &effects_exprs {
        match expr_to_ref_expr_ext(expr, loop_span) {
            Some(ref_e) => {
                effects_ref.insert(var.clone(), ref_e);
            }
            None => return, // RuntimeCheck.
        }
    }

    // post_inv = invariant with post-iteration variable values.
    let post_inv = apply_effects_to_pred(inv_pred, &effects_ref);

    // Add invariant as an induction hypothesis for the variables it mentions.
    let augmented = augment_var_refs_with_invariant(var_refs, inv_pred);

    // Prove the *negation*: if `NOT post_inv` is Proven, the invariant is
    // definitely violated after one iteration → emit error.
    let negated_post = RefExpr::Not {
        inner: Box::new(post_inv),
        span: loop_span,
    };
    let outcome = check_standalone_pred(&negated_post, &augmented, loop_span, ctx);
    if outcome == RefResult::Proven {
        ctx.errors.push(CheckError::InvariantNotPreserved {
            fn_name: fn_name.to_string(),
            pred: display_pred(inv_pred),
            span: loop_span,
        });
    }
    // RuntimeCheck or Failed (positive case — invariant IS preserved): silent.
}

// ── D2: Actor protocol bounded model checker (Phase 8, #37) ──────────────────

/// Check that `requires` clauses and loop invariants are satisfied within all
/// behavior and helper-method bodies of an actor declaration.
///
/// Actor behaviors are async message handlers, but their bodies contain the same
/// kinds of function calls and loop constructs as regular functions — the same
/// contract rules therefore apply.
fn check_actor_behavior_contracts(
    ad: &ActorDecl,
    fn_map: &HashMap<String, FnContracts>,
    ctx: &mut ContractCheckCtx<'_>,
) {
    for method in &ad.methods {
        let var_refs = build_param_var_refs(&method.params);
        walk_stmts(&method.body, ctx, &mut |expr, ctx| {
            if let Expr::FnCall {
                name, args, span, ..
            } = expr
            {
                if let Some(contracts) = fn_map.get(name) {
                    check_requires_at_call(name, args, *span, contracts, &var_refs, ctx);
                }
            }
        });
        // Note: `ensures` checking is omitted because `ActorMethod` has no `ensures`
        // field today. If ensures support is added to actor methods, wire it in here.
        check_invariants_in_block(&method.body, &method.name, &var_refs, ctx);
    }
}

/// Check that every `actor ActorType { field: value, … }` expression provides
/// initial values that satisfy the declared field refinements.
///
/// Uses the same 5-layer solver as refinement types.  A field without a
/// refinement is always accepted.
pub fn check_actor_field_refinements(
    prog: &Program,
    errors: &mut Vec<CheckError>,
    mode: SolverMode,
) {
    // Build a map: actor_name → field declarations (only those with refinements).
    let actor_fields = build_actor_field_map(prog);
    if actor_fields.is_empty() {
        return; // Fast path: no actor has refined fields.
    }
    let fn_decls = build_fn_decls_for_solver(prog);
    let mut counts = RefinementCounts {
        mode,
        ..Default::default()
    };
    let mut ctx = ContractCheckCtx {
        fn_decls: &fn_decls,
        errors,
        counts: &mut counts,
    };

    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                // Seed var_refs from function parameters so the solver can use
                // where-refinements on parameter variables as hypotheses.
                let var_refs = build_param_var_refs(&fd.params);
                walk_stmts(&fd.body, &mut ctx, &mut |expr, ctx| {
                    check_spawn_at_site(expr, &actor_fields, &var_refs, ctx);
                });
            }
            Decl::Impl(impl_d) => {
                for method in &impl_d.methods {
                    let var_refs = build_param_var_refs(&method.params);
                    walk_stmts(&method.body, &mut ctx, &mut |expr, ctx| {
                        check_spawn_at_site(expr, &actor_fields, &var_refs, ctx);
                    });
                }
            }
            Decl::Actor(ad) => {
                for method in &ad.methods {
                    let var_refs = build_param_var_refs(&method.params);
                    walk_stmts(&method.body, &mut ctx, &mut |expr, ctx| {
                        check_spawn_at_site(expr, &actor_fields, &var_refs, ctx);
                    });
                }
            }
            _ => {}
        }
    }
}

/// Build a map from actor type name → its field declarations.
fn build_actor_field_map(prog: &Program) -> HashMap<String, Vec<FieldDecl>> {
    let mut map = HashMap::new();
    for decl in &prog.declarations {
        if let Decl::Actor(ad) = decl {
            let refined: Vec<FieldDecl> = ad
                .fields
                .iter()
                .filter(|f| f.refinement.is_some())
                .cloned()
                .collect();
            if !refined.is_empty() {
                map.insert(ad.name.clone(), refined);
            }
        }
    }
    map
}

/// Leaf checker for `Expr::Spawn` — called by `walk_stmts` for every expression.
/// Checks that actor field-init values satisfy their declared refinements.
fn check_spawn_at_site(
    expr: &Expr,
    actor_fields: &HashMap<String, Vec<FieldDecl>>,
    var_refs: &HashMap<String, Option<RefExpr>>,
    ctx: &mut ContractCheckCtx<'_>,
) {
    let Expr::Spawn {
        actor_type,
        fields,
        span,
    } = expr
    else {
        return;
    };
    let Some(refined_fields) = actor_fields.get(actor_type) else {
        return;
    };
    for (init_name, init_expr) in fields {
        if let Some(field_decl) = refined_fields.iter().find(|f| &f.name == init_name) {
            if let Some(pred) = &field_decl.refinement {
                let outcome = check_arg_against_pred_counted(
                    init_expr,
                    pred,
                    var_refs,
                    ctx.fn_decls,
                    ctx.counts,
                );
                if let RefResult::Failed { counterexample } = outcome {
                    ctx.errors.push(CheckError::RefinementViolated {
                        pred: format!("{actor_type}.{init_name}: {}", display_pred(pred)),
                        counterexample,
                        span: *span,
                    });
                }
            }
        }
    }
}

// ── Struct / enum-variant field refinements (#1067 Gap 1 + Gap 6) ────────────

/// Check that every `TypeName { field: value, … }` struct / enum-variant
/// construction expression satisfies the declared field `where` refinements
/// and the optional `with invariant` clause (Gap 2).
///
/// Mirrors `check_actor_field_refinements` for `Expr::Spawn`, but targets
/// `Expr::Construct` and ordinary `type` declarations.
pub fn check_struct_field_refinements(
    prog: &Program,
    errors: &mut Vec<CheckError>,
    mode: SolverMode,
) {
    let field_map = build_struct_field_map(prog);
    if field_map.is_empty() {
        return;
    }
    let fn_decls = build_fn_decls_for_solver(prog);
    let mut counts = RefinementCounts {
        mode,
        ..Default::default()
    };
    let mut ctx = ContractCheckCtx {
        fn_decls: &fn_decls,
        errors,
        counts: &mut counts,
    };

    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                let var_refs = build_param_var_refs(&fd.params);
                walk_stmts(&fd.body, &mut ctx, &mut |expr, ctx| {
                    check_construct_at_site(expr, &field_map, &var_refs, ctx);
                });
            }
            Decl::Impl(impl_d) => {
                for method in &impl_d.methods {
                    let var_refs = build_param_var_refs(&method.params);
                    walk_stmts(&method.body, &mut ctx, &mut |expr, ctx| {
                        check_construct_at_site(expr, &field_map, &var_refs, ctx);
                    });
                }
            }
            Decl::Actor(ad) => {
                for method in &ad.methods {
                    let var_refs = build_param_var_refs(&method.params);
                    walk_stmts(&method.body, &mut ctx, &mut |expr, ctx| {
                        check_construct_at_site(expr, &field_map, &var_refs, ctx);
                    });
                }
            }
            _ => {}
        }
    }
}

/// Build a map: type_name → (refined_field_decls, optional_struct_invariant).
///
/// Covers:
/// - `type Foo = struct { f: T where pred, … } with invariant …`  → key `"Foo"`
/// - `type Bar = enum { Variant { f: T where pred, … }, … }`      → key `"Bar::Variant"`
fn build_struct_field_map(prog: &Program) -> HashMap<String, (Vec<FieldDecl>, Option<RefExpr>)> {
    let mut map = HashMap::new();
    for decl in &prog.declarations {
        if let Decl::Type(td) = decl {
            match &td.body {
                TypeBody::Struct { fields, invariant } => {
                    let refined: Vec<FieldDecl> = fields
                        .iter()
                        .filter(|f| f.refinement.is_some())
                        .cloned()
                        .collect();
                    if !refined.is_empty() || invariant.is_some() {
                        map.insert(td.name.clone(), (refined, invariant.clone()));
                    }
                }
                TypeBody::Enum(variants) => {
                    for v in variants {
                        if let VariantFields::Struct(vfields) = &v.fields {
                            let refined: Vec<FieldDecl> = vfields
                                .iter()
                                .filter(|f| f.refinement.is_some())
                                .cloned()
                                .collect();
                            if !refined.is_empty() {
                                let key = format!("{}::{}", td.name, v.name);
                                map.insert(key, (refined, None));
                            }
                        }
                    }
                }
                TypeBody::Alias(_) => {}
            }
        }
    }
    map
}

/// Leaf checker for `Expr::Construct` — called by `walk_stmts` for every expression.
/// Checks that struct / enum-variant field-init values satisfy declared refinements
/// and the optional `with invariant` clause.
fn check_construct_at_site(
    expr: &Expr,
    field_map: &HashMap<String, (Vec<FieldDecl>, Option<RefExpr>)>,
    var_refs: &HashMap<String, Option<RefExpr>>,
    ctx: &mut ContractCheckCtx<'_>,
) {
    let Expr::Construct { name, fields, span } = expr else {
        return;
    };
    let Some((refined_fields, invariant)) = field_map.get(name) else {
        return;
    };
    // ── Per-field refinement checks ─────────────────────────────────────────
    for (init_name, init_expr) in fields {
        if let Some(field_decl) = refined_fields.iter().find(|f| &f.name == init_name) {
            if let Some(pred) = &field_decl.refinement {
                let outcome = check_arg_against_pred_counted(
                    init_expr,
                    pred,
                    var_refs,
                    ctx.fn_decls,
                    ctx.counts,
                );
                if let RefResult::Failed { counterexample } = outcome {
                    ctx.errors.push(CheckError::RefinementViolated {
                        pred: format!("{name}.{init_name}: {}", display_pred(pred)),
                        counterexample,
                        span: *span,
                    });
                }
            }
        }
    }
    // ── Struct invariant check (Gap 2) ──────────────────────────────────────
    if let Some(inv) = invariant {
        if let Some(false) = eval_invariant_with_const_fields(inv, fields) {
            ctx.errors.push(CheckError::RefinementViolated {
                pred: format!("{name}: with invariant {}", display_pred(inv)),
                counterexample: None,
                span: *span,
            });
        }
    }
}

// ── Struct invariant evaluation with constant field values (#1067 Gap 2) ─────

/// Try to evaluate a struct `with invariant` predicate by substituting
/// integer-literal field values from the construction site.
///
/// Returns:
/// - `Some(false)` — invariant statically violated (emit compile error)
/// - `Some(true)`  — invariant statically satisfied
/// - `None`        — cannot determine statically (defer to runtime check)
fn eval_invariant_with_const_fields(inv: &RefExpr, fields: &[(String, Expr)]) -> Option<bool> {
    // Build field_name → i64 literal map from constant init expressions.
    let mut field_consts: HashMap<String, i64> = HashMap::new();
    for (name, expr) in fields {
        if let Some(n) = expr_to_i64_literal(expr) {
            field_consts.insert(name.clone(), n);
        }
    }
    eval_ref_expr_bool(inv, &field_consts)
}

/// Extract an i64 literal from a simple expression (literal or negated literal).
fn expr_to_i64_literal(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Literal(Literal::Integer(n), _) => Some(*n),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
            ..
        } => {
            if let Expr::Literal(Literal::Integer(n), _) = inner.as_ref() {
                n.checked_neg()
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Recursively evaluate a `RefExpr` to an integer, substituting
/// `self.field` and bare `field` idents from `field_consts`.
fn eval_ref_expr_int(pred: &RefExpr, field_consts: &HashMap<String, i64>) -> Option<i64> {
    match pred {
        RefExpr::Integer { value, .. } => Some(*value),
        // `self.field_name` — look up the field in our const map.
        RefExpr::FieldAccess { object, field, .. } => {
            if let RefExpr::Ident { name, .. } = object.as_ref() {
                if name == "self" {
                    return field_consts.get(field).copied();
                }
            }
            None
        }
        // Bare identifier that matches a field name.
        RefExpr::Ident { name, .. } => field_consts.get(name).copied(),
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let l = eval_ref_expr_int(left, field_consts)?;
            let r = eval_ref_expr_int(right, field_consts)?;
            match op {
                ArithOp::Add => l.checked_add(r),
                ArithOp::Sub => l.checked_sub(r),
                ArithOp::Mul => l.checked_mul(r),
                ArithOp::Div => {
                    if r != 0 {
                        l.checked_div(r)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        RefExpr::Grouped { inner, .. } => eval_ref_expr_int(inner, field_consts),
        _ => None,
    }
}

/// Recursively evaluate a `RefExpr` to a boolean using `field_consts`.
fn eval_ref_expr_bool(pred: &RefExpr, field_consts: &HashMap<String, i64>) -> Option<bool> {
    match pred {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let l = eval_ref_expr_int(left, field_consts)?;
            let r = eval_ref_expr_int(right, field_consts)?;
            Some(match op {
                CmpOp::Eq => l == r,
                CmpOp::Ne => l != r,
                CmpOp::Lt => l < r,
                CmpOp::Le => l <= r,
                CmpOp::Gt => l > r,
                CmpOp::Ge => l >= r,
            })
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => {
            let l = eval_ref_expr_bool(left, field_consts)?;
            let r = eval_ref_expr_bool(right, field_consts)?;
            Some(match op {
                LogicOp::And => l && r,
                LogicOp::Or => l || r,
            })
        }
        RefExpr::Not { inner, .. } => eval_ref_expr_bool(inner, field_consts).map(|b| !b),
        RefExpr::Grouped { inner, .. } => eval_ref_expr_bool(inner, field_consts),
        _ => None,
    }
}
