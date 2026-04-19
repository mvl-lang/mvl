//! Native behavioral coverage: branch tracking and report generation.
//!
//! When `--coverage` is active in `mvl test`, the transpiler injects
//! `crate::__mvl_cov::hit(id)` calls at each decision branch (if, match, for,
//! while).  After tests run, the hit counts are written to `MVL_COV_OUT` by the
//! generated report test, then read and formatted by `cmd_test`.

/// The kind of decision branch being tracked.
#[derive(Debug, Clone)]
pub enum BranchKind {
    /// The then-block of an if expression/statement.
    IfTrue,
    /// The else-block of an if expression/statement.
    IfFalse,
    /// A match arm (0-indexed from the top of the match).
    MatchArm(usize),
    /// Entry into a for-loop body (counted per iteration).
    ForBody,
    /// Entry into a while-loop body (counted per iteration).
    WhileBody,
    /// Entry into the function body — used so branch-free functions still
    /// appear in the coverage report as 1/1 (called) or 0/1 (not called).
    FnEntry,
}

impl BranchKind {
    pub fn label(&self) -> String {
        match self {
            BranchKind::IfTrue => "if true".to_string(),
            BranchKind::IfFalse => "if false".to_string(),
            BranchKind::MatchArm(n) => format!("arm {n}"),
            BranchKind::ForBody => "for body".to_string(),
            BranchKind::WhileBody => "while body".to_string(),
            BranchKind::FnEntry => "fn entry".to_string(),
        }
    }

    /// True when this branch represents a non-iteration decision point.
    /// Used for per-function branch counts (for/while and fn-entry probes are
    /// not counted as distinct decision branches for the reachability summary).
    pub fn is_decision(&self) -> bool {
        !matches!(
            self,
            BranchKind::ForBody | BranchKind::WhileBody | BranchKind::FnEntry
        )
    }
}

/// Information about a single instrumented branch.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    /// Index into the HITS counter array.
    pub id: usize,
    /// Name of the enclosing MVL function.
    pub fn_name: String,
    /// Source file stem (e.g. "parser" for "parser_test.mvl").
    pub file: String,
    /// Source line number (1-based).
    pub line: u32,
    /// Kind of branch.
    pub kind: BranchKind,
    /// True when the enclosing function is a `test fn` — excluded from the report.
    pub is_test_fn: bool,
}

/// Accumulates branch registrations during a single transpilation pass.
pub struct CoverageMap {
    pub branches: Vec<BranchInfo>,
    next_id: usize,
}

impl CoverageMap {
    pub fn new(start_id: usize) -> Self {
        CoverageMap {
            branches: Vec::new(),
            next_id: start_id,
        }
    }

    /// Register a new branch and return its unique counter index.
    pub fn alloc(
        &mut self,
        fn_name: String,
        file: String,
        line: u32,
        kind: BranchKind,
        is_test_fn: bool,
    ) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.branches.push(BranchInfo {
            id,
            fn_name,
            file,
            line,
            kind,
            is_test_fn,
        });
        id
    }

    /// Next ID that would be allocated (equals total allocated so far + start_id).
    pub fn next_id(&self) -> usize {
        self.next_id
    }
}

// ── Code generation helpers ───────────────────────────────────────────────

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

// ── Report formatting ─────────────────────────────────────────────────────

/// Format a coverage report from branch metadata and raw hit counts.
///
/// `all_files` is the ordered list of all test file stems that were transpiled,
/// so files with zero decision branches still appear in the report.
pub fn format_report(branches: &[BranchInfo], hits: &[u64], all_files: &[&str]) -> String {
    use std::collections::BTreeMap;

    // Accumulate per-function decision-branch coverage and fn-entry hits.
    // file → fn_name → (branches_hit, total_decision_branches)
    let mut fn_decision: BTreeMap<(&str, &str), (usize, usize)> = BTreeMap::new();
    // file+fn → was the fn entry probe hit (true = called at least once)?
    let mut fn_entry_hit: BTreeMap<(&str, &str), bool> = BTreeMap::new();

    for b in branches {
        if b.is_test_fn {
            continue;
        }
        let count = hits.get(b.id).copied().unwrap_or(0);
        let key = (b.file.as_str(), b.fn_name.as_str());
        match &b.kind {
            BranchKind::FnEntry => {
                fn_entry_hit.insert(key, count > 0);
            }
            k if k.is_decision() => {
                let e = fn_decision.entry(key).or_insert((0, 0));
                if count > 0 {
                    e.0 += 1;
                }
                e.1 += 1;
            }
            _ => {} // ForBody, WhileBody
        }
    }

    // Build per-file function list.
    // Functions with decision branches: report their branch hit ratio.
    // Functions with no decision branches but an fn-entry probe: report 1/1 or 0/1.
    let mut by_file: BTreeMap<&str, Vec<(&str, usize, usize)>> = BTreeMap::new();
    for ((file, fn_name), (hit, total)) in &fn_decision {
        by_file
            .entry(file)
            .or_default()
            .push((fn_name, *hit, *total));
    }
    for ((file, fn_name), &called) in &fn_entry_hit {
        if !fn_decision.contains_key(&(*file, *fn_name)) {
            let covered = if called { 1 } else { 0 };
            by_file.entry(file).or_default().push((fn_name, covered, 1));
        }
    }

    let mut out = String::new();
    out.push_str("\nNative behavioral coverage\n");
    out.push_str(&"═".repeat(50));
    out.push('\n');

    let mut total_hit = 0usize;
    let mut total_branches = 0usize;

    // Iterate all_files in order so every tested file appears, even with no branches.
    for file in all_files {
        out.push_str(&format!("\n{file}.mvl\n"));
        let fns = by_file.get(file).map(|v| v.as_slice()).unwrap_or(&[]);

        if fns.is_empty() {
            out.push_str("  (no decision branches)\n");
            continue;
        }

        let mut file_hit = 0usize;
        let mut file_total = 0usize;

        for (fn_name, hit, total) in fns {
            let pct = if *total > 0 { (hit * 100) / total } else { 100 };
            let mark = if *hit == *total { "✓" } else { "△" };
            out.push_str(&format!(
                "  {mark}  {fn_name:<40}  {hit}/{total}  ({pct}%)\n"
            ));
            file_hit += hit;
            file_total += total;
        }

        let file_pct = (file_hit * 100).checked_div(file_total).unwrap_or(100);
        out.push_str(&format!(
            "     {:<40}  {file_hit}/{file_total}  ({file_pct}%)\n",
            format!("{file}.mvl total")
        ));
        total_hit += file_hit;
        total_branches += file_total;
    }

    out.push('\n');
    let total_pct = (total_hit * 100).checked_div(total_branches).unwrap_or(100);
    out.push_str(&format!(
        "Total: {total_hit}/{total_branches} branches  ({total_pct}%)\n"
    ));
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── BranchKind::label ─────────────────────────────────────────────────

    #[test]
    fn label_if_true() {
        assert_eq!(BranchKind::IfTrue.label(), "if true");
    }

    #[test]
    fn label_if_false() {
        assert_eq!(BranchKind::IfFalse.label(), "if false");
    }

    #[test]
    fn label_match_arm_zero() {
        assert_eq!(BranchKind::MatchArm(0).label(), "arm 0");
    }

    #[test]
    fn label_match_arm_nonzero() {
        assert_eq!(BranchKind::MatchArm(3).label(), "arm 3");
    }

    #[test]
    fn label_for_body() {
        assert_eq!(BranchKind::ForBody.label(), "for body");
    }

    #[test]
    fn label_while_body() {
        assert_eq!(BranchKind::WhileBody.label(), "while body");
    }

    #[test]
    fn label_fn_entry() {
        assert_eq!(BranchKind::FnEntry.label(), "fn entry");
    }

    // ── BranchKind::is_decision ───────────────────────────────────────────

    #[test]
    fn is_decision_true_for_if_true() {
        assert!(BranchKind::IfTrue.is_decision());
    }

    #[test]
    fn is_decision_true_for_if_false() {
        assert!(BranchKind::IfFalse.is_decision());
    }

    #[test]
    fn is_decision_true_for_match_arm() {
        assert!(BranchKind::MatchArm(0).is_decision());
        assert!(BranchKind::MatchArm(5).is_decision());
    }

    #[test]
    fn is_decision_false_for_for_body() {
        assert!(!BranchKind::ForBody.is_decision());
    }

    #[test]
    fn is_decision_false_for_while_body() {
        assert!(!BranchKind::WhileBody.is_decision());
    }

    #[test]
    fn is_decision_false_for_fn_entry() {
        assert!(!BranchKind::FnEntry.is_decision());
    }

    // ── CoverageMap ───────────────────────────────────────────────────────

    #[test]
    fn coverage_map_new_starts_empty() {
        let m = CoverageMap::new(0);
        assert!(m.branches.is_empty());
        assert_eq!(m.next_id(), 0);
    }

    #[test]
    fn coverage_map_new_with_nonzero_start_id() {
        let m = CoverageMap::new(10);
        assert_eq!(m.next_id(), 10);
    }

    #[test]
    fn coverage_map_alloc_returns_sequential_ids() {
        let mut m = CoverageMap::new(0);
        let id0 = m.alloc("f".into(), "file".into(), 1, BranchKind::IfTrue, false);
        let id1 = m.alloc("f".into(), "file".into(), 2, BranchKind::IfFalse, false);
        let id2 = m.alloc("f".into(), "file".into(), 3, BranchKind::MatchArm(0), false);
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    #[test]
    fn coverage_map_alloc_with_nonzero_start_id() {
        let mut m = CoverageMap::new(5);
        let id = m.alloc("f".into(), "file".into(), 1, BranchKind::FnEntry, false);
        assert_eq!(id, 5);
        assert_eq!(m.next_id(), 6);
    }

    #[test]
    fn coverage_map_alloc_stores_metadata() {
        let mut m = CoverageMap::new(0);
        m.alloc(
            "my_fn".into(),
            "parser".into(),
            42,
            BranchKind::MatchArm(1),
            true,
        );
        let b = &m.branches[0];
        assert_eq!(b.id, 0);
        assert_eq!(b.fn_name, "my_fn");
        assert_eq!(b.file, "parser");
        assert_eq!(b.line, 42);
        assert!(matches!(b.kind, BranchKind::MatchArm(1)));
        assert!(b.is_test_fn);
    }

    #[test]
    fn coverage_map_next_id_tracks_allocations() {
        let mut m = CoverageMap::new(0);
        assert_eq!(m.next_id(), 0);
        m.alloc("f".into(), "f".into(), 1, BranchKind::IfTrue, false);
        assert_eq!(m.next_id(), 1);
        m.alloc("f".into(), "f".into(), 2, BranchKind::IfFalse, false);
        assert_eq!(m.next_id(), 2);
    }

    // ── emit_cov_preamble ─────────────────────────────────────────────────

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

    // ── emit_cov_report_test ──────────────────────────────────────────────

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
        // For total=3, exactly 3 crate::__mvl_cov::get(N) calls must appear.
        let out = emit_cov_report_test(3);
        assert!(out.contains("get(0)"), "missing get(0)");
        assert!(out.contains("get(1)"), "missing get(1)");
        assert!(out.contains("get(2)"), "missing get(2)");
        assert!(!out.contains("get(3)"), "must not exceed total");
    }

    // ── format_report ─────────────────────────────────────────────────────

    fn make_branch(
        id: usize,
        fn_name: &str,
        file: &str,
        line: u32,
        kind: BranchKind,
        is_test_fn: bool,
    ) -> BranchInfo {
        BranchInfo {
            id,
            fn_name: fn_name.to_string(),
            file: file.to_string(),
            line,
            kind,
            is_test_fn,
        }
    }

    #[test]
    fn format_report_no_decision_branches_for_file() {
        // A file listed in all_files but with no branches shows "(no decision branches)".
        let report = format_report(&[], &[], &["parser"]);
        assert!(report.contains("parser.mvl"));
        assert!(report.contains("(no decision branches)"));
    }

    #[test]
    fn format_report_excludes_test_functions() {
        let branches = vec![make_branch(
            0,
            "test_fn",
            "parser",
            1,
            BranchKind::IfTrue,
            true,
        )];
        let hits = vec![1u64];
        let report = format_report(&branches, &hits, &["parser"]);
        // test fn is excluded, so file should still show "(no decision branches)"
        assert!(
            report.contains("(no decision branches)"),
            "test fns must be excluded"
        );
    }

    #[test]
    fn format_report_all_branches_hit_shows_check_mark() {
        let branches = vec![
            make_branch(0, "parse", "parser", 10, BranchKind::IfTrue, false),
            make_branch(1, "parse", "parser", 11, BranchKind::IfFalse, false),
        ];
        let hits = vec![5u64, 3u64];
        let report = format_report(&branches, &hits, &["parser"]);
        assert!(report.contains("✓"), "fully-covered fn must show ✓");
        assert!(report.contains("2/2"), "both branches must be counted");
        assert!(report.contains("100%"));
    }

    #[test]
    fn format_report_partial_coverage_shows_triangle() {
        let branches = vec![
            make_branch(0, "parse", "parser", 10, BranchKind::IfTrue, false),
            make_branch(1, "parse", "parser", 11, BranchKind::IfFalse, false),
        ];
        let hits = vec![1u64, 0u64]; // only IfTrue hit
        let report = format_report(&branches, &hits, &["parser"]);
        assert!(report.contains("△"), "partially-covered fn must show △");
        assert!(report.contains("1/2"));
        assert!(report.contains("50%"));
    }

    #[test]
    fn format_report_fn_entry_only_shows_called_uncalled() {
        // A function with only FnEntry (no decision branches) reports as 1/1 or 0/1.
        let branches = vec![make_branch(
            0,
            "simple_fn",
            "utils",
            5,
            BranchKind::FnEntry,
            false,
        )];
        let hits_called = vec![1u64];
        let report_called = format_report(&branches, &hits_called, &["utils"]);
        assert!(report_called.contains("1/1"), "called fn must show 1/1");

        let hits_uncalled = vec![0u64];
        let report_uncalled = format_report(&branches, &hits_uncalled, &["utils"]);
        assert!(report_uncalled.contains("0/1"), "uncalled fn must show 0/1");
    }

    #[test]
    fn format_report_for_and_while_bodies_not_counted_as_decisions() {
        // ForBody and WhileBody probes should not appear as decision branches.
        let branches = vec![
            make_branch(0, "loop_fn", "main", 1, BranchKind::FnEntry, false),
            make_branch(1, "loop_fn", "main", 2, BranchKind::ForBody, false),
            make_branch(2, "loop_fn", "main", 3, BranchKind::WhileBody, false),
        ];
        let hits = vec![1u64, 5u64, 0u64];
        let report = format_report(&branches, &hits, &["main"]);
        // loop_fn has only FnEntry — should appear as 1/1 (called), not as 3 branches
        assert!(
            report.contains("1/1"),
            "loop/while not counted as decision branches"
        );
    }

    #[test]
    fn format_report_includes_grand_total_line() {
        let branches = vec![
            make_branch(0, "f", "a", 1, BranchKind::IfTrue, false),
            make_branch(1, "f", "a", 2, BranchKind::IfFalse, false),
        ];
        let hits = vec![1u64, 1u64];
        let report = format_report(&branches, &hits, &["a"]);
        assert!(report.contains("Total:"), "must include grand total line");
    }

    #[test]
    fn format_report_multiple_files_in_order() {
        let branches = vec![
            make_branch(0, "f", "alpha", 1, BranchKind::IfTrue, false),
            make_branch(1, "g", "beta", 1, BranchKind::IfTrue, false),
        ];
        let hits = vec![1u64, 0u64];
        let report = format_report(&branches, &hits, &["alpha", "beta"]);
        let pos_alpha = report.find("alpha.mvl").unwrap();
        let pos_beta = report.find("beta.mvl").unwrap();
        assert!(pos_alpha < pos_beta, "files must appear in all_files order");
    }

    #[test]
    fn format_report_file_in_all_files_but_no_branches_still_appears() {
        // "empty.mvl" has no branches at all — must still appear as "(no decision branches)".
        let report = format_report(&[], &[], &["empty"]);
        assert!(report.contains("empty.mvl"));
        assert!(report.contains("(no decision branches)"));
    }

    #[test]
    fn format_report_total_zero_branches_shows_100_percent() {
        // When there are no branches at all, checked_div returns None → 100%.
        let report = format_report(&[], &[], &[]);
        assert!(report.contains("Total: 0/0 branches  (100%)"));
    }
}
