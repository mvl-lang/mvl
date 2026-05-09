//! Layered refinement solver for MVL `where` predicates.
//!
//! Each layer handles an increasingly complex subset of proofs,
//! from O(1) trivial patterns to full SMT queries:
//!
//! | Layer | Module  | Technique                     | ~Coverage |
//! |-------|---------|-------------------------------|-----------|
//! | 1     | layer1  | Trivial pattern matching      | ~40%      |
//! | 2     | layer2  | Interval arithmetic           | ~60%      |
//! | 3     | —       | Symbolic / Cooper's (future)  |           |
//! | 4     | —       | SMT dispatch (future)         |           |

pub mod layer1;
pub mod layer2;

use std::collections::HashMap;

use crate::mvl::parser::ast::{Expr, FnDecl, RefExpr};

// ── Outcome type ──────────────────────────────────────────────────────────────

/// Three-way outcome for a single refinement predicate check at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefResult {
    /// The argument statically satisfies the predicate — no runtime check needed.
    Proven,
    /// Cannot be proven statically — a runtime assertion must be emitted.
    RuntimeCheck,
    /// The argument statically violates the predicate — a compile-time error.
    Failed,
}

// ── Solver ────────────────────────────────────────────────────────────────────

/// The layered refinement solver.
///
/// Each layer returns `None` to signal "I cannot decide — try the next one."
/// The caller falls back to `RuntimeCheck` when all layers are exhausted.
pub(crate) struct RefinementSolver;

impl RefinementSolver {
    /// Try to prove or disprove `pred` for `arg` using Layer 1 (trivial patterns).
    ///
    /// Returns `None` when this layer cannot make a decision.
    pub(crate) fn try_trivial(
        pred: &RefExpr,
        arg: &Expr,
        var_refs: &HashMap<String, Option<RefExpr>>,
        fn_decls: &HashMap<String, FnDecl>,
    ) -> Option<RefResult> {
        layer1::try_trivial(pred, arg, var_refs, fn_decls)
    }

    /// Try to prove or disprove `pred` for `arg` using Layer 2 (interval arithmetic).
    ///
    /// Returns `None` when this layer cannot make a decision.
    pub(crate) fn try_interval(
        pred: &RefExpr,
        arg: &Expr,
        var_refs: &HashMap<String, Option<RefExpr>>,
    ) -> Option<RefResult> {
        layer2::try_interval(pred, arg, var_refs)
    }
}
