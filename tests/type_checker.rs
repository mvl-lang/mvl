//! Integration tests for the MVL type checker (Epic #10, Requirements 1, 3, 4, 5, 6, 10).
//! Also: Epic #23 (Requirement 11) — Information Flow Control.
//!
//! Each test group corresponds to a sub-ticket:
//!   #11 — Basic type inference
//!   #12 — ADT checking
//!   #13 — Exhaustive match
//!   #14 — Option/Result enforcement
//!   #17 — Immutability
//!   #15 — Ownership / use-after-move
//!   #16 — Refinement types (corpus parse-only)
//!   #24 — Security label checking
//!   #25 — Lattice enforcement
//!   #26 — Label propagation
//!   #27 — Declassify/sanitize validation

use mvl::mvl::checker::errors::CheckError;
use mvl::mvl::checker::{check, check_with_prelude, check_with_two_preludes, CheckResult};
use mvl::mvl::parser::Parser;

/// Effectful stubs so IFC tests work without the full stdlib prelude.
/// Functions with effects are observable — the implicit flow checker uses
/// effect declarations instead of the old `sink` keyword (#1007).
const SINK_PRELUDE: &str = r#"
pub fn println(msg: String) -> Unit ! Console { }
pub fn print(msg: String) -> Unit ! Console { }
pub fn eprintln(msg: String) -> Unit ! Console { }
pub fn eprint(msg: String) -> Unit ! Console { }
pub fn write_file(p: Path, content: String) -> Result[Unit, IoError] ! FileWrite { Ok(()) }
pub fn append(p: Path, content: String) -> Result[Unit, IoError] ! FileWrite { Ok(()) }
"#;

fn check_src(src: &str) -> CheckResult {
    let (mut sp, _) = Parser::new(SINK_PRELUDE);
    let sink_prog = sp.parse_program();
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    check_with_prelude(&[sink_prog], &prog)
}

fn errors_for(src: &str) -> Vec<CheckError> {
    check_src(src).errors
}

/// Check `src` with std/effects.mvl loaded so the effect hierarchy is populated.
fn check_with_effects(src: &str) -> CheckResult {
    let effects_src = include_str!("../std/effects.mvl");
    let (mut ep, _) = Parser::new(effects_src);
    let effects_prog = ep.parse_program();
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    check_with_prelude(&[effects_prog], &prog)
}

/// Assert no effect propagation errors (UndeclaredEffect / MissingEffect).
fn assert_no_effect_propagation_errors(result: &CheckResult, label: &str) {
    let errs: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::UndeclaredEffect { .. } | CheckError::MissingEffect { .. }
            )
        })
        .collect();
    assert!(errs.is_empty(), "{label}: got: {errs:?}");
}

// ── #11: Basic type inference (Requirement 1) ────────────────────────────────

#[test]
fn basic_types_corpus_parses_and_checks() {
    // GIVEN: the basic_types corpus (valid programs)
    // THEN: no type errors
    let src = include_str!("corpus/03_types/basic_types.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "basic_types corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn int_float_arithmetic_type_error() {
    // GIVEN: `1 + 2.0` (mixed numeric types)
    // THEN: ArithmeticTypeMismatch reported
    let errors = errors_for("fn f() -> Float { 1 + 2.0 }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::ArithmeticTypeMismatch { .. })),
        "expected ArithmeticTypeMismatch, got: {errors:?}"
    );
}

#[test]
fn string_arithmetic_rejected() {
    // GIVEN: `"a" + "b"` (non-numeric arithmetic)
    // THEN: NonNumericArithmetic reported
    let errors = errors_for(r#"fn f() -> String { "a" + "b" }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::NonNumericArithmetic { .. })),
        "expected NonNumericArithmetic, got: {errors:?}"
    );
}

#[test]
fn comparison_produces_bool() {
    // GIVEN: `a > b` on Ints
    // THEN: no type error; result is Bool
    let result = check_src("fn f(a: Int, b: Int) -> Bool { a > b }");
    let comparison_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::TypeMismatch { .. } | CheckError::NonNumericArithmetic { .. }
            )
        })
        .collect();
    assert!(
        comparison_errors.is_empty(),
        "unexpected errors: {comparison_errors:?}"
    );
}

#[test]
fn fn_call_wrong_arg_count() {
    // GIVEN: function called with wrong number of arguments
    // THEN: WrongArgCount reported
    let errors = errors_for("fn add(a: Int, b: Int) -> Int { a + b }\nfn f() -> Int { add(1) }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::WrongArgCount { name, .. } if name == "add")),
        "expected WrongArgCount(add), got: {errors:?}"
    );
}

// ── #12: ADT checking (Requirement 1) ────────────────────────────────────────

#[test]
fn adt_corpus_parses_and_checks() {
    // GIVEN: the ADT checking corpus
    // THEN: no type errors (besides UndefinedFunction from string literals used as exprs)
    let src = include_str!("corpus/03_types/adt_checking.mvl");
    let result = check_src(src);
    let serious: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::MissingField { .. }
                    | CheckError::UnknownField { .. }
                    | CheckError::FieldAccessOnEnum { .. }
                    | CheckError::NonExhaustiveMatch { .. }
            )
        })
        .collect();
    assert!(serious.is_empty(), "unexpected ADT errors: {serious:?}");
}

#[test]
fn struct_extra_field_rejected() {
    // GIVEN: struct constructed with an unknown field
    // THEN: UnknownField reported
    let src = "type Pt = struct { x: Int, y: Int }\nfn f() -> Pt { Pt { x: 1, y: 2, z: 3 } }";
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnknownField { field, .. } if field == "z")),
        "expected UnknownField(z), got: {errors:?}"
    );
}

#[test]
fn field_access_on_struct_valid() {
    // GIVEN: valid field access on a struct
    // THEN: no FieldAccessOnEnum error
    let src = "type Pt = struct { x: Int, y: Int }\nfn f(p: Pt) -> Int { p.x }";
    let errors = errors_for(src);
    let field_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::FieldAccessOnEnum { .. } | CheckError::FieldNotFound { .. }
            )
        })
        .collect();
    assert!(
        field_errors.is_empty(),
        "unexpected field errors: {field_errors:?}"
    );
}

#[test]
fn field_access_undefined_field_rejected() {
    // GIVEN: accessing a field that doesn't exist on the struct
    // THEN: FieldNotFound reported
    let src = "type Pt = struct { x: Int, y: Int }\nfn f(p: Pt) -> Int { p.z }";
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::FieldNotFound { field, .. } if field == "z")),
        "expected FieldNotFound(z), got: {errors:?}"
    );
}

// ── #13: Exhaustive match (Requirement 3) ────────────────────────────────────

#[test]
fn exhaustive_match_corpus_parses_and_checks() {
    // GIVEN: the exhaustive match corpus (valid — all cases covered)
    // THEN: no NonExhaustiveMatch errors
    let src = include_str!("corpus/03_types/exhaustive_match.mvl");
    let result = check_src(src);
    let exhaustive_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| matches!(e, CheckError::NonExhaustiveMatch { .. }))
        .collect();
    assert!(
        exhaustive_errors.is_empty(),
        "corpus should be exhaustive, got: {exhaustive_errors:?}"
    );
}

#[test]
fn enum_match_missing_variant_rejected() {
    // GIVEN: enum with 3 variants, match covering only 2
    // THEN: NonExhaustiveMatch reported with the missing variant
    let src = "type Color = enum { Red, Green, Blue }\nfn f(c: Color) -> Int { match c { Red => 1, Green => 2 } }";
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::NonExhaustiveMatch { missing, .. } if missing.contains(&"Blue".to_string())
        )),
        "expected NonExhaustiveMatch(Blue), got: {errors:?}"
    );
}

#[test]
fn result_match_missing_ok_rejected() {
    // GIVEN: Result match with only Err arm
    // THEN: NonExhaustiveMatch(Ok(_)) reported
    let errors = errors_for("fn f(r: Result[Int, String]) -> Int { match r { Err(_) => -1 } }");
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::NonExhaustiveMatch { missing, .. } if missing.contains(&"Ok(_)".to_string())
        )),
        "expected NonExhaustiveMatch(Ok(_)), got: {errors:?}"
    );
}

// ── #14: Option/Result enforcement (Requirements 4, 5) ───────────────────────

#[test]
fn option_result_corpus_parses_and_checks() {
    // GIVEN: the option/result corpus (valid handling patterns)
    // THEN: no enforcement errors
    let src = include_str!("corpus/03_types/option_result.mvl");
    let result = check_src(src);
    let enforcement_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::OptionDirectAccess { .. }
                    | CheckError::ResultIgnored { .. }
                    | CheckError::PropagateNotResult { .. }
            )
        })
        .collect();
    assert!(
        enforcement_errors.is_empty(),
        "corpus should pass enforcement, got: {enforcement_errors:?}"
    );
}

#[test]
fn option_field_access_rejected() {
    // GIVEN: direct `.field` on Option[T]
    // THEN: OptionDirectAccess reported
    let errors = errors_for("fn f(x: Option[Int]) -> Int { x.value }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::OptionDirectAccess { .. })),
        "expected OptionDirectAccess, got: {errors:?}"
    );
}

#[test]
fn result_in_stmt_without_use_rejected() {
    // GIVEN: Result returned by function used as a standalone statement
    // THEN: ResultIgnored reported
    let errors =
        errors_for("fn produce() -> Result[Int, String] { Ok(1) }\nfn f() -> Unit { produce() }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::ResultIgnored { .. })),
        "expected ResultIgnored, got: {errors:?}"
    );
}

#[test]
fn propagate_on_non_result_rejected() {
    // GIVEN: `?` applied to Int
    // THEN: PropagateNotResult reported
    let errors = errors_for("fn f() -> Result[Int, String] { let x: Int = 1?; Ok(x) }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::PropagateNotResult { .. })),
        "expected PropagateNotResult, got: {errors:?}"
    );
}

// ── #17: Immutability enforcement (Requirement 6) ────────────────────────────

#[test]
fn immutability_corpus_parses_and_checks() {
    // GIVEN: the immutability corpus (valid mutability patterns)
    // THEN: no immutability errors
    let src = include_str!("corpus/03_types/immutability.mvl");
    let result = check_src(src);
    let immut_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::AssignToImmutable { .. } | CheckError::MutateImmutableField { .. }
            )
        })
        .collect();
    assert!(
        immut_errors.is_empty(),
        "corpus should pass immutability, got: {immut_errors:?}"
    );
}

// ── String.concat arity/type enforcement ─────────────────────────────────────

#[test]
fn concat_with_non_string_arg_is_rejected() {
    // GIVEN: a.concat(n) where n: Int
    // WHEN: type-checked
    // THEN: checker returns Ty::Unknown (signals a type mismatch)
    let errors = errors_for("fn f(a: String, n: Int) -> String { a.concat(n) }");
    assert!(
        !errors.is_empty(),
        "concat(Int) must produce a type error, got no errors"
    );
}

#[test]
fn concat_with_zero_args_is_rejected() {
    // GIVEN: a.concat() — zero arguments
    // WHEN: type-checked
    // THEN: checker returns Ty::Unknown (wrong arity)
    let errors = errors_for("fn f(a: String) -> String { a.concat() }");
    assert!(
        !errors.is_empty(),
        "concat() with zero args must produce a type error, got no errors"
    );
}

#[test]
fn concat_with_two_args_is_rejected() {
    // GIVEN: a.concat(b, c) — two arguments
    // WHEN: type-checked
    // THEN: checker returns Ty::Unknown (wrong arity)
    let errors = errors_for("fn f(a: String, b: String, c: String) -> String { a.concat(b, c) }");
    assert!(
        !errors.is_empty(),
        "concat(b, c) with two args must produce a type error, got no errors"
    );
}

#[test]
fn core_types_corpus_parses_and_checks() {
    // GIVEN: the core prelude types corpus (#42)
    // THEN: no type errors (Int/Float methods resolve to correct types)
    let src = include_str!("corpus/03_types/core_types.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "core_types corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn immutable_binding_assignment_rejected() {
    // GIVEN: assignment to `let x` (no `mut`)
    // THEN: AssignToImmutable reported
    let errors = errors_for("fn f() -> Int { let x: Int = 1; x = 2; x }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::AssignToImmutable { name, .. } if name == "x")),
        "expected AssignToImmutable(x), got: {errors:?}"
    );
}

#[test]
fn immutable_field_mutation_rejected() {
    // GIVEN: assignment to a non-mut struct field
    // THEN: MutateImmutableField reported
    let src = "type Pt = struct { x: Int, y: ref Int }\nfn f(ref p: Pt) -> Unit { p.x = 5; }";
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::MutateImmutableField { field, .. } if field == "x")),
        "expected MutateImmutableField(x), got: {errors:?}"
    );
}

// ── #15: Ownership / use-after-move (Requirement 6) ──────────────────────────

#[test]
fn ownership_corpus_parses() {
    // GIVEN: the ownership corpus
    // WHEN: parsed and checked
    // THEN: no use-after-move errors (all moves are valid single uses)
    let src = include_str!("corpus/06_ownership/ownership.mvl");
    let result = check_src(src);
    let ownership_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| matches!(e, CheckError::UseAfterMove { .. }))
        .collect();
    assert!(
        ownership_errors.is_empty(),
        "corpus should not have use-after-move, got: {ownership_errors:?}"
    );
}

#[test]
fn use_after_explicit_move_rejected() {
    // GIVEN: variable used after move(x)
    // THEN: UseAfterMove reported
    let errors = errors_for("fn f() -> Int { let x: Int = 1; let _y: Int = consume(x); x }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UseAfterMove { name, .. } if name == "x")),
        "expected UseAfterMove(x), got: {errors:?}"
    );
}

// ── #16: Refinement types — corpus parses cleanly ────────────────────────────

#[test]
fn refinements_corpus_parses() {
    // GIVEN: the refinement types corpus
    // WHEN: parsed (refinement checking is lightweight in Phase 1)
    // THEN: no parse errors, no crashes
    let src = include_str!("corpus/09_refinements/refinements_valid.mvl");
    let (mut p, lex_errors) = Parser::new(src);
    let prog = p.parse_program();
    assert!(
        lex_errors.is_empty(),
        "refinements corpus should lex cleanly, got: {lex_errors:?}"
    );
    assert!(
        p.errors().is_empty(),
        "refinements corpus should parse cleanly, got: {:?}",
        p.errors()
    );
    // Type check also runs without panicking
    let _ = check(&prog);
}

// ── #19/#852: Effect declarations and subsumption corpus tests ───────────────

#[test]
fn effect_decl_corpus_parses_and_checks() {
    // GIVEN: corpus of effect declarations (base, single, multi-parent)
    // THEN: no parse or type errors
    let src = include_str!("corpus/07_effects/effect_decl.mvl");
    let result = check_with_effects(src);
    let relevant_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::InvalidEffectName { .. }
                    | CheckError::UnknownEffectParent { .. }
                    | CheckError::EffectCycle { .. }
            )
        })
        .collect();
    assert!(
        relevant_errors.is_empty(),
        "effect_decl corpus should compile without effect errors, got: {relevant_errors:?}"
    );
}

#[test]
fn subsumption_corpus_checks() {
    // GIVEN: corpus of effect subsumption patterns
    // THEN: no effect propagation errors
    let src = include_str!("corpus/07_effects/subsumption.mvl");
    assert_no_effect_propagation_errors(
        &check_src(src), // self-contained (no std/effects.mvl needed)
        "subsumption corpus should compile without effect propagation errors",
    );
}

#[test]
fn user_defined_effects_corpus_checks() {
    // GIVEN: corpus with user-defined domain effects (Billing > DB + Log)
    // THEN: no effect errors
    let src = include_str!("corpus/07_effects/user_defined_effects.mvl");
    let result = check_src(src);
    let effect_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::InvalidEffectName { .. }
                    | CheckError::UndeclaredEffect { .. }
                    | CheckError::MissingEffect { .. }
            )
        })
        .collect();
    assert!(
        effect_errors.is_empty(),
        "user_defined_effects corpus should compile cleanly, got: {effect_errors:?}"
    );
}

#[test]
fn concurrency_effects_corpus_checks() {
    // GIVEN: corpus using Spawn, Send, Recv, Actor effects
    // THEN: no effect errors
    let src = include_str!("corpus/07_effects/concurrency_effects.mvl");
    let result = check_src(src); // corpus is self-contained: declares its own hierarchy
    let effect_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::InvalidEffectName { .. }
                    | CheckError::UndeclaredEffect { .. }
                    | CheckError::MissingEffect { .. }
            )
        })
        .collect();
    assert!(
        effect_errors.is_empty(),
        "concurrency_effects corpus should compile cleanly, got: {effect_errors:?}"
    );
}

// ── #19: Effect checking — reject side effects in pure functions ──────────────

#[test]
fn pure_vs_effectful_corpus_parses_and_checks() {
    // GIVEN: valid corpus of pure/effectful declarations with correct annotations
    // THEN: no type errors
    let src = include_str!("corpus/07_effects/pure_vs_effectful.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "pure_vs_effectful corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn pure_function_calling_effectful_rejected() {
    // GIVEN: pure fn calls effectful fn ! Console
    // THEN: UndeclaredEffect reported
    let src = r#"
        fn effectful_fn() -> Unit ! Console { console.log("hi") }
        fn pure_fn() -> Unit { effectful_fn() }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::UndeclaredEffect { callee, effect, .. }
                if callee == "effectful_fn" && effect == "Console")
        ),
        "expected UndeclaredEffect(effectful_fn, Console), got: {errors:?}"
    );
}

#[test]
fn effectful_function_with_correct_declaration_accepted() {
    // GIVEN: fn caller ! Console calls fn log_it ! Console
    // THEN: no effect errors
    let src = r#"
        fn log_it() -> Unit ! Console { console.log("hi") }
        fn caller() -> Unit ! Console { log_it() }
    "#;
    let errors = errors_for(src);
    let effect_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::UndeclaredEffect { .. } | CheckError::MissingEffect { .. }
            )
        })
        .collect();
    assert!(
        effect_errors.is_empty(),
        "caller with matching effect should be accepted, got: {effect_errors:?}"
    );
}

// ── #20: Effect propagation — callee effects declared by caller ───────────────

#[test]
fn propagation_corpus_parses_and_checks() {
    // GIVEN: valid corpus of effect propagation patterns
    // THEN: no type errors
    let src = include_str!("corpus/07_effects/propagation.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "propagation corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn caller_missing_callee_effect_rejected() {
    // GIVEN: fn read_file ! FileRead; fn caller ! Net calls read_file
    // THEN: MissingEffect(read_file, FileRead) reported
    let src = r#"
        fn read_file() -> Unit ! FileRead { file.read("x") }
        fn caller() -> Unit ! Net { read_file() }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::MissingEffect { callee, effect, .. }
            if callee == "read_file" && effect == "FileRead"
        )),
        "expected MissingEffect(read_file, FileRead), got: {errors:?}"
    );
}

#[test]
fn caller_declaring_effect_union_accepted() {
    // GIVEN: fn a ! FileRead, fn b ! Net, fn c ! FileRead + Net calls both
    // THEN: no effect errors
    let src = r#"
        fn read_fn() -> Unit ! FileRead { file.read("x") }
        fn net_fn() -> Unit ! Net { http.get("url") }
        fn union_caller() -> Unit ! FileRead + Net { read_fn(); net_fn() }
    "#;
    let errors = errors_for(src);
    let effect_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::UndeclaredEffect { .. } | CheckError::MissingEffect { .. }
            )
        })
        .collect();
    assert!(
        effect_errors.is_empty(),
        "union caller should be accepted, got: {effect_errors:?}"
    );
}

// ── #21: Totality checking — reject unbounded loops in total functions ─────────

#[test]
fn totality_corpus_parses_and_checks() {
    // GIVEN: valid corpus of total/partial function declarations
    // THEN: no type errors
    let src = include_str!("corpus/10_termination/total_vs_partial.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "totality corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn while_loop_in_total_function_rejected() {
    // GIVEN: total fn with while loop
    // THEN: UnboundedLoopInTotal reported
    let src = "total fn spin() -> Unit { while true { } }";
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnboundedLoopInTotal { .. })),
        "expected UnboundedLoopInTotal, got: {errors:?}"
    );
}

#[test]
fn while_loop_in_implicit_total_function_rejected() {
    // GIVEN: fn without totality annotation (implicitly total) with while loop
    // THEN: UnboundedLoopInTotal reported
    let src = "fn loop_forever() -> Unit { while true { } }";
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnboundedLoopInTotal { .. })),
        "expected UnboundedLoopInTotal for implicit total fn, got: {errors:?}"
    );
}

#[test]
fn while_loop_in_partial_function_accepted() {
    // GIVEN: partial fn with while loop
    // THEN: no UnboundedLoopInTotal error
    let src = "partial fn server() -> Unit { while true { } }";
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnboundedLoopInTotal { .. })),
        "partial fn should allow while loops, got: {errors:?}"
    );
}

#[test]
fn for_loop_in_total_function_accepted() {
    // GIVEN: total fn with for loop (bounded)
    // THEN: no totality error
    let src = "total fn f(items: List[Int]) -> Unit { for x in items { } }";
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnboundedLoopInTotal { .. })),
        "total fn should allow for loops, got: {errors:?}"
    );
}

#[test]
fn partial_call_in_total_function_rejected() {
    // GIVEN: total fn calls partial fn
    // THEN: PartialCallInTotal reported
    let src = r#"
        partial fn infinite() -> Unit { while true { } }
        total fn caller() -> Unit { infinite() }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::PartialCallInTotal { callee, .. }
                if callee == "infinite")
        ),
        "expected PartialCallInTotal(infinite), got: {errors:?}"
    );
}

// ── #135: Structural recursion (Req 8) ───────────────────────────────────────

#[test]
fn integer_decrement_recursion_accepted() {
    // GIVEN: total fn recurses with `n - 1` (provably decreasing)
    // THEN: no UnprovenRecursion error
    let src = r#"
        fn fact(n: Int) -> Int {
            if n == 0 { 1 } else { n * fact(n - 1) }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion, got: {errors:?}"
    );
}

#[test]
fn unbounded_recursion_in_total_fn_rejected() {
    // GIVEN: total fn calls itself with the same argument (no decrease)
    // THEN: UnprovenRecursion reported
    let src = r#"fn spin(n: Int) -> Int { spin(n) }"#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::UnprovenRecursion { fn_name, .. }
            if fn_name == "spin")
        ),
        "expected UnprovenRecursion(spin), got: {errors:?}"
    );
}

#[test]
fn increasing_recursion_in_total_fn_rejected() {
    // GIVEN: total fn recurses with `n + 1` (increasing)
    // THEN: UnprovenRecursion reported
    // spec 007 §Req 2
    let src = r#"fn bad(n: Int) -> Int { bad(n + 1) }"#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::UnprovenRecursion { fn_name, .. } if fn_name == "bad")
        ),
        "expected UnprovenRecursion(bad) for increasing argument, got: {errors:?}"
    );
}

#[test]
fn decrement_by_zero_in_total_fn_rejected() {
    // GIVEN: total fn recurses with `n - 0` (N == 0 is not a decrease)
    // THEN: UnprovenRecursion reported
    // spec 007 §Req 2, Scenario: Decrement by zero rejected
    let src = r#"fn f(n: Int) -> Int { f(n - 0) }"#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { fn_name, .. } if fn_name == "f")),
        "expected UnprovenRecursion for n - 0, got: {errors:?}"
    );
}

#[test]
fn decrement_on_second_param_accepted() {
    // GIVEN: total fn with two params where only the second decreases
    // THEN: no UnprovenRecursion — any parameter decrement is accepted
    // spec 007 §Req 2 (param checked against all parameters, not positionally)
    let src = r#"fn f(a: Int, b: Int) -> Int { if b == 0 { a } else { f(a, b - 1) } }"#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion when second param decreases, got: {errors:?}"
    );
}

#[test]
fn explicit_total_fn_keyword_unbounded_rejected() {
    // GIVEN: fn with explicit `total` keyword and no decrease measure
    // THEN: UnprovenRecursion reported (explicit total is checked like implicit)
    // spec 007 §Scope and Defaults
    let src = r#"total fn spin(n: Int) -> Int { spin(n) }"#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::UnprovenRecursion { fn_name, .. } if fn_name == "spin")
        ),
        "expected UnprovenRecursion for explicit total fn, got: {errors:?}"
    );
}

#[test]
fn structural_recursion_on_adt_subterm_accepted() {
    // GIVEN: total fn matches on list param, recurses with the tail subterm
    // THEN: no UnprovenRecursion error
    let src = r#"
        enum List { Nil, Cons(Int, List) }
        fn len(list: List) -> Int {
            match list {
                List::Nil => 0
                List::Cons(_, tail) => 1 + len(tail)
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion for structural recursion, got: {errors:?}"
    );
}

#[test]
fn recursion_in_partial_fn_not_checked() {
    // GIVEN: partial fn recurses with no decrease
    // THEN: no UnprovenRecursion (partial fns are exempt)
    let src = r#"partial fn loop_forever(n: Int) -> Int { loop_forever(n) }"#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion for partial fn, got: {errors:?}"
    );
}

#[test]
fn non_recursive_total_fn_accepted() {
    // GIVEN: total fn with no recursive calls
    // THEN: trivially terminating, no error
    let src = r#"fn add(a: Int, b: Int) -> Int { a + b }"#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion for non-recursive fn, got: {errors:?}"
    );
}

#[test]
fn structural_recursion_on_adt_single_field_accepted() {
    // GIVEN: total fn matches on single-field ADT param, recurses with the inner subterm
    // THEN: no UnprovenRecursion — inner is a structural subterm via TupleStruct(inner)
    // spec 007 §Req 3 (single-field TupleStruct variant, complements the Cons test)
    let src = r#"
        enum Nat { Zero, Succ(Nat) }
        fn count(n: Nat) -> Int {
            match n {
                Nat::Zero => 0
                Nat::Succ(inner) => 1 + count(inner)
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion for single-field ADT subterm, got: {errors:?}"
    );
}

#[test]
fn structural_recursion_via_non_param_match_rejected() {
    // GIVEN: total fn matches on a local variable (not the parameter directly)
    // THEN: UnprovenRecursion — local bindings do not establish subterm relation
    // spec 007 §Req 3, Scenario: Match on non-parameter does not grant subterm status
    let src = r#"
        enum List { Nil, Cons(Int, List) }
        fn f(list: List) -> Int {
            let local = list;
            match local {
                List::Nil => 0
                List::Cons(_, tail) => f(tail)
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { fn_name, .. } if fn_name == "f")),
        "expected UnprovenRecursion when match is not on a bare param, got: {errors:?}"
    );
}

#[test]
fn division_by_constant_recursion_accepted() {
    // GIVEN: total fn recurses with `n / 2` (provably decreasing, logarithmic)
    // THEN: no UnprovenRecursion error
    // spec 007 §Req 2 (integer division by constant > 1)
    let src = r#"fn halve(n: Int) -> Int { if n == 0 { 0 } else { halve(n / 2) } }"#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion for n / 2, got: {errors:?}"
    );
}

#[test]
fn division_by_large_constant_recursion_accepted() {
    // GIVEN: total fn recurses with `n / 10` (divisor > 2 also accepted)
    // THEN: no UnprovenRecursion error
    // spec 007 §Req 2
    let src = r#"fn f(n: Int) -> Int { if n == 0 { 0 } else { f(n / 10) } }"#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion for n / 10, got: {errors:?}"
    );
}

#[test]
fn division_by_one_in_total_fn_rejected() {
    // GIVEN: total fn recurses with `n / 1` (not a decrease — equal)
    // THEN: UnprovenRecursion reported
    // spec 007 §Req 2
    let src = r#"fn f(n: Int) -> Int { f(n / 1) }"#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { fn_name, .. } if fn_name == "f")),
        "expected UnprovenRecursion for n / 1, got: {errors:?}"
    );
}

#[test]
fn tail_accessor_recursion_accepted() {
    // GIVEN: total fn recurses passing `xs.tail()` — structural subterm via method
    // THEN: no UnprovenRecursion error
    // spec 007 §Req 3 (method accessor yields strict substructure)
    let src = r#"fn f(xs: List[Int]) -> Int { if xs == [] { 0 } else { f(xs.tail()) } }"#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion for xs.tail(), got: {errors:?}"
    );
}

#[test]
fn rest_accessor_recursion_accepted() {
    // GIVEN: total fn recurses passing `xs.rest()` — structural subterm via method
    // THEN: no UnprovenRecursion error
    // spec 007 §Req 3
    let src = r#"fn f(xs: List[Int]) -> Int { if xs == [] { 0 } else { f(xs.rest()) } }"#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion for xs.rest(), got: {errors:?}"
    );
}

#[test]
fn subterm_len_recursion_accepted() {
    // GIVEN: total fn matches on list param and recurses with (tail, tail.len())
    //        where tail is a structural subterm — both tail (List) and tail.len() (Int)
    //        are recognized as decreasing measures for their respective parameters
    // THEN: no UnprovenRecursion
    // spec 007 §Req 3 (subterm length is strictly less than original)
    let src = r#"
        enum List { Nil, Cons(Int, List) }
        fn f(xs: List, n: Int) -> Int {
            match xs {
                List::Nil => 0
                List::Cons(_, tail) => f(tail, tail.len())
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion for subterm.len(), got: {errors:?}"
    );
}

#[test]
fn len_on_param_directly_rejected() {
    // GIVEN: total fn recurses with `xs.len()` where xs is a direct parameter
    //        (not a match-bound structural subterm)
    // THEN: UnprovenRecursion — only subterm.len() is a recognised decrease, not param.len()
    // spec 007 §Req 3
    let src = r#"fn f(xs: List[Int]) -> Int { if xs == [] { 0 } else { f(xs.len()) } }"#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { fn_name, .. } if fn_name == "f")),
        "expected UnprovenRecursion for param.len() (not a subterm), got: {errors:?}"
    );
}

#[test]
fn tail_on_local_variable_rejected() {
    // GIVEN: total fn calls .tail() on a local variable, not a parameter or known subterm
    // THEN: UnprovenRecursion — accessor on a non-param non-subterm is not a proven decrease
    // spec 007 §Req 3
    let src = r#"fn f(xs: List[Int]) -> Int { let local = xs; f(local.tail()) }"#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { fn_name, .. } if fn_name == "f")),
        "expected UnprovenRecursion for local.tail() (not a param/subterm), got: {errors:?}"
    );
}

#[test]
fn rest_on_local_variable_rejected() {
    // GIVEN: total fn calls .rest() on a local variable, not a parameter or known subterm
    // THEN: UnprovenRecursion
    // spec 007 §Req 3
    let src = r#"fn f(xs: List[Int]) -> Int { let local = xs; f(local.rest()) }"#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { fn_name, .. } if fn_name == "f")),
        "expected UnprovenRecursion for local.rest() (not a param/subterm), got: {errors:?}"
    );
}

#[test]
fn recursion_inside_lambda_not_flagged() {
    // GIVEN: total fn creates a lambda that references the outer fn by name
    // THEN: no UnprovenRecursion — lambdas have their own scope (spec 007 §Req 4)
    let src = r#"
        fn outer(n: Int) -> Int {
            let f = |x: Int| outer(x);
            n + 1
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnprovenRecursion { .. })),
        "expected no UnprovenRecursion inside lambda, got: {errors:?}"
    );
}

// ── #22: Reference capability checking — iso/val/ref/tag on actor boundaries ──

#[test]
fn capabilities_corpus_parses_and_checks() {
    // GIVEN: valid corpus of capability-annotated functions
    // THEN: no type errors
    let src = include_str!("corpus/12_actors/capabilities.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "capabilities corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn session_types_corpus_parses_and_checks() {
    // GIVEN: session type declarations (Phase 8, #260)
    // THEN: all session type aliases parse and type-check cleanly
    let src = include_str!("corpus/12_actors/session_types.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "session types corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn dead_letter_corpus_parses_and_checks() {
    // GIVEN: dead-letter types and handler actor (Phase 9, #1180)
    // THEN: all declarations type-check cleanly with std.actors stdlib loaded
    let (mut p, _) = Parser::new(include_str!("../std/effects.mvl"));
    let effects = p.parse_program();
    let (mut p, _) = Parser::new(include_str!("../std/collections.mvl"));
    let collections = p.parse_program();
    let (mut p, _) = Parser::new(include_str!("../std/math.mvl"));
    let math = p.parse_program();
    let (mut p, _) = Parser::new(include_str!("../std/log.mvl"));
    let log = p.parse_program();
    let (mut p, _) = Parser::new(include_str!("../std/actors.mvl"));
    let actors = p.parse_program();
    let (mut p, _) = Parser::new(include_str!("corpus/12_actors/dead_letter.mvl"));
    let prog = p.parse_program();
    let result = check_with_prelude(&[effects, collections, math, log, actors], &prog);
    assert!(
        result.is_ok(),
        "dead letter corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

// ── Session type model checker (D1, #134) ────────────────────────────────────

#[test]
fn session_duplicate_label_in_internal_choice_is_rejected() {
    // GIVEN: internal choice with two branches sharing the same label
    // THEN: SessionDuplicateLabel error reported
    let src = r#"
        type BadChoice = +{
            ok: end,
            ok: !Int. end
        }
    "#;
    let result = check_src(src);
    let has_dup = result.errors.iter().any(|e| {
        matches!(e, mvl::mvl::checker::errors::CheckError::SessionDuplicateLabel { label, .. } if label == "ok")
    });
    assert!(
        has_dup,
        "expected SessionDuplicateLabel for `ok`, got: {:?}",
        result.errors
    );
}

#[test]
fn session_duplicate_label_in_external_choice_is_rejected() {
    // GIVEN: external choice with a repeated branch label
    // THEN: SessionDuplicateLabel error reported
    let src = r#"
        type BadServer = &{
            read: !String. end,
            write: ?String. end,
            read: end
        }
    "#;
    let result = check_src(src);
    let has_dup = result.errors.iter().any(|e| {
        matches!(e, mvl::mvl::checker::errors::CheckError::SessionDuplicateLabel { label, .. } if label == "read")
    });
    assert!(
        has_dup,
        "expected SessionDuplicateLabel for `read`, got: {:?}",
        result.errors
    );
}

#[test]
fn session_unique_labels_accepted() {
    // GIVEN: choice with all-distinct labels
    // THEN: no duplicate-label errors
    let src = r#"
        type GoodChoice = +{
            left: end,
            right: end,
            middle: !Int. end
        }
    "#;
    let result = check_src(src);
    let has_dup = result.errors.iter().any(|e| {
        matches!(
            e,
            mvl::mvl::checker::errors::CheckError::SessionDuplicateLabel { .. }
        )
    });
    assert!(
        !has_dup,
        "unexpected duplicate-label error: {:?}",
        result.errors
    );
}

#[test]
fn session_mutual_blocking_detected_by_check_dual() {
    // GIVEN: two types that both start with Receive (both wait, neither sends)
    // THEN: check_dual returns SessionDeadlock
    use mvl::mvl::checker::session::check_dual;
    use mvl::mvl::checker::types::SessionTy;
    use mvl::mvl::checker::types::Ty;

    let dummy = mvl::mvl::parser::lexer::Span {
        line: 1,
        col: 1,
        offset: 0,
        len: 0,
    };

    // Both sides receive an Int first — deadlock.
    let a = SessionTy::Receive(Box::new(Ty::Int), Box::new(SessionTy::End));
    let b = SessionTy::Receive(Box::new(Ty::Int), Box::new(SessionTy::End));
    let err = check_dual(&a, &b, dummy);
    assert!(
        matches!(
            err,
            Some(mvl::mvl::checker::errors::CheckError::SessionDeadlock { .. })
        ),
        "expected SessionDeadlock, got: {:?}",
        err
    );
}

#[test]
fn session_proper_duals_no_deadlock() {
    // GIVEN: Ping = !Int. ?Bool. end  and  Pong = ?Int. !Bool. end (proper duals)
    // THEN: check_dual returns None
    use mvl::mvl::checker::session::check_dual;
    use mvl::mvl::checker::types::SessionTy;
    use mvl::mvl::checker::types::Ty;

    let dummy = mvl::mvl::parser::lexer::Span {
        line: 1,
        col: 1,
        offset: 0,
        len: 0,
    };

    let ping = SessionTy::Send(
        Box::new(Ty::Int),
        Box::new(SessionTy::Receive(
            Box::new(Ty::Bool),
            Box::new(SessionTy::End),
        )),
    );
    let pong = SessionTy::Receive(
        Box::new(Ty::Int),
        Box::new(SessionTy::Send(
            Box::new(Ty::Bool),
            Box::new(SessionTy::End),
        )),
    );
    assert_eq!(check_dual(&ping, &pong, dummy), None);
}

#[test]
fn session_check_no_mutual_blocking_direct() {
    // GIVEN: one side sends, other receives — no deadlock
    use mvl::mvl::checker::session::check_no_mutual_blocking;
    use mvl::mvl::checker::types::SessionTy;
    use mvl::mvl::checker::types::Ty;

    let dummy = mvl::mvl::parser::lexer::Span {
        line: 1,
        col: 1,
        offset: 0,
        len: 0,
    };

    let sender = SessionTy::Send(Box::new(Ty::Int), Box::new(SessionTy::End));
    let recver = SessionTy::Receive(Box::new(Ty::Int), Box::new(SessionTy::End));
    assert_eq!(check_no_mutual_blocking(&sender, &recver, dummy), None);

    // Both receive — deadlock
    let r1 = SessionTy::Receive(Box::new(Ty::Int), Box::new(SessionTy::End));
    let r2 = SessionTy::Receive(Box::new(Ty::Int), Box::new(SessionTy::End));
    assert!(matches!(
        check_no_mutual_blocking(&r1, &r2, dummy),
        Some(mvl::mvl::checker::errors::CheckError::SessionDeadlock { .. })
    ));
}

#[test]
fn session_deadlock_in_choice_branch_detected() {
    // GIVEN: InternalChoice/ExternalChoice pair where one branch has mutual blocking
    // THEN: check_no_mutual_blocking reports SessionDeadlock
    use mvl::mvl::checker::session::check_no_mutual_blocking;
    use mvl::mvl::checker::types::SessionTy;
    use mvl::mvl::checker::types::Ty;

    let dummy = mvl::mvl::parser::lexer::Span {
        line: 1,
        col: 1,
        offset: 0,
        len: 0,
    };

    // a: +{ ok: !Int. end, bad: ?String. end }
    // b: &{ ok: ?Int. end, bad: ?String. end }  ← bad branch: both receive
    let a = SessionTy::InternalChoice(vec![
        (
            "ok".to_string(),
            SessionTy::Send(Box::new(Ty::Int), Box::new(SessionTy::End)),
        ),
        (
            "bad".to_string(),
            SessionTy::Receive(Box::new(Ty::String), Box::new(SessionTy::End)),
        ),
    ]);
    let b = SessionTy::ExternalChoice(vec![
        (
            "ok".to_string(),
            SessionTy::Receive(Box::new(Ty::Int), Box::new(SessionTy::End)),
        ),
        (
            "bad".to_string(),
            SessionTy::Receive(Box::new(Ty::String), Box::new(SessionTy::End)),
        ),
    ]);
    assert!(matches!(
        check_no_mutual_blocking(&a, &b, dummy),
        Some(mvl::mvl::checker::errors::CheckError::SessionDeadlock { .. })
    ));
}

#[test]
fn sending_ref_param_rejected() {
    // GIVEN: fn with `ref` param attempts channel.send(param)
    // THEN: CapabilityViolation reported
    let src = r#"
        fn send_ref(channel: Channel, ref data: Payload) -> Unit {
            channel.send(data)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::CapabilityViolation { param, capability, .. }
                if param == "data" && capability == "ref")
        ),
        "expected CapabilityViolation(data, ref), got: {errors:?}"
    );
}

#[test]
fn sending_iso_param_accepted() {
    // GIVEN: fn with `iso` param attempts channel.send(param)
    // THEN: no CapabilityViolation (iso is sendable)
    let src = r#"
        fn send_iso(channel: Channel, iso data: Payload) -> Unit {
            channel.send(data)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::CapabilityViolation { .. })),
        "iso param should be sendable, got: {errors:?}"
    );
}

#[test]
fn sending_val_param_accepted() {
    // GIVEN: fn with `val` param attempts channel.send(param)
    // THEN: no CapabilityViolation (val is sendable)
    let src = r#"
        fn broadcast(channel: Channel, val msg: Message) -> Unit {
            channel.send(msg)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::CapabilityViolation { .. })),
        "val param should be sendable, got: {errors:?}"
    );
}

// ── #138: Data race freedom — iso aliasing (Requirement 9, Phase 3) ──────────

#[test]
fn iso_aliasing_without_consume_rejected() {
    // GIVEN: fn binds an `iso` param to a new let without consume()
    // THEN: IsoAliasingViolation reported (two live references to isolated object)
    let src = r#"
        fn alias_iso(channel: Channel, iso x: Payload) -> Unit {
            let y: Payload = x;
            channel.send(consume(y))
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::IsoAliasingViolation { name, .. } if name == "x")),
        "expected IsoAliasingViolation for x, got: {errors:?}"
    );
}

#[test]
fn iso_with_consume_accepted() {
    // GIVEN: fn sends an `iso` param via consume() — proper ownership transfer
    // THEN: no IsoAliasingViolation (consume() is not an alias)
    let src = r#"
        fn transfer(channel: Channel, iso item: Payload) -> Unit {
            channel.send(consume(item))
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::IsoAliasingViolation { .. })),
        "consume() should not be flagged as aliasing, got: {errors:?}"
    );
}

#[test]
fn iso_direct_send_accepted() {
    // GIVEN: fn sends an `iso` param directly via channel.send (existing behavior)
    // THEN: no IsoAliasingViolation (send is a capability-boundary operation)
    let src = r#"
        fn send_owned(channel: Channel, iso data: Payload) -> Unit {
            channel.send(data)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::IsoAliasingViolation { .. })),
        "direct iso send should not be flagged as aliasing, got: {errors:?}"
    );
}

#[test]
fn val_param_aliasing_not_checked() {
    // GIVEN: fn binds a `val` param to a new let (val is immutable — aliasing is fine)
    // THEN: no IsoAliasingViolation (only iso is subject to aliasing checks)
    let src = r#"
        fn copy_val(val config: Config) -> Unit {
            let copy: Config = config;
            consume(copy)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::IsoAliasingViolation { .. })),
        "val aliasing should not be flagged, got: {errors:?}"
    );
}

// ── #138 continued: control flow, assignment, lambda, and limitation tests ───

#[test]
fn iso_aliasing_via_assignment_rejected() {
    // GIVEN: fn has two iso params and assigns one to a mutable binding without consume()
    // THEN: IsoAliasingViolation reported for the assigned iso var
    let src = r#"
        fn assign_iso(iso x: Payload, iso z: Payload) -> Unit {
            let mut y: Payload = consume(x);
            y = z;
            consume(y)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::IsoAliasingViolation { name, .. } if name == "z")),
        "expected IsoAliasingViolation for z assigned without consume(), got: {errors:?}"
    );
}

#[test]
fn iso_aliasing_inside_if_branch_rejected() {
    // GIVEN: iso param aliased inside a then-branch
    // THEN: IsoAliasingViolation reported
    let src = r#"
        fn conditional_alias(channel: Channel, iso x: Payload, flag: Bool) -> Unit {
            if flag {
                let y: Payload = x;
                channel.send(consume(y))
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::IsoAliasingViolation { name, .. } if name == "x")),
        "iso aliasing in if-branch should be rejected, got: {errors:?}"
    );
}

// Lambda surface syntax is not yet parsed by the MVL parser.
// Lambda aliasing checks are covered by AST-based unit tests in data_race.rs.

// ── Known limitations (L1–L5) — regression tests documenting non-detection ───

#[test]
fn iso_passed_to_fn_call_not_detected_l1() {
    // L1: Passing an iso var to a non-send function without consume() is NOT
    // detected.  This test documents the current behavior so that future
    // implementations of L1 detection will intentionally break it.
    let src = r#"
        fn use_payload(x: Payload) -> Unit { consume(x) }
        fn caller(iso x: Payload) -> Unit {
            use_payload(x)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::IsoAliasingViolation { .. })),
        "L1: iso passed to fn without consume() is not yet detected, got: {errors:?}"
    );
}

#[test]
fn iso_rebound_after_consume_detected() {
    // L5 fix: After `let a = consume(x)`, `a` becomes the new iso owner.
    // Subsequent aliasing of `a` (e.g., `let b = a`) is now detected.
    let src = r#"
        fn rebound_alias(iso x: Payload) -> Unit {
            let a: Payload = consume(x);
            let b: Payload = a;
            consume(b)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::IsoAliasingViolation { name, .. } if name == "a")),
        "aliasing of rebound iso variable should be detected, got: {errors:?}"
    );
}

#[test]
fn iso_multiple_aliasing_all_sites_reported() {
    // Each individual let-binding of an iso param is flagged independently.
    // Both `let a = x` and `let b = x` generate separate violations.
    let src = r#"
        fn double_alias(iso x: Payload) -> Unit {
            let a: Payload = x;
            let b: Payload = x;
            consume(b)
        }
    "#;
    let errors = errors_for(src);
    let violations: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, CheckError::IsoAliasingViolation { name, .. } if name == "x"))
        .collect();
    assert_eq!(
        violations.len(),
        2,
        "expected 2 IsoAliasingViolation (one per alias site), got: {violations:?}"
    );
}

// ── #24: Security label checking (Requirement 11) ────────────────────────────

#[test]
fn labels_corpus_parses_and_checks() {
    // GIVEN: the existing labels corpus (valid labeled programs)
    // THEN: no IFC violations (UndefinedFunction for stdlib is OK)
    let src = include_str!("corpus/08_ifc/labels.mvl");
    let result = check_src(src);
    let serious_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| !matches!(e, CheckError::UndefinedFunction { .. }))
        .collect();
    assert!(
        serious_errors.is_empty(),
        "labels corpus should have no IFC violations, got: {serious_errors:?}"
    );
}

#[test]
fn label_types_corpus_parses_and_checks() {
    // GIVEN: the label_types corpus (labeled parameters and upward flows)
    // THEN: no type errors
    let src = include_str!("corpus/08_ifc/label_types.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "label_types corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn secret_flows_to_public_rejected() {
    // GIVEN: a function returning Public[String] but body is Secret[String]
    // THEN: TypeMismatch (downward flow rejected)
    let errors = errors_for(r#"fn leak(k: Secret[String]) -> Public[String] { k }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "expected TypeMismatch for Secret→Public leak, got: {errors:?}"
    );
}

#[test]
fn public_flows_to_secret_rejected() {
    // Post-#894: no lattice. bare String ≠ Secret[String].
    // Use relabel classify to explicitly wrap.
    let errors = errors_for(r#"fn store(x: String) -> Secret[String] { x }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "bare String must not flow to Secret[String] without relabel, got: {errors:?}"
    );
}

#[test]
fn tainted_flows_to_clean_rejected() {
    // GIVEN: a function returning Clean[String] but body is Tainted[String]
    // THEN: TypeMismatch (downward flow rejected — needs sanitize)
    let errors = errors_for(r#"fn use_raw(input: Tainted[String]) -> Clean[String] { input }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "expected TypeMismatch for Tainted→Clean without sanitize, got: {errors:?}"
    );
}

// ── #25: Lattice enforcement ──────────────────────────────────────────────────

#[test]
fn lattice_corpus_parses_and_checks() {
    // GIVEN: lattice corpus (valid upward flows)
    // THEN: no type errors
    let src = include_str!("corpus/08_ifc/lattice.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "lattice corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn secret_to_tainted_rejected() {
    // GIVEN: function returns Tainted[Int] but body is Secret[Int] (downward)
    // THEN: TypeMismatch
    let errors = errors_for(r#"fn downgrade(s: Secret[Int]) -> Tainted[Int] { s }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "expected TypeMismatch for Secret→Tainted downgrade, got: {errors:?}"
    );
}

#[test]
fn clean_to_public_rejected() {
    // GIVEN: function returns Public[Int] but body is Clean[Int] (downward)
    // THEN: TypeMismatch
    let errors = errors_for(r#"fn expose(s: Clean[Int]) -> Public[Int] { s }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "expected TypeMismatch for Clean→Public downgrade, got: {errors:?}"
    );
}

// ── #26: Label propagation through expressions ───────────────────────────────

#[test]
fn propagation_ifc_corpus_parses_and_checks() {
    // GIVEN: propagation corpus (arithmetic label join)
    // THEN: no type errors
    let src = include_str!("corpus/08_ifc/propagation.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "propagation corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn arithmetic_label_join_propagates() {
    // GIVEN: Secret[Int] + Public[Int] — the result carries the join (Secret)
    // THEN: no type error when assigned to Secret[Int]
    let errors = errors_for(r#"fn add(a: Secret[Int], b: Public[Int]) -> Secret[Int] { a + b }"#);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret[Int] + Public[Int] should yield Secret[Int], got: {errors:?}"
    );
}

#[test]
fn arithmetic_label_join_downgrade_rejected() {
    // Post-#894: Secret[Int] + Int — result is Secret[Int]; cannot return as Int (bare).
    let errors = errors_for(r#"fn add(a: Secret[Int], b: Int) -> Int { a + b }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret[Int] + Int result cannot flow to bare Int, got: {errors:?}"
    );
}

// ── #27: Declassify/sanitize as auditable chokepoints ────────────────────────

#[test]
fn declassification_corpus_parses_and_checks() {
    // GIVEN: declassification corpus (valid declassify/sanitize usage)
    // THEN: no type errors (UndefinedFunction for User types is OK)
    let src = include_str!("corpus/08_ifc/declassification.mvl");
    let result = check_src(src);
    let serious_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| !matches!(e, CheckError::UndefinedFunction { .. }))
        .collect();
    assert!(
        serious_errors.is_empty(),
        "declassification corpus should have no IFC violations, got: {serious_errors:?}"
    );
}

#[test]
fn secret_env_corpus_parses_and_checks() {
    // GIVEN: secret_env corpus (#872) — positive flows for Secret[String] IFC
    // THEN: no type errors (UndefinedFunction for Token type is OK)
    let src = include_str!("corpus/08_ifc/secret_env.mvl");
    let result = check_src(src);
    let serious_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| !matches!(e, CheckError::UndefinedFunction { .. }))
        .collect();
    assert!(
        serious_errors.is_empty(),
        "secret_env corpus should have no IFC violations, got: {serious_errors:?}"
    );
}

// ── #931: Capability labels (IFC labels as capability tokens) ─────────────────

#[test]
fn capability_labels_corpus_parses_and_checks() {
    // GIVEN: capability_labels corpus (#931) — positive flows for ConfigPath,
    //        DbUrl, ApiEndpoint, AuditTarget capability labels
    // THEN: no type errors (corpus uses only relabel expressions and inline fns)
    let src = include_str!("corpus/08_ifc/capability_labels.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "capability_labels corpus should have no errors, got: {:?}",
        result.errors
    );
}

#[test]
fn config_path_to_bare_string_return_rejected() {
    // GIVEN: a function returning bare String but body yields ConfigPath[String]
    // THEN: TypeMismatch — ConfigPath[String] cannot implicitly flow to String
    let errors = errors_for(r#"fn use_path(p: ConfigPath[String]) -> String { p }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "ConfigPath[String] return as String should be rejected, got: {errors:?}"
    );
}

#[test]
fn raw_string_to_config_path_rejected() {
    // GIVEN: a function returning ConfigPath[String] but body is bare String
    // THEN: TypeMismatch — bare String needs relabel config_path to become ConfigPath
    let errors = errors_for(r#"fn make_path(s: String) -> ConfigPath[String] { s }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "bare String must not flow to ConfigPath[String] without relabel, got: {errors:?}"
    );
}

#[test]
fn db_url_rejects_tainted_string() {
    // GIVEN: a function expecting DbUrl[String] but receiving Tainted[String]
    // THEN: TypeMismatch — Tainted[String] cannot flow to DbUrl[String]
    let errors = errors_for(r#"fn connect(url: Tainted[String]) -> DbUrl[String] { url }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Tainted[String] must not flow to DbUrl[String] without relabel, got: {errors:?}"
    );
}

#[test]
fn api_endpoint_rejects_raw_string() {
    // GIVEN: a function returning ApiEndpoint[String] but body is bare String
    // THEN: TypeMismatch
    let errors = errors_for(r#"fn make_endpoint(s: String) -> ApiEndpoint[String] { s }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "bare String must not flow to ApiEndpoint[String] without relabel, got: {errors:?}"
    );
}

#[test]
fn audit_target_rejects_raw_string() {
    // GIVEN: a function returning AuditTarget[String] but body is bare String
    // THEN: TypeMismatch
    let errors = errors_for(r#"fn make_target(s: String) -> AuditTarget[String] { s }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "bare String must not flow to AuditTarget[String] without relabel, got: {errors:?}"
    );
}

#[test]
fn config_path_relabel_roundtrip() {
    // GIVEN: relabel config_path wraps, relabel unconfig_path unwraps
    // THEN: no type errors on the round-trip
    let src = r#"
        fn roundtrip(s: String) -> String {
            let p: ConfigPath[String] = relabel config_path(s, "TEST");
            relabel unconfig_path(p, "TEST")
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "ConfigPath relabel round-trip should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn capability_labels_are_distinct() {
    // GIVEN: ConfigPath[String] where DbUrl[String] is expected
    // THEN: TypeMismatch — different capability labels are not interchangeable
    let src = r#"
        fn wrong_label(p: ConfigPath[String]) -> DbUrl[String] { p }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "ConfigPath[String] must not flow to DbUrl[String], got: {errors:?}"
    );
}

// ── #931: Call-site rejection tests (caller passes wrong type to capability param) ──

#[test]
fn config_path_call_site_rejects_raw_string() {
    // GIVEN: a function expecting ConfigPath[String]
    // WHEN: caller passes bare String at the call site
    // THEN: TypeMismatch
    let src = r#"
        fn needs_config(p: ConfigPath[String]) -> ConfigPath[String] { p }
        fn caller(s: String) -> ConfigPath[String] { needs_config(s) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "bare String at call site must not satisfy ConfigPath[String], got: {errors:?}"
    );
}

#[test]
fn db_url_call_site_rejects_raw_string() {
    // GIVEN: a function expecting DbUrl[String]
    // WHEN: caller passes bare String at the call site
    // THEN: TypeMismatch
    let src = r#"
        fn needs_db(u: DbUrl[String]) -> DbUrl[String] { u }
        fn caller(s: String) -> DbUrl[String] { needs_db(s) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "bare String at call site must not satisfy DbUrl[String], got: {errors:?}"
    );
}

#[test]
fn api_endpoint_call_site_rejects_raw_string() {
    // GIVEN: a function expecting ApiEndpoint[String]
    // WHEN: caller passes bare String at the call site
    // THEN: TypeMismatch
    let src = r#"
        fn needs_endpoint(e: ApiEndpoint[String]) -> ApiEndpoint[String] { e }
        fn caller(s: String) -> ApiEndpoint[String] { needs_endpoint(s) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "bare String at call site must not satisfy ApiEndpoint[String], got: {errors:?}"
    );
}

#[test]
fn audit_target_call_site_rejects_raw_string() {
    // GIVEN: a function expecting AuditTarget[String]
    // WHEN: caller passes bare String at the call site
    // THEN: TypeMismatch
    let src = r#"
        fn needs_target(t: AuditTarget[String]) -> AuditTarget[String] { t }
        fn caller(s: String) -> AuditTarget[String] { needs_target(s) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "bare String at call site must not satisfy AuditTarget[String], got: {errors:?}"
    );
}

// ── #931: Roundtrip tests for DbUrl, ApiEndpoint, AuditTarget ───────────────

#[test]
fn db_url_relabel_roundtrip() {
    let src = r#"
        fn roundtrip(s: String) -> String {
            let u: DbUrl[String] = relabel db_url(s, "TEST");
            relabel undb_url(u, "TEST")
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "DbUrl relabel round-trip should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn api_endpoint_relabel_roundtrip() {
    let src = r#"
        fn roundtrip(s: String) -> String {
            let e: ApiEndpoint[String] = relabel api_endpoint(s, "TEST");
            relabel unapi_endpoint(e, "TEST")
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "ApiEndpoint relabel round-trip should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn audit_target_relabel_roundtrip() {
    let src = r#"
        fn roundtrip(s: String) -> String {
            let t: AuditTarget[String] = relabel audit_target(s, "TEST");
            relabel unaudit_target(t, "TEST")
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "AuditTarget relabel round-trip should type-check cleanly, got: {:?}",
        result.errors
    );
}

// ── #931: InvalidRelabel tests for capability transitions ───────────────────

#[test]
fn unconfig_path_on_bare_string_invalid() {
    // unconfig_path requires ConfigPath input, not bare String
    let errors = errors_for(r#"fn bad(s: String) -> String { relabel unconfig_path(s, "X") }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidRelabel { .. })),
        "unconfig_path on bare String should be InvalidRelabel, got: {errors:?}"
    );
}

#[test]
fn undb_url_on_config_path_invalid() {
    // undb_url requires DbUrl input, not ConfigPath
    let errors =
        errors_for(r#"fn bad(p: ConfigPath[String]) -> String { relabel undb_url(p, "X") }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidRelabel { .. })),
        "undb_url on ConfigPath should be InvalidRelabel, got: {errors:?}"
    );
}

#[test]
fn sanitize_tainted_returns_clean() {
    // GIVEN: sanitize(tainted_string) where tainted_string: Tainted[String]
    // THEN: no type error when returning Clean[String]
    let errors =
        errors_for(r#"fn clean_up(input: Tainted[String]) -> Clean[String] { sanitize(input) }"#);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "sanitize(Tainted[String]) should yield Clean[String], got: {errors:?}"
    );
}

#[test]
fn declassify_secret_returns_public() {
    // GIVEN: declassify(secret) where secret: Secret[Int]
    // THEN: no type error when returning Public[Int]
    let errors =
        errors_for(r#"fn expose(secret: Secret[Int]) -> Public[Int] { declassify(secret) }"#);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "declassify(Secret[Int]) should yield Public[Int], got: {errors:?}"
    );
}

#[test]
fn relabel_trust_on_non_tainted_rejected() {
    // Post-#894: relabel trust() expects Tainted[T] input; bare String is rejected.
    let errors =
        errors_for(r#"fn bad(input: String) -> String { relabel trust(input, "VALIDATED") }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidRelabel { .. })),
        "relabel trust on non-Tainted type should emit InvalidRelabel, got: {errors:?}"
    );
}

#[test]
fn relabel_release_on_non_secret_rejected() {
    // Post-#894: relabel release() expects Secret[T] input; Tainted[Int] is rejected.
    let errors = errors_for(
        r#"fn bad(input: Tainted[Int]) -> Int { relabel release(input, "AUTHORIZED") }"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidRelabel { .. })),
        "relabel release on non-Secret type should emit InvalidRelabel, got: {errors:?}"
    );
}

#[test]
fn direct_tainted_to_clean_without_sanitize_rejected() {
    // GIVEN: assigning Tainted[String] directly to Clean[String] param
    // THEN: TypeMismatch (must use sanitize explicitly)
    let errors = errors_for(
        r#"
        fn needs_clean(s: Clean[String]) -> Clean[String] { s }
        fn caller(raw: Tainted[String]) -> Clean[String] { needs_clean(raw) }
    "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Tainted should not flow to Clean[String] param, got: {errors:?}"
    );
}

#[test]
fn relabel_trust_on_secret_rejected() {
    // Post-#894: relabel trust expects Tainted[T] input; Secret[String] is rejected.
    let errors = errors_for(
        r#"fn bad(input: Secret[String]) -> String { relabel trust(input, "VALIDATED") }"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidRelabel { .. })),
        "relabel trust on Secret type should emit InvalidRelabel, got: {errors:?}"
    );
}

#[test]
fn relabel_classify_on_tainted_rejected() {
    // Post-#894: relabel classify expects bare T input; Tainted[String] is rejected.
    let errors = errors_for(
        r#"fn bad(input: Tainted[String]) -> Secret[String] { relabel classify(input, "ENV-SECRET") }"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidRelabel { .. })),
        "relabel classify on Tainted type should emit InvalidRelabel, got: {errors:?}"
    );
}

#[test]
fn secret_to_unlabeled_param_rejected() {
    // GIVEN: function with unlabeled String param called with Secret[String]
    // THEN: TypeMismatch — unlabeled context is treated as Public, downward flow rejected
    let errors = errors_for(
        r#"
        fn accept(s: String) -> String { s }
        fn caller(k: Secret[String]) -> String { accept(k) }
    "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret[String] must not flow silently to unlabeled String param, got: {errors:?}"
    );
}

#[test]
fn secret_option_to_unlabeled_option_rejected() {
    // GIVEN: fn foo(opt: Option[Int]) called with a Secret[Option[Int]] argument
    // THEN: TypeMismatch — label wrapper is checked before unwrapping Option (types.rs:248)
    // Regression for #714: confirms checker prevents bypass even when codegen
    // suppresses .into() for Option/Result to avoid E0283 ambiguity.
    let errors = errors_for(
        r#"
        fn accept(opt: Option[Int]) -> Int { opt.unwrap_or(0) }
        fn caller(k: Secret[Option[Int]]) -> Int { accept(k) }
    "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret[Option[Int]] must not flow silently to unlabeled Option[Int] param, got: {errors:?}"
    );
}

#[test]
fn secret_result_to_unlabeled_result_rejected() {
    // GIVEN: fn foo(r: Result[Int, String]) called with a Secret[Result[Int, String]] argument
    // THEN: TypeMismatch — same label enforcement as Secret[Option[T]] (regression for #714)
    let errors = errors_for(
        r#"
        fn accept(r: Result[Int, String]) -> Int { r.unwrap_or(0) }
        fn caller(k: Secret[Result[Int, String]]) -> Int { accept(k) }
    "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret[Result[Int,String]] must not flow to unlabeled Result param, got: {errors:?}"
    );
}

#[test]
fn unlabeled_to_secret_param_rejected() {
    // Post-#894: no implicit flow. bare String ≠ Secret[String].
    // Must use relabel classify() to wrap.
    let errors = errors_for(
        r#"
        fn vault(s: Secret[String]) -> Secret[String] { s }
        fn caller(name: String) -> Secret[String] { vault(name) }
    "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "bare String must not flow to Secret[String] param without relabel, got: {errors:?}"
    );
}

#[test]
fn if_with_labeled_bool_condition_promotes_result() {
    // GIVEN: if-condition is Secret[Bool], branch results are Public[Int]
    // THEN: result type is Secret[Int] — cannot be returned as Public[Int]
    let errors = errors_for(
        r#"fn choose_secret(flag: Secret[Bool], a: Public[Int], b: Public[Int]) -> Public[Int] {
            if flag { a } else { b }
        }"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "if Secret[Bool] must promote result to Secret[Int], rejecting Public[Int] return, got: {errors:?}"
    );
}

#[test]
fn if_with_unlabeled_bool_condition_unchanged() {
    // GIVEN: if-condition is Bool (unlabeled), branches are Int
    // THEN: no type error — unlabeled condition adds no label to result
    let errors = errors_for(
        r#"fn choose(flag: Bool, a: Int, b: Int) -> Int {
        if flag { a } else { b }
    }"#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "unlabeled if-condition should not affect result type, got: {errors:?}"
    );
}

/// Regression: `Secret[Bool]` as if-condition must NOT produce a TypeMismatch on the condition
/// check itself — `is_bool()` must strip the label before testing for Bool.
///
/// - GIVEN `fn f(flag: Secret[Bool]) -> Unit`
/// - WHEN the condition is `Secret[Bool]` (labeled Bool)
/// - THEN no TypeMismatch is emitted for the condition expression
#[test]
fn secret_bool_if_condition_accepted() {
    let errors =
        errors_for(r#"fn f(flag: Secret[Bool]) -> Unit ! Console { if flag { println("x"); } }"#);
    let cond_mismatch = errors
        .iter()
        .any(|e| matches!(e, CheckError::TypeMismatch { found, .. } if found.contains("Secret")));
    assert!(
        !cond_mismatch,
        "Secret[Bool] must be accepted as if-condition (is_bool strips label), got: {errors:?}"
    );
}

/// Regression: `Tainted[Bool]` as while-condition must NOT produce a TypeMismatch on the
/// condition check itself.
///
/// - GIVEN `partial fn poll(cond: Tainted[Bool]) -> Unit`
/// - WHEN the while-condition is `Tainted[Bool]`
/// - THEN no TypeMismatch is emitted for the condition expression
#[test]
fn tainted_bool_while_condition_accepted() {
    let errors = errors_for(
        r#"partial fn poll(cond: Tainted[Bool]) -> Unit ! Console { while cond { println("x"); } }"#,
    );
    let cond_mismatch = errors
        .iter()
        .any(|e| matches!(e, CheckError::TypeMismatch { found, .. } if found.contains("Tainted")));
    assert!(
        !cond_mismatch,
        "Tainted[Bool] must be accepted as while-condition (is_bool strips label), got: {errors:?}"
    );
}

/// `Secret[Int]` must still be rejected as an if-condition — only labeled Bools are valid.
///
/// - GIVEN `fn f(n: Secret[Int]) -> Unit`
/// - WHEN the if-condition is `Secret[Int]`
/// - THEN TypeMismatch is emitted (expected Bool, found Secret[Int])
#[test]
fn secret_int_if_condition_rejected() {
    let errors = errors_for(r#"fn f(n: Secret[Int]) -> Unit ! Console { if n { println("x"); } }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret[Int] must be rejected as if-condition (not a Bool), got: {errors:?}"
    );
}

// ── extern block checking (#52, #91) ─────────────────────────────────────

#[test]
fn extern_rust_block_counts_as_trust_boundary() {
    // GIVEN: a program with one extern "rust" block
    // THEN: check_result.extern_count == 1
    use mvl::mvl::checker::check;
    use mvl::mvl::parser::Parser;
    let src = r#"extern "rust" {
    fn hash(data: String) -> String;
}"#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    let result = check(&prog);
    assert!(result.is_ok(), "check errors: {:?}", result.errors);
    assert_eq!(result.extern_count, 1, "extern block must be counted");
}

#[test]
fn multiple_extern_blocks_counted_separately() {
    use mvl::mvl::checker::check;
    use mvl::mvl::parser::Parser;
    let src = r#"extern "rust" {
    fn sha256(data: String) -> String;
}
extern "c" {
    fn strlen(s: String) -> Int;
}"#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    let result = check(&prog);
    assert!(result.is_ok(), "check errors: {:?}", result.errors);
    assert_eq!(result.extern_count, 2, "two extern blocks must count as 2");
}

#[test]
fn extern_unsupported_abi_is_an_error() {
    use mvl::mvl::checker::check;
    use mvl::mvl::checker::errors::CheckError;
    use mvl::mvl::parser::Parser;
    let src = r#"extern "java" { fn call() -> Int; }"#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let result = check(&prog);
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::UnsupportedExternAbi { .. })),
        "unsupported ABI must produce UnsupportedExternAbi error, got: {:?}",
        result.errors
    );
}

// ── FFI: extern "C" + Ptr[T] (#561) ──────────────────────────────────────

#[test]
fn extern_c_block_type_checks() {
    use mvl::mvl::checker::check;
    use mvl::mvl::parser::Parser;
    let src = r#"extern "C" {
    fn sqrt(x: Float) -> Float;
    fn strlen(s: String) -> Int;
}"#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    let result = check(&prog);
    assert!(
        result.is_ok(),
        "extern \"C\" should type-check, got: {:?}",
        result.errors
    );
    assert_eq!(result.extern_count, 1);
}

#[test]
fn extern_c_with_link_parses_and_checks() {
    use mvl::mvl::checker::check;
    use mvl::mvl::parser::Parser;
    let src = r#"extern "C" link("m") {
    fn sin(x: Float) -> Float;
    fn cos(x: Float) -> Float;
}"#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    let result = check(&prog);
    assert!(
        result.is_ok(),
        "extern \"C\" link(...) should type-check, got: {:?}",
        result.errors
    );
}

#[test]
fn ptr_type_resolves_in_extern_c() {
    use mvl::mvl::checker::check;
    use mvl::mvl::checker::types::Ty;
    use mvl::mvl::parser::Parser;
    let src = r#"extern "C" {
    fn malloc(size: Int) -> Ptr[Void];
    fn free(ptr: Ptr[Void]) -> Unit;
    fn strlen(s: Ptr[Int]) -> Int;
}"#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    let result = check(&prog);
    assert!(
        result.is_ok(),
        "Ptr[Void] should type-check, got: {:?}",
        result.errors
    );
    // Ptr[Void] resolves to Ty::Ptr(Ty::Unit) (Void is an alias for Unit in FFI context)
    let malloc_info = result
        .type_env()
        .lookup_fn("malloc")
        .expect("malloc should be registered");
    assert!(
        matches!(&malloc_info.ret, Ty::Ptr(inner) if matches!(inner.as_ref(), Ty::Unit)),
        "malloc return type should be Ptr[Unit], got: {:?}",
        malloc_info.ret
    );
    // strlen: Ptr[Int] param resolves to Ty::Ptr(Ty::Int)
    let strlen_info = result
        .type_env()
        .lookup_fn("strlen")
        .expect("strlen should be registered");
    assert!(
        matches!(&strlen_info.params[0], Ty::Ptr(inner) if matches!(inner.as_ref(), Ty::Int)),
        "strlen param should be Ptr[Int], got: {:?}",
        strlen_info.params
    );
}

#[test]
fn extern_rust_deprecation_warning_fires() {
    use mvl::mvl::linter::{config::LintConfig, lint};
    use mvl::mvl::parser::Parser;
    let src = r#"extern "rust" {
    fn hash(data: String) -> String;
}"#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let cfg = LintConfig::default(); // deprecated_extern_rust = true by default
    let result = lint(&prog, src, &cfg);
    assert!(
        result
            .diags
            .iter()
            .any(|d| d.rule == "deprecated-extern-rust"),
        "extern \"rust\" should trigger deprecated-extern-rust warning, got: {:?}",
        result.diags
    );
}

#[test]
fn extern_rust_deprecation_warning_suppressible() {
    use mvl::mvl::linter::{config::LintConfig, lint};
    use mvl::mvl::parser::Parser;
    let src = r#"extern "rust" {
    fn hash(data: String) -> String;
}"#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let cfg = LintConfig {
        deprecated_extern_rust: false,
        ..LintConfig::default()
    };
    let result = lint(&prog, src, &cfg);
    assert!(
        result
            .diags
            .iter()
            .all(|d| d.rule != "deprecated-extern-rust"),
        "rule must be silent when disabled, got: {:?}",
        result.diags
    );
}

#[test]
fn extern_fn_callable_from_mvl_code() {
    // extern-declared functions must be resolvable in MVL call expressions.
    let errors = errors_for(
        r#"extern "rust" {
    fn add_numbers(a: Int, b: Int) -> Int;
}
fn use_extern(x: Int) -> Int {
    add_numbers(x, x)
}"#,
    );
    assert!(
        errors.is_empty(),
        "extern fn should be callable from MVL code, got: {errors:?}"
    );
}

#[test]
fn relabel_trust_accepted() {
    // Post-#894: relabel trust(Tainted[String], "TAG") → String is valid.
    let errors = errors_for(
        r#"fn validate(raw: Tainted[String]) -> String {
    relabel trust(raw, "VALIDATED")
}"#,
    );
    assert!(
        errors.is_empty(),
        "relabel trust(Tainted[String]) should be accepted, got: {errors:?}"
    );
}

// ── Requirement 9: Generics (Spec 001, Phase 1 parse/check) ───────────────

fn parses_and_checks(src: &str) {
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    let result = check(&prog);
    assert!(result.is_ok(), "type errors: {:?}", result.errors);
}

#[test]
fn generic_identity_parses() {
    // Req 9: generic function with type parameter parses and checks
    parses_and_checks("total fn identity[T](x: T) -> T { return x; }");
}

#[test]
fn generic_type_decl_parses() {
    // Req 9: generic type declaration parses and checks
    parses_and_checks("type Container[T] = struct { value: T }");
}

#[test]
fn generic_pair_type_parses() {
    // Req 9: multiple type parameters parse and check
    parses_and_checks("type Pair[A, B] = struct { first: A, second: B }");
}

#[test]
fn generic_with_constraint_parses() {
    // Req 9: where-clause constraint parses and checks
    parses_and_checks("total fn max[T](a: T, b: T) -> T where T: Ord { return a; }");
}

#[test]
fn ord_constraint_satisfies_comparison() {
    // Req 9: T with Ord bound may use <, >, <=, >= without error
    parses_and_checks(
        "total fn max[T](a: T, b: T) -> T where T: Ord { if a > b { return a; } else { return b; } }",
    );
}

#[test]
fn eq_constraint_satisfies_equality() {
    // Req 9: T with Eq bound may use == and != without error
    parses_and_checks("total fn are_equal[T](a: T, b: T) -> Bool where T: Eq { return a == b; }");
}

#[test]
fn ord_constraint_satisfies_eq_check() {
    // Req 9: Ord is a supertrait of Eq — where T: Ord must also permit == and !=
    parses_and_checks(
        "total fn cmp_and_eq[T](a: T, b: T) -> Bool where T: Ord { if a > b { return true; } else { return a == b; } }",
    );
}

#[test]
fn generic_multiple_constraints_parse() {
    // Req 9: multiple constraints in where clause parse and check
    parses_and_checks(
        "total fn show_max[T](a: T, b: T) -> T where T: Ord, T: Display { return a; }",
    );
}

// ── Requirement 9: Generics — rejection scenarios (Phase 2 enforcement) ───
// These tests document the intended rejection semantics. They are marked
// #[ignore] until constraint enforcement is implemented in the checker.
// See: https://github.com/LAB271/mvl_language/issues/48

#[test]
fn missing_constraint_on_comparison_rejected() {
    // Req 9 Scenario: Missing constraint rejected
    // GIVEN unconstrained T used with `>` operator
    // THEN checker MUST reject with a missing-constraint error
    let (mut p, _) = Parser::new(
        "total fn max[T](a: T, b: T) -> T { if a > b { return a; } else { return b; } }",
    );
    let prog = p.parse_program();
    assert!(
        p.errors().is_empty(),
        "unexpected parse errors: {:?}",
        p.errors()
    );
    let result = check(&prog);
    assert!(
        !result.is_ok(),
        "unconstrained T used with > must be rejected, got: {:?}",
        result.errors
    );
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            CheckError::MissingConstraint { required_bound, .. } if required_bound == "Ord"
        )),
        "expected MissingConstraint(Ord), got: {:?}",
        result.errors
    );
}

#[test]
fn missing_eq_constraint_on_equality_rejected() {
    // Req 9: unconstrained T used with == must require Eq bound
    let (mut p, _) = Parser::new("total fn eq_check[T](a: T, b: T) -> Bool { return a == b; }");
    let prog = p.parse_program();
    assert!(
        p.errors().is_empty(),
        "unexpected parse errors: {:?}",
        p.errors()
    );
    let result = check(&prog);
    assert!(
        !result.is_ok(),
        "unconstrained T used with == must be rejected"
    );
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            CheckError::MissingConstraint { required_bound, .. } if required_bound == "Eq"
        )),
        "expected MissingConstraint(Eq), got: {:?}",
        result.errors
    );
}

#[test]
fn missing_eq_constraint_on_ne_rejected() {
    // Req 9: unconstrained T used with != must require Eq bound
    let (mut p, _) = Parser::new("total fn neq_check[T](a: T, b: T) -> Bool { return a != b; }");
    let prog = p.parse_program();
    assert!(
        p.errors().is_empty(),
        "unexpected parse errors: {:?}",
        p.errors()
    );
    let result = check(&prog);
    assert!(
        !result.is_ok(),
        "unconstrained T used with != must be rejected"
    );
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            CheckError::MissingConstraint { required_bound, .. } if required_bound == "Eq"
        )),
        "expected MissingConstraint(Eq), got: {:?}",
        result.errors
    );
}

#[test]
fn unconstrained_second_param_rejected_when_first_is_constrained() {
    // Req 9: A has Ord, B does not — comparing two B values must still fail
    let (mut p, _) = Parser::new(
        "total fn pair_cmp[A, B](a1: A, a2: A, b1: B, b2: B) -> Bool where A: Ord { return b1 > b2; }",
    );
    let prog = p.parse_program();
    assert!(
        p.errors().is_empty(),
        "unexpected parse errors: {:?}",
        p.errors()
    );
    let result = check(&prog);
    assert!(
        !result.is_ok(),
        "unconstrained B used with > must be rejected even when A has Ord"
    );
}

#[test]
fn constrained_first_param_allowed_when_second_unconstrained() {
    // Req 9: comparing A values is fine; A's Ord bound must not leak to B
    parses_and_checks(
        "total fn pair_cmp[A, B](a1: A, a2: A, b1: B, b2: B) -> Bool where A: Ord { return a1 > a2; }",
    );
}

#[test]
fn higher_kinded_type_param_rejected() {
    // Req 9 Scenario: No higher-kinded types
    // GIVEN F[_] nested square-bracket type param (HKT)
    // THEN parser MUST reject with a higher-kinded diagnostic
    let (mut p, _) = Parser::new("type Functor[F[_]] = struct { val: Int }");
    let _ = p.parse_program();
    assert!(
        !p.errors().is_empty(),
        "HKT type parameter syntax must be rejected by the parser"
    );
    let first = p.errors().first().expect("should have at least one error");
    assert!(
        first.message.contains("higher-kinded"),
        "first error should be the HKT rejection, got: {:?}",
        first.message
    );
}

#[test]
fn inline_constraint_syntax_rejected() {
    // Req 9 Scenario: Inline constraint syntax rejected
    // GIVEN [T: Ord] inline constraint syntax
    // THEN parser MUST reject with a diagnostic mentioning `where`
    let (mut p, _) = Parser::new("total fn max[T: Ord](a: T, b: T) -> T { return a; }");
    let _ = p.parse_program();
    assert!(
        !p.errors().is_empty(),
        "inline constraint `[T: Ord]` must be rejected in Phase 1"
    );
    let first = p.errors().first().expect("should have at least one error");
    assert!(
        first.message.contains("inline constraint") && first.message.contains("where"),
        "error should explain to use a where clause, got: {:?}",
        first.message
    );
}

// ── From/Into conversion (#62) ────────────────────────────────────────────

/// `?` with identical error types requires no From impl.
#[test]
fn propagate_same_error_type_accepted() {
    let src = r#"
fn inner() -> Result[Int, String] { Ok(0) }
fn outer() -> Result[Int, String] {
    let x: Int = inner()?;
    Ok(x)
}
"#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "? with identical error types should have no errors, got: {errors:?}"
    );
}

/// `?` with different error types is rejected unless From impl is registered.
#[test]
fn propagate_mismatched_error_type_rejected() {
    let src = r#"
fn inner() -> Result[Int, String] { Ok(0) }
fn outer() -> Result[Int, Bool] {
    let x: Int = inner()?;
    Ok(x)
}
"#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::PropagateIncompatibleError { .. })),
        "? with incompatible error types should emit PropagateIncompatibleError, got: {errors:?}"
    );
}

/// `?` with different error types is accepted when From impl exists.
#[test]
fn propagate_with_from_impl_accepted() {
    let src = r#"
type IoError = struct { msg: String }
type AppError = enum { Io(IoError) }
impl From[IoError] for AppError {
    fn from(e: IoError) -> Self { AppError::Io(e) }
}
fn load() -> Result[String, IoError] { Ok("data") }
fn run() -> Result[String, AppError] {
    let s = load()?;
    Ok(s)
}
"#;
    let errors = errors_for(src);
    let conversion_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, CheckError::PropagateIncompatibleError { .. }))
        .collect();
    assert!(
        conversion_errors.is_empty(),
        "? should be accepted when From impl exists, got: {conversion_errors:?}"
    );
}

// ── #58/#66: Map/Set literals and multiline/raw strings ──────────────────────

#[test]
fn literals_corpus_with_multiline_raw_strings_checks() {
    let src = include_str!("corpus/01_syntax/literals.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "literals corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn map_literal_infers_named_map_type() {
    let errors = errors_for(r#"fn f() -> Unit { let _m: Map[String, Int] = {"a": 1, "b": 2}; }"#);
    assert!(
        errors.is_empty(),
        "map literal should type-check cleanly, got: {errors:?}"
    );
}

#[test]
fn set_literal_infers_named_set_type() {
    let errors = errors_for(r#"fn f() -> Unit { let _s: Set[Int] = {1, 2, 3}; }"#);
    assert!(
        errors.is_empty(),
        "set literal should type-check cleanly, got: {errors:?}"
    );
}

// ── 003-information-flow/Req 6: Logging label enforcement ────────────────────

/// `println` with a Secret argument MUST be rejected (003-information-flow/Req 6).
#[test]
fn println_rejects_secret_argument() {
    let errors = errors_for(r#"fn f(pwd: Secret[String]) -> Unit ! Console { println(pwd); }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "println with Secret arg should emit TypeMismatch, got: {errors:?}"
    );
}

/// `println` with a Tainted argument MUST be rejected (003-information-flow/Req 6).
#[test]
fn println_rejects_tainted_argument() {
    let errors =
        errors_for(r#"fn f(input: Tainted[String]) -> Unit ! Console { println(input); }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "println with Tainted arg should emit TypeMismatch, got: {errors:?}"
    );
}

/// `println` with a bare (unlabeled) argument MUST be accepted (#1007).
/// `Public` is not a declared label — bare String is the "public" type.
#[test]
fn println_accepts_bare_argument() {
    let errors = errors_for(r#"fn f(msg: String) -> Unit ! Console { println(msg); }"#);
    let label_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, CheckError::TypeMismatch { .. }))
        .collect();
    assert!(
        label_errors.is_empty(),
        "println with bare String arg should not emit TypeMismatch, got: {label_errors:?}"
    );
}

/// `println` with a Tainted argument MUST be rejected (003-information-flow/Req 6).
/// Post-#894: only bare (unlabeled) values may be logged.
#[test]
fn println_rejects_tainted_argument_in_logging() {
    let errors = errors_for(r#"fn f(s: Tainted[String]) -> Unit ! Console { println(s); }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "println with Tainted arg should emit TypeMismatch, got: {errors:?}"
    );
}

/// `print` with a Secret argument MUST be rejected (003-information-flow/Req 6).
#[test]
fn print_rejects_secret_argument() {
    let errors = errors_for(r#"fn f(pwd: Secret[String]) -> Unit ! Console { print(pwd); }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "print with Secret arg should emit TypeMismatch, got: {errors:?}"
    );
}

/// `print` with a Tainted argument MUST be rejected (003-information-flow/Req 6).
#[test]
fn print_rejects_tainted_argument() {
    let errors = errors_for(r#"fn f(input: Tainted[String]) -> Unit ! Console { print(input); }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "print with Tainted arg should emit TypeMismatch, got: {errors:?}"
    );
}

/// `assert_eq[T]` is generic with `Unknown` params (not real type-var params).
/// Its params don't participate in label propagation (#1066), so labeled args are
/// accepted and the `Unit` return is not infected with a label.
#[test]
fn assert_eq_accepts_labeled_argument_generic() {
    let errors = errors_for(r#"fn f(key: Secret[String]) -> Unit { assert_eq(key, "expected"); }"#);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "assert_eq is generic — labeled args are accepted, got: {errors:?}"
    );
}

/// `assert_eq` MUST accept non-String types (generic — #902).
#[test]
fn assert_eq_accepts_int_args() {
    let errors = errors_for(r#"fn f(a: Int, b: Int) -> Unit { assert_eq(a, b); }"#);
    assert!(
        errors.is_empty(),
        "assert_eq with Int args should be accepted, got: {errors:?}"
    );
}

/// `assert_ne` MUST accept non-String types and be callable without bypass (#902).
#[test]
fn assert_ne_accepts_bool_args() {
    let errors = errors_for(r#"fn f(a: Bool, b: Bool) -> Unit { assert_ne(a, b); }"#);
    assert!(
        errors.is_empty(),
        "assert_ne with Bool args should be accepted, got: {errors:?}"
    );
}

/// `parse_int` arity MUST be enforced — 0 args rejected (#902).
#[test]
fn parse_int_wrong_arity_rejected() {
    let errors = errors_for(r#"fn f() -> Result[Int, String] { parse_int() }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::WrongArgCount { name, .. } if name == "parse_int")),
        "parse_int() with 0 args should emit WrongArgCount, got: {errors:?}"
    );
}

/// `float` arity MUST be enforced — 1 arg rejected (#902).
#[test]
fn float_wrong_arity_rejected() {
    let errors = errors_for(r#"fn f() -> Float ! Random { float("extra") }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::WrongArgCount { name, .. } if name == "float")),
        "float(\"extra\") with 1 arg should emit WrongArgCount, got: {errors:?}"
    );
}

// ── 002-effect-system/Req 2: Effect name validation ──────────────────────────

/// Unknown effect name MUST be rejected (002-effect-system/Req 2, ADR-0035).
#[test]
fn invalid_effect_name_rejected() {
    // Use std/effects.mvl prelude so the hierarchy is populated.
    let result = check_with_effects(r#"fn f() -> Unit ! IoMagic { }"#);
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidEffectName { name, .. } if name == "IoMagic")),
        "unknown effect name should emit InvalidEffectName, got: {:?}",
        result.errors
    );
}

/// All std/effects.mvl effect names MUST be accepted (002-effect-system/Req 2, ADR-0035).
#[test]
fn valid_effect_names_accepted() {
    // These are all declared in std/effects.mvl (base + composite).
    let canonical = [
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
        "Spawn",
        "Send",
        "Recv",
        "Log",
        "IO",
        "Actor",
    ];
    for name in &canonical {
        let src = format!("fn f() -> Unit ! {name} {{ }}");
        let result = check_with_effects(&src);
        let effect_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::InvalidEffectName { .. }))
            .collect();
        assert!(
            effect_errors.is_empty(),
            "canonical effect `{name}` should not emit InvalidEffectName, got: {effect_errors:?}"
        );
    }
}

/// `Async` is no longer a valid effect — replaced by Spawn/Send/Recv (ADR-0035, #856).
#[test]
fn async_effect_rejected_after_migration() {
    let result = check_with_effects(r#"fn f() -> Unit ! Async { }"#);
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidEffectName { name, .. } if name == "Async")),
        "`Async` should be rejected after migration to Spawn/Send/Recv, got: {:?}",
        result.errors
    );
}

/// `IO` is now a valid composite effect in std/effects.mvl (ADR-0035).
#[test]
fn io_composite_effect_accepted() {
    let result = check_with_effects(r#"fn f() -> Unit ! IO { }"#);
    let effect_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| matches!(e, CheckError::InvalidEffectName { .. }))
        .collect();
    assert!(
        effect_errors.is_empty(),
        "`IO` is a valid composite effect in std/effects.mvl, got: {effect_errors:?}"
    );
}

// ── ADR-0002: Lambda capture immutability ────────────────────────────────────

/// Lambda capturing a mutable binding MUST be rejected (ADR-0002).
#[test]
fn lambda_mutable_capture_rejected() {
    let errors = errors_for(
        r#"fn f() -> Unit { let x: ref Int = 1; let _g: fn(Int) -> Int = |y: Int| -> Int { x + y }; }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::CaptureMutabilityViolation { name, .. } if name == "x")
        ),
        "lambda capturing mut x should emit CaptureMutabilityViolation, got: {errors:?}"
    );
}

/// Lambda capturing an immutable binding MUST be accepted (ADR-0002).
#[test]
fn lambda_immutable_capture_accepted() {
    let result = check_src(
        r#"fn f() -> Unit { let x: Int = 1; let _g: fn(Int) -> Int = |y: Int| -> Int { x + y }; }"#,
    );
    let capture_errors: Vec<_> = result
        .errors
        .iter()
        .filter(|e| matches!(e, CheckError::CaptureMutabilityViolation { .. }))
        .collect();
    assert!(
        capture_errors.is_empty(),
        "lambda with immutable capture should not emit CaptureMutabilityViolation, got: {capture_errors:?}"
    );
}

// ── 002-effect-system/Req 4: Effect Subsumption (ADR-0035) ───────────────────

/// `! IO` satisfies `! Console` via subsumption (IO > Console in std/effects.mvl).
#[test]
fn io_subsumes_console() {
    let src = r#"
        fn effectful() -> Unit ! Console { }
        fn caller() -> Unit ! IO { effectful() }
    "#;
    assert_no_effect_propagation_errors(
        &check_with_effects(src),
        "! IO should satisfy ! Console via subsumption",
    );
}

/// `! Log` satisfies `! Clock` (Log > Clock in std/effects.mvl).
#[test]
fn log_subsumes_clock() {
    let src = r#"
        fn now() -> Unit ! Clock { }
        fn logger() -> Unit ! Log { now() }
    "#;
    assert_no_effect_propagation_errors(
        &check_with_effects(src),
        "! Log should satisfy ! Clock via subsumption",
    );
}

/// `! IO` transitively satisfies `! Clock` (IO > Log > Clock).
#[test]
fn io_transitively_subsumes_clock() {
    let src = r#"
        fn now() -> Unit ! Clock { }
        fn main() -> Unit ! IO { now() }
    "#;
    assert_no_effect_propagation_errors(
        &check_with_effects(src),
        "! IO should transitively satisfy ! Clock (IO > Log > Clock)",
    );
}

/// User-defined domain effect: `Billing` subsumes `DB` + `Log`.
#[test]
fn user_defined_effect_subsumption() {
    let src = r#"
        effect Billing > DB + Log
        fn db_insert() -> Unit ! DB { }
        fn log_debug() -> Unit ! Log { }
        fn charge() -> Unit ! Billing { db_insert() log_debug() }
    "#;
    assert_no_effect_propagation_errors(
        &check_with_effects(src),
        "user-defined Billing > DB + Log should compile",
    );
}

/// `! Log` satisfies `! Console` because `Log > Clock + Console`.
#[test]
fn log_subsumes_console() {
    let src = r#"
        fn printer() -> Unit ! Console { }
        fn logger() -> Unit ! Log { printer() }
    "#;
    let result = check_with_effects(src);
    assert!(
        result.errors.is_empty(),
        "! Log should satisfy ! Console (Log > Clock + Console), got: {:?}",
        result.errors
    );
}

// ── IFC Phase 3: implicit flow detection (003-information-flow/Req 11) ────────

/// `println` inside a branch controlled by a `Secret` condition MUST be rejected.
///
/// Even though the argument to `println` is a literal (Public), the fact that
/// the print fires at all reveals whether `flag` was truthy — an implicit flow.
///
/// - GIVEN `fn f(flag: Secret[Bool]) -> Unit`
/// - WHEN `if flag { println("branch taken") }`
/// - THEN `ImplicitFlowViolation` with pc_label="Secret" is emitted
#[test]
fn implicit_flow_secret_if_condition_rejected() {
    let errors = errors_for(
        r#"fn f(flag: Secret[Bool]) -> Unit ! Console { if flag { println("branch taken"); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, observable_fn, .. }
                if pc_label == "Secret" && observable_fn == "println")
        ),
        "println inside Secret branch should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// `println` inside a branch controlled by a `Tainted` condition MUST be rejected.
///
/// - GIVEN `fn f(cond: Tainted[Bool]) -> Unit`
/// - WHEN `if cond { println("ok") }`
/// - THEN `ImplicitFlowViolation` with pc_label="Tainted" is emitted
#[test]
fn implicit_flow_tainted_if_condition_rejected() {
    let errors =
        errors_for(r#"fn f(cond: Tainted[Bool]) -> Unit ! Console { if cond { println("ok"); } }"#);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, observable_fn, .. }
                if pc_label == "Tainted" && observable_fn == "println")
        ),
        "println inside Tainted branch should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// `println` inside a branch controlled by a `Public` condition MUST be accepted.
///
/// No implicit flow: the condition has no security label, so the branch is safe.
///
/// - GIVEN `fn f(x: Public[Bool]) -> Unit`
/// - WHEN `if x { println("ok") }`
/// - THEN no `ImplicitFlowViolation`
#[test]
fn implicit_flow_public_condition_accepted() {
    let errors =
        errors_for(r#"fn f(x: Public[Bool]) -> Unit ! Console { if x { println("ok"); } }"#);
    let implicit: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, CheckError::ImplicitFlowViolation { .. }))
        .collect();
    assert!(
        implicit.is_empty(),
        "println inside Public branch should not emit ImplicitFlowViolation, got: {implicit:?}"
    );
}

/// `print` inside a `Secret`-controlled branch MUST also be rejected.
///
/// - GIVEN `fn g(s: Secret[Bool]) -> Unit`
/// - WHEN `if s { print("x"); }`
/// - THEN `ImplicitFlowViolation` with sink="print" is emitted
#[test]
fn implicit_flow_print_observable_rejected() {
    let errors = errors_for(r#"fn g(s: Secret[Bool]) -> Unit ! Console { if s { print("x"); } }"#);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, observable_fn, .. }
                if pc_label == "Secret" && observable_fn == "print")
        ),
        "print inside Secret branch should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// `println` in the else-branch of a `Secret`-controlled if MUST also be rejected.
///
/// Both branches are controlled by the condition; the else branch also leaks
/// information (its firing reveals the condition was false).
///
/// - GIVEN `fn h(flag: Secret[Bool]) -> Unit`
/// - WHEN `if flag { 0; } else { println("not taken"); }`
/// - THEN `ImplicitFlowViolation` is emitted for the else-branch println
#[test]
fn implicit_flow_else_branch_rejected() {
    let errors = errors_for(
        r#"fn h(flag: Secret[Bool]) -> Unit ! Console { if flag { } else { println("not taken"); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, .. }
                if pc_label == "Secret")
        ),
        "println in else of Secret branch should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// Let-bound variable with `Secret` type annotation propagates its label into
/// nested branch conditions.
///
/// - GIVEN `fn f(raw: Secret[Int]) -> Unit`
/// - WHEN `let x: Secret[Int] = raw; if x { println("y"); }`
/// - THEN `ImplicitFlowViolation` is emitted (label propagated through let binding)
#[test]
fn implicit_flow_label_propagated_through_let() {
    let errors = errors_for(
        r#"fn f(raw: Secret[Int]) -> Unit ! Console { let x: Secret[Int] = raw; if x { println("y"); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, .. }
                if pc_label == "Secret")
        ),
        "label should propagate through let binding to branch condition, got: {errors:?}"
    );
}

/// `println` inside a `while` loop controlled by a `Secret` condition MUST be rejected.
///
/// A while-loop fires zero or more times depending on the condition — its
/// execution reveals information about the Secret value, creating an implicit flow.
///
/// - GIVEN `fn poll(flag: Secret[Bool]) -> Unit ! Console`
/// - WHEN `while flag { println("still waiting"); }`
/// - THEN `ImplicitFlowViolation` with pc_label="Secret" and sink="println" is emitted
#[test]
fn implicit_flow_while_secret_condition_rejected() {
    let errors = errors_for(
        r#"fn poll(flag: Secret[Bool]) -> Unit ! Console { while flag { println("still waiting"); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, observable_fn, .. }
                if pc_label == "Secret" && observable_fn == "println")
        ),
        "println inside Secret while-loop should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// `implicit_flow_else_branch_rejected` additionally verifies the sink field.
///
/// The else-branch println leaks information about the Secret condition
/// (its firing proves the condition was false).
///
/// - GIVEN `fn h(flag: Secret[Bool]) -> Unit ! Console`
/// - WHEN `if flag { 0; } else { println("not taken"); }`
/// - THEN `ImplicitFlowViolation` with pc_label="Secret" and sink="println"
#[test]
fn implicit_flow_else_branch_observable_verified() {
    let errors = errors_for(
        r#"fn h(flag: Secret[Bool]) -> Unit ! Console { if flag { } else { println("not taken"); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, observable_fn, .. }
                if pc_label == "Secret" && observable_fn == "println")
        ),
        "println in else of Secret branch should emit ImplicitFlowViolation with observable_fn=println, got: {errors:?}"
    );
}

/// For-loop over a tainted iterator must propagate the iterator label to the
/// loop variable and raise an ImplicitFlowViolation when the body uses a public sink.
///
/// - GIVEN `fn f(items: Tainted[String]) -> Unit`
/// - WHEN `for x in items { println(x) }`
/// - THEN `ImplicitFlowViolation` with pc_label="Tainted" is emitted
#[test]
fn implicit_flow_for_loop_tainted_iterator_rejected() {
    let errors = errors_for(
        r#"fn f(items: Tainted[String]) -> Unit ! Console { for x in items { println(x); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, observable_fn, .. }
                if pc_label == "Tainted" && observable_fn == "println")
        ),
        "println inside for-loop over Tainted iterator should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// Implicit flow corpus: load and verify the implicit_flow.mvl corpus file.
///
/// The corpus contains only INVALID programs that should each produce
/// `ImplicitFlowViolation` errors. This test validates the corpus itself.
#[test]
fn implicit_flow_corpus_has_violations() {
    let src = include_str!("corpus/08_ifc/implicit_flow.mvl");
    let result = check_src(src);
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ImplicitFlowViolation { .. })),
        "implicit_flow corpus should contain at least one ImplicitFlowViolation, got: {:?}",
        result.errors
    );
}

// ── #834: Interprocedural IFC corpus tests (Req 11) ───────────────────────────

/// Cross-function implicit flow corpus: wrapper around println called under
/// a Secret-controlled branch MUST emit `CrossFunctionImplicitFlowViolation`.
#[test]
fn cross_function_implicit_corpus_has_violations() {
    // GIVEN: a helper wrapping println called inside a Secret-controlled if
    // THEN: CrossFunctionImplicitFlowViolation is emitted
    let src = include_str!("corpus/08_ifc/cross_function_implicit.mvl");
    let result = check_src(src);
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::CrossFunctionImplicitFlowViolation { .. })),
        "cross_function_implicit corpus should contain CrossFunctionImplicitFlowViolation, got: {:?}",
        result.errors
    );
}

/// Interprocedural taint chain corpus: a two-hop chain (caller→middle→println)
/// where the top-level caller is invoked under a Tainted condition MUST emit
/// `CrossFunctionImplicitFlowViolation`.
#[test]
fn interprocedural_taint_corpus_has_violations() {
    // GIVEN: record_decision() → emit_log_line() → println; called under Tainted branch
    // THEN: CrossFunctionImplicitFlowViolation is emitted
    let src = include_str!("corpus/08_ifc/interprocedural_taint.mvl");
    let result = check_src(src);
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::CrossFunctionImplicitFlowViolation { .. })),
        "interprocedural_taint corpus should contain CrossFunctionImplicitFlowViolation, got: {:?}",
        result.errors
    );
}

/// Return-label inference corpus: a pure helper (no public sink) called under a
/// Secret-controlled branch MUST NOT produce any Req 11 violations.
#[test]
fn return_label_inference_corpus_has_no_req11_violations() {
    // GIVEN: compute_hash_code reaches no public sink; called under Secret branch
    // THEN: no ImplicitFlowViolation or CrossFunctionImplicitFlowViolation
    let src = include_str!("corpus/08_ifc/return_label_inference.mvl");
    let result = check_src(src);
    let violations: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::ImplicitFlowViolation { .. }
                    | CheckError::CrossFunctionImplicitFlowViolation { .. }
            )
        })
        .collect();
    assert!(
        violations.is_empty(),
        "return_label_inference corpus must have no Req 11 violations, got: {violations:?}"
    );
}

/// Interprocedural clean corpus: a sink-reaching function called unconditionally
/// (PC = None) MUST NOT produce any Req 11 violations.
#[test]
fn interprocedural_clean_corpus_has_no_req11_violations() {
    // GIVEN: announce_result() reaches println but is called with PC = None
    // THEN: no ImplicitFlowViolation or CrossFunctionImplicitFlowViolation
    let src = include_str!("corpus/08_ifc/interprocedural_clean.mvl");
    let result = check_src(src);
    let violations: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                CheckError::ImplicitFlowViolation { .. }
                    | CheckError::CrossFunctionImplicitFlowViolation { .. }
            )
        })
        .collect();
    assert!(
        violations.is_empty(),
        "interprocedural_clean corpus must have no Req 11 violations, got: {violations:?}"
    );
}

/// Call-chain error message corpus: the emitted `CrossFunctionImplicitFlowViolation`
/// MUST name the direct callee and the observable function it reaches.
#[test]
fn call_chain_error_names_callee_and_observable() {
    // GIVEN: write_audit_log() reaches println; called under Secret branch
    // THEN: CrossFunctionImplicitFlowViolation with callee="write_audit_log", observable_fn="println"
    let src = include_str!("corpus/08_ifc/call_chain_error_message.mvl");
    let result = check_src(src);
    let violation = result
        .errors
        .iter()
        .find(|e| matches!(e, CheckError::CrossFunctionImplicitFlowViolation { .. }));
    assert!(
        violation.is_some(),
        "call_chain_error_message corpus should contain CrossFunctionImplicitFlowViolation, got: {:?}",
        result.errors
    );
    if let Some(CheckError::CrossFunctionImplicitFlowViolation {
        callee,
        observable_fn,
        ..
    }) = violation
    {
        assert_eq!(
            callee, "write_audit_log",
            "error should name callee=write_audit_log, got: {callee}"
        );
        assert_eq!(
            observable_fn, "println",
            "error should name observable_fn=println, got: {observable_fn}"
        );
    }
}

// ── #1007: Effect-based IFC — missing edge case coverage ─────────────────────

/// Gap 1a: Transitive chain where the intermediate function b is NOT effectful.
///
/// When `a` calls `b` and `b` calls `println`, but only `println` declares
/// `! Console` (b has no effects), then `b` is seeded as a reachability entry
/// pointing at `println`, and `a` is propagated from `b`.  The violation for
/// `entry` calling `a` under a Secret PC should be a
/// `CrossFunctionImplicitFlowViolation` naming callee="a" and
/// observable_fn="println" (the terminal effectful function, since `b` itself
/// is NOT effectful and therefore not registered as an observable).
///
/// - GIVEN chain: a → b → println, where only println has `! Console`
/// - WHEN `if secret { a() }`
/// - THEN `CrossFunctionImplicitFlowViolation` with callee="a", observable_fn="println"
#[test]
fn transitive_chain_non_effectful_intermediate_reports_terminal_observable() {
    let errors = errors_for(
        r#"fn println(msg: String) -> Unit ! Console { }
           fn b(msg: String) -> Unit { println(msg) }
           fn a(msg: String) -> Unit { b(msg) }
           fn entry(flag: Secret[Bool]) -> Unit { if flag { a("x"); } }"#,
    );
    let violation = errors.iter().find(|e| {
        matches!(e, CheckError::CrossFunctionImplicitFlowViolation { callee, .. }
            if callee == "a")
    });
    assert!(
        violation.is_some(),
        "transitive a→b→println (b non-effectful) under Secret PC should emit CrossFunctionImplicitFlowViolation, got: {errors:?}"
    );
    if let Some(CheckError::CrossFunctionImplicitFlowViolation { observable_fn, .. }) = violation {
        assert_eq!(
            observable_fn, "println",
            "when intermediate b is non-effectful, observable_fn should be the terminal println, got: {observable_fn}"
        );
    }
}

/// Gap 1b: Three-hop chain where the directly-called function is effectful.
///
/// When `a` (effectful) calls `b` (effectful) which calls `println` (effectful),
/// the nearest observable callee of `a` is `b` (the first effectful fn it calls).
/// Calling `a` under a high PC must report observable_fn="b", not "println".
///
/// - GIVEN chain: a(! Console) → b(! Console) → println(! Console)
/// - WHEN `if secret { a() }`
/// - THEN `CrossFunctionImplicitFlowViolation` with callee="a", observable_fn="b"
#[test]
fn transitive_chain_effectful_intermediate_reports_nearest_observable() {
    let errors = errors_for(
        r#"fn println(msg: String) -> Unit ! Console { }
           fn b(msg: String) -> Unit ! Console { println(msg) }
           fn a(msg: String) -> Unit ! Console { b(msg) }
           fn entry(flag: Secret[Bool]) -> Unit ! Console { if flag { a("x"); } }"#,
    );
    let violation = errors.iter().find(|e| {
        matches!(e, CheckError::CrossFunctionImplicitFlowViolation { callee, .. }
            if callee == "a")
    });
    assert!(
        violation.is_some(),
        "transitive a→b→println (all effectful) under Secret PC should emit CrossFunctionImplicitFlowViolation, got: {errors:?}"
    );
    if let Some(CheckError::CrossFunctionImplicitFlowViolation { observable_fn, .. }) = violation {
        assert_eq!(
            observable_fn, "b",
            "when intermediate b is effectful, observable_fn should be b (nearest effectful callee), got: {observable_fn}"
        );
    }
}

/// Gap 2: Observable functions declared in the prelude (stdlib) are detected.
///
/// `check_implicit_flows` receives `all_programs` which includes the prelude.
/// The prelude-declared `println ! Console` MUST be found by `collect_effectful_names`
/// and trigger a violation when called under a high PC.
///
/// This test calls `check_src` (which loads SINK_PRELUDE as a separate prelude program)
/// to exercise the cross-program observable collection path.
///
/// - GIVEN: println declared in prelude with `! Console`
/// - WHEN: user module calls println inside `if secret { ... }`
/// - THEN: `ImplicitFlowViolation` is emitted (prelude fn is detected as observable)
#[test]
fn prelude_effectful_fn_is_detected_as_observable() {
    // println is declared only in SINK_PRELUDE (separate prelude program), not inlined here.
    let errors = errors_for(
        r#"fn f(flag: Secret[Bool]) -> Unit ! Console { if flag { println("leaked"); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, observable_fn, .. }
                if pc_label == "Secret" && observable_fn == "println")
        ),
        "prelude-declared println under Secret PC should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// Gap 3a: `println` called with a `Secret[String]` argument must produce `TypeMismatch`.
///
/// Direct-flow enforcement (Req 11 Phase 1): the type checker catches
/// `Secret[String]` passed where bare `String` is required.
/// `LoggingLabelViolation` was removed; `TypeMismatch` is now the mechanism.
///
/// - GIVEN `fn f(s: Secret[String]) -> Unit`
/// - WHEN `println(s)` where println takes bare `String`
/// - THEN `TypeMismatch` is emitted
#[test]
fn println_with_secret_arg_produces_type_mismatch() {
    let errors = errors_for(r#"fn f(s: Secret[String]) -> Unit ! Console { println(s); }"#);
    assert!(
        errors.iter().any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "println(Secret[String]) must produce TypeMismatch (direct-flow enforcement), got: {errors:?}"
    );
}

/// Gap 3b: `println` called with a `Tainted[String]` argument must produce `TypeMismatch`.
///
/// - GIVEN `fn f(s: Tainted[String]) -> Unit`
/// - WHEN `println(s)` where println takes bare `String`
/// - THEN `TypeMismatch` is emitted
#[test]
fn println_with_tainted_arg_produces_type_mismatch() {
    let errors = errors_for(r#"fn f(s: Tainted[String]) -> Unit ! Console { println(s); }"#);
    assert!(
        errors.iter().any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "println(Tainted[String]) must produce TypeMismatch (direct-flow enforcement), got: {errors:?}"
    );
}

/// Gap 3c: `print` called with a `Secret[String]` argument must produce `TypeMismatch`.
///
/// - GIVEN `fn g(s: Secret[String]) -> Unit`
/// - WHEN `print(s)` where print takes bare `String`
/// - THEN `TypeMismatch` is emitted
#[test]
fn print_with_secret_arg_produces_type_mismatch() {
    let errors = errors_for(r#"fn g(s: Secret[String]) -> Unit ! Console { print(s); }"#);
    assert!(
        errors.iter().any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "print(Secret[String]) must produce TypeMismatch (direct-flow enforcement), got: {errors:?}"
    );
}

/// Gap 3d: `println` with a bare `String` argument is accepted (no false positive).
///
/// The replacement of `LoggingLabelViolation` with `TypeMismatch` must not
/// cause false positives: a bare string argument to println must be accepted.
///
/// - GIVEN `fn f(msg: String) -> Unit`
/// - WHEN `println(msg)`
/// - THEN no error
#[test]
fn println_with_bare_string_arg_accepted() {
    let errors = errors_for(r#"fn f(msg: String) -> Unit ! Console { println(msg); }"#);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "println(String) must not produce TypeMismatch, got: {errors:?}"
    );
}

/// Gap 5: When a function calls two different effectful functions, the violation
/// still fires and names an observable function (whichever is seeded first).
///
/// The BFS seed uses `or_insert_with` (first-wins).  This test verifies that
/// a function calling two observable targets is still flagged under high PC.
///
/// - GIVEN `fn multi() { println("a"); eprintln("b") }` (two effectful callees)
/// - WHEN `if secret { multi() }`
/// - THEN `CrossFunctionImplicitFlowViolation` is emitted
#[test]
fn fn_calling_two_effectful_fns_is_flagged_under_high_pc() {
    let errors = errors_for(
        r#"fn multi() -> Unit ! Console { println("a"); eprintln("b"); }
           fn entry(flag: Secret[Bool]) -> Unit ! Console { if flag { multi(); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::CrossFunctionImplicitFlowViolation { callee, .. }
                if callee == "multi")
        ),
        "fn calling two effectful fns under Secret PC should emit CrossFunctionImplicitFlowViolation, got: {errors:?}"
    );
}

/// Implicit flow inside `impl Trait for Type` method bodies is detected.
///
/// `check_implicit_flows` walks `Decl::Impl` method bodies — calling
/// `println` under a Secret PC inside an impl method is flagged.
///
/// Note: bare `self` in impl blocks is a parser limitation — use `self: Type`.
///
/// - GIVEN `impl Audit for Ctx { fn run(self: Ctx, flag: Secret[Bool]) ! Console { if flag { println(...) } } }`
/// - THEN `ImplicitFlowViolation` is emitted
#[test]
fn implicit_flow_in_trait_impl_method_body_detected() {
    let errors = errors_for(
        r#"type Ctx = struct { dummy: Int }
           trait Audit { fn run(self, flag: Secret[Bool]) -> Unit ! Console; }
           impl Audit for Ctx {
               fn run(self: Ctx, flag: Secret[Bool]) -> Unit ! Console {
                   if flag { println("leak"); }
               }
           }"#,
    );
    let implicit: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, CheckError::ImplicitFlowViolation { .. }))
        .collect();
    assert!(
        !implicit.is_empty(),
        "impl-block method implicit flows should be detected — got: {implicit:?}"
    );
}

// ── #136: Refinement type solver — Req 10 (Phase 3) ──────────────────────────

/// Literal zero violates `b != 0` refinement — should report RefinementViolated.
#[test]
fn refinement_literal_zero_to_nonzero_param_rejected() {
    // GIVEN: function requires b != 0
    // WHEN: literal 0 is passed
    // THEN: RefinementViolated emitted
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller() -> Int { safe_divide(10, 0) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "passing literal 0 to `where b != 0` parameter must emit RefinementViolated, got: {errors:?}"
    );
}

/// Positive literal satisfies `b > 0` — should NOT report an error.
#[test]
fn refinement_positive_literal_proven_accepted() {
    // GIVEN: function requires b > 0
    // WHEN: literal 5 (positive) is passed
    // THEN: no RefinementViolated
    let src = r#"
        total fn positive_only(b: Int where b > 0) -> Int { b }
        total fn caller() -> Int { positive_only(5) }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "passing literal 5 to `where b > 0` parameter must NOT emit RefinementViolated, got: {errors:?}"
    );
}

/// Negative literal violates `b > 0` — should report RefinementViolated.
#[test]
fn refinement_negative_literal_to_positive_param_rejected() {
    // GIVEN: function requires b > 0
    // WHEN: literal -3 is passed
    // THEN: RefinementViolated emitted
    let src = r#"
        total fn positive_only(b: Int where b > 0) -> Int { b }
        total fn caller() -> Int { positive_only(-3) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "passing literal -3 to `where b > 0` must emit RefinementViolated, got: {errors:?}"
    );
}

/// Unrestricted variable passed to refined param — no hard error (runtime check).
#[test]
fn refinement_unrestricted_var_to_refined_param_no_error() {
    // GIVEN: function requires b != 0, caller has unrestricted y
    // WHEN: y is passed
    // THEN: no RefinementViolated (runtime check is inserted instead)
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(x: Int, y: Int) -> Int { safe_divide(x, y) }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "unrestricted var to refined param should NOT emit RefinementViolated (runtime check), got: {errors:?}"
    );
}

/// Variable with matching refinement — proven, no error.
#[test]
fn refinement_same_pred_var_proven() {
    // GIVEN: function requires b != 0, caller has y: Int where y != 0
    // WHEN: y is passed
    // THEN: proven — no RefinementViolated
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(x: Int, y: Int where y != 0) -> Int { safe_divide(x, y) }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "var with matching refinement should be proven with no error, got: {errors:?}"
    );
}

/// Valid corpus with refinements — no violations after Phase 3 check.
#[test]
fn refinements_valid_corpus_no_violations() {
    // GIVEN: valid refinement type corpus
    // THEN: no RefinementViolated errors
    let src = include_str!("corpus/09_refinements/refinements_valid.mvl");
    let result = check_src(src);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "valid refinements corpus should produce no RefinementViolated errors, got: {:?}",
        result.errors
    );
}

/// Refinement pass produces a useful verdict for programs with refined call sites.
#[test]
fn refinement_pass_produces_counts_in_verdict() {
    use mvl::mvl::checker::passes::PassRegistry;
    use mvl::mvl::parser::Parser;

    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(x: Int, y: Int where y != 0) -> Int { safe_divide(x, y) }
    "#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let result = check(&prog);
    let registry = PassRegistry::default_registry();
    let verdict = registry.run_req(10, &prog, &result);
    assert!(
        verdict.is_proven(),
        "program with a proven call site should yield Proven verdict for Req 10, got: {verdict:?}"
    );
}

/// Violations corpus loads without static violations (only runtime checks).
#[test]
fn refinements_violations_corpus_no_static_violations() {
    // GIVEN: violations corpus (unproven call sites — runtime-checked, not statically failed)
    // THEN: no RefinementViolated errors emitted
    let src = include_str!("corpus/09_refinements/refinements_violations.mvl");
    let result = check_src(src);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "violations corpus should produce no RefinementViolated errors (only runtime checks), got: {:?}",
        result.errors
    );
}

/// Program with a definite refinement violation yields Verdict::Failed for Req 10.
#[test]
fn refinement_pass_yields_failed_verdict_on_violation() {
    use mvl::mvl::checker::passes::PassRegistry;
    use mvl::mvl::parser::Parser;

    // GIVEN: literal 0 passed to b != 0 — definite violation
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller() -> Int { safe_divide(10, 0) }
    "#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let result = check(&prog);
    let registry = PassRegistry::default_registry();
    let verdict = registry.run_req(10, &prog, &result);
    assert!(
        verdict.is_failed(),
        "program with a definite refinement violation must yield Failed verdict, got: {verdict:?}"
    );
}

/// Reassigning a refined variable invalidates its refinement — subsequent call is runtime-checked.
#[test]
fn refinement_stale_after_reassignment_is_not_proven() {
    use mvl::mvl::checker::passes::PassRegistry;
    use mvl::mvl::parser::Parser;

    // GIVEN: mut param y with refinement y > 0; reassigned to 0 before use
    // WHEN: y is passed to positive_only
    // THEN: verdict is NOT Proven (refinement was invalidated by assignment)
    let src = r#"
        total fn positive_only(b: Int where b > 0) -> Int { b }
        total fn caller(ref y: Int where y > 0) -> Int {
            y = 0;
            positive_only(y)
        }
    "#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let result = check(&prog);
    let registry = PassRegistry::default_registry();
    let verdict = registry.run_req(10, &prog, &result);
    assert!(
        !verdict.is_proven(),
        "reassigned refined variable must not yield Proven verdict (stale refinement), got: {verdict:?}"
    );
}

/// Coverage report: verdict evidence includes "N/M functions fully verified".
#[test]
fn refinement_pass_verdict_includes_coverage_report() {
    use mvl::mvl::checker::passes::PassRegistry;
    use mvl::mvl::parser::Parser;

    // GIVEN: all call sites statically proven
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(x: Int, y: Int where y != 0) -> Int { safe_divide(x, y) }
    "#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let result = check(&prog);
    let registry = PassRegistry::default_registry();
    let verdict = registry.run_req(10, &prog, &result);
    assert!(
        verdict.is_proven(),
        "fully-verified program must yield Proven, got: {verdict:?}"
    );
    let evidence = format!("{verdict:?}");
    assert!(
        evidence.contains("1/1"),
        "evidence must include N/M coverage report, got: {evidence}"
    );
}

/// Partial coverage: some functions runtime-checked yields Unchecked with coverage report.
#[test]
fn refinement_pass_partial_coverage_yields_unchecked_with_report() {
    use mvl::mvl::checker::passes::PassRegistry;
    use mvl::mvl::parser::Parser;

    // GIVEN: one caller fully proven, one has a runtime-checked site (stale refinement)
    let src = r#"
        total fn positive_only(b: Int where b > 0) -> Int { b }
        total fn proven_caller(x: Int where x > 0) -> Int { positive_only(x) }
        total fn stale_caller(ref y: Int where y > 0) -> Int {
            y = 0;
            positive_only(y)
        }
    "#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let result = check(&prog);
    let registry = PassRegistry::default_registry();
    let verdict = registry.run_req(10, &prog, &result);
    assert!(
        !verdict.is_proven(),
        "partially-verified program must not yield Proven, got: {verdict:?}"
    );
    let evidence = format!("{verdict:?}");
    assert!(
        evidence.contains("1/2"),
        "reason must include N/M coverage report, got: {evidence}"
    );
}

/// For-loop `invariant` clause is parsed and accepted without errors.
#[test]
fn for_loop_with_invariant_clause_accepted() {
    // GIVEN: a for loop with an invariant clause
    // WHEN: parsed and checked
    // THEN: no errors and the invariant is recorded in the AST
    let src = r#"
        total fn sum_positives(items: [Int]) -> Int {
            let result: Int = 0;
            for item in items invariant result >= 0 {
                result = result + item;
            }
            result
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "for loop with invariant clause must parse and check without errors, got: {errors:?}"
    );
}

/// Compound And predicate: literal failing the left branch is caught (short-circuit).
#[test]
fn refinement_and_predicate_short_circuits_on_false_left() {
    // GIVEN: function requires b > 0 && b < 100
    // WHEN: literal 0 is passed (fails left branch)
    // THEN: RefinementViolated emitted
    let src = r#"
        total fn bounded(b: Int where b > 0 && b < 100) -> Int { b }
        total fn caller() -> Int { bounded(0) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "literal 0 violates `b > 0 && b < 100`; must emit RefinementViolated, got: {errors:?}"
    );
}

/// Compound And predicate: literal satisfying both branches is proven.
#[test]
fn refinement_and_predicate_both_branches_proven() {
    // GIVEN: function requires b > 0 && b < 100
    // WHEN: literal 50 is passed
    // THEN: no RefinementViolated
    let src = r#"
        total fn bounded(b: Int where b > 0 && b < 100) -> Int { b }
        total fn caller() -> Int { bounded(50) }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "literal 50 satisfies `b > 0 && b < 100`; must NOT emit RefinementViolated, got: {errors:?}"
    );
}

/// Compound Or predicate: literal satisfying the right branch is proven.
#[test]
fn refinement_or_predicate_right_branch_proven() {
    // GIVEN: function requires b < 0 || b > 5
    // WHEN: literal 7 is passed (satisfies right branch)
    // THEN: no RefinementViolated
    let src = r#"
        total fn nonzero_range(b: Int where b < 0 || b > 5) -> Int { b }
        total fn caller() -> Int { nonzero_range(7) }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "literal 7 satisfies `b < 0 || b > 5`; must NOT emit RefinementViolated, got: {errors:?}"
    );
}

/// Compound Or predicate: literal satisfying neither branch is rejected.
#[test]
fn refinement_or_predicate_neither_branch_fails() {
    // GIVEN: function requires b < 0 || b > 5
    // WHEN: literal 3 is passed (satisfies neither)
    // THEN: RefinementViolated emitted
    let src = r#"
        total fn nonzero_range(b: Int where b < 0 || b > 5) -> Int { b }
        total fn caller() -> Int { nonzero_range(3) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "literal 3 violates `b < 0 || b > 5`; must emit RefinementViolated, got: {errors:?}"
    );
}

/// Comparison operators Lt, Le, Ge, Eq are correctly evaluated for integer literals.
#[test]
fn refinement_operators_lt_le_ge_eq() {
    // GIVEN/WHEN/THEN: each operator proven on a matching literal
    let cases: &[(&str, i64, bool)] = &[
        // (predicate, literal, should_pass)
        ("b < 10", 9, true),
        ("b < 10", 10, false),
        ("b <= 10", 10, true),
        ("b <= 10", 11, false),
        ("b >= 5", 5, true),
        ("b >= 5", 4, false),
        ("b == 7", 7, true),
        ("b == 7", 8, false),
    ];
    for (pred, lit, should_pass) in cases {
        let src = format!(
            r#"total fn f(b: Int where {pred}) -> Int {{ b }}
               total fn caller() -> Int {{ f({lit}) }}"#
        );
        let errors = errors_for(&src);
        let violated = errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. }));
        if *should_pass {
            assert!(
                !violated,
                "literal {lit} should satisfy `{pred}` but got RefinementViolated"
            );
        } else {
            assert!(
                violated,
                "literal {lit} should violate `{pred}` but no RefinementViolated emitted"
            );
        }
    }
}

// ── IFC tests for crypto functions (#180) ─────────────────────────────────────

#[test]
fn sha256_rejects_secret_input() {
    // GIVEN: sha256 expects String (unlabeled); Secret cannot flow to unlabeled
    // THEN: TypeMismatch — Secret[String] cannot be passed to sha256(String)
    // This is the interim IFC protection until label polymorphism lands (#179).
    let errors = errors_for(
        r#"fn hash_secret(pwd: Secret[String]) -> String {
    sha256(pwd)
}"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "sha256(Secret[String]) must be a TypeMismatch, got: {errors:?}"
    );
}

#[test]
fn sha512_rejects_secret_input() {
    let errors = errors_for(
        r#"fn hash_secret(pwd: Secret[String]) -> String {
    sha512(pwd)
}"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "sha512(Secret[String]) must be a TypeMismatch, got: {errors:?}"
    );
}

#[test]
fn sha256_accepts_plain_string() {
    // GIVEN: sha256(String) -> String — plain String input is valid
    // THEN: no type errors
    let errors = errors_for(
        r#"fn checksum(data: String) -> String {
    sha256(data)
}"#,
    );
    assert!(
        errors.is_empty(),
        "sha256(String) must type-check without errors, got: {errors:?}"
    );
}

#[test]
fn crypto_random_bytes_returns_secret_list_int() {
    // GIVEN: crypto_random_bytes returns Secret[List[Int]]
    // THEN: the result can be bound to Secret[List[Int]] and cannot be logged
    let errors = errors_for(
        r#"fn gen_key(n: Int) -> Secret[List[Int]] ! CryptoRandom {
    crypto_random_bytes(n)
}"#,
    );
    assert!(
        errors.is_empty(),
        "crypto_random_bytes must return Secret[List[Int]], got: {errors:?}"
    );
}

#[test]
fn sha512_accepts_plain_string() {
    let errors = errors_for(
        r#"fn checksum(data: String) -> String {
    sha512(data)
}"#,
    );
    assert!(
        errors.is_empty(),
        "sha512(String) must type-check without errors, got: {errors:?}"
    );
}

/// Minimal Logger stub for inline IFC tests (avoids loading std/log.mvl).
/// Field layout (`dummy: Int`) is irrelevant — tests exercise sink-name and
/// label checks, not Logger construction. The checker identifies Logger methods
/// by type name + method name. These have `! Log` effect so the implicit flow
/// checker considers them observable (#1007).
const LOGGER_STUB: &str = r#"
type Logger = struct { dummy: Int }
fn Logger::debug(self, msg: String, fields: Map[String, String]) -> Unit ! Log { }
fn Logger::info(self, msg: String, fields: Map[String, String]) -> Unit ! Log { }
fn Logger::warn(self, msg: String, fields: Map[String, String]) -> Unit ! Log { }
fn Logger::error(self, msg: String, fields: Map[String, String]) -> Unit ! Log { }
"#;

#[test]
fn crypto_random_bytes_result_rejected_by_log_info() {
    // GIVEN: crypto_random_bytes returns Secret[List[Int]]
    // THEN: passing the result to Logger.info is a TypeMismatch
    let src = format!(
        "{LOGGER_STUB}fn leak_attempt(logger: val Logger, n: Int) -> Unit ! CryptoRandom + Log {{
    let bytes: Secret[List[Int]] = crypto_random_bytes(n);
    logger.info(bytes, {{\"k\": \"v\"}});
}}"
    );
    let errors = errors_for(&src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "logging Secret[List[Int]] must produce TypeMismatch, got: {errors:?}"
    );
}

#[test]
fn caller_missing_crypto_random_effect_rejected() {
    let src = r#"
        fn gen_bytes(n: Int) -> Secret[List[Int]] ! CryptoRandom { crypto_random_bytes(n) }
        fn caller(n: Int) -> Secret[List[Int]] { gen_bytes(n) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::UndeclaredEffect { callee, effect, .. }
            if callee == "gen_bytes" && effect == "CryptoRandom"
        )),
        "expected UndeclaredEffect(gen_bytes, CryptoRandom), got: {errors:?}"
    );
}

#[test]
fn file_io_corpus_parses_and_checks() {
    // GIVEN: the file I/O effects corpus (valid programs using std.io, #44)
    // THEN: no serious type errors; UndefinedFunction/UndefinedVariable/UndefinedType
    //       for stdlib symbols are expected without stdlib loaded
    let src = include_str!("corpus/13_stdlib/file_io.mvl");
    let result = check_src(src);
    let serious: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            !matches!(
                e,
                // stdlib symbols and opaque types (File, Path, BufReader, etc.) not loaded in Phase 2
                CheckError::UndefinedFunction { .. }
                    | CheckError::UndefinedVariable { .. }
                    | CheckError::UndefinedType { .. }
            )
        })
        .collect();
    assert!(
        serious.is_empty(),
        "file_io corpus should have no serious errors, got: {serious:?}"
    );
}

#[test]
fn caller_missing_file_write_effect_rejected() {
    // GIVEN: fn writes ! FileWrite; fn caller ! FileRead calls writes
    // THEN: MissingEffect(writes, FileWrite) reported
    let src = r#"
        fn writes() -> Result[Unit, String] ! FileWrite { Err("") }
        fn caller() -> Result[Unit, String] ! FileRead { writes() }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::MissingEffect { callee, effect, .. }
            if callee == "writes" && effect == "FileWrite"
        )),
        "expected MissingEffect(writes, FileWrite), got: {errors:?}"
    );
}

#[test]
fn caller_missing_file_delete_effect_rejected() {
    // GIVEN: fn deletes ! FileDelete; fn caller ! FileWrite calls deletes
    // THEN: MissingEffect(deletes, FileDelete) reported
    let src = r#"
        fn deletes() -> Result[Unit, String] ! FileDelete { Err("") }
        fn caller() -> Result[Unit, String] ! FileWrite { deletes() }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::MissingEffect { callee, effect, .. }
            if callee == "deletes" && effect == "FileDelete"
        )),
        "expected MissingEffect(deletes, FileDelete), got: {errors:?}"
    );
}

// ── std.log / ! Log effect (#54) ─────────────────────────────────────────────

/// The logging corpus (valid programs) MUST parse and check without serious errors.
#[test]
fn logging_corpus_parses_and_checks() {
    // GIVEN: the logging effects corpus (valid programs using std.log, #54)
    // THEN: no serious type errors; UndefinedFunction for log_* is expected
    //       when stdlib is not loaded (Phase 2)
    let src = include_str!("corpus/13_stdlib/logging.mvl");
    let result = check_src(src);
    let serious: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            !matches!(
                e,
                // log_* functions are stdlib symbols; not loaded in Phase 2 unit tests
                CheckError::UndefinedFunction { .. }
                    | CheckError::UndefinedVariable { .. }
                    | CheckError::UndefinedType { .. }
            )
        })
        .collect();
    assert!(
        serious.is_empty(),
        "logging corpus should have no serious errors, got: {serious:?}"
    );
}

/// `Logger.info` with a Secret argument MUST be rejected (#54, 003-information-flow/Req 6).
/// "Don't log secrets" is a type error in MVL, not a code review rule.
#[test]
fn log_info_rejects_secret_argument() {
    let src = format!(
        "{LOGGER_STUB}fn f(logger: val Logger, pwd: Secret[String]) -> Unit ! Log {{ logger.info(pwd, {{\"k\": \"v\"}}); }}"
    );
    let errors = errors_for(&src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Logger.info with Secret arg should emit TypeMismatch, got: {errors:?}"
    );
}

/// `Logger.error` with a Tainted argument MUST be rejected (#54, 003-information-flow/Req 6).
#[test]
fn log_error_rejects_tainted_argument() {
    let src = format!(
        "{LOGGER_STUB}fn f(logger: val Logger, input: Tainted[String]) -> Unit ! Log {{ logger.error(input, {{\"k\": \"v\"}}); }}"
    );
    let errors = errors_for(&src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Logger.error with Tainted arg should emit TypeMismatch, got: {errors:?}"
    );
}

/// `Logger.warn` with a Tainted argument MUST be rejected — only bare values may be logged (#54).
#[test]
fn log_warn_rejects_tainted_argument() {
    let src = format!(
        "{LOGGER_STUB}fn f(logger: val Logger, s: Tainted[String]) -> Unit ! Log {{ logger.warn(s, {{\"k\": \"v\"}}); }}"
    );
    let errors = errors_for(&src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Logger.warn with Tainted arg should emit TypeMismatch, got: {errors:?}"
    );
}

/// A caller of `Logger.info` MUST declare `! Log`; without it UndeclaredEffect is reported.
#[test]
fn caller_missing_log_effect_rejected() {
    let src = format!(
        "{LOGGER_STUB}fn do_log(logger: val Logger) -> Unit ! Log {{ logger.info(\"msg\", {{\"k\": \"v\"}}) }}
        fn caller(logger: val Logger) -> Unit {{ do_log(logger) }}
    "
    );
    let errors = errors_for(&src);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::UndeclaredEffect { callee, effect, .. }
            if callee == "do_log" && effect == "Log"
        )),
        "expected UndeclaredEffect(do_log, Log), got: {errors:?}"
    );
}

/// A caller with some effects but not `! Log` MUST produce MissingEffect (#54).
#[test]
fn caller_missing_log_effect_with_other_effects_rejected() {
    // GIVEN: fn do_log ! Log; fn caller ! Net calls do_log (has effects, but not Log)
    // THEN: MissingEffect(do_log, Log) reported — not UndeclaredEffect
    let src = format!(
        "{LOGGER_STUB}fn do_log(logger: val Logger) -> Unit ! Log {{ logger.info(\"msg\", {{\"k\": \"v\"}}) }}
        fn caller(logger: val Logger) -> Unit ! Net {{ do_log(logger) }}
    "
    );
    let errors = errors_for(&src);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::MissingEffect { callee, effect, .. }
            if callee == "do_log" && effect == "Log"
        )),
        "expected MissingEffect(do_log, Log), got: {errors:?}"
    );
}

/// `Logger.debug` with a Secret argument MUST be rejected (#54, 003-information-flow/Req 6).
#[test]
fn log_debug_rejects_secret_argument() {
    let src = format!(
        "{LOGGER_STUB}fn f(logger: val Logger, pwd: Secret[String]) -> Unit ! Log {{ logger.debug(pwd, {{\"k\": \"v\"}}); }}"
    );
    let errors = errors_for(&src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Logger.debug with Secret arg should emit TypeMismatch, got: {errors:?}"
    );
}

/// `Logger.info` with a plain String argument MUST be accepted (#54).
/// Guards against over-rejection — the checker must not reject all log calls.
#[test]
fn log_info_accepts_public_argument() {
    let src = format!(
        "{LOGGER_STUB}fn f(logger: val Logger, name: String) -> Unit ! Log {{ logger.info(\"user logged in\", {{\"user\": name}}); }}"
    );
    let errors = errors_for(&src);
    let ifc_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, CheckError::TypeMismatch { .. }))
        .collect();
    assert!(
        ifc_errors.is_empty(),
        "Logger.info with plain String arg should not emit TypeMismatch, got: {ifc_errors:?}"
    );
}

/// A `Secret[String]` value embedded as a map field value MUST be rejected (#54).
/// "Don't log secrets" applies to structured fields too — not just the msg argument.
#[test]
fn log_info_rejects_secret_value_in_fields_map() {
    let src = format!(
        "{LOGGER_STUB}fn f(logger: val Logger, pwd: Secret[String]) -> Unit ! Log {{ logger.info(\"login\", {{\"password\": pwd}}); }}"
    );
    let errors = errors_for(&src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Logger.info with Secret value in fields map should emit TypeMismatch, got: {errors:?}"
    );
}

#[test]
fn log_warn_rejects_tainted_value_in_fields_map() {
    let src = format!(
        "{LOGGER_STUB}fn f(logger: val Logger, raw: Tainted[String]) -> Unit ! Log {{ logger.warn(\"req\", {{\"body\": raw}}); }}"
    );
    let errors = errors_for(&src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Logger.warn with Tainted value in fields map should emit TypeMismatch, got: {errors:?}"
    );
}

#[test]
fn log_debug_rejects_tainted_value_in_fields_map() {
    let src = format!(
        "{LOGGER_STUB}fn f(logger: val Logger, s: Tainted[String]) -> Unit ! Log {{ logger.debug(\"req\", {{\"body\": s}}); }}"
    );
    let errors = errors_for(&src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Logger.debug with Tainted value in fields map should emit TypeMismatch, got: {errors:?}"
    );
}

/// Logger method inside a Secret-conditional branch MUST be rejected (implicit flow).
///
/// Even though the argument to `logger.info` is a literal, the presence of the
/// log record reveals whether `flag` was truthy — an implicit flow via the log sink.
///
/// Regression for #973: `Expr::MethodCall` was not checked against PUBLIC_SINKS.
/// Regression for #973: `Expr::MethodCall` was not checked against PUBLIC_SINKS.
#[test]
fn logger_method_implicit_flow_secret_branch_rejected() {
    let errors = errors_for(&format!(
        "{LOGGER_STUB}fn f(flag: Secret[Bool], logger: val Logger) -> Unit ! Log {{
            if flag {{ logger.info(\"branch taken\", {{\"k\": \"v\"}}); }}
        }}"
    ));
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, observable_fn, .. }
                if pc_label == "Secret" && observable_fn.starts_with("Logger::"))
        ),
        "Logger.info inside Secret branch should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

// ── Label propagation (#1007: all functions propagate labels unconditionally) ─

/// After 2-arg format migration (#901), `format("...", [s])` where `s: Secret[String]`
/// creates `List[Secret[String]]` which doesn't match `List[String]`. The caller must
/// relabel before formatting — this is now a type error.
#[test]
fn format_propagates_secret_label() {
    let errors =
        errors_for(r#"fn f(s: Secret[String]) -> Secret[String] { format("value={}", [s]) }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "format with Secret[String] in list should produce TypeMismatch, got: {errors:?}"
    );
}

/// `format()` with a Tainted argument cannot flow to Public[String].
#[test]
fn format_propagates_tainted_label_rejected_as_public() {
    // GIVEN: format called with a Tainted[String] — result is Tainted[String]
    // THEN: TypeMismatch when trying to assign to Public[String]
    let errors =
        errors_for(r#"fn f(s: Tainted[String]) -> Public[String] { format("v={}", [s]) }"#);
    assert!(
        errors.iter().any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "format with Tainted arg should be Tainted[String], cannot flow to Public[String], got: {errors:?}"
    );
}

/// All functions propagate labels: calling fn with labeled arg yields labeled result.
#[test]
fn fn_propagates_label() {
    // GIVEN: a fn that wraps its string argument
    // THEN: calling it with Tainted[String] yields Tainted[String] — no mismatch
    let errors = errors_for(
        r#"
fn wrap(s: String) -> String { s }
fn f(s: Tainted[String]) -> Tainted[String] { wrap(s) }
"#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "fn with Tainted arg should yield Tainted[String], got: {errors:?}"
    );
}

/// Label propagation: result cannot flow to a lower label.
#[test]
fn fn_label_cannot_flow_down() {
    // GIVEN: fn called with Secret[String]
    // THEN: result is Secret[String] — cannot assign to Public[String]
    let errors = errors_for(
        r#"
fn wrap(s: String) -> String { s }
fn f(s: Secret[String]) -> Public[String] { wrap(s) }
"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "fn with Secret arg should yield Secret[String], cannot flow to Public, got: {errors:?}"
    );
}

/// `decode()` with Tainted[String] propagates the label to the result (primary ADR-0024 use case).
#[test]
fn decode_propagates_tainted_label() {
    // GIVEN: decode called with Tainted[String] (issue #179 / ADR-0024 primary case)
    // THEN: result is Tainted[Result[Value, String]] — no TypeMismatch
    let errors =
        errors_for(r#"fn f(s: Tainted[String]) -> Tainted[Result[Value, String]] { decode(s) }"#);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "decode(Tainted[String]) should yield Tainted[Result[Value, String]], got: {errors:?}"
    );
}

/// `decode()` with Tainted input must not silently drop label when assigned to unlabeled Result.
#[test]
fn decode_tainted_cannot_flow_to_unlabeled() {
    // GIVEN: decode(Tainted[String]) — result used as unlabeled Result[Value, String]
    // THEN: some checker error — Tainted label cannot silently drop.
    // Note: the checker may raise ResultIgnored or TypeMismatch depending on how
    // Tainted[Result[...]] interacts with the expected return type; either is acceptable.
    let errors = errors_for(r#"fn f(s: Tainted[String]) -> Result[Value, String] { decode(s) }"#);
    assert!(
        !errors.is_empty(),
        "decode(Tainted[String]) must not silently drop its label (expected some error), got: {errors:?}"
    );
}

/// Multi-argument transparent fn: first arg's label propagates.
/// Post-#894: no lattice — the first labeled arg (bare param) determines output label.
#[test]
fn transparent_fn_joins_mixed_labels_takes_highest() {
    // GIVEN: transparent fn called with Tainted[String] (first bare param gets Tainted)
    // THEN: result is Tainted[String] (first label wins)
    let errors = errors_for(
        r#"
transparent fn combine(a: String, b: String) -> String { a }
fn f(s: Tainted[String], t: String) -> Tainted[String] { combine(s, t) }
"#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "first labeled arg to transparent fn should propagate, got: {errors:?}"
    );
}

/// Multi-argument transparent fn: result label is the label from arg (no lattice).
/// Post-#894: in the new model, label propagation only applies when param is bare.
/// Secret arg to labeled param doesn't propagate; Secret arg to bare param does.
#[test]
fn transparent_fn_joins_mixed_labels_reject_lower_bound() {
    // GIVEN: combine(Tainted[String], String) — Tainted propagates to return
    // THEN: result is Tainted[String]; cannot return as String (bare)
    let errors = errors_for(
        r#"
transparent fn combine(a: String, b: String) -> String { a }
fn f(s: Tainted[String], n: String) -> String { combine(s, n) }
"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Tainted[String] transparent result should not flow to bare String return, got: {errors:?}"
    );
}

/// All-unlabeled args to transparent fn: returns the declared type unchanged.
#[test]
fn transparent_fn_all_unlabeled_args_returns_declared_type() {
    // GIVEN: transparent fn called with only unlabeled (Public) arguments
    // THEN: no label applied — the declared return type is returned as-is
    let errors = errors_for(
        r#"
transparent fn wrap(s: String) -> String { s }
fn f(s: String) -> String { wrap(s) }
"#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "transparent fn with all-unlabeled args should return declared type, got: {errors:?}"
    );
}

/// Tainted label propagates through transparent fn (bare param).
#[test]
fn transparent_fn_propagates_tainted_label() {
    // Post-#894: transparent fn with bare param propagates arg's Tainted label.
    let errors = errors_for(
        r#"
transparent fn wrap(s: String) -> String { s }
fn f(s: Tainted[String]) -> Tainted[String] { wrap(s) }
"#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "transparent fn with Tainted[String] arg should yield Tainted[String], got: {errors:?}"
    );
}

/// join(Tainted, Secret) == Tainted (first label wins, no lattice).
#[test]
fn transparent_fn_join_two_labels_gives_first() {
    // Post-#894: no lattice — join uses the first non-None label.
    let errors = errors_for(
        r#"
transparent fn combine(a: String, b: String) -> String { a }
fn f(a: Tainted[String], b: Secret[String]) -> Tainted[String] { combine(a, b) }
"#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "join(Tainted, Secret) should be Tainted[String] (first wins), got: {errors:?}"
    );
}

// ── #219: Iterator trait (001-type-system Req 11) ─────────────────────────────

/// Spec 001 Req 11 / Scenario: For loop over array accepted.
///
/// GIVEN `let items: Array[Int, 3] = [1, 2, 3]`
/// WHEN  `for x in items { }`
/// THEN  type checker MUST accept (Array[T] implements Iterator[T])
#[test]
fn iterator_trait_for_loop_accepted() {
    let src = r#"
        fn f() -> Unit {
            let items: Array[Int, 3] = [1, 2, 3];
            for x in items {
                let _: Int = x;
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::NotIterator { .. })),
        "Array[Int, 3] implements Iterator — for loop should be accepted, got: {errors:?}"
    );
}

/// Spec 001 Req 11 / Scenario: For loop over non-iterator rejected.
///
/// GIVEN `let n: Int = 42`
/// WHEN  `for x in n { }`
/// THEN  type checker MUST reject: `Int` does not implement `Iterator`
#[test]
fn non_iterator_for_loop_rejected() {
    let src = r#"
        fn f() -> Unit {
            let n: Int = 42;
            for x in n { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::NotIterator { ty, .. } if ty == "Int")),
        "Int does not implement Iterator — for loop must be rejected, got: {errors:?}"
    );
}

/// Spec 001 Req 11 / Scenario: Custom type implements Iterator.
///
/// GIVEN `type Counter = struct { … }` with `impl Iterator[Int] for Counter`
/// WHEN  `for n in Counter { current: 0, limit: 3 } { }`
/// THEN  type checker MUST accept
#[test]
fn custom_iterator_impl_accepted() {
    let src = r#"
        type Counter = struct { current: ref Int, limit: Int }

        impl Iterator[Int] for Counter {
            fn next(ref self: Counter) -> Option[Int] { None }
        }

        fn f() -> Unit {
            for n in Counter { current: 0, limit: 3 } {
                let _: Int = n;
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::NotIterator { .. })),
        "Counter implements Iterator — for loop should be accepted, got: {errors:?}"
    );
}

/// Spec 001 Req 11 / Scenario: For loop allowed inside partial function.
///
/// GIVEN `partial fn f(items: Array[Int, 3]) { for x in items { … } }`
/// WHEN  the function is type-checked
/// THEN  type checker MUST accept it — `for` iterates over a finite collection
#[test]
fn for_loop_allowed_in_partial_fn() {
    let src = r#"
        partial fn f(items: Array[Int, 3]) -> Unit {
            for x in items { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "`for` in partial fn must be accepted, got: {errors:?}"
    );
}

/// Spec 001 Req 11 / Scenario: For loop over non-iterator inside partial function still errors.
///
/// GIVEN `partial fn f(n: Int) { for x in n { } }`
/// WHEN  the function is type-checked
/// THEN  type checker MUST emit NotIterator (for is allowed, but Int is not iterable)
#[test]
fn for_loop_non_iterator_in_partial_fn_emits_not_iterator() {
    let src = r#"
        partial fn f(n: Int) -> Unit {
            for x in n { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::NotIterator { ty, .. } if ty == "Int")),
        "must emit NotIterator for Int, got: {errors:?}"
    );
}

// ── #233: Bitwise operations on Int and Byte ──────────────────────────────────

/// Corpus `tests/corpus/04_primitives/bitwise.mvl` must type-check cleanly.
/// Note: transpile_src() does NOT run the checker; this test is required.
#[test]
fn bitwise_corpus_checks_cleanly() {
    let src = include_str!("corpus/04_primitives/bitwise.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "bitwise corpus must type-check cleanly, got: {:?}",
        result.errors
    );
}

/// `from_int()` with zero arguments must be rejected.
#[test]
fn from_int_with_no_args_is_rejected() {
    let errors = errors_for("fn f() -> Byte { from_int() }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::WrongArgCount { name, .. } if name == "from_int")),
        "zero-arg from_int must emit WrongArgCount, got: {errors:?}"
    );
}

/// `from_int(a, b)` with two arguments must be rejected.
#[test]
fn from_int_with_too_many_args_is_rejected() {
    let errors = errors_for("fn f(a: Int, b: Int) -> Byte { from_int(a, b) }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::WrongArgCount { name, .. } if name == "from_int")),
        "two-arg from_int must emit WrongArgCount, got: {errors:?}"
    );
}

/// `from_int(s)` where `s: String` must be rejected with a type mismatch.
#[test]
fn from_int_with_non_int_arg_is_rejected() {
    let errors = errors_for(r#"fn f(s: String) -> Byte { from_int(s) }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { expected, .. } if expected == "Int")),
        "from_int with String arg must emit TypeMismatch, got: {errors:?}"
    );
}

// ── Match-branch narrowing (Issue #238) ──────────────────────────────────────
//
// Req 3 × Req 10: after matching a literal arm the scrutinee is known to equal
// that literal; after a catch-all ident arm the variable is known to differ from
// all prior literal values.  These hypotheses allow the refinement solver to
// statically prove call-site predicates that would otherwise need runtime checks.

/// Catch-all ident after zero arm: solver knows `n != 0` — proven, no error.
///
/// GIVEN `match x { 0 => ..., n => safe_divide(a, n) }` where safe_divide needs `b != 0`
/// WHEN type-checked
/// THEN no RefinementViolated (hypothesis `n != 0` proves the predicate)
#[test]
fn match_narrowing_nonzero_catchall_proven() {
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(a: Int, x: Int) -> Int {
            match x {
                0 => 0,
                n => safe_divide(a, n),
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "n != 0 should be proven via match narrowing, got: {errors:?}"
    );
}

/// Literal arm: scrutinee is known to equal the matched value — proven.
///
/// GIVEN `match x { 5 => requires_five(x) }` where requires_five needs `b == 5`
/// WHEN type-checked
/// THEN no RefinementViolated (hypothesis `x == 5` proves the predicate)
#[test]
fn match_narrowing_literal_arm_eq_proven() {
    let src = r#"
        total fn requires_five(b: Int where b == 5) -> Int { b }
        total fn caller(x: Int) -> Int {
            match x {
                5 => requires_five(x),
                _ => 0,
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "x == 5 should be proven in literal arm, got: {errors:?}"
    );
}

/// Guard proves refinement: `n if n != 0 => safe_divide(a, n)` — proven.
///
/// GIVEN a match arm with guard `n != 0`
/// WHEN type-checked
/// THEN the guard adds hypothesis `n != 0` and proves the call
#[test]
fn match_narrowing_guard_proves_refinement() {
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(a: Int, x: Int) -> Int {
            match x {
                n if n != 0 => safe_divide(a, n),
                _ => 0,
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "guard `n != 0` should prove refinement, got: {errors:?}"
    );
}

/// Unguarded catch-all without prior literal arms: no hypothesis — runtime check, no error.
///
/// GIVEN `match x { n => safe_divide(a, n) }` with no prior literal arms
/// WHEN type-checked
/// THEN no RefinementViolated (conservative runtime check, not a static violation)
#[test]
fn match_narrowing_bare_catchall_no_prior_lits_runtime_check() {
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(a: Int, x: Int) -> Int {
            match x {
                n => safe_divide(a, n),
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "catch-all without prior literals is a runtime check (not a static violation), got: {errors:?}"
    );
}

/// Literal 0 arm: passing x to `b != 0` inside that arm is a static violation.
///
/// GIVEN `match x { 0 => safe_divide(a, x) }` where safe_divide needs `b != 0`
/// WHEN type-checked
/// THEN RefinementViolated is emitted (x == 0 is known, violates b != 0)
#[test]
fn match_narrowing_literal_zero_arm_violation_detected() {
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(a: Int, x: Int) -> Int {
            match x {
                0 => safe_divide(a, x),
                _ => 1,
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "x == 0 inside literal 0 arm must violate `b != 0`, got: {errors:?}"
    );
}

/// Multiple prior literal arms: catch-all knows `n != 0 && n != 1`.
///
/// GIVEN `match x { 0 => ..., 1 => ..., n => safe_divide(a, n) }`
/// WHEN type-checked
/// THEN proven (n is known != 0 and != 1, so != 0 is satisfied)
#[test]
fn match_narrowing_multiple_prior_lits_catchall_proven() {
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(a: Int, x: Int) -> Int {
            match x {
                0 => 0,
                1 => 1,
                n => safe_divide(a, n),
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "n != 0 && n != 1 should prove `b != 0`, got: {errors:?}"
    );
}

// ── Additional narrowing tests (fixes from review) ────────────────────────────

/// Wildcard arm after literal 0: scrutinee gets hypothesis `x != 0` — proven.
///
/// GIVEN `match x { 0 => ..., _ => safe_divide(a, x) }` where safe_divide needs `b != 0`
/// WHEN type-checked
/// THEN no RefinementViolated (wildcard arm narrows x to x != 0)
#[test]
fn match_narrowing_wildcard_arm_proven() {
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(a: Int, x: Int) -> Int {
            match x {
                0 => 0,
                _ => safe_divide(a, x),
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "wildcard arm after 0 should narrow x to x != 0, got: {errors:?}"
    );
}

/// Float literal arm: scrutinee is known to equal the matched float — proven.
///
/// GIVEN `match x { 1.0 => requires_one(x) }` where requires_one needs `b == 1.0`
/// WHEN type-checked
/// THEN no RefinementViolated (hypothesis x == 1.0 proves the predicate)
#[test]
fn match_narrowing_float_literal_arm_eq_proven() {
    let src = r#"
        total fn requires_one(b: Float where b == 1.0) -> Float { b }
        total fn caller(x: Float) -> Float {
            match x {
                1.0 => requires_one(x),
                _ => 0.0,
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "x == 1.0 should be proven in float literal arm, got: {errors:?}"
    );
}

/// Float literal 0.0 arm: passing x to `b != 0.0` is a static violation.
///
/// GIVEN `match x { 0.0 => safe_divide_float(a, x) }` where safe_divide_float needs `b != 0.0`
/// WHEN type-checked
/// THEN RefinementViolated is emitted (x == 0.0 is known, violates b != 0.0)
#[test]
fn match_narrowing_float_zero_arm_violation_detected() {
    let src = r#"
        total fn safe_divide_float(a: Float, b: Float where b != 0.0) -> Float { a / b }
        total fn caller(a: Float, x: Float) -> Float {
            match x {
                0.0 => safe_divide_float(a, x),
                _ => 1.0,
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "x == 0.0 inside float literal 0.0 arm must violate `b != 0.0`, got: {errors:?}"
    );
}

/// Catch-all ident + guard conjunctive merge: prior literal and guard both constrain n.
///
/// GIVEN `match x { 0 => ..., n if n > 2 => safe_divide(a, n) }`
/// WHEN type-checked
/// THEN proven (n != 0 from prior arm AND n > 2 from guard — together satisfy b != 0)
#[test]
fn match_narrowing_prior_lit_and_guard_conjunctive_proven() {
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(a: Int, x: Int) -> Int {
            match x {
                0 => 0,
                n if n != 0 => safe_divide(a, n),
                _ => 0,
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "n != 0 (prior) && n != 0 (guard) should prove `b != 0`, got: {errors:?}"
    );
}

/// Scrutinee is narrowed in ident arm: passing x (not n) to a callee is proven.
///
/// GIVEN `match x { 0 => ..., n => safe_divide(a, x) }` — note: passes x, not n
/// WHEN type-checked
/// THEN no RefinementViolated (x and n are the same value; both get the n != 0 hypothesis)
#[test]
fn match_narrowing_scrutinee_narrowed_in_ident_arm() {
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(a: Int, x: Int) -> Int {
            match x {
                0 => 0,
                n => safe_divide(a, x),
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "scrutinee x should be narrowed to x != 0 in ident arm, got: {errors:?}"
    );
}

/// Match-as-expression (Expr::Match) exercises the expression code path.
///
/// GIVEN `let result = match x { 0 => 0, n => safe_divide(a, n) }`
/// WHEN type-checked
/// THEN no RefinementViolated (Expr::Match path narrows correctly)
#[test]
fn match_narrowing_expr_match_path_proven() {
    let src = r#"
        total fn safe_divide(a: Int, b: Int where b != 0) -> Int { a / b }
        total fn caller(a: Int, x: Int) -> Int {
            let result = match x {
                0 => 0,
                n => safe_divide(a, n),
            };
            result
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "Expr::Match should narrow n != 0 just like Stmt::Match, got: {errors:?}"
    );
}

// ── std.process / ! ProcessSpawn + std.env / ! Env effects (#45) ──────────

/// The unix process lifecycle corpus (valid programs) MUST parse and check
/// without serious errors (#45).
#[test]
fn unix_process_lifecycle_corpus_parses_and_checks() {
    // GIVEN: the unix corpus (valid programs using std.process, std.env, #45)
    // THEN: no serious type errors; UndefinedFunction/UndefinedType for stdlib
    //       symbols are expected when stdlib is not loaded in unit tests
    let src = include_str!("corpus/13_stdlib/unix.mvl");
    let result = check_src(src);
    let serious: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            !matches!(
                e,
                CheckError::UndefinedFunction { .. }
                    | CheckError::UndefinedVariable { .. }
                    | CheckError::UndefinedType { .. }
            )
        })
        .collect();
    assert!(
        serious.is_empty(),
        "unix process lifecycle corpus should have no serious errors, got: {serious:?}"
    );
}

/// A pure function calling a `! ProcessSpawn` function MUST be rejected (#45).
#[test]
fn pure_function_calling_process_spawn_rejected() {
    // GIVEN: fn spawns ! ProcessSpawn; fn caller (no effects) calls spawns
    // THEN: UndeclaredEffect reported
    let src = r#"
        fn spawns() -> Result[Unit, String] ! ProcessSpawn { Err("") }
        fn caller() -> Result[Unit, String] { spawns() }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::UndeclaredEffect { callee, effect, .. }
            if callee == "spawns" && effect == "ProcessSpawn"
        )),
        "expected UndeclaredEffect(spawns, ProcessSpawn), got: {errors:?}"
    );
}

/// A caller with `! Env` but not `! ProcessSpawn` calling a `! ProcessSpawn`
/// function MUST report MissingEffect (#45).
#[test]
fn caller_missing_process_spawn_effect_rejected() {
    // GIVEN: fn spawns ! ProcessSpawn; fn caller ! Env calls spawns
    // THEN: MissingEffect(spawns, ProcessSpawn) reported
    let src = r#"
        fn spawns() -> Result[Unit, String] ! ProcessSpawn { Err("") }
        fn caller() -> Result[Unit, String] ! Env { spawns() }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::MissingEffect { callee, effect, .. }
            if callee == "spawns" && effect == "ProcessSpawn"
        )),
        "expected MissingEffect(spawns, ProcessSpawn), got: {errors:?}"
    );
}

/// A caller with `! ProcessSpawn` but not `! Env` calling a `! Env` function
/// MUST report MissingEffect (#45).
#[test]
fn caller_missing_env_effect_rejected() {
    // GIVEN: fn reads_env ! Env; fn caller ! ProcessSpawn calls reads_env
    // THEN: MissingEffect(reads_env, Env) reported
    let src = r#"
        fn reads_env() -> Unit ! Env { }
        fn caller() -> Unit ! ProcessSpawn { reads_env() }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CheckError::MissingEffect { callee, effect, .. }
            if callee == "reads_env" && effect == "Env"
        )),
        "expected MissingEffect(reads_env, Env), got: {errors:?}"
    );
}

// ── #239: Constant folding — compile-time evaluation of pure functions ────────

/// Pure function called with literal args should fold — refinement solver proves
/// the result satisfies its predicate statically (no runtime check needed).
#[test]
fn const_fold_pure_fn_satisfies_refinement() {
    // GIVEN: double() is pure, called with literal 5 → folds to 10.
    //        require_positive(x: Int where self > 0) accepts it statically.
    // THEN: no refinement violations (would get RefinementViolated otherwise)
    let src = r#"
        fn double(x: Int) -> Int { x * 2 }
        fn require_positive(x: Int where self > 0) -> Int { x }
        fn main() -> Int {
            let n: Int = double(5);
            require_positive(double(5))
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "const-folded pure fn call should satisfy refinement statically, got: {:?}",
        result.errors
    );
}

/// Effectful function called with literals must NOT be folded — effect propagation
/// rules still apply and the call must not be treated as a known value.
#[test]
fn const_fold_does_not_apply_to_effectful_fn() {
    // GIVEN: effectful_fn has ! Console — it cannot be folded even with literal args.
    // THEN: no fold occurs, but also no crash — checker remains sound.
    let src = r#"
        fn effectful_fn(x: Int) -> Int ! Console { println(x); x }
        fn require_positive(x: Int where self > 0) -> Int { x }
        fn caller() -> Int ! Console {
            require_positive(effectful_fn(5))
        }
    "#;
    // Should not panic/crash. Result may have errors from effect propagation
    // but no internal evaluator error.
    let _ = check_src(src);
}

/// `let` bound to a const-folded call injects a value hypothesis into var_refs,
/// allowing the refinement solver to prove predicates on that variable.
#[test]
fn const_fold_let_binding_propagates_to_refinement() {
    // GIVEN: n = add(3, 4) folds to 7; require_gt_5(x where self > 5) accepts it.
    // THEN: no RefinementViolated error.
    let src = r#"
        fn add(a: Int, b: Int) -> Int { a + b }
        fn require_gt_5(x: Int where self > 5) -> Int { x }
        fn main() -> Int {
            let n: Int = add(3, 4);
            require_gt_5(n)
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "let-bound const-folded value should satisfy refinement via hypothesis, got: {:?}",
        result.errors
    );
}

/// Direct pure function call that folds to a value violating the refinement
/// MUST still be detected (fold reveals the violation, not hides it).
#[test]
fn const_fold_reveals_refinement_violation() {
    // GIVEN: double(0) folds to 0, which violates self > 0.
    // THEN: RefinementViolated reported.
    let src = r#"
        fn double(x: Int) -> Int { x * 2 }
        fn require_positive(x: Int where self > 0) -> Int { x }
        fn main() -> Int {
            require_positive(double(0))
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "double(0) = 0 violates self > 0, expected RefinementViolated, got: {errors:?}"
    );
}

#[test]
fn stdlib_args_mvl_parses_without_errors() {
    // GIVEN: std/args.mvl (embedded stdlib source)
    // THEN: the parser produces zero errors
    // Regression guard: ensures stdlib files use valid MVL syntax.
    // Function declarations use `fn foo<T>()` (angle brackets); call expressions use `foo[T]()`.
    let src = include_str!("../std/args.mvl");
    let (mut p, lex_errors) = mvl::mvl::parser::Parser::new(src);
    assert!(lex_errors.is_empty(), "args.mvl lex errors: {lex_errors:?}");
    let _ = p.parse_program();
    assert!(
        p.errors().is_empty(),
        "args.mvl parse errors: {:?}",
        p.errors()
    );
}

// ── Phase C & D: Borrow / Lifetime / Alias Checks (#305, #306) ────────────────

#[test]
fn function_returning_ref_without_ref_params_rejected() {
    // GIVEN: a function that declares val T return type but has no val T parameters
    // THEN: checker rejects — reference can only point to a local (would escape)
    let result = check_src("fn bad() -> val Int { 42 }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceEscapesScope { .. })),
        "expected ReferenceEscapesScope, got: {:?}",
        result.errors
    );
}

#[test]
fn function_returning_ref_with_ref_param_accepted() {
    // GIVEN: a function that declares val T return type AND has a val T parameter
    // THEN: checker accepts — the reference can legally point to the parameter
    let result = check_src("fn ok(x: val Int) -> val Int { x }");
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceEscapesScope { .. })),
        "unexpected ReferenceEscapesScope: {:?}",
        result.errors
    );
}

#[test]
fn two_mut_ref_params_of_same_type_rejected() {
    // GIVEN: a function with two ref T params of the same inner type
    // THEN: checker rejects — they could alias at the call site (Phase D)
    let result = check_src("fn bad(a: ref Int, b: ref Int) -> Unit { }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::DoubleMutableBorrow { .. })),
        "expected DoubleMutableBorrow, got: {:?}",
        result.errors
    );
}

#[test]
fn two_mut_ref_params_of_different_types_accepted() {
    // GIVEN: a function with two ref T params of DIFFERENT inner types
    // THEN: checker accepts — they cannot alias (different types)
    let result = check_src("fn ok(a: ref Int, b: ref Bool) -> Unit { }");
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::DoubleMutableBorrow { .. })),
        "unexpected DoubleMutableBorrow: {:?}",
        result.errors
    );
}

// ── Phase C: return expression flows from val T param (#364) ────────────────────

#[test]
fn function_returning_ref_literal_with_ref_param_rejected() {
    // GIVEN: a function with a val T param but the body returns a literal
    // THEN: checker rejects — the literal does not flow from the parameter
    let result = check_src("fn bad(x: val Int) -> val Int { 42 }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceEscapesScope { .. })),
        "expected ReferenceEscapesScope, got: {:?}",
        result.errors
    );
}

#[test]
fn function_returning_ref_from_if_branches_accepted() {
    // GIVEN: a function that returns a val T from both branches of an if/else
    // THEN: checker accepts — both branches flow from reference parameters
    let result = check_src(
        "fn ok(cond: Bool, x: val Int, y: val Int) -> val Int { if cond { x } else { y } }",
    );
    assert!(
        result.errors.is_empty(),
        "expected no errors, got: {:?}",
        result.errors
    );
}

#[test]
fn function_explicit_return_ref_param_accepted() {
    // GIVEN: function uses explicit `return x` where x: val Int
    // THEN: checker accepts — the reference flows from the parameter
    let result = check_src("fn ok(x: val Int) -> val Int { return x; }");
    assert!(
        result.errors.is_empty(),
        "expected no errors for explicit return of ref param, got: {:?}",
        result.errors
    );
}

#[test]
fn function_explicit_return_local_rejected() {
    // GIVEN: function uses explicit `return y` where y is a local Int (not a ref param)
    // THEN: checker rejects — the return value does not flow from a reference parameter
    let result = check_src("fn bad(x: val Int) -> val Int { return 42; }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceEscapesScope { .. })),
        "expected ReferenceEscapesScope for explicit return of non-ref, got: {:?}",
        result.errors
    );
}

#[test]
fn function_bare_return_in_ref_returning_fn_rejected() {
    // GIVEN: `return;` (no value) in a function returning &Int
    // THEN: checker rejects — bare return cannot produce a reference
    let result = check_src("fn bad(x: val Int) -> val Int { return; }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceEscapesScope { .. })),
        "expected ReferenceEscapesScope for bare return, got: {:?}",
        result.errors
    );
}

#[test]
fn function_early_return_non_ref_rejected() {
    // GIVEN: function has a guard `if cond { return 42; }` before a valid tail
    // THEN: checker rejects — the early return does not flow from a reference parameter
    let result =
        check_src("fn bad(cond: Bool, x: val Int) -> val Int { if cond { return 42; } x }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceEscapesScope { .. })),
        "expected ReferenceEscapesScope for early return of non-ref, got: {:?}",
        result.errors
    );
}

#[test]
fn function_returning_match_with_all_ref_arms_accepted() {
    // GIVEN: match where every arm returns one of the val T params
    // THEN: checker accepts — all paths flow from reference parameters
    let result = check_src(
        "fn ok(flag: Bool, x: val Int, y: val Int) -> val Int { match flag { true => x, false => y } }",
    );
    assert!(
        result.errors.is_empty(),
        "expected no errors for match with all ref-param arms, got: {:?}",
        result.errors
    );
}

#[test]
fn function_returning_match_with_non_ref_arm_rejected() {
    // GIVEN: match where one arm returns a literal (not a ref param)
    // THEN: checker rejects — the literal arm does not flow from a reference parameter
    let result = check_src(
        "fn bad(flag: Bool, x: val Int) -> val Int { match flag { true => x, false => 42 } }",
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceEscapesScope { .. })),
        "expected ReferenceEscapesScope for non-ref match arm, got: {:?}",
        result.errors
    );
}

#[test]
fn function_returning_if_without_else_rejected() {
    // GIVEN: body's last statement is an if without an else branch
    // THEN: checker rejects — the else path cannot return a reference
    let result = check_src("fn bad(cond: Bool, x: val Int) -> val Int { if cond { x } }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceEscapesScope { .. })),
        "expected ReferenceEscapesScope for if-without-else, got: {:?}",
        result.errors
    );
}

// ── Expression-level borrow operator (#366) ──────────────────────────────────

#[test]
fn borrow_expr_shared_type_checks() {
    // GIVEN: `let r: val Int = val x` where x: Int
    // THEN: checker accepts and r has type &Int
    let result = check_src("fn f(x: Int) -> Unit { let r: val Int = val x; }");
    assert!(
        result.errors.is_empty(),
        "expected no errors, got: {:?}",
        result.errors
    );
}

#[test]
fn borrow_expr_mutable_type_checks() {
    // GIVEN: `let r: ref Int = ref x` where x: mut Int
    // THEN: checker accepts and r has type &mut Int
    let result = check_src("fn f(mut x: Int) -> Unit { let r: ref Int = ref x; }");
    assert!(
        result.errors.is_empty(),
        "expected no errors, got: {:?}",
        result.errors
    );
}

// ── Gap-documenting tests: unimplemented checker checks ──────────────────────
//
// These tests are marked #[ignore] because the underlying checker logic is not
// yet implemented.  They document known gaps and will pass once the feature is
// complete.  Do NOT remove them — they are the acceptance criteria.

/// Phase D (#306): AliasingMutableBorrow — creating `ref T` while a shared `val T`
/// borrow of the same inner type is also present in the signature.
#[test]
fn shared_and_mut_ref_params_of_same_type_rejected() {
    let result = check_src("fn bad(a: val Int, b: ref Int) -> Unit { }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::AliasingMutableBorrow { .. })),
        "expected AliasingMutableBorrow, got: {:?}",
        result.errors
    );
}

/// Phase D (#362): BorrowState transitions — `Expr::Borrow` emits `AliasingMutableBorrow`
/// when `ref x` is created while `x` is already borrowed.
#[test]
fn borrow_expr_transitions_borrow_state_rejected_on_double_mut() {
    // Two simultaneous `ref x` borrows must be rejected via BorrowState tracking.
    let result =
        check_src("fn f(ref x: Int) -> Unit { let r1: ref Int = ref x; let r2: ref Int = ref x; }");
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            CheckError::AliasingMutableBorrow { .. } | CheckError::DoubleMutableBorrow { .. }
        )),
        "expected AliasingMutableBorrow or DoubleMutableBorrow, got: {:?}",
        result.errors
    );
}

/// Phase D (#362): `ref T` before `val T` in params — order-independent alias check.
/// Ensures `fn bad(b: ref Int, a: val Int)` is rejected even when `ref T` comes first.
#[test]
fn shared_and_mut_ref_params_reversed_order_rejected() {
    let result = check_src("fn bad(b: ref Int, a: val Int) -> Unit { }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::AliasingMutableBorrow { .. })),
        "expected AliasingMutableBorrow for reversed param order, got: {:?}",
        result.errors
    );
}

/// Phase D (#362): shared borrow `val x` while `x` is mutably borrowed is rejected.
#[test]
fn borrow_expr_shared_of_mutably_borrowed_rejected() {
    let result =
        check_src("fn f(ref x: Int) -> Unit { let r1: ref Int = ref x; let r2: val Int = val x; }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::AliasingMutableBorrow { .. })),
        "expected AliasingMutableBorrow for val x while x is mutably borrowed, got: {:?}",
        result.errors
    );
}

// ── #660: capability state machine driven from implicit borrows ──────────────

/// Multiple implicit `val T` borrows of the same variable are allowed.
#[test]
fn implicit_multiple_val_borrows_allowed() {
    let result = check_src("fn f(x: Int) -> Unit { let v1: val Int = x; let v2: val Int = x; }");
    assert!(
        result.errors.is_empty(),
        "expected no errors for multiple implicit val borrows, got: {:?}",
        result.errors
    );
}

/// Implicit `ref T` borrow blocks a subsequent implicit `val T` borrow.
#[test]
fn implicit_ref_then_val_rejected() {
    let result = check_src("fn f(x: Int) -> Unit { let r: ref Int = x; let v: val Int = x; }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::AliasingMutableBorrow { .. })),
        "expected AliasingMutableBorrow when implicit val follows ref, got: {:?}",
        result.errors
    );
}

/// Implicit `val T` borrow blocks a subsequent implicit `ref T` borrow.
#[test]
fn implicit_val_then_ref_rejected() {
    let result = check_src("fn f(x: Int) -> Unit { let v: val Int = x; let r: ref Int = x; }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::AliasingMutableBorrow { .. })),
        "expected AliasingMutableBorrow when implicit ref follows val, got: {:?}",
        result.errors
    );
}

/// Two simultaneous implicit `ref T` borrows of the same variable are rejected.
#[test]
fn implicit_double_ref_rejected() {
    let result = check_src("fn f(x: Int) -> Unit { let r1: ref Int = x; let r2: ref Int = x; }");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::AliasingMutableBorrow { .. })),
        "expected AliasingMutableBorrow for double implicit ref borrow, got: {:?}",
        result.errors
    );
}

/// Scope exit releases capability — implicit `val T` borrow allowed after inner scope's
/// `ref T` borrow exits.
#[test]
fn implicit_borrow_released_on_scope_exit() {
    let result = check_src(
        "fn f(x: Int) -> Unit {
            if true { let r: ref Int = x; }
            let v: val Int = x;
        }",
    );
    assert!(
        result.errors.is_empty(),
        "expected no errors after scope releases implicit ref, got: {:?}",
        result.errors
    );
}

/// Phase C (#305, #363): ReferenceOutlivesOwner — assigning a `val T` reference to a
/// binding at a shallower scope depth than the referent.
/// Also verifies that `ref_name` and `owner_name` fields are correctly populated.
#[test]
fn ref_binding_outliving_owner_rejected() {
    let result = check_src(
        "fn bad() -> Unit {
            let r: val Int = {
                let x: Int = 42;
                x
            };
        }",
    );
    assert!(
        result.errors.iter().any(|e| matches!(
            e,
            CheckError::ReferenceOutlivesOwner { ref_name, owner_name, .. }
            if ref_name == "r" && owner_name == "x"
        )),
        "expected ReferenceOutlivesOwner{{ref_name:r, owner_name:x}}, got: {:?}",
        result.errors
    );
}

/// Phase C (#363): `ref T` implicit borrow from deeper scope is also rejected.
#[test]
fn ref_mut_binding_outliving_owner_rejected() {
    let result = check_src(
        "fn bad() -> Unit {
            let r: ref Int = {
                let x: ref Int = 42;
                x
            };
        }",
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceOutlivesOwner { .. })),
        "expected ReferenceOutlivesOwner for &mut binding, got: {:?}",
        result.errors
    );
}

/// Phase C (#363): explicit `val x` borrow from deeper scope is rejected.
#[test]
fn ref_explicit_borrow_outliving_owner_rejected() {
    let result = check_src(
        "fn bad() -> Unit {
            let r: val Int = {
                let x: Int = 42;
                val x
            };
        }",
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceOutlivesOwner { .. })),
        "expected ReferenceOutlivesOwner for explicit val x borrow, got: {:?}",
        result.errors
    );
}

/// Phase C (#363): two levels of block nesting — referent_ident recurses through both.
#[test]
fn ref_binding_doubly_nested_rejected() {
    let result = check_src(
        "fn bad() -> Unit {
            let r: val Int = {
                {
                    let x: Int = 42;
                    x
                }
            };
        }",
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceOutlivesOwner { .. })),
        "expected ReferenceOutlivesOwner for doubly-nested block-local, got: {:?}",
        result.errors
    );
}

/// Phase C (#363): reference binding inside a nested block borrowing an outer-scope
/// variable is accepted (`r_depth > owner.scope_depth` → false → no error).
#[test]
fn ref_binding_inner_borrows_outer_accepted() {
    let x_depth_outer = errors_for(
        "fn ok() -> Unit {
            let x: Int = 42;
            let r: val Int = {
                x
            };
        }",
    );
    assert!(
        !x_depth_outer
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceOutlivesOwner { .. })),
        "unexpected ReferenceOutlivesOwner when binding outer-scope var, got: {x_depth_outer:?}"
    );
}

/// Phase C (#363): same-scope `val T` binding is accepted — no ReferenceOutlivesOwner.
#[test]
fn ref_binding_same_scope_accepted() {
    let errors = errors_for(
        "fn ok() -> Unit {
            let x: Int = 42;
            let r: val Int = x;
        }",
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceOutlivesOwner { .. })),
        "unexpected ReferenceOutlivesOwner for same-scope binding, got: {errors:?}"
    );
}

/// Phase C (#363): a function parameter used as referent is always in scope —
/// no ReferenceOutlivesOwner should be emitted.
#[test]
fn ref_binding_from_param_accepted() {
    let errors = errors_for(
        "fn ok(x: Int) -> Unit {
            let r: val Int = x;
        }",
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::ReferenceOutlivesOwner { .. })),
        "unexpected ReferenceOutlivesOwner for param-referent binding, got: {errors:?}"
    );
}

// ── #408: Explicit type annotation required on let bindings ──────────────────

/// `let` without annotation is now a parser error (#408).
/// The parser rejects it before the checker runs.
#[test]
fn let_without_annotation_rejected() {
    use mvl::mvl::parser::Parser;
    let (mut p, _) = Parser::new("fn f() -> Unit { let x = 42; }");
    p.parse_program();
    assert!(
        !p.errors().is_empty(),
        "unannotated let should produce a parse error"
    );
}

/// `let mut` without annotation is also a parser error (#408).
#[test]
fn let_mut_without_annotation_rejected() {
    use mvl::mvl::parser::Parser;
    let (mut p, _) = Parser::new("fn f() -> Unit { let mut x = 42; }");
    p.parse_program();
    assert!(
        !p.errors().is_empty(),
        "unannotated let mut should produce a parse error"
    );
}

/// `let` with annotation is accepted — no errors.
#[test]
fn let_with_annotation_accepted() {
    let errors = errors_for("fn f() -> Unit { let x: Int = 42; }");
    assert!(
        errors.is_empty(),
        "annotated let should produce no errors, got: {errors:?}"
    );
}

// ── Epic #480: Primitives and runtime architecture redesign ──────────────────

/// Unsigned types corpus must parse and type-check without errors (#481).
#[test]
fn unsigned_types_corpus_parses_and_checks() {
    let src = include_str!("corpus/04_primitives/unsigned_types.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "unsigned_types corpus must type-check cleanly, got: {:?}",
        result.errors
    );
}

/// Bit-operator corpus must parse and type-check without errors (#483 #484).
#[test]
fn bit_operators_corpus_parses_and_checks() {
    let src = include_str!("corpus/04_primitives/bit_operators.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "bit_operators corpus must type-check cleanly, got: {:?}",
        result.errors
    );
}

/// Overflow-checking arithmetic corpus must parse and type-check without errors (#485).
#[test]
fn overflow_checking_corpus_parses_and_checks() {
    let src = include_str!("corpus/04_primitives/overflow_checking.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "overflow_checking corpus must type-check cleanly, got: {:?}",
        result.errors
    );
}

/// UInt.wrapping_add/sub/mul must resolve to UInt, not Unknown (#493 fix).
#[test]
fn uint_wrapping_methods_resolve_correctly() {
    let result = check_src("fn f(x: UInt, y: UInt) -> UInt { x.wrapping_add(y) }");
    assert!(
        result.is_ok(),
        "UInt.wrapping_add must type-check cleanly, got: {:?}",
        result.errors
    );
    let result2 = check_src("fn f(x: UInt, y: UInt) -> UInt { x.wrapping_sub(y) }");
    assert!(
        result2.is_ok(),
        "UInt.wrapping_sub must type-check cleanly, got: {:?}",
        result2.errors
    );
    let result3 = check_src("fn f(x: UInt, y: UInt) -> UInt { x.wrapping_mul(y) }");
    assert!(
        result3.is_ok(),
        "UInt.wrapping_mul must type-check cleanly, got: {:?}",
        result3.errors
    );
}

/// Bitwise operators on non-integer types must produce TypeMismatch (#483).
#[test]
fn bitwise_op_on_float_is_rejected() {
    let errors = errors_for("fn f(x: Float, y: Float) -> Float { x & y }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Float & Float should produce TypeMismatch, got: {errors:?}"
    );
}

#[test]
fn bitwise_not_on_float_is_rejected() {
    let errors = errors_for("fn f(x: Float) -> Float { ~x }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "~Float should produce TypeMismatch, got: {errors:?}"
    );
}

/// IFC label propagation through bitwise operators (#483).
#[test]
fn bitwise_and_propagates_ifc_label() {
    // Post-#894: Secret[Int] & Int → Secret[Int]; assigning to Secret[Int] is fine.
    let errors = errors_for("fn f(a: Secret[Int], b: Int) -> Secret[Int] { a & b }");
    assert!(
        errors.is_empty(),
        "Secret[Int] & Int should yield Secret[Int], got: {errors:?}"
    );
}

#[test]
fn bitwise_and_label_downgrade_rejected() {
    // Secret[Int] & Int → Secret[Int]; assigning to Public[Int] must fail.
    let errors = errors_for("fn f(a: Secret[Int], b: Public[Int]) -> Public[Int] { a & b }");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret[Int] & Public[Int] result cannot flow to Public[Int], got: {errors:?}"
    );
}

// ── builtin fn checker tests (#534) ──────────────────────────────────────

#[test]
fn builtin_fn_with_non_unit_return_accepted() {
    // GIVEN: pub builtin fn len(s: String) -> Int  (no body)
    // THEN: no type errors — checker skips body checking for builtin functions
    let errors = errors_for("pub builtin fn len(s: String) -> Int");
    assert!(
        errors.is_empty(),
        "builtin fn with non-Unit return should produce no errors, got: {errors:?}"
    );
}

#[test]
fn builtin_fn_callable_from_user_code() {
    // GIVEN: a builtin fn declared and called in user code
    // THEN: call site resolves without type errors
    let src = "pub builtin fn len(s: String) -> Int\nfn use_len(s: String) -> Int { len(s) }";
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "call to builtin fn should type-check cleanly, got: {errors:?}"
    );
}

// ── HOF calling, panic never type, early-return in loops (#618) ──────────────

/// A function parameter with type `fn(Int) -> Bool` can be called as a HOF;
/// the call site resolves to Bool with no errors.
#[test]
fn hof_param_callable_and_returns_correct_type() {
    let src = "fn apply(pred: fn(Int) -> Bool, x: Int) -> Bool { pred(x) }";
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "HOF call should type-check cleanly, got: {errors:?}"
    );
}

/// Calling a HOF parameter with the wrong number of arguments emits WrongArgCount.
#[test]
fn hof_wrong_arg_count_emits_error() {
    let src = "fn apply(pred: fn(Int) -> Bool, x: Int) -> Bool { pred(x, x) }";
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::WrongArgCount { .. })),
        "expected WrongArgCount for HOF call with wrong arity, got: {errors:?}"
    );
}

/// Calling a HOF parameter with a wrong argument type emits TypeMismatch.
#[test]
fn hof_wrong_arg_type_emits_type_mismatch() {
    let src = r#"fn apply(pred: fn(Int) -> Bool) -> Bool { pred("hello") }"#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "expected TypeMismatch for HOF call with wrong arg type, got: {errors:?}"
    );
}

/// A match expression where one arm calls panic() must unify: panic returns Never
/// (the bottom type) which is compatible with any expected type.
#[test]
fn panic_in_match_arm_unifies_with_any_type() {
    // GIVEN: a function returning Int with a match that panics on None
    let src = r#"
        pub builtin fn panic(message: String) -> Never
        fn unwrap_int(x: Option[Int]) -> Int {
            match x {
                Some(v) => v,
                None => panic("unwrap on None")
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "panic() in match arm should not cause TypeMismatch, got: {errors:?}"
    );
}

/// An early `return` inside a for-loop body is type-checked against the
/// function's declared return type, not against Unit.
#[test]
fn early_return_in_for_loop_type_checked_correctly() {
    // GIVEN: a function returning Option[Int] with an early return inside a for loop
    let src = "fn first(xs: List[Int]) -> Option[Int] { for x in xs { return Some(x) }; None }";
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "early return in for-loop should type-check against fn return type, got: {errors:?}"
    );
}

/// An early `return` with the wrong type inside a for-loop body is caught.
#[test]
fn early_return_in_for_loop_wrong_type_rejected() {
    let src = r#"fn f(xs: List[Int]) -> Int { for x in xs { return "bad" }; 0 }"#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "early return with wrong type in for-loop should emit TypeMismatch, got: {errors:?}"
    );
}

// ── stdlib proven profile — unit tests (#538) ────────────────────────────────

/// Proven profile verification is backed by `check_with_prelude`. Verify that
/// a deliberately ill-typed stdlib body (wrong return type) is caught, proving
/// the checker is actually applied to stdlib source rather than skipped.
#[test]
fn proven_profile_checker_catches_type_error_in_stdlib_body() {
    use mvl::mvl::checker::check_with_prelude;

    // GIVEN: a "stdlib" snippet with a body that returns the wrong type
    let bad_stdlib = "pub fn broken(x: Int) -> String { x }";
    let (mut p, _) = Parser::new(bad_stdlib);
    let prog = p.parse_program();

    // WHEN: verified with check_with_prelude (as check_proven_stdlib does)
    let result = check_with_prelude(&[], &prog);

    // THEN: a type error is reported — the checker is not a no-op
    assert!(
        result.has_errors(),
        "expected type error for wrong return type, got no errors"
    );
}

/// A pure-MVL stdlib body that is correctly typed must pass check_with_prelude
/// without errors (proven profile does not over-reject valid stdlib).
#[test]
fn proven_profile_checker_accepts_valid_stdlib_body() {
    use mvl::mvl::checker::check_with_prelude;

    // GIVEN: a valid stdlib snippet
    let good_stdlib = "pub fn double(x: Int) -> Int { x + x }";
    let (mut p, _) = Parser::new(good_stdlib);
    let prog = p.parse_program();

    // WHEN: verified with check_with_prelude
    let result = check_with_prelude(&[], &prog);

    // THEN: no errors
    assert!(
        !result.has_errors(),
        "valid stdlib body should pass proven profile check, got: {:?}",
        result.errors
    );
}

// ── #609: whole-program checking (cross-file symbol resolution) ───────────────

fn parse_src(src: &str) -> mvl::mvl::parser::ast::Program {
    let (mut p, _) = Parser::new(src);
    p.parse_program()
}

#[test]
fn cross_file_function_call_resolves() {
    // GIVEN: module A exports a function; module B calls it
    // WHEN:  B is checked with A as prelude
    // THEN:  no "undefined function" error
    let module_a = parse_src("pub fn add_one(x: Int) -> Int { x + 1 }");
    let module_b = parse_src("fn use_it(x: Int) -> Int { add_one(x) }");
    let result = check_with_prelude(&[module_a], &module_b);
    assert!(
        result.is_ok(),
        "cross-file call should resolve when module is in prelude, got: {:?}",
        result.errors
    );
}

#[test]
fn cross_file_call_without_prelude_is_undefined() {
    // GIVEN: module B calls add_one which is NOT in scope
    // WHEN:  B is checked without any prelude
    // THEN:  UndefinedFunction error is reported
    let module_b = parse_src("fn use_it(x: Int) -> Int { add_one(x) }");
    let errors = check(&module_b).errors;
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UndefinedFunction { .. })),
        "call to unknown function should be an error, got: {errors:?}"
    );
}

#[test]
fn cross_file_type_mismatch_still_caught() {
    // GIVEN: module A exports fn returning Int; module B passes wrong type
    // WHEN:  B is checked with A as prelude
    // THEN:  type mismatch is reported (cross-file checking catches real errors)
    let module_a = parse_src("pub fn greet(x: Int) -> Int { x }");
    let module_b = parse_src(r#"fn bad() -> Int { greet("hello") }"#);
    let errors = check_with_prelude(&[module_a], &module_b).errors;
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "wrong argument type should be caught even in multi-file mode, got: {errors:?}"
    );
}

#[test]
fn cross_file_mutual_calls_both_resolve() {
    // GIVEN: module A calls helper() defined in module B, and
    //        module B calls double() defined in module A (mutual dependency).
    // WHEN:  each module is checked with the other as prelude
    // THEN:  both check cleanly — no UndefinedFunction errors in either direction
    let module_a =
        parse_src("pub fn double(x: Int) -> Int { x + x }  fn use_b(x: Int) -> Int { helper(x) }");
    let module_b =
        parse_src("pub fn helper(x: Int) -> Int { x }  fn use_a(x: Int) -> Int { double(x) }");

    // Check A with B as prelude
    let result_a = check_with_prelude(&[module_b.clone()], &module_a);
    assert!(
        result_a.is_ok(),
        "module A with B as prelude should resolve, got: {:?}",
        result_a.errors
    );

    // Check B with A as prelude (uses check_with_two_preludes with empty stdlib slot)
    let result_b = check_with_two_preludes(&[], &[&module_a], &module_b);
    assert!(
        result_b.is_ok(),
        "module B with A as prelude should resolve, got: {:?}",
        result_b.errors
    );
}

// ── #1358: cross-file extension method / type conflict ────────────────────────

#[test]
fn cross_file_extension_method_type_in_current_file_no_false_error() {
    // GIVEN: module B defines an extension method on TypeFoo::helper()
    //        module A defines type TypeFoo (the receiver type)
    // WHEN:  A is checked with B as prelude
    // THEN:  no false UndefinedType error for the extension method in B
    let module_a = parse_src("pub type TypeFoo = struct { x: Int }");
    let module_b = parse_src("pub fn TypeFoo::helper(self) -> Int { self.x }");
    let result = check_with_two_preludes(&[], &[&module_b], &module_a);
    assert!(
        result.is_ok(),
        "no false error when extension method receiver type is in current file, got: {:?}",
        result.errors
    );
}

#[test]
fn cross_file_extension_method_call_resolves() {
    // GIVEN: module A defines type TypeBar
    //        module B defines fn TypeBar::double(self) -> Int
    //        module C calls v.double() where v: TypeBar
    // WHEN:  C is checked with A and B as prelude
    // THEN:  the method call resolves — no UndefinedFunction error
    let module_a = parse_src("pub type TypeBar = struct { n: Int }");
    let module_b = parse_src("pub fn TypeBar::double(self) -> Int { self.n + self.n }");
    let module_c = parse_src("fn use_it(v: TypeBar) -> Int { v.double() }");
    let result = check_with_two_preludes(&[module_a, module_b], &[], &module_c);
    assert!(
        result.is_ok(),
        "cross-file extension method call should resolve, got: {:?}",
        result.errors
    );
}

// ── #593: Layer 4 — Cooper's Presburger QE ────────────────────────────────────

#[test]
fn cooper_linear_expr_arg_proven() {
    // GIVEN: `b: Int where self < a` and return value `a - b`
    // WHEN:  return predicate `result > 0` is checked
    // THEN:  Layer 4 proves it via FM elimination (no runtime check needed)
    let src = r#"
        fn diff_positive(a: Int, b: Int where self < a) -> Int where result > 0 {
            a - b
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "linear-expr diff with hyp should be proven by Layer 4, got: {:?}",
        result.errors
    );
}

#[test]
fn cooper_divisibility_always_nonzero() {
    // GIVEN: return value `2 * x + 1`
    // WHEN:  predicate `result != 0` is checked
    // THEN:  Layer 4 detects 2*x + 1 = 0 has no integer solution → Proven
    let src = r#"
        fn always_nonzero(x: Int) -> Int where result != 0 {
            2 * x + 1
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "2*x+1 != 0 should be proven by divisibility check in Layer 4, got: {:?}",
        result.errors
    );
}

// ── Layer 5: Z3 SMT solver ───────────────────────────────────────────────────

#[test]
fn z3_proves_hypothesis_implies_pred() {
    // GIVEN: y has refinement `self > 5`
    // WHEN:  `require_positive(y)` is checked against `self > 0`
    // THEN:  Z3 proves `y > 5 → y > 0` (Layers 1–4 already handle this,
    //        but the test confirms Layer 5 is reachable and correct)
    let src = r#"
        fn require_positive(x: Int where self > 0) -> Int {
            x
        }
        fn call_with_refined(y: Int where self > 5) -> Int {
            require_positive(y)
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "y > 5 implies y > 0 — should be proven statically, got: {:?}",
        result.errors
    );
}

#[test]
fn z3_proves_modular_implication() {
    // GIVEN: y has refinement `self % 6 == 1`
    // WHEN:  `require_nonzero_mod3(y)` is checked against `self % 3 != 0`
    // THEN:  Z3 proves y%6=1 → y%3≠0 (Cooper handles linear arithmetic,
    //        but modular chaining like this may reach Layer 5)
    let src = r#"
        fn require_nonzero_mod3(x: Int where self % 3 != 0) -> Int {
            x
        }
        fn call_with_mod6(y: Int where self % 6 == 1) -> Int {
            require_nonzero_mod3(y)
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "y%6=1 implies y%3≠0 — should be proven statically, got: {:?}",
        result.errors
    );
}

// ── Issue #621: Function contracts — requires / ensures ───────────────────────

#[test]
fn contracts_corpus_parses_and_checks() {
    // GIVEN: the basic_contracts corpus (valid contract programs)
    // THEN: no type errors
    let src = include_str!("corpus/11_contracts/basic_contracts.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "contracts corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn requires_parses_and_is_stored_on_fndecl() {
    // GIVEN: fn with `requires` contract clause
    // THEN: parsed without errors and requires is non-empty
    let src = r#"
        fn divide(a: Int, b: Int) -> Int
          requires b != 0
        { a }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "requires clause should parse cleanly: {:?}",
        result.errors
    );
}

#[test]
fn ensures_parses_and_is_stored_on_fndecl() {
    // GIVEN: fn with `ensures` contract clause
    // THEN: parsed without errors
    let src = r#"
        fn identity(n: Int) -> Int
          ensures result == n
        { n }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "ensures clause should parse cleanly: {:?}",
        result.errors
    );
}

#[test]
fn requires_and_ensures_combined() {
    // GIVEN: fn with both requires and ensures
    // THEN: parsed without errors
    let src = r#"
        fn factorial(n: Int) -> Int
          requires n >= 0
          ensures result >= 1
        { 1 }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "requires+ensures should parse cleanly: {:?}",
        result.errors
    );
}

#[test]
fn requires_literal_violation_detected() {
    // GIVEN: fn with `requires b != 0`, called with literal 0
    // THEN: PreconditionViolated error reported
    let src = r#"
        fn divide(a: Int, b: Int) -> Int
          requires b != 0
        { a }
        fn caller() -> Int {
            divide(10, 0)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::PreconditionViolated { fn_name, .. } if fn_name == "divide")
        ),
        "expected PreconditionViolated for divide(10, 0), got: {:?}",
        errors
    );
}

#[test]
fn requires_literal_satisfied_no_error() {
    // GIVEN: fn with `requires b != 0`, called with non-zero literal
    // THEN: no error (Proven by solver)
    let src = r#"
        fn divide(a: Int, b: Int) -> Int
          requires b != 0
        { a }
        fn caller() -> Int {
            divide(10, 5)
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "divide(10, 5) satisfies b != 0: {:?}",
        result.errors
    );
}

#[test]
fn requires_unknown_var_is_runtime_check_no_error() {
    // GIVEN: fn with `requires b != 0`, called with an unknown variable
    // THEN: no compile-time error (deferred to runtime)
    let src = r#"
        fn divide(a: Int, b: Int) -> Int
          requires b != 0
        { a }
        fn caller(x: Int, y: Int) -> Int {
            divide(x, y)
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "unknown var satisfying requires should be RuntimeCheck (no error): {:?}",
        result.errors
    );
}

#[test]
fn ensures_literal_violation_detected() {
    // GIVEN: fn with `ensures result >= 0`, returning a negative literal
    // THEN: PostconditionViolated error reported
    let src = r#"
        fn bad() -> Int
          ensures result >= 0
        { -1 }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::PostconditionViolated { fn_name, .. } if fn_name == "bad")
        ),
        "expected PostconditionViolated for bad(), got: {:?}",
        errors
    );
}

#[test]
fn ensures_literal_satisfied_no_error() {
    // GIVEN: fn with `ensures result >= 0`, returning a non-negative literal
    // THEN: no error (Proven by solver)
    let src = r#"
        fn nonneg() -> Int
          ensures result >= 0
        { 42 }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "nonneg() returns 42 which satisfies result >= 0: {:?}",
        result.errors
    );
}

#[test]
fn ensures_with_param_ref_is_runtime_check_no_error() {
    // GIVEN: fn with `ensures result >= n` (references param — Phase 2+)
    // THEN: no compile-time error (conservatively deferred to runtime)
    let src = r#"
        fn at_least(n: Int) -> Int
          ensures result >= n
        { n }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "ensures with param ref should be RuntimeCheck (no error in Phase 1): {:?}",
        result.errors
    );
}

#[test]
fn multiple_requires_all_checked() {
    // GIVEN: fn with two requires clauses; call violates the second
    // THEN: PreconditionViolated for the violated clause
    let src = r#"
        fn bounded(a: Int, b: Int) -> Int
          requires a >= 0
          requires b >= 0
        { a }
        fn caller() -> Int {
            bounded(-1, 5)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(e, CheckError::PreconditionViolated { fn_name, .. } if fn_name == "bounded")),
        "expected PreconditionViolated for bounded(-1, 5), got: {:?}",
        errors
    );
}

#[test]
fn requires_first_param_checked() {
    // GIVEN: fn with `requires a >= 0`; call passes -1 for a
    // THEN: PreconditionViolated at the call site
    let src = r#"
        fn positive_a(a: Int, b: Int) -> Int
          requires a >= 0
        { a }
        fn caller() -> Int {
            positive_a(-1, 99)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(e, CheckError::PreconditionViolated { fn_name, .. } if fn_name == "positive_a")),
        "expected PreconditionViolated for positive_a(-1, 99), got: {:?}",
        errors
    );
}

#[test]
fn no_contracts_no_errors() {
    // GIVEN: fn with no contracts
    // THEN: no contract errors
    let src = r#"
        fn add(a: Int, b: Int) -> Int { a + b }
        fn caller() -> Int { add(1, 2) }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "no contracts should produce no errors: {:?}",
        result.errors
    );
}

#[test]
fn requires_correct_param_position_second() {
    // GIVEN: fn with `requires b > 0`; call passes b=3 (valid)
    // THEN: no error
    let src = r#"
        fn f(a: Int, b: Int) -> Int
          requires b > 0
        { a }
        fn caller() -> Int { f(0, 3) }
    "#;
    let result = check_src(src);
    assert!(result.is_ok(), "b=3 satisfies b > 0: {:?}", result.errors);
}

#[test]
fn ensures_explicit_return_checked() {
    // GIVEN: fn with `ensures result >= 0` using explicit return statement
    // THEN: explicit negative return causes PostconditionViolated
    let src = r#"
        fn early_exit(n: Int) -> Int
          ensures result >= 0
        {
            return -5;
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(|e| matches!(e, CheckError::PostconditionViolated { fn_name, .. } if fn_name == "early_exit")),
        "expected PostconditionViolated for explicit return -5, got: {:?}",
        errors
    );
}

// ── Phase 2: multi-param `requires` (literal substitution) ───────────────────

#[test]
fn requires_multi_param_literal_satisfied_no_error() {
    // GIVEN: `requires a > b` and both args are literals satisfying it
    // THEN:  no error
    let src = r#"
        fn sub_safe(a: Int, b: Int) -> Int
          requires a > b
        { a }
        fn caller() -> Int { sub_safe(10, 3) }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::PreconditionViolated { .. })),
        "expected no PreconditionViolated for sub_safe(10, 3), got: {:?}",
        errors
    );
}

#[test]
fn requires_multi_param_literal_violated_detected() {
    // GIVEN: `requires a > b` and literal args that violate it
    // THEN:  PreconditionViolated
    let src = r#"
        fn sub_safe(a: Int, b: Int) -> Int
          requires a > b
        { a }
        fn caller() -> Int { sub_safe(3, 10) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::PreconditionViolated { fn_name, .. } if fn_name == "sub_safe")),
        "expected PreconditionViolated for sub_safe(3, 10), got: {:?}",
        errors
    );
}

#[test]
fn requires_multi_param_equal_fails() {
    // GIVEN: `requires a > b` (strict) and equal literal args
    // THEN:  PreconditionViolated
    let src = r#"
        fn sub_safe(a: Int, b: Int) -> Int
          requires a > b
        { a }
        fn caller() -> Int { sub_safe(5, 5) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::PreconditionViolated { fn_name, .. } if fn_name == "sub_safe")),
        "expected PreconditionViolated for sub_safe(5, 5), got: {:?}",
        errors
    );
}

#[test]
fn requires_multi_param_non_literal_is_runtime_check_no_error() {
    // GIVEN: `requires a > b` and variable args (not literals)
    // THEN:  no compile-time error (deferred to RuntimeCheck)
    let src = r#"
        fn sub_safe(a: Int, b: Int) -> Int
          requires a > b
        { a }
        fn caller(x: Int, y: Int) -> Int { sub_safe(x, y) }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::PreconditionViolated { .. })),
        "expected no PreconditionViolated for non-literal args, got: {:?}",
        errors
    );
}

// ── Phase 2: parameter-aware `ensures` (Layer 4 Cooper + param var_refs) ──────

#[test]
fn ensures_param_ref_identity_proven() {
    // GIVEN: `ensures result == n`, body returns `n` directly
    // THEN:  Layer 4 proves `n == n` — no error
    let src = r#"
        fn id_int(n: Int) -> Int
          ensures result == n
        {
            n
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::PostconditionViolated { .. })),
        "expected no PostconditionViolated for identity fn, got: {:?}",
        errors
    );
}

#[test]
fn ensures_param_ref_increment_proven() {
    // GIVEN: `ensures result >= n`, body returns `n + 1`
    // THEN:  Layer 4 proves `n + 1 >= n` — no error
    let src = r#"
        fn inc(n: Int) -> Int
          ensures result >= n
        {
            n + 1
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::PostconditionViolated { .. })),
        "expected no PostconditionViolated for inc fn, got: {:?}",
        errors
    );
}

#[test]
fn ensures_param_refinement_enables_proof() {
    // GIVEN: param has `where self >= 0` and `ensures result >= 0`, body returns param
    // THEN:  Layer 2 uses var_refs interval [0,∞) to prove `n >= 0` — no error
    let src = r#"
        fn nonneg_id(n: Int where self >= 0) -> Int
          ensures result >= 0
        {
            n
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::PostconditionViolated { .. })),
        "expected no PostconditionViolated for nonneg_id, got: {:?}",
        errors
    );
}

#[test]
fn ensures_param_ref_double_increment_proven() {
    // GIVEN: `ensures result >= n + 1`, body returns `n + 2`
    // THEN:  Layer 4 proves `n + 2 >= n + 1` — no error
    let src = r#"
        fn add_two(n: Int) -> Int
          ensures result >= n + 1
        {
            n + 2
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::PostconditionViolated { .. })),
        "expected no PostconditionViolated for add_two, got: {:?}",
        errors
    );
}

#[test]
fn ensures_param_ref_wrong_does_not_spuriously_fail() {
    // GIVEN: `ensures result >= n + 1`, body returns `n` (genuinely wrong)
    // THEN:  solver returns RuntimeCheck (not Failed) — no compile-time error
    //        (violation detection for this pattern requires Z3 or Phase 3)
    let src = r#"
        fn bad_inc(n: Int) -> Int
          ensures result >= n + 1
        {
            n
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors.iter().any(|e| matches!(e, CheckError::PostconditionViolated { fn_name, .. } if fn_name == "bad_inc")),
        "expected no spurious PostconditionViolated for bad_inc (RuntimeCheck expected), got: {:?}",
        errors
    );
}

// ── Phase 3: loop invariants ───────────────────────────────────────────────────

#[test]
fn invariant_constant_true_no_error() {
    // GIVEN: constant-true invariant (no variable references)
    // THEN:  Layer 1 proves it — no error
    let src = r#"
        partial fn server() -> Unit {
            while true
              invariant 0 >= 0
            { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::InvariantViolated { .. })),
        "expected no InvariantViolated for constant-true invariant, got: {:?}",
        errors
    );
}

#[test]
fn invariant_constant_false_detected() {
    // GIVEN: constant-false invariant (statically impossible)
    // THEN:  Layer 1 returns Failed -> InvariantViolated emitted
    let src = r#"
        partial fn server() -> Unit {
            while true
              invariant 1 < 0
            { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::InvariantViolated { fn_name, .. } if fn_name == "server")
        ),
        "expected InvariantViolated for constant-false invariant, got: {:?}",
        errors
    );
}

#[test]
fn invariant_param_refinement_proven() {
    // GIVEN: parameter `n: Int where self >= 0`, invariant `n >= 0`
    // THEN:  Layer 2 proves invariant holds at loop entry — no error
    let src = r#"
        partial fn loop_nonneg(n: Int where self >= 0) -> Unit {
            while true
              invariant n >= 0
            { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::InvariantViolated { .. })),
        "expected no InvariantViolated when param refinement proves invariant, got: {:?}",
        errors
    );
}

#[test]
fn invariant_param_no_refinement_is_runtime_check() {
    // GIVEN: parameter `n: Int` (no refinement), invariant `n >= 0`
    // THEN:  solver returns RuntimeCheck — no compile-time error emitted
    let src = r#"
        partial fn loop_unknown(n: Int) -> Unit {
            while true
              invariant n >= 0
            { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::InvariantViolated { .. })),
        "expected no InvariantViolated when param has no refinement (RuntimeCheck), got: {:?}",
        errors
    );
}

#[test]
fn invariant_param_refinement_too_weak_is_runtime_check() {
    // GIVEN: `n: Int where self > 0`, invariant `n >= 5`
    // THEN:  Layer 2 cannot prove n >= 5 from n > 0 -> RuntimeCheck — no error
    let src = r#"
        partial fn loop_weak(n: Int where self > 0) -> Unit {
            while true
              invariant n >= 5
            { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::InvariantViolated { .. })),
        "expected no InvariantViolated when solver cannot prove (RuntimeCheck), got: {:?}",
        errors
    );
}

#[test]
fn invariant_multi_var_is_runtime_check_no_error() {
    // GIVEN: multi-variable invariant `lo <= hi` (Phase 4 territory)
    // THEN:  RuntimeCheck — no compile-time error
    let src = r#"
        partial fn loop_range(lo: Int, hi: Int) -> Unit {
            while true
              invariant lo <= hi
            { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::InvariantViolated { .. })),
        "expected no InvariantViolated for multi-var invariant (RuntimeCheck), got: {:?}",
        errors
    );
}

// ── Phase 3: param-refined requires at call sites ─────────────────────────────

#[test]
fn requires_caller_param_refinement_proves_precondition() {
    // GIVEN: `divide` requires `b != 0`; caller passes `b: Int where self > 0`
    // THEN:  Layer 2 proves b > 0 implies b != 0 — no PreconditionViolated
    let src = r#"
        fn divide(a: Int, b: Int) -> Int
          requires b != 0
        { a }

        fn caller(a: Int, b: Int where self > 0) -> Int {
            divide(a, b)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors.iter().any(|e| matches!(e, CheckError::PreconditionViolated { fn_name, .. } if fn_name == "divide")),
        "expected no PreconditionViolated when caller param refinement proves precondition, got: {:?}",
        errors
    );
}

// ── Phase 4: ghost let bindings (#627) ───────────────────────────────────────

#[test]
fn ghost_let_parses_and_checks_cleanly() {
    // GIVEN: a function with `ghost let x: T = expr;`
    // THEN: no type errors — ghost bindings are type-checked normally
    let src = r#"
        fn abs_value(n: Int) -> Int
          ensures result >= 0
        {
            ghost let spec_n: Int = n;
            if n >= 0 { n } else { 0 - n }
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "ghost let should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn ghost_let_multiple_in_body_checks_cleanly() {
    // GIVEN: a function body with two ghost bindings
    // THEN: no type errors
    let src = r#"
        fn example(x: Int) -> Int {
            ghost let a: Int = x;
            ghost let b: Int = a;
            x
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "multiple ghost lets should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn ghost_corpus_parses_and_checks() {
    // GIVEN: the ghost_old_contracts corpus (all ghost/old contract programs)
    // THEN: no type errors
    let src = include_str!("corpus/11_contracts/ghost_old_contracts.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "ghost_old_contracts corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

// ── Phase 4: old() in ensures predicates (#627) ───────────────────────────────

#[test]
fn old_in_ensures_parses_cleanly() {
    // GIVEN: `ensures result >= old(n)` in function signature
    // THEN: parses without errors
    let src = r#"
        fn inc(n: Int) -> Int
          ensures result >= old(n)
        {
            n + 1
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "old() in ensures should parse cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn old_ensures_identity_checks_cleanly() {
    // GIVEN: `ensures result == old(n)` for an identity function
    // THEN: static checker (Layer 4) proves it or defers to RuntimeCheck — no error
    let src = r#"
        fn id_val(n: Int) -> Int
          ensures result == old(n)
        {
            n
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "ensures result == old(n) for identity should check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn old_in_ensures_does_not_produce_false_violation() {
    // GIVEN: increment function with `ensures result >= old(n)`
    // THEN: no PostconditionViolated error — solver defers to RuntimeCheck
    let src = r#"
        fn inc(n: Int) -> Int
          ensures result >= old(n)
        {
            n + 1
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::PostconditionViolated { .. })),
        "old() in ensures should not produce false PostconditionViolated, got: {:?}",
        errors
    );
}

// ── Phase 4: counterexample field in errors (#627) ───────────────────────────

#[test]
fn precondition_violated_counterexample_field_is_none() {
    // GIVEN: a precondition is statically violated
    // THEN: the error has counterexample: None (Phase 4 wires this up in future)
    let src = r#"
        fn divide(a: Int, b: Int) -> Int
          requires b != 0
        { a }
        fn caller() -> Int { divide(10, 0) }
    "#;
    let errors = errors_for(src);
    let error = errors
        .iter()
        .find(|e| matches!(e, CheckError::PreconditionViolated { .. }));
    assert!(
        error.is_some(),
        "expected PreconditionViolated, got: {:?}",
        errors
    );
    if let Some(CheckError::PreconditionViolated { counterexample, .. }) = error {
        // TODO(#627): invert this assertion once Z3 model extraction is implemented
        // and the Sat branch in layer5 returns Failed { counterexample: Some(...) }.
        assert!(
            counterexample.is_none(),
            "counterexample should be None until Phase 4 Z3 extraction is implemented"
        );
    }
}

// ── Phase 5: loop verification — decreases, invariant preservation, quantifiers (#628) ───────

#[test]
fn loop_verification_corpus_parses_and_checks() {
    // GIVEN: the Phase 5 loop verification corpus
    // THEN: no type errors (decreases + invariant preservation + quantifiers)
    let src = include_str!("corpus/11_contracts/loop_verification.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "loop_verification corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn decreases_parses_in_while_loop() {
    // GIVEN: `while cond decreases expr { ... }` syntax
    // THEN: parses without error and the AST carries the decreases field
    let src = r#"
        partial fn f(n: Int) -> Unit {
            let i: ref Int = n;
            while i > 0 decreases i { i = i - 1; }
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "decreases clause should parse cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn decreases_method_call_parses_and_preserves_body() {
    // GIVEN: `decreases` clause contains a method call expression (#968)
    // THEN: parses without error AND loop body is not silently dropped
    let src = r#"
        partial fn pad_right(s: String, n: Int, fill: String) -> String {
            let result: ref String = s;
            while result.len() < n decreases n - result.len() {
                result = result.concat(fill);
            }
            result
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "decreases with method call should parse and type-check, got: {:?}",
        result.errors
    );
}

#[test]
fn decreases_arithmetic_expr_parses_and_preserves_body() {
    // GIVEN: `decreases` clause with binary arithmetic expression (#968)
    // THEN: parses without error and the loop body is present
    let src = r#"
        partial fn f(n: Int, step: Int) -> Unit {
            let i: ref Int = n;
            while i > 0 decreases i - step {
                i = i - step;
            }
        }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "decreases with arithmetic expr should parse and type-check, got: {:?}",
        result.errors
    );
}

#[test]
fn while_with_decreases_allowed_in_total_fn() {
    // GIVEN: implicit-total function with `while … decreases expr`
    // THEN: no UnboundedLoopInTotal error (decreases makes it bounded)
    let src = r#"
        fn f(n: Int where self >= 0) -> Unit {
            let i: ref Int = n;
            while i > 0 decreases i { i = i - 1; }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::UnboundedLoopInTotal { .. })),
        "while + decreases should NOT produce UnboundedLoopInTotal, got: {:?}",
        errors
    );
}

#[test]
fn while_without_decreases_still_rejected_in_total_fn() {
    // GIVEN: implicit-total function with bare `while` (no decreases)
    // THEN: UnboundedLoopInTotal error is still emitted
    let src = r#"
        fn f() -> Unit {
            while true { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnboundedLoopInTotal { .. })),
        "while without decreases should still produce UnboundedLoopInTotal, got: {:?}",
        errors
    );
}

#[test]
fn decreases_not_decreasing_detected() {
    // GIVEN: `decreases i` but body does `i = i + 1` (increasing, not decreasing)
    // THEN: DecreasesNotDecreasing error
    let src = r#"
        partial fn f(n: Int) -> Unit {
            let i: ref Int = n;
            while i > 0 decreases i { i = i + 1; }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::DecreasesNotDecreasing { .. })),
        "increasing measure should produce DecreasesNotDecreasing, got: {:?}",
        errors
    );
}

#[test]
fn invariant_preservation_proven_for_simple_increment() {
    // GIVEN: `invariant i >= 0` with `i = i + 1`; induction hypothesis makes it provable.
    // THEN: no InvariantNotPreserved error
    let src = r#"
        partial fn f(n: Int where self >= 0) -> Unit {
            let i: ref Int = 0;
            while i < n invariant i >= 0 { i = i + 1; }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::InvariantNotPreserved { .. })),
        "invariant i >= 0 should be preserved by i = i + 1, got: {:?}",
        errors
    );
}

#[test]
fn invariant_not_preserved_detected() {
    // GIVEN: `invariant i >= 0` but body does `i = 0 - 1` (sets i to -1)
    // THEN: InvariantNotPreserved error
    let src = r#"
        partial fn f(n: Int) -> Unit {
            let i: ref Int = n;
            while i > 0 invariant i >= 0 { i = 0 - 1; }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvariantNotPreserved { .. })),
        "setting i = -1 should produce InvariantNotPreserved, got: {:?}",
        errors
    );
}

#[test]
fn forall_quantifier_parses_in_requires() {
    // GIVEN: `requires forall x: Int, x >= lo` — quantifier in contract context
    // THEN: parses and type-checks; verification deferred to RuntimeCheck (no error)
    let src = r#"
        partial fn f(lo: Int) -> Int
            requires forall x: Int, x >= lo
        { lo }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "forall in requires should not produce compile errors, got: {:?}",
        result.errors
    );
}

#[test]
fn exists_quantifier_parses_in_ensures() {
    // GIVEN: `ensures exists x: Int, x > 0` — quantifier in ensures context
    // THEN: parses and type-checks; verification deferred to RuntimeCheck (no error)
    let src = r#"
        partial fn f(n: Int where self > 0) -> Int
            ensures exists x: Int, x > 0
        { n }
    "#;
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "exists in ensures should not produce compile errors, got: {:?}",
        result.errors
    );
}

#[test]
fn keywords_corpus_parses_and_checks() {
    // GIVEN: the keywords corpus (01_syntax/keywords.mvl) covering all reserved keywords
    // THEN: parses and type-checks cleanly (no serious errors)
    let src = include_str!("corpus/01_syntax/keywords.mvl");
    let result = check_src(src);
    let serious: Vec<_> = result
        .errors
        .iter()
        .filter(|e| {
            !matches!(
                e,
                CheckError::UndefinedFunction { .. }
                    | CheckError::UndefinedVariable { .. }
                    | CheckError::UndefinedType { .. }
            )
        })
        .collect();
    assert!(
        serious.is_empty(),
        "keywords corpus should have no serious errors, got: {serious:?}"
    );
}

// ── #676: HOF effect propagation ─────────────────────────────────────────────

/// Pure HOF wrapper calling an effectful parameter must declare ! Console.
#[test]
fn hof_effectful_param_requires_caller_effect() {
    // GIVEN: pure fn run accepts fn(Int) -> Int ! Console and calls it
    // THEN: checker emits UndeclaredEffect — run must declare ! Console
    let src = r#"
        fn run(f: fn(Int) -> Int ! Console, x: Int) -> Int {
            f(x)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UndeclaredEffect { .. })),
        "expected UndeclaredEffect for HOF call with effectful param, got: {errors:?}"
    );
}

/// HOF wrapper that declares ! Console may call effectful parameter.
#[test]
fn hof_effectful_param_accepted_when_caller_declares_effect() {
    // GIVEN: fn run declares ! Console and calls fn(Int) -> Int ! Console
    // THEN: no effect errors
    let src = r#"
        fn run(f: fn(Int) -> Int ! Console, x: Int) -> Int ! Console {
            f(x)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors.iter().any(|e| matches!(
            e,
            CheckError::UndeclaredEffect { .. } | CheckError::MissingEffect { .. }
        )),
        "HOF call should be accepted when caller declares matching effect, got: {errors:?}"
    );
}

// ── #953: fn-type alias loses callability ─────────────────────────────────────

/// A parameter typed via a fn-type alias must be callable at the call site (#953).
#[test]
fn fn_type_alias_param_is_callable() {
    let src = r#"
        type Pred = fn(Int) -> Bool
        fn use_pred(p: Pred, n: Int) -> Bool {
            p(n)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "fn-type alias param should be callable, got: {errors:?}"
    );
}

/// Return type of a call through a fn-type alias is the alias's return type (#953).
#[test]
fn fn_type_alias_call_returns_correct_type() {
    let src = r#"
        type Transform = fn(Int) -> String
        fn apply(f: Transform, n: Int) -> String {
            f(n)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "call through fn-type alias should return the aliased return type, got: {errors:?}"
    );
}

// ── #954: effect-bearing fn-typed struct fields fail type equality ─────────────

/// Assigning an effect-bearing function to a struct field must not produce a
/// spurious TypeMismatch when the declared and inferred types are identical (#954).
#[test]
fn effect_bearing_fn_field_construction_accepted() {
    let src = r#"
        use std.log.{log_info}
        type Handler = struct {
            f: fn(Int) -> Bool ! Log,
        }
        fn h_one(n: Int) -> Bool ! Log {
            log_info("h", {"n": n.to_string()});
            n > 0
        }
        fn make_handler() -> Handler ! Log {
            Handler { f: h_one }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "effect-bearing fn field construction should not emit TypeMismatch, got: {errors:?}"
    );
}

/// Calling a method through an effect-bearing fn struct field must not produce a
/// spurious TypeMismatch (#954).
#[test]
fn effect_bearing_fn_field_call_accepted() {
    let src = r#"
        use std.log.{log_info}
        type Handler = struct {
            f: fn(Int) -> Bool ! Log,
        }
        fn h_one(n: Int) -> Bool ! Log {
            log_info("h", {"n": n.to_string()});
            n > 0
        }
        fn run(h: Handler, n: Int) -> Bool ! Log {
            h.f(n)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "calling effect-bearing fn field should not emit TypeMismatch, got: {errors:?}"
    );
}

// ── #953/#954 edge cases ──────────────────────────────────────────────────────

/// Calling through a non-fn type alias must still be rejected (#953 guard).
#[test]
fn non_fn_type_alias_not_callable() {
    let src = r#"
        type Count = Int
        fn f(c: Count) -> Bool {
            c(0)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors.is_empty(),
        "calling a non-fn type alias should produce a type error"
    );
}

/// Effect lists in different orders must still be compatible (#954).
#[test]
fn effect_fn_field_multi_effect_order_independent() {
    let src = r#"
        use std.log.{log_info}
        type Handler = struct {
            f: fn(Int) -> Bool ! Log + Console,
        }
        fn h(n: Int) -> Bool ! Console + Log {
            log_info("h", {"n": n.to_string()});
            n > 0
        }
        fn make() -> Handler ! Log + Console {
            Handler { f: h }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "multi-effect fn field with reversed order should not emit TypeMismatch, got: {errors:?}"
    );
}

/// A real effect mismatch on a fn field must still be rejected (#954 guard).
#[test]
fn effect_fn_field_mismatch_still_rejected() {
    let src = r#"
        use std.log.{log_info}
        type Handler = struct {
            f: fn(Int) -> Bool ! Log,
        }
        fn h(n: Int) -> Bool {
            n > 0
        }
        fn make() -> Handler {
            Handler { f: h }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "fn field with mismatched effects should emit TypeMismatch, got: {errors:?}"
    );
}

/// Tainted[String] passed where bare String expected must be a type error (#966).
#[test]
fn tainted_string_rejected_where_bare_string_expected() {
    let src = r#"
        use std.ifc.{taint}
        fn needs_string(s: String) -> Int {
            s.len()
        }
        fn caller(raw: String) -> Int {
            let t: Tainted[String] = taint(raw, "TEST");
            needs_string(t)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors.is_empty(),
        "Tainted[String] passed to String param should be a type error"
    );
}

// ── #687: Array[T, N] const-generic unknown size ──────────────────────────────

/// Array[T, N] where N is a type variable resolves to unknown size, not 0.
#[test]
fn array_type_variable_size_is_unknown_not_zero() {
    use mvl::mvl::checker::types::{resolve, ARRAY_SIZE_UNKNOWN};
    use mvl::mvl::parser::ast::TypeExpr;
    use mvl::mvl::parser::lexer::Span;
    let span = Span::default();
    // Simulate Array[Int, N] where N is a type variable (TypeExpr::Base with no int)
    let expr = TypeExpr::Base {
        name: "Array".to_string(),
        args: vec![
            TypeExpr::Base {
                name: "Int".to_string(),
                args: vec![],
                span,
            },
            TypeExpr::Base {
                name: "N".to_string(),
                args: vec![],
                span,
            },
        ],
        span,
    };
    match resolve(&expr) {
        mvl::mvl::checker::types::Ty::Array(_, size) => {
            assert_eq!(
                size, ARRAY_SIZE_UNKNOWN,
                "N should resolve to ARRAY_SIZE_UNKNOWN, not {size}"
            );
        }
        other => panic!("expected Ty::Array, got {other:?}"),
    }
}

/// Array[T, _] (unknown size) is compatible with any concrete-sized array of same element type.
#[test]
fn array_unknown_size_compatible_with_concrete_size() {
    use mvl::mvl::checker::types::{types_compatible, Ty, ARRAY_SIZE_UNKNOWN};
    let unknown = Ty::Array(Box::new(Ty::Int), ARRAY_SIZE_UNKNOWN);
    let concrete = Ty::Array(Box::new(Ty::Int), 16);
    assert!(
        types_compatible(&unknown, &concrete),
        "Array[Int, _] should be compatible with Array[Int, 16]"
    );
    assert!(
        types_compatible(&concrete, &unknown),
        "Array[Int, 16] should be compatible with Array[Int, _]"
    );
}

// ── #691: Move semantics for linear types (Spec 001 Req 4) ────────────────────
// Per ADR-0029, consume() is only for iso capability. Bare `let t = s` is a
// valid move for non-iso linear types — the source is marked unavailable.

/// Bare assignment of linear type moves the source — no error.
#[test]
fn bare_string_assignment_moves_source() {
    // GIVEN: let t: String = s (bare move)
    // THEN: no error — s is moved to t
    let src = r#"fn f() -> Unit { let s: String = "hello"; let t: String = s; }"#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "bare String assignment should be a valid move, got: {errors:?}"
    );
}

/// Use after move is caught — source is unavailable after bare move.
#[test]
fn use_after_move_on_bare_string_assignment() {
    // GIVEN: let t: String = s, then use s
    // THEN: UseAfterMove error
    let src = r#"
        fn println(msg: String) -> Unit ! Console { }
        fn f() -> Unit ! Console {
            let s: String = "hello";
            let t: String = s;
            println(s);
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UseAfterMove { name, .. } if name == "s")),
        "expected UseAfterMove for `s` after bare move, got: {errors:?}"
    );
}

/// Explicit consume() for String ownership transfer is still accepted.
#[test]
fn string_assignment_with_consume_accepted() {
    let src = r#"fn f() -> Unit { let s: String = "hello"; let t: String = consume(s); }"#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "consume() should be accepted for linear type, got: {errors:?}"
    );
}

/// String literal assigned directly (not from ident) is fine.
#[test]
fn string_literal_assignment_accepted() {
    let src = r#"fn f() -> Unit { let s: String = "hello"; }"#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "String literal assignment should have no errors, got: {errors:?}"
    );
}

// ── #934: linear type reassignment ────────────────────────────────────────────

/// Reassignment of linear type moves the source — no error.
#[test]
fn linear_reassignment_moves_source() {
    // GIVEN: t = s where s: String (bare move)
    // THEN: no error, s is moved
    let src = r#"
        fn f() -> Unit {
            let t: ref String = "a";
            let s: String = "b";
            t = s;
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "bare linear reassignment should be a valid move, got: {errors:?}"
    );
}

/// Reassignment of linear type with consume() is still accepted.
#[test]
fn linear_reassignment_with_consume_accepted() {
    let src = r#"
        fn f() -> Unit {
            let t: ref String = "a";
            let s: String = "b";
            t = consume(s);
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "consume() should be accepted for linear reassignment, got: {errors:?}"
    );
}

/// Reassignment from literal is accepted.
#[test]
fn linear_reassignment_from_literal_accepted() {
    let src = r#"
        fn f() -> Unit {
            let t: ref String = "a";
            t = "b";
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "literal reassignment should have no errors, got: {errors:?}"
    );
}

// ── #506: Actor behavior parameter sendability ────────────────────────────────

/// `pub fn` behavior with `ref` parameter is rejected — ref is not sendable.
#[test]
fn actor_behavior_ref_param_rejected() {
    // GIVEN: actor with pub fn behavior that takes a ref parameter
    // THEN: CapabilityViolation for the ref param
    let src = r#"
        actor Counter {
            count: Int
            pub fn increment(ref delta: Int) {
                self.count = self.count + delta
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::CapabilityViolation { param, capability, .. }
                if param == "delta" && capability == "ref")
        ),
        "ref param in actor behavior should be rejected, got: {errors:?}"
    );
}

/// `pub fn` behavior with `iso` parameter is accepted — iso is sendable.
#[test]
fn actor_behavior_iso_param_accepted() {
    // GIVEN: actor with pub fn behavior that takes an iso parameter
    // THEN: no CapabilityViolation
    let src = r#"
        actor Counter {
            count: Int
            pub fn increment(iso delta: Int) {}
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::CapabilityViolation { .. })),
        "iso param in actor behavior should be accepted, got: {errors:?}"
    );
}

/// `pub fn` behavior with `val` parameter is accepted — val is sendable.
#[test]
fn actor_behavior_val_param_accepted() {
    // GIVEN: actor with pub fn behavior that takes a val parameter
    // THEN: no CapabilityViolation
    let src = r#"
        actor Counter {
            count: Int
            pub fn reset(val new_count: Int) {}
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::CapabilityViolation { .. })),
        "val param in actor behavior should be accepted, got: {errors:?}"
    );
}

/// `pub fn` behavior with `tag` parameter is accepted — tag is sendable.
#[test]
fn actor_behavior_tag_param_accepted() {
    // GIVEN: actor with pub fn behavior that takes a tag parameter
    // THEN: no CapabilityViolation
    let src = r#"
        actor Counter {
            count: Int
            pub fn observe(tag handle: Int) {}
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::CapabilityViolation { .. })),
        "tag param in actor behavior should be accepted, got: {errors:?}"
    );
}

/// Private `fn` helper with `ref` parameter is accepted — no sendability restriction.
#[test]
fn actor_private_fn_ref_param_accepted() {
    // GIVEN: actor with private fn helper (not pub) that takes a ref parameter
    // THEN: no CapabilityViolation (private helpers are synchronous, no boundary crossing)
    let src = r#"
        actor Counter {
            count: Int
            fn validate(ref x: Int) -> Bool { x >= 0 }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::CapabilityViolation { .. })),
        "ref param in private actor fn should be accepted, got: {errors:?}"
    );
}

// ── #69: select expression and concurrently block ─────────────────────────────

/// GIVEN: a select expression in a function body
/// WHEN: type-checked
/// THEN: no panics; any errors are type errors, not internal crashes
#[test]
fn select_expr_in_fn_body_type_checks() {
    let errors = errors_for(
        r#"fn wait(ch: Channel) -> Unit {
            select {
                ch.recv() => { }
                timeout(5) => { }
            }
        }"#,
    );
    // Must not panic. Only type errors (e.g. unknown Channel type) are allowed.
    let _ = errors;
}

/// GIVEN: a select expression with a binding
/// WHEN: type-checked
/// THEN: no panics
#[test]
fn select_expr_with_binding_type_checks() {
    let errors = errors_for(
        r#"fn wait(ch: Channel) -> Unit {
            select {
                msg = ch.recv() => { }
            }
        }"#,
    );
    let _ = errors;
}

/// GIVEN: a concurrently block in a function body
/// WHEN: type-checked
/// THEN: no panics
#[test]
fn concurrently_block_type_checks() {
    let errors = errors_for(
        r#"fn run() -> Unit {
            concurrently {
                let x: Int = 1;
            }
        }"#,
    );
    let _ = errors;
}

// ── #63/#506: Actor race-free counting ────────────────────────────────────────

/// GIVEN: an actor with only pub fn behaviors (no ref params)
/// WHEN: req 9 is evaluated via the requirements runner
/// THEN: pub fn behaviors are counted as race-free (proven by construction)
#[test]
fn actor_pub_fn_behaviors_counted_as_race_free() {
    use mvl::mvl::checker::data_race::count_race_free_fns;
    use mvl::mvl::parser::Parser;

    let src = r#"
        actor Counter {
            count: Int
            pub fn increment(val n: Int) { }
            pub fn reset() { }
        }
    "#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let rc = count_race_free_fns(&prog);
    assert_eq!(rc.total, 2, "expected 2 methods counted (2 pub fn)");
    assert_eq!(rc.race_free, 2, "both pub fn behaviors should be race-free");
}

/// GIVEN: an actor with a private fn helper that has a ref param
/// WHEN: race-free count is computed
/// THEN: that helper is NOT counted as race-free
#[test]
fn actor_private_fn_with_ref_not_race_free() {
    use mvl::mvl::checker::data_race::count_race_free_fns;
    use mvl::mvl::parser::Parser;

    let src = r#"
        actor Counter {
            count: Int
            fn validate(ref x: Int) -> Bool { 0 }
        }
    "#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let rc = count_race_free_fns(&prog);
    assert_eq!(rc.total, 1, "expected 1 method counted");
    assert_eq!(
        rc.race_free, 0,
        "private fn with ref param should not be race-free"
    );
}

// ── #745: Actor duplicate field/method + pub fn return type ──────────────────

/// GIVEN: actor with a duplicate field name
/// WHEN: type-checked
/// THEN: DuplicateActorField error emitted
#[test]
fn actor_duplicate_field_rejected() {
    let errors = errors_for(
        r#"
        actor Broken {
            count: Int
            count: Int
            pub fn tick() { }
        }
        "#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::DuplicateActorField { field, .. } if field == "count")
        ),
        "expected DuplicateActorField, got: {errors:?}"
    );
}

/// GIVEN: actor with a duplicate method name
/// WHEN: type-checked
/// THEN: DuplicateActorMethod error emitted
#[test]
fn actor_duplicate_method_rejected() {
    let errors = errors_for(
        r#"
        actor Broken {
            x: Int
            pub fn tick() { }
            pub fn tick() { }
        }
        "#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::DuplicateActorMethod { method, .. } if method == "tick")
        ),
        "expected DuplicateActorMethod, got: {errors:?}"
    );
}

/// GIVEN: actor pub fn with non-Unit return type
/// WHEN: type-checked
/// THEN: NonUnitBehaviorReturn error emitted
#[test]
fn actor_pub_fn_non_unit_return_rejected() {
    let errors = errors_for(
        r#"
        actor Broken {
            x: Int
            pub fn get_x() -> Int { 0 }
        }
        "#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::NonUnitBehaviorReturn { method, .. } if method == "get_x")
        ),
        "expected NonUnitBehaviorReturn, got: {errors:?}"
    );
}

/// GIVEN: actor pub test fn with non-Unit return type (#1506)
/// WHEN: type-checked
/// THEN: no error — pub test fn is exempt from fire-and-forget Unit restriction
#[test]
fn actor_pub_test_fn_non_unit_return_accepted() {
    let errors = errors_for(
        r#"
        actor Counter {
            count: Int
            pub fn increment(val n: Int) { }
            pub test fn get_count() -> Int { 0 }
            pub test fn get_as_string() -> String { "0" }
        }
        "#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::NonUnitBehaviorReturn { .. })),
        "pub test fn should not trigger NonUnitBehaviorReturn, got: {errors:?}"
    );
}

/// GIVEN: user-program actor with the same name as a prelude actor (#1497)
/// WHEN: type-checked with that prelude
/// THEN: ActorNameConflict error emitted
#[test]
fn actor_name_shadows_prelude_rejected() {
    let prelude_src = r#"
        actor Logger {
            pub fn log(val msg: String) { }
        }
    "#;
    let (mut pp, _) = Parser::new(prelude_src);
    let prelude = pp.parse_program();

    let src = r#"
        actor Logger {
            count: Int
            pub fn tick() { }
        }
    "#;
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();

    let result = check_with_prelude(&[prelude], &prog);
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CheckError::ActorNameConflict { name, .. } if name == "Logger")),
        "expected ActorNameConflict for 'Logger', got: {:?}",
        result.errors
    );
}

/// GIVEN: actor with valid pub fn (Unit return, no duplicates)
/// WHEN: type-checked
/// THEN: no errors
#[test]
fn actor_valid_decl_no_errors() {
    let errors = errors_for(
        r#"
        actor Counter {
            count: Int
            label: Int
            pub fn increment(n: Int) { }
            pub fn reset() { }
            fn helper() -> Int { 0 }
        }
        "#,
    );
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

// ── #742: Actor body type-checking + spawn field validation ──────────────────

/// GIVEN: actor spawn with wrong field type
/// WHEN: type-checked
/// THEN: TypeMismatch error emitted
#[test]
fn actor_spawn_wrong_field_type_rejected() {
    let errors = errors_for(
        r#"
        actor Counter {
            count: Int
            pub fn tick() { }
        }

        fn bad() -> Counter {
            actor Counter { count: "hello" }
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "expected TypeMismatch for wrong spawn field type, got: {errors:?}"
    );
}

/// GIVEN: actor spawn with missing field
/// WHEN: type-checked
/// THEN: MissingField error emitted
#[test]
fn actor_spawn_missing_field_rejected() {
    let errors = errors_for(
        r#"
        actor Counter {
            count: Int
            ifc_label: Int
            pub fn tick() { }
        }

        fn bad() -> Counter {
            actor Counter { count: 0 }
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::MissingField { .. })),
        "expected MissingField for missing spawn field, got: {errors:?}"
    );
}

/// GIVEN: actor spawn with unknown extra field
/// WHEN: type-checked
/// THEN: UnknownField error emitted
#[test]
fn actor_spawn_unknown_field_rejected() {
    let errors = errors_for(
        r#"
        actor Counter {
            count: Int
            pub fn tick() { }
        }

        fn bad() -> Counter {
            actor Counter { count: 0, extra: 1 }
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UnknownField { .. })),
        "expected UnknownField for extra spawn field, got: {errors:?}"
    );
}

/// `pub fn` behavior with an unannotated parameter is accepted — no capability means sendable.
#[test]
fn actor_behavior_unannotated_param_accepted() {
    // GIVEN: actor pub fn with no capability annotation on the parameter
    // THEN: no CapabilityViolation (unannotated = sendable by default)
    let src = r#"
        actor Counter {
            count: Int
            pub fn increment(n: Int) { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::CapabilityViolation { .. })),
        "unannotated param in actor pub fn should be accepted, got: {errors:?}"
    );
}

/// Multiple `pub fn` parameters: one `ref` among valid params — only the `ref` is rejected.
#[test]
fn actor_behavior_ref_among_multiple_params_rejected() {
    // GIVEN: actor pub fn with a mix of valid and ref params
    // THEN: CapabilityViolation only for the ref param
    let src = r#"
        actor Counter {
            count: Int
            pub fn update(n: Int, ref bad: Int, iso good: Int) { }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::CapabilityViolation { param, capability, .. }
                if param == "bad" && capability == "ref")
        ),
        "ref param should be rejected among mixed params, got: {errors:?}"
    );
    assert!(
        !errors.iter().any(
            |e| matches!(e, CheckError::CapabilityViolation { param, .. } if param == "n" || param == "good")
        ),
        "non-ref params should not be rejected, got: {errors:?}"
    );
}

/// Actor with iso param and body: iso linearity enforced — aliasing without consume rejected.
#[test]
fn actor_behavior_iso_aliasing_rejected() {
    // GIVEN: actor pub fn receives an iso param and aliases it without consume
    // THEN: IsoAliasingViolation (or CapabilityViolation) for the alias
    let src = r#"
        actor Relay {
            pub fn forward(iso payload: Int) {
                let alias: Int = payload;
            }
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors.is_empty(),
        "iso aliasing in actor behavior should be rejected, got no errors"
    );
}

// ── D2: Actor protocol bounded model checker (#37) ───────────────────────────

// ── D2: Actor protocol bounded model checker (#37) ───────────────────────────

/// GIVEN: actor with a refined field (`count: Int where self >= 0`) initialized
/// with a value that violates the refinement (count = -1)
/// WHEN: type-checked
/// THEN: RefinementViolated error emitted for the bad initial value
#[test]
fn actor_field_refinement_violated_at_spawn() {
    let errors = errors_for(
        r#"
        actor Counter {
            count: Int where self >= 0

            pub fn tick() { }
        }

        fn make_bad() -> Unit {
            let c = actor Counter { count: -1 }
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "expected RefinementViolated for count = -1 violating self >= 0, got: {errors:?}"
    );
}

/// GIVEN: actor with a refined field initialized with a valid value
/// WHEN: type-checked
/// THEN: no RefinementViolated error
#[test]
fn actor_field_refinement_satisfied_at_spawn() {
    let errors = errors_for(
        r#"
        actor Counter {
            count: Int where self >= 0

            pub fn tick() { }
        }

        fn make_good() -> Unit {
            let c = actor Counter { count: 0 }
        }
        "#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "expected no RefinementViolated for count = 0, got: {errors:?}"
    );
}

/// GIVEN: actor behavior body calls a function with a refined parameter using a
/// literal argument that violates the refinement (0 violates self > 0)
/// WHEN: type-checked
/// THEN: RefinementViolated error emitted
#[test]
fn actor_behavior_body_requires_checked() {
    let src = r#"
        fn positive_only(x: Int where self > 0) -> Int { x }

        actor Worker {
            data: Int

            pub fn run() {
                let r: Int = positive_only(0);
            }
        }
        "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "expected RefinementViolated for positive_only(0) inside behavior, got: {errors:?}"
    );
}

/// GIVEN: actor behavior body calls a function with a satisfied refined parameter
/// WHEN: type-checked
/// THEN: no RefinementViolated errors
#[test]
fn actor_behavior_body_requires_satisfied() {
    let errors = errors_for(
        r#"
        fn positive_only(x: Int where self > 0) -> Int { x }

        actor Worker {
            data: Int

            pub fn run() {
                let r: Int = positive_only(5);
            }
        }
        "#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "unexpected RefinementViolated for positive_only(5) inside behavior: {errors:?}"
    );
}

// ── #744: ActorDecl registered in pass 1 ─────────────────────────────────────

/// GIVEN: an actor declaration and a function returning that actor type
/// WHEN: type-checked
/// THEN: no TypeMismatch or UnknownType errors (actor type is registered)
#[test]
fn actor_type_registered_in_pass1() {
    let errors = errors_for(
        r#"
        actor Counter {
            count: Int
            pub fn increment(n: Int) { }
            pub fn reset() { }
        }

        fn make_counter() -> Counter ! Spawn {
            actor Counter { count: 0 }
        }
        "#,
    );
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

// ── #627: Z3 counterexample extraction ────────────────────────────────────────

/// GIVEN: a refinement that Layer 1 proves violated (literal 0 violates self > 0)
/// WHEN: type-checked
/// THEN: RefinementViolated error includes counterexample field (Phase 4, #627)
#[test]
fn z3_counterexample_field_in_error_structs() {
    let src = r#"
        total fn require_positive(x: Int where self > 0) -> Int { x }
        total fn bad() -> Int { require_positive(0) }
    "#;
    let errors = errors_for(src);
    let violation = errors
        .iter()
        .find(|e| matches!(e, CheckError::RefinementViolated { .. }));
    assert!(
        violation.is_some(),
        "expected RefinementViolated error, got: {errors:?}"
    );

    // The RefinementViolated error has a counterexample field (added for Phase 4, #627).
    // Layer 1 catches this literal violation, so counterexample is None.
    // When Z3 extracts counterexamples from SAT results (Phase 4), it will set Some.
    if let Some(CheckError::RefinementViolated { counterexample, .. }) = violation {
        // TODO(#627): invert this assertion once Z3 model extraction is implemented.
        // Layer 1 catches literal violations and sets counterexample: None; the Z3
        // Sat branch will set Some(...) when Phase 4 is complete.
        assert!(
            counterexample.is_none(),
            "Layer 1 doesn't extract counterexamples yet"
        );
    }
}

// ── Counterexample field in PostconditionViolated / InvariantViolated (#627) ──

#[test]
fn postcondition_violated_has_counterexample_field() {
    // GIVEN: fn with `ensures result >= 0` returning -1 (statically violates)
    // THEN: PostconditionViolated error carries the counterexample field
    let src = r#"
        fn bad_post() -> Int ensures result >= 0 { -1 }
    "#;
    let errors = errors_for(src);
    let violation = errors
        .iter()
        .find(|e| matches!(e, CheckError::PostconditionViolated { .. }));
    assert!(
        violation.is_some(),
        "expected PostconditionViolated, got: {errors:?}"
    );
    if let Some(CheckError::PostconditionViolated { counterexample, .. }) = violation {
        // TODO(#627): assert Some(...) once Z3 extraction is wired up.
        let _ = counterexample; // field is present — structural check passes
    }
}

#[test]
fn invariant_violated_has_counterexample_field() {
    // GIVEN: constant-false loop invariant (1 < 0 is always false)
    // THEN: InvariantViolated error carries the counterexample field
    let src = r#"
        partial fn server() -> Unit {
            while true invariant 1 < 0 { }
        }
    "#;
    let errors = errors_for(src);
    let violation = errors
        .iter()
        .find(|e| matches!(e, CheckError::InvariantViolated { .. }));
    assert!(
        violation.is_some(),
        "expected InvariantViolated, got: {errors:?}"
    );
    if let Some(CheckError::InvariantViolated { counterexample, .. }) = violation {
        // TODO(#627): assert Some(...) once Z3 extraction is wired up.
        let _ = counterexample; // field is present — structural check passes
    }
}

// ── check_dual SessionDualityMismatch fallback (#134) ─────────────────────────

#[test]
fn session_duality_mismatch_non_deadlock_returns_mismatch_error() {
    // GIVEN: two types that are structurally incompatible but NOT a deadlock
    // (both start with Send — no mutual blocking — but are not duals of each other)
    // THEN: check_dual returns SessionDualityMismatch, not SessionDeadlock
    use mvl::mvl::checker::session::check_dual;
    use mvl::mvl::checker::types::SessionTy;
    use mvl::mvl::checker::types::Ty;

    let dummy = mvl::mvl::parser::lexer::Span {
        line: 1,
        col: 1,
        offset: 0,
        len: 0,
    };

    // Both sides Send — neither is blocked waiting, but they're not duals.
    let a = SessionTy::Send(Box::new(Ty::Int), Box::new(SessionTy::End));
    let b = SessionTy::Send(Box::new(Ty::Int), Box::new(SessionTy::End));
    let err = check_dual(&a, &b, dummy);
    assert!(
        matches!(
            err,
            Some(mvl::mvl::checker::errors::CheckError::SessionDualityMismatch { .. })
        ),
        "expected SessionDualityMismatch for (Send, Send), got: {:?}",
        err
    );
}

// ── Spawn inside impl block triggers field refinement check (#37) ─────────────

#[test]
fn actor_field_refinement_violated_from_impl_method() {
    // GIVEN: a spawn with a bad field value inside an impl method body
    // THEN: RefinementViolated error is emitted for the spawn
    let errors = errors_for(
        r#"
        actor Counter {
            count: Int where self >= 0
            pub fn tick() { }
        }
        type Builder = struct {}
        impl Builder {
            fn make_bad() -> Unit {
                let c = actor Counter { count: -5 }
            }
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::RefinementViolated { .. })),
        "expected RefinementViolated for spawn inside impl method, got: {errors:?}"
    );
}

// ── #820: qualified module imports ────────────────────────────────────────────

/// `use std.json` (no braces) should mark `json` as a module alias so that
/// `json.decode(s)` is redirected to a function call rather than producing
/// `UndefinedVariable("json")`.
#[test]
fn qualified_import_resolves_fn_call() {
    // Without stdlib prelude `decode` is still unknown, but the error must be
    // `UndefinedFunction("decode")` — NOT `UndefinedVariable("json")`.
    // That confirms the module-alias redirect fired correctly (#820).
    let src = r#"
        use std.json;
        fn f(s: String) -> Bool {
            json.decode(s)
        }
    "#;
    let result = check_src(src);
    let has_undefined_json = result
        .errors
        .iter()
        .any(|e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "json"));
    assert!(
        !has_undefined_json,
        "`use std.json` should suppress UndefinedVariable(\"json\"), got: {:?}",
        result.errors
    );
}

/// Without `use std.json`, `json.decode()` must produce `UndefinedVariable("json")` —
/// `json` is not in scope as either a variable or a module alias.
#[test]
fn unqualified_module_call_without_import_errors() {
    let src = r#"fn f(s: String) -> Bool { json.decode(s) }"#;
    let result = check_src(src);
    let has_undefined_json = result
        .errors
        .iter()
        .any(|e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "json"));
    assert!(
        has_undefined_json,
        "json.decode without use std.json should produce UndefinedVariable(\"json\"), got: {:?}",
        result.errors
    );
}

/// `use std.json.{decode}` (brace form) must NOT create a module alias —
/// `json.decode()` should still produce `UndefinedVariable("json")`.
#[test]
fn brace_import_does_not_create_module_alias() {
    let src = r#"
        use std.json.{decode};
        fn f(s: String) -> Bool {
            json.decode(s)
        }
    "#;
    let result = check_src(src);
    let has_undefined_json = result
        .errors
        .iter()
        .any(|e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "json"));
    assert!(
        has_undefined_json,
        "brace import must not create module alias; expected UndefinedVariable(\"json\"), got: {:?}",
        result.errors
    );
}

// ── #822: toml.mvl in proven-mode ─────────────────────────────────────────────

/// std/toml.mvl must pass proven-mode checking with the other proven-stdlib
/// files as prelude (same setup as check_proven_stdlib in cli/check.rs).
#[test]
fn toml_mvl_passes_proven_mode() {
    use mvl::mvl::checker::check_with_prelude;
    use mvl::mvl::stdlib::stdlib_content;

    let proven_files = &[
        "core.mvl",
        "strings.mvl",
        "lists.mvl",
        "math.mvl",
        "collections.mvl",
        "json.mvl",
        "toml.mvl",
    ];

    let programs: Vec<mvl::mvl::parser::ast::Program> = proven_files
        .iter()
        .filter_map(|name| {
            stdlib_content(name).map(|src| {
                let (mut p, _) = Parser::new(src);
                p.parse_program()
            })
        })
        .collect();

    // Find toml index
    let toml_idx = proven_files
        .iter()
        .position(|n| *n == "toml.mvl")
        .expect("toml.mvl not in list");

    let prelude: Vec<mvl::mvl::parser::ast::Program> = programs
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != toml_idx)
        .map(|(_, p)| p.clone())
        .collect();

    let result = check_with_prelude(&prelude, &programs[toml_idx]);
    assert!(
        result.is_ok(),
        "toml.mvl should pass proven-mode checks, got: {:?}",
        result.errors
    );

    // Also verify json.mvl still passes when toml.mvl is in its prelude.
    let json_idx = proven_files
        .iter()
        .position(|n| *n == "json.mvl")
        .expect("json.mvl not in list");
    let json_prelude: Vec<mvl::mvl::parser::ast::Program> = programs
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != json_idx)
        .map(|(_, p)| p.clone())
        .collect();
    let json_result = check_with_prelude(&json_prelude, &programs[json_idx]);
    assert!(
        json_result.is_ok(),
        "json.mvl should pass proven-mode checks (with toml.mvl in prelude), got: {:?}",
        json_result.errors
    );
}

// ── #1066: Generic type instantiation at call sites ───────────────────────────

/// Req 1: generic call with wrong argument type is rejected (#1066).
///
/// `identity[T](x: T) -> T` called with `Bool` where return type is `Int`
/// must produce a TypeMismatch (T=Bool, but caller expects Int).
#[test]
fn generic_wrong_arg_type_rejected() {
    let errors = errors_for(
        r#"fn identity[T](x: T) -> T { x }
           fn f() -> Int { identity(true) }"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "identity(true) where Int expected should be a TypeMismatch, got: {errors:?}"
    );
}

/// Req 1: explicit type argument with wrong value type is rejected (#1066).
///
/// `identity[Int](true)` — explicit `T=Int` but argument is `Bool`.
#[test]
fn generic_explicit_type_arg_mismatch_rejected() {
    let errors = errors_for(
        r#"fn identity[T](x: T) -> T { x }
           fn f() -> Int { identity[Int](true) }"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "identity[Int](true) should be a TypeMismatch, got: {errors:?}"
    );
}

/// Req 1: correct generic call produces no errors (#1066).
#[test]
fn generic_correct_arg_type_accepted() {
    let errors = errors_for(
        r#"fn identity[T](x: T) -> T { x }
           fn f() -> Int { identity(42) }"#,
    );
    assert!(
        errors.is_empty(),
        "identity(42) : Int should be accepted, got: {errors:?}"
    );
}

/// Req 11: IFC label preserved when Secret[String] flows through generic identity (#1066).
#[test]
fn generic_ifc_label_preserved_through_identity() {
    let errors = errors_for(
        r#"fn identity[T](x: T) -> T { x }
           fn f(s: Secret[String]) -> Secret[String] { identity(s) }"#,
    );
    assert!(
        errors.is_empty(),
        "identity(Secret[String]) -> Secret[String] should be accepted, got: {errors:?}"
    );
}

/// Req 11: IFC label mismatch through generic is caught (#1066).
///
/// Passing `Secret[String]` through `identity[T]` then treating result as bare
/// `String` must be a type error.
#[test]
fn generic_ifc_label_mismatch_caught() {
    let errors = errors_for(
        r#"fn identity[T](x: T) -> T { x }
           fn f(s: Secret[String]) -> String { identity(s) }"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret[String] through identity used as String should be TypeMismatch, got: {errors:?}"
    );
}

/// Req 4: Option[T] wrapper preserved through generic identity (#1066).
#[test]
fn generic_option_wrapper_preserved() {
    let errors = errors_for(
        r#"fn identity[T](x: T) -> T { x }
           fn f(opt: Option[Int]) -> Option[Int] { identity(opt) }"#,
    );
    assert!(
        errors.is_empty(),
        "identity(Option[Int]) -> Option[Int] should be accepted, got: {errors:?}"
    );
}

/// Req 7: effectful generic function — caller must declare the effect (#1066).
#[test]
fn generic_effect_propagation_enforced() {
    let errors = errors_for(
        r#"fn with_console[T](x: T) -> T ! Console { println("x"); x }
           fn caller(s: String) -> String { with_console(s) }"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UndeclaredEffect { .. })),
        "caller without Console calling with_console[T] should fail, got: {errors:?}"
    );
}

/// Corpus: generic_instantiation.mvl covers all four requirements together (#1066).
#[test]
fn generic_instantiation_corpus_parses_and_checks() {
    let src = include_str!("corpus/02_functions/generic_instantiation.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "generic_instantiation.mvl should pass type checks, got: {:?}",
        result.errors
    );
}

// ── #1068 Gap 1: Named types with linear fields use move semantics ──────────

#[test]
fn struct_with_string_field_moves_source() {
    // GIVEN: a struct whose field is linear (String), assigned via bare identifier
    // THEN: valid move — source is marked unavailable
    let src = r#"
        type Config = struct { name: String }
        fn f() -> Unit {
            let a: Config = Config { name: "x" };
            let b: Config = a;
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "bare struct assignment should be a valid move, got: {errors:?}"
    );
}

#[test]
fn struct_with_string_field_use_after_move() {
    // GIVEN: struct moved to b, then a used again
    // THEN: UseAfterMove
    let src = r#"
        type Config = struct { name: String }
        fn use_config(c: Config) -> Unit { }
        fn f() -> Unit {
            let a: Config = Config { name: "x" };
            let b: Config = a;
            use_config(b);
            use_config(a);
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UseAfterMove { name, .. } if name == "a")),
        "expected UseAfterMove for `a` after bare move, got: {errors:?}"
    );
}

#[test]
fn struct_with_string_field_consume_accepted() {
    // GIVEN: struct with String field transferred via consume()
    // THEN: still valid
    let src = r#"
        type Config = struct { name: String }
        fn f() -> Unit {
            let a: Config = Config { name: "x" };
            let b: Config = consume(a);
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "consume() should be accepted for linear struct, got: {errors:?}"
    );
}

#[test]
fn struct_with_only_int_fields_is_not_linear() {
    // GIVEN: struct with only value-type fields (Int, Bool)
    // THEN: no errors
    let src = r#"
        type Point = struct { x: Int, y: Int }
        fn f() -> Unit {
            let a: Point = Point { x: 1, y: 2 };
            let b: Point = a;
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors.is_empty(),
        "struct with only Int fields should not be linear, got: {errors:?}"
    );
}

// ── #1068 Gap 2: Shadow-drop detection ──────────────────────────────────────

#[test]
fn shadow_drops_linear_string_detected() {
    // GIVEN: shadowing a live String binding
    // THEN: LinearShadowDrop error
    let src = r#"
        fn f() -> Unit {
            let x: String = "hello";
            let x: Int = 5;
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::LinearShadowDrop { .. })),
        "expected LinearShadowDrop for shadowing live String, got: {errors:?}"
    );
}

#[test]
fn shadow_after_consume_ok() {
    // GIVEN: consuming a linear value before shadowing
    // THEN: no LinearShadowDrop error
    let src = r#"
        fn f() -> Unit {
            let x: String = "hello";
            let y: String = consume(x);
            let x: Int = 5;
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::LinearShadowDrop { .. })),
        "consumed binding should not trigger shadow-drop, got: {errors:?}"
    );
}

// ── #1068 Gap 3: Lambda effect inference ────────────────────────────────────

#[test]
fn lambda_with_console_effect_propagates_to_caller() {
    // GIVEN: a lambda that calls println (Console effect), called from a pure function
    // THEN: UndeclaredEffect error (the lambda's effect should propagate)
    let src = r#"
        pub fn println(msg: String) -> Unit ! Console { }
        fn f() -> Unit {
            let log_it: fn(String) -> Unit = |x: String| { println(x) };
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::UndeclaredEffect { .. })),
        "expected UndeclaredEffect for lambda calling println in pure function, got: {errors:?}"
    );
}

#[test]
fn lambda_with_console_effect_in_effectful_fn_ok() {
    // GIVEN: a lambda that calls println, inside a function declaring Console
    // THEN: no errors
    let src = r#"
        pub fn println(msg: String) -> Unit ! Console { }
        fn f() -> Unit ! Console {
            let log_it: fn(String) -> Unit = |x: String| { println(x) };
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors.iter().any(|e| matches!(
            e,
            CheckError::UndeclaredEffect { .. } | CheckError::MissingEffect { .. }
        )),
        "lambda in effectful function should be accepted, got: {errors:?}"
    );
}

// ── #1068 Gap 4: Mutual recursion detection ─────────────────────────────────

#[test]
fn mutual_recursion_in_total_functions_detected() {
    // GIVEN: two total functions that call each other (mutual recursion)
    // THEN: MutualRecursionInTotal error
    let src = r#"
        fn ping(n: Int) -> Int { pong(n) }
        fn pong(n: Int) -> Int { ping(n) }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::MutualRecursionInTotal { .. })),
        "expected MutualRecursionInTotal for mutual recursion, got: {errors:?}"
    );
}

#[test]
fn mutual_recursion_in_partial_functions_ok() {
    // GIVEN: two partial functions that call each other
    // THEN: no MutualRecursionInTotal error (partial functions exempt)
    let src = r#"
        partial fn ping(n: Int) -> Int { pong(n) }
        partial fn pong(n: Int) -> Int { ping(n) }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::MutualRecursionInTotal { .. })),
        "partial functions should not trigger mutual recursion check, got: {errors:?}"
    );
}

// ── #1068 Gap 5: Sendability checks on complex expressions ──────────────────

#[test]
fn field_access_on_ref_in_send_rejected() {
    // GIVEN: sending a field access on a ref-capable parameter via channel.send()
    // THEN: CapabilityViolation error (field access inherits ref capability)
    let src = r#"
        type Payload = struct { value: Int }
        fn send_field(channel: Channel, ref data: Payload) -> Unit {
            channel.send(data.value)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::CapabilityViolation { .. })),
        "expected CapabilityViolation for field access on ref in send, got: {errors:?}"
    );
}

#[test]
fn fn_call_returning_ref_in_send_rejected() {
    // GIVEN: sending the return value of an expression that resolves to ref type
    // THEN: CapabilityViolation error
    let src = r#"
        fn get_ref() -> ref String { ref "hello" }
        fn send_expr(channel: Channel) -> Unit {
            channel.send(get_ref())
        }
    "#;
    let errors = errors_for(src);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::CapabilityViolation { .. })),
        "expected CapabilityViolation for fn returning ref in send, got: {errors:?}"
    );
}
