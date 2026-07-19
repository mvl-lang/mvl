// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Layer 5: Z3 SMT solver — complete integer arithmetic.
//!
//! When Layers 1–4 cannot statically prove or disprove a refinement predicate,
//! this layer delegates to the Z3 theorem prover via the `z3` crate.
//!
//! # Query structure
//!
//! To check whether a call-site argument `arg` satisfies predicate `pred`:
//!
//! 1. For each in-scope variable `v` with hypothesis `h(self)`, assert `h(v)`.
//! 2. Assert `¬pred(arg)`.
//! 3. If Z3 returns `unsat`, the negation is unsatisfiable → `pred(arg)` holds
//!    for every value consistent with the hypotheses → **Proven**.
//! 4. Any other result (sat, unknown, timeout) → **None** (fall through to
//!    RuntimeCheck).
//!
//! # Supported subset
//!
//! **QF-NIA path** (default): integer constants, free variables, `+`, `-`, `*`,
//! `div`, `rem`, and all six comparison operators, `&&`, `||`, `!`.
//!
//! **QF-BV path** (#1928): triggered when the predicate contains any bitwise
//! operation (`bit_and`, `bit_or`, `bit_xor`, `bit_not`, `shift_left`,
//! `shift_right`).  All variables and literals are encoded as 64-bit signed
//! bit-vectors.  Returns `RefResult::ProvenBv` so the caller can label the
//! proof site `(5:z3-bv)` in `mvl prove` output.
//!
//! Float predicates, `Len` nodes, and non-linear multiplication of two unknowns
//! are translated to `None` so the layer safely falls through.
//!
//! Compile-time gated: the entire implementation is `#[cfg(feature = "z3")]`.
//! When the feature is absent, `try_z3` is a no-op returning `None`.
//!
//! # Builtin axioms (#597)
//!
//! When the predicate or variable hypotheses reference `len(ident)`, the solver
//! pre-creates a Z3 integer variable `len_<ident>` and asserts the universal
//! axiom `len_<ident> >= 0` (lengths are non-negative).
//!
//! For string-literal arguments the solver additionally asserts
//! `len_self = <actual byte length>`, connecting the concrete value to Z3's
//! integer domain.  Variable arguments that carry a `len(self)` hypothesis
//! (e.g. `x: String where len(self) > 5`) have their length hypothesis
//! asserted as `len_x > 5`, propagating known constraints into the proof.

use std::collections::HashMap;

use crate::mvl::parser::ast::{CmpOp, Expr, RefExpr};
use crate::mvl::parser::lexer::Span;

use super::RefResult;

// ── Public entry point ────────────────────────────────────────────────────────

/// Try to prove `pred(arg)` using the Z3 SMT solver.
///
/// When the predicate contains bitwise operations, dispatches to the QF-BV
/// path and returns `RefResult::ProvenBv` on success.  Otherwise uses the
/// standard QF-NIA path and returns `RefResult::Proven`.
/// Returns `None` for unsupported constructs or when Z3 cannot decide within
/// the built-in 1 second timeout.
pub(crate) fn try_z3(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<RefResult> {
    #[cfg(feature = "z3")]
    {
        if has_bitwise_ops(pred) {
            if let Some(r) = impl_z3_bv(pred, arg, var_refs) {
                return Some(r);
            }
            // BV path returned None (unsupported shape) — fall through to NIA.
        }
        return impl_z3(pred, arg, var_refs);
    }
    #[cfg(not(feature = "z3"))]
    {
        let _ = (pred, arg, var_refs);
        None
    }
}

// ── Bitwise-op detection ──────────────────────────────────────────────────────

/// Returns `true` if `pred` contains any `BitwiseOp` or `BitwiseNot` node.
fn has_bitwise_ops(pred: &RefExpr) -> bool {
    match pred {
        RefExpr::BitwiseOp { .. } | RefExpr::BitwiseNot { .. } => true,
        RefExpr::LogicOp { left, right, .. }
        | RefExpr::Compare { left, right, .. }
        | RefExpr::ArithOp { left, right, .. } => has_bitwise_ops(left) || has_bitwise_ops(right),
        RefExpr::Not { inner, .. }
        | RefExpr::Grouped { inner, .. }
        | RefExpr::Old { inner, .. } => has_bitwise_ops(inner),
        RefExpr::Forall { body, .. } | RefExpr::Exists { body, .. } => has_bitwise_ops(body),
        RefExpr::FieldAccess { object, .. } => has_bitwise_ops(object),
        _ => false,
    }
}

// ── Axis 2: contract tightening ──────────────────────────────────────────────

/// Result of a tightening binary search.
#[derive(Debug, Clone)]
pub(crate) struct TightenResult {
    pub op: CmpOp,
    pub tighter_bound: i64,
}

impl TightenResult {
    /// Format the tighter predicate as it would appear in an `ensures` clause.
    pub fn tighter_ensures(&self, prefix: &str) -> String {
        let op_str = match self.op {
            CmpOp::Ge => ">=",
            CmpOp::Gt => ">",
            CmpOp::Le => "<=",
            CmpOp::Lt => "<",
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
        };
        format!("ensures {prefix} {op_str} {}", self.tighter_bound)
    }
}

/// Build a `self OP bound` RefExpr from parts.
fn make_self_cmp(op: CmpOp, bound: i64, span: Span) -> RefExpr {
    RefExpr::Compare {
        op,
        left: Box::new(RefExpr::Ident {
            name: "self".into(),
            span,
        }),
        right: Box::new(RefExpr::Integer { value: bound, span }),
        span,
    }
}

/// Try to find a tighter provable integer bound for a simple `self OP N` predicate.
///
/// Called after `check_ensures_for_return` determines that the declared `ensures`
/// clause is **Proven** — this function asks: is there a strictly tighter bound
/// that is also provable?  Binary-searches ±1 000 000 around the declared bound.
///
/// Returns `Some(TightenResult)` when a strictly tighter bound is found.
/// Returns `None` when the predicate is not a simple `self OP N` form, when no
/// improvement exists within the search range, or when the `z3` feature is absent.
pub(crate) fn try_z3_tighten(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<TightenResult> {
    #[cfg(feature = "z3")]
    return impl_z3_tighten(pred, arg, var_refs);
    #[cfg(not(feature = "z3"))]
    {
        let _ = (pred, arg, var_refs);
        None
    }
}

// ── Axis 3: boundary witness synthesis ───────────────────────────────────────

/// Try to synthesize a concrete witness input that satisfies the branch
/// conditions active at a tightening candidate's return point.
///
/// For each `Int` parameter, a Z3 integer variable is created under the
/// parameter name.  For struct parameters, one Z3 integer variable is
/// created per field using the naming convention `param__field`.  Branch
/// hypotheses (active `if`-conditions) are asserted as constraints.  If Z3
/// returns `Sat`, the model is extracted and returned as `WitnessArg` values.
///
/// Returns `None` when the `z3` feature is absent, when all parameters are
/// non-integer, or when Z3 cannot find a satisfying assignment within the
/// 1-second timeout.
pub(crate) fn try_z3_witness(
    params: &[crate::mvl::parser::ast::Param],
    branch_hyps: &[Expr],
    struct_fields: &HashMap<String, Vec<(String, String)>>,
) -> Option<Vec<crate::mvl::checker::refinements::WitnessArg>> {
    #[cfg(feature = "z3")]
    return impl_z3_witness(params, branch_hyps, struct_fields);
    #[cfg(not(feature = "z3"))]
    {
        let _ = (params, branch_hyps, struct_fields);
        None
    }
}

// ── Z3 implementation (feature-gated) ────────────────────────────────────────

/// Axis 2 tightening implementation (Z3-gated).
///
/// For `self >= N` predicates: binary-searches UPWARD for the largest N' > N
/// that is still provable (larger lower bound = tighter).
/// For `self <= N` predicates: binary-searches DOWNWARD for the smallest N' < N
/// that is still provable (smaller upper bound = tighter).
/// `self > N` and `self < N` are handled analogously.
#[cfg(feature = "z3")]
fn impl_z3_tighten(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<TightenResult> {
    use super::dummy_span;

    let (op, current_bound) = extract_simple_self_bound(pred)?;

    // Search range: ±1_000_000 from the current bound.
    const RANGE: i64 = 1_000_000;
    let (mut lo, mut hi, upward) = match op {
        CmpOp::Ge | CmpOp::Gt => (current_bound + 1, current_bound + RANGE, true),
        CmpOp::Le | CmpOp::Lt => (current_bound - RANGE, current_bound - 1, false),
        _ => return None,
    };

    // Guard: if even the first step isn't provable (shouldn't happen for Proven
    // input, but Z3 may timeout), bail early.
    if lo > hi {
        return None;
    }

    let span = dummy_span();
    let mut best: Option<i64> = None;

    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let candidate = make_self_cmp(op, mid, span);
        let proven = try_z3(&candidate, arg, var_refs) == Some(RefResult::Proven);

        if upward {
            if proven {
                best = Some(mid);
                lo = mid + 1; // try even higher
            } else {
                hi = mid - 1; // too tight, back off
            }
        } else if proven {
            best = Some(mid);
            hi = mid - 1; // try even lower
        } else {
            lo = mid + 1; // too tight, back off
        }
    }

    best.map(|tighter_bound| TightenResult { op, tighter_bound })
}

/// Extract `(op, bound)` from a simple `self OP bound` RefExpr.
#[cfg(feature = "z3")]
fn extract_simple_self_bound(pred: &RefExpr) -> Option<(CmpOp, i64)> {
    let RefExpr::Compare {
        op, left, right, ..
    } = pred
    else {
        return None;
    };
    match (left.as_ref(), right.as_ref()) {
        (RefExpr::Ident { name, .. }, RefExpr::Integer { value, .. }) if name == "self" => {
            Some((*op, *value))
        }
        (RefExpr::Integer { value, .. }, RefExpr::Ident { name, .. }) if name == "self" => {
            Some((flip_cmp(*op), *value))
        }
        _ => None,
    }
}

#[cfg(feature = "z3")]
fn flip_cmp(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Ge => CmpOp::Le,
        CmpOp::Le => CmpOp::Ge,
        CmpOp::Gt => CmpOp::Lt,
        CmpOp::Lt => CmpOp::Gt,
        other => other,
    }
}

/// Witness synthesis implementation (Z3-gated).
///
/// Creates Z3 integer variables for each Int/struct parameter, asserts branch
/// conditions, and extracts a concrete model when SAT.  Struct params are
/// decomposed into `param__field` variables; the model values are reassembled
/// into `WitnessValue::Struct` records.
#[cfg(feature = "z3")]
fn impl_z3_witness(
    params: &[crate::mvl::parser::ast::Param],
    branch_hyps: &[Expr],
    struct_fields: &HashMap<String, Vec<(String, String)>>,
) -> Option<Vec<crate::mvl::checker::refinements::WitnessArg>> {
    use crate::mvl::checker::refinements::{WitnessArg, WitnessValue};
    use crate::mvl::parser::ast::{BinaryOp, CmpOp as AstCmp, Literal, TypeExpr};
    use z3::{Config, Context, SatResult, Solver};

    let mut cfg = Config::new();
    cfg.set_timeout_msec(1_000);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    // ── Create Z3 variables per parameter ────────────────────────────────────
    //
    // `int_vars`: param_name (or param__field) → Z3 Int
    // `param_kinds`: param_name → "int" | "struct:<TypeName>"
    let mut int_vars: HashMap<String, z3::ast::Int> = HashMap::new();

    for param in params {
        let type_name = match &param.ty {
            TypeExpr::Base { name, .. } => name.as_str(),
            TypeExpr::Refined { inner, .. } => match inner.as_ref() {
                TypeExpr::Base { name, .. } => name.as_str(),
                _ => continue,
            },
            _ => continue,
        };
        match type_name {
            "Int" | "Bool" => {
                let var = z3::ast::Int::new_const(&ctx, param.name.as_str());
                int_vars.insert(param.name.clone(), var);
            }
            other => {
                if let Some(fields) = struct_fields.get(other) {
                    for (field_name, field_type) in fields {
                        if matches!(field_type.as_str(), "Int" | "Bool") {
                            let key = format!("{}__{field_name}", param.name);
                            let var = z3::ast::Int::new_const(&ctx, key.as_str());
                            int_vars.insert(key, var);
                        }
                    }
                }
            }
        }
    }

    if int_vars.is_empty() {
        return None;
    }

    // ── Assert branch hypotheses ──────────────────────────────────────────────
    //
    // Branch hypotheses are MVL `Expr` nodes (e.g. `score > 0` for the then-
    // branch, `!(score > 0)` for the else-branch).  We translate simple
    // comparisons and logical operators; anything unsupported is silently
    // skipped (conservative: we may not find the exact boundary value, but
    // the witness is still valid).
    fn expr_to_z3_bool<'ctx>(
        e: &Expr,
        vars: &HashMap<String, z3::ast::Int<'ctx>>,
        ctx: &'ctx Context,
    ) -> Option<z3::ast::Bool<'ctx>> {
        use z3::ast::Ast;
        match e {
            Expr::Binary {
                op, left, right, ..
            } => {
                // Handle comparison operators.
                let cmp = match op {
                    BinaryOp::Eq => Some(AstCmp::Eq),
                    BinaryOp::Ne => Some(AstCmp::Ne),
                    BinaryOp::Lt => Some(AstCmp::Lt),
                    BinaryOp::Le => Some(AstCmp::Le),
                    BinaryOp::Gt => Some(AstCmp::Gt),
                    BinaryOp::Ge => Some(AstCmp::Ge),
                    BinaryOp::And => {
                        let l = expr_to_z3_bool(left, vars, ctx)?;
                        let r = expr_to_z3_bool(right, vars, ctx)?;
                        return Some(z3::ast::Bool::and(ctx, &[&l, &r]));
                    }
                    BinaryOp::Or => {
                        let l = expr_to_z3_bool(left, vars, ctx)?;
                        let r = expr_to_z3_bool(right, vars, ctx)?;
                        return Some(z3::ast::Bool::or(ctx, &[&l, &r]));
                    }
                    _ => None,
                };
                if let Some(op) = cmp {
                    let l = expr_to_z3_int(left, vars, ctx)?;
                    let r = expr_to_z3_int(right, vars, ctx)?;
                    Some(match op {
                        AstCmp::Eq => l._eq(&r),
                        AstCmp::Ne => l._eq(&r).not(),
                        AstCmp::Lt => l.lt(&r),
                        AstCmp::Le => l.le(&r),
                        AstCmp::Gt => l.gt(&r),
                        AstCmp::Ge => l.ge(&r),
                    })
                } else {
                    None
                }
            }
            Expr::Unary {
                op, expr: inner, ..
            } => {
                use crate::mvl::parser::ast::UnaryOp;
                if matches!(op, UnaryOp::Not) {
                    Some(expr_to_z3_bool(inner, vars, ctx)?.not())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn expr_to_z3_int<'ctx>(
        e: &Expr,
        vars: &HashMap<String, z3::ast::Int<'ctx>>,
        ctx: &'ctx Context,
    ) -> Option<z3::ast::Int<'ctx>> {
        match e {
            Expr::Literal(Literal::Integer(i), _) => Some(z3::ast::Int::from_i64(ctx, *i)),
            Expr::Ident(name, _) => vars.get(name.as_str()).cloned(),
            // `param.field` → look up `param__field` variable.
            Expr::FieldAccess { expr, field, .. } => {
                if let Expr::Ident(obj, _) = expr.as_ref() {
                    vars.get(format!("{obj}__{field}").as_str()).cloned()
                } else {
                    None
                }
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let l = expr_to_z3_int(left, vars, ctx)?;
                let r = expr_to_z3_int(right, vars, ctx)?;
                Some(match op {
                    BinaryOp::Add => z3::ast::Int::add(ctx, &[&l, &r]),
                    BinaryOp::Sub => z3::ast::Int::sub(ctx, &[&l, &r]),
                    BinaryOp::Mul => z3::ast::Int::mul(ctx, &[&l, &r]),
                    BinaryOp::Div => l.div(&r),
                    BinaryOp::Rem => l.modulo(&r),
                    _ => return None,
                })
            }
            _ => None,
        }
    }

    for hyp in branch_hyps {
        if let Some(z3_hyp) = expr_to_z3_bool(hyp, &int_vars, &ctx) {
            solver.assert(&z3_hyp);
        }
    }

    // ── Extract witness ───────────────────────────────────────────────────────
    if solver.check() != SatResult::Sat {
        return None;
    }
    let model = solver.get_model()?;

    let mut witnesses: Vec<WitnessArg> = Vec::new();
    for param in params {
        let type_name = match &param.ty {
            TypeExpr::Base { name, .. } => name.clone(),
            TypeExpr::Refined { inner, .. } => match inner.as_ref() {
                TypeExpr::Base { name, .. } => name.clone(),
                _ => {
                    witnesses.push(WitnessArg {
                        param_name: param.name.clone(),
                        value: WitnessValue::Unknown,
                    });
                    continue;
                }
            },
            _ => {
                witnesses.push(WitnessArg {
                    param_name: param.name.clone(),
                    value: WitnessValue::Unknown,
                });
                continue;
            }
        };
        match type_name.as_str() {
            "Int" | "Bool" => {
                let var = int_vars.get(&param.name)?;
                let val = model
                    .eval(var, true)
                    .and_then(|v| v.as_i64())
                    .map(WitnessValue::Int)
                    .unwrap_or(WitnessValue::Unknown);
                witnesses.push(WitnessArg {
                    param_name: param.name.clone(),
                    value: val,
                });
            }
            other => {
                if let Some(fields) = struct_fields.get(other) {
                    let mut field_witnesses: Vec<(String, WitnessValue)> = Vec::new();
                    for (field_name, field_type) in fields {
                        if matches!(field_type.as_str(), "Int" | "Bool") {
                            let key = format!("{}__{field_name}", param.name);
                            let val = if let Some(var) = int_vars.get(&key) {
                                model
                                    .eval(var, true)
                                    .and_then(|v| v.as_i64())
                                    .map(WitnessValue::Int)
                                    .unwrap_or(WitnessValue::Unknown)
                            } else {
                                WitnessValue::Unknown
                            };
                            field_witnesses.push((field_name.clone(), val));
                        }
                    }
                    witnesses.push(WitnessArg {
                        param_name: param.name.clone(),
                        value: WitnessValue::Struct {
                            type_name: other.to_string(),
                            fields: field_witnesses,
                        },
                    });
                } else {
                    witnesses.push(WitnessArg {
                        param_name: param.name.clone(),
                        value: WitnessValue::Unknown,
                    });
                }
            }
        }
    }

    Some(witnesses)
}

/// Collect all `Len { ident }` identifier names referenced in a `RefExpr`.
///
/// Used to determine which `len_<ident>` integer variables must be created
/// in the Z3 context before asserting the non-negativity axioms.
#[cfg(feature = "z3")]
fn collect_len_idents(expr: &RefExpr, out: &mut Vec<String>) {
    match expr {
        RefExpr::Len { ident, .. } => out.push(ident.clone()),
        RefExpr::LogicOp { left, right, .. }
        | RefExpr::Compare { left, right, .. }
        | RefExpr::ArithOp { left, right, .. }
        | RefExpr::BitwiseOp { left, right, .. } => {
            collect_len_idents(left, out);
            collect_len_idents(right, out);
        }
        RefExpr::Not { inner, .. }
        | RefExpr::Grouped { inner, .. }
        | RefExpr::Old { inner, .. }
        | RefExpr::BitwiseNot { inner, .. } => collect_len_idents(inner, out),
        RefExpr::Forall { body, .. } | RefExpr::Exists { body, .. } => {
            collect_len_idents(body, out);
        }
        RefExpr::FieldAccess { object, .. } => collect_len_idents(object, out),
        _ => {}
    }
}

#[cfg(feature = "z3")]
fn impl_z3(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<RefResult> {
    use crate::mvl::parser::ast::Literal;
    use z3::ast::Ast as _;
    use z3::{Config, Context, SatResult, Solver};

    let mut cfg = Config::new();
    cfg.set_timeout_msec(1_000);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    // Create one Z3 integer constant per in-scope variable.
    let vars: HashMap<String, z3::ast::Int> = var_refs
        .keys()
        .map(|name| (name.clone(), z3::ast::Int::new_const(&ctx, name.as_str())))
        .collect();

    // ── Builtin length axioms (#597) ──────────────────────────────────────────
    //
    // Build `len_vars`: a map from ident name → Z3 integer variable representing
    // the *length* of that ident.  For each such variable we assert `len >= 0`.
    let zero = z3::ast::Int::from_i64(&ctx, 0);

    // Collect all Len-referenced idents from the predicate.
    let mut len_ident_names: Vec<String> = Vec::new();
    collect_len_idents(pred, &mut len_ident_names);

    // Also collect from every hypothesis so that cross-variable length
    // constraints (e.g. `x: String where len(self) > 5`) are asserted.
    for maybe_hyp in var_refs.values().flatten() {
        collect_len_idents(maybe_hyp, &mut len_ident_names);
    }

    // De-duplicate: each ident gets exactly one len variable.
    len_ident_names.sort_unstable();
    len_ident_names.dedup();

    // Create `len_<ident>` Z3 Int constants and assert non-negativity.
    let mut len_vars: HashMap<String, z3::ast::Int> = HashMap::new();
    for ident in &len_ident_names {
        // Skip "self" here; it is resolved per-context below.
        if ident != "self" {
            let len_var = z3::ast::Int::new_const(&ctx, format!("len_{ident}").as_str());
            solver.assert(&len_var.ge(&zero)); // ∀ ident, len(ident) ≥ 0
            len_vars.insert(ident.clone(), len_var);
        }
    }

    // Create `len_self` if the predicate references `len(self)`.
    let pred_uses_len = len_ident_names.iter().any(|s| s == "self");
    let len_self_var = if pred_uses_len {
        let lsv = z3::ast::Int::new_const(&ctx, "len_self");
        solver.assert(&lsv.ge(&zero)); // len(self) ≥ 0
        len_vars.insert("self".to_string(), lsv.clone());
        Some(lsv)
    } else {
        None
    };

    // Assert each variable's refinement hypothesis using per-variable len maps.
    for (var_name, maybe_hyp) in var_refs {
        if let Some(hyp) = maybe_hyp {
            let var = vars.get(var_name)?;
            // Map "self" → len_<var_name> when translating this hypothesis,
            // so that `len(self)` in the hypothesis is the length of var_name.
            let mut hyp_len = len_vars.clone();
            if let Some(lv) = len_vars.get(var_name).cloned() {
                hyp_len.insert("self".to_string(), lv);
            }
            let z3_hyp = ref_to_bool(&ctx, hyp, var, &vars, &hyp_len)?;
            solver.assert(&z3_hyp);
        }
    }

    // ── Translate the call-site argument to a Z3 integer ─────────────────────
    //
    // For string literals when the predicate is Len-typed: the "self" integer
    // IS the length of the string.  Assert `len_self = actual_len`.
    // For all other args: normal integer translation.
    let arg_int: z3::ast::Int = match arg {
        Expr::Literal(Literal::Str(s), _) if len_self_var.is_some() => {
            let actual_len = z3::ast::Int::from_i64(&ctx, s.len() as i64);
            // Constrain len_self to the known byte count.
            solver.assert(&len_self_var.as_ref().unwrap()._eq(&actual_len));
            // Use len_self as the self_term so that `len(self)` in the pred
            // evaluates to the same concrete value.
            len_self_var.clone().unwrap()
        }
        Expr::Ident(var_name, _) if pred_uses_len => {
            // Variable arg: connect pred's len(self) to len_<var_name>.
            // If len_<var_name> exists (from hypothesis scan), assert equality;
            // otherwise leave len_self unconstrained (non-negativity still holds).
            if let Some(lv) = len_vars.get(var_name.as_str()).cloned() {
                if let Some(ls) = &len_self_var {
                    solver.assert(&ls._eq(&lv));
                }
            }
            // self_term for non-Len parts of the predicate: the variable itself.
            expr_to_int(&ctx, arg, &vars)?
        }
        _ => expr_to_int(&ctx, arg, &vars)?,
    };

    // Assert the negation of pred(arg).  Unsat ↔ pred holds for all satisfying
    // assignments ↔ Proven.  Sat ↔ counterexample exists showing pred fails.
    let z3_pred = ref_to_bool(&ctx, pred, &arg_int, &vars, &len_vars)?;
    solver.assert(&z3_pred.not());

    match solver.check() {
        SatResult::Unsat => Some(RefResult::Proven),
        SatResult::Sat => {
            // Z3 found a satisfying assignment for ¬pred — pred fails for some input.
            // Two cases:
            //   - Fully-constrained literal arg: the violation is definite at compile
            //     time → extract the Z3 model witness and return Failed.
            //   - Symbolic/variable arg: the violation is only potential (the caller
            //     might pass a value that satisfies pred) → fall through to RuntimeCheck.
            if is_constrained_literal(arg, pred_uses_len) {
                let model = solver.get_model()?;
                let val = model.eval(&arg_int, true)?;
                let label = if pred_uses_len {
                    if let Expr::Literal(crate::mvl::parser::ast::Literal::Str(_), _) = arg {
                        "len(self)"
                    } else {
                        "self"
                    }
                } else {
                    "self"
                };
                let counterexample = val.as_i64().map(|n| format!("{label}={n}"));
                Some(RefResult::Failed { counterexample })
            } else {
                None
            }
        }
        _ => None,
    }
}

// ── RefExpr → Bool ────────────────────────────────────────────────────────────

/// Translate a `RefExpr` to a Z3 boolean.
///
/// `self_term` is the Z3 integer that the identifier `"self"` maps to in
/// this context (the call-site argument or a hypothesis variable).
///
/// `len_vars` maps ident names to their Z3 integer length variables so that
/// `Len { ident }` nodes translate to the appropriate `len_<ident>` constant.
#[cfg(feature = "z3")]
fn ref_to_bool<'ctx>(
    ctx: &'ctx z3::Context,
    expr: &RefExpr,
    self_term: &z3::ast::Int<'ctx>,
    vars: &HashMap<String, z3::ast::Int<'ctx>>,
    len_vars: &HashMap<String, z3::ast::Int<'ctx>>,
) -> Option<z3::ast::Bool<'ctx>> {
    use crate::mvl::parser::ast::{CmpOp, LogicOp};
    use z3::ast::Ast;

    match expr {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let l = ref_to_int(ctx, left, self_term, vars, len_vars)?;
            let r = ref_to_int(ctx, right, self_term, vars, len_vars)?;
            Some(match op {
                CmpOp::Eq => l._eq(&r),
                CmpOp::Ne => l._eq(&r).not(),
                CmpOp::Lt => l.lt(&r),
                CmpOp::Le => l.le(&r),
                CmpOp::Gt => l.gt(&r),
                CmpOp::Ge => l.ge(&r),
            })
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => {
            let l = ref_to_bool(ctx, left, self_term, vars, len_vars)?;
            let r = ref_to_bool(ctx, right, self_term, vars, len_vars)?;
            Some(match op {
                LogicOp::And => z3::ast::Bool::and(ctx, &[&l, &r]),
                LogicOp::Or => z3::ast::Bool::or(ctx, &[&l, &r]),
            })
        }
        RefExpr::Not { inner, .. } => {
            Some(ref_to_bool(ctx, inner, self_term, vars, len_vars)?.not())
        }
        RefExpr::Grouped { inner, .. } => ref_to_bool(ctx, inner, self_term, vars, len_vars),
        // Quantifiers (Phase 5, #628): translate to Z3 first-order quantifiers.
        // The bound variable is introduced as a fresh Z3 integer constant and
        // added to a local `vars` copy for the duration of the body translation.
        RefExpr::Forall { var, body, .. } => {
            let bound = z3::ast::Int::new_const(ctx, var.as_str());
            let mut inner_vars = vars.clone();
            inner_vars.insert(var.clone(), bound.clone());
            let body_bool = ref_to_bool(ctx, body, self_term, &inner_vars, len_vars)?;
            // forall x: Int, P(x)  ↔  ¬(∃ x: Int, ¬P(x))
            // Z3 universal quantifier via the `forall` builder.
            let bound_ast: &dyn z3::ast::Ast = &bound;
            Some(z3::ast::forall_const(ctx, &[bound_ast], &[], &body_bool))
        }
        RefExpr::Exists { var, body, .. } => {
            let bound = z3::ast::Int::new_const(ctx, var.as_str());
            let mut inner_vars = vars.clone();
            inner_vars.insert(var.clone(), bound.clone());
            let body_bool = ref_to_bool(ctx, body, self_term, &inner_vars, len_vars)?;
            let bound_ast: &dyn z3::ast::Ast = &bound;
            Some(z3::ast::exists_const(ctx, &[bound_ast], &[], &body_bool))
        }
        // Float, Len, and bare Ident/Integer as booleans are not supported.
        _ => None,
    }
}

// ── RefExpr → Int ─────────────────────────────────────────────────────────────

/// Translate a `RefExpr` to a Z3 integer.
///
/// `len_vars` is consulted for `Len { ident }` nodes, returning the
/// pre-created `len_<ident>` Z3 constant (with its non-negativity axiom
/// already asserted).  Returns `None` for unknown idents.
#[cfg(feature = "z3")]
fn ref_to_int<'ctx>(
    ctx: &'ctx z3::Context,
    expr: &RefExpr,
    self_term: &z3::ast::Int<'ctx>,
    vars: &HashMap<String, z3::ast::Int<'ctx>>,
    len_vars: &HashMap<String, z3::ast::Int<'ctx>>,
) -> Option<z3::ast::Int<'ctx>> {
    use crate::mvl::parser::ast::ArithOp;

    match expr {
        RefExpr::Integer { value, .. } => Some(z3::ast::Int::from_i64(ctx, *value)),
        RefExpr::Ident { name, .. } => {
            if name == "self" {
                Some(self_term.clone())
            } else {
                vars.get(name).cloned()
            }
        }
        // `len(ident)` in a predicate — look up the pre-created len variable.
        RefExpr::Len { ident, .. } => len_vars.get(ident.as_str()).cloned(),
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let l = ref_to_int(ctx, left, self_term, vars, len_vars)?;
            let r = ref_to_int(ctx, right, self_term, vars, len_vars)?;
            Some(match op {
                ArithOp::Add => z3::ast::Int::add(ctx, &[&l, &r]),
                ArithOp::Sub => z3::ast::Int::sub(ctx, &[&l, &r]),
                ArithOp::Mul => z3::ast::Int::mul(ctx, &[&l, &r]),
                ArithOp::Div => l.div(&r),
                ArithOp::Rem => l.modulo(&r),
            })
        }
        RefExpr::Grouped { inner, .. } => ref_to_int(ctx, inner, self_term, vars, len_vars),
        // Float is not supported in the integer domain.
        _ => None,
    }
}

// ── Expr → Int ────────────────────────────────────────────────────────────────

/// Translate a call-site `Expr` to a Z3 integer.
///
/// Only integer-typed expressions are handled: integer literals, variable
/// references, and linear arithmetic.  Any unsupported construct returns `None`.
#[cfg(feature = "z3")]
fn expr_to_int<'ctx>(
    ctx: &'ctx z3::Context,
    expr: &Expr,
    vars: &HashMap<String, z3::ast::Int<'ctx>>,
) -> Option<z3::ast::Int<'ctx>> {
    use crate::mvl::parser::ast::{BinaryOp, Literal, UnaryOp};

    match expr {
        Expr::Literal(Literal::Integer(i), _) => Some(z3::ast::Int::from_i64(ctx, *i)),
        Expr::Ident(name, _) => vars.get(name).cloned(),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
            ..
        } => {
            let v = expr_to_int(ctx, inner, vars)?;
            let zero = z3::ast::Int::from_i64(ctx, 0);
            Some(z3::ast::Int::sub(ctx, &[&zero, &v]))
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            let l = expr_to_int(ctx, left, vars)?;
            let r = expr_to_int(ctx, right, vars)?;
            Some(match op {
                BinaryOp::Add => z3::ast::Int::add(ctx, &[&l, &r]),
                BinaryOp::Sub => z3::ast::Int::sub(ctx, &[&l, &r]),
                BinaryOp::Mul => z3::ast::Int::mul(ctx, &[&l, &r]),
                BinaryOp::Div => l.div(&r),
                BinaryOp::Rem => l.modulo(&r),
                _ => return None,
            })
        }
        _ => None,
    }
}

// ── Literal-constraint detection ──────────────────────────────────────────────

/// Returns `true` when `arg` is fully constrained at compile time — i.e. its
/// integer value (or length, for string literals in len-predicate context) is
/// a known constant that requires no runtime information.
///
/// Used by the `SatResult::Sat` arm to decide whether a Z3 counterexample is
/// definite (literal → `Failed`) or only potential (variable → `RuntimeCheck`).
#[cfg(feature = "z3")]
fn is_constrained_literal(arg: &Expr, pred_uses_len: bool) -> bool {
    use crate::mvl::parser::ast::{BinaryOp, Literal, UnaryOp};
    match arg {
        Expr::Literal(Literal::Integer(_), _) => true,
        // A string literal is constrained when the predicate is len-typed: Z3
        // receives the concrete byte-length, so a Sat result is definite.
        Expr::Literal(Literal::Str(_), _) => pred_uses_len,
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
            ..
        } => is_constrained_literal(inner, pred_uses_len),
        Expr::Binary {
            op, left, right, ..
        } => {
            matches!(
                op,
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
            ) && is_constrained_literal(left, pred_uses_len)
                && is_constrained_literal(right, pred_uses_len)
        }
        _ => false,
    }
}

// ── QF-BV implementation (#1928) ──────────────────────────────────────────────

/// Prove `pred(arg)` using Z3's bit-vector theory (QF-BV).
///
/// All integer values are encoded as 64-bit signed bit-vectors.  Triggered
/// only when `has_bitwise_ops(pred)` is true.  Returns `Some(ProvenBv)` on
/// success, or `None` to fall through.
#[cfg(feature = "z3")]
fn impl_z3_bv(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<RefResult> {
    use z3::{Config, Context, SatResult, Solver};

    const BV_WIDTH: u32 = 64;

    let mut cfg = Config::new();
    cfg.set_timeout_msec(1_000);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    // Create one Z3 BV constant per in-scope variable.
    let vars: HashMap<String, z3::ast::BV> = var_refs
        .keys()
        .map(|name| {
            (
                name.clone(),
                z3::ast::BV::new_const(&ctx, name.as_str(), BV_WIDTH),
            )
        })
        .collect();

    // Assert each variable's refinement hypothesis in BV domain.
    for (var_name, maybe_hyp) in var_refs {
        if let Some(hyp) = maybe_hyp {
            let var = vars.get(var_name)?;
            let z3_hyp = bv_pred_to_bool(&ctx, hyp, var, &vars, BV_WIDTH)?;
            solver.assert(&z3_hyp);
        }
    }

    // Translate the call-site argument to a Z3 BV.
    let arg_bv = bv_from_expr(&ctx, arg, &vars, BV_WIDTH)?;

    // Assert ¬pred(arg); unsat ↔ Proven.
    let z3_pred = bv_pred_to_bool(&ctx, pred, &arg_bv, &vars, BV_WIDTH)?;
    solver.assert(&z3_pred.not());

    match solver.check() {
        SatResult::Unsat => Some(RefResult::ProvenBv),
        _ => None,
    }
}

/// Translate a `RefExpr` to a Z3 Bool using bit-vector arithmetic.
#[cfg(feature = "z3")]
fn bv_pred_to_bool<'ctx>(
    ctx: &'ctx z3::Context,
    expr: &RefExpr,
    self_term: &z3::ast::BV<'ctx>,
    vars: &HashMap<String, z3::ast::BV<'ctx>>,
    width: u32,
) -> Option<z3::ast::Bool<'ctx>> {
    use crate::mvl::parser::ast::{CmpOp, LogicOp};
    use z3::ast::Ast;

    match expr {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let l = bv_from_ref(ctx, left, self_term, vars, width)?;
            let r = bv_from_ref(ctx, right, self_term, vars, width)?;
            Some(match op {
                CmpOp::Eq => l._eq(&r),
                CmpOp::Ne => l._eq(&r).not(),
                CmpOp::Lt => l.bvslt(&r),
                CmpOp::Le => l.bvsle(&r),
                CmpOp::Gt => l.bvsgt(&r),
                CmpOp::Ge => l.bvsge(&r),
            })
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => {
            let l = bv_pred_to_bool(ctx, left, self_term, vars, width)?;
            let r = bv_pred_to_bool(ctx, right, self_term, vars, width)?;
            Some(match op {
                LogicOp::And => z3::ast::Bool::and(ctx, &[&l, &r]),
                LogicOp::Or => z3::ast::Bool::or(ctx, &[&l, &r]),
            })
        }
        RefExpr::Not { inner, .. } => {
            Some(bv_pred_to_bool(ctx, inner, self_term, vars, width)?.not())
        }
        RefExpr::Grouped { inner, .. } => bv_pred_to_bool(ctx, inner, self_term, vars, width),
        _ => None,
    }
}

/// Translate a `RefExpr` to a Z3 BV (bit-vector integer).
#[cfg(feature = "z3")]
fn bv_from_ref<'ctx>(
    ctx: &'ctx z3::Context,
    expr: &RefExpr,
    self_term: &z3::ast::BV<'ctx>,
    vars: &HashMap<String, z3::ast::BV<'ctx>>,
    width: u32,
) -> Option<z3::ast::BV<'ctx>> {
    use crate::mvl::parser::ast::{ArithOp, BitwiseOp};

    match expr {
        RefExpr::Integer { value, .. } => Some(z3::ast::BV::from_i64(ctx, *value, width)),
        RefExpr::Ident { name, .. } => {
            if name == "self" {
                Some(self_term.clone())
            } else {
                vars.get(name).cloned()
            }
        }
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let l = bv_from_ref(ctx, left, self_term, vars, width)?;
            let r = bv_from_ref(ctx, right, self_term, vars, width)?;
            Some(match op {
                ArithOp::Add => l.bvadd(&r),
                ArithOp::Sub => l.bvsub(&r),
                ArithOp::Mul => l.bvmul(&r),
                ArithOp::Div => l.bvsdiv(&r),
                ArithOp::Rem => l.bvsrem(&r),
            })
        }
        RefExpr::BitwiseOp {
            op, left, right, ..
        } => {
            let l = bv_from_ref(ctx, left, self_term, vars, width)?;
            let r = bv_from_ref(ctx, right, self_term, vars, width)?;
            Some(match op {
                BitwiseOp::And => l.bvand(&r),
                BitwiseOp::Or => l.bvor(&r),
                BitwiseOp::Xor => l.bvxor(&r),
                BitwiseOp::Shl => l.bvshl(&r),
                BitwiseOp::Shr => l.bvashr(&r),
            })
        }
        RefExpr::BitwiseNot { inner, .. } => {
            Some(bv_from_ref(ctx, inner, self_term, vars, width)?.bvnot())
        }
        RefExpr::Grouped { inner, .. } => bv_from_ref(ctx, inner, self_term, vars, width),
        _ => None,
    }
}

/// Translate a call-site `Expr` to a Z3 BV.
#[cfg(feature = "z3")]
fn bv_from_expr<'ctx>(
    ctx: &'ctx z3::Context,
    expr: &Expr,
    vars: &HashMap<String, z3::ast::BV<'ctx>>,
    width: u32,
) -> Option<z3::ast::BV<'ctx>> {
    use crate::mvl::parser::ast::{BinaryOp, BitwiseOp, Literal, UnaryOp};

    match expr {
        Expr::Literal(Literal::Integer(i), _) => Some(z3::ast::BV::from_i64(ctx, *i, width)),
        Expr::Ident(name, _) => vars.get(name).cloned(),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
            ..
        } => Some(bv_from_expr(ctx, inner, vars, width)?.bvneg()),
        Expr::Unary {
            op: UnaryOp::BitNot,
            expr: inner,
            ..
        } => Some(bv_from_expr(ctx, inner, vars, width)?.bvnot()),
        Expr::Binary {
            op, left, right, ..
        } => {
            let l = bv_from_expr(ctx, left, vars, width)?;
            let r = bv_from_expr(ctx, right, vars, width)?;
            Some(match op {
                BinaryOp::Add => l.bvadd(&r),
                BinaryOp::Sub => l.bvsub(&r),
                BinaryOp::Mul => l.bvmul(&r),
                BinaryOp::Div => l.bvsdiv(&r),
                BinaryOp::Rem => l.bvsrem(&r),
                BinaryOp::BitAnd => l.bvand(&r),
                BinaryOp::BitOr => l.bvor(&r),
                BinaryOp::BitXor => l.bvxor(&r),
                BinaryOp::Shl => l.bvshl(&r),
                BinaryOp::Shr => l.bvashr(&r),
                _ => return None,
            })
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } if args.len() == 1
            && matches!(
                method.as_str(),
                "bit_and" | "bit_or" | "bit_xor" | "shift_left" | "shift_right"
            ) =>
        {
            let l = bv_from_expr(ctx, receiver, vars, width)?;
            let r = bv_from_expr(ctx, &args[0], vars, width)?;
            let bop = match method.as_str() {
                "bit_and" => BitwiseOp::And,
                "bit_or" => BitwiseOp::Or,
                "bit_xor" => BitwiseOp::Xor,
                "shift_left" => BitwiseOp::Shl,
                "shift_right" => BitwiseOp::Shr,
                _ => unreachable!(),
            };
            Some(match bop {
                BitwiseOp::And => l.bvand(&r),
                BitwiseOp::Or => l.bvor(&r),
                BitwiseOp::Xor => l.bvxor(&r),
                BitwiseOp::Shl => l.bvshl(&r),
                BitwiseOp::Shr => l.bvashr(&r),
            })
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } if method == "bit_not" && args.is_empty() => {
            Some(bv_from_expr(ctx, receiver, vars, width)?.bvnot())
        }
        _ => None,
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "z3"))]
mod tests {
    use super::*;
    use crate::mvl::checker::solver::dummy_span;
    use crate::mvl::parser::ast::{CmpOp, Expr, Literal, RefExpr};

    fn int_lit(v: i64) -> Expr {
        Expr::Literal(Literal::Integer(v), dummy_span())
    }

    fn self_gt(n: i64) -> RefExpr {
        RefExpr::Compare {
            op: CmpOp::Gt,
            left: Box::new(RefExpr::Ident {
                name: "self".into(),
                span: dummy_span(),
            }),
            right: Box::new(RefExpr::Integer {
                value: n,
                span: dummy_span(),
            }),
            span: dummy_span(),
        }
    }

    /// y > 5 implies y > 0: proven by Z3.
    #[test]
    fn z3_proves_implication_via_hypothesis() {
        let pred = self_gt(0); // self > 0
        let arg = Expr::Ident("y".into(), dummy_span());
        // var_refs: y has hypothesis self > 5 (i.e., y > 5)
        let mut var_refs = HashMap::new();
        var_refs.insert("y".into(), Some(self_gt(5)));
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::Proven));
    }

    /// Literal 7 satisfies self > 0: proven by Z3.
    #[test]
    fn z3_proves_literal_satisfies_pred() {
        let pred = self_gt(0); // self > 0
        let arg = int_lit(7);
        let var_refs = HashMap::new();
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::Proven));
    }

    /// Literal 0 does NOT satisfy self > 0: Z3 returns Failed with counterexample.
    #[test]
    fn z3_finds_counterexample_for_zero() {
        let pred = self_gt(0); // self > 0
        let arg = int_lit(0);
        let var_refs = HashMap::new();
        // Literal arg → definite violation → Failed with Z3 model witness.
        assert_eq!(
            try_z3(&pred, &arg, &var_refs),
            Some(RefResult::Failed {
                counterexample: Some("self=0".to_string())
            })
        );
    }

    /// y > 5 implies y > 3: proven even when the hypothesis uses a different
    /// bound than the predicate (cross-variable strength test).
    #[test]
    fn z3_proves_stronger_hypothesis_implies_weaker_pred() {
        let pred = self_gt(3); // self > 3
        let arg = Expr::Ident("y".into(), dummy_span());
        let mut var_refs = HashMap::new();
        var_refs.insert("y".into(), Some(self_gt(5)));
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::Proven));
    }

    /// Variable without refinement does not prove self > 0.
    #[test]
    fn z3_no_hypothesis_cannot_prove() {
        let pred = self_gt(0); // self > 0
        let arg = Expr::Ident("x".into(), dummy_span());
        // x has no refinement predicate
        let mut var_refs = HashMap::new();
        var_refs.insert("x".into(), None::<RefExpr>);
        assert_eq!(try_z3(&pred, &arg, &var_refs), None);
    }

    /// Two-variable case: x > 10 and y > x implies y > 5.
    #[test]
    fn z3_proves_two_variable_chain() {
        use crate::mvl::parser::ast::LogicOp;
        // pred: self > 5
        let pred = self_gt(5);
        let arg = Expr::Ident("y".into(), dummy_span());
        // var_refs: x has hypothesis self > 10; y has hypothesis self > x
        let x_gt_10 = self_gt(10); // x > 10
        let y_gt_x = RefExpr::Compare {
            op: CmpOp::Gt,
            left: Box::new(RefExpr::Ident {
                name: "self".into(),
                span: dummy_span(),
            }),
            right: Box::new(RefExpr::Ident {
                name: "x".into(),
                span: dummy_span(),
            }),
            span: dummy_span(),
        }; // y > x
        let mut var_refs = HashMap::new();
        var_refs.insert("x".into(), Some(x_gt_10));
        var_refs.insert("y".into(), Some(y_gt_x));
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::Proven));
        let _ = LogicOp::And; // suppress unused import warning
    }

    // ── Builtin length axiom tests (#597) ────────────────────────────────────

    fn len_self_lt(n: i64) -> RefExpr {
        RefExpr::Compare {
            op: CmpOp::Lt,
            left: Box::new(RefExpr::Len {
                ident: "self".into(),
                span: dummy_span(),
            }),
            right: Box::new(RefExpr::Integer {
                value: n,
                span: dummy_span(),
            }),
            span: dummy_span(),
        }
    }

    fn len_self_ge(n: i64) -> RefExpr {
        RefExpr::Compare {
            op: CmpOp::Ge,
            left: Box::new(RefExpr::Len {
                ident: "self".into(),
                span: dummy_span(),
            }),
            right: Box::new(RefExpr::Integer {
                value: n,
                span: dummy_span(),
            }),
            span: dummy_span(),
        }
    }

    /// String literal "hello" (length 5) satisfies `len(self) < 256`.
    #[test]
    fn z3_axiom_string_literal_len_lt_bound() {
        let pred = len_self_lt(256);
        let arg = Expr::Literal(Literal::Str("hello".into()), dummy_span());
        let var_refs = HashMap::new();
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::Proven));
    }

    /// Empty string satisfies `len(self) >= 0` (non-negativity axiom).
    #[test]
    fn z3_axiom_string_literal_len_nonneg() {
        let pred = len_self_ge(0);
        let arg = Expr::Literal(Literal::Str("".into()), dummy_span());
        let var_refs = HashMap::new();
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::Proven));
    }

    /// String literal "hello" does NOT satisfy `len(self) < 3` — Z3 returns Failed.
    #[test]
    fn z3_axiom_string_literal_len_too_long_returns_failed() {
        let pred = len_self_lt(3);
        let arg = Expr::Literal(Literal::Str("hello".into()), dummy_span());
        let var_refs = HashMap::new();
        // String literal in len-predicate context is constrained: len("hello")=5 → definite
        // violation → Failed with Z3 model witness.
        assert_eq!(
            try_z3(&pred, &arg, &var_refs),
            Some(RefResult::Failed {
                counterexample: Some("len(self)=5".to_string())
            })
        );
    }

    /// Variable `s` with hypothesis `len(self) > 10` satisfies `len(self) >= 0`.
    #[test]
    fn z3_axiom_variable_len_hypothesis_implies_nonneg() {
        let pred = len_self_ge(0); // len(self) >= 0
        let arg = Expr::Ident("s".into(), dummy_span());
        // s has hypothesis len(self) > 10, so len_s > 10 → len_s >= 0 trivially.
        let mut var_refs = HashMap::new();
        var_refs.insert(
            "s".into(),
            Some(RefExpr::Compare {
                op: CmpOp::Gt,
                left: Box::new(RefExpr::Len {
                    ident: "self".into(),
                    span: dummy_span(),
                }),
                right: Box::new(RefExpr::Integer {
                    value: 10,
                    span: dummy_span(),
                }),
                span: dummy_span(),
            }),
        );
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::Proven));
    }

    /// Variable `s` with hypothesis `len(self) > 5` satisfies `len(self) > 3`.
    #[test]
    fn z3_axiom_variable_len_stronger_hypothesis_implies_weaker_pred() {
        let pred = RefExpr::Compare {
            op: CmpOp::Gt,
            left: Box::new(RefExpr::Len {
                ident: "self".into(),
                span: dummy_span(),
            }),
            right: Box::new(RefExpr::Integer {
                value: 3,
                span: dummy_span(),
            }),
            span: dummy_span(),
        };
        let arg = Expr::Ident("s".into(), dummy_span());
        let mut var_refs = HashMap::new();
        var_refs.insert(
            "s".into(),
            Some(RefExpr::Compare {
                op: CmpOp::Gt,
                left: Box::new(RefExpr::Len {
                    ident: "self".into(),
                    span: dummy_span(),
                }),
                right: Box::new(RefExpr::Integer {
                    value: 5,
                    span: dummy_span(),
                }),
                span: dummy_span(),
            }),
        );
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::Proven));
    }

    // ── QF-BV tests (#1928) ──────────────────────────────────────────────────

    /// Helper: `(self.bit_and(mask)) == self` as a RefExpr.
    fn self_bit_and_eq(mask: i64) -> RefExpr {
        RefExpr::Compare {
            op: CmpOp::Eq,
            left: Box::new(RefExpr::BitwiseOp {
                op: crate::mvl::parser::ast::BitwiseOp::And,
                left: Box::new(RefExpr::Ident {
                    name: "self".into(),
                    span: dummy_span(),
                }),
                right: Box::new(RefExpr::Integer {
                    value: mask,
                    span: dummy_span(),
                }),
                span: dummy_span(),
            }),
            right: Box::new(RefExpr::Ident {
                name: "self".into(),
                span: dummy_span(),
            }),
            span: dummy_span(),
        }
    }

    /// 4 & 15 == 4: Z3 QF-BV proves a nibble-range constraint.
    #[test]
    fn z3_bv_literal_satisfies_nibble_pred() {
        let pred = self_bit_and_eq(15); // (self & 15) == self
        let arg = int_lit(4);
        let var_refs = HashMap::new();
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::ProvenBv));
    }

    /// 16 does NOT satisfy (self & 15) == self — 16 & 15 = 0 ≠ 16.
    #[test]
    fn z3_bv_literal_violates_nibble_pred() {
        let pred = self_bit_and_eq(15);
        let arg = int_lit(16);
        let var_refs = HashMap::new();
        // 16 is a literal → definite failure but impl_z3_bv returns None on Sat
        // (no counterexample extraction in BV path — falls through to runtime).
        assert_eq!(try_z3(&pred, &arg, &var_refs), None);
    }

    /// Variable y with hypothesis (y & 15) == y satisfies (self & 15) == self.
    #[test]
    fn z3_bv_hypothesis_implies_nibble_pred() {
        let pred = self_bit_and_eq(15); // (self & 15) == self
        let arg = Expr::Ident("y".into(), dummy_span());
        let mut var_refs = HashMap::new();
        var_refs.insert("y".into(), Some(self_bit_and_eq(15)));
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::ProvenBv));
    }

    /// 255 & 255 == 255: full byte mask proves trivially.
    #[test]
    fn z3_bv_byte_mask_satisfied() {
        let pred = self_bit_and_eq(255); // (self & 255) == self
        let arg = int_lit(128);
        let var_refs = HashMap::new();
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::ProvenBv));
    }

    /// Shift: (self << 0) == self is trivially true.
    #[test]
    fn z3_bv_shift_zero_identity() {
        let pred = RefExpr::Compare {
            op: CmpOp::Eq,
            left: Box::new(RefExpr::BitwiseOp {
                op: crate::mvl::parser::ast::BitwiseOp::Shl,
                left: Box::new(RefExpr::Ident {
                    name: "self".into(),
                    span: dummy_span(),
                }),
                right: Box::new(RefExpr::Integer {
                    value: 0,
                    span: dummy_span(),
                }),
                span: dummy_span(),
            }),
            right: Box::new(RefExpr::Ident {
                name: "self".into(),
                span: dummy_span(),
            }),
            span: dummy_span(),
        };
        let arg = int_lit(42);
        let var_refs = HashMap::new();
        assert_eq!(try_z3(&pred, &arg, &var_refs), Some(RefResult::ProvenBv));
    }

    /// Test that mimics the full pipeline: atom-normalize then try_z3.
    #[test]
    fn z3_bv_pipeline_after_atom_norm() {
        use crate::mvl::checker::solver::atom_norm::AtomNormalizer;
        use crate::mvl::parser::ast::BitwiseOp as Bop;

        let pred = RefExpr::Compare {
            op: CmpOp::Eq,
            left: Box::new(RefExpr::BitwiseOp {
                op: Bop::And,
                left: Box::new(RefExpr::Ident {
                    name: "self".into(),
                    span: dummy_span(),
                }),
                right: Box::new(RefExpr::Integer {
                    value: 15,
                    span: dummy_span(),
                }),
                span: dummy_span(),
            }),
            right: Box::new(RefExpr::Ident {
                name: "self".into(),
                span: dummy_span(),
            }),
            span: dummy_span(),
        };
        let arg = int_lit(4);
        let var_refs = HashMap::new();

        let mut norm = AtomNormalizer::new();
        let n_pred = norm.rewrite_refexpr(&pred);
        let n_arg = norm.rewrite_expr(&arg);
        let n_var_refs = norm.rewrite_var_refs(&var_refs);

        assert_eq!(
            try_z3(&n_pred, &n_arg, &n_var_refs),
            Some(RefResult::ProvenBv)
        );
    }
}
