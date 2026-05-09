//! Refinement type checker — symbolic proof for `where` predicates.
//!
//! Implements Requirement 10 of the MVL spec (001-type-system/Req 5).
//!
//! # Approach
//!
//! Three outcomes per call-site argument that has a refined parameter:
//!
//! | Outcome      | Meaning                                                       |
//! |--------------|---------------------------------------------------------------|
//! | Proven       | The argument's value/type statically satisfies the refinement |
//! | RuntimeCheck | Cannot prove statically — runtime assertion needed            |
//! | Failed       | The argument statically violates the refinement               |
//!
//! ## Constraint evaluation strategy
//!
//! - **Literals** (`42`, `0.0`): evaluate the predicate with the literal as `self`.
//! - **Same-refinement variables**: if the argument identifier carries a structurally
//!   equivalent refinement predicate, subsumption is proven.
//! - **Everything else**: falls back to `RuntimeCheck`.
//!
//! This approach covers the acceptance criteria for Phase 3 without requiring
//! an external SMT solver.  Full Z3/CVC5 integration is deferred to a later phase.

use std::collections::HashMap;

use crate::mvl::checker::const_eval;
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::solver::{RefResult, RefinementSolver};
use crate::mvl::parser::ast::{
    ArithOp, BinaryOp, Block, CmpOp, Decl, ElseBranch, Expr, FnDecl, LValue, Literal, LogicOp,
    MatchArm, MatchBody, Pattern, Program, RefExpr, Stmt, TypeBody, TypeExpr,
};
use crate::mvl::parser::lexer::Span;

// ── Counts ────────────────────────────────────────────────────────────────────

/// Per-program refinement check outcome counts.
#[derive(Debug, Default, Clone)]
pub struct RefinementCounts {
    /// Call-site arguments proven to satisfy their refinement statically.
    pub proven: usize,
    /// Call-site arguments that could not be proven; will need runtime checks.
    pub runtime_checked: usize,
    /// Call-site arguments definitively known to violate their refinement.
    pub failed: usize,
}

// ── Entry points ──────────────────────────────────────────────────────────────

/// Emit [`CheckError::RefinementViolated`] for every definite predicate violation.
///
/// Called from `checker::check()` after the main type-checking pass.
pub fn check_refinements(prog: &Program, errors: &mut Vec<CheckError>) {
    let mut counts = RefinementCounts::default();
    let fn_params = build_fn_param_refinements(prog);
    let type_refs = build_type_alias_refinements(prog);
    let fn_decls = build_pure_fn_decls(prog);
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                let mut var_refs = param_refinements(fd, &type_refs);
                analyze_block(
                    &fd.body,
                    &mut var_refs,
                    &fn_params,
                    &type_refs,
                    &fn_decls,
                    errors,
                    &mut counts,
                );
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    let mut var_refs = param_refinements(method, &type_refs);
                    analyze_block(
                        &method.body,
                        &mut var_refs,
                        &fn_params,
                        &type_refs,
                        &fn_decls,
                        errors,
                        &mut counts,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Count proven / runtime-checked / failed refinement call sites.
///
/// Does not emit errors; used by [`crate::mvl::checker::passes::RefinementsPass`]
/// to build the assurance verdict.
pub fn count_refinements(prog: &Program) -> RefinementCounts {
    let mut errors = Vec::new();
    let mut counts = RefinementCounts::default();
    let fn_params = build_fn_param_refinements(prog);
    let type_refs = build_type_alias_refinements(prog);
    let fn_decls = build_pure_fn_decls(prog);
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                let mut var_refs = param_refinements(fd, &type_refs);
                analyze_block(
                    &fd.body,
                    &mut var_refs,
                    &fn_params,
                    &type_refs,
                    &fn_decls,
                    &mut errors,
                    &mut counts,
                );
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    let mut var_refs = param_refinements(method, &type_refs);
                    analyze_block(
                        &method.body,
                        &mut var_refs,
                        &fn_params,
                        &type_refs,
                        &fn_decls,
                        &mut errors,
                        &mut counts,
                    );
                }
            }
            _ => {}
        }
    }
    counts
}

// ── Lookup table builders ─────────────────────────────────────────────────────

/// Maps pure function name → `FnDecl` for compile-time constant folding.
///
/// Only pure functions (empty effects list) are included; effectful functions
/// cannot be safely evaluated at compile time.
///
/// Both top-level `fn` declarations and pure methods inside `impl` blocks are
/// collected.  Methods are registered under their bare name; if two methods on
/// different types share the same name the last one wins (acceptable — folding
/// is conservative and `None` is always a safe fallback).
fn build_pure_fn_decls(prog: &Program) -> HashMap<String, FnDecl> {
    let mut map = HashMap::new();
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) if fd.effects.is_empty() => {
                map.insert(fd.name.clone(), fd.clone());
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    if method.effects.is_empty() {
                        map.insert(method.name.clone(), method.clone());
                    }
                }
            }
            _ => {}
        }
    }
    map
}

/// Maps function name → `Vec<(param_name, Option<RefExpr>)>` for top-level functions.
fn build_fn_param_refinements(prog: &Program) -> HashMap<String, Vec<(String, Option<RefExpr>)>> {
    let mut map = HashMap::new();
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                map.insert(fd.name.clone(), param_ref_vec(fd));
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    // Methods are registered under their bare name for simplicity;
                    // collision between methods on different types is acceptable
                    // at this phase — the analysis is conservative.
                    map.insert(method.name.clone(), param_ref_vec(method));
                }
            }
            _ => {}
        }
    }
    map
}

fn param_ref_vec(fd: &FnDecl) -> Vec<(String, Option<RefExpr>)> {
    fd.params
        .iter()
        .map(|p| {
            // Normalise the parameter name to "self" so that predicates written
            // as `b != 0` (where `b` is the param name) compare equal to
            // `self != 0` and to caller-side predicates like `y != 0`.
            let pred = p.refinement.as_ref().map(|r| normalize_pred(r, &p.name));
            (p.name.clone(), pred)
        })
        .collect()
}

/// Maps type alias name → the refinement attached to that alias (if any).
///
/// E.g. `type PositiveInt = Int where self > 0` → `"PositiveInt" → Some(self > 0)`.
fn build_type_alias_refinements(prog: &Program) -> HashMap<String, Option<RefExpr>> {
    let mut map = HashMap::new();
    for decl in &prog.declarations {
        if let Decl::Type(td) = decl {
            // Only simple `type A = B where pred` aliases carry a refinement.
            // Struct / enum bodies do not resolve to a single predicate here.
            let pred = match &td.body {
                TypeBody::Alias(inner) => extract_type_refinement(inner),
                _ => None,
            };
            map.insert(td.name.clone(), pred);
        }
    }
    map
}

/// Extract the outermost refinement from a `TypeExpr`, if present.
fn extract_type_refinement(ty: &TypeExpr) -> Option<RefExpr> {
    match ty {
        TypeExpr::Refined { pred, .. } => Some(pred.clone()),
        _ => None,
    }
}

/// Build the variable-refinement map for a function's own parameters.
///
/// Inline refinements are normalised so the parameter name becomes `"self"`,
/// matching the canonical form used in type aliases and in the callee table.
fn param_refinements(
    fd: &FnDecl,
    type_refs: &HashMap<String, Option<RefExpr>>,
) -> HashMap<String, Option<RefExpr>> {
    let mut map = HashMap::new();
    for p in &fd.params {
        // Inline refinement takes priority; normalise param name → "self".
        let pred = p
            .refinement
            .as_ref()
            .map(|r| normalize_pred(r, &p.name))
            .or_else(|| resolve_type_alias_pred(&p.ty, type_refs));
        map.insert(p.name.clone(), pred);
    }
    map
}

// ── Synthetic predicate helpers ──────────────────────────────────────────────

/// A zero-length span used for compiler-synthesised predicates.
fn dummy_span() -> Span {
    Span::new(0, 0, 0, 0)
}

/// `self == n` (integer literal equality).
fn self_eq_int(n: i64) -> RefExpr {
    let s = dummy_span();
    RefExpr::Compare {
        op: CmpOp::Eq,
        left: Box::new(RefExpr::Ident {
            name: "self".to_string(),
            span: s,
        }),
        right: Box::new(RefExpr::Integer { value: n, span: s }),
        span: s,
    }
}

/// `self != n` (integer literal inequality).
fn self_ne_int(n: i64) -> RefExpr {
    let s = dummy_span();
    RefExpr::Compare {
        op: CmpOp::Ne,
        left: Box::new(RefExpr::Ident {
            name: "self".to_string(),
            span: s,
        }),
        right: Box::new(RefExpr::Integer { value: n, span: s }),
        span: s,
    }
}

/// `self == f` (float literal equality).
fn self_eq_float(f: f64) -> RefExpr {
    let s = dummy_span();
    RefExpr::Compare {
        op: CmpOp::Eq,
        left: Box::new(RefExpr::Ident {
            name: "self".to_string(),
            span: s,
        }),
        right: Box::new(RefExpr::Float { value: f, span: s }),
        span: s,
    }
}

/// `self != f` (float literal inequality).
fn self_ne_float(f: f64) -> RefExpr {
    let s = dummy_span();
    RefExpr::Compare {
        op: CmpOp::Ne,
        left: Box::new(RefExpr::Ident {
            name: "self".to_string(),
            span: s,
        }),
        right: Box::new(RefExpr::Float { value: f, span: s }),
        span: s,
    }
}

/// Conjoin a non-empty list of predicates with `&&`.  Returns `None` when empty.
fn conj_preds(preds: Vec<RefExpr>) -> Option<RefExpr> {
    let s = dummy_span();
    let mut iter = preds.into_iter();
    let first = iter.next()?;
    Some(iter.fold(first, |acc, p| RefExpr::LogicOp {
        op: LogicOp::And,
        left: Box::new(acc),
        right: Box::new(p),
        span: s,
    }))
}

/// Build a `self == value` refinement predicate from a numeric `ConstValue`.
///
/// Used when a `let` binding is initialised with a constant-folded pure-function
/// call — we inject `self == <folded_value>` into `var_refs` so that the
/// refinement solver can statically prove predicates on that variable.
///
/// Returns `None` for non-numeric values (`Bool`, `Str`, `Unit`) because the
/// refinement language has no literal form for those types. Callers must skip
/// insertion into `var_refs` when `None` is returned.
fn lit_eq_pred(cv: &const_eval::ConstValue) -> Option<RefExpr> {
    let dummy = Span::default();
    let self_ref = Box::new(RefExpr::Ident {
        name: "self".to_string(),
        span: dummy,
    });
    let rhs = match cv {
        const_eval::ConstValue::Integer(n) => Box::new(RefExpr::Integer {
            value: *n,
            span: dummy,
        }),
        const_eval::ConstValue::Float(f) => Box::new(RefExpr::Float {
            value: *f,
            span: dummy,
        }),
        // Non-numeric folded values have no useful refinement hypothesis.
        _ => return None,
    };
    Some(RefExpr::Compare {
        op: CmpOp::Eq,
        left: self_ref,
        right: rhs,
        span: dummy,
    })
}

/// Extract the identifier name from a simple `Expr::Ident`, if present.
fn ident_name_from_expr(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name.as_str()),
        _ => None,
    }
}

/// Inject pattern-induced narrowing hypotheses into `arm_refs` for one match arm.
///
/// Four kinds of hypotheses are generated:
///
/// 1. **Literal arm** — `0 => ...` tells the solver the scrutinee equals `0`.
///    A guard on a literal arm (unusual but valid) is conjoined with the equality.
///    NaN float literals are skipped — no hypothesis is injected.
/// 2. **Catch-all ident arm** — `n => ...` after literal arms `0`, `1` tells the
///    solver that `n != 0 && n != 1` (complement of all prior literal values).
///    The complement is also written under the scrutinee name so that passing
///    either `n` or `x` to a callee proves the same refinement.
/// 3. **Wildcard arm** — `_ => ...` after literal arms gets the same complement
///    hypothesis injected under the scrutinee name.
/// 4. **Guard** — `n if n > 0 => ...` adds `n > 0` as a hypothesis for `n`.
fn inject_arm_hypotheses(
    scrutinee_name: Option<&str>,
    pattern: &Pattern,
    guard: Option<&RefExpr>,
    prior_int_lits: &[i64],
    prior_float_lits: &[f64],
    arm_refs: &mut HashMap<String, Option<RefExpr>>,
) {
    match pattern {
        // ── Literal arms: scrutinee is known to equal the matched literal ──────
        Pattern::Literal(Literal::Integer(n), _) => {
            if let Some(name) = scrutinee_name {
                let eq_hyp = self_eq_int(*n);
                let hyp = if let Some(g) = guard {
                    let normalized = normalize_pred(g, name);
                    RefExpr::LogicOp {
                        op: LogicOp::And,
                        left: Box::new(eq_hyp),
                        right: Box::new(normalized),
                        span: dummy_span(),
                    }
                } else {
                    eq_hyp
                };
                arm_refs.insert(name.to_string(), Some(hyp));
            }
        }
        // NaN cannot be a concrete equality hypothesis (NaN != NaN in IEEE 754).
        Pattern::Literal(Literal::Float(f), _) if !f.is_nan() => {
            if let Some(name) = scrutinee_name {
                let eq_hyp = self_eq_float(*f);
                let hyp = if let Some(g) = guard {
                    let normalized = normalize_pred(g, name);
                    RefExpr::LogicOp {
                        op: LogicOp::And,
                        left: Box::new(eq_hyp),
                        right: Box::new(normalized),
                        span: dummy_span(),
                    }
                } else {
                    eq_hyp
                };
                arm_refs.insert(name.to_string(), Some(hyp));
            }
        }
        // ── Catch-all ident: bound variable differs from all prior literals ────
        Pattern::Ident(var_name, _) => {
            let mut ne_preds: Vec<RefExpr> =
                prior_int_lits.iter().map(|&n| self_ne_int(n)).collect();
            ne_preds.extend(
                prior_float_lits
                    .iter()
                    .filter(|f| !f.is_nan())
                    .map(|&f| self_ne_float(f)),
            );
            let base_hyp = conj_preds(ne_preds);
            // Merge with guard predicate (if any).
            let hyp = match (base_hyp, guard) {
                (Some(base), Some(g)) => {
                    let normalized = normalize_pred(g, var_name);
                    Some(RefExpr::LogicOp {
                        op: LogicOp::And,
                        left: Box::new(base),
                        right: Box::new(normalized),
                        span: dummy_span(),
                    })
                }
                (Some(base), None) => Some(base),
                (None, Some(g)) => Some(normalize_pred(g, var_name)),
                (None, None) => None,
            };
            if let Some(h) = &hyp {
                arm_refs.insert(var_name.clone(), Some(h.clone()));
                // The scrutinee and the bound variable carry the same value;
                // narrow both so callers can use either name.
                if let Some(sname) = scrutinee_name {
                    if sname != var_name.as_str() {
                        arm_refs.insert(sname.to_string(), Some(h.clone()));
                    }
                }
            }
        }
        // ── Wildcard: complement of all prior literals on the scrutinee ───────
        Pattern::Wildcard(_) => {
            let mut ne_preds: Vec<RefExpr> =
                prior_int_lits.iter().map(|&n| self_ne_int(n)).collect();
            ne_preds.extend(
                prior_float_lits
                    .iter()
                    .filter(|f| !f.is_nan())
                    .map(|&f| self_ne_float(f)),
            );
            if let (Some(name), Some(hyp)) = (scrutinee_name, conj_preds(ne_preds)) {
                arm_refs.insert(name.to_string(), Some(hyp));
            }
        }
        // ── Other patterns: no scalar refinement hypothesis ───────────────────
        _ => {}
    }
}

// ── Predicate normalisation ───────────────────────────────────────────────────

/// Replace every occurrence of `param_name` with `"self"` in a predicate.
///
/// This canonicalises predicates written as `b != 0` (where `b` is the param
/// name) into `self != 0`, so that structural comparison works regardless of
/// what the parameter is called in different functions.
fn normalize_pred(pred: &RefExpr, param_name: &str) -> RefExpr {
    match pred {
        RefExpr::Ident { name, span } => RefExpr::Ident {
            name: if name == param_name {
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
            left: Box::new(normalize_pred(left, param_name)),
            right: Box::new(normalize_pred(right, param_name)),
            span: *span,
        },
        RefExpr::LogicOp {
            op,
            left,
            right,
            span,
        } => RefExpr::LogicOp {
            op: *op,
            left: Box::new(normalize_pred(left, param_name)),
            right: Box::new(normalize_pred(right, param_name)),
            span: *span,
        },
        RefExpr::ArithOp {
            op,
            left,
            right,
            span,
        } => RefExpr::ArithOp {
            op: *op,
            left: Box::new(normalize_pred(left, param_name)),
            right: Box::new(normalize_pred(right, param_name)),
            span: *span,
        },
        RefExpr::Not { inner, span } => RefExpr::Not {
            inner: Box::new(normalize_pred(inner, param_name)),
            span: *span,
        },
        RefExpr::Grouped { inner, span } => RefExpr::Grouped {
            inner: Box::new(normalize_pred(inner, param_name)),
            span: *span,
        },
        // Literals and Len don't contain the param name.
        other => other.clone(),
    }
}

/// If `ty` names a type alias that itself has a refinement, return that
/// refinement (so that `fn f(x: PositiveInt)` is equivalent to
/// `fn f(x: Int where self > 0)` for call-site checking).
fn resolve_type_alias_pred(
    ty: &TypeExpr,
    type_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<RefExpr> {
    if let TypeExpr::Base { name, .. } = ty {
        return type_refs.get(name).and_then(|v| v.clone());
    }
    None
}

// ── If-condition narrowing ────────────────────────────────────────────────────

/// Inject narrowing hypotheses derived from an if-condition into `var_refs`.
///
/// Handles simple integer comparisons (`x op n`, `n op x`) and `&&`
/// conjunctions.  Everything else is silently ignored — conservative and
/// always sound.  The caller is responsible for working on a *clone* of
/// `var_refs` so that the narrowing does not escape the if-branch.
fn inject_if_hypothesis(cond: &Expr, var_refs: &mut HashMap<String, Option<RefExpr>>) {
    let Expr::Binary {
        op, left, right, ..
    } = cond
    else {
        return;
    };
    if let Some(cmp) = binary_op_to_cmp(op) {
        // Recognise `x op n` and `n op x` (integer literal only).
        let (var_name, cmp_op, int_val) =
            if let (Expr::Ident(name, _), Expr::Literal(Literal::Integer(n), _)) =
                (left.as_ref(), right.as_ref())
            {
                (name.clone(), cmp, *n)
            } else if let (Expr::Literal(Literal::Integer(n), _), Expr::Ident(name, _)) =
                (left.as_ref(), right.as_ref())
            {
                (name.clone(), flip_cmp(cmp), *n)
            } else {
                return;
            };

        let s = dummy_span();
        let ref_expr = RefExpr::Compare {
            op: cmp_op,
            left: Box::new(RefExpr::Ident {
                name: "self".to_string(),
                span: s,
            }),
            right: Box::new(RefExpr::Integer {
                value: int_val,
                span: s,
            }),
            span: s,
        };
        // Conjoin with any existing hypothesis for this variable.
        let new_hyp = match var_refs.get(&var_name).and_then(|v| v.clone()) {
            Some(existing) => RefExpr::LogicOp {
                op: LogicOp::And,
                left: Box::new(existing),
                right: Box::new(ref_expr),
                span: s,
            },
            None => ref_expr,
        };
        var_refs.insert(var_name, Some(new_hyp));
    } else if *op == BinaryOp::And {
        // Recurse into both arms of a `&&` conjunction.
        inject_if_hypothesis(left, var_refs);
        inject_if_hypothesis(right, var_refs);
    }
}

/// Convert a `BinaryOp` comparison to the corresponding `CmpOp`, if applicable.
fn binary_op_to_cmp(op: &BinaryOp) -> Option<CmpOp> {
    match op {
        BinaryOp::Gt => Some(CmpOp::Gt),
        BinaryOp::Ge => Some(CmpOp::Ge),
        BinaryOp::Lt => Some(CmpOp::Lt),
        BinaryOp::Le => Some(CmpOp::Le),
        BinaryOp::Eq => Some(CmpOp::Eq),
        BinaryOp::Ne => Some(CmpOp::Ne),
        _ => None,
    }
}

/// Flip a comparison operator (swap left/right operands).
fn flip_cmp(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Lt => CmpOp::Gt,
        CmpOp::Gt => CmpOp::Lt,
        CmpOp::Le => CmpOp::Ge,
        CmpOp::Ge => CmpOp::Le,
        CmpOp::Eq => CmpOp::Eq,
        CmpOp::Ne => CmpOp::Ne,
    }
}

// ── AST walkers ───────────────────────────────────────────────────────────────

/// Walk the arms of a match expression/statement, injecting per-arm hypotheses.
///
/// Shared by `Stmt::Match` and `Expr::Match` — the loop body is identical in
/// both cases and lives here to avoid duplication.
#[allow(clippy::too_many_arguments)]
fn analyze_match_arms(
    scrutinee: &Expr,
    arms: &[MatchArm],
    var_refs: &mut HashMap<String, Option<RefExpr>>,
    fn_params: &HashMap<String, Vec<(String, Option<RefExpr>)>>,
    type_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
    counts: &mut RefinementCounts,
) {
    analyze_expr(
        scrutinee, var_refs, fn_params, type_refs, fn_decls, errors, counts,
    );
    let scrutinee_name = ident_name_from_expr(scrutinee);
    let mut prior_int_lits: Vec<i64> = Vec::new();
    let mut prior_float_lits: Vec<f64> = Vec::new();
    for arm in arms {
        // Each arm gets its own hypothesis set cloned from the outer scope.
        let mut arm_refs = var_refs.clone();
        inject_arm_hypotheses(
            scrutinee_name,
            &arm.pattern,
            arm.guard.as_ref(),
            &prior_int_lits,
            &prior_float_lits,
            &mut arm_refs,
        );
        // Record literal values so subsequent catch-all/wildcard arms know what was excluded.
        match &arm.pattern {
            Pattern::Literal(Literal::Integer(n), _) => prior_int_lits.push(*n),
            Pattern::Literal(Literal::Float(f), _) if !f.is_nan() => prior_float_lits.push(*f),
            _ => {}
        }
        match &arm.body {
            MatchBody::Expr(e) => analyze_expr(
                e,
                &mut arm_refs,
                fn_params,
                type_refs,
                fn_decls,
                errors,
                counts,
            ),
            MatchBody::Block(b) => analyze_block(
                b,
                &mut arm_refs,
                fn_params,
                type_refs,
                fn_decls,
                errors,
                counts,
            ),
        }
    }
}

fn analyze_block(
    block: &Block,
    var_refs: &mut HashMap<String, Option<RefExpr>>,
    fn_params: &HashMap<String, Vec<(String, Option<RefExpr>)>>,
    type_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
    counts: &mut RefinementCounts,
) {
    for stmt in &block.stmts {
        analyze_stmt(
            stmt, var_refs, fn_params, type_refs, fn_decls, errors, counts,
        );
    }
}

fn analyze_stmt(
    stmt: &Stmt,
    var_refs: &mut HashMap<String, Option<RefExpr>>,
    fn_params: &HashMap<String, Vec<(String, Option<RefExpr>)>>,
    type_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
    counts: &mut RefinementCounts,
) {
    match stmt {
        Stmt::Let {
            pattern, ty, init, ..
        } => {
            analyze_expr(
                init, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
            // Record refinement for the new variable, from its declared type or alias.
            let mut pred =
                extract_type_refinement(ty).or_else(|| resolve_type_alias_pred(ty, type_refs));
            // If no explicit refinement, try to constant-fold the initialiser.
            // When successful, inject a `self == folded_value` hypothesis so that
            // the refinement solver can prove predicates on the bound name statically.
            if pred.is_none() {
                if let Expr::FnCall { name, args, .. } = init {
                    if let Some(fd) = fn_decls.get(name) {
                        if let Some(cv) = const_eval::try_fold_call(fd, args, fn_decls) {
                            pred = lit_eq_pred(&cv);
                        }
                    }
                }
            }
            // Bind the refinement to any simple identifier in the pattern.
            if let Pattern::Ident(name, _) = pattern {
                var_refs.insert(name.clone(), pred);
            }
        }
        Stmt::Assign { target, value, .. } => {
            analyze_expr(
                value, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
            // Reassignment invalidates any refinement the variable carried from its binding.
            // Field assignments don't affect the variable's top-level refinement.
            if let LValue::Ident(name, _) = target {
                var_refs.insert(name.clone(), None);
            }
        }
        Stmt::Return { value, .. } => {
            if let Some(e) = value {
                analyze_expr(e, var_refs, fn_params, type_refs, fn_decls, errors, counts);
            }
        }
        Stmt::Expr { expr: e, .. } => {
            analyze_expr(e, var_refs, fn_params, type_refs, fn_decls, errors, counts);
        }
        Stmt::If {
            cond, then, else_, ..
        } => {
            analyze_expr(
                cond, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
            // Narrow the then-branch: clone var_refs and inject the condition
            // as an integer hypothesis.  Narrowings do not propagate out of the
            // branch — the original var_refs is left unchanged.
            let mut then_refs = var_refs.clone();
            inject_if_hypothesis(cond, &mut then_refs);
            analyze_block(
                then,
                &mut then_refs,
                fn_params,
                type_refs,
                fn_decls,
                errors,
                counts,
            );
            if let Some(eb) = else_ {
                match eb {
                    ElseBranch::Block(b) => {
                        analyze_block(b, var_refs, fn_params, type_refs, fn_decls, errors, counts)
                    }
                    ElseBranch::If(s) => {
                        analyze_stmt(s, var_refs, fn_params, type_refs, fn_decls, errors, counts)
                    }
                }
            }
        }
        Stmt::While { cond, body, .. } => {
            analyze_expr(
                cond, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
            analyze_block(
                body, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
        }
        Stmt::For { iter, body, .. } => {
            analyze_expr(
                iter, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
            analyze_block(
                body, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            analyze_match_arms(
                scrutinee, arms, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
        }
    }
}

fn analyze_expr(
    expr: &Expr,
    var_refs: &mut HashMap<String, Option<RefExpr>>,
    fn_params: &HashMap<String, Vec<(String, Option<RefExpr>)>>,
    type_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
    counts: &mut RefinementCounts,
) {
    match expr {
        Expr::FnCall {
            name, args, span, ..
        } => {
            // Check each argument against the callee's parameter refinements.
            if let Some(param_refs) = fn_params.get(name) {
                check_call_site(
                    name, args, *span, param_refs, var_refs, fn_decls, errors, counts,
                );
            }
            // Recurse into arguments regardless.
            for arg in args {
                analyze_expr(
                    arg, var_refs, fn_params, type_refs, fn_decls, errors, counts,
                );
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            analyze_expr(
                receiver, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
            for arg in args {
                analyze_expr(
                    arg, var_refs, fn_params, type_refs, fn_decls, errors, counts,
                );
            }
        }
        Expr::Binary { left, right, .. } => {
            analyze_expr(
                left, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
            analyze_expr(
                right, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
        }
        Expr::Unary { expr: inner, .. }
        | Expr::FieldAccess { expr: inner, .. }
        | Expr::Propagate { expr: inner, .. }
        | Expr::Move { expr: inner, .. }
        | Expr::Consume { expr: inner, .. }
        | Expr::Declassify { expr: inner, .. }
        | Expr::Sanitize { expr: inner, .. }
        | Expr::Borrow { expr: inner, .. } => {
            analyze_expr(
                inner, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            analyze_expr(
                cond, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
            let mut then_refs = var_refs.clone();
            inject_if_hypothesis(cond, &mut then_refs);
            analyze_block(
                then,
                &mut then_refs,
                fn_params,
                type_refs,
                fn_decls,
                errors,
                counts,
            );
            if let Some(e) = else_ {
                analyze_expr(e, var_refs, fn_params, type_refs, fn_decls, errors, counts);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            analyze_match_arms(
                scrutinee, arms, var_refs, fn_params, type_refs, fn_decls, errors, counts,
            );
        }
        Expr::Lambda { params, body, .. } => {
            // Lambda params may have refinements; normalise them (param name → "self")
            // before inserting so that preds_equivalent works correctly.
            let mut child_refs = var_refs.clone();
            for p in params {
                let pred = p.refinement.as_ref().map(|r| normalize_pred(r, &p.name));
                child_refs.insert(p.name.clone(), pred);
            }
            analyze_expr(
                body,
                &mut child_refs,
                fn_params,
                type_refs,
                fn_decls,
                errors,
                counts,
            );
        }
        Expr::Block(b) => {
            analyze_block(b, var_refs, fn_params, type_refs, fn_decls, errors, counts);
        }
        Expr::Construct { fields, .. } => {
            for (_, e) in fields {
                analyze_expr(e, var_refs, fn_params, type_refs, fn_decls, errors, counts);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                analyze_expr(e, var_refs, fn_params, type_refs, fn_decls, errors, counts);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                analyze_expr(k, var_refs, fn_params, type_refs, fn_decls, errors, counts);
                analyze_expr(v, var_refs, fn_params, type_refs, fn_decls, errors, counts);
            }
        }
        // Leaves: Literal, Ident — no sub-expressions to walk.
        Expr::Literal(_, _) | Expr::Ident(_, _) => {}
    }
}

// ── Call-site checker ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn check_call_site(
    fn_name: &str,
    args: &[Expr],
    call_span: Span,
    param_refs: &[(String, Option<RefExpr>)],
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
    counts: &mut RefinementCounts,
) {
    for (arg, (_, param_pred)) in args.iter().zip(param_refs.iter()) {
        let Some(pred) = param_pred else { continue };
        let outcome = check_arg_against_pred(arg, pred, var_refs, fn_decls);
        match outcome {
            RefResult::Proven => counts.proven += 1,
            RefResult::RuntimeCheck => counts.runtime_checked += 1,
            RefResult::Failed => {
                counts.failed += 1;
                errors.push(CheckError::RefinementViolated {
                    pred: format!(
                        "argument to `{fn_name}` violates refinement `{}`",
                        display_pred(pred)
                    ),
                    span: call_span,
                });
            }
        }
    }
}

// ── Argument checking ─────────────────────────────────────────────────────────

fn check_arg_against_pred(
    arg: &Expr,
    pred: &RefExpr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
) -> RefResult {
    RefinementSolver::try_trivial(pred, arg, var_refs, fn_decls)
        .or_else(|| RefinementSolver::try_interval(pred, arg, var_refs))
        .or_else(|| RefinementSolver::try_symbolic(pred, arg, var_refs, fn_decls))
        .unwrap_or(RefResult::RuntimeCheck)
}

// ── Predicate display ─────────────────────────────────────────────────────────

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
        RefExpr::Len { ident, .. } => format!("len({ident})"),
    }
}
