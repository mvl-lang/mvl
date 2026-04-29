//! Integration tests for parser error recovery and diagnostics (Requirement 8).
//!
//! Tests that the parser:
//! 1. Reports multiple errors per file (not just the first one)
//! 2. Includes accurate line and column numbers in each error
//! 3. Recovers and continues parsing after an error

use mvl::mvl::parser::diagnostics::render_errors;
use mvl::mvl::parser::Parser;

// ── Requirement 8 / Scenario: Multiple errors reported ───────────────────────

#[test]
fn parser_reports_multiple_errors() {
    // GIVEN: a source file with three syntax errors
    // WHEN: parsed
    // THEN: the parser MUST report all three errors, not just the first
    let src = include_str!("../integration/error_messages/multiple_errors.mvl");
    let (mut p, _) = Parser::new(src);
    let _prog = p.parse_program();
    // There are 3 broken functions; parser should report at least 3 errors
    assert!(
        p.errors().len() >= 3,
        "expected at least 3 errors, got {}: {:?}",
        p.errors().len(),
        p.errors()
    );
}

// ── Requirement 8 / Scenario: Error with source location ─────────────────────

#[test]
fn error_includes_line_and_column() {
    // GIVEN: a syntax error at a known position
    // THEN: the error span has correct line/col (non-zero)
    let src = "fn bad( -> Int { }";
    let (mut p, _) = Parser::new(src);
    let _ = p.parse_fn_decl();
    assert!(!p.errors().is_empty(), "expected parse errors");
    let first = &p.errors()[0];
    assert!(first.span.line >= 1, "line must be >= 1");
    assert!(first.span.col >= 1, "col must be >= 1");
}

// ── Requirement 8 / Scenario: Recovery after error ───────────────────────────

#[test]
fn parser_recovers_after_error() {
    // GIVEN: `fn broken( { }` followed by `fn valid() -> Int { 42 }`
    // WHEN: parsed
    // THEN: `broken` produces an error AND `valid` parses successfully
    let src = "fn broken( { }\nfn valid() -> Int { 42 }";
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    // At least one error from `broken`
    assert!(
        !p.errors().is_empty(),
        "expected errors from broken function"
    );
    // `valid` should have been parsed successfully — check declarations
    let fn_names: Vec<_> = prog
        .declarations
        .iter()
        .filter_map(|d| match d {
            mvl::mvl::parser::ast::Decl::Fn(f) => Some(f.name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        fn_names.contains(&"valid"),
        "valid function should parse successfully after recovery, declarations: {:?}",
        fn_names
    );
}

// ── Diagnostic rendering ──────────────────────────────────────────────────────

#[test]
fn render_errors_format() {
    // GIVEN: a source with a syntax error
    // THEN: render_errors produces a non-empty string with line info
    let src = "fn f( -> Int { }";
    let (mut p, _) = Parser::new(src);
    let _ = p.parse_fn_decl();
    let rendered = render_errors(src, p.errors());
    assert!(!rendered.is_empty(), "expected rendered diagnostic output");
    // Should contain "error at"
    assert!(
        rendered.contains("error at"),
        "expected 'error at' in output:\n{}",
        rendered
    );
}
