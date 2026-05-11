// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust-specific coverage emission helpers.
//!
//! Generates the `__mvl_cov` runtime module and report test that are embedded
//! in the transpiled test crate.  These functions are Rust-backend-only;
//! an LLVM-backend equivalent would live in `codegen/coverage_emit.rs`.
//!
//! # LLVM backend integration (future)
//!
//! An LLVM-side coverage pass would call LLVM's built-in coverage
//! instrumentation APIs (e.g. `__llvm_profile_*`) instead of emitting Rust
//! source.  The pass data (`CoverageMap`, `BranchInfo`) from
//! `passes/coverage/transform.rs` is already target-neutral and reusable.

/// Generate the `__mvl_cov` runtime module to embed at the top of the test crate.
pub fn emit_cov_preamble(total: usize) -> String {
    if total == 0 {
        return String::new();
    }
    format!(
        r#"// ── MVL native behavioral coverage runtime ──────────────────────────────────
#[cfg(test)]
pub mod __mvl_cov {{
    use std::sync::atomic::{{AtomicU64, Ordering}};
    pub static HITS: [AtomicU64; {total}] =
        [const {{ AtomicU64::new(0) }}; {total}];
    #[inline(always)]
    pub fn hit(id: usize) {{
        HITS[id].fetch_add(1, Ordering::Relaxed);
    }}
    pub fn get(id: usize) -> u64 {{
        HITS[id].load(Ordering::Relaxed)
    }}
}}

"#
    )
}

/// Generate the report test that writes hit counts to `MVL_COV_OUT` env var path.
pub fn emit_cov_report_test(total: usize) -> String {
    if total == 0 {
        return String::new();
    }
    let mut s = String::new();
    s.push_str("// ── MVL coverage report (auto-generated) ────────────────────────────────────\n");
    s.push_str("#[cfg(test)]\n");
    s.push_str("#[test]\n");
    s.push_str("fn zzz_mvl_cov_report() {\n");
    s.push_str("    if let Ok(path) = std::env::var(\"MVL_COV_OUT\") {\n");
    s.push_str("        let mut out = String::new();\n");
    for i in 0..total {
        s.push_str(&format!(
            "        out.push_str(&format!(\"{{}}\\n\", crate::__mvl_cov::get({i})));\n"
        ));
    }
    s.push_str("        std::fs::write(&path, out).ok();\n");
    s.push_str("    }\n");
    s.push_str("}\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_cov_preamble_empty_when_total_zero() {
        assert_eq!(emit_cov_preamble(0), "");
    }

    #[test]
    fn emit_cov_preamble_contains_module_declaration() {
        let out = emit_cov_preamble(1);
        assert!(out.contains("mod __mvl_cov"), "missing module declaration");
    }

    #[test]
    fn emit_cov_preamble_array_size_matches_total() {
        let out = emit_cov_preamble(7);
        assert!(out.contains("AtomicU64; 7]"), "array size must match total");
    }

    #[test]
    fn emit_cov_preamble_exposes_hit_and_get_fns() {
        let out = emit_cov_preamble(3);
        assert!(out.contains("pub fn hit(id: usize)"), "missing hit fn");
        assert!(out.contains("pub fn get(id: usize)"), "missing get fn");
    }

    #[test]
    fn emit_cov_preamble_is_cfg_test_gated() {
        let out = emit_cov_preamble(1);
        assert!(out.contains("#[cfg(test)]"), "must be cfg(test) gated");
    }

    #[test]
    fn emit_cov_report_test_empty_when_total_zero() {
        assert_eq!(emit_cov_report_test(0), "");
    }

    #[test]
    fn emit_cov_report_test_contains_test_fn() {
        let out = emit_cov_report_test(1);
        assert!(out.contains("#[test]"), "missing #[test]");
        assert!(out.contains("fn zzz_mvl_cov_report()"), "missing report fn");
    }

    #[test]
    fn emit_cov_report_test_reads_mvl_cov_out_env_var() {
        let out = emit_cov_report_test(1);
        assert!(out.contains("MVL_COV_OUT"), "must read MVL_COV_OUT");
    }

    #[test]
    fn emit_cov_report_test_emits_one_get_per_branch() {
        let out = emit_cov_report_test(3);
        assert!(out.contains("get(0)"), "missing get(0)");
        assert!(out.contains("get(1)"), "missing get(1)");
        assert!(out.contains("get(2)"), "missing get(2)");
        assert!(!out.contains("get(3)"), "must not exceed total");
    }
}
