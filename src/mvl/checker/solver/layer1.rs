// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Layer 1 — trivial pattern matching for refinement predicates.
//!
//! Handles the simplest ~40% of refinement proofs with O(1) complexity:
//!
//! | Pattern               | Example                                 |
//! |-----------------------|-----------------------------------------|
//! | Literal argument      | `positive(42)` where pred is `self > 0` |
//! | Subsumption           | `bar(x)` when `x` carries `x > 0` too  |
//! | Tautology predicate   | `self == self` → proven for any arg     |
//! | Contradiction pred.   | `self > 0 && self < 0` → fails always   |
//! | Constant folding      | `bar(abs(-3))` via pure-fn evaluation   |

use std::collections::HashMap;

use crate::mvl::checker::const_eval;
use crate::mvl::parser::ast::{ArithOp, CmpOp, Expr, FnDecl, Literal, LogicOp, RefExpr, UnaryOp};

use super::RefResult;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Try to prove or refute `pred` for `arg` using trivial-pattern rules.
///
/// Returns `None` when Layer 1 cannot make a determination; the caller should
/// escalate to a deeper layer or fall back to `RuntimeCheck`.
pub(super) fn try_trivial(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
) -> Option<RefResult> {
    // ── Predicate-level analysis (argument-independent) ───────────────────
    if is_tautology(pred) {
        return Some(RefResult::Proven);
    }
    if is_contradiction(pred) {
        return Some(RefResult::Failed {
            counterexample: None,
        });
    }

    // ── Argument-level analysis ───────────────────────────────────────────
    match arg {
        // Integer literal: evaluate the predicate at that value.
        Expr::Literal(Literal::Integer(n), _) => Some(eval_pred_int(*n, pred)),

        // Float literal: evaluate in the float domain.
        Expr::Literal(Literal::Float(f), _) => Some(eval_pred_float(*f, pred)),

        // Unary negation of a literal.
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
            ..
        } => match inner.as_ref() {
            Expr::Literal(Literal::Integer(n), _) => {
                n.checked_neg().map(|neg| eval_pred_int(neg, pred))
            }
            Expr::Literal(Literal::Float(f), _) => Some(eval_pred_float(-f, pred)),
            _ => None,
        },

        // Variable: subsumption check or concrete hypothesis evaluation.
        Expr::Ident(name, _) => match var_refs.get(name.as_str()) {
            Some(Some(arg_pred)) => {
                if preds_equivalent(arg_pred, pred) {
                    Some(RefResult::Proven)
                } else if let Some(n) = extract_eq_int_from_hyp(arg_pred) {
                    Some(eval_pred_int(n, pred))
                } else if let Some(f) = extract_eq_float_from_hyp(arg_pred) {
                    if f.is_nan() {
                        None
                    } else {
                        Some(eval_pred_float(f, pred))
                    }
                } else {
                    None
                }
            }
            _ => None,
        },

        // Pure function call with all-literal arguments: constant-fold then check.
        Expr::FnCall { name, args, .. } => fn_decls.get(name.as_str()).and_then(|fd| {
            const_eval::try_fold_call(fd, args, fn_decls).and_then(|cv| match cv {
                const_eval::ConstValue::Integer(n) => Some(eval_pred_int(n, pred)),
                const_eval::ConstValue::Float(f) if !f.is_nan() => Some(eval_pred_float(f, pred)),
                _ => None,
            })
        }),

        // String literal: evaluate `len(ident)` predicates against the literal's length.
        // Enables static proof of e.g. `validate_log_path("app.log")` where pred is `len(p) > 0`.
        Expr::Literal(Literal::Str(s), _) => Some(eval_pred_str_len(s.len() as i64, pred)),

        // Struct literal as return value (#1540): resolve `self.field` against the
        // construct's field exprs. Enables `ensures result.field == literal` to be
        // proven statically when the function body is a struct literal.
        Expr::Construct { fields, .. } => Some(eval_pred_struct(fields, pred)),

        // Everything else: Layer 1 cannot decide.
        _ => None,
    }
}

// ── Predicate-level trivial analysis ─────────────────────────────────────────

/// Returns `true` when `pred` is trivially satisfied by every value of `self`.
///
/// Detected patterns:
/// - `self == self`, `self <= self`, `self >= self` (reflexive comparisons)
/// - `A || B` where either arm is a tautology
/// - `!A` where `A` is a contradiction
/// - Grouping is transparent
fn is_tautology(pred: &RefExpr) -> bool {
    match pred {
        // Reflexive: self == self, self <= self, self >= self are always true.
        RefExpr::Compare {
            op: CmpOp::Eq | CmpOp::Le | CmpOp::Ge,
            left,
            right,
            ..
        } => preds_equivalent(left, right),
        RefExpr::Compare { .. } => false,
        RefExpr::LogicOp {
            op: LogicOp::Or,
            left,
            right,
            ..
        } => is_tautology(left) || is_tautology(right),
        RefExpr::Not { inner, .. } => is_contradiction(inner),
        RefExpr::Grouped { inner, .. } => is_tautology(inner),
        _ => false,
    }
}

/// Returns `true` when `pred` cannot be satisfied by any value of `self`.
///
/// Detected patterns:
/// - `self != self`, `self < self`, `self > self` (impossible self-comparisons)
/// - `A && !A` / `!A && A` (syntactic negation)
/// - Integer-interval emptiness: `self > N && self < M` where `N ≥ M`
/// - `!A` where `A` is a tautology
/// - Grouping is transparent
fn is_contradiction(pred: &RefExpr) -> bool {
    match pred {
        // Irreflexive on self: self < self, self > self, self != self are always false.
        RefExpr::Compare {
            op: CmpOp::Ne | CmpOp::Lt | CmpOp::Gt,
            left,
            right,
            ..
        } => preds_equivalent(left, right),
        RefExpr::Compare { .. } => false,
        RefExpr::LogicOp {
            op: LogicOp::And,
            left,
            right,
            ..
        } => {
            // A && !A
            if let RefExpr::Not { inner, .. } = right.as_ref() {
                if preds_equivalent(left, inner) {
                    return true;
                }
            }
            // !A && A
            if let RefExpr::Not { inner, .. } = left.as_ref() {
                if preds_equivalent(inner, right) {
                    return true;
                }
            }
            // Integer-interval emptiness: self > N && self < M where N >= M.
            let a = extract_self_int_bound(left);
            let b = extract_self_int_bound(right);
            if let (Some(a), Some(b)) = (a, b) {
                if bounds_contradictory(a, b) || bounds_contradictory(b, a) {
                    return true;
                }
            }
            false
        }
        RefExpr::Not { inner, .. } => is_tautology(inner),
        RefExpr::Grouped { inner, .. } => is_contradiction(inner),
        _ => false,
    }
}

/// Extract a bound of the form `self <op> <integer>` from a simple comparison,
/// normalising so the result is always from `self`'s perspective.
///
/// `5 > self` is returned as `(CmpOp::Lt, 5)` (i.e. `self < 5`).
fn extract_self_int_bound(pred: &RefExpr) -> Option<(CmpOp, i64)> {
    match pred {
        RefExpr::Compare {
            op, left, right, ..
        } => match (left.as_ref(), right.as_ref()) {
            (RefExpr::Ident { name, .. }, RefExpr::Integer { value, .. }) if is_self_like(name) => {
                Some((*op, *value))
            }
            (RefExpr::Integer { value, .. }, RefExpr::Ident { name, .. }) if is_self_like(name) => {
                // Flip: `N op self` → `self flip(op) N`
                Some((op.flip(), *value))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Returns `true` when `a` is a lower bound and `b` is an upper bound whose
/// combination makes the integer interval empty.
///
/// | `a` op | `b` op | Empty when |
/// |--------|--------|------------|
/// | `>`    | `<`    | b ≤ a      |
/// | `>`    | `<=`   | b ≤ a      |
/// | `>=`   | `<`    | b ≤ a      |
/// | `>=`   | `<=`   | b < a      |
fn bounds_contradictory((op_a, v_a): (CmpOp, i64), (op_b, v_b): (CmpOp, i64)) -> bool {
    match (op_a, op_b) {
        (CmpOp::Gt, CmpOp::Lt) | (CmpOp::Gt, CmpOp::Le) | (CmpOp::Ge, CmpOp::Lt) => v_b <= v_a,
        (CmpOp::Ge, CmpOp::Le) => v_b < v_a,
        _ => false,
    }
}

// ── Predicate evaluation for literal values ───────────────────────────────────

/// Evaluate a predicate against an integer literal.
///
/// Returns `Proven`, `Failed`, or `RuntimeCheck` (when the predicate contains
/// nodes that cannot be evaluated in the integer domain, e.g. `Len`).
pub(super) fn eval_pred_int(self_val: i64, pred: &RefExpr) -> RefResult {
    match eval_bool_int(self_val, pred) {
        Some(true) => RefResult::Proven,
        Some(false) => RefResult::Failed {
            counterexample: None,
        },
        None => RefResult::RuntimeCheck,
    }
}

/// Evaluate a predicate against a float literal.
pub(super) fn eval_pred_float(self_val: f64, pred: &RefExpr) -> RefResult {
    match eval_bool_float(self_val, pred) {
        Some(true) => RefResult::Proven,
        Some(false) => RefResult::Failed {
            counterexample: None,
        },
        None => RefResult::RuntimeCheck,
    }
}

/// Evaluate a predicate against a string literal, treating `len(ident)` as the
/// string's character count.
///
/// Enables static proof of refinements like `len(p) > 0` when the argument is
/// a string literal such as `"app.log"` (len = 7).
pub(super) fn eval_pred_str_len(len_val: i64, pred: &RefExpr) -> RefResult {
    match eval_bool_str_len(len_val, pred) {
        Some(true) => RefResult::Proven,
        Some(false) => RefResult::Failed {
            counterexample: None,
        },
        None => RefResult::RuntimeCheck,
    }
}

/// Evaluate a boolean predicate with `len(x) = len_val` for any identifier `x`.
fn eval_bool_str_len(len_val: i64, pred: &RefExpr) -> Option<bool> {
    match pred {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let l = eval_num_str_len(len_val, left)?;
            let r = eval_num_str_len(len_val, right)?;
            Some(match op {
                CmpOp::Eq => l == r,
                CmpOp::Ne => l != r,
                CmpOp::Lt => l < r,
                CmpOp::Gt => l > r,
                CmpOp::Le => l <= r,
                CmpOp::Ge => l >= r,
            })
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => match op {
            LogicOp::And => {
                let l = eval_bool_str_len(len_val, left);
                if l == Some(false) {
                    return Some(false);
                }
                let r = eval_bool_str_len(len_val, right);
                match (l, r) {
                    (Some(a), Some(b)) => Some(a && b),
                    _ => None,
                }
            }
            LogicOp::Or => {
                let l = eval_bool_str_len(len_val, left);
                if l == Some(true) {
                    return Some(true);
                }
                let r = eval_bool_str_len(len_val, right);
                match (l, r) {
                    (Some(a), Some(b)) => Some(a || b),
                    _ => None,
                }
            }
        },
        RefExpr::Not { inner, .. } => Some(!eval_bool_str_len(len_val, inner)?),
        RefExpr::Grouped { inner, .. } => eval_bool_str_len(len_val, inner),
        _ => None,
    }
}

/// Evaluate a numeric sub-expression where `len(x)` (for any identifier `x`)
/// is treated as `len_val`.
fn eval_num_str_len(len_val: i64, expr: &RefExpr) -> Option<i64> {
    match expr {
        // Any `len(ident)` node evaluates to the string's length.
        RefExpr::Len { .. } => Some(len_val),
        RefExpr::Integer { value, .. } => Some(*value),
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let l = eval_num_str_len(len_val, left)?;
            let r = eval_num_str_len(len_val, right)?;
            match op {
                ArithOp::Add => l.checked_add(r),
                ArithOp::Sub => l.checked_sub(r),
                ArithOp::Mul => l.checked_mul(r),
                ArithOp::Div => {
                    if r == 0 {
                        None
                    } else {
                        Some(l / r)
                    }
                }
                ArithOp::Rem => {
                    if r == 0 {
                        None
                    } else {
                        Some(l % r)
                    }
                }
            }
        }
        RefExpr::Grouped { inner, .. } => eval_num_str_len(len_val, inner),
        _ => None,
    }
}

/// Evaluate a boolean predicate with `self = self_val` in the integer domain.
///
/// Returns `None` when a sub-expression cannot be evaluated (conservative
/// fallback to `RuntimeCheck`).  Short-circuits `And`/`Or`.
fn eval_bool_int(self_val: i64, pred: &RefExpr) -> Option<bool> {
    match pred {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let l = eval_num_int(self_val, left)?;
            let r = eval_num_int(self_val, right)?;
            Some(match op {
                CmpOp::Eq => l == r,
                CmpOp::Ne => l != r,
                CmpOp::Lt => l < r,
                CmpOp::Gt => l > r,
                CmpOp::Le => l <= r,
                CmpOp::Ge => l >= r,
            })
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => match op {
            LogicOp::And => {
                let l = eval_bool_int(self_val, left);
                if l == Some(false) {
                    return Some(false);
                }
                let r = eval_bool_int(self_val, right);
                match (l, r) {
                    (Some(a), Some(b)) => Some(a && b),
                    _ => None,
                }
            }
            LogicOp::Or => {
                let l = eval_bool_int(self_val, left);
                if l == Some(true) {
                    return Some(true);
                }
                let r = eval_bool_int(self_val, right);
                match (l, r) {
                    (Some(a), Some(b)) => Some(a || b),
                    _ => None,
                }
            }
        },
        RefExpr::Not { inner, .. } => Some(!eval_bool_int(self_val, inner)?),
        RefExpr::Grouped { inner, .. } => eval_bool_int(self_val, inner),
        _ => None,
    }
}

/// Evaluate a numeric sub-expression with `self = self_val` in the integer domain.
///
/// Returns `None` for nodes not representable as `i64` (e.g. `Float`, `Len`),
/// causing the caller to fall back to `RuntimeCheck`.
fn eval_num_int(self_val: i64, expr: &RefExpr) -> Option<i64> {
    match expr {
        RefExpr::Ident { name, .. } if name == "self" => Some(self_val),
        RefExpr::Integer { value, .. } => Some(*value),
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let l = eval_num_int(self_val, left)?;
            let r = eval_num_int(self_val, right)?;
            match op {
                ArithOp::Add => l.checked_add(r),
                ArithOp::Sub => l.checked_sub(r),
                ArithOp::Mul => l.checked_mul(r),
                ArithOp::Div => {
                    if r == 0 {
                        None
                    } else {
                        Some(l / r)
                    }
                }
                ArithOp::Rem => {
                    if r == 0 {
                        None
                    } else {
                        Some(l % r)
                    }
                }
            }
        }
        RefExpr::Grouped { inner, .. } => eval_num_int(self_val, inner),
        // Float literals and Len are not in the integer domain.
        _ => None,
    }
}

/// Evaluate a boolean predicate with `self = self_val` in the float domain.
///
/// Short-circuits `And`/`Or` when one branch is definitively `false`/`true`.
fn eval_bool_float(self_val: f64, pred: &RefExpr) -> Option<bool> {
    match pred {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let l = eval_num_float(self_val, left)?;
            let r = eval_num_float(self_val, right)?;
            Some(match op {
                CmpOp::Eq => l == r,
                CmpOp::Ne => l != r,
                CmpOp::Lt => l < r,
                CmpOp::Gt => l > r,
                CmpOp::Le => l <= r,
                CmpOp::Ge => l >= r,
            })
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => match op {
            LogicOp::And => {
                let l = eval_bool_float(self_val, left);
                if l == Some(false) {
                    return Some(false);
                }
                let r = eval_bool_float(self_val, right);
                match (l, r) {
                    (Some(a), Some(b)) => Some(a && b),
                    _ => None,
                }
            }
            LogicOp::Or => {
                let l = eval_bool_float(self_val, left);
                if l == Some(true) {
                    return Some(true);
                }
                let r = eval_bool_float(self_val, right);
                match (l, r) {
                    (Some(a), Some(b)) => Some(a || b),
                    _ => None,
                }
            }
        },
        RefExpr::Not { inner, .. } => Some(!eval_bool_float(self_val, inner)?),
        RefExpr::Grouped { inner, .. } => eval_bool_float(self_val, inner),
        _ => None,
    }
}

fn eval_num_float(self_val: f64, expr: &RefExpr) -> Option<f64> {
    match expr {
        RefExpr::Ident { name, .. } if name == "self" => Some(self_val),
        RefExpr::Integer { value, .. } => {
            // i64 values above 2^53 cannot be exactly represented in f64;
            // fall back to RuntimeCheck rather than silently losing precision.
            if value.unsigned_abs() > (1u64 << 53) {
                return None;
            }
            Some(*value as f64)
        }
        RefExpr::Float { value, .. } => {
            // NaN literals have no useful ordering; fall back to RuntimeCheck.
            if value.is_nan() {
                return None;
            }
            Some(*value)
        }
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let l = eval_num_float(self_val, left)?;
            let r = eval_num_float(self_val, right)?;
            let result = match op {
                ArithOp::Add => l + r,
                ArithOp::Sub => l - r,
                ArithOp::Mul => l * r,
                ArithOp::Div => {
                    if r == 0.0 {
                        return None;
                    }
                    l / r
                }
                ArithOp::Rem => {
                    if r == 0.0 {
                        return None;
                    }
                    l % r
                }
            };
            // Guard against overflow to infinity or NaN (e.g. inf/inf).
            if result.is_finite() {
                Some(result)
            } else {
                None
            }
        }
        RefExpr::Grouped { inner, .. } => eval_num_float(self_val, inner),
        _ => None,
    }
}

// ── Struct-literal field projection (#1540) ──────────────────────────────────

/// Evaluate a predicate against a struct literal's field expressions.
///
/// Resolves `self.field` references in the predicate by looking up `field`
/// in the struct's init expressions and using its literal value. Returns
/// `RuntimeCheck` when any field referenced by the predicate has a non-literal
/// init expression.
pub(super) fn eval_pred_struct(fields: &[(String, Expr)], pred: &RefExpr) -> RefResult {
    match eval_bool_struct(fields, pred) {
        Some(true) => RefResult::Proven,
        Some(false) => RefResult::Failed {
            counterexample: None,
        },
        None => RefResult::RuntimeCheck,
    }
}

/// Evaluate a boolean predicate where `self.field` references resolve to
/// literal field values from the struct construction `fields`.
fn eval_bool_struct(fields: &[(String, Expr)], pred: &RefExpr) -> Option<bool> {
    match pred {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            // Try a bool-domain comparison first (covers `self.alive == true`).
            if let (Some(l), Some(r)) = (
                eval_bool_value_struct(fields, left),
                eval_bool_value_struct(fields, right),
            ) {
                return Some(match op {
                    CmpOp::Eq => l == r,
                    CmpOp::Ne => l != r,
                    // Bool ordering is not defined for the other ops; bail out.
                    _ => return None,
                });
            }
            // Fall back to the integer domain.
            let l = eval_num_int_struct(fields, left)?;
            let r = eval_num_int_struct(fields, right)?;
            Some(match op {
                CmpOp::Eq => l == r,
                CmpOp::Ne => l != r,
                CmpOp::Lt => l < r,
                CmpOp::Gt => l > r,
                CmpOp::Le => l <= r,
                CmpOp::Ge => l >= r,
            })
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => match op {
            LogicOp::And => {
                let l = eval_bool_struct(fields, left);
                if l == Some(false) {
                    return Some(false);
                }
                let r = eval_bool_struct(fields, right);
                match (l, r) {
                    (Some(a), Some(b)) => Some(a && b),
                    _ => None,
                }
            }
            LogicOp::Or => {
                let l = eval_bool_struct(fields, left);
                if l == Some(true) {
                    return Some(true);
                }
                let r = eval_bool_struct(fields, right);
                match (l, r) {
                    (Some(a), Some(b)) => Some(a || b),
                    _ => None,
                }
            }
        },
        RefExpr::Not { inner, .. } => Some(!eval_bool_struct(fields, inner)?),
        RefExpr::Grouped { inner, .. } => eval_bool_struct(fields, inner),
        // A bare `self.field` of bool type in a predicate context (e.g. an
        // ensures clause that's just `result.alive`) is treated as the field's
        // boolean value, if it resolves to a bool literal.
        RefExpr::FieldAccess { .. } => eval_bool_value_struct(fields, pred),
        RefExpr::Bool { value, .. } => Some(*value),
        _ => None,
    }
}

/// Resolve a numeric sub-expression to an integer using struct field values.
fn eval_num_int_struct(fields: &[(String, Expr)], expr: &RefExpr) -> Option<i64> {
    match expr {
        RefExpr::Integer { value, .. } => Some(*value),
        RefExpr::FieldAccess { object, field, .. } => {
            if let RefExpr::Ident { name, .. } = object.as_ref() {
                if is_self_like(name) {
                    let init = fields.iter().find(|(n, _)| n == field).map(|(_, e)| e)?;
                    return extract_int_from_expr(init);
                }
            }
            None
        }
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let l = eval_num_int_struct(fields, left)?;
            let r = eval_num_int_struct(fields, right)?;
            match op {
                ArithOp::Add => l.checked_add(r),
                ArithOp::Sub => l.checked_sub(r),
                ArithOp::Mul => l.checked_mul(r),
                ArithOp::Div => {
                    if r == 0 {
                        None
                    } else {
                        Some(l / r)
                    }
                }
                ArithOp::Rem => {
                    if r == 0 {
                        None
                    } else {
                        Some(l % r)
                    }
                }
            }
        }
        RefExpr::Grouped { inner, .. } => eval_num_int_struct(fields, inner),
        _ => None,
    }
}

/// Resolve a sub-expression to a boolean value using struct field values.
/// Handles `self.field` where the field's init is a `Literal::Bool`, and
/// bare `Bool` literals in the predicate.
fn eval_bool_value_struct(fields: &[(String, Expr)], expr: &RefExpr) -> Option<bool> {
    match expr {
        RefExpr::Bool { value, .. } => Some(*value),
        RefExpr::FieldAccess { object, field, .. } => {
            if let RefExpr::Ident { name, .. } = object.as_ref() {
                if is_self_like(name) {
                    let init = fields.iter().find(|(n, _)| n == field).map(|(_, e)| e)?;
                    return extract_bool_from_expr(init);
                }
            }
            None
        }
        RefExpr::Grouped { inner, .. } => eval_bool_value_struct(fields, inner),
        RefExpr::Not { inner, .. } => Some(!eval_bool_value_struct(fields, inner)?),
        _ => None,
    }
}

/// Extract a literal integer from a struct field's init expression.
fn extract_int_from_expr(expr: &Expr) -> Option<i64> {
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

/// Extract a literal boolean from a struct field's init expression.
fn extract_bool_from_expr(expr: &Expr) -> Option<bool> {
    match expr {
        Expr::Literal(Literal::Bool(b), _) => Some(*b),
        _ => None,
    }
}

// ── Structural predicate equivalence ─────────────────────────────────────────

/// Returns `true` when two predicates are structurally equivalent after
/// normalising all parameter names to the canonical `"self"`.
///
/// This lets us prove that `fn f(x: Int where x > 0)` satisfies
/// `fn g(y: Int where y > 0)` — the refinement is the same predicate,
/// just with a different parameter name in place of `self`.
pub(super) fn preds_equivalent(a: &RefExpr, b: &RefExpr) -> bool {
    match (a, b) {
        (RefExpr::Ident { name: na, .. }, RefExpr::Ident { name: nb, .. }) => {
            is_self_like(na) && is_self_like(nb)
                || (!is_self_like(na) && !is_self_like(nb) && na == nb)
        }
        (RefExpr::Integer { value: va, .. }, RefExpr::Integer { value: vb, .. }) => va == vb,
        (RefExpr::Float { value: va, .. }, RefExpr::Float { value: vb, .. }) => {
            // NaN is never structurally equivalent even to itself.
            if va.is_nan() || vb.is_nan() {
                return false;
            }
            va.to_bits() == vb.to_bits()
        }
        (
            RefExpr::Compare {
                op: oa,
                left: la,
                right: ra,
                ..
            },
            RefExpr::Compare {
                op: ob,
                left: lb,
                right: rb,
                ..
            },
        ) => oa == ob && preds_equivalent(la, lb) && preds_equivalent(ra, rb),
        (
            RefExpr::LogicOp {
                op: oa,
                left: la,
                right: ra,
                ..
            },
            RefExpr::LogicOp {
                op: ob,
                left: lb,
                right: rb,
                ..
            },
        ) => oa == ob && preds_equivalent(la, lb) && preds_equivalent(ra, rb),
        (
            RefExpr::ArithOp {
                op: oa,
                left: la,
                right: ra,
                ..
            },
            RefExpr::ArithOp {
                op: ob,
                left: lb,
                right: rb,
                ..
            },
        ) => oa == ob && preds_equivalent(la, lb) && preds_equivalent(ra, rb),
        (RefExpr::Not { inner: ia, .. }, RefExpr::Not { inner: ib, .. }) => {
            preds_equivalent(ia, ib)
        }
        (RefExpr::Grouped { inner: ia, .. }, RefExpr::Grouped { inner: ib, .. }) => {
            preds_equivalent(ia, ib)
        }
        (RefExpr::Grouped { inner, .. }, other) | (other, RefExpr::Grouped { inner, .. }) => {
            preds_equivalent(inner, other)
        }
        (RefExpr::Len { ident: ia, .. }, RefExpr::Len { ident: ib, .. }) => {
            is_self_like(ia) && is_self_like(ib)
        }
        _ => false,
    }
}

/// Returns `true` for `"self"` — the canonical parameter name used after
/// predicate normalisation.
pub(super) fn is_self_like(name: &str) -> bool {
    name == "self"
}

// ── Hypothesis helpers ────────────────────────────────────────────────────────

/// If `pred` has the form `self == <int>`, extract that integer.
pub(super) fn extract_eq_int_from_hyp(pred: &RefExpr) -> Option<i64> {
    match pred {
        RefExpr::Compare {
            op: CmpOp::Eq,
            left,
            right,
            ..
        } => match (left.as_ref(), right.as_ref()) {
            (RefExpr::Ident { name, .. }, RefExpr::Integer { value, .. }) if is_self_like(name) => {
                Some(*value)
            }
            (RefExpr::Integer { value, .. }, RefExpr::Ident { name, .. }) if is_self_like(name) => {
                Some(*value)
            }
            _ => None,
        },
        _ => None,
    }
}

/// If `pred` has the form `self == <float>`, extract that float.
pub(super) fn extract_eq_float_from_hyp(pred: &RefExpr) -> Option<f64> {
    match pred {
        RefExpr::Compare {
            op: CmpOp::Eq,
            left,
            right,
            ..
        } => match (left.as_ref(), right.as_ref()) {
            (RefExpr::Ident { name, .. }, RefExpr::Float { value, .. }) if is_self_like(name) => {
                Some(*value)
            }
            (RefExpr::Float { value, .. }, RefExpr::Ident { name, .. }) if is_self_like(name) => {
                Some(*value)
            }
            _ => None,
        },
        _ => None,
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::lexer::Span;

    fn sp() -> Span {
        Span {
            line: 1,
            col: 1,
            offset: 0,
            len: 0,
        }
    }

    fn self_ident() -> RefExpr {
        RefExpr::Ident {
            name: "self".into(),
            span: sp(),
        }
    }

    fn int_lit(n: i64) -> RefExpr {
        RefExpr::Integer {
            value: n,
            span: sp(),
        }
    }

    fn compare(op: CmpOp, left: RefExpr, right: RefExpr) -> RefExpr {
        RefExpr::Compare {
            op,
            left: Box::new(left),
            right: Box::new(right),
            span: sp(),
        }
    }

    fn and(a: RefExpr, b: RefExpr) -> RefExpr {
        RefExpr::LogicOp {
            op: LogicOp::And,
            left: Box::new(a),
            right: Box::new(b),
            span: sp(),
        }
    }

    fn or(a: RefExpr, b: RefExpr) -> RefExpr {
        RefExpr::LogicOp {
            op: LogicOp::Or,
            left: Box::new(a),
            right: Box::new(b),
            span: sp(),
        }
    }

    fn not(inner: RefExpr) -> RefExpr {
        RefExpr::Not {
            inner: Box::new(inner),
            span: sp(),
        }
    }

    fn self_gt(n: i64) -> RefExpr {
        compare(CmpOp::Gt, self_ident(), int_lit(n))
    }

    fn self_lt(n: i64) -> RefExpr {
        compare(CmpOp::Lt, self_ident(), int_lit(n))
    }

    fn self_ge(n: i64) -> RefExpr {
        compare(CmpOp::Ge, self_ident(), int_lit(n))
    }

    fn self_le(n: i64) -> RefExpr {
        compare(CmpOp::Le, self_ident(), int_lit(n))
    }

    fn self_eq_self() -> RefExpr {
        compare(CmpOp::Eq, self_ident(), self_ident())
    }

    fn int_arg(n: i64) -> Expr {
        Expr::Literal(Literal::Integer(n), sp())
    }

    fn unknown_var_arg() -> Expr {
        Expr::Ident("some_unknown_var".into(), sp())
    }

    // ── Tautology ──────────────────────────────────────────────────────────

    #[test]
    fn tautology_self_eq_self() {
        assert!(is_tautology(&self_eq_self()));
    }

    #[test]
    fn tautology_self_le_self() {
        assert!(is_tautology(&compare(
            CmpOp::Le,
            self_ident(),
            self_ident()
        )));
    }

    #[test]
    fn tautology_self_ge_self() {
        assert!(is_tautology(&compare(
            CmpOp::Ge,
            self_ident(),
            self_ident()
        )));
    }

    #[test]
    fn tautology_not_of_contradiction() {
        // !(self > 0 && self < 0) is a tautology
        assert!(is_tautology(&not(and(self_gt(0), self_lt(0)))));
    }

    #[test]
    fn tautology_or_with_tautology_arm() {
        // (self > 0) || (self == self) — second arm is a tautology
        assert!(is_tautology(&or(self_gt(0), self_eq_self())));
    }

    #[test]
    fn not_tautology_gt_zero() {
        assert!(!is_tautology(&self_gt(0)));
    }

    // ── Contradiction ──────────────────────────────────────────────────────

    #[test]
    fn contradiction_self_lt_self() {
        assert!(is_contradiction(&compare(
            CmpOp::Lt,
            self_ident(),
            self_ident()
        )));
    }

    #[test]
    fn contradiction_self_ne_self() {
        assert!(is_contradiction(&compare(
            CmpOp::Ne,
            self_ident(),
            self_ident()
        )));
    }

    #[test]
    fn contradiction_a_and_not_a() {
        assert!(is_contradiction(&and(self_gt(0), not(self_gt(0)))));
    }

    #[test]
    fn contradiction_not_a_and_a() {
        assert!(is_contradiction(&and(not(self_gt(0)), self_gt(0))));
    }

    #[test]
    fn contradiction_interval_gt_lt_same_bound() {
        // self > 0 && self < 0 — classic empty interval
        assert!(is_contradiction(&and(self_gt(0), self_lt(0))));
    }

    #[test]
    fn contradiction_interval_ge_le_reversed() {
        // self >= 5 && self <= 3 — empty
        assert!(is_contradiction(&and(self_ge(5), self_le(3))));
    }

    #[test]
    fn not_contradiction_valid_range() {
        // self > 0 && self < 10 — valid
        assert!(!is_contradiction(&and(self_gt(0), self_lt(10))));
    }

    #[test]
    fn contradiction_not_of_tautology() {
        assert!(is_contradiction(&not(self_eq_self())));
    }

    // ── try_trivial ────────────────────────────────────────────────────────

    #[test]
    fn trivial_literal_proven() {
        let result = try_trivial(&self_gt(0), &int_arg(42), &HashMap::new(), &HashMap::new());
        assert_eq!(result, Some(RefResult::Proven));
    }

    #[test]
    fn trivial_literal_failed() {
        let result = try_trivial(&self_gt(0), &int_arg(-1), &HashMap::new(), &HashMap::new());
        assert_eq!(
            result,
            Some(RefResult::Failed {
                counterexample: None
            })
        );
    }

    #[test]
    fn trivial_tautology_any_arg() {
        // Even a negative arg satisfies `self == self`
        let result = try_trivial(
            &self_eq_self(),
            &int_arg(-999),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(result, Some(RefResult::Proven));
    }

    #[test]
    fn trivial_contradiction_any_arg() {
        // `self > 0 && self < 0` fails regardless of arg
        let pred = and(self_gt(0), self_lt(0));
        let result = try_trivial(&pred, &int_arg(0), &HashMap::new(), &HashMap::new());
        assert_eq!(
            result,
            Some(RefResult::Failed {
                counterexample: None
            })
        );
    }

    #[test]
    fn trivial_unknown_var_returns_none() {
        // Unknown variable with no refinement in env → Layer 1 cannot decide
        let result = try_trivial(
            &self_gt(0),
            &unknown_var_arg(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(result, None);
    }

    #[test]
    fn trivial_subsumption_proven() {
        // Variable carries the same refinement as the required pred
        let pred = self_gt(0);
        let mut var_refs = HashMap::new();
        var_refs.insert("x".to_string(), Some(self_gt(0)));
        let arg = Expr::Ident("x".into(), sp());
        let result = try_trivial(&pred, &arg, &var_refs, &HashMap::new());
        assert_eq!(result, Some(RefResult::Proven));
    }
}
