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

use std::path::Path;

use mvl::mvl::linter::{config::LintConfig, lint};
use mvl::mvl::parser::Parser;

fn lint_file(src: &str) -> Vec<String> {
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    let cfg = LintConfig::default();
    let result = lint(&prog, src, &cfg, Path::new(""));
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
    let src = include_str!("fixtures/linting/clean_program.mvl");
    assert_lint_clean(src, "clean_program.mvl");
}

// ── Complexity rules ─────────────────────────────────────────────────────────

#[test]
fn lint_complexity_cyclomatic() {
    let src = include_str!("fixtures/linting/complexity_demo.mvl");
    assert_lint_has_rule(src, "complexity-cyclomatic", "complexity_demo.mvl");
}

#[test]
fn lint_complexity_match_depth() {
    let src = include_str!("fixtures/linting/complexity_demo.mvl");
    assert_lint_has_rule(src, "complexity-match-depth", "complexity_demo.mvl");
}

#[test]
fn lint_complexity_effect_width() {
    let src = include_str!("fixtures/linting/complexity_demo.mvl");
    assert_lint_has_rule(src, "complexity-effect-width", "complexity_demo.mvl");
}

#[test]
fn lint_complexity_trait_impl_count() {
    let src = include_str!("fixtures/linting/complexity_demo.mvl");
    assert_lint_has_rule(src, "complexity-trait-impl-count", "complexity_demo.mvl");
}

// ── Semantic rules ───────────────────────────────────────────────────────────

#[test]
fn lint_unreachable_code() {
    let src = include_str!("fixtures/linting/unreachable_code.mvl");
    assert_lint_has_rule(src, "unreachable-code", "unreachable_code.mvl");
}

#[test]
fn lint_missing_totality() {
    let src = include_str!("fixtures/linting/missing_totality.mvl");
    assert_lint_has_rule(src, "missing-totality", "missing_totality.mvl");
}

// ── Effect rules ─────────────────────────────────────────────────────────────

#[test]
fn lint_redundant_effects() {
    let src = include_str!("fixtures/linting/redundant_effects.mvl");
    assert_lint_has_rule(src, "redundant-effects", "redundant_effects.mvl");
}

// ── Suggestion rules ─────────────────────────────────────────────────────────

#[test]
fn lint_for_iter_antipattern() {
    let src = include_str!("fixtures/linting/for_iter_antipattern.mvl");
    assert_lint_has_rule(src, "for-iter-antipattern", "for_iter_antipattern.mvl");
}

#[test]
fn lint_while_to_for_range() {
    let src = include_str!("fixtures/linting/while_to_for_range.mvl");
    assert_lint_has_rule(src, "while-to-for-range", "while_to_for_range.mvl");
}

// ── Cross-rule: for_iter_antipattern also triggers while-to-for-range ────────

#[test]
fn lint_for_iter_also_triggers_while_to_for_range() {
    let src = include_str!("fixtures/linting/for_iter_antipattern.mvl");
    assert_lint_has_rule(src, "while-to-for-range", "for_iter_antipattern.mvl");
}

// ── Naming rules ─────────────────────────────────────────────────────────────

#[test]
fn lint_naming_conventions_clean() {
    let src = include_str!("fixtures/linting/naming_conventions.mvl");
    // Good names should produce no naming diagnostics
    let rules = lint_file(src);
    assert!(
        rules.iter().all(|r| r != "naming-convention"),
        "naming_conventions.mvl: unexpected naming-convention diagnostics: {rules:?}"
    );
}

// ── New semantic rules (#1373, #1465, #1466) ─────────────────────────────────

#[test]
fn lint_unused_function() {
    let src = include_str!("fixtures/linting/unused_function.mvl");
    assert_lint_has_rule(src, "unused-function", "unused_function.mvl");
}

#[test]
fn lint_silent_result_discard() {
    let src = include_str!("fixtures/linting/silent_result_discard.mvl");
    assert_lint_has_rule(src, "silent-result-discard", "silent_result_discard.mvl");
}

#[test]
fn lint_relabel_tag_hygiene() {
    let src = include_str!("fixtures/linting/relabel_tag_hygiene.mvl");
    assert_lint_has_rule(src, "relabel-tag-hygiene", "relabel_tag_hygiene.mvl");
}

#[test]
fn lint_allow_comment_suppresses_rule() {
    // A `// allow: <rule-id> <reason>` comment on the preceding line suppresses the diagnostic.
    let src = concat!(
        "fn parse(s: String) -> Result[Int, String] { Ok(0) }\n",
        "fn main() -> Unit {\n",
        "    // allow: silent-result-discard background fire-and-forget\n",
        "    parse(\"hello\");\n",
        "}\n",
    );
    let rules = lint_file(src);
    assert!(
        rules.iter().all(|r| r != "silent-result-discard"),
        "allow comment must suppress silent-result-discard; got: {rules:?}"
    );
}

// ── test-shadow rule (#1901) ─────────────────────────────────────────────────

fn lint_file_at(src: &str, path: &Path) -> Vec<String> {
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    let cfg = LintConfig::default();
    let result = lint(&prog, src, &cfg, path);
    result.diags.iter().map(|d| d.rule.to_string()).collect()
}

#[test]
fn test_shadow_type_decl_in_test_file_fires() {
    let src = "type Alias = struct { x: Int }\n";
    let rules = lint_file_at(src, Path::new("foo_test.mvl"));
    assert!(
        rules.iter().any(|r| r == "test-shadow"),
        "expected test-shadow for type decl in test file, got: {rules:?}"
    );
}

#[test]
fn test_shadow_type_decl_not_in_test_file_clean() {
    let src = "type Alias = struct { x: Int }\n";
    let rules = lint_file_at(src, Path::new("foo.mvl"));
    assert!(
        rules.iter().all(|r| r != "test-shadow"),
        "test-shadow must not fire for non-test files, got: {rules:?}"
    );
}

#[test]
fn test_shadow_disabled_via_config() {
    let src = "type Alias = struct { x: Int }\n";
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let cfg = LintConfig {
        test_shadow: false,
        ..LintConfig::default()
    };
    let result = lint(&prog, src, &cfg, Path::new("foo_test.mvl"));
    let rules: Vec<String> = result.diags.iter().map(|d| d.rule.to_string()).collect();
    assert!(
        rules.iter().all(|r| r != "test-shadow"),
        "test-shadow must be suppressed when disabled, got: {rules:?}"
    );
}

// ── Pipeline validation: parse errors prevent linting ────────────────────────

#[test]
fn lint_all_corpus_files_parse_cleanly() {
    let files: &[(&str, &str)] = &[
        (
            "clean_program",
            include_str!("fixtures/linting/clean_program.mvl"),
        ),
        (
            "complexity_demo",
            include_str!("fixtures/linting/complexity_demo.mvl"),
        ),
        ("fn_length", include_str!("fixtures/linting/fn_length.mvl")),
        (
            "for_iter_antipattern",
            include_str!("fixtures/linting/for_iter_antipattern.mvl"),
        ),
        (
            "missing_totality",
            include_str!("fixtures/linting/missing_totality.mvl"),
        ),
        (
            "naming_conventions",
            include_str!("fixtures/linting/naming_conventions.mvl"),
        ),
        (
            "unused_function",
            include_str!("fixtures/linting/unused_function.mvl"),
        ),
        (
            "silent_result_discard",
            include_str!("fixtures/linting/silent_result_discard.mvl"),
        ),
        (
            "relabel_tag_hygiene",
            include_str!("fixtures/linting/relabel_tag_hygiene.mvl"),
        ),
        (
            "redundant_effects",
            include_str!("fixtures/linting/redundant_effects.mvl"),
        ),
        (
            "redundant_match",
            include_str!("fixtures/linting/redundant_match.mvl"),
        ),
        (
            "trailing_whitespace",
            include_str!("fixtures/linting/trailing_whitespace.mvl"),
        ),
        (
            "unreachable_code",
            include_str!("fixtures/linting/unreachable_code.mvl"),
        ),
        (
            "while_to_for_range",
            include_str!("fixtures/linting/while_to_for_range.mvl"),
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
