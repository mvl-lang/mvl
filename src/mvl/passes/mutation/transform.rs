// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Native behavioral mutation testing: mutation point tracking and report generation.
//!
//! When `mvl mutate` is active, the transpiler injects env-var dispatch wrappers
//! around mutation-eligible expressions.  A single compilation embeds all mutants;
//! N parallel test-binary runs (each with `MVL_MUTANT=mN`) determine which are killed.
//!
//! See ADR-0014 for the execution model rationale.

use crate::mvl::parser::ast::BinaryOp;

// ── Mutation tables ────────────────────────────────────────────────────────

/// All alternative operator strings for a binary operator mutation.
/// Returns `(rust_op_str, description_fragment)` pairs.
/// Empty slice = no behavioral mutations available for this operator.
pub fn mutations_for_binary_op(op: BinaryOp) -> &'static [(&'static str, &'static str)] {
    match op {
        BinaryOp::Add => &[
            ("-", "+ → -"),
            ("*", "+ → *"),
            ("/", "+ → /"),
            ("%", "+ → %"),
        ],
        BinaryOp::Sub => &[
            ("+", "- → +"),
            ("*", "- → *"),
            ("/", "- → /"),
            ("%", "- → %"),
        ],
        BinaryOp::Mul => &[
            ("+", "* → +"),
            ("-", "* → -"),
            ("/", "* → /"),
            ("%", "* → %"),
        ],
        BinaryOp::Div => &[
            ("+", "/ → +"),
            ("-", "/ → -"),
            ("*", "/ → *"),
            ("%", "/ → %"),
        ],
        BinaryOp::Rem => &[
            ("+", "% → +"),
            ("-", "% → -"),
            ("*", "% → *"),
            ("/", "% → /"),
        ],
        BinaryOp::Eq => &[("!=", "== → !=")],
        BinaryOp::Ne => &[("==", "!= → ==")],
        BinaryOp::Lt => &[("<=", "< → <="), (">", "< → >"), (">=", "< → >=")],
        BinaryOp::Gt => &[(">=", "> → >="), ("<", "> → <"), ("<=", "> → <=")],
        BinaryOp::Le => &[("<", "<= → <"), (">=", "<= → >=")],
        BinaryOp::Ge => &[(">", ">= → >"), ("<=", ">= → <=")],
        BinaryOp::And => &[("||", "&& → ||")],
        BinaryOp::Or => &[("&&", "|| → &&")],
        BinaryOp::BitAnd => &[("|", "& → |"), ("^", "& → ^")],
        BinaryOp::BitOr => &[("&", "| → &"), ("^", "| → ^")],
        BinaryOp::BitXor => &[("&", "^ → &"), ("|", "^ → |")],
        BinaryOp::Shl => &[(">>", "<< → >>")],
        BinaryOp::Shr => &[("<<", ">> → <<")],
    }
}

/// Integer constant replacements for a given literal value.
/// Returns a deduplicated set of alternative values (excluding `original`).
pub fn mutations_for_int_literal(original: i64) -> Vec<i64> {
    use std::collections::BTreeSet;
    let candidates = [
        0i64,
        1,
        -1,
        original.saturating_add(1),
        original.saturating_sub(1),
    ];
    candidates
        .iter()
        .copied()
        .filter(|&v| v != original)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

// ── Tracking ───────────────────────────────────────────────────────────────

/// A single concrete mutation variant (one alternative at one location).
#[derive(Debug, Clone)]
pub struct MutantInfo {
    /// Env-var value that activates this mutant: `"m0"`, `"m1"`, etc.
    pub id: String,
    /// Enclosing function name.
    pub fn_name: String,
    /// Source file stem (e.g. `"math_test"`).
    pub file: String,
    /// Source line number (1-based).
    pub line: u32,
    /// Human-readable description: `"BinaryOp(+ → -)"`.
    pub description: String,
}

/// Accumulates mutation variant registrations during a transpilation pass.
pub struct MutationMap {
    pub mutants: Vec<MutantInfo>,
    next_id: usize,
}

impl Default for MutationMap {
    fn default() -> Self {
        Self::new()
    }
}

impl MutationMap {
    pub fn new() -> Self {
        MutationMap {
            mutants: Vec::new(),
            next_id: 0,
        }
    }

    /// Register one mutant variant and return its unique ID string (`"m0"`, etc.).
    pub fn alloc(
        &mut self,
        fn_name: String,
        file: String,
        line: u32,
        description: String,
    ) -> String {
        let id = format!("m{}", self.next_id);
        self.next_id += 1;
        self.mutants.push(MutantInfo {
            id: id.clone(),
            fn_name,
            file,
            line,
            description,
        });
        id
    }

    pub fn len(&self) -> usize {
        self.mutants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mutants.is_empty()
    }
}

// ── Report formatting ──────────────────────────────────────────────────────

/// Format the mutation score report from mutant metadata and run results.
///
/// `results` maps mutant ID → true (killed) / false (survived).
/// `all_files` is the ordered list of file stems so empty files still appear.
pub fn format_mutation_report(
    mutants: &[MutantInfo],
    results: &std::collections::HashMap<String, bool>,
    all_files: &[&str],
) -> String {
    use std::collections::BTreeMap;

    // Group mutants by file → fn_name → Vec<MutantInfo>.
    //
    // MVL-port note (#1581):
    // - Outer map is only accessed via `.get(file)` below (no iteration), so the
    //   port may use `Map[String, Map[...]]` directly — order doesn't matter.
    // - Inner map IS iterated for the per-function report (line ~200) and the
    //   `fn_name` order is human-visible.  The reader below sorts explicitly,
    //   so the port can use unordered `Map[String, List[MutantInfo]]` and copy
    //   the sort step verbatim.
    let mut by_file: BTreeMap<&str, BTreeMap<&str, Vec<&MutantInfo>>> = BTreeMap::new();
    for m in mutants {
        by_file
            .entry(m.file.as_str())
            .or_default()
            .entry(m.fn_name.as_str())
            .or_default()
            .push(m);
    }

    let mut out = String::new();
    out.push_str("\nNative behavioral mutation score\n");
    out.push_str(&"═".repeat(50));
    out.push('\n');

    let mut grand_killed = 0usize;
    let mut grand_total = 0usize;

    for file in all_files {
        out.push_str(&format!("\n{file}.mvl\n"));
        let fns = match by_file.get(file) {
            Some(m) => m,
            None => {
                out.push_str("  (no mutation points)\n");
                continue;
            }
        };

        let mut file_killed = 0usize;
        let mut file_total = 0usize;

        // Explicit sort for MVL-port one-to-one translation (#1581) — the
        // `fn_name` ordering appears in user-visible report output.  Same
        // sequence the `BTreeMap` iterator would have produced.
        let mut fn_entries: Vec<_> = fns.iter().collect();
        fn_entries.sort_by_key(|(name, _)| **name);
        for (fn_name, fn_mutants) in fn_entries {
            out.push_str(&format!("  {fn_name}\n"));
            for m in fn_mutants {
                let killed = results.get(&m.id).copied().unwrap_or(false);
                let mark = if killed { "✓" } else { "△" };
                let status = if killed { "killed  " } else { "survived" };
                out.push_str(&format!(
                    "    {mark}  {:<8}  line {:>4}  {}\n",
                    status, m.line, m.description
                ));
                if killed {
                    file_killed += 1;
                }
                file_total += 1;
            }
        }

        let pct = (file_killed * 100).checked_div(file_total).unwrap_or(100);
        out.push_str(&format!(
            "  {:<44}  {file_killed}/{file_total}  ({pct}%)\n",
            format!("{file}.mvl total")
        ));
        grand_killed += file_killed;
        grand_total += file_total;
    }

    out.push('\n');
    let grand_pct = (grand_killed * 100).checked_div(grand_total).unwrap_or(100);
    out.push_str(&format!(
        "Total: {grand_killed}/{grand_total} mutants killed  ({grand_pct}%)\n"
    ));
    out.push_str(&format!("Behavioral mutation score: {grand_pct}%\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── mutations_for_int_literal ──────────────────────────────────────────

    #[test]
    fn int_literal_zero_excludes_self() {
        let alts = mutations_for_int_literal(0);
        assert!(!alts.contains(&0), "original must be excluded");
        assert!(alts.contains(&1));
        assert!(alts.contains(&-1));
    }

    #[test]
    fn int_literal_one_excludes_self_and_deduplicates() {
        let alts = mutations_for_int_literal(1);
        assert!(!alts.contains(&1), "original must be excluded");
        // 1-1=0 and the constant 0 both appear; BTreeSet deduplicates
        assert_eq!(alts.iter().filter(|&&v| v == 0).count(), 1);
        assert!(alts.contains(&-1));
        assert!(alts.contains(&2)); // original+1
    }

    #[test]
    fn int_literal_minus_one_deduplicates() {
        let alts = mutations_for_int_literal(-1);
        assert!(!alts.contains(&-1));
        assert_eq!(alts.iter().filter(|&&v| v == 0).count(), 1); // -1+1=0 deduped
        assert!(alts.contains(&1));
        assert!(alts.contains(&-2)); // original-1
    }

    #[test]
    fn int_literal_large_value() {
        let alts = mutations_for_int_literal(100);
        assert!(!alts.contains(&100));
        assert!(alts.contains(&99));
        assert!(alts.contains(&101));
        assert!(alts.contains(&0));
        assert!(alts.contains(&1));
        assert!(alts.contains(&-1));
    }

    #[test]
    fn int_literal_i64_max_saturates() {
        let alts = mutations_for_int_literal(i64::MAX);
        assert!(!alts.contains(&i64::MAX));
        // saturating_add(1) == MAX (excluded), saturating_sub(1) == MAX-1
        assert!(alts.contains(&(i64::MAX - 1)));
    }

    #[test]
    fn int_literal_i64_min_saturates() {
        let alts = mutations_for_int_literal(i64::MIN);
        assert!(!alts.contains(&i64::MIN));
        assert!(alts.contains(&(i64::MIN + 1)));
    }

    #[test]
    fn int_literal_result_is_sorted_ascending() {
        let alts = mutations_for_int_literal(5);
        let sorted = {
            let mut v = alts.clone();
            v.sort();
            v
        };
        assert_eq!(alts, sorted, "BTreeSet iteration must be ascending");
    }

    // ── MutationMap ───────────────────────────────────────────────────────

    #[test]
    fn mutation_map_empty_on_new() {
        let m = MutationMap::new();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn mutation_map_default_equals_new() {
        let a = MutationMap::new();
        let b = MutationMap::default();
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn mutation_map_alloc_sequential_ids() {
        let mut m = MutationMap::new();
        let id0 = m.alloc("f".into(), "file".into(), 1, "d0".into());
        let id1 = m.alloc("f".into(), "file".into(), 2, "d1".into());
        let id2 = m.alloc("f".into(), "file".into(), 3, "d2".into());
        assert_eq!(id0, "m0");
        assert_eq!(id1, "m1");
        assert_eq!(id2, "m2");
        assert_eq!(m.len(), 3);
        assert!(!m.is_empty());
    }

    #[test]
    fn mutation_map_alloc_stores_metadata() {
        let mut m = MutationMap::new();
        m.alloc("my_fn".into(), "my_file".into(), 42, "desc".into());
        let info = &m.mutants[0];
        assert_eq!(info.id, "m0");
        assert_eq!(info.fn_name, "my_fn");
        assert_eq!(info.file, "my_file");
        assert_eq!(info.line, 42);
        assert_eq!(info.description, "desc");
    }

    // ── format_mutation_report ────────────────────────────────────────────

    fn make_mutant(id: &str, file: &str, fn_name: &str, line: u32, desc: &str) -> MutantInfo {
        MutantInfo {
            id: id.to_string(),
            fn_name: fn_name.to_string(),
            file: file.to_string(),
            line,
            description: desc.to_string(),
        }
    }

    #[test]
    fn report_all_killed() {
        let mutants = vec![make_mutant("m0", "math", "add", 10, "+ → -")];
        let results: HashMap<String, bool> = [("m0".to_string(), true)].into();
        let report = format_mutation_report(&mutants, &results, &["math"]);
        assert!(report.contains("✓"));
        assert!(report.contains("killed"));
        assert!(report.contains("1/1"));
        assert!(report.contains("100%"));
    }

    #[test]
    fn report_all_survived() {
        let mutants = vec![make_mutant("m0", "math", "add", 10, "+ → -")];
        let results: HashMap<String, bool> = [("m0".to_string(), false)].into();
        let report = format_mutation_report(&mutants, &results, &["math"]);
        assert!(report.contains("△"));
        assert!(report.contains("survived"));
        assert!(report.contains("0/1"));
        assert!(report.contains("0%"));
    }

    #[test]
    fn report_missing_result_treated_as_survived() {
        let mutants = vec![make_mutant("m0", "math", "add", 10, "+ → -")];
        let results: HashMap<String, bool> = HashMap::new(); // no entry for m0
        let report = format_mutation_report(&mutants, &results, &["math"]);
        assert!(report.contains("0/1"));
    }

    #[test]
    fn report_file_with_no_mutation_points() {
        let mutants: Vec<MutantInfo> = vec![];
        let results: HashMap<String, bool> = HashMap::new();
        let report = format_mutation_report(&mutants, &results, &["empty_file"]);
        assert!(report.contains("no mutation points"));
        // Grand total: 0/0 → checked_div returns unwrap_or(100) = 100%
        assert!(report.contains("100%"));
    }

    #[test]
    fn report_mixed_kill_survive() {
        let mutants = vec![
            make_mutant("m0", "math", "add", 10, "+ → -"),
            make_mutant("m1", "math", "add", 10, "+ → *"),
        ];
        let results: HashMap<String, bool> =
            [("m0".to_string(), true), ("m1".to_string(), false)].into();
        let report = format_mutation_report(&mutants, &results, &["math"]);
        assert!(report.contains("1/2"));
        assert!(report.contains("50%"));
    }
}
