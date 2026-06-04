// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Round-trip tests: `check(fmt(src))` must be semantically equivalent to `check(src)`.
//!
//! The formatter must not introduce or remove type errors.  We verify this by
//! comparing `req_errors` (error counts per requirement) and total error counts
//! between the original and formatted source.  Spans are deliberately excluded
//! from the comparison because formatting legitimately changes line numbers.
//!
//! We also verify idempotency: `fmt(fmt(src)) == fmt(src)` — formatting twice
//! should produce the same output as formatting once.
//!
//! Run with: `cargo test --test fmt_roundtrip`

use mvl::mvl::checker::check;
use mvl::mvl::parser::Parser;
use mvl::mvl::printer::format_source;

/// Parse and check a source string; return `(total_errors, req_errors)`.
fn check_src(src: &str) -> (usize, [usize; 12]) {
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let result = check(&prog);
    (result.errors.len(), result.req_errors)
}

/// Assert that formatting `src` does not change the checker results.
///
/// Compares total error count and per-requirement error counts so that the test
/// is independent of span positions (which legitimately change after formatting).
fn assert_fmt_preserves_check(src: &str, label: &str) {
    let formatted =
        format_source(src).unwrap_or_else(|e| panic!("{label}: format_source failed: {e}"));

    let (orig_count, orig_req) = check_src(src);
    let (fmt_count, fmt_req) = check_src(&formatted);

    assert_eq!(
        orig_count, fmt_count,
        "{label}: total error count changed after formatting ({orig_count} → {fmt_count})"
    );
    assert_eq!(
        orig_req, fmt_req,
        "{label}: per-requirement error counts changed after formatting\n  before: {orig_req:?}\n  after:  {fmt_req:?}"
    );
}

/// Assert that `format_source` is idempotent: `fmt(fmt(src)) == fmt(src)`.
fn assert_fmt_idempotent(src: &str, label: &str) {
    let first =
        format_source(src).unwrap_or_else(|e| panic!("{label}: first format_source failed: {e}"));
    let second = format_source(&first)
        .unwrap_or_else(|e| panic!("{label}: second format_source failed: {e}"));
    assert_eq!(
        first, second,
        "{label}: formatter is not idempotent (second pass changed output)"
    );
}

// ── Basics ────────────────────────────────────────────────────────────────────

#[test]
fn roundtrip_expressions() {
    let src = include_str!("corpus/01_syntax/expressions.mvl");
    assert_fmt_preserves_check(src, "01_syntax/expressions");
    assert_fmt_idempotent(src, "01_syntax/expressions");
}

#[test]
fn roundtrip_functions() {
    let src = include_str!("corpus/02_functions/functions.mvl");
    assert_fmt_preserves_check(src, "02_functions/functions");
    assert_fmt_idempotent(src, "02_functions/functions");
}

#[test]
fn roundtrip_statements() {
    let src = include_str!("corpus/01_syntax/statements.mvl");
    assert_fmt_preserves_check(src, "01_syntax/statements");
    assert_fmt_idempotent(src, "01_syntax/statements");
}

#[test]
fn roundtrip_literals() {
    let src = include_str!("corpus/01_syntax/literals.mvl");
    assert_fmt_preserves_check(src, "01_syntax/literals");
    assert_fmt_idempotent(src, "01_syntax/literals");
}

#[test]
fn roundtrip_keywords() {
    let src = include_str!("corpus/01_syntax/keywords.mvl");
    assert_fmt_preserves_check(src, "01_syntax/keywords");
    assert_fmt_idempotent(src, "01_syntax/keywords");
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[test]
fn roundtrip_basic_types() {
    let src = include_str!("corpus/03_types/basic_types.mvl");
    assert_fmt_preserves_check(src, "03_types/basic_types");
    assert_fmt_idempotent(src, "03_types/basic_types");
}

#[test]
fn roundtrip_structs() {
    let src = include_str!("corpus/03_types/structs.mvl");
    assert_fmt_preserves_check(src, "03_types/structs");
    assert_fmt_idempotent(src, "03_types/structs");
}

#[test]
fn roundtrip_enums() {
    let src = include_str!("corpus/03_types/enums.mvl");
    assert_fmt_preserves_check(src, "03_types/enums");
    assert_fmt_idempotent(src, "03_types/enums");
}

#[test]
fn roundtrip_option_result() {
    let src = include_str!("corpus/03_types/option_result.mvl");
    assert_fmt_preserves_check(src, "03_types/option_result");
    assert_fmt_idempotent(src, "03_types/option_result");
}

#[test]
fn roundtrip_exhaustive_match() {
    let src = include_str!("corpus/03_types/exhaustive_match.mvl");
    assert_fmt_preserves_check(src, "03_types/exhaustive_match");
    assert_fmt_idempotent(src, "03_types/exhaustive_match");
}

#[test]
fn roundtrip_immutability() {
    let src = include_str!("corpus/03_types/immutability.mvl");
    assert_fmt_preserves_check(src, "03_types/immutability");
    assert_fmt_idempotent(src, "03_types/immutability");
}

#[test]
fn roundtrip_refinements() {
    let src = include_str!("corpus/03_types/refinements.mvl");
    assert_fmt_preserves_check(src, "03_types/refinements");
    assert_fmt_idempotent(src, "03_types/refinements");
}

// ── Ownership ─────────────────────────────────────────────────────────────────

#[test]
fn roundtrip_ownership() {
    let src = include_str!("corpus/06_ownership/ownership.mvl");
    assert_fmt_preserves_check(src, "06_ownership/ownership");
    assert_fmt_idempotent(src, "06_ownership/ownership");
}

// ── Effects ───────────────────────────────────────────────────────────────────

#[test]
fn roundtrip_effects() {
    let src = include_str!("corpus/07_effects/pure_vs_effectful.mvl");
    assert_fmt_preserves_check(src, "07_effects/effects");
    assert_fmt_idempotent(src, "07_effects/effects");
}

// ── Termination ───────────────────────────────────────────────────────────────

#[test]
fn roundtrip_termination() {
    let src = include_str!("corpus/10_termination/total_vs_partial.mvl");
    assert_fmt_preserves_check(src, "10_termination/termination");
    assert_fmt_idempotent(src, "10_termination/termination");
}

// ── Contracts ─────────────────────────────────────────────────────────────────

#[test]
fn roundtrip_contracts() {
    let src = include_str!("corpus/11_contracts/basic_contracts.mvl");
    assert_fmt_preserves_check(src, "11_contracts/contracts");
    assert_fmt_idempotent(src, "11_contracts/contracts");
}
