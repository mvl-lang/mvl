//! Layer 4 — Cooper's algorithm for Presburger arithmetic.
//!
//! Handles refinement predicates that Layers 1–3 cannot decide by applying
//! quantifier elimination over linear integer arithmetic.
//!
//! | Pattern                     | Example                                       |
//! |-----------------------------|-----------------------------------------------|
//! | Linear-expr argument        | `a - b` with hyp `b < a` proves `self > 0`   |
//! | Divisibility (always ≠ 0)   | `2*x + 1 ≠ 0` — odd number, never zero       |
//! | Variable-to-variable bounds | `a + b > c` given `a > c && b > 0`           |
//!
//! # Algorithm
//!
//! 1. Extract a [`LinTerm`] (linear integer expression) from `arg`.
//! 2. Convert `¬pred` (with `self → LinTerm`) and `var_refs` hypotheses into a
//!    conjunction of [`Constraint`]s.
//! 3. Check unsatisfiability:
//!    - **Divisibility**: `a*x = b` is UNSAT iff `a ∤ b`.
//!    - **Fourier-Motzkin**: eliminate free variables; UNSAT if a constant
//!      contradiction `c > 0` (stored as `c ≤ 0`) is derived.
//! 4. Return `Some(Proven)` if UNSAT; `None` otherwise (conservative).
//!
//! Complexity guard: returns `None` immediately for > 5 free variables.

use std::collections::{HashMap, HashSet};

use crate::mvl::parser::ast::{ArithOp, BinaryOp, CmpOp, Expr, Literal, LogicOp, RefExpr, UnaryOp};

use super::RefResult;

// ── Linear term ───────────────────────────────────────────────────────────────

/// A linear integer expression: `constant + Σ (coeff_i · var_i)`.
///
/// Represents expressions like `3`, `x`, `a − b`, `2·x + 1`.
#[derive(Debug, Clone, PartialEq)]
struct LinTerm {
    constant: i64,
    vars: HashMap<String, i64>,
}

impl LinTerm {
    fn constant(n: i64) -> Self {
        Self {
            constant: n,
            vars: HashMap::new(),
        }
    }

    fn var(name: impl Into<String>) -> Self {
        let mut vars = HashMap::new();
        vars.insert(name.into(), 1);
        Self { constant: 0, vars }
    }

    fn add(&self, other: &LinTerm) -> LinTerm {
        let mut vars = self.vars.clone();
        for (k, v) in &other.vars {
            let entry = vars.entry(k.clone()).or_insert(0);
            *entry += v;
            if *entry == 0 {
                vars.remove(k);
            }
        }
        LinTerm {
            constant: self.constant + other.constant,
            vars,
        }
    }

    fn sub(&self, other: &LinTerm) -> LinTerm {
        self.add(&other.negate())
    }

    fn scale(&self, c: i64) -> LinTerm {
        if c == 0 {
            return LinTerm::constant(0);
        }
        LinTerm {
            constant: self.constant * c,
            vars: self.vars.iter().map(|(k, v)| (k.clone(), v * c)).collect(),
        }
    }

    fn negate(&self) -> LinTerm {
        self.scale(-1)
    }

    fn is_constant(&self) -> bool {
        self.vars.is_empty()
    }

    fn coeff_of(&self, var: &str) -> i64 {
        self.vars.get(var).copied().unwrap_or(0)
    }

    /// Return a copy with the given variable removed.
    fn without_var(&self, var: &str) -> LinTerm {
        let mut t = self.clone();
        t.vars.remove(var);
        t
    }

    fn free_vars(&self) -> impl Iterator<Item = &String> {
        self.vars.keys()
    }
}

// ── Constraint ────────────────────────────────────────────────────────────────

/// A linear constraint: `term OP 0`.
#[derive(Debug, Clone)]
enum Constraint {
    /// `term ≤ 0`
    Le(LinTerm),
    /// `term = 0`
    Eq(LinTerm),
    /// `term ≠ 0`
    Ne(LinTerm),
}

impl Constraint {
    fn is_trivially_false(&self) -> bool {
        match self {
            Constraint::Le(t) => t.is_constant() && t.constant > 0,
            Constraint::Eq(t) => t.is_constant() && t.constant != 0,
            Constraint::Ne(t) => t.is_constant() && t.constant == 0,
        }
    }

    fn is_trivially_true(&self) -> bool {
        match self {
            Constraint::Le(t) => t.is_constant() && t.constant <= 0,
            Constraint::Eq(t) => t.is_constant() && t.constant == 0,
            Constraint::Ne(t) => t.is_constant() && t.constant != 0,
        }
    }
}

// ── Extract LinTerm from Expr ─────────────────────────────────────────────────

/// Try to extract a linear integer term from an `Expr`.
///
/// Returns `None` for non-linear expressions (function calls, field access,
/// variable × variable, etc.).
fn linterm_from_expr(expr: &Expr) -> Option<LinTerm> {
    match expr {
        Expr::Literal(Literal::Integer(n), _) => Some(LinTerm::constant(*n)),
        Expr::Ident(name, _) => Some(LinTerm::var(name.as_str())),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
            ..
        } => Some(linterm_from_expr(inner)?.negate()),
        Expr::Binary {
            op, left, right, ..
        } => {
            let l = linterm_from_expr(left)?;
            let r = linterm_from_expr(right)?;
            match op {
                BinaryOp::Add => Some(l.add(&r)),
                BinaryOp::Sub => Some(l.sub(&r)),
                BinaryOp::Mul => {
                    // Linear iff one side is a constant scalar.
                    if l.is_constant() {
                        Some(r.scale(l.constant))
                    } else if r.is_constant() {
                        Some(l.scale(r.constant))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

// ── Extract LinTerm from RefExpr ──────────────────────────────────────────────

/// Extract a linear integer term from a `RefExpr` arithmetic sub-expression.
///
/// `self_term` is substituted for `"self"` (and `self_name` if they differ).
fn linterm_from_ref(expr: &RefExpr, self_term: &LinTerm, self_name: &str) -> Option<LinTerm> {
    match expr {
        RefExpr::Integer { value, .. } => Some(LinTerm::constant(*value)),
        RefExpr::Ident { name, .. } => {
            if name == "self" || name == self_name {
                Some(self_term.clone())
            } else {
                Some(LinTerm::var(name.as_str()))
            }
        }
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let l = linterm_from_ref(left, self_term, self_name)?;
            let r = linterm_from_ref(right, self_term, self_name)?;
            match op {
                ArithOp::Add => Some(l.add(&r)),
                ArithOp::Sub => Some(l.sub(&r)),
                ArithOp::Mul => {
                    if l.is_constant() {
                        Some(r.scale(l.constant))
                    } else if r.is_constant() {
                        Some(l.scale(r.constant))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        RefExpr::Grouped { inner, .. } => linterm_from_ref(inner, self_term, self_name),
        _ => None,
    }
}

// ── RefExpr → Constraints ─────────────────────────────────────────────────────

/// Convert a `RefExpr` predicate to a list of constraints (conjunction).
///
/// `negate = true` converts `¬pred` instead of `pred`.
/// Returns `None` if the predicate is not Presburger-linear (e.g. contains
/// disjunctions, float arithmetic, array lengths).
fn ref_to_constraints(
    pred: &RefExpr,
    self_term: &LinTerm,
    self_name: &str,
    negate: bool,
) -> Option<Vec<Constraint>> {
    match pred {
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let l = linterm_from_ref(left, self_term, self_name)?;
            let r = linterm_from_ref(right, self_term, self_name)?;
            let diff = l.sub(&r); // (left − right) OP 0
            let effective_op = if negate { negate_cmp(*op) } else { *op };
            Some(cmp_to_constraints(effective_op, diff))
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => {
            // De Morgan when negated: ¬(A∧B) = ¬A∨¬B; ¬(A∨B) = ¬A∧¬B
            let effective_op = if negate {
                match op {
                    LogicOp::And => LogicOp::Or,
                    LogicOp::Or => LogicOp::And,
                }
            } else {
                *op
            };
            let (left_neg, right_neg) = (negate, negate);
            match effective_op {
                LogicOp::And => {
                    let mut cs = ref_to_constraints(left, self_term, self_name, left_neg)?;
                    cs.extend(ref_to_constraints(right, self_term, self_name, right_neg)?);
                    Some(cs)
                }
                LogicOp::Or => {
                    // Disjunctions require case-splitting; return None (conservative).
                    None
                }
            }
        }
        RefExpr::Not { inner, .. } => ref_to_constraints(inner, self_term, self_name, !negate),
        RefExpr::Grouped { inner, .. } => ref_to_constraints(inner, self_term, self_name, negate),
        // Float literals, Len, bare Ident booleans: not Presburger-linear.
        _ => None,
    }
}

fn negate_cmp(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Eq => CmpOp::Ne,
        CmpOp::Ne => CmpOp::Eq,
        CmpOp::Lt => CmpOp::Ge,
        CmpOp::Ge => CmpOp::Lt,
        CmpOp::Le => CmpOp::Gt,
        CmpOp::Gt => CmpOp::Le,
    }
}

/// Convert `(left − right) OP 0` to `Constraint`s.
///
/// Strict integer inequalities are tightened:
/// - `t < 0` ↔ `t + 1 ≤ 0` (integer: t < 0 iff t ≤ −1 iff t+1 ≤ 0)
/// - `t > 0` ↔ `−t + 1 ≤ 0` (integer: t > 0 iff t ≥ 1 iff −t ≤ −1 iff −t+1 ≤ 0)
fn cmp_to_constraints(op: CmpOp, term: LinTerm) -> Vec<Constraint> {
    match op {
        CmpOp::Le => vec![Constraint::Le(term)],
        CmpOp::Lt => {
            let mut t = term;
            t.constant += 1; // t < 0  →  t+1 ≤ 0
            vec![Constraint::Le(t)]
        }
        CmpOp::Ge => vec![Constraint::Le(term.negate())], // t ≥ 0  →  −t ≤ 0
        CmpOp::Gt => {
            let mut neg = term.negate();
            neg.constant += 1; // t > 0  →  −t+1 ≤ 0
            vec![Constraint::Le(neg)]
        }
        CmpOp::Eq => vec![Constraint::Eq(term)],
        CmpOp::Ne => vec![Constraint::Ne(term)],
    }
}

// ── Hypothesis extraction ─────────────────────────────────────────────────────

/// Collect `Constraint`s from `var_refs` hypotheses.
///
/// For each variable `v` with hypothesis `H(self)`, substitutes `self → v`
/// and converts to constraints. Non-linear hypotheses are silently skipped
/// (conservative: we lose precision but never unsoundly prove things).
fn hyp_constraints(var_refs: &HashMap<String, Option<RefExpr>>) -> Vec<Constraint> {
    let mut all = Vec::new();
    for (var_name, hyp_opt) in var_refs {
        if let Some(hyp) = hyp_opt {
            let self_term = LinTerm::var(var_name.as_str());
            if let Some(cs) = ref_to_constraints(hyp, &self_term, var_name, false) {
                all.extend(cs);
            }
        }
    }
    all
}

// ── Unsatisfiability check ────────────────────────────────────────────────────

/// Return `true` if the conjunction of constraints is unsatisfiable over ℤ.
fn is_unsat(constraints: Vec<Constraint>) -> bool {
    // Trivially false?
    if constraints.iter().any(|c| c.is_trivially_false()) {
        return true;
    }

    let constraints: Vec<Constraint> = constraints
        .into_iter()
        .filter(|c| !c.is_trivially_true())
        .collect();

    // Equality + divisibility check: a·x + c = 0 is UNSAT iff a ∤ c (no integer solution).
    for c in &constraints {
        if let Constraint::Eq(t) = c {
            if t.vars.len() == 1 {
                let (_name, &coeff) = t.vars.iter().next().unwrap();
                if t.constant % coeff != 0 {
                    return true;
                }
            }
        }
    }

    // Collect ≤ constraints for Fourier-Motzkin.
    let le_terms: Vec<LinTerm> = constraints
        .into_iter()
        .filter_map(|c| {
            if let Constraint::Le(t) = c {
                Some(t)
            } else {
                None
            }
        })
        .collect();

    if le_terms.is_empty() {
        return false;
    }

    // Collect free variables, check complexity guard.
    let free_vars: Vec<String> = le_terms
        .iter()
        .flat_map(|t| t.free_vars().cloned())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .into_iter()
        .collect();

    let mut sorted_vars = free_vars;
    sorted_vars.sort();

    if sorted_vars.len() > 5 {
        return false; // Too complex; fall through to RuntimeCheck.
    }

    fm_eliminate(le_terms, &sorted_vars)
}

/// Fourier-Motzkin elimination: eliminate variables one by one.
///
/// All constraints have the form `term ≤ 0`.  Returns `true` if UNSAT.
fn fm_eliminate(constraints: Vec<LinTerm>, vars: &[String]) -> bool {
    // Base: check for constant contradiction.
    if constraints
        .iter()
        .any(|t| t.is_constant() && t.constant > 0)
    {
        return true;
    }
    if vars.is_empty() {
        return false;
    }

    let var = &vars[0];
    let rest = &vars[1..];

    // Partition by sign of coeff(var).
    //   upper: coeff > 0  →  c·var + r ≤ 0  →  upper bound on var
    //   lower: coeff < 0  →  −b·var + s ≤ 0  →  lower bound on var  (b = |coeff| > 0)
    //   free:  coeff = 0  →  no information about var
    let mut uppers: Vec<(i64, LinTerm)> = Vec::new();
    let mut lowers: Vec<(i64, LinTerm)> = Vec::new();
    let mut new_constraints: Vec<LinTerm> = Vec::new();

    for t in constraints {
        let c = t.coeff_of(var);
        if c == 0 {
            new_constraints.push(t);
        } else if c > 0 {
            uppers.push((c, t.without_var(var)));
        } else {
            lowers.push((-c, t.without_var(var)));
        }
    }

    // FM: each (upper aᵢ, lower bⱼ) pair produces  bⱼ·rᵢ + aᵢ·sⱼ ≤ 0
    // where rᵢ = upper remainder and sⱼ = lower remainder.
    for (a_i, r_i) in &uppers {
        for (b_j, s_j) in &lowers {
            // Eliminate overflow risk: bail conservatively if coefficients are huge.
            if a_i.saturating_abs() > 1_000_000 || b_j.saturating_abs() > 1_000_000 {
                return false;
            }
            let new_term = r_i.scale(*b_j).add(&s_j.scale(*a_i));
            new_constraints.push(new_term);

            // Complexity guard: prevent combinatorial explosion.
            if new_constraints.len() > 128 {
                return false;
            }
        }
    }

    fm_eliminate(new_constraints, rest)
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Try to prove or refute `pred` for `arg` using Cooper's algorithm (Layer 4).
///
/// Returns `None` when the predicate is non-linear or the system is too
/// complex; the caller should fall back to `RuntimeCheck`.
pub(super) fn try_cooper(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<RefResult> {
    // Step 1: extract linear representation of the argument.
    let arg_term = linterm_from_expr(arg)?;

    // Step 2: convert ¬pred (with self → arg_term) to constraints.
    let neg_pred_cs = ref_to_constraints(pred, &arg_term, "self", /*negate=*/ true)?;

    // Step 3: collect hypothesis constraints from var_refs.
    let hyp_cs = hyp_constraints(var_refs);

    // Step 4: build conjunction and check UNSAT.
    let mut all = hyp_cs;
    all.extend(neg_pred_cs);

    if is_unsat(all) {
        Some(RefResult::Proven)
    } else {
        None
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fm_constant_contradiction() {
        // 1 ≤ 0 → UNSAT
        let cs = vec![LinTerm::constant(1)];
        assert!(fm_eliminate(cs, &[]));
    }

    #[test]
    fn fm_consistent_single_var() {
        // x ≤ 0  (satisfiable: x = 0)
        let cs = vec![LinTerm::var("x")]; // x ≤ 0
        assert!(!fm_eliminate(cs, &["x".to_string()]));
    }

    #[test]
    fn fm_eliminates_to_contradiction() {
        // b − a + 1 ≤ 0  AND  a − b ≤ 0  →  1 ≤ 0  →  UNSAT
        let t1 = LinTerm {
            constant: 1,
            vars: [("b".into(), 1i64), ("a".into(), -1i64)].into(),
        };
        let t2 = LinTerm {
            constant: 0,
            vars: [("a".into(), 1i64), ("b".into(), -1i64)].into(),
        };
        let vars = vec!["a".to_string(), "b".to_string()];
        assert!(fm_eliminate(vec![t1, t2], &vars));
    }

    #[test]
    fn divisibility_no_solution() {
        // 2·x + 1 = 0 → x = −½ → no integer solution → UNSAT
        let t = LinTerm {
            constant: 1,
            vars: [("x".into(), 2i64)].into(),
        };
        let cs = vec![Constraint::Eq(t)];
        assert!(is_unsat(cs));
    }

    #[test]
    fn divisibility_has_solution() {
        // 2·x = 4 → x = 2 → satisfiable → not UNSAT
        let t = LinTerm {
            constant: -4,
            vars: [("x".into(), 2i64)].into(),
        };
        // 2x − 4 = 0
        let cs = vec![Constraint::Eq(t)];
        assert!(!is_unsat(cs));
    }
}
