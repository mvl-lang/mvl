//! Integration tests for the MVL → Rust transpiler (Epic 5, issues #29–#34).
//!
//! Each test follows the pattern:
//!   1. Parse MVL source
//!   2. Transpile to Rust source string
//!   3. Assert expected Rust snippets are present in the output

use mvl::mvl::parser::Parser;
use mvl::mvl::transpiler::transpile;

// ── Helpers ───────────────────────────────────────────────────────────────

fn transpile_src(src: &str) -> String {
    let (mut parser, lex_errors) = Parser::new(src);
    assert!(lex_errors.is_empty(), "lex errors: {lex_errors:?}");
    let prog = parser.parse_program();
    assert!(
        parser.errors().is_empty(),
        "parse errors: {:?}",
        parser.errors()
    );
    let out = transpile(&prog, "test_crate");
    out.lib_rs
}

fn assert_contains(src: &str, snippet: &str) {
    assert!(
        src.contains(snippet),
        "expected to find:\n  {snippet:?}\nin:\n{src}"
    );
}

// ── #29: Type declarations ────────────────────────────────────────────────

/// Requirement 1 / Scenario: MVL struct → Rust pub struct
#[test]
fn struct_transpiles_to_pub_struct() {
    let src = "type Point = struct { x: Float, y: Float }";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub struct Point {");
    assert_contains(&rust, "pub x: f64,");
    assert_contains(&rust, "pub y: f64,");
}

/// Requirement 1 / Scenario: MVL enum → Rust pub enum
#[test]
fn enum_transpiles_to_pub_enum() {
    let src = "type Color = enum { Red, Green, Blue }";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub enum Color {");
    assert_contains(&rust, "Red,");
    assert_contains(&rust, "Green,");
    assert_contains(&rust, "Blue,");
}

/// Requirement 1 / Scenario: Enum with struct variant
#[test]
fn enum_struct_variant_transpiles() {
    let src = "type Err = enum { NotFound, WithMsg { msg: String } }";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub enum Err {");
    assert_contains(&rust, "NotFound,");
    assert_contains(&rust, "WithMsg {");
    assert_contains(&rust, "msg: String,");
}

/// Requirement 3 / Scenario: Type alias → Rust type alias
#[test]
fn plain_alias_transpiles_to_type_alias() {
    let src = "type Name = String";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub type Name = String;");
}

/// Requirement 10 / Scenario: Refined type alias → Rust newtype with debug_assert
#[test]
fn refined_alias_transpiles_to_newtype() {
    let src = "type PositiveInt = Int where self > 0";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub struct PositiveInt(pub i64)");
    assert_contains(&rust, "pub fn new(v: i64) -> Self");
    assert_contains(&rust, "debug_assert!(");
    assert_contains(&rust, "(v > 0)");
}

/// Requirement 10 / Scenario: Refined type alias with float predicate
#[test]
fn refined_alias_float_predicate_transpiles() {
    let src = "type NonNegative = Float where self >= 0.0";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub struct NonNegative(pub f64)");
    assert_contains(&rust, "debug_assert!(");
    assert_contains(&rust, "(v >= 0.0)");
}

/// Requirement 11 / Scenario: Security label preamble always emitted
#[test]
fn security_preamble_always_emitted() {
    let src = "type X = Int";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub struct Public<T>(pub T);");
    assert_contains(&rust, "pub struct Tainted<T>(pub T);");
    assert_contains(&rust, "pub struct Secret<T>(pub T);");
    assert_contains(&rust, "pub struct Clean<T>(pub T);");
    assert_contains(&rust, "pub fn sanitize<T>");
    assert_contains(&rust, "pub fn declassify<T>");
}

/// Requirement 11 / Scenario: Security labeled fields in struct
#[test]
fn struct_with_labeled_fields_transpiles() {
    let src = "type Session = struct { token: Secret<String>, visible: Public<Int> }";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub token: Secret<String>,");
    assert_contains(&rust, "pub visible: Public<i64>,");
}

// ── #30: Function declarations ────────────────────────────────────────────

/// Requirement 2 / Scenario: Simple function
#[test]
fn simple_fn_transpiles() {
    let src = "fn add(a: Int, b: Int) -> Int { a + b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub fn add(a: i64, b: i64) -> i64 {");
}

/// Requirement 8 / Scenario: Total function → doc comment
#[test]
fn total_fn_emits_doc_comment() {
    let src = "total fn square(x: Int) -> Int { x * x }";
    let rust = transpile_src(src);
    assert_contains(&rust, "/// # Totality");
    assert_contains(&rust, "pub fn square(x: i64) -> i64 {");
}

/// Requirement 7 / Scenario: Effects → doc comment
#[test]
fn effectful_fn_emits_effects_doc() {
    let src = "fn log_value(x: Int) -> Unit ! Console { x }";
    let rust = transpile_src(src);
    assert_contains(&rust, "/// # Effects: Console");
    assert_contains(&rust, "pub fn log_value(x: i64) -> () {");
}

/// Requirement 9 / Scenario: Capability parameter → comment
#[test]
fn capability_param_emits_comment() {
    let src = "fn use_conn(iso conn: &Int) -> Int { 0 }";
    let rust = transpile_src(src);
    assert_contains(&rust, "/* iso */");
    assert_contains(&rust, "conn: &i64");
}

/// Requirement 4 / Scenario: Option return type transpiles
#[test]
fn option_return_type_transpiles() {
    let src = "fn find(x: Int) -> Option<Int> { x }";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub fn find(x: i64) -> Option<i64> {");
}

/// Requirement 5 / Scenario: Result return type transpiles
#[test]
fn result_return_type_transpiles() {
    let src = "type MyErr = enum { Oops }  fn risky(x: Int) -> Result<Int, MyErr> { x }";
    let rust = transpile_src(src);
    assert_contains(&rust, "-> Result<i64, MyErr>");
}

// ── #31: Security labels ──────────────────────────────────────────────────

/// Requirement 11 / Scenario: Labeled parameter type transpiles
#[test]
fn labeled_param_transpiles() {
    let src = "fn process(input: Tainted<String>) -> Clean<String> { sanitize(input) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "input: Tainted<String>");
    assert_contains(&rust, "-> Clean<String>");
    assert_contains(&rust, "sanitize(input)");
}

/// Requirement 11 / Scenario: Declassify expression transpiles
#[test]
fn declassify_expr_transpiles() {
    let src = "fn reveal(s: Secret<Int>) -> Public<Int> { declassify(s) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "declassify(s)");
}

// ── #32: Refinement types ─────────────────────────────────────────────────

/// Requirement 10 / Scenario: Struct field refinement → constructor with debug_assert
#[test]
fn struct_field_refinement_emits_constructor() {
    let src = "type Age = struct { value: Int where self >= 0 }";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub fn new(value: i64) -> Self {");
    assert_contains(&rust, "debug_assert!(");
    assert_contains(&rust, "(value >= 0)");
}

// ── Corpus roundtrip tests ────────────────────────────────────────────────

/// Parse and transpile every corpus file that is known to parse cleanly.
/// The test just checks that transpilation does not panic.
#[test]
fn corpus_structs_transpiles() {
    let src = include_str!("corpus/02_types/structs.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "pub struct");
}

#[test]
fn corpus_enums_transpiles() {
    let src = include_str!("corpus/02_types/enums.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "pub enum");
}

#[test]
fn corpus_option_result_transpiles() {
    let src = include_str!("corpus/02_types/option_result.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "pub fn");
}

#[test]
fn corpus_ifc_label_types_transpiles() {
    let src = include_str!("corpus/05_ifc/label_types.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "pub struct Public<T>");
}

#[test]
fn corpus_total_vs_partial_transpiles() {
    let src = include_str!("corpus/07_termination/total_vs_partial.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "/// # Totality");
}

// ── #33: Full program transpilation ──────────────────────────────────────

/// The safe_division.mvl reference example transpiles without panicking.
#[test]
fn full_program_safe_division_transpiles() {
    let src = include_str!("corpus/09_full_programs/safe_division.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "pub struct Amount");
    assert_contains(&rust, "pub struct NonZero");
    assert_contains(&rust, "pub enum DivError");
    assert_contains(&rust, "pub fn safe_divide");
    assert_contains(&rust, "pub fn calculate_share");
    assert_contains(&rust, "/// # Totality");
    assert_contains(&rust, "/// # Effects: Console");
}

/// The auth_handler.mvl reference example transpiles without panicking.
#[test]
fn full_program_auth_handler_transpiles() {
    let src = include_str!("corpus/09_full_programs/auth_handler.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "pub struct UserId");
    assert_contains(&rust, "pub enum AuthError");
    assert_contains(&rust, "pub struct Session");
    assert_contains(&rust, "pub fn authenticate");
    assert_contains(&rust, "/// # Totality");
    assert_contains(&rust, "/// # Effects: DB, Console");
}
