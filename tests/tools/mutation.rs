//! Regression tests for mutation count inflation (issue #330).
//!
//! Root cause: `emit_expr` was called N+1 times for each operand of a binary
//! expression in mutation mode (once per variant arm + once for the default arm).
//! Sub-expressions with their own mutation points therefore allocated N+1 sets of
//! mutation IDs, causing exponential growth.
//!
//! Fix: hoist left/right into temp `let` bindings before the match block so each
//! sub-expression is emitted — and its mutations allocated — exactly once.

use mvl::mvl::parser::Parser;
use mvl::mvl::transpiler::transpile_mutated_source_with_prelude;

fn parse(src: &str) -> mvl::mvl::parser::ast::Program {
    let (mut p, lex_errs) = Parser::new(src);
    assert!(lex_errs.is_empty(), "lex errors: {lex_errs:?}");
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    prog
}

/// `x < 92` — the comparison operator contributes 3 mutation variants and the
/// integer literal `92` contributes 5 (0, 1, -1, 91, 93).
///
/// Before the fix `emit_expr(right)` was called 4 times (3 variants + 1 default),
/// allocating 4 × 5 = 20 literal mutations instead of 5.  Total was 23, not 8.
#[test]
fn binary_with_int_literal_rhs_produces_exact_mutation_count() {
    let src = "fn f(x: Int) -> Bool { x < 92 }";
    let prog = parse(src);
    let (_out, mutants) = transpile_mutated_source_with_prelude(&prog, "my_crate", "f", &[]);

    // Verify no duplicate mutation IDs (a symptom of the inflation bug).
    let ids: Vec<&str> = mutants.iter().map(|m| m.id.as_str()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "duplicate mutation IDs detected — inflation bug present\n  ids: {ids:?}"
    );

    // 3 operator variants for `<`  +  5 literal variants for `92`  =  8 total.
    assert_eq!(
        mutants.len(),
        8,
        "expected 8 mutations (3 for `<` + 5 for literal 92), got {}\n  mutants: {:#?}",
        mutants.len(),
        mutants
    );
}

/// Compound: `(a < b) || (a > 0)`.
///
/// Expected breakdown:
///   `||`  → 1 variant
///   `<`   → 3 variants
///   `>`   → 3 variants
///   `0`   → 2 variants (1, -1)
///   Total = 9
///
/// The inflation bug caused `emit_expr` on each operand of `||` to run twice,
/// then each operand of `<`/`>` to run four times, cascading to a much higher count.
#[test]
fn compound_binary_expression_produces_exact_mutation_count() {
    let src = "fn f(a: Int, b: Int) -> Bool { (a < b) || (a > 0) }";
    let prog = parse(src);
    let (_out, mutants) = transpile_mutated_source_with_prelude(&prog, "my_crate", "f", &[]);

    let ids: Vec<&str> = mutants.iter().map(|m| m.id.as_str()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "duplicate mutation IDs detected — inflation bug present\n  ids: {ids:?}"
    );

    // 1(||) + 3(<) + 3(>) + 2(0) = 9
    assert_eq!(
        mutants.len(),
        9,
        "expected 9 mutations (1+3+3+2), got {}\n  mutants: {:#?}",
        mutants.len(),
        mutants
    );
}
