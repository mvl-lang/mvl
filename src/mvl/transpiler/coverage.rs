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
}

impl BranchKind {
    pub fn label(&self) -> String {
        match self {
            BranchKind::IfTrue => "if true".to_string(),
            BranchKind::IfFalse => "if false".to_string(),
            BranchKind::MatchArm(n) => format!("arm {n}"),
            BranchKind::ForBody => "for body".to_string(),
            BranchKind::WhileBody => "while body".to_string(),
        }
    }

    /// True when this branch represents a non-iteration decision point.
    /// Used for per-function branch counts (for/while are not counted as
    /// distinct decision branches for the reachability summary).
    pub fn is_decision(&self) -> bool {
        !matches!(self, BranchKind::ForBody | BranchKind::WhileBody)
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
    pub fn alloc(&mut self, fn_name: String, file: String, line: u32, kind: BranchKind) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.branches.push(BranchInfo {
            id,
            fn_name,
            file,
            line,
            kind,
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
pub fn format_report(branches: &[BranchInfo], hits: &[u64]) -> String {
    use std::collections::BTreeMap;

    if branches.is_empty() {
        return "No behavioral branches found in test files.\n".to_string();
    }

    // Group: file → fn_name → branches (preserving registration order)
    let mut by_file: BTreeMap<&str, BTreeMap<&str, Vec<&BranchInfo>>> = BTreeMap::new();
    for b in branches {
        by_file
            .entry(&b.file)
            .or_default()
            .entry(&b.fn_name)
            .or_default()
            .push(b);
    }

    let mut out = String::new();
    out.push_str("\nNative behavioral coverage\n");
    out.push_str(&"═".repeat(50));
    out.push('\n');

    let mut total_hit = 0usize;
    let mut total_branches = 0usize;

    for (file, fns) in &by_file {
        out.push_str(&format!("\n{file}.mvl\n"));
        let mut file_hit = 0usize;
        let mut file_total = 0usize;

        for (fn_name, brs) in fns {
            out.push_str(&format!("  fn {fn_name}\n"));
            for b in brs {
                let count = hits.get(b.id).copied().unwrap_or(0);
                let mark = if count > 0 { "✓" } else { "✗" };
                out.push_str(&format!(
                    "    {mark} {} (line {}): {count}\n",
                    b.kind.label(),
                    b.line,
                ));
                if b.kind.is_decision() {
                    if count > 0 {
                        file_hit += 1;
                    }
                    file_total += 1;
                }
            }
        }

        let pct = if file_total > 0 {
            (file_hit * 100) / file_total
        } else {
            100
        };
        out.push_str(&format!(
            "  coverage: {file_hit}/{file_total} branches ({pct}%)\n"
        ));
        total_hit += file_hit;
        total_branches += file_total;
    }

    out.push('\n');
    let total_pct = if total_branches > 0 {
        (total_hit * 100) / total_branches
    } else {
        100
    };
    out.push_str(&format!(
        "Total: {total_hit}/{total_branches} branches ({total_pct}%)\n"
    ));
    out
}
