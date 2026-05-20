// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

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

use std::collections::{HashMap, HashSet};

use crate::mvl::parser::ast::{Block, Decl, ElseBranch, Expr, MatchBody, Program, Stmt};

/// Maps function name → per-parameter field-path sets.
///
/// Each inner `Vec` is indexed by parameter position.  Every `String` in the
/// inner `HashSet` is a full dotted access path as it appears in the function
/// body (e.g. `"p.color"`, `"p.vitals.pulse"`).  A bare parameter name
/// without a field selector (e.g. `"p"`) indicates the parameter is used
/// without field decomposition — callers should fall back to conservative
/// (syntactic) coupling in that case.
///
/// Built from the current program's source by [`build_fn_field_reads`].
/// External / cross-file functions are absent from the map; callers treat
/// that as a signal to use conservative coupling behaviour.
pub type FnFieldReads = HashMap<String, Vec<HashSet<String>>>;

/// Walk every function in `prog` and collect, for each parameter, the set of
/// dotted access paths (field reads) that appear anywhere in the function body.
///
/// The traversal is purely syntactic — it records every `Ident` or
/// `FieldAccess` chain that `expr_to_path` can extract.  Paths are kept if
/// their root matches a parameter name.
pub fn build_fn_field_reads(prog: &Program) -> FnFieldReads {
    let mut result = FnFieldReads::new();
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            let mut all_paths: Vec<String> = Vec::new();
            collect_paths_from_block(&fd.body, &mut all_paths);

            let mut param_reads: Vec<HashSet<String>> =
                fd.params.iter().map(|_| HashSet::new()).collect();

            for path in all_paths {
                for (i, param) in fd.params.iter().enumerate() {
                    let root = &param.name;
                    if path == *root || path.starts_with(&format!("{}.", root)) {
                        param_reads[i].insert(path.clone());
                    }
                }
            }
            result.insert(fd.name.clone(), param_reads);
        }
    }
    result
}

fn collect_paths_from_block(block: &Block, out: &mut Vec<String>) {
    for stmt in &block.stmts {
        collect_paths_from_stmt(stmt, out);
    }
}

fn collect_paths_from_stmt(stmt: &Stmt, out: &mut Vec<String>) {
    match stmt {
        Stmt::Let { init, .. } => collect_paths_from_expr(init, out),
        Stmt::Assign { value, .. } => collect_paths_from_expr(value, out),
        Stmt::Return { value: Some(e), .. } => collect_paths_from_expr(e, out),
        Stmt::Return { value: None, .. } => {}
        Stmt::Expr { expr, .. } => collect_paths_from_expr(expr, out),
        Stmt::If {
            cond, then, else_, ..
        } => {
            collect_paths_from_expr(cond, out);
            collect_paths_from_block(then, out);
            if let Some(eb) = else_ {
                match eb {
                    ElseBranch::Block(b) => collect_paths_from_block(b, out),
                    ElseBranch::If(s) => collect_paths_from_stmt(s, out),
                }
            }
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            collect_paths_from_expr(scrutinee, out);
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => collect_paths_from_block(b, out),
                    MatchBody::Expr(e) => collect_paths_from_expr(e, out),
                }
            }
        }
        Stmt::For { iter, body, .. } => {
            collect_paths_from_expr(iter, out);
            collect_paths_from_block(body, out);
        }
        Stmt::While { cond, body, .. } => {
            collect_paths_from_expr(cond, out);
            collect_paths_from_block(body, out);
        }
    }
}

fn collect_paths_from_expr(expr: &Expr, out: &mut Vec<String>) {
    // `expr_to_path` extracts the full dotted path for pure ident / field-access chains.
    // Return early — don't recurse further so sub-paths aren't double-counted.
    if let Some(path) = expr_to_path(expr) {
        out.push(path);
        return;
    }
    match expr {
        Expr::Binary { left, right, .. } => {
            collect_paths_from_expr(left, out);
            collect_paths_from_expr(right, out);
        }
        Expr::Unary { expr: e, .. }
        | Expr::Borrow { expr: e, .. }
        | Expr::Propagate { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Relabel { expr: e, .. } => collect_paths_from_expr(e, out),
        Expr::FnCall { args, .. } => {
            for a in args {
                collect_paths_from_expr(a, out);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_paths_from_expr(receiver, out);
            for a in args {
                collect_paths_from_expr(a, out);
            }
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            collect_paths_from_expr(cond, out);
            collect_paths_from_block(then, out);
            if let Some(e) = else_ {
                collect_paths_from_expr(e, out);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_paths_from_expr(scrutinee, out);
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => collect_paths_from_block(b, out),
                    MatchBody::Expr(e) => collect_paths_from_expr(e, out),
                }
            }
        }
        Expr::Construct { fields, .. } => {
            for (_, e) in fields {
                collect_paths_from_expr(e, out);
            }
        }
        Expr::Lambda { body, .. } => collect_paths_from_expr(body, out),
        Expr::Block(block) => collect_paths_from_block(block, out),
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                collect_paths_from_expr(e, out);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                collect_paths_from_expr(k, out);
                collect_paths_from_expr(v, out);
            }
        }
        // Literal, Ident already handled by expr_to_path above.
        _ => {}
    }
}

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
/// When `fn_field_reads` is provided, call sites of locally-defined functions
/// are resolved interprocedurally: a bare-variable argument `f(p)` becomes the
/// set of field paths that `f` actually reads on `p` (e.g. `["p.color"]`),
/// so two clauses `f(p)` and `g(p)` that read disjoint fields are **not**
/// reported as coupled.  Functions absent from the map fall back to the
/// conservative (syntactic) behaviour — the bare argument path is used.
///
/// Examples (without interprocedural reads):
///   `breathing_absent(v.breathing)` → ["v.breathing"]
///   `oxygen_low(v.oxygen_sat)`      → ["v.oxygen_sat"]
///   `v.systolic_bp < 90`            → ["v.systolic_bp"]
///   `f(a, b)`                       → ["a", "b"]
///   `f(v)`                          → ["v"]       (bare var, not a field)
fn collect_access_paths(expr: &Expr, out: &mut Vec<String>, fn_field_reads: Option<&FnFieldReads>) {
    // Try to extract a pure access path first (handles Ident and FieldAccess chains).
    if let Some(path) = expr_to_path(expr) {
        out.push(path);
        return;
    }
    // Recurse into sub-expressions for calls, operators, etc.
    match expr {
        Expr::Binary { left, right, .. } => {
            collect_access_paths(left, out, fn_field_reads);
            collect_access_paths(right, out, fn_field_reads);
        }
        Expr::FnCall { name, args, .. } => {
            // Interprocedural resolution: when the callee is defined in the same
            // compilation unit and all its parameter accesses are field-level,
            // replace the bare-var argument with the resolved field paths.
            if let Some(param_reads) = fn_field_reads.and_then(|m| m.get(name.as_str())) {
                if args.len() == param_reads.len() {
                    for (i, arg) in args.iter().enumerate() {
                        let fields = &param_reads[i];
                        if let Some(arg_path) = expr_to_path(arg) {
                            // Only refine when the param is accessed exclusively through
                            // field selectors (no bare usage).  If the set contains a
                            // bare param name (no '.') the callee may forward the whole
                            // value, so fall back to the conservative arg path.
                            if !fields.is_empty() && fields.iter().all(|p| p.contains('.')) {
                                for field_path in fields {
                                    let dot = field_path.find('.').unwrap();
                                    out.push(format!("{}{}", arg_path, &field_path[dot..]));
                                }
                            } else {
                                out.push(arg_path);
                            }
                        } else {
                            collect_access_paths(arg, out, fn_field_reads);
                        }
                    }
                    return;
                }
            }
            // Conservative fallback: recurse into all arguments.
            for a in args {
                collect_access_paths(a, out, fn_field_reads);
            }
        }
        Expr::FieldAccess { expr, .. } => collect_access_paths(expr, out, fn_field_reads),
        Expr::Unary { expr, .. } | Expr::Borrow { expr, .. } => {
            collect_access_paths(expr, out, fn_field_reads)
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_access_paths(receiver, out, fn_field_reads);
            for a in args {
                collect_access_paths(a, out, fn_field_reads);
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
/// When `fn_field_reads` is `Some`, call sites of locally-defined functions
/// are resolved interprocedurally before the overlap check — see
/// [`collect_access_paths`] for details.  Pass `None` to use purely
/// syntactic (conservative) coupling.
///
/// Returns a list of `(clause_i, clause_j, shared_paths)` triples,
/// one entry per coupled pair.
pub fn detect_coupled_pairs(
    clauses: &[&Expr],
    fn_field_reads: Option<&FnFieldReads>,
) -> Vec<(usize, usize, Vec<String>)> {
    let paths: Vec<HashSet<String>> = clauses
        .iter()
        .map(|expr| {
            let mut v = Vec::new();
            collect_access_paths(expr, &mut v, fn_field_reads);
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
    /// A `match` statement/expression — each arm is an independent outcome.
    /// Observations are encoded as the arm index (u32), not the 2N+1 bit scheme.
    Match,
    /// A compound guard condition (`if cond` with `&&`/`||`) on a match arm.
    /// Uses the standard 2N+1 bit observation encoding like `If`/`While`.
    MatchGuard,
}

impl DecisionKind {
    /// Short label used in the verbose MC/DC report.
    pub fn label(&self) -> &'static str {
        match self {
            DecisionKind::If => "if",
            DecisionKind::While => "while",
            DecisionKind::Return => "return",
            DecisionKind::Match => "match",
            DecisionKind::MatchGuard => "guard",
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
        // Match arm decisions use arm-index encoding (not 2N+1 bits), so they
        // are not subject to the 15-clause limit.  All other kinds (If, While,
        // Return, MatchGuard) use the 2N+1 bit u32 encoding — limit enforced.
        if !matches!(kind, DecisionKind::Match) {
            assert!(
                clause_count <= 15,
                "MC/DC: decision at line {line} has {clause_count} clauses; max supported is 15 (u32 encoding, 2N+1 bits, see ADR-0015)"
            );
        }
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

/// Check whether match arm `arm_index` was exercised in at least one test.
///
/// Match observations are encoded as the plain arm index (u32), unlike the
/// 2N+1 bit scheme used for `If`/`While`/`MatchGuard` decisions.  Coverage
/// is satisfied when any recorded observation equals the arm index.
pub fn is_match_arm_covered(arm_index: usize, observations: &[u32]) -> bool {
    observations.contains(&(arm_index as u32))
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
