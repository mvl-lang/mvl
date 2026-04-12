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

#[test]
fn sanitize_on_clean_rejected() {
    // GIVEN: sanitize() applied to Clean<String> (not Tainted)
    // THEN: InvalidSanitize error
    let errors = errors_for(r#"fn bad(input: Clean<String>) -> Clean<String> { sanitize(input) }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidSanitize { .. })),
        "sanitize on Clean type should emit InvalidSanitize, got: {errors:?}"
    );
}

#[test]
fn sanitize_on_secret_rejected() {
    // GIVEN: sanitize() applied to Secret<String> (not Tainted)
    // THEN: InvalidSanitize error (use declassify for Secret)
    let errors =
        errors_for(r#"fn bad(input: Secret<String>) -> Clean<String> { sanitize(input) }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidSanitize { .. })),
        "sanitize on Secret type should emit InvalidSanitize, got: {errors:?}"
    );
}

#[test]
fn secret_to_unlabeled_param_rejected() {
    // GIVEN: function with unlabeled String param called with Secret<String>
    // THEN: TypeMismatch — unlabeled context is treated as Public, downward flow rejected
    let errors = errors_for(
        r#"
        fn sink(s: String) -> String { s }
        fn caller(k: Secret<String>) -> String { sink(k) }
    "#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "Secret<String> must not flow silently to unlabeled String param, got: {errors:?}"
    );
}

#[test]
fn unlabeled_to_secret_param_accepted() {
    // GIVEN: function with Secret<String> param called with unlabeled String
    // THEN: accepted — unlabeled data is treated as Public, upward flow to Secret is fine
    let errors = errors_for(
        r#"
        fn vault(s: Secret<String>) -> Secret<String> { s }
        fn caller(name: String) -> Secret<String> { vault(name) }
    "#,
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "unlabeled String should flow up to Secret<String> param, got: {errors:?}"
    );
}

#[test]
fn if_with_labeled_bool_condition_promotes_result() {
    // GIVEN: if-condition is Secret<Bool>, branch results are Public<Int>
    // THEN: result type is Secret<Int> — cannot be returned as Public<Int>
    let errors = errors_for(
        r#"fn select(flag: Secret<Bool>, a: Public<Int>, b: Public<Int>) -> Public<Int> {
            if flag { a } else { b }
        }"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
        "if Secret<Bool> must promote result to Secret<Int>, rejecting Public<Int> return, got: {errors:?}"
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
fn sanitize_before_validation_guard_accepted() {
    // GIVEN: sanitize() called after an explicit guard check (correct ordering)
    // THEN: no type error — sanitize(Tainted<String>) → Clean<String> is valid
    let errors = errors_for(
        r#"fn validate(raw: Tainted<String>) -> Result<Clean<String>, String> {
    if raw.len() < 8 {
        return Err("too short".to_string());
    }
    Ok(sanitize(raw))
}"#,
    );
    assert!(
        errors.is_empty(),
        "sanitize after guard should be accepted, got: {errors:?}"
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
    parses_and_checks("total fn identity<T>(x: T) -> T { return x; }");
}

#[test]
fn generic_type_decl_parses() {
    // Req 9: generic type declaration parses and checks
    parses_and_checks("type Container<T> = struct { value: T }");
}

#[test]
fn generic_pair_type_parses() {
    // Req 9: multiple type parameters parse and check
    parses_and_checks("type Pair<A, B> = struct { first: A, second: B }");
}

#[test]
fn generic_with_constraint_parses() {
    // Req 9: where-clause constraint parses and checks
    parses_and_checks("total fn max<T>(a: T, b: T) -> T where T: Ord { return a; }");
}

#[test]
fn generic_multiple_constraints_parse() {
    // Req 9: multiple constraints in where clause parse and check
    parses_and_checks(
        "total fn show_max<T>(a: T, b: T) -> T where T: Ord, T: Display { return a; }",
    );
}

// ── Requirement 9: Generics — rejection scenarios (Phase 2 enforcement) ───
// These tests document the intended rejection semantics. They are marked
// #[ignore] until constraint enforcement is implemented in the checker.
// See: https://github.com/LAB271/mvl_language/issues/48

#[test]
#[ignore = "constraint enforcement not yet implemented (Phase 2)"]
fn missing_constraint_on_comparison_rejected() {
    // Req 9 Scenario: Missing constraint rejected
    // GIVEN unconstrained T used with `>` operator
    // THEN checker MUST reject with a missing-constraint error
    let (mut p, _) = Parser::new(
        "total fn max<T>(a: T, b: T) -> T { if a > b { return a; } else { return b; } }",
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
}

#[test]
#[ignore = "HKT diagnostic not yet implemented (Phase 2)"]
fn higher_kinded_type_param_rejected() {
    // Req 9 Scenario: No higher-kinded types
    // GIVEN F<_> nested angle-bracket type param
    // THEN parser MUST reject
    let (mut p, _) = Parser::new("type Functor<F<_>> = struct { val: Int }");
    let _ = p.parse_program();
    assert!(
        !p.errors().is_empty(),
        "HKT type parameter syntax must be rejected by the parser"
    );
}

#[test]
#[ignore = "inline constraint rejection not yet implemented (Phase 2)"]
fn inline_constraint_syntax_rejected() {
    // Req 9 Scenario: Inline constraint syntax rejected
    // GIVEN <T: Ord> inline constraint syntax
    // THEN parser MUST reject in Phase 1
    let (mut p, _) = Parser::new("total fn max<T: Ord>(a: T, b: T) -> T { return a; }");
    let _ = p.parse_program();
    assert!(
        !p.errors().is_empty(),
        "inline constraint `<T: Ord>` must be rejected in Phase 1"
    );
}

// ── From/Into conversion (#62) ────────────────────────────────────────────

/// `?` with identical error types requires no From impl.
#[test]
fn propagate_same_error_type_accepted() {
    let src = r#"
fn inner() -> Result<Int, String> { Ok(0) }
fn outer() -> Result<Int, String> {
    let x = inner()?;
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
fn inner() -> Result<Int, String> { Ok(0) }
fn outer() -> Result<Int, Bool> {
    let x = inner()?;
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
impl From<IoError> for AppError {
    fn from(e: IoError) -> Self { AppError::Io(e) }
}
fn load() -> Result<String, IoError> { Ok("data") }
fn run() -> Result<String, AppError> {
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
