// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust-specific MC/DC emission helpers.
//!
//! Generates the `__mvl_mcdc` runtime module and report test embedded in the
//! transpiled test crate.  These functions are Rust-backend-only; an
//! LLVM-backend equivalent would live in `codegen/mcdc_emit.rs`.
//!
//! # LLVM backend integration (future)
//!
//! An LLVM-side MC/DC pass would write observation records via LLVM IR (e.g.
//! using a thread-local or atomic buffer) rather than emitting Rust source.
//! The pass data (`MCDCDecision`, `MCDCMap`) from
//! `passes/mcdc/transform.rs` is already target-neutral and reusable.

/// Generate the `__mvl_mcdc` runtime module embedded in the test crate.
///
/// Uses `OnceLock` + `Mutex` so observations are safe across parallel tests.
/// Each observation is a `u32` encoding clause values, eval flags, and outcome.
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

    static OBS: OnceLock<Mutex<Vec<HashSet<u32>>>> = OnceLock::new();

    fn storage() -> &'static Mutex<Vec<HashSet<u32>>> {{
        OBS.get_or_init(|| Mutex::new(vec![HashSet::new(); {n_decisions}]))
    }}

    /// Record a single test-case observation for a decision.
    ///
    /// Encoding (u32): bits 0..N-1 = clause values, bits N..2N-1 = eval flags,
    /// bit 2N = outcome.  Eval flag i = 1 means clause i was actually evaluated;
    /// 0 means it was masked by short-circuit evaluation.
    pub fn record(decision_id: usize, encoded: u32) {{
        if let Ok(mut guard) = storage().lock() {{
            if let Some(set) = guard.get_mut(decision_id) {{
                set.insert(encoded);
            }}
        }}
    }}

    pub fn get(decision_id: usize) -> Vec<u32> {{
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
/// The line contains comma-separated 8-hex-digit observation values,
/// or is empty if no observations were recorded for that decision.
pub fn emit_mcdc_report_test(n_decisions: usize) -> String {
    if n_decisions == 0 {
        return String::new();
    }
    let mut s = String::new();
    s.push_str("// ── MVL MC/DC report (auto-generated) ───────────────────────────────────────\n");
    s.push_str("#[cfg(test)]\n#[test]\n");
    // IMPORTANT: The `zzz_` prefix is relied upon to sort this test last in
    // cargo's default alphabetic ordering, so all clause observations are
    // recorded before the file is written.  Cargo does not formally guarantee
    // test execution order; if a future cargo version changes ordering, some
    // observations may be missing from the output file.
    s.push_str("fn zzz_mvl_mcdc_report() {\n");
    s.push_str("    if let Ok(path) = std::env::var(\"MVL_MCDC_OUT\") {\n");
    s.push_str("        let mut out = String::new();\n");
    for i in 0..n_decisions {
        s.push_str(&format!(
            "        {{\
\n            let mut obs = crate::__mvl_mcdc::get({i});\
\n            obs.sort();\
\n            let enc: Vec<String> = obs.iter().map(|x| format!(\"{{:08x}}\", x)).collect();\
\n            out.push_str(&format!(\"{{}}\\n\", enc.join(\",\")));\
\n        }}\n"
        ));
    }
    s.push_str("        std::fs::write(&path, out).ok();\n");
    s.push_str("    }\n}\n");
    s
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
        assert!(s.contains("u32"), "must use u32 encoding");
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
        assert!(s.contains("{:08x}"), "must use 8-digit hex for u32");
    }
}
