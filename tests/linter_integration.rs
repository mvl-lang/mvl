//! Linter integration tests: parse → lint full pipeline on corpus files.
//!
//! These tests exercise the complete parse→lint chain (not just isolated
//! rule unit tests). Each test runs the linter on a real .mvl corpus file
//! and verifies expected diagnostics.
//!
//! Convention:
//!   - Files with `// lint:expect <rule>` MUST produce at least one diagnostic
//!     matching the named rule.
//!   - Files without `lint:expect` MUST produce zero diagnostics.
//!
//! Issue: #1232

use mvl::mvl::linter::{config::LintConfig, lint};
use mvl::mvl::parser::Parser;

fn lint_file(src: &str) -> Vec<String> {
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    let cfg = LintConfig::default();
    let result = lint(&prog, src, &cfg);
    result.diags.iter().map(|d| d.rule.to_string()).collect()
}

fn assert_lint_has_rule(src: &str, expected_rule: &str, file_label: &str) {
    let rules = lint_file(src);
    assert!(
        rules.iter().any(|r| r == expected_rule),
        "{file_label}: expected lint rule `{expected_rule}` but got: {rules:?}"
    );
}

fn assert_lint_clean(src: &str, file_label: &str) {
    let rules = lint_file(src);
    assert!(
        rules.is_empty(),
        "{file_label}: expected no lint diagnostics but got: {rules:?}"
    );
}

// ── Clean programs ───────────────────────────────────────────────────────────

#[test]
fn lint_clean_program_no_warnings() {
    let src = include_str!("corpus/03_linting/clean_program.mvl");
    assert_lint_clean(src, "clean_program.mvl");
}

// ── Complexity rules ─────────────────────────────────────────────────────────

#[test]
fn lint_complexity_cyclomatic() {
    let src = include_str!("corpus/03_linting/complexity_demo.mvl");
    assert_lint_has_rule(src, "complexity-cyclomatic", "complexity_demo.mvl");
}

#[test]
fn lint_complexity_match_depth() {
    let src = include_str!("corpus/03_linting/complexity_demo.mvl");
    assert_lint_has_rule(src, "complexity-match-depth", "complexity_demo.mvl");
}

#[test]
fn lint_complexity_effect_width() {
    let src = include_str!("corpus/03_linting/complexity_demo.mvl");
    assert_lint_has_rule(src, "complexity-effect-width", "complexity_demo.mvl");
}

#[test]
fn lint_complexity_trait_impl_count() {
    let src = include_str!("corpus/03_linting/complexity_demo.mvl");
    assert_lint_has_rule(src, "complexity-trait-impl-count", "complexity_demo.mvl");
}

// ── Semantic rules ───────────────────────────────────────────────────────────

#[test]
fn lint_unreachable_code() {
    let src = include_str!("corpus/03_linting/unreachable_code.mvl");
    assert_lint_has_rule(src, "unreachable-code", "unreachable_code.mvl");
}

#[test]
fn lint_missing_totality() {
    let src = include_str!("corpus/03_linting/missing_totality.mvl");
    assert_lint_has_rule(src, "missing-totality", "missing_totality.mvl");
}

// ── Effect rules ─────────────────────────────────────────────────────────────

#[test]
fn lint_redundant_effects() {
    let src = include_str!("corpus/03_linting/redundant_effects.mvl");
    assert_lint_has_rule(src, "redundant-effects", "redundant_effects.mvl");
}

// ── Suggestion rules ─────────────────────────────────────────────────────────

#[test]
fn lint_for_iter_antipattern() {
    let src = include_str!("corpus/03_linting/for_iter_antipattern.mvl");
    assert_lint_has_rule(src, "for-iter-antipattern", "for_iter_antipattern.mvl");
}

#[test]
fn lint_while_to_for_range() {
    let src = include_str!("corpus/03_linting/while_to_for_range.mvl");
    assert_lint_has_rule(src, "while-to-for-range", "while_to_for_range.mvl");
}

// ── Cross-rule: for_iter_antipattern also triggers while-to-for-range ────────

#[test]
fn lint_for_iter_also_triggers_while_to_for_range() {
    let src = include_str!("corpus/03_linting/for_iter_antipattern.mvl");
    assert_lint_has_rule(src, "while-to-for-range", "for_iter_antipattern.mvl");
}

// ── Naming rules ─────────────────────────────────────────────────────────────

#[test]
fn lint_naming_conventions_clean() {
    let src = include_str!("corpus/03_linting/naming_conventions.mvl");
    // Good names should produce no naming diagnostics
    let rules = lint_file(src);
    assert!(
        rules.iter().all(|r| r != "naming-convention"),
        "naming_conventions.mvl: unexpected naming-convention diagnostics: {rules:?}"
    );
}

// ── Pipeline validation: parse errors prevent linting ────────────────────────

#[test]
fn lint_all_corpus_files_parse_cleanly() {
    let files: &[(&str, &str)] = &[
        (
            "clean_program",
            include_str!("corpus/03_linting/clean_program.mvl"),
        ),
        (
            "complexity_demo",
            include_str!("corpus/03_linting/complexity_demo.mvl"),
        ),
        ("fn_length", include_str!("corpus/03_linting/fn_length.mvl")),
        (
            "for_iter_antipattern",
            include_str!("corpus/03_linting/for_iter_antipattern.mvl"),
        ),
        (
            "missing_totality",
            include_str!("corpus/03_linting/missing_totality.mvl"),
        ),
        (
            "naming_conventions",
            include_str!("corpus/03_linting/naming_conventions.mvl"),
        ),
        (
            "redundant_effects",
            include_str!("corpus/03_linting/redundant_effects.mvl"),
        ),
        (
            "redundant_match",
            include_str!("corpus/03_linting/redundant_match.mvl"),
        ),
        (
            "trailing_whitespace",
            include_str!("corpus/03_linting/trailing_whitespace.mvl"),
        ),
        (
            "unreachable_code",
            include_str!("corpus/03_linting/unreachable_code.mvl"),
        ),
        (
            "while_to_for_range",
            include_str!("corpus/03_linting/while_to_for_range.mvl"),
        ),
    ];
    for (name, src) in files {
        let (mut p, _) = Parser::new(src);
        let _ = p.parse_program();
        assert!(
            p.errors().is_empty(),
            "{name}: parse errors in lint corpus: {:?}",
            p.errors()
        );
    }
}
