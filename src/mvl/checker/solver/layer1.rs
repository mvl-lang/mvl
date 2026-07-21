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
use crate::mvl::parser::ast::{
    ArithOp, BinaryOp, CmpOp, Expr, FnDecl, Literal, LogicOp, RefExpr, StringOp, UnaryOp,
};

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
    // Closed-form evaluation (#1915): predicate has no free identifiers and can
    // be fully evaluated as a concrete boolean. Enables L1 discharge of bounded-
    // quantifier expansion instances like `Integer(0) < Integer(10)`.
    if let Some(b) = try_eval_closed(pred) {
        return Some(if b {
            RefResult::Proven
        } else {
            RefResult::Failed {
                counterexample: None,
            }
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

        // String literal: evaluate `len(ident)` and string-content predicates against the literal.
        // Enables static proof of e.g. `validate_log_path("app.log")` where pred is `len(p) > 0`,
        // or `validate_param("safe")` where pred is `!self.contains("'")`.
        Expr::Literal(Literal::Str(s), _) => {
            // Try string-content evaluation first (covers StringOp nodes); fall back to len-only.
            match eval_bool_str_content(s, pred) {
                Some(b) => Some(if b {
                    RefResult::Proven
                } else {
                    RefResult::Failed {
                        counterexample: None,
                    }
                }),
                None => Some(eval_pred_str_len(s.len() as i64, pred)),
            }
        }

        // String concat chain: prove len-based predicates using a conservative lower
        // bound derived from all literal substrings in the chain.  For example:
        //   `"{\"id\":".concat(x).concat(",")`  →  min_len = 6 + 0 + 1 = 7  →  proves `len > 0`
        //   `"[".concat(acc).concat("]")`        →  min_len = 1 + 0 + 1 = 2  →  proves `len > 0`
        // Unknown sub-expressions (variables, function calls) contribute 0.
        Expr::MethodCall { method, .. } if method == "concat" => {
            let min_len = min_str_len_lower(arg);
            if min_len > 0 {
                Some(eval_pred_str_len(min_len, pred))
            } else {
                None
            }
        }

        // Struct literal as return value (#1540): resolve `self.field` against the
        // construct's field exprs. Enables `ensures result.field == literal` to be
        // proven statically when the function body is a struct literal.
        Expr::Construct { fields, .. } => Some(eval_pred_struct(fields, pred)),

        // Everything else: Layer 1 cannot decide.
        _ => None,
    }
}

// ── String length lower-bound helper ─────────────────────────────────────────

/// Conservative lower bound on the length of a string expression.
///
/// Recursively sums the character counts of all `Literal::Str` nodes reachable
/// through `concat` chains.  Unknown sub-expressions (variables, calls, etc.)
/// contribute 0 — they might be empty.
///
/// Examples:
/// - `"hello"` → 5
/// - `"hello".concat(x)` → 5
/// - `"a".concat(x).concat("b")` → 2
/// - `x.concat(y)` → 0  (nothing known)
fn min_str_len_lower(expr: &Expr) -> i64 {
    match expr {
        Expr::Literal(Literal::Str(s), _) => s.len() as i64,
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } if method == "concat" => {
            let r = min_str_len_lower(receiver);
            let a = args.first().map(min_str_len_lower).unwrap_or(0);
            r + a
        }
        _ => 0,
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

// ── Closed-form evaluation (#1915) ────────────────────────────────────────────

/// Evaluate a predicate that has no free identifiers to a concrete boolean.
///
/// Returns `None` when the predicate references any identifier (including
/// `self`), any `len(...)`, `old(...)`, quantifier, or field access. Enables
/// L1 to discharge instances produced by bounded-quantifier expansion whose
/// bound variable has already been substituted with a literal integer.
fn try_eval_closed(pred: &RefExpr) -> Option<bool> {
    match pred {
        RefExpr::Bool { value, .. } => Some(*value),
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let l = eval_closed_num(left)?;
            let r = eval_closed_num(right)?;
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
        } => {
            let l = try_eval_closed(left);
            let r = try_eval_closed(right);
            match op {
                LogicOp::And => match (l, r) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                },
                LogicOp::Or => match (l, r) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                },
            }
        }
        RefExpr::Not { inner, .. } => Some(!try_eval_closed(inner)?),
        RefExpr::Grouped { inner, .. } => try_eval_closed(inner),
        _ => None,
    }
}

/// Evaluate a numeric sub-expression with no free identifiers.
fn eval_closed_num(expr: &RefExpr) -> Option<i64> {
    match expr {
        RefExpr::Integer { value, .. } => Some(*value),
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let l = eval_closed_num(left)?;
            let r = eval_closed_num(right)?;
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
        RefExpr::Grouped { inner, .. } => eval_closed_num(inner),
        _ => None,
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

/// Evaluate a boolean predicate against a concrete string literal (#1919).
///
/// Handles `StringOp` nodes (contains/starts_with/ends_with) and `len(x)` nodes
/// together — returns `Some(bool)` when all sub-expressions can be determined from
/// the literal, `None` when any sub-expression is symbolic or unknown.
///
/// Short-circuits `And`/`Or` like the other evaluators.
fn eval_bool_str_content(s: &str, pred: &RefExpr) -> Option<bool> {
    let len_val = s.len() as i64;
    match pred {
        RefExpr::StringOp { op, literal, .. } => Some(match op {
            StringOp::Contains => s.contains(literal.as_str()),
            StringOp::StartsWith => s.starts_with(literal.as_str()),
            StringOp::EndsWith => s.ends_with(literal.as_str()),
        }),
        // Regex-membership fold (#1921). The pattern has already been validated
        // by the parser-side fragment checker, so it should compile — but if a
        // pattern still fails to compile in the `regex` crate (unlikely), return
        // `None` and let a higher tier handle it rather than panicking.
        RefExpr::RegexMatch { pattern, .. } => match ::regex::Regex::new(pattern) {
            Ok(re) => Some(re.is_match(s)),
            Err(_) => None,
        },
        RefExpr::Not { inner, .. } => Some(!eval_bool_str_content(s, inner)?),
        RefExpr::Grouped { inner, .. } => eval_bool_str_content(s, inner),
        RefExpr::LogicOp {
            op, left, right, ..
        } => match op {
            LogicOp::And => {
                let l = eval_bool_str_content(s, left);
                if l == Some(false) {
                    return Some(false);
                }
                let r = eval_bool_str_content(s, right);
                match (l, r) {
                    (Some(a), Some(b)) => Some(a && b),
                    _ => None,
                }
            }
            LogicOp::Or => {
                let l = eval_bool_str_content(s, left);
                if l == Some(true) {
                    return Some(true);
                }
                let r = eval_bool_str_content(s, right);
                match (l, r) {
                    (Some(a), Some(b)) => Some(a || b),
                    _ => None,
                }
            }
        },
        // Compare involving len(x) — delegate to the len evaluator for the numeric part.
        RefExpr::Compare { .. } => {
            // Use eval_bool_str_len to handle len(x) comparisons within a compound predicate.
            eval_bool_str_len(len_val, pred)
        }
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
        // Bitwise operations on integer literals (#1928).
        RefExpr::BitwiseOp {
            op, left, right, ..
        } => {
            use crate::mvl::parser::ast::BitwiseOp;
            let l = eval_num_int(self_val, left)?;
            let r = eval_num_int(self_val, right)?;
            Some(match op {
                BitwiseOp::And => l & r,
                BitwiseOp::Or => l | r,
                BitwiseOp::Xor => l ^ r,
                BitwiseOp::Shl => l.checked_shl(r.try_into().ok()?).unwrap_or(0),
                BitwiseOp::Shr => l.checked_shr(r.try_into().ok()?).unwrap_or(0),
            })
        }
        RefExpr::BitwiseNot { inner, .. } => eval_num_int(self_val, inner).map(|v| !v),
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
            // Symbolic equality (#1796): if pred is `self.<f> <op> Y` and the
            // field's init in the struct is exactly `Y`, the predicate holds
            // (for Eq / Le / Ge / etc.) without needing to eval to an integer.
            //
            // Covers ensures clauses like `result.x == ball.x` in bounce_paddle
            // where the returned struct sets `x: ball.x` — the two sides are
            // structurally identical, but neither is a literal integer.
            if let Some(l_sub) = subst_self_field(left, fields) {
                if let Some(r_sub) = subst_self_field(right, fields) {
                    if ref_expr_shape_eq(&l_sub, &r_sub) {
                        return Some(match op {
                            CmpOp::Eq | CmpOp::Le | CmpOp::Ge => true,
                            CmpOp::Ne | CmpOp::Lt | CmpOp::Gt => false,
                        });
                    }
                }
            }
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

// ── Symbolic substitution + shape equality (#1796) ───────────────────────────
//
// These helpers let Layer 1 discharge ensures postconditions of the form
// `self.<field> == <expr>` when the returned struct literal sets that field
// to `<expr>` verbatim, without needing to reduce to a literal integer.

/// Structural equality on `RefExpr` values, ignoring spans and grouping.
/// Two predicates are shape-equal iff they parse to the same tree.
pub(super) fn ref_expr_shape_eq(a: &RefExpr, b: &RefExpr) -> bool {
    match (a, b) {
        (RefExpr::Grouped { inner, .. }, other) | (other, RefExpr::Grouped { inner, .. }) => {
            ref_expr_shape_eq(inner, other)
        }
        (RefExpr::Ident { name: n1, .. }, RefExpr::Ident { name: n2, .. }) => n1 == n2,
        (RefExpr::Integer { value: v1, .. }, RefExpr::Integer { value: v2, .. }) => v1 == v2,
        (RefExpr::Float { value: v1, .. }, RefExpr::Float { value: v2, .. }) => {
            !v1.is_nan() && !v2.is_nan() && v1.to_bits() == v2.to_bits()
        }
        (RefExpr::Bool { value: v1, .. }, RefExpr::Bool { value: v2, .. }) => v1 == v2,
        (
            RefExpr::FieldAccess {
                object: o1,
                field: f1,
                ..
            },
            RefExpr::FieldAccess {
                object: o2,
                field: f2,
                ..
            },
        ) => f1 == f2 && ref_expr_shape_eq(o1, o2),
        (
            RefExpr::ArithOp {
                op: o1,
                left: l1,
                right: r1,
                ..
            },
            RefExpr::ArithOp {
                op: o2,
                left: l2,
                right: r2,
                ..
            },
        ) => o1 == o2 && ref_expr_shape_eq(l1, l2) && ref_expr_shape_eq(r1, r2),
        (
            RefExpr::Compare {
                op: o1,
                left: l1,
                right: r1,
                ..
            },
            RefExpr::Compare {
                op: o2,
                left: l2,
                right: r2,
                ..
            },
        ) => o1 == o2 && ref_expr_shape_eq(l1, l2) && ref_expr_shape_eq(r1, r2),
        (
            RefExpr::LogicOp {
                op: o1,
                left: l1,
                right: r1,
                ..
            },
            RefExpr::LogicOp {
                op: o2,
                left: l2,
                right: r2,
                ..
            },
        ) => o1 == o2 && ref_expr_shape_eq(l1, l2) && ref_expr_shape_eq(r1, r2),
        (RefExpr::Not { inner: i1, .. }, RefExpr::Not { inner: i2, .. }) => {
            ref_expr_shape_eq(i1, i2)
        }
        _ => false,
    }
}

/// Convert an `Expr` (as it appears in an argument or struct field init)
/// into a `RefExpr` for structural comparison.  Best-effort — only the
/// AST shapes that can appear on both sides of an ensures clause are
/// handled; anything else returns `None` and the caller falls through
/// to numeric or runtime evaluation.
pub(super) fn expr_to_ref_expr_symbolic(expr: &Expr) -> Option<RefExpr> {
    let span = crate::mvl::parser::lexer::Span::new(0, 0, 0, 0);
    match expr {
        Expr::Literal(Literal::Integer(n), _) => Some(RefExpr::Integer { value: *n, span }),
        Expr::Literal(Literal::Float(f), _) => Some(RefExpr::Float { value: *f, span }),
        Expr::Literal(Literal::Bool(b), _) => Some(RefExpr::Bool { value: *b, span }),
        Expr::Ident(name, _) => Some(RefExpr::Ident {
            name: name.clone(),
            span,
        }),
        Expr::FieldAccess {
            expr: inner, field, ..
        } => Some(RefExpr::FieldAccess {
            object: Box::new(expr_to_ref_expr_symbolic(inner)?),
            field: field.clone(),
            span,
        }),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
            ..
        } => Some(RefExpr::ArithOp {
            op: ArithOp::Sub,
            left: Box::new(RefExpr::Integer { value: 0, span }),
            right: Box::new(expr_to_ref_expr_symbolic(inner)?),
            span,
        }),
        Expr::Binary {
            op, left, right, ..
        } => {
            let arith_op = match op {
                BinaryOp::Add => Some(ArithOp::Add),
                BinaryOp::Sub => Some(ArithOp::Sub),
                BinaryOp::Mul => Some(ArithOp::Mul),
                BinaryOp::Div => Some(ArithOp::Div),
                BinaryOp::Rem => Some(ArithOp::Rem),
                _ => None,
            }?;
            Some(RefExpr::ArithOp {
                op: arith_op,
                left: Box::new(expr_to_ref_expr_symbolic(left)?),
                right: Box::new(expr_to_ref_expr_symbolic(right)?),
                span,
            })
        }
        _ => None,
    }
}

/// Substitute occurrences of `self.<F>` in `pred` with the initialiser
/// expression of field `F` in `fields`, then convert the whole thing to a
/// `RefExpr`.  Returns `None` if any field is missing or its initialiser
/// can't be converted (see [`expr_to_ref_expr_symbolic`]).
pub(super) fn subst_self_field(pred: &RefExpr, fields: &[(String, Expr)]) -> Option<RefExpr> {
    let span = crate::mvl::parser::lexer::Span::new(0, 0, 0, 0);
    match pred {
        RefExpr::FieldAccess { object, field, .. } => {
            // `self.<field>` → the init expression for that field.
            if let RefExpr::Ident { name, .. } = object.as_ref() {
                if is_self_like(name) {
                    let init = fields.iter().find(|(n, _)| n == field).map(|(_, e)| e)?;
                    return expr_to_ref_expr_symbolic(init);
                }
            }
            // Nested field access we can't resolve here.
            Some(RefExpr::FieldAccess {
                object: Box::new(subst_self_field(object, fields)?),
                field: field.clone(),
                span,
            })
        }
        RefExpr::ArithOp {
            op, left, right, ..
        } => Some(RefExpr::ArithOp {
            op: *op,
            left: Box::new(subst_self_field(left, fields)?),
            right: Box::new(subst_self_field(right, fields)?),
            span,
        }),
        RefExpr::Grouped { inner, .. } => subst_self_field(inner, fields),
        RefExpr::Integer { value, .. } => Some(RefExpr::Integer {
            value: *value,
            span,
        }),
        RefExpr::Float { value, .. } => Some(RefExpr::Float {
            value: *value,
            span,
        }),
        RefExpr::Bool { value, .. } => Some(RefExpr::Bool {
            value: *value,
            span,
        }),
        RefExpr::Ident { name, .. } => Some(RefExpr::Ident {
            name: name.clone(),
            span,
        }),
        _ => None,
    }
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

    // ── min_str_len_lower ─────────────────────────────────────────────────

    fn str_lit(s: &str) -> Expr {
        Expr::Literal(Literal::Str(s.to_string()), sp())
    }

    fn concat_call(receiver: Expr, arg: Expr) -> Expr {
        Expr::MethodCall {
            receiver: Box::new(receiver),
            method: "concat".to_string(),
            args: vec![arg],
            span: sp(),
        }
    }

    fn var_arg(name: &str) -> Expr {
        Expr::Ident(name.to_string(), sp())
    }

    #[test]
    fn min_str_len_lower_literal() {
        assert_eq!(min_str_len_lower(&str_lit("hello")), 5);
    }

    #[test]
    fn min_str_len_lower_empty_literal() {
        assert_eq!(min_str_len_lower(&str_lit("")), 0);
    }

    #[test]
    fn min_str_len_lower_concat_with_var() {
        // `"prefix".concat(x)` → min = 6
        let expr = concat_call(str_lit("prefix"), var_arg("x"));
        assert_eq!(min_str_len_lower(&expr), 6);
    }

    #[test]
    fn min_str_len_lower_chained_literals() {
        // `"[".concat(acc).concat("]")` → min = 1 + 0 + 1 = 2
        let inner = concat_call(str_lit("["), var_arg("acc"));
        let expr = concat_call(inner, str_lit("]"));
        assert_eq!(min_str_len_lower(&expr), 2);
    }

    #[test]
    fn min_str_len_lower_all_vars() {
        // `x.concat(y)` → 0 (nothing known)
        let expr = concat_call(var_arg("x"), var_arg("y"));
        assert_eq!(min_str_len_lower(&expr), 0);
    }

    // ── try_trivial: concat chain ─────────────────────────────────────────

    fn len_gt_zero() -> RefExpr {
        // `len(self) > 0`
        RefExpr::Compare {
            op: CmpOp::Gt,
            left: Box::new(RefExpr::Len {
                ident: "self".to_string(),
                span: sp(),
            }),
            right: Box::new(RefExpr::Integer {
                value: 0,
                span: sp(),
            }),
            span: sp(),
        }
    }

    #[test]
    fn trivial_concat_nonempty_prefix_proves_len_gt_zero() {
        // `"{\"id\":".concat(x)` — prefix has len 6 → proves `len(self) > 0`
        let expr = concat_call(str_lit("{\"id\":"), var_arg("x"));
        let result = try_trivial(&len_gt_zero(), &expr, &HashMap::new(), &HashMap::new());
        assert_eq!(result, Some(RefResult::Proven));
    }

    #[test]
    fn trivial_concat_bracket_chain_proves_len_gt_zero() {
        // `"[".concat(acc).concat("]")` — min = 2 → proves `len(self) > 0`
        let inner = concat_call(str_lit("["), var_arg("acc"));
        let expr = concat_call(inner, str_lit("]"));
        let result = try_trivial(&len_gt_zero(), &expr, &HashMap::new(), &HashMap::new());
        assert_eq!(result, Some(RefResult::Proven));
    }

    #[test]
    fn trivial_concat_all_vars_returns_none() {
        // `x.concat(y)` — nothing known → Layer 1 cannot decide
        let expr = concat_call(var_arg("x"), var_arg("y"));
        let result = try_trivial(&len_gt_zero(), &expr, &HashMap::new(), &HashMap::new());
        assert_eq!(result, None);
    }
}
