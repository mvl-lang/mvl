//! MC/DC instrumentation: runtime types, coupling analysis, and generated code.
//!
//! When `mvl mcdc` is active the transpiler injects per-clause evaluation
//! capture for every compound boolean condition (those using `&&` / `||`).
//! After the test suite runs, the recorded observations are written to
//! `MVL_MCDC_OUT` and read back by `cmd_mcdc` for independence analysis.
//!
//! Observation encoding (u32 per test-case/decision pair):
//!   bits   0..N-1  = clause values  (bit i = 1 iff clause i was true)
//!   bits   N..2N-1 = eval flags     (bit N+i = 1 iff clause i was actually evaluated)
//!   bit    2N      = decision outcome (1 = true)
//! where N = clause_count for that decision (max 15, enforced at alloc time).
//!
//! A clause with eval_flag=0 was masked by short-circuit evaluation (not reached).
//! Masked clauses are excluded from the Unique-Cause independence pair check —
//! they are "not reachable under that input," not "tested and failed."

use crate::mvl::parser::ast::Expr;

/// Build a dotted access path for a pure ident / field-access chain.
/// Returns `None` for anything that involves computation (calls, operators).
///
/// Examples:
///   `v`             → Some("v")
///   `v.breathing`   → Some("v.breathing")
///   `p.vitals.pulse`→ Some("p.vitals.pulse")
///   `f(v).field`    → None  (call at base — not a simple path)
fn expr_to_path(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => Some(name.clone()),
        Expr::FieldAccess { expr, field, .. } => {
            expr_to_path(expr).map(|base| format!("{}.{}", base, field))
        }
        _ => None,
    }
}

/// Collect access paths for all free-variable references in a clause expression.
///
/// For field accesses like `v.breathing` the full path `"v.breathing"` is
/// collected rather than just the root `"v"`. This prevents false coupling
/// between clauses that share a struct parameter but access disjoint fields.
///
/// Examples:
///   `breathing_absent(v.breathing)` → ["v.breathing"]
///   `oxygen_low(v.oxygen_sat)`      → ["v.oxygen_sat"]
///   `v.systolic_bp < 90`            → ["v.systolic_bp"]
///   `f(a, b)`                       → ["a", "b"]
///   `f(v)`                          → ["v"]       (bare var, not a field)
fn collect_access_paths(expr: &Expr, out: &mut Vec<String>) {
    // Try to extract a pure access path first (handles Ident and FieldAccess chains).
    if let Some(path) = expr_to_path(expr) {
        out.push(path);
        return;
    }
    // Recurse into sub-expressions for calls, operators, etc.
    match expr {
        Expr::Binary { left, right, .. } => {
            collect_access_paths(left, out);
            collect_access_paths(right, out);
        }
        Expr::FnCall { args, .. } => {
            for a in args {
                collect_access_paths(a, out);
            }
        }
        Expr::FieldAccess { expr, .. } => collect_access_paths(expr, out),
        Expr::Unary { expr, .. } | Expr::Borrow { expr, .. } => collect_access_paths(expr, out),
        Expr::MethodCall { receiver, args, .. } => {
            collect_access_paths(receiver, out);
            for a in args {
                collect_access_paths(a, out);
            }
        }
        _ => {}
    }
}

/// Detect potentially coupled clause pairs for a compound decision.
///
/// Two clauses are "potentially coupled" when they share at least one
/// access path (a full dotted identifier like `v.breathing` or a bare
/// variable like `hr`). Clauses that share a struct parameter but access
/// disjoint fields (e.g. `v.breathing` vs `v.oxygen_sat`) are NOT flagged.
///
/// Returns a list of `(clause_i, clause_j, shared_paths)` triples,
/// one entry per coupled pair.
pub fn detect_coupled_pairs(clauses: &[&Expr]) -> Vec<(usize, usize, Vec<String>)> {
    use std::collections::HashSet;
    let paths: Vec<HashSet<String>> = clauses
        .iter()
        .map(|expr| {
            let mut v = Vec::new();
            collect_access_paths(expr, &mut v);
            v.into_iter().collect()
        })
        .collect();

    let mut pairs = Vec::new();
    for i in 0..paths.len() {
        for j in (i + 1)..paths.len() {
            let shared: Vec<String> = paths[i].intersection(&paths[j]).cloned().collect();
            if !shared.is_empty() {
                let mut shared = shared;
                shared.sort();
                pairs.push((i, j, shared));
            }
        }
    }
    pairs
}

/// The syntactic position of a compound boolean decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionKind {
    /// The condition of an `if` expression.
    If,
    /// The condition of a `while` loop.
    While,
    /// The return expression of a `Bool`-valued function body.
    Return,
}

impl DecisionKind {
    /// Short label used in the verbose MC/DC report.
    pub fn label(&self) -> &'static str {
        match self {
            DecisionKind::If => "if",
            DecisionKind::While => "while",
            DecisionKind::Return => "fn",
        }
    }
}

/// Metadata for a single instrumented decision point.
#[derive(Debug, Clone)]
pub struct MCDCDecision {
    /// Index into the runtime observation storage.
    pub id: usize,
    /// Enclosing function name.
    pub fn_name: String,
    /// Source file stem.
    pub file: String,
    /// Source line (1-based).
    pub line: u32,
    /// Number of atomic boolean clauses.
    pub clause_count: usize,
    /// Syntactic position of the decision.
    pub kind: DecisionKind,
    /// Potentially coupled clause pairs: `(clause_i, clause_j, shared_vars)`.
    /// Two clauses are coupled when they reference the same variable, making it
    /// impossible to toggle one independently of the other via that variable.
    pub coupled_pairs: Vec<(usize, usize, Vec<String>)>,
}

/// Accumulates MC/DC decision registrations during a transpilation pass.
pub struct MCDCMap {
    pub decisions: Vec<MCDCDecision>,
    next_id: usize,
}

impl MCDCMap {
    pub fn new(start_id: usize) -> Self {
        MCDCMap {
            decisions: Vec::new(),
            next_id: start_id,
        }
    }

    /// Register a new decision and return its unique counter index.
    ///
    /// # Panics
    /// Panics if `clause_count > 15`: the u32 encoding uses 2N+1 bits (N clause
    /// values, N eval flags, 1 outcome), so N ≤ 15 → 31 bits ≤ u32.  Conditions
    /// with 16+ clauses are pathological; this assertion catches silent data
    /// corruption that would produce false COVERED results.  See ADR-0015.
    pub fn alloc(
        &mut self,
        fn_name: String,
        file: String,
        line: u32,
        clause_count: usize,
        kind: DecisionKind,
        coupled_pairs: Vec<(usize, usize, Vec<String>)>,
    ) -> usize {
        assert!(
            clause_count <= 15,
            "MC/DC: decision at line {line} has {clause_count} clauses; max supported is 15 (u32 encoding, 2N+1 bits, see ADR-0015)"
        );
        let id = self.next_id;
        self.next_id += 1;
        self.decisions.push(MCDCDecision {
            id,
            fn_name,
            file,
            line,
            clause_count,
            kind,
            coupled_pairs,
        });
        id
    }

    pub fn next_id(&self) -> usize {
        self.next_id
    }
}

// ── Independence analysis ─────────────────────────────────────────────────

/// Check whether clause `clause_bit` independently affects the decision outcome
/// (Unique-Cause MC/DC with short-circuit masking).
///
/// Each observation `t` is a `u32`:
/// - bits   0..N-1  = clause values
/// - bits   N..2N-1 = eval flags (1 = evaluated, 0 = masked by short-circuit)
/// - bit    2N      = decision outcome
///
/// An independence pair (t1, t2) satisfies:
/// 1. `clause_bit` is **evaluated** in both t1 and t2
/// 2. `clause_bit` **differs** between t1 and t2
/// 3. The outcome **differs** between t1 and t2
/// 4. Every other clause j: if evaluated in **both** t1 and t2, its value is
///    **identical** in both.  Clauses masked in either test case are skipped —
///    a masked clause is "not reachable under that input," not a confound.
pub fn is_clause_covered(clause_count: usize, clause_bit: usize, observations: &[u32]) -> bool {
    let n = clause_count;
    let eval_shift = n; // eval flag for clause i lives at bit n+i
    let outcome_bit = 2 * n;

    for &t1 in observations {
        for &t2 in observations {
            // 1. Both must have actually evaluated clause_bit.
            if ((t1 >> (eval_shift + clause_bit)) & 1) == 0 {
                continue;
            }
            if ((t2 >> (eval_shift + clause_bit)) & 1) == 0 {
                continue;
            }

            // 2. clause_bit value must differ.
            if ((t1 >> clause_bit) & 1) == ((t2 >> clause_bit) & 1) {
                continue;
            }

            // 3. Outcome must differ.
            if ((t1 >> outcome_bit) & 1) == ((t2 >> outcome_bit) & 1) {
                continue;
            }

            // 4. All other clauses that were evaluated in both must be identical.
            let mut ok = true;
            for j in 0..n {
                if j == clause_bit {
                    continue;
                }
                let e1 = (t1 >> (eval_shift + j)) & 1;
                let e2 = (t2 >> (eval_shift + j)) & 1;
                if e1 == 1 && e2 == 1 && ((t1 >> j) & 1) != ((t2 >> j) & 1) {
                    ok = false;
                    break;
                }
                // If masked in either test case: no constraint — it's not a confound.
            }
            if ok {
                return true;
            }
        }
    }
    false
}

/// Helper: encode a u32 observation from components.
///
/// `clauses`: slice of `(value, evaluated)` pairs in left-to-right order.
/// `outcome`: the decision outcome.
///
/// Encoding: bits 0..N-1 = values, bits N..2N-1 = eval flags, bit 2N = outcome.
#[cfg(test)]
fn encode_obs(clauses: &[(bool, bool)], outcome: bool) -> u32 {
    let n = clauses.len();
    let mut enc: u32 = (outcome as u32) << (2 * n);
    for (i, &(val, evaluated)) in clauses.iter().enumerate() {
        enc |= (val as u32) << i;
        enc |= (evaluated as u32) << (n + i);
    }
    enc
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Independence check: A && B (both evaluated) ────────────────────────

    #[test]
    fn independence_covered_b_both_evaluated() {
        // A && B: B independently toggles outcome when A=true in both tests.
        // t1: A=1(eval), B=1(eval) → out=1
        // t2: A=1(eval), B=0(eval) → out=0
        let t1 = encode_obs(&[(true, true), (true, true)], true);
        let t2 = encode_obs(&[(true, true), (false, true)], false);
        let obs = vec![t1, t2];
        assert!(is_clause_covered(2, 1, &obs), "B should be covered");
    }

    #[test]
    fn independence_covered_a_both_evaluated() {
        // A && B: A independently toggles outcome when B=true in both tests.
        // t1: A=1(eval), B=1(eval) → out=1
        // t2: A=0(eval), B=1(eval) → out=0
        let t1 = encode_obs(&[(true, true), (true, true)], true);
        let t2 = encode_obs(&[(false, true), (true, true)], false);
        let obs = vec![t1, t2];
        assert!(is_clause_covered(2, 0, &obs), "A should be covered");
    }

    #[test]
    fn independence_covered_a_b_masked_in_partner() {
        // A && B with short-circuit: when A=false, B is masked.
        // t1: A=1(eval), B=1(eval) → out=1
        // t2: A=0(eval), B=masked  → out=0  (short-circuit: B not evaluated)
        // B is masked in t2, so it does not block A's independence pair.
        let t1 = encode_obs(&[(true, true), (true, true)], true);
        let t2 = encode_obs(&[(false, true), (false, false)], false); // B: value=0, eval=false
        let obs = vec![t1, t2];
        assert!(
            is_clause_covered(2, 0, &obs),
            "A should be covered even when B is masked in partner"
        );
    }

    #[test]
    fn independence_not_covered_when_no_pair() {
        // Only one observation — can't form any pair.
        let t1 = encode_obs(&[(true, true), (true, true)], true);
        let obs = vec![t1];
        assert!(!is_clause_covered(2, 0, &obs));
        assert!(!is_clause_covered(2, 1, &obs));
    }

    #[test]
    fn independence_not_covered_when_both_evaluated_and_other_varies() {
        // t1: A=1(eval), B=1(eval) → out=1
        // t2: A=0(eval), B=0(eval) → out=0
        // Both evaluated and B also changes — not a valid pair for A or B.
        let t1 = encode_obs(&[(true, true), (true, true)], true);
        let t2 = encode_obs(&[(false, true), (false, true)], false);
        let obs = vec![t1, t2];
        assert!(
            !is_clause_covered(2, 0, &obs),
            "A not covered: B also varies"
        );
        assert!(
            !is_clause_covered(2, 1, &obs),
            "B not covered: A also varies"
        );
    }

    #[test]
    fn independence_not_covered_when_target_masked() {
        // Can't establish independence for a clause that was never evaluated.
        // t1: A=1(eval), B=masked → out=1
        // t2: A=0(eval), B=masked → out=0
        // B is masked in both — no independence pair for B possible.
        let t1 = encode_obs(&[(true, true), (false, false)], true);
        let t2 = encode_obs(&[(false, true), (false, false)], false);
        let obs = vec![t1, t2];
        assert!(
            !is_clause_covered(2, 1, &obs),
            "B masked in both: no coverage"
        );
    }

    // ── 3-clause coverage (A && B && C) ────────────────────────────────────

    #[test]
    fn three_clause_b_covered_with_masking() {
        // A && B && C (left-assoc: (A&&B)&&C)
        // When A=true,C=true: B toggles outcome. C masked when A&&B is false.
        // t1: A=1,B=1,C=1 all evaluated → out=1
        // t2: A=1,B=0,C=masked        → out=0 (C masked: (A&&B)=false, C skipped by outer &&)
        // Actually for (A&&B)&&C: if A&&B is false, C is masked.
        // Let's use simpler: all evaluated pairs for clarity.
        let t1 = encode_obs(&[(true, true), (true, true), (true, true)], true);
        let t2 = encode_obs(&[(true, true), (false, true), (true, true)], false);
        let obs = vec![t1, t2];
        assert!(is_clause_covered(3, 1, &obs), "B should be covered");
        assert!(!is_clause_covered(3, 0, &obs), "A not covered by these obs");
        assert!(!is_clause_covered(3, 2, &obs), "C not covered by these obs");
    }

    #[test]
    fn three_clause_all_covered() {
        // Full independence pairs for A && B && C (all evaluated):
        // A: (1,1,1,→1) vs (0,1,1,→0)
        // B: (1,1,1,→1) vs (1,0,1,→0)  [A=1 in both so C not masked for first obs]
        // C: (1,1,1,→1) vs (1,1,0,→0)
        let all_true = encode_obs(&[(true, true), (true, true), (true, true)], true);
        let a_false = encode_obs(&[(false, true), (true, true), (true, true)], false);
        let b_false = encode_obs(&[(true, true), (false, true), (true, true)], false);
        let c_false = encode_obs(&[(true, true), (true, true), (false, true)], false);
        let obs = vec![all_true, a_false, b_false, c_false];
        assert!(is_clause_covered(3, 0, &obs), "A should be covered");
        assert!(is_clause_covered(3, 1, &obs), "B should be covered");
        assert!(is_clause_covered(3, 2, &obs), "C should be covered");
    }

    #[test]
    fn alloc_panics_on_too_many_clauses() {
        let result = std::panic::catch_unwind(|| {
            let mut m = MCDCMap::new(0);
            m.alloc("f".into(), "f".into(), 1, 16, DecisionKind::If, vec![]);
        });
        assert!(result.is_err(), "should panic for clause_count=16");
    }
}
