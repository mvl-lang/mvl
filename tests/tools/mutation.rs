//! Regression tests for mutation count inflation (issue #330).
//!
//! Root cause: `emit_expr` was called N+1 times for each operand of a binary
//! expression in mutation mode (once per variant arm + once for the default arm).
//! Sub-expressions with their own mutation points therefore allocated N+1 sets of
//! mutation IDs, causing exponential growth.
//!
//! Fix: hoist left/right into temp `let` bindings before the match block so each
//! sub-expression is emitted — and its mutations allocated — exactly once.

use mvl::mvl::backends::rust::transpile_mutated_source_with_prelude;
use mvl::mvl::parser::Parser;

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
        "expected 9 mutations (1(||) + 3(<) + 3(>) + 2(0)), got {}\n  mutants: {:#?}",
        mutants.len(),
        mutants
    );
}

/// Three-level deep nesting: `(a + b) < (c - d)`.
///
/// Expected breakdown:
///   `<`  → 3 variants
///   `+`  → 4 variants
///   `-`  → 4 variants
///   Total = 11
///
/// Verifies that the hoisting fix holds under full recursion: the inner `+` and `-`
/// nodes each enter the mutation path and allocate their own unique temp-var names.
#[test]
fn three_level_arithmetic_binary_produces_exact_mutation_count() {
    let src = "fn f(a: Int, b: Int, c: Int, d: Int) -> Bool { (a + b) < (c - d) }";
    let prog = parse(src);
    let (_out, mutants) = transpile_mutated_source_with_prelude(&prog, "my_crate", "f", &[]);

    let ids: Vec<&str> = mutants.iter().map(|m| m.id.as_str()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "duplicate mutation IDs detected — inflation bug present\n  ids: {ids:?}"
    );

    // 3(<) + 4(+) + 4(-) = 11
    assert_eq!(
        mutants.len(),
        11,
        "expected 11 mutations (3(<) + 4(+) + 4(-)), got {}\n  mutants: {:#?}",
        mutants.len(),
        mutants
    );
}

/// Integer literal on LHS: `5 < x`.
///
/// Expected breakdown:
///   `<`  → 3 variants
///   `5`  → 5 variants (0, 1, -1, 4, 6)
///   Total = 8
///
/// Confirms the left-operand hoisting path is exercised (the existing tests only
/// place mutable literals on the right-hand side).
#[test]
fn integer_literal_on_lhs_produces_exact_mutation_count() {
    let src = "fn f(x: Int) -> Bool { 5 < x }";
    let prog = parse(src);
    let (_out, mutants) = transpile_mutated_source_with_prelude(&prog, "my_crate", "f", &[]);

    let ids: Vec<&str> = mutants.iter().map(|m| m.id.as_str()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "duplicate mutation IDs detected — inflation bug present\n  ids: {ids:?}"
    );

    // 3(<) + 5(literal 5: 0,1,-1,4,6) = 8
    assert_eq!(
        mutants.len(),
        8,
        "expected 8 mutations (3(<) + 5(literal 5)), got {}\n  mutants: {:#?}",
        mutants.len(),
        mutants
    );
}

/// Left-associative chain: `a + b + c` parses as `(a + b) + c` — two `Add` nodes.
///
/// Expected breakdown:
///   outer `+`  → 4 variants
///   inner `+`  → 4 variants
///   Total = 8
///
/// Verifies that a same-operator repeated chain produces the correct count and that
/// temp-var names from the two `+` nodes do not collide.
#[test]
fn left_associative_chain_produces_exact_mutation_count() {
    let src = "fn f(a: Int, b: Int, c: Int) -> Int { a + b + c }";
    let prog = parse(src);
    let (_out, mutants) = transpile_mutated_source_with_prelude(&prog, "my_crate", "f", &[]);

    let ids: Vec<&str> = mutants.iter().map(|m| m.id.as_str()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "duplicate mutation IDs detected — inflation bug present\n  ids: {ids:?}"
    );

    // 4(outer +) + 4(inner +) = 8
    assert_eq!(
        mutants.len(),
        8,
        "expected 8 mutations (4 + 4 for two `+` nodes), got {}\n  mutants: {:#?}",
        mutants.len(),
        mutants
    );
}

/// Single-variant operator: `a && b` — `&&` has exactly one mutation variant (`||`).
///
/// Verifies the lower boundary: a binary with no literal sub-expressions and an
/// operator that has only one alternative produces exactly 1 mutation.
#[test]
fn single_variant_operator_produces_exact_mutation_count() {
    let src = "fn f(a: Bool, b: Bool) -> Bool { a && b }";
    let prog = parse(src);
    let (_out, mutants) = transpile_mutated_source_with_prelude(&prog, "my_crate", "f", &[]);

    let ids: Vec<&str> = mutants.iter().map(|m| m.id.as_str()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "duplicate mutation IDs detected — inflation bug present\n  ids: {ids:?}"
    );

    // &&→|| is the only variant
    assert_eq!(
        mutants.len(),
        1,
        "expected 1 mutation (&& → ||), got {}\n  mutants: {:#?}",
        mutants.len(),
        mutants
    );
}
