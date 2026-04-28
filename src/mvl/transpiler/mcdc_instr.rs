//! MC/DC instrumentation: runtime types and generated code.
//!
//! When `mvl mcdc` is active the transpiler injects per-clause evaluation
//! capture for every compound boolean condition (those using `&&` / `||`).
//! After the test suite runs, the recorded observations are written to
//! `MVL_MCDC_OUT` and read back by `cmd_mcdc` for independence analysis.
//!
//! Observation encoding (u16 per test-case/decision pair):
//!   bits 0..N-1  = clause values (bit i = 1 iff clause i was true)
//!   bit  N       = decision outcome (1 = true)
//! where N = clause_count for that decision (max 8).

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
    pub fn alloc(
        &mut self,
        fn_name: String,
        file: String,
        line: u32,
        clause_count: usize,
    ) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.decisions.push(MCDCDecision {
            id,
            fn_name,
            file,
            line,
            clause_count,
        });
        id
    }

    pub fn next_id(&self) -> usize {
        self.next_id
    }
}

// ── Code generation ───────────────────────────────────────────────────────

/// Generate the `__mvl_mcdc` runtime module embedded in the test crate.
///
/// Uses `OnceLock` + `Mutex` so observations are safe across parallel tests.
pub fn emit_mcdc_preamble(n_decisions: usize) -> String {
    if n_decisions == 0 {
        return String::new();
    }
    format!(
        r#"// ── MVL MC/DC runtime ────────────────────────────────────────────────────────
#[cfg(test)]
pub mod __mvl_mcdc {{
    use std::sync::OnceLock;
    use std::sync::Mutex;
    use std::collections::HashSet;

    static OBS: OnceLock<Mutex<Vec<HashSet<u16>>>> = OnceLock::new();

    fn storage() -> &'static Mutex<Vec<HashSet<u16>>> {{
        OBS.get_or_init(|| Mutex::new(vec![HashSet::new(); {n_decisions}]))
    }}

    /// Record a single test-case observation for a decision.
    ///
    /// `encoded`: bits 0..clause_count-1 = clause values, bit clause_count = outcome.
    pub fn record(decision_id: usize, encoded: u16) {{
        if let Ok(mut guard) = storage().lock() {{
            if let Some(set) = guard.get_mut(decision_id) {{
                set.insert(encoded);
            }}
        }}
    }}

    pub fn get(decision_id: usize) -> Vec<u16> {{
        storage()
            .lock()
            .ok()
            .and_then(|g| g.get(decision_id).map(|s| s.iter().cloned().collect()))
            .unwrap_or_default()
    }}
}}

"#
    )
}

/// Generate the report test that writes observations to `MVL_MCDC_OUT`.
///
/// Each line in the output file corresponds to one decision (by id).
/// The line contains comma-separated 4-hex-digit observation values,
/// or is empty if no observations were recorded for that decision.
pub fn emit_mcdc_report_test(n_decisions: usize) -> String {
    if n_decisions == 0 {
        return String::new();
    }
    let mut s = String::new();
    s.push_str("// ── MVL MC/DC report (auto-generated) ───────────────────────────────────────\n");
    s.push_str("#[cfg(test)]\n#[test]\n");
    s.push_str("fn zzz_mvl_mcdc_report() {\n");
    s.push_str("    if let Ok(path) = std::env::var(\"MVL_MCDC_OUT\") {\n");
    s.push_str("        let mut out = String::new();\n");
    for i in 0..n_decisions {
        s.push_str(&format!(
            "        {{\
\n            let mut obs = crate::__mvl_mcdc::get({i});\
\n            obs.sort();\
\n            let enc: Vec<String> = obs.iter().map(|x| format!(\"{{:04x}}\", x)).collect();\
\n            out.push_str(&format!(\"{{}}\\n\", enc.join(\",\")));\
\n        }}\n"
        ));
    }
    s.push_str("        std::fs::write(&path, out).ok();\n");
    s.push_str("    }\n}\n");
    s
}

// ── Independence analysis ─────────────────────────────────────────────────

/// Check whether clause `clause_bit` is independently covered in the
/// given observation set.
///
/// An independence pair exists when two observations t1, t2 satisfy:
/// - bit `clause_bit` differs between t1 and t2
/// - all other clause bits are the same
/// - the outcome bit differs
pub fn is_clause_covered(clause_count: usize, clause_bit: usize, observations: &[u16]) -> bool {
    let outcome_bit = clause_count as u16;
    let clause_mask = (1u16 << clause_count) - 1;
    let other_mask = clause_mask & !(1u16 << clause_bit);

    for &t1 in observations {
        for &t2 in observations {
            let v1 = t1 & clause_mask;
            let v2 = t2 & clause_mask;
            let o1 = (t1 >> outcome_bit) & 1;
            let o2 = (t2 >> outcome_bit) & 1;

            // clause_bit must differ
            if (v1 >> clause_bit) & 1 == (v2 >> clause_bit) & 1 {
                continue;
            }
            // all other clauses must be the same
            if (v1 & other_mask) != (v2 & other_mask) {
                continue;
            }
            // outcome must differ
            if o1 != o2 {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_preamble_empty_when_zero() {
        assert_eq!(emit_mcdc_preamble(0), "");
    }

    #[test]
    fn emit_preamble_has_record_fn() {
        let s = emit_mcdc_preamble(2);
        assert!(s.contains("fn record("), "missing record fn");
        assert!(s.contains("OnceLock"), "missing OnceLock");
    }

    #[test]
    fn emit_report_test_empty_when_zero() {
        assert_eq!(emit_mcdc_report_test(0), "");
    }

    #[test]
    fn emit_report_test_has_report_fn() {
        let s = emit_mcdc_report_test(2);
        assert!(s.contains("fn zzz_mvl_mcdc_report()"), "missing report fn");
        assert!(s.contains("MVL_MCDC_OUT"), "missing env var");
    }

    #[test]
    fn independence_covered_and_b() {
        // `A && B`: observations where B independently toggles outcome
        // t1: A=1,B=1 → outcome=1 → bits: vals=0b11, outcome=1 → (0b11 | (1<<2)) = 0b111 = 7
        // t2: A=1,B=0 → outcome=0 → bits: vals=0b01, outcome=0 → 0b001 = 1
        let obs = vec![0b111u16, 0b001u16];
        // clause 1 = B, clause_count = 2
        assert!(is_clause_covered(2, 1, &obs), "B should be covered");
    }

    #[test]
    fn independence_covered_a() {
        // t1: A=1,B=1 → outcome=1 → 0b111 = 7
        // t2: A=0,B=1 → outcome=0 → 0b010 = 2  (vals=0b10, outcome=0)
        let obs = vec![0b111u16, 0b010u16];
        // clause 0 = A, clause_count = 2
        assert!(is_clause_covered(2, 0, &obs), "A should be covered");
    }

    #[test]
    fn independence_not_covered_when_no_pair() {
        // Only one observation: A=1,B=1 → outcome=1
        let obs = vec![0b111u16];
        assert!(!is_clause_covered(2, 0, &obs));
        assert!(!is_clause_covered(2, 1, &obs));
    }

    #[test]
    fn independence_not_covered_when_other_clause_varies() {
        // t1: A=1,B=1 → outcome=1 → 0b111 = 7
        // t2: A=0,B=0 → outcome=0 → 0b000 = 0
        // Both clauses change simultaneously — not an independence pair for either
        let obs = vec![0b111u16, 0b000u16];
        // A: bit 0 differs (1 vs 0), other clause B: bit 1 differs too (1 vs 0) → fail
        assert!(!is_clause_covered(2, 0, &obs));
        assert!(!is_clause_covered(2, 1, &obs));
    }
}
