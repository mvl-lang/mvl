// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Phase 3–5 contract checks: while/for loop invariants, `decreases`
//! termination measures, and actor/struct field-refinement checks at
//! spawn/construct sites.
//!
//! Split out of `contracts.rs` (#1561) so the requires/ensures pass and the
//! invariant/decreases/field passes live in focused files.  Helpers shared
//! with the parent module (e.g. `ContractCheckCtx`, `walk_stmts`,
//! `display_pred`, `collect_ident_names`, `normalize_pred`,
//! `build_param_var_refs`, `build_fn_decls_for_solver`) are accessed via
//! `super::`; visibility is `pub(super)` on the parent definitions.

use std::collections::HashMap;

use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::refinements::{
    check_arg_against_pred_counted, ProofOutcome, ProofSite, RefinementCounts,
};
use crate::mvl::checker::solver::{RefResult, SolverMode};
use crate::mvl::parser::ast::{
    expr_to_ref_expr_ext, ActorDecl, ArithOp, Block, CmpOp, Decl, ElseBranch, Expr, FieldDecl,
    LValue, LetKind, Literal, LogicOp, MatchBody, Program, RefExpr, Stmt, TypeBody, UnaryOp,
    VariantFields,
};
use crate::mvl::parser::lexer::Span;

use super::{
    build_fn_decls_for_solver, build_param_var_refs_full, check_requires_at_call,
    collect_ident_names, display_pred, normalize_pred, walk_stmts, ContractCheckCtx, FnContracts,
};

// ── Phase 3: invariant checker ────────────────────────────────────────────────

/// Walk a block and check every `while` loop's invariants at loop entry.
pub(super) fn check_invariants_in_block(
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

    let (outcome_opt, param_label) = match distinct.as_slice() {
        [] => {
            // Constant predicate (e.g., `invariant 0 >= 0` or `invariant 1 < 0`).
            // The predicate has no `self` reference; pass a dummy literal as the argument.
            // Layer 1 will const-fold the comparison directly.
            let dummy = Expr::Literal(Literal::Integer(0), loop_span);
            let layer_before = ctx.counts.by_layer;
            let outcome = check_arg_against_pred_counted(
                &dummy,
                inv_pred,
                var_refs,
                ctx.fn_decls,
                ctx.counts,
            );
            (Some((outcome, layer_before)), "const".to_string())
        }
        [var_name] => {
            // Single free variable — normalise it to "self" and check via Ident lookup.
            let normalized = normalize_pred(inv_pred, var_name);
            let ident_expr = Expr::Ident(var_name.clone(), loop_span);
            let layer_before = ctx.counts.by_layer;
            let outcome = check_arg_against_pred_counted(
                &ident_expr,
                &normalized,
                var_refs,
                ctx.fn_decls,
                ctx.counts,
            );
            (Some((outcome, layer_before)), var_name.clone())
        }
        _ => {
            // Multiple variables: defer to Phase 4 (RuntimeCheck, no error).
            (None, String::new())
        }
    };

    if let Some((outcome, layer_before)) = outcome_opt {
        let layer = (1..6)
            .find(|&i| ctx.counts.by_layer[i] > layer_before[i])
            .unwrap_or(0);
        let proof_outcome = match &outcome {
            RefResult::Proven => ProofOutcome::Proven {
                layer,
                is_bv: false,
            },
            RefResult::ProvenBv => ProofOutcome::Proven { layer, is_bv: true },
            RefResult::RuntimeCheck => ProofOutcome::RuntimeCheck,
            RefResult::Failed { counterexample } => {
                ctx.errors.push(CheckError::InvariantViolated {
                    fn_name: fn_name.to_string(),
                    pred: display_pred(inv_pred),
                    span: loop_span,
                    counterexample: counterexample.clone(),
                });
                ProofOutcome::Failed
            }
        };
        // #836: Record invariant proof in sites for `mvl prove`.
        ctx.counts.sites.push(ProofSite {
            caller_fn: ctx.counts.current_fn.clone(),
            fn_name: fn_name.to_string(),
            param_name: param_label,
            predicate: format!("invariant {}", display_pred(inv_pred)),
            span: loop_span,
            outcome: proof_outcome,
        });
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
    if matches!(outcome, RefResult::Proven | RefResult::ProvenBv) {
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
    if matches!(outcome, RefResult::Proven | RefResult::ProvenBv) {
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
    if matches!(outcome, RefResult::Proven | RefResult::ProvenBv) {
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
pub(super) fn check_actor_behavior_contracts(
    ad: &ActorDecl,
    fn_map: &HashMap<String, FnContracts>,
    ctx: &mut ContractCheckCtx<'_>,
) {
    for method in &ad.methods {
        ctx.counts.current_fn = format!("{}::{}", ad.name, method.name);
        let var_refs = build_param_var_refs_full(
            &method.params,
            ctx.type_refs,
            ctx.struct_fields,
            ctx.const_refs,
        );
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
    counts: &mut RefinementCounts,
) {
    // Build a map: actor_name → field declarations (only those with refinements).
    let actor_fields = build_actor_field_map(prog);
    if actor_fields.is_empty() {
        return; // Fast path: no actor has refined fields.
    }
    let fn_decls = build_fn_decls_for_solver(prog);
    let type_refs = crate::mvl::checker::refinements::build_type_alias_refinements(prog);
    let struct_fields =
        crate::mvl::checker::refinements::build_struct_field_refinements_combined(&[prog]);
    let const_map = crate::mvl::checker::refinements::build_const_map(&[prog]);
    let const_refs = crate::mvl::checker::refinements::const_map_to_var_refs(&const_map);
    counts.mode = mode;
    let mut ctx = ContractCheckCtx {
        fn_decls: &fn_decls,
        errors,
        counts,
        type_refs: &type_refs,
        struct_fields: &struct_fields,
        const_refs: &const_refs,
    };

    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                // Seed var_refs from function parameters so the solver can use
                // where-refinements on parameter variables as hypotheses.
                let var_refs = build_param_var_refs_full(
                    &fd.params,
                    ctx.type_refs,
                    ctx.struct_fields,
                    ctx.const_refs,
                );
                walk_stmts(&fd.body, &mut ctx, &mut |expr, ctx| {
                    check_spawn_at_site(expr, &actor_fields, &var_refs, ctx);
                });
            }
            Decl::Impl(impl_d) => {
                for method in &impl_d.methods {
                    let var_refs = build_param_var_refs_full(
                        &method.params,
                        ctx.type_refs,
                        ctx.struct_fields,
                        ctx.const_refs,
                    );
                    walk_stmts(&method.body, &mut ctx, &mut |expr, ctx| {
                        check_spawn_at_site(expr, &actor_fields, &var_refs, ctx);
                    });
                }
            }
            Decl::Actor(ad) => {
                for method in &ad.methods {
                    let var_refs = build_param_var_refs_full(
                        &method.params,
                        ctx.type_refs,
                        ctx.struct_fields,
                        ctx.const_refs,
                    );
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
                let layer_before = ctx.counts.by_layer;
                let outcome = check_arg_against_pred_counted(
                    init_expr,
                    pred,
                    var_refs,
                    ctx.fn_decls,
                    ctx.counts,
                );
                let layer = (1..6)
                    .find(|&i| ctx.counts.by_layer[i] > layer_before[i])
                    .unwrap_or(0);
                let proof_outcome = match &outcome {
                    RefResult::Proven => ProofOutcome::Proven {
                        layer,
                        is_bv: false,
                    },
                    RefResult::ProvenBv => ProofOutcome::Proven { layer, is_bv: true },
                    RefResult::RuntimeCheck => ProofOutcome::RuntimeCheck,
                    RefResult::Failed { counterexample } => {
                        ctx.errors.push(CheckError::RefinementViolated {
                            pred: format!("{actor_type}.{init_name}: {}", display_pred(pred)),
                            counterexample: counterexample.clone(),
                            span: *span,
                        });
                        ProofOutcome::Failed
                    }
                };
                // #836: Record actor-field init refinement in sites.
                ctx.counts.sites.push(ProofSite {
                    caller_fn: ctx.counts.current_fn.clone(),
                    fn_name: actor_type.clone(),
                    param_name: init_name.clone(),
                    predicate: format!("field {}", display_pred(pred)),
                    span: *span,
                    outcome: proof_outcome,
                });
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
    counts: &mut RefinementCounts,
) {
    let field_map = build_struct_field_map(prog);
    if field_map.is_empty() {
        return;
    }
    let fn_decls = build_fn_decls_for_solver(prog);
    let type_refs = crate::mvl::checker::refinements::build_type_alias_refinements(prog);
    let struct_fields =
        crate::mvl::checker::refinements::build_struct_field_refinements_combined(&[prog]);
    let const_map = crate::mvl::checker::refinements::build_const_map(&[prog]);
    let const_refs = crate::mvl::checker::refinements::const_map_to_var_refs(&const_map);
    counts.mode = mode;
    let mut ctx = ContractCheckCtx {
        fn_decls: &fn_decls,
        errors,
        counts,
        type_refs: &type_refs,
        struct_fields: &struct_fields,
        const_refs: &const_refs,
    };

    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                let var_refs = build_param_var_refs_full(
                    &fd.params,
                    ctx.type_refs,
                    ctx.struct_fields,
                    ctx.const_refs,
                );
                walk_stmts(&fd.body, &mut ctx, &mut |expr, ctx| {
                    check_construct_at_site(expr, &field_map, &var_refs, ctx);
                });
            }
            Decl::Impl(impl_d) => {
                for method in &impl_d.methods {
                    let var_refs = build_param_var_refs_full(
                        &method.params,
                        ctx.type_refs,
                        ctx.struct_fields,
                        ctx.const_refs,
                    );
                    walk_stmts(&method.body, &mut ctx, &mut |expr, ctx| {
                        check_construct_at_site(expr, &field_map, &var_refs, ctx);
                    });
                }
            }
            Decl::Actor(ad) => {
                for method in &ad.methods {
                    let var_refs = build_param_var_refs_full(
                        &method.params,
                        ctx.type_refs,
                        ctx.struct_fields,
                        ctx.const_refs,
                    );
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
                let layer_before = ctx.counts.by_layer;
                let outcome = check_arg_against_pred_counted(
                    init_expr,
                    pred,
                    var_refs,
                    ctx.fn_decls,
                    ctx.counts,
                );
                let layer = (1..6)
                    .find(|&i| ctx.counts.by_layer[i] > layer_before[i])
                    .unwrap_or(0);
                let proof_outcome = match &outcome {
                    RefResult::Proven => ProofOutcome::Proven {
                        layer,
                        is_bv: false,
                    },
                    RefResult::ProvenBv => ProofOutcome::Proven { layer, is_bv: true },
                    RefResult::RuntimeCheck => ProofOutcome::RuntimeCheck,
                    RefResult::Failed { counterexample } => {
                        ctx.errors.push(CheckError::RefinementViolated {
                            pred: format!("{name}.{init_name}: {}", display_pred(pred)),
                            counterexample: counterexample.clone(),
                            span: *span,
                        });
                        ProofOutcome::Failed
                    }
                };
                // #836: Record struct-field init refinement in sites.
                ctx.counts.sites.push(ProofSite {
                    caller_fn: ctx.counts.current_fn.clone(),
                    fn_name: name.clone(),
                    param_name: init_name.clone(),
                    predicate: format!("field {}", display_pred(pred)),
                    span: *span,
                    outcome: proof_outcome,
                });
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
