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
    check_arg_against_pred_counted, ProofEntry, ProofOutcome, ProofSite, RefinementCounts,
};
use crate::mvl::checker::solver::{RefResult, SolverMode};
use crate::mvl::parser::ast::{
    expr_to_ref_expr_ext, ArithOp, Block, CmpOp, Decl, ElseBranch, Expr, FnDecl, Literal, LogicOp,
    MatchBody, Param, Program, RefExpr, Stmt,
};
use crate::mvl::parser::lexer::Span;
use crate::mvl::parser::visit::{walk_expr as ast_walk_expr, Visit};

// ── Shared checker context ────────────────────────────────────────────────────

/// Context passed to all internal contract-checking functions.
///
/// Bundles the three parameters that are shared across every checker call:
/// the pure-function map for the solver, the error accumulator, and the
/// proof-layer counts (replaces the old `thread_local! CONTRACT_COUNTS`).
/// Solver mode is embedded in `counts.mode`.
pub(super) struct ContractCheckCtx<'a> {
    pub(super) fn_decls: &'a HashMap<String, FnDecl>,
    pub(super) errors: &'a mut Vec<CheckError>,
    pub(super) counts: &'a mut RefinementCounts,
}

// ── Generic AST walker (post-order, context-threaded) ────────────────────────

/// Visit every `Expr` in `block`, calling `f` on each one in **post-order**:
/// sub-expressions are visited before the expression that contains them, so
/// inner call sites are checked before outer ones.
///
/// Built on top of [`crate::mvl::parser::visit::Visit`] — recursion is handled
/// by the shared `walk_expr`, this helper only re-orders the visitor callback
/// to fire after descent.
pub(super) fn walk_stmts<F>(block: &Block, ctx: &mut ContractCheckCtx<'_>, f: &mut F)
where
    F: FnMut(&Expr, &mut ContractCheckCtx<'_>),
{
    struct PostOrderVisitor<'v, 'cx, 'f, F> {
        ctx: &'v mut ContractCheckCtx<'cx>,
        f: &'f mut F,
    }
    impl<'ast, 'v, 'cx, 'f, F> Visit<'ast> for PostOrderVisitor<'v, 'cx, 'f, F>
    where
        F: FnMut(&Expr, &mut ContractCheckCtx<'cx>),
    {
        fn visit_expr(&mut self, e: &'ast Expr) {
            ast_walk_expr(self, e);
            (self.f)(e, self.ctx);
        }
    }
    let mut v = PostOrderVisitor { ctx, f };
    v.visit_block(block);
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
                    ctx.counts.current_fn = fd.name.clone();
                    let sites_before = ctx.counts.sites.len();
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
                    // Update fn_total/fully_verified_fns for Req 10 (#1498).
                    if ctx.counts.sites.len() > sites_before {
                        ctx.counts.fn_total += 1;
                        if ctx.counts.sites[sites_before..]
                            .iter()
                            .all(|s| matches!(s.outcome, ProofOutcome::Proven { .. }))
                        {
                            ctx.counts.fully_verified_fns += 1;
                        }
                    }
                }
                Decl::Impl(impl_d) => {
                    for method in &impl_d.methods {
                        ctx.counts.current_fn = method.name.clone();
                        let sites_before = ctx.counts.sites.len();
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
                        // Update fn_total/fully_verified_fns for Req 10 (#1498).
                        if ctx.counts.sites.len() > sites_before {
                            ctx.counts.fn_total += 1;
                            if ctx.counts.sites[sites_before..]
                                .iter()
                                .all(|s| matches!(s.outcome, ProofOutcome::Proven { .. }))
                            {
                                ctx.counts.fully_verified_fns += 1;
                            }
                        }
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
                ctx.counts.current_fn = fd.name.clone();
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
    let layer_before = ctx.counts.by_layer;
    let outcome =
        check_arg_against_pred_counted(ret_expr, ret_pred, var_refs, ctx.fn_decls, ctx.counts);
    let layer = (1..6)
        .find(|&i| ctx.counts.by_layer[i] > layer_before[i])
        .unwrap_or(0);
    let proof_outcome = match &outcome {
        RefResult::Proven => ProofOutcome::Proven { layer },
        RefResult::RuntimeCheck => ProofOutcome::RuntimeCheck,
        RefResult::Failed { counterexample } => {
            ctx.errors.push(CheckError::RefinementViolated {
                pred: format!(
                    "return value of `{fn_name}` violates return refinement `{}`",
                    display_pred(ret_pred)
                ),
                span: ret_span,
                counterexample: counterexample.clone(),
            });
            ProofOutcome::Failed
        }
    };
    // #836: Record return-refinement proof in sites for `mvl prove`.
    ctx.counts.sites.push(ProofSite {
        caller_fn: ctx.counts.current_fn.clone(),
        fn_name: fn_name.to_string(),
        param_name: "result".to_string(),
        predicate: format!("return {}", display_pred(ret_pred)),
        span: ret_span,
        outcome: proof_outcome,
    });
}

// ── Lookup table builders ─────────────────────────────────────────────────────

/// Per-function contract information used when checking call sites.
pub(super) struct FnContracts {
    params: Vec<Param>,
    requires: Vec<Expr>,
}

pub(super) fn build_fn_contract_map(prog: &Program) -> HashMap<String, FnContracts> {
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
pub(super) fn build_fn_decls_for_solver(prog: &Program) -> HashMap<String, FnDecl> {
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
pub(super) fn check_requires_at_call(
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
                let layer_before = ctx.counts.by_layer;
                let outcome = check_arg_against_pred_counted(
                    arg,
                    &normalized,
                    var_refs,
                    ctx.fn_decls,
                    ctx.counts,
                );
                // Compute layer for #836 sites entry.
                let layer = (1..6)
                    .find(|&i| ctx.counts.by_layer[i] > layer_before[i])
                    .unwrap_or(0);
                let proof_outcome = match &outcome {
                    RefResult::Proven => ProofOutcome::Proven { layer },
                    RefResult::RuntimeCheck => ProofOutcome::RuntimeCheck,
                    RefResult::Failed { counterexample } => {
                        ctx.errors.push(CheckError::PreconditionViolated {
                            fn_name: fn_name.to_string(),
                            pred: display_pred(&req_pred),
                            span: call_span,
                            counterexample: counterexample.clone(),
                        });
                        ProofOutcome::Failed
                    }
                };
                // #836: Record requires-contract proof in sites for `mvl prove`.
                ctx.counts.sites.push(ProofSite {
                    caller_fn: ctx.counts.current_fn.clone(),
                    fn_name: fn_name.to_string(),
                    param_name: param_name.clone(),
                    predicate: format!("requires {}", display_pred(&req_pred)),
                    span: call_span,
                    outcome: proof_outcome,
                });
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
pub(super) fn check_ensures_for_return(
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
        // Compute layer for both proof_log (existing) and sites (#836).
        let layer = (1..6)
            .find(|&i| ctx.counts.by_layer[i] > layer_before[i])
            .unwrap_or(0);
        let proof_outcome = match &outcome {
            RefResult::Proven => {
                ctx.counts.proof_log.push(ProofEntry {
                    file: String::new(),
                    line: ret_span.line,
                    caller: String::new(),
                    callee: fn_name.to_string(),
                    predicate: format!("ensures {}", display_pred(&ens_pred)),
                    layer,
                });
                ProofOutcome::Proven { layer }
            }
            RefResult::RuntimeCheck => ProofOutcome::RuntimeCheck,
            RefResult::Failed { counterexample } => {
                ctx.errors.push(CheckError::PostconditionViolated {
                    fn_name: fn_name.to_string(),
                    pred: display_pred(&ens_pred),
                    span: ret_span,
                    counterexample: counterexample.clone(),
                });
                ProofOutcome::Failed
            }
        };
        // #836: Record contract proofs in sites for `mvl prove` breakdown.
        ctx.counts.sites.push(ProofSite {
            caller_fn: ctx.counts.current_fn.clone(),
            fn_name: fn_name.to_string(),
            param_name: "result".to_string(),
            predicate: format!("ensures {}", display_pred(&ens_pred)),
            span: ret_span,
            outcome: proof_outcome,
        });
    }
}

// ── Predicate helpers ─────────────────────────────────────────────────────────

/// Find which single parameter the predicate references.
/// Returns `Some((param_index, param_name))` if exactly one distinct param is referenced,
/// `None` if zero or multiple different params are referenced.
pub(super) fn single_param_ref(pred: &RefExpr, params: &[Param]) -> Option<(usize, String)> {
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
pub(super) fn normalize_pred(pred: &RefExpr, old_name: &str) -> RefExpr {
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
        RefExpr::Integer { .. }
        | RefExpr::Float { .. }
        | RefExpr::Bool { .. }
        | RefExpr::Len { .. } => pred.clone(),
    }
}

// ── Phase 2 helpers ───────────────────────────────────────────────────────────

/// Build a `var_refs` map from a function's parameter inline `where`-refinements.
///
/// Each predicate is normalised so the parameter name becomes `"self"`,
/// matching the form expected by the 5-layer solver.
pub(super) fn build_param_var_refs(params: &[Param]) -> HashMap<String, Option<RefExpr>> {
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
pub(super) fn subst_pred_ident(pred: &RefExpr, old_name: &str, new_val: &RefExpr) -> RefExpr {
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
        RefExpr::Integer { .. }
        | RefExpr::Float { .. }
        | RefExpr::Bool { .. }
        | RefExpr::Len { .. } => pred.clone(),
    }
}

/// Convert a simple `Expr` to a `RefExpr` literal for predicate substitution.
///
/// Only integer and float literals are converted; returns `None` for anything
/// more complex, causing the multi-param check to fall back to `RuntimeCheck`.
pub(super) fn expr_to_ref_expr(expr: &Expr) -> Option<RefExpr> {
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
pub(super) fn check_multi_param_requires_literal(
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

    let layer_before = ctx.counts.by_layer;
    let outcome = check_arg_against_pred_counted(
        &args[*primary_idx],
        &modified_pred,
        var_refs,
        ctx.fn_decls,
        ctx.counts,
    );
    let layer = (1..6)
        .find(|&i| ctx.counts.by_layer[i] > layer_before[i])
        .unwrap_or(0);
    let proof_outcome = match &outcome {
        RefResult::Proven => ProofOutcome::Proven { layer },
        RefResult::RuntimeCheck => ProofOutcome::RuntimeCheck,
        RefResult::Failed { counterexample } => {
            ctx.errors.push(CheckError::PreconditionViolated {
                fn_name: fn_name.to_string(),
                pred: display_pred(pred),
                span: call_span,
                counterexample: counterexample.clone(),
            });
            ProofOutcome::Failed
        }
    };
    // #836: Record multi-param requires-contract proof in sites.
    ctx.counts.sites.push(ProofSite {
        caller_fn: ctx.counts.current_fn.clone(),
        fn_name: fn_name.to_string(),
        param_name: primary_name.clone(),
        predicate: format!("requires {}", display_pred(pred)),
        span: call_span,
        outcome: proof_outcome,
    });
}

// ── Internal sub-module ───────────────────────────────────────────────────────
//
// Phase 3/4/5 — invariant/decreases/field-refinement checks. Shares the helpers
// defined below via `pub(super)` visibility.
mod loop_and_field;

use loop_and_field::{check_actor_behavior_contracts, check_invariants_in_block};
pub use loop_and_field::{check_actor_field_refinements, check_struct_field_refinements};

// ── Predicate helpers (shared by both checking modules) ──────────────────────

/// Collect all identifier names referenced in a predicate (may contain duplicates).
pub(super) fn collect_ident_names(pred: &RefExpr) -> Vec<String> {
    let mut names = Vec::new();
    collect_idents_inner(pred, &mut names);
    names
}

pub(super) fn collect_idents_inner(pred: &RefExpr, names: &mut Vec<String>) {
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
        RefExpr::Integer { .. } | RefExpr::Float { .. } | RefExpr::Bool { .. } => {}
        // Field access: collect idents from the object (e.g. `self` in `self.size`).
        RefExpr::FieldAccess { object, .. } => collect_idents_inner(object, names),
    }
}

/// Format a predicate for error messages.
pub(super) fn display_pred(pred: &RefExpr) -> String {
    match pred {
        RefExpr::Ident { name, .. } => name.clone(),
        RefExpr::Integer { value, .. } => value.to_string(),
        RefExpr::Float { value, .. } => value.to_string(),
        RefExpr::Bool { value, .. } => value.to_string(),
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
