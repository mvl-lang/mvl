//! Integration tests for the MVL → Rust transpiler (Epic 5, issues #29–#34).
//!
//! Each test follows the pattern:
//!   1. Parse MVL source
//!   2. Transpile to Rust source string
//!   3. Assert expected Rust snippets are present in the output

use mvl::mvl::parser::Parser;
use mvl::mvl::transpiler::transpile;

// ── Helpers ───────────────────────────────────────────────────────────────

use mvl::mvl::transpiler::TranspileOutput;

fn transpile_src(src: &str) -> String {
    transpile_full(src).lib_rs
}

fn transpile_full(src: &str) -> TranspileOutput {
    let (mut parser, lex_errors) = Parser::new(src);
    assert!(lex_errors.is_empty(), "lex errors: {lex_errors:?}");
    let prog = parser.parse_program();
    assert!(
        parser.errors().is_empty(),
        "parse errors: {:?}",
        parser.errors()
    );
    transpile(&prog, "test_crate")
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
    let src = "type Session = struct { token: Secret[String], visible: Public[Int] }";
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
    let src = "fn use_conn(iso conn: val Int) -> Int { 0 }";
    let rust = transpile_src(src);
    assert_contains(&rust, "/* iso */");
    assert_contains(&rust, "conn: &i64");
}

/// Requirement 4 / Scenario: Option return type transpiles
#[test]
fn option_return_type_transpiles() {
    let src = "fn find(x: Int) -> Option[Int] { x }";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub fn find(x: i64) -> Option<i64> {");
}

/// Requirement 5 / Scenario: Result return type transpiles
#[test]
fn result_return_type_transpiles() {
    let src = "type MyErr = enum { Oops }  fn risky(x: Int) -> Result[Int, MyErr] { x }";
    let rust = transpile_src(src);
    assert_contains(&rust, "-> Result<i64, MyErr>");
}

// ── #31: Security labels ──────────────────────────────────────────────────

/// Requirement 11 / Scenario: Labeled parameter type transpiles
#[test]
fn labeled_param_transpiles() {
    let src = "fn process(input: Tainted[String]) -> Clean[String] { sanitize(input) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "input: Tainted<String>");
    assert_contains(&rust, "-> Clean<String>");
    assert_contains(&rust, "sanitize(input)");
}

/// Requirement 11 / Scenario: Declassify expression transpiles
#[test]
fn declassify_expr_transpiles() {
    let src = "fn reveal(s: Secret[Int]) -> Public[Int] { declassify(s) }";
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
    let src = include_str!("corpus/06_ifc/label_types.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "pub struct Public<T>");
}

#[test]
fn corpus_total_vs_partial_transpiles() {
    let src = include_str!("corpus/08_termination/total_vs_partial.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "/// # Totality");
}

// ── #33: Full program transpilation ──────────────────────────────────────

/// The safe_division.mvl reference example transpiles without panicking.
#[test]
fn full_program_safe_division_transpiles() {
    let src = include_str!("corpus/11_programs/safe_division.mvl");
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
    let src = include_str!("corpus/11_programs/auth_handler.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "pub struct UserId");
    assert_contains(&rust, "pub enum AuthError");
    assert_contains(&rust, "pub struct Session");
    assert_contains(&rust, "pub fn authenticate");
    assert_contains(&rust, "/// # Totality");
    assert_contains(&rust, "/// # Effects: Console");
}

// ── Extern "rust" blocks (#52, #91, #93) ──────────────────────────────────

/// extern "rust" block parses and transpiles to a Rust extern "Rust" block.
#[test]
fn extern_rust_block_transpiles() {
    let src = r#"extern "rust" {
    fn hash_password(password: String) -> String;
    fn verify_password(password: String, hash: String) -> Bool;
}"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "extern \"Rust\"");
    // `pub` is not valid inside Rust extern blocks
    assert_contains(&rust, "fn hash_password");
    assert_contains(&rust, "fn verify_password");
    // Security preamble replaced by mvl_runtime prelude
    assert_contains(&rust, "use mvl_runtime::prelude::*");
}

/// extern "rust" with declared effects emits the effect as a comment.
#[test]
fn extern_rust_fn_effects_emitted_as_comment() {
    let src = r#"extern "rust" {
    fn fetch_url(url: String) -> Result[String, String] ! Net;
}"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "// ! Net");
    // `pub` is not valid inside Rust extern blocks
    assert_contains(&rust, "fn fetch_url");
}

/// Programs without extern blocks keep the inlined security preamble.
#[test]
fn no_extern_uses_inline_preamble() {
    let src = "fn add(a: Int, b: Int) -> Int { a + b }";
    let rust = transpile_src(src);
    // No extern blocks → inline preamble, not runtime import
    assert!(
        !rust.contains("use mvl_runtime::prelude::*"),
        "no extern → should not use mvl_runtime: {rust}"
    );
    assert_contains(&rust, "pub struct Public");
}

/// Cargo.toml includes mvl_runtime dependency when extern blocks are present.
#[test]
fn extern_rust_adds_mvl_runtime_to_cargo_toml() {
    use mvl::mvl::transpiler::transpile;
    let src = r#"extern "rust" {
    fn greet(name: String) -> String;
}"#;
    let (mut p, _) = mvl::mvl::parser::Parser::new(src);
    let prog = p.parse_program();
    let out = transpile(&prog, "my_crate");
    assert!(
        out.cargo_toml.contains("mvl_runtime"),
        "Cargo.toml must reference mvl_runtime: {}",
        out.cargo_toml
    );
    assert_eq!(out.extern_count, 1);
}

/// Full password_checker.mvl parses, checks, and transpiles cleanly.
#[test]
fn full_program_password_checker_transpiles() {
    use mvl::mvl::checker::check;
    use mvl::mvl::transpiler::transpile;
    let src = include_str!("corpus/11_programs/password_checker.mvl");
    let (mut p, lex_errs) = mvl::mvl::parser::Parser::new(src);
    assert!(lex_errs.is_empty(), "lex errors: {lex_errs:?}");
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());

    let check_result = check(&prog);
    assert!(
        check_result.is_ok(),
        "check errors: {:?}",
        check_result.errors
    );
    assert_eq!(
        check_result.extern_count, 1,
        "should have 1 extern trust boundary"
    );

    let out = transpile(&prog, "password_checker");
    assert_contains(&out.lib_rs, "use mvl_runtime::prelude::*");
    assert_contains(&out.lib_rs, "extern \"Rust\"");
    // `pub` is not valid inside Rust extern blocks
    assert_contains(&out.lib_rs, "fn hash_password");
    assert_contains(&out.lib_rs, "fn verify_password");
    assert_contains(&out.lib_rs, "pub fn validate_password");
    assert_contains(&out.lib_rs, "pub fn hash_clean");
    assert_contains(&out.lib_rs, "pub fn verify_candidate");
    assert_contains(&out.lib_rs, "pub fn authenticate");
    assert_eq!(out.extern_count, 1);
    // Cargo.toml includes mvl_runtime
    assert!(
        out.cargo_toml.contains("mvl_runtime"),
        "Cargo.toml must reference mvl_runtime:\n{}",
        out.cargo_toml
    );
}

// ── Entry point: binary vs library inference ─────────────────────────────

#[test]
fn fn_main_produces_binary_crate() {
    let out = transpile_full("fn main() -> Unit { }");
    assert!(out.has_main, "fn main should produce a binary crate");
    // Binary Cargo.toml should NOT have [lib] section
    assert!(
        !out.cargo_toml.contains("[lib]"),
        "binary crate should not have [lib] in Cargo.toml:\n{}",
        out.cargo_toml
    );
}

#[test]
fn no_fn_main_produces_library_crate() {
    let out = transpile_full("fn add(a: Int, b: Int) -> Int { a + b }");
    assert!(!out.has_main, "no fn main should produce a library crate");
    // Library Cargo.toml should have [lib] section
    assert!(
        out.cargo_toml.contains("[lib]"),
        "library crate should have [lib] in Cargo.toml:\n{}",
        out.cargo_toml
    );
}

#[test]
fn fn_main_with_effects_produces_binary() {
    let out = transpile_full("fn main() -> Unit ! Console { println(\"hello\"); }");
    assert!(
        out.has_main,
        "effectful fn main should still produce a binary"
    );
}

#[test]
fn no_top_level_main_means_library() {
    let out = transpile_full(
        "type App = struct { }
         fn run() -> Int { 42 }",
    );
    assert!(!out.has_main, "no top-level fn main means library");
}

// ── #38: Test function transpilation ─────────────────────────────────────

/// Requirement: `test fn` is wrapped in `#[cfg(test)] mod tests { #[test] fn … }`
#[test]
fn test_fn_emits_cfg_test_block() {
    let src = "fn add(a: Int, b: Int) -> Int { a + b }\ntest fn check_add() -> Unit { }";
    let out = transpile_src(src);
    assert_contains(&out, "#[cfg(test)]");
    assert_contains(&out, "mod tests {");
    assert_contains(&out, "#[test]");
    assert_contains(&out, "fn check_add()");
    assert_contains(&out, "use super::*;");
}

#[test]
fn test_fn_not_pub() {
    // test functions must NOT be `pub` — Rust `#[test]` fns are private by convention
    let src = "test fn my_test() -> Unit { }";
    let out = transpile_src(src);
    assert_contains(&out, "fn my_test()");
    assert!(!out.contains("pub fn my_test"), "test fn should not be pub");
}

#[test]
fn no_test_fns_no_cfg_test_block() {
    let out = transpile_src("fn add(a: Int, b: Int) -> Int { a + b }");
    assert!(
        !out.contains("#[cfg(test)]"),
        "no test fns → no #[cfg(test)] block"
    );
}

// ── #65: Debug/Display traits + format() ──────────────────────────────────

/// format() maps to Rust's format!() macro.
#[test]
fn format_call_emits_format_macro() {
    let src = r#"fn greeting(name: String) -> String { format("{} world", name) }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "format!(");
    assert_contains(&rust, "\"{} world\"");
}

/// All structs automatically derive Debug, Clone, and PartialEq (Spec 009 Req 1).
#[test]
fn struct_derives_debug() {
    let src = "type Point = struct { x: Float, y: Float }";
    let rust = transpile_src(src);
    assert_contains(&rust, "#[derive(Debug, Clone, PartialEq)]");
}

/// All enums automatically derive Debug, Clone, and PartialEq (Spec 009 Req 1).
#[test]
fn enum_derives_debug() {
    let src = "type Color = enum { Red, Green, Blue }";
    let rust = transpile_src(src);
    assert_contains(&rust, "#[derive(Debug, Clone, PartialEq)]");
}

/// impl Display for T emits std::fmt::Display implementation.
#[test]
fn impl_display_emits_display_trait() {
    let src = r#"
type Point = struct { x: Float, y: Float }
impl Display for Point {
    fn fmt(self: Point) -> String {
        format("({}, {})", self.x, self.y)
    }
}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "impl std::fmt::Display for Point {");
    assert_contains(
        &rust,
        "fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {",
    );
    assert_contains(&rust, "write!(f, \"{}\",");
    assert_contains(&rust, "format!(");
}

/// Hex literals lex and transpile to their integer value.
#[test]
fn hex_literal_transpiles_to_integer() {
    let src = "fn mask() -> Int { 0xFF }";
    let rust = transpile_src(src);
    assert_contains(&rust, "255");
}

/// Binary literals lex and transpile to their integer value.
#[test]
fn binary_literal_transpiles_to_integer() {
    let src = "fn flags() -> Int { 0b1010 }";
    let rust = transpile_src(src);
    assert_contains(&rust, "10");
}

/// Octal literals lex and transpile to their integer value.
#[test]
fn octal_literal_transpiles_to_integer() {
    let src = "fn perms() -> Int { 0o755 }";
    let rust = transpile_src(src);
    assert_contains(&rust, "493");
}

/// Scientific notation transpiles to float literal.
#[test]
fn scientific_notation_transpiles_to_float() {
    let src = "fn big() -> Float { 1.5e10 }";
    let rust = transpile_src(src);
    assert_contains(&rust, "15000000000");
}

// ── From/Into conversion (#62) ────────────────────────────────────────────

/// `impl From[A] for B` emits a `std::convert::From` implementation.
#[test]
fn impl_from_emits_from_trait() {
    let src = r#"
type IoError = struct { msg: String }
type AppError = enum { Io(IoError), Other }
impl From[IoError] for AppError {
    fn from(e: IoError) -> Self {
        AppError::Io(e)
    }
}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "impl std::convert::From<IoError> for AppError {");
    assert_contains(&rust, "fn from(e: IoError) -> Self {");
    // Phase A: e is used exactly once — last use is a move, not a clone.
    assert_contains(&rust, "AppError::Io(e)");
}

/// `impl From[A] for B` with no `from` method emits a todo!().
#[test]
fn impl_from_without_method_emits_todo() {
    let src = r#"
type ParseError = struct { msg: String }
type MyError = enum { Parse(ParseError) }
impl From[ParseError] for MyError {}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "impl std::convert::From<ParseError> for MyError {");
    assert_contains(&rust, "todo!(\"From::from not implemented\")");
}

// ── #58/#66: Map/Set literals and multiline/raw strings ───────────────────────

/// Map literal emits HashMap::from([…]).
#[test]
fn map_literal_transpiles_to_hashmap_from() {
    let src = r#"fn f() -> Unit { let _m: Map[String, Int] = {"a": 1}; }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "std::collections::HashMap::from([");
    assert_contains(&rust, "\"a\".to_string()");
}

/// Set literal emits HashSet::from([…]).
#[test]
fn set_literal_transpiles_to_hashset_from() {
    let src = r#"fn f() -> Unit { let _s: Set[Int] = {1, 2, 3}; }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "std::collections::HashSet::from([");
}

/// Raw string backslashes are re-escaped in generated Rust output.
#[test]
fn raw_string_backslash_escaped_in_output() {
    let src = r#"fn f() -> String { r"C:\path" }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "C:\\\\path");
}

/// Multiline string with literal newline emits escape sequence in Rust output.
#[test]
fn multiline_string_newline_escaped_in_output() {
    let src = "fn f() -> String { \"\"\"hello\nworld\"\"\" }";
    let rust = transpile_src(src);
    assert_contains(&rust, "\\n");
}

// ── #68: Const generics — Array[T, N] ─────────────────────────────────────

/// Array[T, N] in a parameter type emits Rust fixed-size array syntax [T; N].
#[test]
fn array_type_emits_fixed_size_rust_array() {
    let src = "fn process(buf: Array[Byte, 16]) -> Int { 0 }";
    let rust = transpile_src(src);
    assert_contains(&rust, "[u8; 16]");
}

/// Array[T, N] as a return type emits [T; N].
#[test]
fn array_return_type_emits_fixed_size_rust_array() {
    let src = "fn zeros() -> Array[Int, 4] { [0, 0, 0, 0] }";
    let rust = transpile_src(src);
    assert_contains(&rust, "[i64; 4]");
}

/// A type alias with const generic param emits Rust const generic syntax.
#[test]
fn type_alias_with_const_generic_emits_rust_const_generic() {
    let src = "type FixedBuf[T, const N: Int] = struct { len: Int }";
    let rust = transpile_src(src);
    assert_contains(&rust, "const N: usize");
}

/// A function with a const generic param emits Rust const generic syntax.
#[test]
fn fn_with_const_generic_emits_rust_const_generic() {
    let src = "fn fill[T, const N: Int](item: T) -> Int { 0 }";
    let rust = transpile_src(src);
    assert_contains(&rust, "const N: usize");
}

// ── Spec 009 Req 7: For-loop iterable clone ───────────────────────────────

/// For-loop iterates over a cloned copy of the collection.
/// Spec 009 Req 7: iterable MUST be wrapped as `(expr).clone()`.
#[test]
fn for_loop_clone_expression() {
    let src = r#"
fn process(items: List[Int]) -> Unit ! Console {
    for x in items {
        println(x);
    }
}
"#;
    let rust = transpile_src(src);
    // The iterable must be cloned so `items` remains usable after the loop.
    assert_contains(&rust, "(items).clone()");
}

/// For-loop over a function call expression clones the returned collection.
/// Spec 009 Req 7: clone wraps any expression, not just identifiers.
#[test]
fn for_loop_clone_fn_call_expression() {
    let src = r#"
fn get_items() -> List[Int] { [1, 2, 3] }
fn process() -> Unit ! Console {
    for x in get_items() {
        println(x);
    }
}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "(get_items()).clone()");
}

/// For-loop over a field access iterable clones it — Spec 009 Req 7.
/// The most common real-world case: iterating a struct field collection.
#[test]
fn for_loop_clone_field_access_expression() {
    let src = r#"
type Container = struct { items: List[Int] }
fn process(c: Container) -> Unit ! Console {
    for x in c.items {
        println(x);
    }
}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "(c.items).clone()");
}

// ── Spec 009 Req 2: Clone-on-pass ─────────────────────────────────────────

/// Phase B (borrow inference): read-only struct params (`val T`) are emitted as `&T` in Rust.
/// Call sites emit &p (borrow) rather than move or clone.
/// Spec 009 Req 2.
#[test]
fn struct_ident_inferred_borrow_at_call_site() {
    let src = r#"
type Point = struct { x: Int, y: Int }
fn show(p: Point) -> Int { p.x }
fn f(p: Point) -> Int { show(p) }
"#;
    let rust = transpile_src(src);
    // show's p only uses p.x (field access) → inferred as &Point.
    // f's p is passed to show → disqualified from inference → owned.
    // Call site emits &p (borrow) not a move or clone.
    assert_contains(&rust, "show(&p)");
    assert!(
        !rust.contains("show(p.clone())"),
        "inferred-borrow param must not produce clone at call site"
    );
}

/// Phase B: borrow inference eliminates clones even for multi-use params.
/// Spec 009 Req 2, Scenario "Struct passed to two functions".
#[test]
fn struct_ident_borrow_inference_eliminates_clones() {
    let src = r#"
type Point = struct { x: Int, y: Int }
fn show(p: Point) -> Int { p.x }
fn g(p: Point) -> Int { show(p) }
fn f(p: Point) -> Int { g(p) }
fn double_show(p: Point) -> Int {
    let a: Int = show(p);
    let b: Int = show(p);
    a
}
"#;
    let rust = transpile_src(src);
    // show inferred as &Point → call sites use &p, no clones needed.
    assert_contains(&rust, "show(&p)");
    assert!(
        !rust.contains("show(p.clone())"),
        "borrow inference must eliminate clones for read-only params"
    );
}

/// Regression #465: borrow-inferred ident param inside a struct-literal field value.
/// Previously a fresh RustEmitter with empty borrow_params_map was used for struct
/// fields, so nested FnCalls always fell back to .clone().
#[test]
fn borrow_inferred_param_in_struct_literal_field() {
    let src = r#"
type Point = struct { x: Int, y: Int }
type Pair = struct { a: Int, b: Int }
fn get_x(p: Point) -> Int { p.x }
fn make_pair(p: Point) -> Pair {
    Pair { a: get_x(p), b: 0 }
}
"#;
    let rust = transpile_src(src);
    // get_x only accesses p.x → inferred as &Point.
    // The call inside the struct literal must emit &p, not p.clone().
    assert_contains(&rust, "get_x(&p)");
    assert!(
        !rust.contains("get_x(p.clone())"),
        "borrow-inferred param inside struct literal field must use &x, not .clone() (#465)"
    );
}

/// Regression #465: borrow-inferred field-access param inside a struct-literal field value.
#[test]
fn borrow_inferred_field_access_in_struct_literal_field() {
    let src = r#"
type Inner = struct { n: Int }
type Outer = struct { inner: Inner }
type Wrap = struct { out: Int }
fn use_inner(i: Inner) -> Int { i.n }
fn make_wrap(o: Outer) -> Wrap {
    Wrap { out: use_inner(o.inner) }
}
"#;
    let rust = transpile_src(src);
    // use_inner reads i.n only → inferred as &Inner.
    // Field-access arg inside struct literal must emit &o.inner, not o.inner.clone().
    assert_contains(&rust, "use_inner(&o.inner)");
    assert!(
        !rust.contains("use_inner(o.inner.clone())"),
        "borrow-inferred field access inside struct literal field must use &x (#465)"
    );
}

/// Phase 1: Int idents are also cloned at call sites. Redundant for Copy types
/// but harmless — LLVM removes it. The transpiler has no type info at emit time.
/// Spec 009 Req 2 "Copy types not cloned" is a Phase 3 goal; this documents
/// current Phase 1 behaviour.
#[test]
fn copy_type_ident_clone_is_emitted_but_harmless() {
    let src = r#"
fn add(a: Int, b: Int) -> Int { a + b }
fn f(x: Int) -> Int { add(x, x) }
"#;
    let rust = transpile_src(src);
    // Phase 1: clones are emitted; redundant for Copy types but correct.
    assert_contains(&rust, "x.clone()");
}

// ── #219: Iterator trait transpilation (001-type-system Req 11) ───────────────

/// Spec 001 Req 11 / Scenario: `impl Iterator[T] for X` emits Rust iterator impl.
///
/// GIVEN `impl Iterator[Int] for Counter { fn next(…) -> Option[Int] { … } }`
/// THEN  transpiler emits `impl std::iter::Iterator for Counter { type Item = i64; … }`
#[test]
fn iterator_impl_emits_rust_iterator() {
    let src = r#"
type Counter = struct { mut current: Int, limit: Int }

impl Iterator[Int] for Counter {
    fn next(mut self: Counter) -> Option[Int] {
        None
    }
}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "impl std::iter::Iterator for Counter {");
    assert_contains(&rust, "type Item = i64;");
    assert_contains(&rust, "fn next(&mut self) -> Option<i64> {");
}

/// `impl Iterator[T] for X` with no `next` method emits a todo!().
#[test]
fn iterator_impl_without_next_emits_todo() {
    let src = r#"
type Counter = struct { current: Int }
impl Iterator[Int] for Counter {}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "impl std::iter::Iterator for Counter {");
    assert_contains(&rust, "todo!(\"Iterator::next not implemented\")");
}

// ── #55: args.parse[T]() — struct-derived CLI parsing ─────────────────────

/// Concrete struct with stdlib import emits `impl ParseFromArgs`.
#[test]
fn struct_with_args_import_emits_parse_from_args_impl() {
    let src = r#"
use std.args.{parse}
type AppArgs = struct {
    host: String,
    port: Int,
    verbose: Bool,
}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "impl ParseFromArgs for AppArgs {");
    assert_contains(&rust, "fn parse_from_args() -> Result<Self, String> {");
}

/// `String` field emits required-flag parsing with `get_arg`.
#[test]
fn string_field_emits_required_get_arg() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { host: String }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "impl ParseFromArgs for Cfg {");
    assert_contains(&rust, "get_arg(Clean(\"host\".to_string()))");
    assert_contains(
        &rust,
        ".ok_or_else(|| \"missing required argument: --host\"",
    );
}

/// `Int` field emits integer parsing with `parse::<i64>()`.
#[test]
fn int_field_emits_integer_parsing() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { port: Int }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "get_arg(Clean(\"port\".to_string()))");
    assert_contains(&rust, ".parse::<i64>()");
}

/// `Bool` field emits flag-presence check via `std::env::args().any(…)`.
#[test]
fn bool_field_emits_flag_presence_check() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { verbose: Bool }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "std::env::args().any(|__a| __a == \"--verbose\")");
}

/// `Option[String]` field emits optional `get_arg` with `.map(…)`.
#[test]
fn option_string_field_emits_optional_parse() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { config: Option[String] }
"#;
    let rust = transpile_src(src);
    assert_contains(
        &rust,
        "get_arg(Clean(\"config\".to_string())).map(|__v| __v.0)",
    );
}

/// `Option[Int]` field emits optional integer parsing with error propagation.
#[test]
fn option_int_field_emits_optional_int_parse() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { count: Option[Int] }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "get_arg(Clean(\"count\".to_string()))");
    assert_contains(&rust, "parse::<i64>()");
}

/// Refined `Int` field emits integer parsing + runtime refinement check.
#[test]
fn refined_int_field_emits_parse_and_refinement_check() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { port: Int where self > 0 && self <= 65535 }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, ".parse::<i64>()");
    // Runtime refinement check — returns Err, not debug_assert; includes field value
    assert_contains(
        &rust,
        "return Err(format!(\"--port: refinement violated: {}\", port));",
    );
}

/// Struct with generic params does NOT get a `ParseFromArgs` impl.
#[test]
fn generic_struct_does_not_emit_parse_from_args() {
    let src = r#"
use std.args.{parse}
type Pair[A, B] = struct { first: A, second: B }
"#;
    let rust = transpile_src(src);
    assert!(
        !rust.contains("impl ParseFromArgs for Pair"),
        "generic structs must not get ParseFromArgs"
    );
}

/// Struct with unsupported field type does NOT get a `ParseFromArgs` impl.
#[test]
fn struct_with_unsupported_field_type_omits_parse_from_args() {
    let src = r#"
use std.args.{parse}
type Nested = struct { inner: Point }
type Point = struct { x: Int, y: Int }
"#;
    let rust = transpile_src(src);
    // Point gets the impl (Int fields), Nested does not (Point field is unsupported)
    assert_contains(&rust, "impl ParseFromArgs for Point {");
    assert!(
        !rust.contains("impl ParseFromArgs for Nested"),
        "struct with unsupported field must not get ParseFromArgs"
    );
}

/// Without stdlib imports, structs do NOT get `ParseFromArgs` impls.
#[test]
fn struct_without_stdlib_import_omits_parse_from_args() {
    let src = "type Point = struct { x: Int, y: Int }";
    let rust = transpile_src(src);
    assert!(
        !rust.contains("ParseFromArgs"),
        "no ParseFromArgs without mvl_runtime"
    );
}

/// `Float` field emits float parsing with `parse::<f64>()`.
#[test]
fn float_field_emits_float_parsing() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { scale: Float }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "get_arg(Clean(\"scale\".to_string()))");
    assert_contains(&rust, ".parse::<f64>()");
    assert_contains(&rust, "missing required argument: --scale");
}

/// `Option[Float]` field emits optional float parsing.
#[test]
fn option_float_field_emits_optional_float_parse() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { ratio: Option[Float] }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "get_arg(Clean(\"ratio\".to_string()))");
    assert_contains(&rust, "parse::<f64>()");
}

/// Struct with `Option[Bool]` field does NOT get a `ParseFromArgs` impl.
///
/// `Option[Bool]` is excluded because a bare `Bool` already encodes presence;
/// `Option[Bool]` has no meaningful CLI representation.
#[test]
fn struct_with_option_bool_field_omits_parse_from_args() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { verbose: Option[Bool] }
"#;
    let rust = transpile_src(src);
    assert!(
        !rust.contains("impl ParseFromArgs for Cfg"),
        "Option[Bool] is not parseable; ParseFromArgs impl must be omitted"
    );
}

/// Multi-field struct emits `Ok(Self { ... })` with all field names.
#[test]
fn multi_field_struct_emits_ok_self_construction() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { host: String, port: Int, verbose: Bool }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "Ok(Self { host, port, verbose })");
}

/// Bool field with underscore emits the exact flag name (no kebab-case conversion).
#[test]
fn bool_field_with_underscore_emits_exact_flag_name() {
    let src = r#"
use std.args.{parse}
type Cfg = struct { dry_run: Bool }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "std::env::args().any(|__a| __a == \"--dry_run\")");
}

/// Corpus: `tests/corpus/01_basics/args.mvl` transpiles without errors and
/// emits `ParseFromArgs` impls for all four structs in the file.
#[test]
fn corpus_args_transpiles() {
    let src = include_str!("corpus/01_basics/args.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "impl ParseFromArgs for AppArgs {");
    assert_contains(&rust, "impl ParseFromArgs for ServerConfig {");
    assert_contains(&rust, "impl ParseFromArgs for Flags {");
    assert_contains(&rust, "impl ParseFromArgs for OptArgs {");
    // Bool fields → presence flags
    assert_contains(&rust, "std::env::args().any(|__a| __a == \"--verbose\")");
    // Option[Float] → f64 parse
    assert_contains(&rust, "parse::<f64>()");
}

/// Corpus: `tests/corpus/01_basics/bitwise.mvl` transpiles without errors and
/// emits Rust bitwise operators for Int and Byte methods (#233).
#[test]
fn corpus_bitwise_transpiles() {
    let src = include_str!("corpus/01_basics/bitwise.mvl");
    let rust = transpile_src(src);
    // Int bitwise — Rust operators
    assert_contains(&rust, "(a & b)");
    assert_contains(&rust, "(a | b)");
    assert_contains(&rust, "(a ^ b)");
    assert_contains(&rust, "(!a)");
    assert_contains(&rust, ".wrapping_shl(");
    assert_contains(&rust, ".wrapping_shr(");
    // Byte to_int — cast to i64
    assert_contains(&rust, " as i64)");
    // from_int — cast to u8
    assert_contains(&rust, " as u8)");
    // Byte functions use u8 types
    assert_contains(&rust, "pub fn byte_bit_and(a: u8, b: u8) -> u8");
}

// ── Prelude emission (issue #229, Phase 4) ────────────────────────────────

use mvl::mvl::transpiler::transpile_project;

fn parse_prog(src: &str) -> mvl::mvl::parser::ast::Program {
    let (mut p, lex_errs) = Parser::new(src);
    assert!(lex_errs.is_empty(), "lex errors: {lex_errs:?}");
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    prog
}

/// A prelude function with a real body is emitted before user declarations.
#[test]
fn prelude_fn_with_body_is_emitted() {
    let prelude_src = r#"
pub partial fn greet(n: Int) -> String {
    let x: Int = n;
    x.to_string()
}
"#;
    let user_src = "fn main() -> Unit { }";
    let prelude = vec![parse_prog(prelude_src)];
    let user_prog = parse_prog(user_src);

    let out = transpile_project("crate", &user_prog, &[], &prelude);
    assert!(
        out.main_rs.contains("// ── stdlib prelude"),
        "prelude section header must appear:\n{}",
        out.main_rs
    );
    assert!(
        out.main_rs.contains("fn greet("),
        "greet fn must be emitted from prelude:\n{}",
        out.main_rs
    );
}

/// A prelude stub (empty body) is NOT emitted.
#[test]
fn prelude_stub_with_empty_body_is_skipped() {
    let prelude_src = "pub fn stub_fn(x: Int) -> Int { }";
    let user_src = "fn f() -> Unit { }";
    let prelude = vec![parse_prog(prelude_src)];
    let user_prog = parse_prog(user_src);

    let out = transpile_project("crate", &user_prog, &[], &prelude);
    assert!(
        !out.main_rs.contains("// ── stdlib prelude"),
        "stub-only prelude must not emit section header:\n{}",
        out.main_rs
    );
    assert!(
        !out.main_rs.contains("fn stub_fn("),
        "stub fn must not appear in output:\n{}",
        out.main_rs
    );
}

/// MACRO_HANDLED names (println, print, eprintln, format) are excluded even
/// when they have non-empty bodies.
#[test]
fn macro_handled_names_are_excluded_from_prelude() {
    let prelude_src = r#"pub fn println(value: String) -> Unit { let _x: String = value; }"#;
    let user_src = "fn f() -> Unit { }";
    let prelude = vec![parse_prog(prelude_src)];
    let user_prog = parse_prog(user_src);

    let out = transpile_project("crate", &user_prog, &[], &prelude);
    assert!(
        !out.main_rs.contains("fn println("),
        "macro-handled fn must not appear as a regular function:\n{}",
        out.main_rs
    );
}

/// Empty prelude_progs slice produces no prelude section.
#[test]
fn empty_prelude_progs_emits_no_prelude_section() {
    let user_src = "fn f() -> Unit { }";
    let user_prog = parse_prog(user_src);

    let out = transpile_project("crate", &user_prog, &[], &[]);
    assert!(
        !out.main_rs.contains("// ── stdlib prelude"),
        "no prelude section must appear when prelude_progs is empty:\n{}",
        out.main_rs
    );
}

/// User-defined function shadows the prelude — no duplicate definitions.
#[test]
fn user_fn_shadows_prelude_fn_no_duplicate() {
    let prelude_src = r#"
pub partial fn my_fn(x: Int) -> Int {
    let a: Int = x;
    a
}
"#;
    let user_src = r#"
pub fn my_fn(x: Int) -> Int { x }
fn main() -> Unit { }
"#;
    let prelude = vec![parse_prog(prelude_src)];
    let user_prog = parse_prog(user_src);

    let out = transpile_project("crate", &user_prog, &[], &prelude);
    let count = out.main_rs.matches("fn my_fn(").count();
    assert_eq!(
        count, 1,
        "exactly one my_fn definition must appear (user shadows prelude):\n{}",
        out.main_rs
    );
}

#[test]
fn string_concat_method_emits_clone_plus_borrow() {
    // GIVEN: s.concat(other) in MVL
    // WHEN: transpiled
    // THEN: emits UFCS free-function call `concat(a.clone().into(), ...)`
    let src = r#"fn f(a: String, b: String) -> String { a.concat(b) }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "concat(");
    assert_contains(&rust, ".clone().into()");
}

/// range() call is NOT expanded inline — it emits as a plain function call.
/// Regression: before #229 the transpiler emitted (start..end).collect::<Vec<i64>>()
#[test]
fn range_call_emits_as_plain_fn_call_not_inline_rust_range() {
    let src = "fn f() -> Unit { let xs: List[Int] = range(0, 5); }";
    let rust = transpile_src(src);
    assert!(
        !rust.contains("collect::<Vec<i64>>()"),
        "range must not be expanded inline as a Rust iterator:\n{rust}"
    );
    assert_contains(&rust, "range(");
}

// ── emit_exprs coverage: method calls ────────────────────────────────────────

#[test]
fn method_take_and_skip_emit_iterator_adapters() {
    let src = "fn f(xs: List[Int]) -> List[Int] { xs.take(3) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "take(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_take_while_emits_closure_clone() {
    let src = "fn f(xs: List[Int], p: fn(Int) -> Bool) -> List[Int] { xs.take_while(p) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "take_while(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_skip_while_emits_closure_clone() {
    let src = "fn f(xs: List[Int], p: fn(Int) -> Bool) -> List[Int] { xs.skip_while(p) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "skip_while(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_windows_emits_map_to_vec() {
    let src = "fn f(xs: List[Int]) -> List[List[Int]] { xs.windows(2) }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".windows(");
    assert_contains(&rust, ".map(|w| w.to_vec()).collect::<Vec<_>>()");
}

#[test]
fn method_chunks_emits_map_to_vec() {
    let src = "fn f(xs: List[Int]) -> List[List[Int]] { xs.chunks(3) }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".chunks(");
    assert_contains(&rust, ".map(|w| w.to_vec()).collect::<Vec<_>>()");
}

#[test]
fn method_flatten_emits_iterator_flatten() {
    let src = "fn f(xs: List[List[Int]]) -> List[Int] { xs.flatten() }";
    let rust = transpile_src(src);
    assert_contains(&rust, "flatten(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_partition_emits_turbofish() {
    let src =
        "fn f(xs: List[Int], p: fn(Int) -> Bool) -> List[Int] { let (a, b): (List[Int], List[Int]) = xs.partition(p); a }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".into_iter().partition::<Vec<_>, _>(|__x|");
}

#[test]
fn method_group_by_emits_hashmap_fold() {
    let src = "fn f(xs: List[Int], k: fn(Int) -> Int) -> Unit { let m: Map[Int, List[Int]] = xs.group_by(k); }";
    let rust = transpile_src(src);
    assert_contains(&rust, "std::collections::HashMap");
    assert_contains(&rust, "__m.entry(");
}

#[test]
fn method_chars_emits_char_to_string() {
    let src = r#"fn f(s: String) -> List[String] { s.chars() }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "chars(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_first_last_emit_cloned() {
    let src = "fn f(xs: List[Int]) -> Option[Int] { xs.first() }";
    let rust = transpile_src(src);
    assert_contains(&rust, "first(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_contains_emits_mvl_contains() {
    // List[T].contains(x) — emits via MvlContains trait, not hardcoded Rust
    let src = "fn f(xs: List[Int], n: Int) -> Bool { xs.contains(n) }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".mvl_contains(&(n");
    assert_contains(&rust, "))");
}

#[test]
fn method_contains_string_emits_mvl_contains() {
    // String.contains(sub) — same MvlContains trait dispatch
    let src = r#"fn f(s: String, sub: String) -> Bool { s.contains(sub) }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, ".mvl_contains(&(sub");
    assert_contains(&rust, "))");
}

#[test]
fn method_contains_set_emits_mvl_contains() {
    // Set[T].contains(x) — routes to HashSet MvlContains impl
    let src = "fn f(ss: Set[Int], n: Int) -> Bool { ss.contains(n) }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".mvl_contains(&(");
}

#[test]
fn mvl_contains_trait_is_emitted_in_preamble() {
    // The preamble must define MvlContains so generated code can call .mvl_contains()
    let src = "fn f(xs: List[Int], n: Int) -> Bool { xs.contains(n) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "pub trait MvlContains<T:");
    assert_contains(&rust, "impl<T: PartialEq> MvlContains<T> for Vec<T>");
    assert_contains(&rust, "impl MvlContains<String> for String");
    assert_contains(&rust, "impl MvlContains<str> for String");
    assert_contains(
        &rust,
        "impl<T: Eq + std::hash::Hash> MvlContains<T> for std::collections::HashSet<T>",
    );
}

#[test]
fn method_contains_wrong_arity_falls_through_to_generic() {
    // contains() with 0 args fails the arity guard and falls to the generic arm
    let src = "fn f(xs: List[Int]) -> Unit { xs.contains() }";
    let rust = transpile_src(src);
    // Must not emit the MvlContains path
    assert!(
        !rust.contains(".mvl_contains("),
        "zero-arg contains must not use mvl_contains"
    );
}

#[test]
fn method_slice_emits_safe_wrapper() {
    let src = "fn f(xs: List[Int]) -> List[Int] { xs.slice(1, 3) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "slice(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_substring_emits_safe_wrapper() {
    let src = r#"fn f(s: String) -> String { s.substring(1, 4) }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "substring(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_clamp_emits_safe_wrapper() {
    let src = "fn f(n: Int) -> Int { n.clamp(0, 100) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "_mvl_n");
    assert_contains(&rust, "_mvl_lo");
    assert_contains(&rust, "_mvl_hi");
}

#[test]
fn method_bit_and_emits_operator() {
    let src = "fn f(a: Int, b: Int) -> Int { a.bit_and(b) }";
    let rust = transpile_src(src);
    assert_contains(&rust, " & ");
}

#[test]
fn method_bit_or_emits_operator() {
    let src = "fn f(a: Int, b: Int) -> Int { a.bit_or(b) }";
    let rust = transpile_src(src);
    assert_contains(&rust, " | ");
}

#[test]
fn method_bit_xor_emits_operator() {
    let src = "fn f(a: Int, b: Int) -> Int { a.bit_xor(b) }";
    let rust = transpile_src(src);
    assert_contains(&rust, " ^ ");
}

#[test]
fn method_bit_not_emits_prefix_bang() {
    let src = "fn f(a: Int) -> Int { a.bit_not() }";
    let rust = transpile_src(src);
    assert_contains(&rust, "(!");
}

#[test]
fn method_shift_left_emits_wrapping_shl() {
    let src = "fn f(a: Int, n: Int) -> Int { a.shift_left(n) }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".wrapping_shl(");
    assert_contains(&rust, " as u32)");
}

#[test]
fn method_shift_right_emits_wrapping_shr() {
    let src = "fn f(a: Int, n: Int) -> Int { a.shift_right(n) }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".wrapping_shr(");
}

#[test]
fn method_to_int_emits_cast() {
    let src = "fn f(b: Byte) -> Int { b.to_int() }";
    let rust = transpile_src(src);
    assert_contains(&rust, " as i64)");
}

#[test]
fn method_map_emits_mvl_map() {
    let src = "fn f(xs: List[Int], g: fn(Int) -> Int) -> List[Int] { xs.map(g) }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".mvl_map(|__x|");
    assert_contains(&rust, "__x.clone()");
}

#[test]
fn method_filter_emits_iterator_filter() {
    let src = "fn f(xs: List[Int], p: fn(Int) -> Bool) -> List[Int] { xs.filter(p) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "filter(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_fold_emits_iterator_fold() {
    let src = "fn f(xs: List[Int], init: Int, g: fn(Int, Int) -> Int) -> Int { xs.fold(init, g) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "fold(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_any_emits_ufcs_call() {
    let src = "fn f(xs: List[Int], p: fn(Int) -> Bool) -> Bool { xs.any(p) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "any(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_all_emits_ufcs_call() {
    let src = "fn f(xs: List[Int], p: fn(Int) -> Bool) -> Bool { xs.all(p) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "all(");
    assert_contains(&rust, ".clone().into()");
}

/// Verifies that `fn(T) -> U` typed parameters emit as `impl Fn(T) -> U` so
/// that closures are accepted at HOF call sites (not just bare fn pointers).
#[test]
fn fn_type_param_emits_impl_fn_not_bare_fn() {
    let src = "fn apply(xs: List[Int], p: fn(Int) -> Bool) -> List[Int] { xs.filter(p) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "impl Fn(");
    assert!(
        !rust.contains("fn(i64) -> bool"),
        "bare fn pointer must not appear as a parameter type; got:\n{rust}"
    );
}

#[test]
fn method_reverse_emits_rev_collect() {
    let src = "fn f(xs: List[Int]) -> List[Int] { xs.reverse() }";
    let rust = transpile_src(src);
    assert_contains(&rust, "reverse(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_sort_emits_sort_by_partial_cmp() {
    let src = "fn f(xs: List[Int]) -> List[Int] { xs.sort() }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".sort_by(|__a,__b|");
    assert_contains(&rust, "partial_cmp");
}

#[test]
fn method_and_then_emits_closure() {
    let src = "fn f(x: Option[Int], g: fn(Int) -> Option[Int]) -> Option[Int] { x.and_then(g) }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".and_then(|__x|");
}

#[test]
fn method_trim_emits_to_string() {
    let src = r#"fn f(s: String) -> String { s.trim() }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "trim(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_to_upper_emits_to_uppercase() {
    let src = r#"fn f(s: String) -> String { s.to_upper() }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "to_upper(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_to_lower_emits_to_lowercase() {
    let src = r#"fn f(s: String) -> String { s.to_lower() }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "to_lower(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_starts_with_borrows_arg() {
    let src = r#"fn f(s: String, p: String) -> Bool { s.starts_with(p) }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "starts_with(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_ends_with_borrows_arg() {
    let src = r#"fn f(s: String, p: String) -> Bool { s.ends_with(p) }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "ends_with(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_find_emits_cast_to_i64() {
    let src = r#"fn f(s: String, p: String) -> Option[Int] { s.find(p) }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "find(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_replace_emits_deref_on_second_arg() {
    let src = r#"fn f(s: String) -> String { s.replace("a", "b") }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "replace(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_split_emits_map_to_string() {
    let src = r#"fn f(s: String) -> List[String] { s.split(",") }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "split(");
    assert_contains(&rust, ".clone().into()");
}

#[test]
fn method_is_zero_emits_eq_zero() {
    let src = "fn f(n: Int) -> Bool { n.is_zero() }";
    let rust = transpile_src(src);
    assert_contains(&rust, " == 0)");
}

#[test]
fn method_pow_emits_mvl_pow() {
    let src = "fn f(n: Int, e: Int) -> Int { n.pow(e) }";
    let rust = transpile_src(src);
    assert_contains(&rust, ".mvl_pow(");
}

// ── emit_exprs coverage: escape_char ─────────────────────────────────────────

#[test]
fn char_literal_special_chars_are_escaped() {
    let src = r#"fn f() -> Char { '\n' }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "'\\n'");
}

#[test]
fn char_literal_tab_is_escaped() {
    let src = "fn f() -> Char { '\t' }";
    let rust = transpile_src(src);
    assert_contains(&rust, "'\\t'");
}

#[test]
fn char_literal_backslash_is_escaped() {
    let src = "fn f() -> Char { '\\\\' }";
    let rust = transpile_src(src);
    // A single backslash char in MVL emits as '\\' in Rust (escaped backslash char literal).
    assert_contains(&rust, "'\\\\'");
}

// ── emit_exprs coverage: expr types ──────────────────────────────────────────

#[test]
fn move_expr_emits_inner() {
    let src = "fn f(x: Int) -> Int { move(x) }";
    let rust = transpile_src(src);
    // move(x) strips the wrapper — the inner expr is emitted directly.
    assert!(
        !rust.contains("move("),
        "move wrapper should not appear in output: {rust}"
    );
    assert_contains(&rust, "x");
}

#[test]
fn consume_expr_emits_inner() {
    let src = "fn f(x: Int) -> Int { consume(x) }";
    let rust = transpile_src(src);
    // consume(x) strips the wrapper — the inner expr is emitted directly.
    assert!(
        !rust.contains("consume("),
        "consume wrapper should not appear in output: {rust}"
    );
    assert_contains(&rust, "x");
}

#[test]
fn lambda_expr_emits_closure() {
    let src = "fn f() -> fn(Int) -> Int { |x: Int| x }";
    let rust = transpile_src(src);
    assert_contains(&rust, "|x: i64|");
}

#[test]
fn lambda_with_return_type_emits_arrow() {
    let src = "fn f() -> fn(Int) -> Int { |x: Int| -> Int x }";
    let rust = transpile_src(src);
    assert_contains(&rust, " -> i64 ");
}

#[test]
fn from_int_call_emits_as_u8() {
    let src = "fn f(n: Int) -> Byte { from_int(n) }";
    let rust = transpile_src(src);
    assert_contains(&rust, " as u8)");
}

#[test]
fn emit_args_for_macro_non_literal_first_arg_generates_placeholder() {
    // When println's first arg is not a string literal, a "{}" placeholder
    // must be generated for each argument.
    let src = r#"fn f(msg: String) -> Unit { println(msg) }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "println!(");
    assert_contains(&rust, "\"{}\"");
}

#[test]
fn propagate_expr_emits_question_mark() {
    let src = r#"fn f(r: Result[Int, String]) -> Result[Int, String] {
    let x: Int = r?;
    Ok(x)
}"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "?");
}

#[test]
fn type_args_in_fn_call_emit_turbofish() {
    // Functions with explicit type arguments use bracket syntax: name[Type](args)
    let src = "fn identity[T](x: T) -> T { x }\nfn f() -> Int { identity[Int](42) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "::<");
}

// ── transpiler/mod.rs: uncovered entry points ─────────────────────────────────

use mvl::mvl::transpiler::{
    has_main_fn, has_std_imports, transpile_covered, transpile_covered_source,
    transpile_covered_source_with_prelude, transpile_covered_with_prelude,
    transpile_mutated_source_with_prelude, transpile_mutated_with_prelude,
    transpile_source_with_prelude, transpile_with_prelude,
};

#[test]
fn has_main_fn_true_when_main_present() {
    let prog = parse_prog("fn main() -> Unit { }");
    assert!(has_main_fn(&prog));
}

#[test]
fn has_main_fn_false_when_no_main() {
    let prog = parse_prog("fn foo() -> Int { 1 }");
    assert!(!has_main_fn(&prog));
}

#[test]
fn has_std_imports_true_for_std_use() {
    let prog = parse_prog("use std.io.{read_file}");
    assert!(has_std_imports(&prog));
}

#[test]
fn has_std_imports_false_for_non_std_use() {
    let prog = parse_prog("use mylib.utils.{helper}");
    assert!(!has_std_imports(&prog));
}

#[test]
fn transpile_with_prelude_produces_valid_output() {
    let prog = parse_prog("fn f(x: Int) -> Int { x }");
    let out = transpile_with_prelude(&prog, "my_crate", &[]);
    assert_contains(&out.lib_rs, "fn f(");
    assert!(!out.has_main);
    assert_contains(&out.cargo_toml, "my_crate");
}

#[test]
fn transpile_with_prelude_has_main_sets_binary() {
    let prog = parse_prog("fn main() -> Unit { }");
    let out = transpile_with_prelude(&prog, "my_app", &[]);
    assert!(out.has_main);
    assert!(
        !out.cargo_toml.contains("[lib]"),
        "binary crate should not have [lib] section"
    );
}

#[test]
fn transpile_source_with_prelude_produces_valid_output() {
    let prog = parse_prog("fn f(x: Int) -> Int { x }");
    let out = transpile_source_with_prelude(&prog, "my_crate", &[]);
    assert_contains(&out.lib_rs, "fn f(");
    assert!(!out.has_main);
}

#[test]
fn transpile_covered_instruments_branches() {
    let src = "fn f(n: Int) -> Int { if n > 0 { 1 } else { 0 } }";
    let prog = parse_prog(src);
    let (out, branches) = transpile_covered(&prog, "my_crate", "f", 0);
    assert!(!branches.is_empty(), "expected branch coverage entries");
    assert_contains(&out.lib_rs, "fn f(");
}

#[test]
fn transpile_covered_with_prelude_instruments_branches() {
    let src = "fn f(n: Int) -> Int { if n > 0 { 1 } else { 0 } }";
    let prog = parse_prog(src);
    let (out, branches) = transpile_covered_with_prelude(&prog, "my_crate", "f", 0, &[]);
    assert!(!branches.is_empty(), "expected branch coverage entries");
    assert_contains(&out.lib_rs, "fn f(");
}

#[test]
fn transpile_covered_source_instruments_branches() {
    let src = "fn f(n: Int) -> Int { match n { 0 => 0, _ => 1 } }";
    let prog = parse_prog(src);
    let (out, branches) = transpile_covered_source(&prog, "my_crate", "f", 0);
    assert!(!branches.is_empty(), "expected branch coverage entries");
    assert_contains(&out.lib_rs, "fn f(");
}

#[test]
fn transpile_covered_source_with_prelude_instruments_branches() {
    let src = "fn f(n: Int) -> Int { match n { 0 => 0, _ => 1 } }";
    let prog = parse_prog(src);
    let (out, branches) = transpile_covered_source_with_prelude(&prog, "my_crate", "f", 0, &[]);
    assert!(!branches.is_empty());
    assert_contains(&out.lib_rs, "fn f(");
}

#[test]
fn transpile_mutated_with_prelude_mutates_non_test_fn_bodies() {
    // Regular fn bodies in _test.mvl files are re-declarations of source functions
    // (workaround for #96, no cross-module imports yet).  cmd_mutate skips the source
    // file when a _test.mvl covers it, so mutations must come from the test file's
    // non-test fn re-declarations.
    let src = "fn f(a: Int, b: Int) -> Int { a + b }";
    let prog = parse_prog(src);
    let (out, mutants) = transpile_mutated_with_prelude(&prog, "my_crate", "f", &[]);
    assert!(
        !mutants.is_empty(),
        "non-test fn bodies in test files must produce mutants"
    );
    assert_contains(&out.lib_rs, "fn f(");
}

#[test]
fn transpile_mutated_source_with_prelude_produces_mutants() {
    let src = "fn f(a: Int, b: Int) -> Int { a + b }";
    let prog = parse_prog(src);
    let (out, mutants) = transpile_mutated_source_with_prelude(&prog, "my_crate", "f", &[]);
    assert!(!mutants.is_empty(), "expected mutation variants");
    assert_contains(&out.lib_rs, "fn f(");
}

#[test]
fn transpile_mutated_with_prelude_mutates_bool_literal_in_non_test_fn() {
    // alloc_bool_mutation must fire for non-test fn bodies in test files (they are
    // source function re-declarations that cmd_mutate relies on for mutation points).
    let src = "fn flag() -> Bool { true }";
    let prog = parse_prog(src);
    let (_out, mutants) = transpile_mutated_with_prelude(&prog, "my_crate", "flag", &[]);
    assert!(
        !mutants.is_empty(),
        "bool literal in non-test fn of test file must produce mutants"
    );
}

#[test]
fn transpile_mutated_with_prelude_mutates_int_literal_in_non_test_fn() {
    // alloc_int_mutations must fire for non-test fn bodies in test files (they are
    // source function re-declarations that cmd_mutate relies on for mutation points).
    let src = "fn answer() -> Int { 42 }";
    let prog = parse_prog(src);
    let (_out, mutants) = transpile_mutated_with_prelude(&prog, "my_crate", "answer", &[]);
    assert!(
        !mutants.is_empty(),
        "int literal in non-test fn of test file must produce mutants"
    );
}

#[test]
fn transpile_mutated_with_prelude_mixed_file_non_test_fn_produces_mutants() {
    // In a _test.mvl file the non-test fn `f` is a re-declaration of the source
    // function and MUST produce mutants (cmd_mutate skips the source file when a
    // _test.mvl covers it, so the test file's non-test fns are the only mutation points).
    // The test fn body is suppressed via current_fn_is_test.
    // TODO: issue #330 also lists "mutate test fn bodies" as a requirement; that
    // half is deferred — when implemented, flip the assertion below.
    let src =
        "fn f(a: Int, b: Int) -> Int { a + b }\ntest fn check_f() -> Unit { let _x: Int = 1 + 2; }";
    let prog = parse_prog(src);
    let (_out, mutants) = transpile_mutated_with_prelude(&prog, "my_crate", "my_test_file", &[]);
    let non_test_mutants: Vec<_> = mutants.iter().filter(|m| m.fn_name == "f").collect();
    assert!(
        !non_test_mutants.is_empty(),
        "non-test fn `f` in test file must produce mutants"
    );
    let test_mutants: Vec<_> = mutants.iter().filter(|m| m.fn_name == "check_f").collect();
    assert!(
        test_mutants.is_empty(),
        "test fn `check_f` must not produce mutants (got {})",
        test_mutants.len()
    );
}

// ── Phase B: borrow params — call-site &x / &mut x emission (#304) ───────────

/// Shared ref param: call site emits `&y`, signature emits `&i64`.
#[test]
fn ref_param_fn_call_emits_ampersand() {
    let src = "fn f(x: val Int) -> Unit { }  fn g(y: Int) -> Unit { f(y) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "f(&y)");
    assert_contains(&rust, "x: &i64");
}

/// Mutable ref param: call site emits `&mut y`, signature emits `&mut i64`.
#[test]
fn mut_ref_param_fn_call_emits_ampersand_mut() {
    let src = "fn f(x: ref Int) -> Unit { }  fn g(y: Int) -> Unit { f(y) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "f(&mut y)");
    assert_contains(&rust, "x: &mut i64");
}

/// Mixed params: only the val-annotated argument gets `&`; owned stays as-is.
#[test]
fn mixed_params_selective_borrow_emission() {
    let src = "fn f(a: Int, b: val Int) -> Unit { }  fn g(x: Int, y: Int) -> Unit { f(x, y) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "&y");
    // First arg must NOT receive a & prefix
    assert!(
        !rust.contains("f(&x"),
        "owned first arg must not be prefixed with &: {rust}"
    );
}

/// Literal argument to val param: wrapped as `&(42)`.
#[test]
fn literal_arg_to_ref_param_wrapped_in_ampersand_paren() {
    let src = "fn f(x: val Int) -> Unit { }  fn g() -> Unit { f(42) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "&(42)");
}

/// Multiple call sites to the same val fn both emit `&`.
#[test]
fn multiple_call_sites_both_emit_ampersand() {
    let src = "fn f(x: val Int) -> Unit { }  fn g(a: Int, b: Int) -> Unit { f(a)  f(b) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "f(&a)");
    assert_contains(&rust, "f(&b)");
}

/// Expression-level borrow `val x` emits `&x` in Rust. (#366)
#[test]
fn borrow_expr_shared_emits_ampersand() {
    let src = "fn f(x: Int) -> Unit { let r: val Int = val x; }";
    let rust = transpile_src(src);
    assert_contains(&rust, "&x");
}

/// Expression-level mutable borrow `ref x` emits `&mut x` in Rust. (#366)
#[test]
fn borrow_expr_mutable_emits_ampersand_mut() {
    let src = "fn f(mut x: Int) -> Unit { let r: ref Int = ref x; }";
    let rust = transpile_src(src);
    assert_contains(&rust, "&mut x");
}

/// Fix 5: group_by with a declared `val String` key function emits `&__v.clone()`. (#366)
/// Phase B borrow inference maps declared functions with explicit val T params so
/// group_by correctly wraps the loop variable in `&__v.clone()`.
#[test]
fn group_by_with_ref_string_key_emits_borrow_on_clone() {
    let src = "fn key(s: val String) -> String { s }  fn f(xs: List[String]) -> Unit { let m: Map[String, List[String]] = xs.group_by(key); }";
    let rust = transpile_src(src);
    assert_contains(&rust, "&__v.clone()");
}

// ── Mutation regression: emit_binary_op operator table (#206) ─────────────
//
// Each test pins exactly one operator string so that a mutation of the table
// entry (e.g. "+" → "-") produces a test failure.  Without these tests every
// entry in `emit_binary_op` is a surviving mutant.

/// Addition operator emits `+`.
#[test]
fn binary_add_emits_plus() {
    let src = "fn f(a: Int, b: Int) -> Int { a + b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a + b");
}

/// Subtraction operator emits `-`.
#[test]
fn binary_sub_emits_minus() {
    let src = "fn f(a: Int, b: Int) -> Int { a - b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a - b");
}

/// Multiplication operator emits `*`.
#[test]
fn binary_mul_emits_star() {
    let src = "fn f(a: Int, b: Int) -> Int { a * b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a * b");
}

/// Division operator emits `/`.
#[test]
fn binary_div_emits_slash() {
    let src = "fn f(a: Int, b: Int) -> Int { a / b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a / b");
}

/// Remainder operator emits `%`.
#[test]
fn binary_rem_emits_percent() {
    let src = "fn f(a: Int, b: Int) -> Int { a % b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a % b");
}

/// Equality operator emits `==`.
#[test]
fn binary_eq_emits_double_eq() {
    let src = "fn f(a: Int, b: Int) -> Bool { a == b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a == b");
}

/// Inequality operator emits `!=`.
#[test]
fn binary_ne_emits_bang_eq() {
    let src = "fn f(a: Int, b: Int) -> Bool { a != b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a != b");
}

/// Less-than operator emits `<`.
#[test]
fn binary_lt_emits_less_than() {
    let src = "fn f(a: Int, b: Int) -> Bool { a < b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a < b");
}

/// Greater-than operator emits `>`.
#[test]
fn binary_gt_emits_greater_than() {
    let src = "fn f(a: Int, b: Int) -> Bool { a > b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a > b");
}

/// Less-or-equal operator emits `<=`.
#[test]
fn binary_le_emits_le() {
    let src = "fn f(a: Int, b: Int) -> Bool { a <= b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a <= b");
}

/// Greater-or-equal operator emits `>=`.
#[test]
fn binary_ge_emits_ge() {
    let src = "fn f(a: Int, b: Int) -> Bool { a >= b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a >= b");
}

/// Logical and emits `&&`.
#[test]
fn binary_and_emits_double_ampersand() {
    let src = "fn f(a: Bool, b: Bool) -> Bool { a && b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a && b");
}

/// Logical or emits `||`.
#[test]
fn binary_or_emits_double_pipe() {
    let src = "fn f(a: Bool, b: Bool) -> Bool { a || b }";
    let rust = transpile_src(src);
    assert_contains(&rust, "a || b");
}

// ── Mutation regression: emit_literal dispatch (#206) ────────────────────

/// `true` literal emits `true`, not `false`.
#[test]
fn bool_true_literal_emits_true() {
    let src = "fn f() -> Bool { true }";
    let rust = transpile_src(src);
    assert_contains(&rust, "true");
    assert!(
        !rust.contains("false"),
        "true literal must not emit false:\n{rust}"
    );
}

/// `false` literal emits `false`, not `true`.
#[test]
fn bool_false_literal_emits_false() {
    let src = "fn f() -> Bool { false }";
    let rust = transpile_src(src);
    assert_contains(&rust, "false");
    assert!(
        !rust.contains("true"),
        "false literal must not emit true:\n{rust}"
    );
}

/// Whole-number float literal always gets a `.0` suffix so it stays a float.
/// The guard `s.contains('.') || s.contains('e')` must remain an `||` not `&&`.
#[test]
fn whole_number_float_gets_decimal_suffix() {
    let src = "fn f() -> Float { 2.0 }";
    let rust = transpile_src(src);
    assert!(
        rust.contains("2.0") || rust.contains("2."),
        "whole-number float must have decimal suffix:\n{rust}"
    );
}

// ── Mutation regression: emit_stmt let-mutability (#206) ─────────────────

/// Immutable let-binding emits `let ` (not `let mut `).
#[test]
fn immutable_let_emits_let_not_let_mut() {
    let src = "fn f() -> Int { let x: Int = 1; x }";
    let rust = transpile_src(src);
    assert_contains(&rust, "let x:");
    assert!(
        !rust.contains("let mut x:"),
        "immutable binding must not emit `let mut`:\n{rust}"
    );
}

/// Mutable let-binding emits `let mut ` (not just `let `).
#[test]
fn mutable_let_emits_let_mut() {
    let src = "fn f() -> Int { let mut x: Int = 1; x = 2; x }";
    let rust = transpile_src(src);
    assert_contains(&rust, "let mut x:");
}

// ── Mutation regression: string match pattern (.as_str() coercion) (#206) ─

/// Match on a String value with string literal arms must coerce via `.as_str()`.
/// `arms_have_str_pattern` returning the wrong value would break this.
#[test]
fn match_string_scrutinee_with_literal_arm_emits_as_str() {
    let src = r#"fn f(s: String) -> Int { match s { "hello" => 1, _ => 0 } }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, ".as_str()");
}

/// Match on an Int scrutinee must NOT emit `.as_str()` after the matched variable.
/// Guards against `arms_have_str_pattern` returning `true` unconditionally.
#[test]
fn match_int_scrutinee_does_not_coerce_to_str() {
    let src = "fn f(n: Int) -> Int { match n { 0 => 1, _ => 0 } }";
    let rust = transpile_src(src);
    // The match expression must be `match n {`, not `match n.as_str() {`
    assert_contains(&rust, "match n {");
    assert!(
        !rust.contains("match n.as_str()"),
        "integer match must not coerce scrutinee to &str:\n{rust}"
    );
}

/// String literal in pattern position must be a bare `"s"`, not `"s".to_string()`.
/// `emit_literal_in_pattern` has a separate path from `emit_literal`.
#[test]
fn string_literal_in_match_arm_is_bare_not_to_string() {
    let src = r#"fn f(s: String) -> Int { match s { "hello" => 1, _ => 0 } }"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "\"hello\"");
    assert!(
        !rust.contains("\"hello\".to_string()"),
        "pattern string must not call .to_string():\n{rust}"
    );
}

// ── Mutation regression: else-if chaining (#206, related to #197) ─────────

/// `else if` chains must emit as inline `else if`, not as `else { if ... }`.
/// A mutation that skips the `ElseBranch::If` inline path would emit a nested block.
#[test]
fn else_if_emits_inline_not_nested_block() {
    let src = "fn f(n: Int) -> Int { if n > 0 { 1 } else if n < 0 { -1 } else { 0 } }";
    let rust = transpile_src(src);
    assert_contains(&rust, "} else if");
}

// ── Mutation regression: field access clone in arg position (#206) ────────

/// A field access passed as a function argument must emit `.clone()`.
/// `emit_expr_as_arg` handles `Expr::FieldAccess` via the conservative clone path.
#[test]
fn field_access_arg_emits_clone() {
    let src = "type P = struct { x: Int }  fn use_int(n: Int) -> Int { n }  fn f(p: P) -> Int { use_int(p.x) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "p.x.clone()");
}

/// Block used as a value (e.g., if expression) emits the block body correctly.
/// Guards against `emit_block_as_value` being replaced with a no-op.
#[test]
fn if_expr_as_value_emits_block_body() {
    let src = "fn f(b: Bool) -> Int { let x: Int = if b { 1 } else { 2 }; x }";
    let rust = transpile_src(src);
    assert_contains(&rust, "1");
    assert_contains(&rust, "2");
}

/// An identifier passed as a function argument emits `.clone()` when not the last use.
#[test]
fn ident_arg_non_last_use_emits_clone() {
    let src = "fn double(n: Int) -> Int { n }  fn f(x: Int) -> Int { let _a: Int = double(x); double(x) }";
    let rust = transpile_src(src);
    assert_contains(&rust, "double(x.clone())");
}

// ── Epic #480: Primitives and runtime architecture redesign ──────────────────

/// Bit operators corpus transpiles without errors (#483 #484).
#[test]
fn corpus_bit_operators_transpiles() {
    let src = include_str!("corpus/02_types/bit_operators.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "&");
    assert_contains(&rust, "|");
    assert_contains(&rust, "^");
    assert_contains(&rust, "<<");
    assert_contains(&rust, ">>");
}

/// Overflow-checking arithmetic corpus transpiles correctly (#485).
#[test]
fn corpus_overflow_checking_transpiles() {
    let src = include_str!("corpus/02_types/overflow_checking.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "checked_add");
    assert_contains(&rust, "wrapping_add");
}

/// Unsigned types corpus transpiles: UByte → u8, UInt → u64 (#481).
#[test]
fn corpus_unsigned_types_transpiles() {
    let src = include_str!("corpus/02_types/unsigned_types.mvl");
    let rust = transpile_src(src);
    assert_contains(&rust, "u8");
    assert_contains(&rust, "u64");
}

// ── Stdlib type stub suppression (#530) ──────────────────────────────────

/// Types declared in Rust-backed stdlib modules (e.g. Path from std.io) must
/// NOT be emitted as placeholder `pub struct` stubs — they are already defined
/// in mvl_runtime via the `use mvl_runtime::stdlib::io::*` wildcard import.
#[test]
fn stdlib_types_not_emitted_as_stubs() {
    let src = "use std.io.{open}\nfn f(p: Path) -> Unit { }";
    let rust = transpile_src(src);
    assert!(
        !rust.contains("pub struct Path;"),
        "Path is provided by mvl_runtime::stdlib::io — must not appear as a stub:\n{rust}"
    );
    assert!(
        !rust.contains("pub struct File;"),
        "File is provided by mvl_runtime::stdlib::io — must not appear as a stub:\n{rust}"
    );
}

/// Non-stdlib external types must still get placeholder stubs even when a
/// Rust-backed stdlib module is also imported (#530 regression guard).
#[test]
fn non_stdlib_types_still_get_stubs_alongside_stdlib_import() {
    let src = "use std.io.{open}\nfn f(p: Path, db: DbConn) -> Unit { }";
    let rust = transpile_src(src);
    assert!(
        rust.contains("pub struct DbConn;"),
        "DbConn is not in any stdlib — must still be emitted as a stub:\n{rust}"
    );
    assert!(
        !rust.contains("pub struct Path;"),
        "Path is provided by mvl_runtime::stdlib::io — must not appear as a stub:\n{rust}"
    );
}

/// Brace-import syntax (`use std.io.{File}`) must also trigger stub suppression
/// — the parser discards brace items, leaving path = ["std", "io"], so both
/// import forms should produce the same suppression behaviour.
#[test]
fn stdlib_stub_suppression_works_for_brace_import_syntax() {
    let src = "use std.io.{open, read_to_string}\nfn f(p: Path) -> Unit { }";
    let rust = transpile_src(src);
    assert!(
        !rust.contains("pub struct Path;"),
        "Path must not be stubbed when imported via brace syntax:\n{rust}"
    );
}

/// A `builtin fn` in the prelude is NOT emitted as Rust code — the runtime
/// provides the implementation.  Emitting it would produce dead code or a
/// `todo!()` stub that shadows the real runtime function.
#[test]
fn prelude_builtin_fn_is_not_emitted() {
    let prelude_src = "pub builtin fn len(s: String) -> Int";
    let user_src = "fn main() -> Unit { }";
    let prelude = vec![parse_prog(prelude_src)];
    let user_prog = parse_prog(user_src);

    let out = transpile_project("crate", &user_prog, &[], &prelude);
    assert!(
        !out.main_rs.contains("fn len("),
        "builtin fn must not be emitted into Rust output:\n{}",
        out.main_rs
    );
}

/// A `builtin fn` with a non-Unit return type must not produce a `todo!()` stub.
/// This would previously happen because an empty body + non-Unit return was
/// treated as a missing implementation.
#[test]
fn prelude_builtin_fn_does_not_produce_todo_stub() {
    let prelude_src = "pub builtin fn len(s: String) -> Int";
    let user_src = "fn main() -> Unit { }";
    let prelude = vec![parse_prog(prelude_src)];
    let user_prog = parse_prog(user_src);

    let out = transpile_project("crate", &user_prog, &[], &prelude);
    assert!(
        !out.main_rs.contains("todo!"),
        "builtin fn must not produce a todo! stub:\n{}",
        out.main_rs
    );
}
