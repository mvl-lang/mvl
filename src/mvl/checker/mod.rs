//! MVL type checker — verifies requirements 1, 3, 4, 5, 6, 10.
//!
//! The checker runs after parsing and before transpilation.  It reports
//! [`CheckError`] values for every violation found; unlike the parser, it
//! does not short-circuit on the first error.
//!
//! # Architecture
//!
//! ```text
//! Program
//!   └─ pass 1: collect_declarations  (populate type/function tables)
//!   └─ pass 2: check_declarations    (verify each decl)
//!              └─ check_fn_decl      (type-check function body)
//!                 └─ check_block / check_stmt / infer_expr
//! ```

mod borrows;
mod calls;
pub mod const_eval;
pub mod context;
pub mod data_race;
mod decls;
pub mod errors;
pub mod ifc;
mod infer;
mod method_types;
pub mod passes;
mod patterns;
pub mod refinements;
pub(crate) mod solver;
mod stmts;
pub mod termination;
pub mod types;

use crate::mvl::checker::context::TypeEnv;
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{Effect, Program, Totality};
use crate::mvl::parser::lexer::Span;
use std::collections::{HashMap, HashSet};

// ── Public API ───────────────────────────────────────────────────────────────

/// Result of running the type checker over a [`Program`].
#[derive(Debug, Default)]
pub struct CheckResult {
    pub errors: Vec<CheckError>,
    /// Number of `extern` blocks found — each is a trust boundary.
    /// Reported in the assurance summary: "N extern declarations".
    pub extern_count: usize,
    /// Error counts indexed by requirement number (1–11). Index 0 is unused.
    pub req_errors: [usize; 12],
    /// Inferred type for every expression in the program, keyed by span.
    /// Used by the transpiler to emit type-specific Rust code (#554).
    pub expr_types: HashMap<Span, Ty>,
}

impl CheckResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Entry point: type-check a parsed [`Program`].
/// Check a program with additional prelude programs whose declarations are
/// registered (but not checked) before the user program is type-checked.
/// Use this when stdlib files have been parsed and should be visible to the
/// checker (e.g. `use std.io.{...}` imports in corpus / CLI check mode).
pub fn check_with_prelude(prelude: &[Program], prog: &Program) -> CheckResult {
    let mut checker = TypeChecker::new();
    for p in prelude {
        checker.collect_declarations(&p.declarations);
    }
    checker.check_program(prog);
    termination::check_structural_recursion(prog, &mut checker.errors);
    data_race::check_iso_aliasing(prog, &mut checker.errors);
    ifc::check_implicit_flows(prog, &mut checker.errors);
    refinements::check_refinements(prog, &mut checker.errors);
    let mut req_errors = [0usize; 12];
    for e in &checker.errors {
        let req = e.requirement_number() as usize;
        debug_assert!(
            (1..=11).contains(&req),
            "requirement_number() returned {req}, must be 1–11"
        );
        req_errors[req] += 1;
    }
    debug_assert_eq!(
        req_errors[1..].iter().sum::<usize>(),
        checker.errors.len(),
        "req_errors sum must equal total error count"
    );
    CheckResult {
        errors: checker.errors,
        extern_count: checker.extern_count,
        req_errors,
        expr_types: checker.expr_types,
    }
}

/// Collect inferred expression types for a set of programs without surfacing errors.
///
/// Used by the transpiler to get type information for stdlib prelude programs
/// (e.g. json.mvl, collections.mvl) so method-call sites in those files can
/// emit direct Rust rather than trait-dispatch (#554).
pub fn collect_prelude_expr_types(programs: &[Program]) -> HashMap<Span, Ty> {
    let mut checker = TypeChecker::new();
    for p in programs {
        checker.collect_declarations(&p.declarations);
    }
    for p in programs {
        for decl in &p.declarations {
            checker.check_decl(decl);
        }
    }
    checker.expr_types
}

pub fn check(prog: &Program) -> CheckResult {
    check_with_prelude(&[], prog)
}

// ── Effect subsetting (002-effect-system/Req 3) ───────────────────────────────

/// Returns `true` when `declared` covers `required` for effect propagation.
///
/// Rules:
/// - Different effect names never satisfy each other.
/// - `declared` with no param (wildcard) satisfies any `required` for that name.
/// - `declared` with a param satisfies `required` with the same or more-specific param,
///   using prefix matching for path-style params (e.g. `declared("/data")` covers
///   `required("/data/config.toml")`).
/// - `declared` with a param does NOT satisfy a `required` with no param (general
///   access required but only restricted access declared).
fn effect_satisfies(declared: &Effect, required: &Effect) -> bool {
    if declared.name != required.name {
        return false;
    }
    match (&declared.param, &required.param) {
        // Wildcard declared covers everything with that name.
        (None, _) => true,
        // Specific declared cannot cover a general requirement.
        (Some(_), None) => false,
        // Both parametrized: declared must be a prefix of required (path subsetting).
        (Some(d), Some(r)) => r.starts_with(d.as_str()),
    }
}

// ── Valid effect names (002-effect-system/Req 2) ──────────────────────────────

/// The canonical set of effect names permitted in `! Effect` declarations.
///
/// Per 002-effect-system/Req 2: "Effects MUST be fine-grained, not a single `IO` bucket.
/// The minimum set: Console, FileRead, FileWrite, FileDelete, Net, DB, ProcessSpawn,
/// Random, CryptoRandom, Clock, Env, Log, Async."
///
/// `Terminal` is an extended effect for raw terminal control (cursor, colors, raw key input)
/// distinct from `Console` (line-oriented stdin/stdout). See pkg.tui / std.tui (#174).
const VALID_EFFECT_NAMES: &[&str] = &[
    "Console",
    "FileRead",
    "FileWrite",
    "FileDelete",
    "Net",
    "DB",
    "ProcessSpawn",
    "Random",
    "CryptoRandom",
    "Clock",
    "Env",
    "Log",
    "Async",
    "Terminal",
];

// ── TypeChecker ──────────────────────────────────────────────────────────────

struct TypeChecker {
    errors: Vec<CheckError>,
    env: TypeEnv,
    /// Return type of the function currently being checked (for `?` and `return`).
    current_return_ty: Option<Ty>,
    /// Name of the function currently being checked (for effect error messages).
    current_fn_name: String,
    /// Effects declared by the current function (Req 7, 8).
    current_fn_effects: Vec<Effect>,
    /// Totality of the current function (Req 8); None = implicitly total.
    current_fn_totality: Option<Totality>,
    /// Count of extern declarations for assurance reporting.
    extern_count: usize,
    /// Stack of scope depths at each lambda entry point.
    ///
    /// Used by capture-immutability checking (ADR-0002): when a variable is looked
    /// up and its scope index is strictly less than the boundary recorded here, it
    /// was captured from an outer scope and must be immutable.
    lambda_scope_starts: Vec<usize>,
    /// Types that implement `Iterator<T>`, mapped to their element type.
    /// Populated by `register_impl` for `impl Iterator<T> for X` declarations.
    iterator_impls: HashMap<String, Ty>,
    /// Type parameter names in scope for the current function.
    current_type_params: HashSet<String>,
    /// Trait bounds for type params in the current function (from `where` clauses).
    current_type_constraints: HashMap<String, Vec<String>>,
    /// Inferred type for every expression, keyed by span. Populated during
    /// `infer_expr` and surfaced in [`CheckResult::expr_types`] for the
    /// transpiler to use when emitting type-specific Rust code (#554).
    expr_types: HashMap<Span, Ty>,
}

impl TypeChecker {
    fn new() -> Self {
        TypeChecker {
            errors: Vec::new(),
            env: TypeEnv::new(),
            current_return_ty: None,
            current_fn_name: String::new(),
            current_fn_effects: Vec::new(),
            current_fn_totality: None,
            extern_count: 0,
            lambda_scope_starts: Vec::new(),
            iterator_impls: HashMap::new(),
            current_type_params: HashSet::new(),
            current_type_constraints: HashMap::new(),
            expr_types: HashMap::new(),
        }
    }

    fn emit(&mut self, err: CheckError) {
        self.errors.push(err);
    }

    // ── Program ──────────────────────────────────────────────────────────

    fn check_program(&mut self, prog: &Program) {
        self.collect_declarations(&prog.declarations);
        for decl in &prog.declarations {
            self.check_decl(decl);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn check_src(src: &str) -> CheckResult {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        check(&prog)
    }

    fn errors_for(src: &str) -> Vec<CheckError> {
        check_src(src).errors
    }

    // ── Requirement 1 / Scenario: Basic type inference (#11) ─────────────

    #[test]
    fn literal_int_inferred() {
        let result = check_src("fn f() -> Int { 42 }");
        assert!(result.is_ok(), "errors: {:?}", result.errors);
    }

    #[test]
    fn literal_bool_inferred() {
        let result = check_src("fn f() -> Bool { true }");
        assert!(result.is_ok(), "errors: {:?}", result.errors);
    }

    #[test]
    fn arithmetic_requires_numeric() {
        let errors = errors_for("fn f() -> Int { true + 1 }");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::NonNumericArithmetic { .. })),
            "expected NonNumericArithmetic, got: {errors:?}"
        );
    }

    #[test]
    fn arithmetic_mixed_types_rejected() {
        let errors = errors_for("fn f() -> Float { 1 + 2.0 }");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::ArithmeticTypeMismatch { .. })),
            "expected ArithmeticTypeMismatch, got: {errors:?}"
        );
    }

    #[test]
    fn logic_requires_bool() {
        let errors = errors_for("fn f() -> Bool { 1 && true }");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::LogicTypeMismatch { .. })),
            "expected LogicTypeMismatch, got: {errors:?}"
        );
    }

    // ── Requirement 1 / Scenario: ADT checking (#12) ─────────────────────

    #[test]
    fn struct_construction_valid() {
        let src =
            "type Point = struct { x: Int, y: Int }\nfn make() -> Point { Point { x: 1, y: 2 } }";
        let result = check_src(src);
        // UndefinedFunction for `Point { x: 1, y: 2 }` should not appear;
        // struct construction goes through Construct not FnCall
        let serious: Vec<_> = result
            .errors
            .iter()
            .filter(|e| !matches!(e, CheckError::TypeMismatch { .. }))
            .collect();
        assert!(
            serious.iter().all(|e| !matches!(
                e,
                CheckError::MissingField { .. } | CheckError::UnknownField { .. }
            )),
            "unexpected errors: {serious:?}"
        );
    }

    #[test]
    fn struct_missing_field_rejected() {
        let src = "type Point = struct { x: Int, y: Int }\nfn make() -> Point { Point { x: 1 } }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::MissingField { field, .. } if field == "y")),
            "expected MissingField(y), got: {errors:?}"
        );
    }

    #[test]
    fn field_access_on_enum_rejected() {
        let src = "type Color = enum { Red, Green, Blue }\nfn f(c: Color) -> Int { c.value }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::FieldAccessOnEnum { .. })),
            "expected FieldAccessOnEnum, got: {errors:?}"
        );
    }

    // ── Requirement 3 / Scenario: Exhaustive match (#13) ─────────────────

    #[test]
    fn option_match_exhaustive() {
        let src = "fn f(x: Option[Int]) -> Int { match x { Some(v) => v, None => 0 } }";
        let result = check_src(src);
        let exhaustive_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::NonExhaustiveMatch { .. }))
            .collect();
        assert!(
            exhaustive_errors.is_empty(),
            "should be exhaustive, got: {exhaustive_errors:?}"
        );
    }

    #[test]
    fn option_match_missing_none_rejected() {
        let src = "fn f(x: Option[Int]) -> Int { match x { Some(v) => v } }";
        let errors = errors_for(src);
        assert!(
            errors.iter().any(|e| matches!(
                e,
                CheckError::NonExhaustiveMatch { missing, .. } if missing.contains(&"None".to_string())
            )),
            "expected NonExhaustiveMatch(None), got: {errors:?}"
        );
    }

    #[test]
    fn result_match_exhaustive() {
        let src = "fn f(x: Result[Int, String]) -> Int { match x { Ok(v) => v, Err(_) => 0 } }";
        let result = check_src(src);
        let exhaustive_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::NonExhaustiveMatch { .. }))
            .collect();
        assert!(
            exhaustive_errors.is_empty(),
            "should be exhaustive, got: {exhaustive_errors:?}"
        );
    }

    #[test]
    fn result_match_missing_err_rejected() {
        let src = "fn f(x: Result[Int, String]) -> Int { match x { Ok(v) => v } }";
        let errors = errors_for(src);
        assert!(
            errors.iter().any(|e| matches!(
                e,
                CheckError::NonExhaustiveMatch { missing, .. } if missing.contains(&"Err(_)".to_string())
            )),
            "expected NonExhaustiveMatch(Err(_)), got: {errors:?}"
        );
    }

    // ── Requirement 4/5 / Scenario: Option/Result enforcement (#14) ───────

    #[test]
    fn option_direct_access_rejected() {
        let src = "fn f(x: Option[Int]) -> Int { x.value }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::OptionDirectAccess { .. })),
            "expected OptionDirectAccess, got: {errors:?}"
        );
    }

    #[test]
    fn result_ignored_rejected() {
        let src = "fn produce() -> Result[Int, String] { Ok(1) }\nfn f() -> Unit { produce() }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::ResultIgnored { .. })),
            "expected ResultIgnored, got: {errors:?}"
        );
    }

    // ── Requirement 6 / Scenario: Immutability enforcement (#17) ──────────

    #[test]
    fn assign_to_immutable_rejected() {
        let src = "fn f() -> Unit { let x: Int = 1; x = 2; }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::AssignToImmutable { name, .. } if name == "x")),
            "expected AssignToImmutable(x), got: {errors:?}"
        );
    }

    #[test]
    fn assign_to_mutable_allowed() {
        let src = "fn f() -> Unit { let mut x: Int = 1; x = 2; }";
        let errors = errors_for(src);
        let assign_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, CheckError::AssignToImmutable { .. }))
            .collect();
        assert!(
            assign_errors.is_empty(),
            "should allow mut assignment, got: {assign_errors:?}"
        );
    }

    // ── Requirement 2 / Scenario: Ownership / use-after-move (#15) ────────

    #[test]
    fn use_after_move_rejected() {
        // move(x) is the MVL syntax for explicit move
        let src = "fn f() -> Int { let x: Int = 1; let y: Int = move(x); x }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::UseAfterMove { name, .. } if name == "x")),
            "expected UseAfterMove(x), got: {errors:?}"
        );
    }

    // ── Fix: enum constructors as expressions ─────────────────────────────

    #[test]
    fn some_constructor_no_undefined_function() {
        let src = "fn f(x: Int) -> Option[Int] { Some(x) }";
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedFunction { name, .. } if name == "Some")),
            "Some() should not emit UndefinedFunction, got: {errors:?}"
        );
    }

    #[test]
    fn ok_constructor_no_undefined_function() {
        let src = "fn produce() -> Result[Int, String] { Ok(1) }";
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedFunction { name, .. } if name == "Ok")),
            "Ok() should not emit UndefinedFunction, got: {errors:?}"
        );
    }

    #[test]
    fn err_constructor_no_undefined_function() {
        let src = "fn f() -> Result[Int, String] { Err(\"oops\") }";
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedFunction { name, .. } if name == "Err")),
            "Err() should not emit UndefinedFunction, got: {errors:?}"
        );
    }

    #[test]
    fn none_ident_no_undefined_variable() {
        let src = "fn f() -> Option[Int] { None }";
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "None")),
            "None should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn user_enum_unit_variant_no_undefined_variable() {
        let src = "type Dir = enum { North, South }\nfn f() -> Dir { North }";
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "North")
            ),
            "enum unit variant should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    // ── Fix: assignment type-check ────────────────────────────────────────

    #[test]
    fn assign_type_mismatch_rejected() {
        let src = "fn f() -> Unit { let mut x: Int = 1; x = true; }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
            "expected TypeMismatch on type-incompatible assignment, got: {errors:?}"
        );
    }

    // ── Fix: lambda return type check ─────────────────────────────────────
    //
    // Lambda parsing is not yet implemented in the parser; the lambda return-type
    // check in infer_expr is exercised via direct AST construction in the
    // integration test suite when lambda parsing lands.  The check itself is
    // verified by ensuring the guard condition compiles and the path exists.

    // ── Effect name validation (002-effect-system Req 2) ──────────────────

    #[test]
    fn invalid_effect_name_rejected() {
        let src = r#"fn f() -> Unit ! Foo { println("hi"); }"#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::InvalidEffectName { name, .. } if name == "Foo")),
            "expected InvalidEffectName for unknown effect 'Foo', got: {errors:?}"
        );
    }

    #[test]
    fn valid_effect_names_accepted() {
        let src = r#"fn f() -> Unit ! Console, Net, DB, Terminal { println("hi"); }"#;
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::InvalidEffectName { .. })),
            "expected no InvalidEffectName for valid effects, got: {errors:?}"
        );
    }

    #[test]
    fn terminal_effect_name_accepted() {
        // Terminal is a distinct effect from Console — raw terminal control (cursor,
        // colors, single keypress) vs line-oriented I/O. See std.tui / #174.
        let src = r#"fn f() -> Unit ! Terminal { }"#;
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::InvalidEffectName { .. })),
            "expected Terminal to be a valid effect name, got: {errors:?}"
        );
    }

    // ── Parametrized effects (002-effect-system Req 3 / issue #290) ──────

    #[test]
    fn parametrized_effect_parses_and_is_accepted() {
        let src = r#"fn f() -> Unit ! FileRead("/etc/app/") { }"#;
        let errors = errors_for(src);
        assert!(
            errors.is_empty(),
            "expected no errors for parametrized effect, got: {errors:?}"
        );
    }

    #[test]
    fn parametrized_effect_invalid_name_rejected() {
        let src = r#"fn f() -> Unit ! Bogus("/path") { }"#;
        let errors = errors_for(src);
        assert!(
            errors.iter().any(
                |e| matches!(e, CheckError::InvalidEffectName { name, .. } if name == "Bogus")
            ),
            "expected InvalidEffectName for parametrized unknown effect, got: {errors:?}"
        );
    }

    #[test]
    fn general_caller_covers_parametrized_callee() {
        // Unparametrized FileRead (wildcard) covers FileRead("/etc/").
        let src = r#"
            extern "kernel" {
                fn read_config() -> Unit ! FileRead("/etc/")
            }
            fn caller() -> Unit ! FileRead {
                read_config()
            }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(|e| matches!(
                e,
                CheckError::UndeclaredEffect { .. } | CheckError::MissingEffect { .. }
            )),
            "general FileRead should cover FileRead(\"/etc/\"), got: {errors:?}"
        );
    }

    #[test]
    fn prefix_caller_covers_more_specific_parametrized_callee() {
        // FileRead("/etc/") covers FileRead("/etc/app/config.toml") via prefix match.
        let src = r#"
            extern "kernel" {
                fn read_file() -> Unit ! FileRead("/etc/app/config.toml")
            }
            fn caller() -> Unit ! FileRead("/etc/") {
                read_file()
            }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(|e| matches!(
                e,
                CheckError::MissingEffect { .. } | CheckError::UndeclaredEffect { .. }
            )),
            "FileRead(\"/etc/\") should cover FileRead(\"/etc/app/config.toml\"), got: {errors:?}"
        );
    }

    #[test]
    fn specific_caller_does_not_cover_different_path_callee() {
        // FileRead("/data/") must NOT satisfy FileRead("/etc/").
        let src = r#"
            extern "kernel" {
                fn read_etc() -> Unit ! FileRead("/etc/")
            }
            fn caller() -> Unit ! FileRead("/data/") {
                read_etc()
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::MissingEffect { .. })),
            "FileRead(\"/data/\") should NOT cover FileRead(\"/etc/\"), got: {errors:?}"
        );
    }

    #[test]
    fn specific_caller_does_not_cover_general_callee() {
        // FileRead("/etc/") must NOT satisfy unparametrized FileRead.
        let src = r#"
            extern "kernel" {
                fn read_any() -> Unit ! FileRead
            }
            fn caller() -> Unit ! FileRead("/etc/") {
                read_any()
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::MissingEffect { .. })),
            "FileRead(\"/etc/\") should NOT cover general FileRead, got: {errors:?}"
        );
    }

    // ── Lambda capture immutability (ADR-0002) ────────────────────────────
    //
    // Lambda surface syntax is not yet parsed (parser does not handle `|p| expr`).
    // The capture-check logic is exercised via direct AST construction below.

    #[test]
    fn lambda_mutable_capture_rejected_via_ast() {
        use crate::mvl::parser::ast::{
            Block, Decl, Expr, FnDecl, Literal, Param, Pattern, Program, Stmt, TypeExpr,
        };
        use crate::mvl::parser::lexer::Span;

        let dummy_span = Span {
            line: 1,
            col: 1,
            offset: 0,
            len: 0,
        };
        let unit_ty = TypeExpr::Base {
            name: "Unit".into(),
            args: vec![],
            span: dummy_span,
        };
        let int_ty = TypeExpr::Base {
            name: "Int".into(),
            args: vec![],
            span: dummy_span,
        };

        // Build: fn f() -> Unit {
        //    let mut x = 1;
        //    let g = |y: Int| -> Int { x };
        // }
        let lambda = Expr::Lambda {
            params: vec![Param {
                name: "y".into(),
                ty: int_ty.clone(),
                mutable: false,
                capability: None,
                refinement: None,
                span: dummy_span,
            }],
            ret_type: Some(Box::new(int_ty.clone())),
            body: Box::new(Expr::Ident("x".into(), dummy_span)),
            span: dummy_span,
        };

        let fn_int_int_ty = TypeExpr::Fn {
            params: vec![int_ty.clone()],
            ret: Box::new(int_ty.clone()),
            effects: vec![],
            span: dummy_span,
        };
        let prog = Program {
            span: dummy_span,
            declarations: vec![Decl::Fn(FnDecl {
                visible: false,
                is_test: false,
                is_builtin: false,
                totality: None,
                name: "f".into(),
                type_params: vec![],
                params: vec![],
                return_type: Box::new(unit_ty),
                return_refinement: None,
                effects: vec![],
                constraints: vec![],
                body: Block {
                    stmts: vec![
                        Stmt::Let {
                            mutable: true,
                            pattern: Pattern::Ident("x".into(), dummy_span),
                            ty: int_ty,
                            init: Expr::Literal(Literal::Integer(1), dummy_span),
                            span: dummy_span,
                        },
                        Stmt::Let {
                            mutable: false,
                            pattern: Pattern::Ident("g".into(), dummy_span),
                            ty: fn_int_int_ty,
                            init: lambda,
                            span: dummy_span,
                        },
                    ],
                    span: dummy_span,
                },
                span: dummy_span,
            })],
        };

        let result = check(&prog);
        assert!(
            result.errors.iter().any(|e| matches!(
                e, CheckError::CaptureMutabilityViolation { name, .. } if name == "x"
            )),
            "expected CaptureMutabilityViolation for mutable capture of 'x', got: {:?}",
            result.errors
        );
    }

    #[test]
    fn lambda_immutable_capture_accepted_via_ast() {
        use crate::mvl::parser::ast::{
            Block, Decl, Expr, FnDecl, Literal, Param, Pattern, Program, Stmt, TypeExpr,
        };
        use crate::mvl::parser::lexer::Span;

        let dummy_span = Span {
            line: 1,
            col: 1,
            offset: 0,
            len: 0,
        };
        let unit_ty = TypeExpr::Base {
            name: "Unit".into(),
            args: vec![],
            span: dummy_span,
        };
        let int_ty = TypeExpr::Base {
            name: "Int".into(),
            args: vec![],
            span: dummy_span,
        };

        // Build: fn f() -> Unit {
        //    let x = 1;          ← immutable
        //    let g = |y: Int| -> Int { x };
        // }
        let lambda = Expr::Lambda {
            params: vec![Param {
                name: "y".into(),
                ty: int_ty.clone(),
                mutable: false,
                capability: None,
                refinement: None,
                span: dummy_span,
            }],
            ret_type: Some(Box::new(int_ty.clone())),
            body: Box::new(Expr::Ident("x".into(), dummy_span)),
            span: dummy_span,
        };

        let fn_int_int_ty = TypeExpr::Fn {
            params: vec![int_ty.clone()],
            ret: Box::new(int_ty.clone()),
            effects: vec![],
            span: dummy_span,
        };
        let prog = Program {
            span: dummy_span,
            declarations: vec![Decl::Fn(FnDecl {
                visible: false,
                is_test: false,
                is_builtin: false,
                totality: None,
                name: "f".into(),
                type_params: vec![],
                params: vec![],
                return_type: Box::new(unit_ty),
                return_refinement: None,
                effects: vec![],
                constraints: vec![],
                body: Block {
                    stmts: vec![
                        Stmt::Let {
                            mutable: false, // immutable
                            pattern: Pattern::Ident("x".into(), dummy_span),
                            ty: int_ty,
                            init: Expr::Literal(Literal::Integer(1), dummy_span),
                            span: dummy_span,
                        },
                        Stmt::Let {
                            mutable: false,
                            pattern: Pattern::Ident("g".into(), dummy_span),
                            ty: fn_int_int_ty,
                            init: lambda,
                            span: dummy_span,
                        },
                    ],
                    span: dummy_span,
                },
                span: dummy_span,
            })],
        };

        let result = check(&prog);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CheckError::CaptureMutabilityViolation { .. })),
            "expected no CaptureMutabilityViolation for immutable capture, got: {:?}",
            result.errors
        );
    }

    // ── Fix: if-expression branch type check ─────────────────────────────

    #[test]
    fn if_expr_branch_type_mismatch_rejected() {
        // The `if` must be in expression position (init of `let`) to hit Expr::If.
        let src = "fn f(b: Bool) -> Int { let x: Int = if b { 1 } else { true }; x }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
            "expected TypeMismatch for mismatched if-expression branches, got: {errors:?}"
        );
    }

    // ── Fix #189: match/if in tail position returns arm values, not Unit ──

    #[test]
    fn match_in_tail_position_accepted() {
        // GIVEN: a function whose only statement is a match in tail position
        // THEN: no type errors — the match's arm type satisfies the return type
        let src = r#"
            fn classify(x: Int) -> Int {
                match x {
                    0 => 1,
                    _ => 2,
                }
            }
        "#;
        let result = check_src(src);
        assert!(
            result.is_ok(),
            "expected no errors for match in tail position, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn match_in_tail_position_wrong_type_rejected() {
        // GIVEN: a function whose tail match arms return Bool but the fn declares Int
        // THEN: TypeMismatch error is emitted (was silently accepted before fix)
        let src = r#"
            fn wrong(x: Int) -> Int {
                match x {
                    0 => true,
                    _ => false,
                }
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
            "expected TypeMismatch for tail match returning Bool in Int function, got: {errors:?}"
        );
    }

    #[test]
    fn if_else_in_tail_position_accepted() {
        // GIVEN: a function whose only statement is an if/else in tail position
        // THEN: no type errors
        let src = r#"
            fn abs(x: Int) -> Int {
                if x >= 0 {
                    x
                } else {
                    0 - x
                }
            }
        "#;
        let result = check_src(src);
        assert!(
            result.is_ok(),
            "expected no errors for if/else in tail position, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn if_else_in_tail_position_branch_mismatch_rejected() {
        // GIVEN: if/else in tail position where branches return different types
        // THEN: TypeMismatch error
        let src = r#"
            fn bad(b: Bool) -> Int {
                if b {
                    1
                } else {
                    true
                }
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
            "expected TypeMismatch for mismatched if/else tail branches, got: {errors:?}"
        );
    }

    #[test]
    fn else_if_chain_in_tail_position_accepted() {
        // GIVEN: if/else-if/else chain in tail position, all branches return Int
        // THEN: no type errors — the nested else-if types are inferred recursively
        let src = r#"
            fn classify(x: Int) -> Int {
                if x > 0 {
                    1
                } else if x < 0 {
                    0 - 1
                } else {
                    0
                }
            }
        "#;
        let result = check_src(src);
        assert!(
            result.is_ok(),
            "expected no errors for else-if chain in tail position, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn else_if_branch_type_mismatch_rejected() {
        // GIVEN: else-if branch returns a different type than the then branch
        // THEN: TypeMismatch should be emitted (was silently accepted before fix)
        let src = r#"
            fn bad(x: Int) -> Int {
                if x > 0 {
                    1
                } else if x < 0 {
                    true
                } else {
                    0
                }
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
            "expected TypeMismatch for else-if branch type mismatch, got: {errors:?}"
        );
    }

    #[test]
    fn match_in_tail_position_result_ignored_rejected() {
        // GIVEN: a Unit function whose tail match returns a Result
        // THEN: ResultIgnored is emitted — same behaviour as a bare tail expression
        let src = r#"
            fn produce() -> Result[Int, String] { Ok(1) }
            fn f(x: Int) -> Unit {
                match x {
                    0 => produce(),
                    _ => produce(),
                }
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::ResultIgnored { .. })),
            "expected ResultIgnored for match-in-tail-position discarding Result, got: {errors:?}"
        );
    }

    #[test]
    fn match_inside_if_else_tail_position_accepted() {
        // GIVEN: if/else in tail position where the else branch contains a match
        // THEN: no type errors — nested combinations resolve correctly
        let src = r#"
            fn f(flag: Bool, x: Int) -> Int {
                if flag {
                    0
                } else {
                    match x {
                        0 => 1,
                        _ => 2,
                    }
                }
            }
        "#;
        let result = check_src(src);
        assert!(
            result.is_ok(),
            "expected no errors for match inside if-else tail position, got: {:?}",
            result.errors
        );
    }
    // ── Fix #332: enum variant qualified paths in == / != expressions ─────

    #[test]
    fn enum_variant_qualified_path_in_eq_no_undefined_variable() {
        let src = "type Status = enum { Absent, Present }
fn f(s: Status) -> Bool { s == Status::Absent }";
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "Status::Absent")
            ),
            "qualified enum variant in == should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_in_ne_no_undefined_variable() {
        let src = "type Status = enum { Absent, Present }
fn f(s: Status) -> Bool { s != Status::Absent }";
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "Status::Absent")
            ),
            "qualified enum variant in != should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_compound_boolean_expr() {
        // Two distinct enums with different variant names — both paths must resolve.
        let src = "type A = enum { X, Y }
type B = enum { P, Q }
fn f(a: A, b: B) -> Bool { a == A::X && b == B::P }";
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedVariable { .. })),
            "compound enum == should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_in_struct_field_eq() {
        // Matches the medical_triage scenario: field access on the left, qualified variant on the right.
        let src = r#"
            type BreathingStatus = enum { Absent, Labored, Normal }
            type Patient = struct { breathing: BreathingStatus, age: Int }
            fn is_apnoeic(v: Patient) -> Bool {
                v.breathing == BreathingStatus::Absent
            }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "BreathingStatus::Absent")
            ),
            "qualified enum variant in struct field == should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_in_let_binding() {
        let src = r#"
            type Status = enum { Absent, Present }
            fn f() -> Bool {
                let x = Status::Absent
                x == Status::Absent
            }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "Status::Absent")
            ),
            "qualified enum variant in let binding should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_in_return_value() {
        let src = r#"
            type Status = enum { Absent, Present }
            fn default_status() -> Status {
                Status::Absent
            }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "Status::Absent")
            ),
            "qualified enum variant in return should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_as_fn_arg() {
        let src = r#"
            type Status = enum { Absent, Present }
            fn check(s: Status) -> Bool { s == Status::Absent }
            fn outer() -> Bool { check(Status::Absent) }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "Status::Absent")
            ),
            "qualified enum variant as fn arg should not emit UndefinedVariable, got: {errors:?}"
        );
    }
}
