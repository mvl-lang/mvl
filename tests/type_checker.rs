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
use mvl::mvl::checker::{check, CheckResult};
use mvl::mvl::parser::Parser;

fn check_src(src: &str) -> CheckResult {
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    check(&prog)
}

fn errors_for(src: &str) -> Vec<CheckError> {
    check_src(src).errors
}

// ── #11: Basic type inference (Requirement 1) ────────────────────────────────

#[test]
fn basic_types_corpus_parses_and_checks() {
    // GIVEN: the basic_types corpus (valid programs)
    // THEN: no type errors
    let src = include_str!("corpus/02_types/basic_types.mvl");
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
    let src = include_str!("corpus/02_types/adt_checking.mvl");
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
    let src = include_str!("corpus/02_types/exhaustive_match.mvl");
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
    let errors = errors_for("fn f(r: Result<Int, String>) -> Int { match r { Err(_) => -1 } }");
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
    let src = include_str!("corpus/02_types/option_result.mvl");
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
    // GIVEN: direct `.field` on Option<T>
    // THEN: OptionDirectAccess reported
    let errors = errors_for("fn f(x: Option<Int>) -> Int { x.value }");
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
        errors_for("fn produce() -> Result<Int, String> { Ok(1) }\nfn f() -> Unit { produce() }");
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
    let errors = errors_for("fn f() -> Result<Int, String> { let x = 1?; Ok(x) }");
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
    let src = include_str!("corpus/02_types/immutability.mvl");
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

#[test]
fn immutable_binding_assignment_rejected() {
    // GIVEN: assignment to `let x` (no `mut`)
    // THEN: AssignToImmutable reported
    let errors = errors_for("fn f() -> Int { let x = 1; x = 2; x }");
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
    let src = "type Pt = struct { x: Int, mut y: Int }\nfn f(mut p: Pt) -> Unit { p.x = 5; }";
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
    let src = include_str!("corpus/03_ownership/ownership.mvl");
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
    let errors = errors_for("fn f() -> Int { let x = 1; let _y = move(x); x }");
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
    let src = include_str!("corpus/06_refinements/refinements_valid.mvl");
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

// ── #19: Effect checking — reject side effects in pure functions ──────────────

#[test]
fn pure_vs_effectful_corpus_parses_and_checks() {
    // GIVEN: valid corpus of pure/effectful declarations with correct annotations
    // THEN: no type errors
    let src = include_str!("corpus/04_effects/pure_vs_effectful.mvl");
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
    let src = include_str!("corpus/04_effects/propagation.mvl");
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
    // GIVEN: fn a ! FileRead, fn b ! Net, fn c ! FileRead, Net calls both
    // THEN: no effect errors
    let src = r#"
        fn read_fn() -> Unit ! FileRead { file.read("x") }
        fn net_fn() -> Unit ! Net { http.get("url") }
        fn union_caller() -> Unit ! FileRead, Net { read_fn(); net_fn() }
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
    let src = include_str!("corpus/07_termination/total_vs_partial.mvl");
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
    let src = "total fn f(items: List<Int>) -> Unit { for x in items { } }";
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

// ── #22: Reference capability checking — iso/val/ref/tag on actor boundaries ──

#[test]
fn capabilities_corpus_parses_and_checks() {
    // GIVEN: valid corpus of capability-annotated functions
    // THEN: no type errors
    let src = include_str!("corpus/08_concurrency/capabilities.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "capabilities corpus should type-check cleanly, got: {:?}",
        result.errors
    );
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

// ── #24: Security label checking (Requirement 11) ────────────────────────────

#[test]
fn labels_corpus_parses_and_checks() {
    // GIVEN: the existing labels corpus (valid labeled programs)
    // THEN: no IFC violations (UndefinedFunction for stdlib is OK)
    let src = include_str!("corpus/05_ifc/labels.mvl");
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
    let src = include_str!("corpus/05_ifc/label_types.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "label_types corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn secret_flows_to_public_rejected() {
    // GIVEN: a function returning Public<String> but body is Secret<String>
    // THEN: TypeMismatch (downward flow rejected)
    let errors = errors_for(r#"fn leak(k: Secret<String>) -> Public<String> { k }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "expected TypeMismatch for Secret→Public leak, got: {errors:?}"
    );
}

#[test]
fn public_flows_to_secret_accepted() {
    // GIVEN: a function accepting Public<String> parameter assigned to Secret<String>
    // THEN: no type error (upward flow)
    let errors = errors_for(r#"fn store(x: Public<String>) -> Secret<String> { x }"#);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "upward flow Public→Secret should be accepted, got: {errors:?}"
    );
}

#[test]
fn tainted_flows_to_clean_rejected() {
    // GIVEN: a function returning Clean<String> but body is Tainted<String>
    // THEN: TypeMismatch (downward flow rejected — needs sanitize)
    let errors = errors_for(r#"fn use_raw(input: Tainted<String>) -> Clean<String> { input }"#);
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
    let src = include_str!("corpus/05_ifc/lattice.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "lattice corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn secret_to_tainted_rejected() {
    // GIVEN: function returns Tainted<Int> but body is Secret<Int> (downward)
    // THEN: TypeMismatch
    let errors = errors_for(r#"fn downgrade(s: Secret<Int>) -> Tainted<Int> { s }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "expected TypeMismatch for Secret→Tainted downgrade, got: {errors:?}"
    );
}

#[test]
fn clean_to_public_rejected() {
    // GIVEN: function returns Public<Int> but body is Clean<Int> (downward)
    // THEN: TypeMismatch
    let errors = errors_for(r#"fn expose(s: Clean<Int>) -> Public<Int> { s }"#);
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
    let src = include_str!("corpus/05_ifc/propagation.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "propagation corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn arithmetic_label_join_propagates() {
    // GIVEN: Secret<Int> + Public<Int> — the result carries the join (Secret)
    // THEN: no type error when assigned to Secret<Int>
    let errors = errors_for(r#"fn add(a: Secret<Int>, b: Public<Int>) -> Secret<Int> { a + b }"#);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret<Int> + Public<Int> should yield Secret<Int>, got: {errors:?}"
    );
}

#[test]
fn arithmetic_label_join_downgrade_rejected() {
    // GIVEN: Secret<Int> + Public<Int> — trying to assign to Public<Int>
    // THEN: TypeMismatch (result is Secret, expected Public)
    let errors = errors_for(r#"fn add(a: Secret<Int>, b: Public<Int>) -> Public<Int> { a + b }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret+Public result cannot flow to Public<Int>, got: {errors:?}"
    );
}

// ── #27: Declassify/sanitize as auditable chokepoints ────────────────────────

#[test]
fn declassification_corpus_parses_and_checks() {
    // GIVEN: declassification corpus (valid declassify/sanitize usage)
    // THEN: no type errors (UndefinedFunction for User types is OK)
    let src = include_str!("corpus/05_ifc/declassification.mvl");
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
fn sanitize_tainted_returns_clean() {
    // GIVEN: sanitize(tainted_string) where tainted_string: Tainted<String>
    // THEN: no type error when returning Clean<String>
    let errors =
        errors_for(r#"fn clean_up(input: Tainted<String>) -> Clean<String> { sanitize(input) }"#);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "sanitize(Tainted<String>) should yield Clean<String>, got: {errors:?}"
    );
}

#[test]
fn declassify_secret_returns_public() {
    // GIVEN: declassify(secret) where secret: Secret<Int>
    // THEN: no type error when returning Public<Int>
    let errors =
        errors_for(r#"fn expose(secret: Secret<Int>) -> Public<Int> { declassify(secret) }"#);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "declassify(Secret<Int>) should yield Public<Int>, got: {errors:?}"
    );
}

#[test]
fn sanitize_on_non_tainted_rejected() {
    // GIVEN: sanitize() applied to Public<String> (not Tainted)
    // THEN: InvalidSanitize error
    let errors =
        errors_for(r#"fn bad(input: Public<String>) -> Clean<String> { sanitize(input) }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidSanitize { .. })),
        "sanitize on non-Tainted type should emit InvalidSanitize, got: {errors:?}"
    );
}

#[test]
fn declassify_on_non_secret_rejected() {
    // GIVEN: declassify() applied to Tainted<Int> (not Secret)
    // THEN: InvalidDeclassify error
    let errors = errors_for(r#"fn bad(input: Tainted<Int>) -> Public<Int> { declassify(input) }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidDeclassify { .. })),
        "declassify on non-Secret type should emit InvalidDeclassify, got: {errors:?}"
    );
}

#[test]
fn direct_tainted_to_clean_without_sanitize_rejected() {
    // GIVEN: assigning Tainted<String> directly to Clean<String> param
    // THEN: TypeMismatch (must use sanitize explicitly)
    let errors = errors_for(
        r#"
        fn needs_clean(s: Clean<String>) -> Clean<String> { s }
        fn caller(raw: Tainted<String>) -> Clean<String> { needs_clean(raw) }
    "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Tainted should not flow to Clean<String> param, got: {errors:?}"
    );
}
