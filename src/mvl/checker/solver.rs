// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Layered refinement solver for MVL `where` predicates.
//!
//! Each layer handles an increasingly complex subset of proofs,
//! from O(1) trivial patterns to full SMT queries:
//!
//! | Layer | Module  | Technique                     | ~Coverage |
//! |-------|---------|-------------------------------|-----------|
//! | 1     | layer1  | Trivial pattern matching      | ~40%      |
//! | 2     | layer2  | Interval arithmetic           | ~60%      |
//! | 3     | layer3  | Symbolic path analysis        | ~15%      |
//! | 4     | layer4  | Presburger / Cooper's QE      | ~5%       |
//! | 5     | layer5  | Z3 SMT solver (feature = z3)  | ~remaining|

pub mod layer1;
pub mod layer2;
pub mod layer3;
pub mod layer4;
pub mod layer5;
pub(super) mod rewrite;

use std::collections::HashMap;

use crate::mvl::parser::ast::{BinaryOp, CmpOp, Expr, FnDecl, RefExpr};
use crate::mvl::parser::lexer::Span;

// ── Solver mode ───────────────────────────────────────────────────────────────

/// Controls which layers the refinement solver activates.
///
/// | Mode      | Layers active          | Typical use                     |
/// |-----------|------------------------|---------------------------------|
/// | Layered   | 1 → 2 → 3 → 4 → 5    | Default: full cascade           |
/// | Z3Only    | 5 only                 | Debug: force Z3 for every check |
/// | FastOnly  | 1 → 2 only             | Fast CI: skip symbolic/SMT      |
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SolverMode {
    /// Run all layers in order until one decides (default).
    #[default]
    Layered,
    /// Skip Layers 1–4; delegate every check directly to Z3 (feature = z3).
    Z3Only,
    /// Run only Layers 1–2 (trivial + interval); never invoke symbolic or SMT.
    FastOnly,
}

impl SolverMode {
    /// Canonical CLI string for this mode (matches `--refinement-solver=<value>`).
    pub fn as_str(self) -> &'static str {
        match self {
            SolverMode::Layered => "layered",
            SolverMode::Z3Only => "z3-only",
            SolverMode::FastOnly => "fast-only",
        }
    }
}

// ── Outcome type ──────────────────────────────────────────────────────────────

/// Three-way outcome for a single refinement predicate check at a call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefResult {
    /// The argument statically satisfies the predicate — no runtime check needed.
    Proven,
    /// Cannot be proven statically — a runtime assertion must be emitted.
    RuntimeCheck,
    /// The argument statically violates the predicate — a compile-time error.
    /// Optionally includes a counterexample extracted by Z3 (Phase 4, #627).
    Failed { counterexample: Option<String> },
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Construct a zero-span placeholder used for synthetic AST nodes in the solver.
/// These nodes are only used for proof evaluation and never appear in user-facing
/// error messages, so the span position (0,0) is acceptable.
pub(crate) fn dummy_span() -> Span {
    Span::new(0, 0, 0, 0)
}

/// Convert a `BinaryOp` comparison to the corresponding `CmpOp`, if applicable.
/// Returns `None` for non-comparison operators (arithmetic, logical, bitwise).
pub(crate) fn binary_op_to_cmp(op: BinaryOp) -> Option<CmpOp> {
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

    /// Try to prove or disprove `pred` for `arg` using Layer 3 (symbolic path analysis).
    ///
    /// Only applicable when `arg` is a call to a pure function in `fn_decls`.
    /// Returns `None` when this layer cannot make a decision.
    pub(crate) fn try_symbolic(
        pred: &RefExpr,
        arg: &Expr,
        var_refs: &HashMap<String, Option<RefExpr>>,
        fn_decls: &HashMap<String, FnDecl>,
    ) -> Option<RefResult> {
        layer3::try_symbolic(pred, arg, var_refs, fn_decls)
    }

    /// Try to prove `pred` for `arg` using Layer 4 (Cooper's Presburger QE).
    ///
    /// Handles linear-expression arguments and divisibility constraints that
    /// Layers 1–3 cannot decide.
    /// Returns `None` when the predicate is non-linear or too complex.
    pub(crate) fn try_cooper(
        pred: &RefExpr,
        arg: &Expr,
        var_refs: &HashMap<String, Option<RefExpr>>,
    ) -> Option<RefResult> {
        layer4::try_cooper(pred, arg, var_refs)
    }

    /// Try to prove `pred` for `arg` using Layer 5 (Z3 SMT solver).
    ///
    /// Only available when compiled with the `z3` feature.  Returns `Some(Proven)`
    /// when Z3 confirms the implication, `None` on unsupported constructs,
    /// timeout, or satisfiable negation.
    pub(crate) fn try_z3(
        pred: &RefExpr,
        arg: &Expr,
        var_refs: &HashMap<String, Option<RefExpr>>,
    ) -> Option<RefResult> {
        layer5::try_z3(pred, arg, var_refs)
    }
}
