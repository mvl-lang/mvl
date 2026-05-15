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
//! Integer constants, free variables, `+`, `-`, `*`, `div`, `rem`, and all
//! six comparison operators, `&&`, `||`, `!`.  Float predicates, `Len` nodes,
//! and non-linear multiplication of two unknowns are translated to `None` so
//! the layer safely falls through.
//!
//! Compile-time gated: the entire implementation is `#[cfg(feature = "z3")]`.
//! When the feature is absent, `try_z3` is a no-op returning `None`.

use std::collections::HashMap;

use crate::mvl::parser::ast::{Expr, RefExpr};

use super::RefResult;

// ── Public entry point ────────────────────────────────────────────────────────

/// Try to prove `pred(arg)` using the Z3 SMT solver.
///
/// Returns `Some(Proven)` when Z3 confirms the implication is valid.
/// Returns `None` for unsupported constructs or when Z3 cannot decide within
/// the built-in 1 second timeout.
pub(crate) fn try_z3(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<RefResult> {
    #[cfg(feature = "z3")]
    return impl_z3(pred, arg, var_refs);
    #[cfg(not(feature = "z3"))]
    {
        let _ = (pred, arg, var_refs);
        None
    }
}

// ── Z3 implementation (feature-gated) ────────────────────────────────────────

#[cfg(feature = "z3")]
fn impl_z3(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<RefResult> {
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

    // Assert each variable's refinement hypothesis.
    for (var_name, maybe_hyp) in var_refs {
        if let Some(hyp) = maybe_hyp {
            let var = vars.get(var_name)?;
            let z3_hyp = ref_to_bool(&ctx, hyp, var, &vars)?;
            solver.assert(&z3_hyp);
        }
    }

    // Translate the call-site argument to a Z3 integer expression.
    let arg_int = expr_to_int(&ctx, arg, &vars)?;

    // Assert the negation of pred(arg).  Unsat ↔ pred holds for all satisfying
    // assignments ↔ Proven.  Sat ↔ counterexample exists showing pred fails.
    let z3_pred = ref_to_bool(&ctx, pred, &arg_int, &vars)?;
    solver.assert(&z3_pred.not());

    match solver.check() {
        SatResult::Unsat => Some(RefResult::Proven),
        SatResult::Sat => {
            // Z3 found a satisfying assignment for ¬pred, meaning pred can fail for some
            // input.  This may be a definite violation (constrained literal arg) or a
            // potential violation (unconstrained variable arg).  We return None here so
            // the solver cascade falls through to RuntimeCheck rather than Failed.
            //
            // TODO(#627): In Phase 4, split this into:
            //   - Sat on a fully-constrained arg → Failed { counterexample: Some(...) }
            //   - Sat on a symbolic arg          → None (RuntimeCheck, deferred)
            // Use solver.get_model() to extract the witness at that point.
            None
        }
        _ => None,
    }
}

// ── RefExpr → Bool ────────────────────────────────────────────────────────────

/// Translate a `RefExpr` to a Z3 boolean.
///
/// `self_term` is the Z3 integer that the identifier `"self"` maps to in
/// this context (the call-site argument or a hypothesis variable).
#[cfg(feature = "z3")]
fn ref_to_bool<'ctx>(
    ctx: &'ctx z3::Context,
    expr: &RefExpr,
    self_term: &z3::ast::Int<'ctx>,
    vars: &HashMap<String, z3::ast::Int<'ctx>>,
) -> Option<z3::ast::Bool<'ctx>> {
    use crate::mvl::parser::ast::{CmpOp, LogicOp};
    use z3::ast::Ast;

    match expr {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let l = ref_to_int(ctx, left, self_term, vars)?;
            let r = ref_to_int(ctx, right, self_term, vars)?;
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
            let l = ref_to_bool(ctx, left, self_term, vars)?;
            let r = ref_to_bool(ctx, right, self_term, vars)?;
            Some(match op {
                LogicOp::And => z3::ast::Bool::and(ctx, &[&l, &r]),
                LogicOp::Or => z3::ast::Bool::or(ctx, &[&l, &r]),
            })
        }
        RefExpr::Not { inner, .. } => Some(ref_to_bool(ctx, inner, self_term, vars)?.not()),
        RefExpr::Grouped { inner, .. } => ref_to_bool(ctx, inner, self_term, vars),
        // Quantifiers (Phase 5, #628): translate to Z3 first-order quantifiers.
        // The bound variable is introduced as a fresh Z3 integer constant and
        // added to a local `vars` copy for the duration of the body translation.
        RefExpr::Forall { var, body, .. } => {
            let bound = z3::ast::Int::new_const(ctx, var.as_str());
            let mut inner_vars = vars.clone();
            inner_vars.insert(var.clone(), bound.clone());
            let body_bool = ref_to_bool(ctx, body, self_term, &inner_vars)?;
            // forall x: Int, P(x)  ↔  ¬(∃ x: Int, ¬P(x))
            // Z3 universal quantifier via the `forall` builder.
            let bound_ast: &dyn z3::ast::Ast = &bound;
            Some(z3::ast::forall_const(ctx, &[bound_ast], &[], &body_bool))
        }
        RefExpr::Exists { var, body, .. } => {
            let bound = z3::ast::Int::new_const(ctx, var.as_str());
            let mut inner_vars = vars.clone();
            inner_vars.insert(var.clone(), bound.clone());
            let body_bool = ref_to_bool(ctx, body, self_term, &inner_vars)?;
            let bound_ast: &dyn z3::ast::Ast = &bound;
            Some(z3::ast::exists_const(ctx, &[bound_ast], &[], &body_bool))
        }
        // Float, Len, and bare Ident/Integer as booleans are not supported.
        _ => None,
    }
}

// ── RefExpr → Int ─────────────────────────────────────────────────────────────

/// Translate a `RefExpr` to a Z3 integer.
#[cfg(feature = "z3")]
fn ref_to_int<'ctx>(
    ctx: &'ctx z3::Context,
    expr: &RefExpr,
    self_term: &z3::ast::Int<'ctx>,
    vars: &HashMap<String, z3::ast::Int<'ctx>>,
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
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let l = ref_to_int(ctx, left, self_term, vars)?;
            let r = ref_to_int(ctx, right, self_term, vars)?;
            Some(match op {
                ArithOp::Add => z3::ast::Int::add(ctx, &[&l, &r]),
                ArithOp::Sub => z3::ast::Int::sub(ctx, &[&l, &r]),
                ArithOp::Mul => z3::ast::Int::mul(ctx, &[&l, &r]),
                ArithOp::Div => l.div(&r),
                ArithOp::Rem => l.modulo(&r),
            })
        }
        RefExpr::Grouped { inner, .. } => ref_to_int(ctx, inner, self_term, vars),
        // Float and Len are not supported in the integer domain.
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

    /// Literal 0 does NOT satisfy self > 0: Z3 finds a counterexample (Sat).
    #[test]
    fn z3_finds_counterexample_for_zero() {
        let pred = self_gt(0); // self > 0
        let arg = int_lit(0);
        let var_refs = HashMap::new();
        // Z3 returns Sat (not UNSAT), so try_z3 returns None (cannot prove, RuntimeCheck).
        assert_eq!(try_z3(&pred, &arg, &var_refs), None);
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
}
