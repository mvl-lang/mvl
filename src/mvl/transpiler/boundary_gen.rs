//! Boundary value analysis for surviving mutation testing candidates.
//!
//! After a mutation run, identifies surviving `IntLiteral` and comparison-operator
//! mutants and reports the exact boundary values that would kill them.
//!
//! This is Phase 1 (IntLiteral survivors) + Phase 2 (comparison-op survivors) of
//! the boundary value analysis described in issue #331.

use crate::mvl::transpiler::mutation::MutantInfo;
use std::collections::HashMap;

/// Produce a boundary value analysis report for surviving mutants.
///
/// `file_paths` maps module_name (= `MutantInfo::file`) → absolute source file path.
/// Returns a formatted string suitable for printing to stdout.
pub fn format_boundary_report(
    mutants: &[MutantInfo],
    results: &HashMap<String, bool>,
    file_paths: &HashMap<String, String>,
) -> String {
    let items: Vec<(&MutantInfo, Option<String>, Vec<String>)> = mutants
        .iter()
        .filter(|m| !results.get(&m.id).copied().unwrap_or(false))
        .filter_map(|m| make_item(m, file_paths))
        .collect();

    if items.is_empty() {
        return "\nNo surviving boundary-relevant mutants (IntLiteral or comparison operator).\n"
            .to_string();
    }

    let mut out = format!(
        "\nBoundary value analysis — {} surviving comparison/threshold mutant(s)\n",
        items.len()
    );
    out.push_str(&"═".repeat(62));
    out.push_str("\nAdd tests with these specific values to kill the surviving mutants:\n");

    for (info, src_line, hints) in &items {
        out.push('\n');
        out.push_str(&format!(
            "  {}.mvl · {} · line {}\n",
            info.file, info.fn_name, info.line
        ));
        out.push_str(&format!("  Mutant:  {}\n", info.description));
        if let Some(line) = src_line {
            out.push_str(&format!("  Context: {}\n", line.trim()));
        }
        for hint in hints {
            out.push_str(&format!("  → {}\n", hint));
        }
    }

    out.push('\n');
    out.push_str(&"═".repeat(62));
    out.push('\n');
    out
}

// ── Item construction ──────────────────────────────────────────────────────

fn make_item<'a>(
    m: &'a MutantInfo,
    file_paths: &HashMap<String, String>,
) -> Option<(&'a MutantInfo, Option<String>, Vec<String>)> {
    let src_line = read_source_line(m, file_paths);

    if let Some((orig, mutated)) = parse_int_literal(&m.description) {
        let hints = int_literal_hints(orig, mutated, src_line.as_deref());
        return Some((m, src_line, hints));
    }

    if let Some((op, _)) = parse_comparison_op(&m.description) {
        let hints = comparison_op_hints(&op, src_line.as_deref());
        return Some((m, src_line, hints));
    }

    None
}

fn read_source_line(m: &MutantInfo, file_paths: &HashMap<String, String>) -> Option<String> {
    let path = file_paths.get(&m.file)?;
    let content = std::fs::read_to_string(path).ok()?;
    let line_idx = (m.line as usize).saturating_sub(1);
    content.lines().nth(line_idx).map(str::to_owned)
}

// ── Description parsers ───────────────────────────────────────────────────

/// Parse `IntLiteral(N → M)` → `(N, M)`.
fn parse_int_literal(desc: &str) -> Option<(i64, i64)> {
    let inner = desc.strip_prefix("IntLiteral(")?.strip_suffix(')')?;
    let (lhs, rhs) = inner.split_once(" → ")?;
    let orig: i64 = lhs.trim().parse().ok()?;
    let mutated: i64 = rhs.trim().parse().ok()?;
    Some((orig, mutated))
}

/// Parse a comparison-operator mutation description `"op1 → op2"` → `(op1, op2)`.
///
/// Binary-op descriptions are emitted as bare fragments like `"< → <="` (no wrapper).
fn parse_comparison_op(desc: &str) -> Option<(String, String)> {
    // Strip optional "BinaryOp(...)" wrapper for forward-compatibility
    let inner = if let Some(s) = desc
        .strip_prefix("BinaryOp(")
        .and_then(|s| s.strip_suffix(')'))
    {
        s
    } else {
        desc
    };
    let (lhs, rhs) = inner.split_once(" → ")?;
    let orig = lhs.trim();
    let mutated = rhs.trim();
    if is_cmp(orig) {
        Some((orig.to_string(), mutated.to_string()))
    } else {
        None
    }
}

fn is_cmp(op: &str) -> bool {
    matches!(op, "<" | "<=" | ">" | ">=")
}

// ── Hint generators ───────────────────────────────────────────────────────

fn int_literal_hints(orig: i64, mutated: i64, src: Option<&str>) -> Vec<String> {
    let field = src.and_then(|l| field_for_threshold(l, orig));
    let fname = field.as_deref().unwrap_or("<field>");

    // The value that distinguishes `field op orig` from `field op mutated`:
    // for `<` comparisons (most common), min(orig, mutated) is the kill value.
    let kill_at = orig.min(mutated);

    vec![
        format!(
            "Set {} = {} → distinguishes threshold {} from {} (kills this mutant)",
            fname, kill_at, orig, mutated
        ),
        format!(
            "Boundary sweep: {} = {}, {}, {} (N-1, N, N+1 around threshold {})",
            fname,
            orig - 1,
            orig,
            orig + 1,
            orig
        ),
    ]
}

fn comparison_op_hints(op: &str, src: Option<&str>) -> Vec<String> {
    let n = src.and_then(|l| threshold_for_op(l, op));
    match n {
        Some(n) => {
            let (kill_at, reason) = match op {
                "<" => (
                    n,
                    format!("{n} < {n}=false vs {n} <= {n}=true → kills < → <="),
                ),
                "<=" => {
                    let n1 = n + 1;
                    (
                        n1,
                        format!("{n1} <= {n}=false vs {n1} < {n}=false → kills <= → <"),
                    )
                }
                ">" => (
                    n,
                    format!("{n} > {n}=false vs {n} >= {n}=true → kills > → >="),
                ),
                ">=" => {
                    let n1 = n - 1;
                    (
                        n1,
                        format!("{n1} >= {n}=false vs {n1} > {n}=false → kills >= → >"),
                    )
                }
                _ => (n, format!("test at threshold {n}")),
            };
            vec![
                format!("Threshold: field {} {}", op, n),
                format!("Kill with: set field = {kill_at} ({reason})"),
            ]
        }
        None => vec![format!(
            "Operator {} — test at the exact threshold value (could not parse source line)",
            op
        )],
    }
}

// ── Source-line analysis ──────────────────────────────────────────────────

/// Extract the field/identifier compared against `threshold` on this source line.
/// e.g. `"v.oxygen_sat < 92"` with threshold=92 → `Some("v.oxygen_sat")`.
fn field_for_threshold(line: &str, threshold: i64) -> Option<String> {
    let t = threshold.to_string();
    let pos = find_whole_int(line, &t)?;
    // Everything before the threshold (trimmed) ends with `<field> <op>`
    let before = line[..pos].trim_end();
    let (before_op, _op) = strip_trailing_cmp(before)?;
    last_ident(before_op.trim_end())
}

/// Extract the integer threshold adjacent to `op` on this source line.
/// e.g. `"v.systolic_bp < 90"` with op=`"<"` → `Some(90)`.
fn threshold_for_op(line: &str, op: &str) -> Option<i64> {
    let pos = find_op_in_line(line, op)?;
    let after = line[pos + op.len()..].trim_start();
    parse_leading_int(after)
}

// ── String utilities ──────────────────────────────────────────────────────

/// Find the byte position of `op` in `line`, skipping partial matches
/// (e.g. `<` must not match the `<` in `<=`).
fn find_op_in_line(line: &str, op: &str) -> Option<usize> {
    let mut from = 0;
    while from < line.len() {
        let rel = line[from..].find(op)?;
        let abs = from + rel;
        let after = &line[abs + op.len()..];
        // Guard: prevent `<` from matching `<=`, `>` from matching `>=`
        if (op == "<" || op == ">") && after.starts_with('=') {
            from = abs + 1;
            continue;
        }
        return Some(abs);
    }
    None
}

/// Find byte position of `n_str` as a whole integer in `line`
/// (not part of a longer digit sequence).
fn find_whole_int(line: &str, n_str: &str) -> Option<usize> {
    let mut from = 0;
    while from < line.len() {
        let rel = line[from..].find(n_str)?;
        let abs = from + rel;
        let before_digit = abs > 0
            && line
                .as_bytes()
                .get(abs - 1)
                .copied()
                .map(|b| b.is_ascii_digit())
                .unwrap_or(false);
        let after_digit = line
            .as_bytes()
            .get(abs + n_str.len())
            .copied()
            .map(|b| b.is_ascii_digit())
            .unwrap_or(false);
        if !before_digit && !after_digit {
            return Some(abs);
        }
        from = abs + 1;
    }
    None
}

/// Strip the last comparison operator from the end of `s`.
/// Returns `(remaining, op)` where `remaining` precedes the operator.
fn strip_trailing_cmp(s: &str) -> Option<(&str, &str)> {
    // Try longest ops first
    for op in &["<=", ">=", "<", ">"] {
        if let Some(pos) = s.rfind(op) {
            if s[pos + op.len()..].trim().is_empty() {
                return Some((&s[..pos], op));
            }
        }
    }
    None
}

/// Extract the last identifier or dotted path from a string.
/// e.g. `"(v.oxygen_sat"` → `Some("v.oxygen_sat")`.
fn last_ident(s: &str) -> Option<String> {
    let s = s.trim_end();
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();
    let mut end = bytes.len();
    // Walk backwards through alphanumeric, `_`, `.`
    let mut i = end;
    while i > 0 {
        i -= 1;
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' {
            end = i; // keep extending left
        } else {
            break;
        }
    }
    // `end` is now the start of the identifier
    let slice = s[end..].trim_start_matches('.');
    if slice.is_empty() {
        None
    } else {
        Some(slice.to_string())
    }
}

/// Parse a leading integer (optionally negative) from `s`.
fn parse_leading_int(s: &str) -> Option<i64> {
    let (neg, rest) = if let Some(stripped) = s.strip_prefix('-') {
        (true, stripped)
    } else {
        (false, s)
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    Some(if neg { -n } else { n })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mutant(id: &str, file: &str, fn_name: &str, line: u32, desc: &str) -> MutantInfo {
        MutantInfo {
            id: id.to_string(),
            fn_name: fn_name.to_string(),
            file: file.to_string(),
            line,
            description: desc.to_string(),
        }
    }

    // ── parse_int_literal ──────────────────────────────────────────────────

    #[test]
    fn parse_int_literal_basic() {
        assert_eq!(parse_int_literal("IntLiteral(92 → 91)"), Some((92, 91)));
        assert_eq!(parse_int_literal("IntLiteral(92 → 93)"), Some((92, 93)));
        assert_eq!(parse_int_literal("IntLiteral(0 → 1)"), Some((0, 1)));
        assert_eq!(parse_int_literal("IntLiteral(-1 → 0)"), Some((-1, 0)));
    }

    #[test]
    fn parse_int_literal_non_matching() {
        assert_eq!(parse_int_literal("BinaryOp(< → <=)"), None);
        assert_eq!(parse_int_literal("BoolLiteral(true → false)"), None);
        assert_eq!(parse_int_literal("IntLiteral(bad)"), None);
    }

    // ── parse_comparison_op ───────────────────────────────────────────────

    #[test]
    fn parse_comparison_op_basic() {
        // Bare format as emitted by codegen
        assert_eq!(
            parse_comparison_op("< → <="),
            Some(("<".to_string(), "<=".to_string()))
        );
        assert_eq!(
            parse_comparison_op("> → >="),
            Some((">".to_string(), ">=".to_string()))
        );
        // Also accept wrapped format for forward-compatibility
        assert_eq!(
            parse_comparison_op("BinaryOp(< → <=)"),
            Some(("<".to_string(), "<=".to_string()))
        );
    }

    #[test]
    fn parse_comparison_op_arithmetic_excluded() {
        assert_eq!(parse_comparison_op("+ → -"), None);
        assert_eq!(parse_comparison_op("&& → ||"), None);
    }

    // ── find_op_in_line ───────────────────────────────────────────────────

    #[test]
    fn find_lt_not_confused_with_lte() {
        // Should not match < inside <=
        assert_eq!(find_op_in_line("v.x <= 92", "<"), None);
        // Should match the standalone <
        assert!(find_op_in_line("v.x < 92", "<").is_some());
    }

    #[test]
    fn find_op_lte() {
        let pos = find_op_in_line("v.x <= 92", "<=");
        assert!(pos.is_some());
    }

    // ── find_whole_int ────────────────────────────────────────────────────

    #[test]
    fn find_whole_int_basic() {
        assert!(find_whole_int("v.x < 92", "92").is_some());
        assert_eq!(find_whole_int("v.x < 920", "92"), None); // part of 920
        assert!(find_whole_int("v.x < 92 && v.y > 0", "92").is_some());
    }

    // ── field_for_threshold ───────────────────────────────────────────────

    #[test]
    fn field_for_threshold_simple() {
        let line = "    v.oxygen_sat < 92";
        assert_eq!(
            field_for_threshold(line, 92),
            Some("v.oxygen_sat".to_string())
        );
    }

    #[test]
    fn field_for_threshold_complex_line() {
        let line = "    || (v.breathing == BreathingStatus::Labored && v.oxygen_sat < 92)";
        assert_eq!(
            field_for_threshold(line, 92),
            Some("v.oxygen_sat".to_string())
        );
    }

    #[test]
    fn field_for_threshold_gt() {
        let line = "    v.heart_rate > 130";
        assert_eq!(
            field_for_threshold(line, 130),
            Some("v.heart_rate".to_string())
        );
    }

    // ── threshold_for_op ──────────────────────────────────────────────────

    #[test]
    fn threshold_for_op_lt() {
        assert_eq!(threshold_for_op("    v.systolic_bp < 90", "<"), Some(90));
    }

    #[test]
    fn threshold_for_op_gt() {
        assert_eq!(threshold_for_op("v.heart_rate > 130", ">"), Some(130));
    }

    #[test]
    fn threshold_for_op_lte() {
        assert_eq!(threshold_for_op("v.x <= 50", "<="), Some(50));
    }

    // ── last_ident ────────────────────────────────────────────────────────

    #[test]
    fn last_ident_simple() {
        assert_eq!(
            last_ident("v.oxygen_sat "),
            Some("v.oxygen_sat".to_string())
        );
        assert_eq!(last_ident("(v.foo"), Some("v.foo".to_string()));
        assert_eq!(last_ident("  "), None);
    }

    // ── format_boundary_report ────────────────────────────────────────────

    #[test]
    fn report_no_survivors() {
        let mutants = vec![mutant("m0", "f", "g", 1, "IntLiteral(5 → 4)")];
        let results: HashMap<String, bool> = [("m0".to_string(), true)].into();
        let report = format_boundary_report(&mutants, &results, &HashMap::new());
        assert!(report.contains("No surviving boundary-relevant mutants"));
    }

    #[test]
    fn report_int_literal_survivor() {
        let mutants = vec![mutant("m0", "f", "my_fn", 10, "IntLiteral(92 → 91)")];
        let results: HashMap<String, bool> = [("m0".to_string(), false)].into();
        let report = format_boundary_report(&mutants, &results, &HashMap::new());
        assert!(report.contains("IntLiteral(92 → 91)"));
        assert!(report.contains("91")); // kill_at = min(92, 91)
        assert!(report.contains("threshold 92"));
    }

    #[test]
    fn report_comparison_op_survivor() {
        // Bare format as emitted by codegen
        let mutants = vec![mutant("m0", "f", "my_fn", 10, "< → <=")];
        let results: HashMap<String, bool> = [("m0".to_string(), false)].into();
        let report = format_boundary_report(&mutants, &results, &HashMap::new());
        assert!(report.contains("< → <="));
        assert!(report.contains("Operator <"));
    }

    #[test]
    fn report_comparison_op_wrapped_format() {
        // BinaryOp(...) wrapper also accepted
        let mutants = vec![mutant("m0", "f", "my_fn", 10, "BinaryOp(< → <=)")];
        let results: HashMap<String, bool> = [("m0".to_string(), false)].into();
        let report = format_boundary_report(&mutants, &results, &HashMap::new());
        assert!(report.contains("BinaryOp(< → <=)"));
    }

    #[test]
    fn report_arithmetic_op_survivor_excluded() {
        let mutants = vec![mutant("m0", "f", "my_fn", 10, "+ → -")];
        let results: HashMap<String, bool> = [("m0".to_string(), false)].into();
        let report = format_boundary_report(&mutants, &results, &HashMap::new());
        assert!(report.contains("No surviving boundary-relevant mutants"));
    }

    #[test]
    fn report_shows_file_fn_line() {
        let mutants = vec![mutant(
            "m0",
            "triage",
            "airway_compromise",
            29,
            "IntLiteral(92 → 91)",
        )];
        let results: HashMap<String, bool> = [("m0".to_string(), false)].into();
        let report = format_boundary_report(&mutants, &results, &HashMap::new());
        assert!(report.contains("triage.mvl"));
        assert!(report.contains("airway_compromise"));
        assert!(report.contains("line 29"));
    }
}
