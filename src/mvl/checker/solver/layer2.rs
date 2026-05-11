// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Layer 2 — interval arithmetic for refinement predicates.
//!
//! Handles cases where Layer 1 cannot decide by extracting integer intervals
//! from variable hypotheses and checking whether the entire interval satisfies
//! the required predicate.
//!
//! | Capability          | Example                                                |
//! |---------------------|--------------------------------------------------------|
//! | Interval proving    | `x: self > 5` → proves `require_positive(x)` (> 0)    |
//! | Interval refuting   | `x: self < 0` → refutes `require_positive(x)` (> 0)   |
//! | Compound intervals  | `self > 0 && self <= 100` → intersects to [1, 100]     |
//!
//! If-condition narrowing is performed upstream in `refinements.rs` by
//! `inject_if_hypothesis`, which injects condition predicates into the
//! `var_refs` clone for the then-block before calling into the solver.

use std::collections::HashMap;

use crate::mvl::parser::ast::{CmpOp, Expr, LogicOp, RefExpr};

use super::RefResult;

// ── Interval types ─────────────────────────────────────────────────────────────

/// A bound on one end of an integer interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Bound {
    /// No bound in this direction (represents ±∞).
    Unbounded,
    /// The endpoint is included: value ≥ n (lo) or value ≤ n (hi).
    Inclusive(i64),
    /// The endpoint is excluded: value > n (lo) or value < n (hi).
    Exclusive(i64),
}

/// A possibly-unbounded integer interval `[lo, hi]`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Interval {
    /// Lower bound (towards −∞).
    pub lo: Bound,
    /// Upper bound (towards +∞).
    pub hi: Bound,
}

impl Interval {
    /// The smallest concrete integer in the interval, if it has a finite lower bound.
    fn effective_min(self) -> Option<i64> {
        match self.lo {
            Bound::Unbounded => None,
            Bound::Inclusive(n) => Some(n),
            Bound::Exclusive(n) => n.checked_add(1),
        }
    }

    /// The largest concrete integer in the interval, if it has a finite upper bound.
    fn effective_max(self) -> Option<i64> {
        match self.hi {
            Bound::Unbounded => None,
            Bound::Inclusive(n) => Some(n),
            Bound::Exclusive(n) => n.checked_sub(1),
        }
    }
}

// ── Bound arithmetic ──────────────────────────────────────────────────────────

/// Return the tighter (higher effective minimum) of two lower bounds.
fn tighter_lo(a: Bound, b: Bound) -> Bound {
    match (a, b) {
        (Bound::Unbounded, x) | (x, Bound::Unbounded) => x,
        (Bound::Inclusive(m), Bound::Inclusive(n)) => Bound::Inclusive(m.max(n)),
        (Bound::Exclusive(m), Bound::Exclusive(n)) => Bound::Exclusive(m.max(n)),
        (Bound::Inclusive(m), Bound::Exclusive(n)) | (Bound::Exclusive(n), Bound::Inclusive(m)) => {
            // eff_min(Inclusive(m)) = m; eff_min(Exclusive(n)) = n+1
            // Exclusive(n) is tighter when n+1 > m, i.e. n >= m.
            if n >= m {
                Bound::Exclusive(n)
            } else {
                Bound::Inclusive(m)
            }
        }
    }
}

/// Return the tighter (lower effective maximum) of two upper bounds.
fn tighter_hi(a: Bound, b: Bound) -> Bound {
    match (a, b) {
        (Bound::Unbounded, x) | (x, Bound::Unbounded) => x,
        (Bound::Inclusive(m), Bound::Inclusive(n)) => Bound::Inclusive(m.min(n)),
        (Bound::Exclusive(m), Bound::Exclusive(n)) => Bound::Exclusive(m.min(n)),
        (Bound::Inclusive(m), Bound::Exclusive(n)) | (Bound::Exclusive(n), Bound::Inclusive(m)) => {
            // eff_max(Inclusive(m)) = m; eff_max(Exclusive(n)) = n-1
            // Exclusive(n) is tighter when n-1 < m, i.e. n <= m.
            if n <= m {
                Bound::Exclusive(n)
            } else {
                Bound::Inclusive(m)
            }
        }
    }
}

fn intersect(a: Interval, b: Interval) -> Interval {
    Interval {
        lo: tighter_lo(a.lo, b.lo),
        hi: tighter_hi(a.hi, b.hi),
    }
}

// ── Interval extraction ────────────────────────────────────────────────────────

/// Convert a `RefExpr` hypothesis about `self` into an integer interval.
///
/// Returns `None` for predicates that cannot be expressed as a contiguous
/// integer interval (e.g. `self != N`, arbitrary arithmetic).
pub(super) fn interval_from_ref_expr(pred: &RefExpr) -> Option<Interval> {
    match pred {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let (cmp, n) = self_int_bound(op, left, right)?;
            Some(match cmp {
                CmpOp::Gt => Interval {
                    lo: Bound::Exclusive(n),
                    hi: Bound::Unbounded,
                },
                CmpOp::Ge => Interval {
                    lo: Bound::Inclusive(n),
                    hi: Bound::Unbounded,
                },
                CmpOp::Lt => Interval {
                    lo: Bound::Unbounded,
                    hi: Bound::Exclusive(n),
                },
                CmpOp::Le => Interval {
                    lo: Bound::Unbounded,
                    hi: Bound::Inclusive(n),
                },
                CmpOp::Eq => Interval {
                    lo: Bound::Inclusive(n),
                    hi: Bound::Inclusive(n),
                },
                CmpOp::Ne => return None, // Ne does not form a contiguous interval.
            })
        }
        RefExpr::LogicOp {
            op: LogicOp::And,
            left,
            right,
            ..
        } => {
            let a = interval_from_ref_expr(left)?;
            let b = interval_from_ref_expr(right)?;
            Some(intersect(a, b))
        }
        RefExpr::Grouped { inner, .. } => interval_from_ref_expr(inner),
        _ => None,
    }
}

/// Extract `(op, n)` from a comparison of the form `self op n` or `n op self`.
///
/// The result is always normalised to the `self op n` perspective.
fn self_int_bound(op: &CmpOp, left: &RefExpr, right: &RefExpr) -> Option<(CmpOp, i64)> {
    match (left, right) {
        (RefExpr::Ident { name, .. }, RefExpr::Integer { value, .. }) if name == "self" => {
            Some((*op, *value))
        }
        (RefExpr::Integer { value, .. }, RefExpr::Ident { name, .. }) if name == "self" => {
            // Flip: `n op self` → `self flip(op) n`
            Some((op.flip(), *value))
        }
        _ => None,
    }
}

// ── Interval → predicate containment ─────────────────────────────────────────

/// Check whether all integers in `iv` satisfy `pred`.
///
/// - `Some(true)` — every value in the interval satisfies the predicate.
/// - `Some(false)` — no value in the interval satisfies the predicate.
/// - `None` — partial overlap or predicate too complex to decide.
fn interval_satisfies_pred(iv: Interval, pred: &RefExpr) -> Option<bool> {
    match pred {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let (cmp, n) = self_int_bound(op, left, right)?;
            match cmp {
                CmpOp::Gt => {
                    // Proven if eff_min > n; Failed if eff_max <= n.
                    if let Some(mn) = iv.effective_min() {
                        if mn > n {
                            return Some(true);
                        }
                    }
                    if let Some(mx) = iv.effective_max() {
                        if mx <= n {
                            return Some(false);
                        }
                    }
                    None
                }
                CmpOp::Ge => {
                    if let Some(mn) = iv.effective_min() {
                        if mn >= n {
                            return Some(true);
                        }
                    }
                    if let Some(mx) = iv.effective_max() {
                        if mx < n {
                            return Some(false);
                        }
                    }
                    None
                }
                CmpOp::Lt => {
                    if let Some(mx) = iv.effective_max() {
                        if mx < n {
                            return Some(true);
                        }
                    }
                    if let Some(mn) = iv.effective_min() {
                        if mn >= n {
                            return Some(false);
                        }
                    }
                    None
                }
                CmpOp::Le => {
                    if let Some(mx) = iv.effective_max() {
                        if mx <= n {
                            return Some(true);
                        }
                    }
                    if let Some(mn) = iv.effective_min() {
                        if mn > n {
                            return Some(false);
                        }
                    }
                    None
                }
                CmpOp::Eq => {
                    // Proven if interval is exactly the single point [n, n].
                    match (iv.effective_min(), iv.effective_max()) {
                        (Some(mn), Some(mx)) if mn == n && mx == n => Some(true),
                        (Some(mn), _) if mn > n => Some(false),
                        (_, Some(mx)) if mx < n => Some(false),
                        _ => None,
                    }
                }
                CmpOp::Ne => {
                    // Proven if n is entirely outside [eff_min, eff_max].
                    if let Some(mn) = iv.effective_min() {
                        if mn > n {
                            return Some(true);
                        }
                    }
                    if let Some(mx) = iv.effective_max() {
                        if mx < n {
                            return Some(true);
                        }
                    }
                    // Failed if the interval is exactly [n, n].
                    match (iv.effective_min(), iv.effective_max()) {
                        (Some(mn), Some(mx)) if mn == n && mx == n => Some(false),
                        _ => None,
                    }
                }
            }
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => match op {
            LogicOp::And => {
                let l = interval_satisfies_pred(iv, left);
                let r = interval_satisfies_pred(iv, right);
                match (l, r) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                }
            }
            LogicOp::Or => {
                let l = interval_satisfies_pred(iv, left);
                let r = interval_satisfies_pred(iv, right);
                match (l, r) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                }
            }
        },
        RefExpr::Not { inner, .. } => interval_satisfies_pred(iv, inner).map(|b| !b),
        RefExpr::Grouped { inner, .. } => interval_satisfies_pred(iv, inner),
        _ => None,
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Try to prove or refute `pred` for `arg` using interval arithmetic.
///
/// Only applicable when `arg` is a named variable whose hypothesis in
/// `var_refs` can be expressed as an integer interval.
///
/// Returns `None` when Layer 2 cannot make a determination.
pub(super) fn try_interval(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<RefResult> {
    let name = match arg {
        Expr::Ident(name, _) => name.as_str(),
        _ => return None,
    };
    let hyp = match var_refs.get(name) {
        Some(Some(h)) => h,
        _ => return None,
    };
    let iv = interval_from_ref_expr(hyp)?;
    match interval_satisfies_pred(iv, pred) {
        Some(true) => Some(RefResult::Proven),
        Some(false) => Some(RefResult::Failed),
        None => None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::ast::{CmpOp, LogicOp, RefExpr};
    use crate::mvl::parser::lexer::Span;

    fn s() -> Span {
        Span::new(0, 0, 0, 0)
    }

    fn self_ref() -> RefExpr {
        RefExpr::Ident {
            name: "self".to_string(),
            span: s(),
        }
    }

    fn int(n: i64) -> RefExpr {
        RefExpr::Integer {
            value: n,
            span: s(),
        }
    }

    fn cmp(op: CmpOp, left: RefExpr, right: RefExpr) -> RefExpr {
        RefExpr::Compare {
            op,
            left: Box::new(left),
            right: Box::new(right),
            span: s(),
        }
    }

    fn and(left: RefExpr, right: RefExpr) -> RefExpr {
        RefExpr::LogicOp {
            op: LogicOp::And,
            left: Box::new(left),
            right: Box::new(right),
            span: s(),
        }
    }

    fn or(left: RefExpr, right: RefExpr) -> RefExpr {
        RefExpr::LogicOp {
            op: LogicOp::Or,
            left: Box::new(left),
            right: Box::new(right),
            span: s(),
        }
    }

    // ── interval_from_ref_expr ────────────────────────────────────────────────

    #[test]
    fn interval_gt_gives_exclusive_lo() {
        let iv = interval_from_ref_expr(&cmp(CmpOp::Gt, self_ref(), int(5))).unwrap();
        assert_eq!(iv.lo, Bound::Exclusive(5));
        assert_eq!(iv.hi, Bound::Unbounded);
        assert_eq!(iv.effective_min(), Some(6));
        assert_eq!(iv.effective_max(), None);
    }

    #[test]
    fn interval_ge_gives_inclusive_lo() {
        let iv = interval_from_ref_expr(&cmp(CmpOp::Ge, self_ref(), int(1))).unwrap();
        assert_eq!(iv.lo, Bound::Inclusive(1));
        assert_eq!(iv.effective_min(), Some(1));
    }

    #[test]
    fn interval_lt_gives_exclusive_hi() {
        let iv = interval_from_ref_expr(&cmp(CmpOp::Lt, self_ref(), int(0))).unwrap();
        assert_eq!(iv.hi, Bound::Exclusive(0));
        assert_eq!(iv.effective_max(), Some(-1));
    }

    #[test]
    fn interval_le_gives_inclusive_hi() {
        let iv = interval_from_ref_expr(&cmp(CmpOp::Le, self_ref(), int(100))).unwrap();
        assert_eq!(iv.hi, Bound::Inclusive(100));
        assert_eq!(iv.effective_max(), Some(100));
    }

    #[test]
    fn interval_eq_gives_point() {
        let iv = interval_from_ref_expr(&cmp(CmpOp::Eq, self_ref(), int(7))).unwrap();
        assert_eq!(iv.effective_min(), Some(7));
        assert_eq!(iv.effective_max(), Some(7));
    }

    #[test]
    fn interval_ne_returns_none() {
        assert!(interval_from_ref_expr(&cmp(CmpOp::Ne, self_ref(), int(0))).is_none());
    }

    #[test]
    fn interval_and_intersection() {
        // self > 0 && self < 10 → effective [1, 9]
        let pred = and(
            cmp(CmpOp::Gt, self_ref(), int(0)),
            cmp(CmpOp::Lt, self_ref(), int(10)),
        );
        let iv = interval_from_ref_expr(&pred).unwrap();
        assert_eq!(iv.effective_min(), Some(1));
        assert_eq!(iv.effective_max(), Some(9));
    }

    #[test]
    fn interval_flipped_comparison() {
        // 5 < self  →  self > 5
        let iv = interval_from_ref_expr(&cmp(CmpOp::Lt, int(5), self_ref())).unwrap();
        assert_eq!(iv.lo, Bound::Exclusive(5));
        assert_eq!(iv.effective_min(), Some(6));
    }

    // ── interval_satisfies_pred ───────────────────────────────────────────────

    #[test]
    fn gt5_satisfies_gt0() {
        // hyp: self > 5 → [6, ∞); pred: self > 0 → proven (6 > 0)
        let iv = interval_from_ref_expr(&cmp(CmpOp::Gt, self_ref(), int(5))).unwrap();
        let pred = cmp(CmpOp::Gt, self_ref(), int(0));
        assert_eq!(interval_satisfies_pred(iv, &pred), Some(true));
    }

    #[test]
    fn gt5_satisfies_ge1() {
        let iv = interval_from_ref_expr(&cmp(CmpOp::Gt, self_ref(), int(5))).unwrap();
        let pred = cmp(CmpOp::Ge, self_ref(), int(1));
        assert_eq!(interval_satisfies_pred(iv, &pred), Some(true));
    }

    #[test]
    fn gt0_insufficient_for_gt5() {
        // hyp: self > 0 → [1, ∞); pred: self > 5 → cannot prove (1 ≤ 5)
        let iv = interval_from_ref_expr(&cmp(CmpOp::Gt, self_ref(), int(0))).unwrap();
        let pred = cmp(CmpOp::Gt, self_ref(), int(5));
        assert_eq!(interval_satisfies_pred(iv, &pred), None);
    }

    #[test]
    fn lt0_fails_gt0() {
        // hyp: self < 0 → (−∞, −1]; pred: self > 0 → failed (−1 ≤ 0)
        let iv = interval_from_ref_expr(&cmp(CmpOp::Lt, self_ref(), int(0))).unwrap();
        let pred = cmp(CmpOp::Gt, self_ref(), int(0));
        assert_eq!(interval_satisfies_pred(iv, &pred), Some(false));
    }

    #[test]
    fn gt5_fails_lt0() {
        // hyp: self > 5 → [6, ∞); pred: self < 0 → failed (6 ≥ 0)
        let iv = interval_from_ref_expr(&cmp(CmpOp::Gt, self_ref(), int(5))).unwrap();
        let pred = cmp(CmpOp::Lt, self_ref(), int(0));
        assert_eq!(interval_satisfies_pred(iv, &pred), Some(false));
    }

    #[test]
    fn eq5_satisfies_gt0() {
        // hyp: self == 5 → [5, 5]; pred: self > 0 → proven (5 > 0)
        let iv = interval_from_ref_expr(&cmp(CmpOp::Eq, self_ref(), int(5))).unwrap();
        let pred = cmp(CmpOp::Gt, self_ref(), int(0));
        assert_eq!(interval_satisfies_pred(iv, &pred), Some(true));
    }

    #[test]
    fn eq0_fails_gt0() {
        // hyp: self == 0 → [0, 0]; pred: self > 0 → failed (0 ≤ 0)
        let iv = interval_from_ref_expr(&cmp(CmpOp::Eq, self_ref(), int(0))).unwrap();
        let pred = cmp(CmpOp::Gt, self_ref(), int(0));
        assert_eq!(interval_satisfies_pred(iv, &pred), Some(false));
    }

    #[test]
    fn compound_interval_satisfies_both_sides() {
        // hyp: self > 0 && self <= 100 → [1, 100]
        let hyp = and(
            cmp(CmpOp::Gt, self_ref(), int(0)),
            cmp(CmpOp::Le, self_ref(), int(100)),
        );
        let iv = interval_from_ref_expr(&hyp).unwrap();
        assert_eq!(iv.effective_min(), Some(1));
        assert_eq!(iv.effective_max(), Some(100));

        // pred: self > 0 → proven
        let pred_gt = cmp(CmpOp::Gt, self_ref(), int(0));
        assert_eq!(interval_satisfies_pred(iv, &pred_gt), Some(true));

        // pred: self <= 100 → proven
        let pred_le = cmp(CmpOp::Le, self_ref(), int(100));
        assert_eq!(interval_satisfies_pred(iv, &pred_le), Some(true));

        // pred: self >= 0 → proven (since min=1 >= 0)
        let pred_ge = cmp(CmpOp::Ge, self_ref(), int(0));
        assert_eq!(interval_satisfies_pred(iv, &pred_ge), Some(true));
    }

    #[test]
    fn ne_proven_when_n_outside_interval() {
        // hyp: self > 5 → [6, ∞); pred: self != 0 → proven (6 > 0)
        let iv = interval_from_ref_expr(&cmp(CmpOp::Gt, self_ref(), int(5))).unwrap();
        let pred = cmp(CmpOp::Ne, self_ref(), int(0));
        assert_eq!(interval_satisfies_pred(iv, &pred), Some(true));
    }

    #[test]
    fn or_pred_proven_when_one_arm_proven() {
        // hyp: self > 5 → [6, ∞); pred: self > 0 || self < -5
        // Left arm self > 0 is proven, so the disjunction is proven.
        let iv = interval_from_ref_expr(&cmp(CmpOp::Gt, self_ref(), int(5))).unwrap();
        let pred = or(
            cmp(CmpOp::Gt, self_ref(), int(0)),
            cmp(CmpOp::Lt, self_ref(), int(-5)),
        );
        assert_eq!(interval_satisfies_pred(iv, &pred), Some(true));
    }

    // ── tighter_lo / tighter_hi ───────────────────────────────────────────────

    #[test]
    fn tighter_lo_inclusive_vs_exclusive() {
        // Inclusive(5) eff_min=5 vs Exclusive(5) eff_min=6 → Exclusive(5) tighter
        assert_eq!(
            tighter_lo(Bound::Inclusive(5), Bound::Exclusive(5)),
            Bound::Exclusive(5)
        );
        // Inclusive(6) eff_min=6 vs Exclusive(4) eff_min=5 → Inclusive(6) tighter
        assert_eq!(
            tighter_lo(Bound::Inclusive(6), Bound::Exclusive(4)),
            Bound::Inclusive(6)
        );
    }

    #[test]
    fn tighter_hi_inclusive_vs_exclusive() {
        // Inclusive(5) eff_max=5 vs Exclusive(5) eff_max=4 → Exclusive(5) tighter
        assert_eq!(
            tighter_hi(Bound::Inclusive(5), Bound::Exclusive(5)),
            Bound::Exclusive(5)
        );
        // Inclusive(4) eff_max=4 vs Exclusive(6) eff_max=5 → Inclusive(4) tighter
        assert_eq!(
            tighter_hi(Bound::Inclusive(4), Bound::Exclusive(6)),
            Bound::Inclusive(4)
        );
    }
}
