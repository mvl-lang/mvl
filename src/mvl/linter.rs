// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! MVL linter — style and structural checks for source files.
//!
//! # Entry point
//!
//! ```ignore
//! use mvl::mvl::linter::{lint, config::LintConfig};
//!
//! let cfg = LintConfig::load(project_root);
//! let result = lint(&program, source, &cfg);
//! for diag in &result.diags {
//!     eprintln!("{}", diag.render(file_path));
//! }
//! ```

pub mod config;
pub mod errors;
pub mod rules;

use crate::mvl::parser::ast::Program;
use config::LintConfig;
use errors::LintDiag;

/// Aggregated result of linting one file.
pub struct LintResult {
    pub diags: Vec<LintDiag>,
}

impl LintResult {
    /// `true` if there are no error-severity diagnostics.
    /// Hints and warnings do not affect this result.
    pub fn is_ok(&self) -> bool {
        self.diags
            .iter()
            .all(|d| d.severity < errors::Severity::Error)
    }

    /// Total number of hint-severity diagnostics (Spec 011 Req 3).
    pub fn hint_count(&self) -> usize {
        self.diags
            .iter()
            .filter(|d| d.severity == errors::Severity::Hint)
            .count()
    }

    /// Total number of warnings (does not include hints).
    pub fn warning_count(&self) -> usize {
        self.diags
            .iter()
            .filter(|d| d.severity == errors::Severity::Warning)
            .count()
    }

    /// Total number of errors.
    pub fn error_count(&self) -> usize {
        self.diags
            .iter()
            .filter(|d| d.severity == errors::Severity::Error)
            .count()
    }
}

/// Run all enabled lint rules against a parsed program and its source.
///
/// Phase 1 — style rules: trailing whitespace, line length, indentation,
/// final newline, naming conventions, function body length.
///
/// Phase 2 — semantic rules: unreachable code, redundant match, redundant
/// effect declarations, redundant IFC labels, missing annotations (opt-in).
///
/// Phase 3 — LLM corpus quality: consistent comment style, doc-comment
/// coverage, doc-comment example sections.
///
/// Phase 4 — Complexity (regenerability): cyclomatic complexity, nested match
/// depth, effect signature width, trait impl count, module fan-out, extern ratio.
pub fn lint(prog: &Program, src: &str, cfg: &LintConfig) -> LintResult {
    let mut diags: Vec<LintDiag> = Vec::new();

    // Phase 1: source rules
    rules::trailing_whitespace(src, cfg, &mut diags);
    rules::line_length(src, cfg, &mut diags);
    rules::indentation(src, cfg, &mut diags);
    rules::final_newline(src, cfg, &mut diags);

    // Phase 1: AST rules
    rules::naming(prog, cfg, &mut diags);
    rules::fn_length(prog, src, cfg, &mut diags);

    // Phase 2: semantic rules
    rules::unreachable_code(prog, cfg, &mut diags);
    rules::redundant_match(prog, cfg, &mut diags);
    rules::redundant_effects(prog, cfg, &mut diags);
    rules::redundant_ifc_labels(prog, cfg, &mut diags);
    rules::missing_annotations(prog, cfg, &mut diags);
    rules::missing_totality(prog, cfg, &mut diags);
    rules::for_iter_antipattern(prog, cfg, &mut diags);
    rules::while_to_for_range(prog, cfg, &mut diags);
    rules::suggest_decreases(prog, cfg, &mut diags);
    rules::suggest_total_upgrade(prog, cfg, &mut diags);

    // Phase 3: LLM corpus quality rules
    rules::consistent_comment_style(src, cfg, &mut diags);
    rules::doc_comments_required(prog, src, cfg, &mut diags);
    rules::doc_comment_examples(prog, src, cfg, &mut diags);

    // Phase 4: Complexity rules (regenerability metrics)
    rules::complexity_cyclomatic(prog, cfg, &mut diags);
    rules::complexity_match_depth(prog, cfg, &mut diags);
    rules::complexity_effect_width(prog, cfg, &mut diags);
    rules::complexity_trait_impl_count(prog, cfg, &mut diags);
    rules::complexity_module_fanout(prog, cfg, &mut diags);
    rules::complexity_extern_ratio(prog, cfg, &mut diags);

    // Sort by line then col for consistent output
    diags.sort_by_key(|d| (d.span.line, d.span.col));

    LintResult { diags }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn parse(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    fn default_cfg() -> LintConfig {
        LintConfig::default()
    }

    // ── LintResult methods ────────────────────────────────────────────────

    #[test]
    fn lint_result_is_ok_no_diags() {
        let r = LintResult { diags: vec![] };
        assert!(r.is_ok());
        assert_eq!(r.hint_count(), 0);
        assert_eq!(r.warning_count(), 0);
        assert_eq!(r.error_count(), 0);
    }

    #[test]
    fn lint_result_hints_only_is_ok(/* Spec 011 Req 3 */) {
        let r = LintResult {
            diags: vec![
                errors::LintDiag::hint("redundant-ifc-label", "hint msg", 1, 1),
                errors::LintDiag::hint("redundant-ifc-label", "hint msg2", 2, 1),
            ],
        };
        assert!(r.is_ok(), "hints alone must not make is_ok() false");
        assert_eq!(r.hint_count(), 2);
        assert_eq!(r.warning_count(), 0);
        assert_eq!(r.error_count(), 0);
    }

    #[test]
    fn lint_result_hints_not_counted_as_warnings(/* Spec 011 Req 3 */) {
        let r = LintResult {
            diags: vec![
                errors::LintDiag::hint("redundant-ifc-label", "hint", 1, 1),
                errors::LintDiag::warning("line-length", "warn", 2, 1),
            ],
        };
        assert_eq!(r.hint_count(), 1);
        assert_eq!(r.warning_count(), 1);
    }

    #[test]
    fn lint_result_is_ok_warnings_only() {
        let r = LintResult {
            diags: vec![errors::LintDiag::warning("test-rule", "msg", 1, 1)],
        };
        assert!(r.is_ok(), "warnings alone should not make is_ok() false");
        assert_eq!(r.warning_count(), 1);
        assert_eq!(r.error_count(), 0);
    }

    #[test]
    fn lint_result_is_ok_false_on_error() {
        let r = LintResult {
            diags: vec![errors::LintDiag::error("test-rule", "msg", 1, 1)],
        };
        assert!(!r.is_ok());
        assert_eq!(r.warning_count(), 0);
        assert_eq!(r.error_count(), 1);
    }

    #[test]
    fn lint_result_counts_mixed_severity() {
        let r = LintResult {
            diags: vec![
                errors::LintDiag::warning("w1", "warn", 1, 1),
                errors::LintDiag::error("e1", "err", 2, 1),
                errors::LintDiag::warning("w2", "warn2", 3, 1),
            ],
        };
        assert!(!r.is_ok());
        assert_eq!(r.warning_count(), 2);
        assert_eq!(r.error_count(), 1);
    }

    // ── lint() integration ────────────────────────────────────────────────

    #[test]
    fn lint_clean_source_no_diags() {
        let src = "fn add(x: Int, y: Int) -> Int {\n    x + y\n}\n";
        let prog = parse(src);
        let result = lint(&prog, src, &default_cfg());
        assert!(result.is_ok());
    }

    #[test]
    fn lint_detects_trailing_whitespace() {
        let src = "fn foo() -> Int { 1 }   \n";
        let prog = parse(src);
        let mut cfg = default_cfg();
        cfg.trailing_ws = true;
        let result = lint(&prog, src, &cfg);
        assert!(
            result.diags.iter().any(|d| d.rule == "trailing-whitespace"),
            "expected trailing-whitespace diagnostic"
        );
    }

    #[test]
    fn lint_detects_long_line() {
        let long_line = "x".repeat(200);
        let src = format!("fn foo() -> Int {{\n    {long_line}\n}}\n");
        let prog = parse(&src);
        let mut cfg = default_cfg();
        cfg.line_length = 120;
        let result = lint(&prog, &src, &cfg);
        assert!(
            result.diags.iter().any(|d| d.rule == "line-length"),
            "expected line-length diagnostic"
        );
    }

    #[test]
    fn lint_results_sorted_by_line() {
        let src = "fn foo() -> Int { 1 }   \nfn bar() -> Int { 2 }   \n";
        let prog = parse(src);
        let result = lint(&prog, src, &default_cfg());
        let lines: Vec<u32> = result.diags.iter().map(|d| d.span.line).collect();
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(lines, sorted, "diagnostics should be sorted by line");
    }

    #[test]
    fn lint_respects_disabled_trailing_ws_rule() {
        let src = "fn foo() -> Int { 1 }   \n";
        let prog = parse(src);
        let mut cfg = default_cfg();
        cfg.trailing_ws = false;
        let result = lint(&prog, src, &cfg);
        assert!(
            result.diags.iter().all(|d| d.rule != "trailing-whitespace"),
            "trailing-whitespace should be suppressed when disabled"
        );
    }
}
