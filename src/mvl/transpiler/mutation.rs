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

    // Group mutants by file → fn_name → Vec<MutantInfo>
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

        for (fn_name, fn_mutants) in fns {
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
