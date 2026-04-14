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
fn recursion_inside_lambda_not_flagged() {
    // GIVEN: total fn creates a lambda that references the outer fn by name
    // THEN: no UnprovenRecursion — lambdas have their own scope (spec 007 §Req 4)
    let src = r#"
        fn outer(n: Int) -> Int {
            let f = |x| outer(x);
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

// ── #138: Data race freedom — iso aliasing (Requirement 9, Phase 3) ──────────

#[test]
fn iso_aliasing_without_consume_rejected() {
    // GIVEN: fn binds an `iso` param to a new let without consume()
    // THEN: IsoAliasingViolation reported (two live references to isolated object)
    let src = r#"
        fn alias_iso(channel: Channel, iso x: Payload) -> Unit {
            let y = x;
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
            let copy = config;
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
            let mut y = consume(x);
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
                let y = x;
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
fn iso_rebound_after_consume_not_detected_l5() {
    // L5: After `let y = consume(x)`, `y` becomes the new iso owner but is NOT
    // added to iso_vars.  Subsequent aliasing of `y` is therefore undetected.
    // This is a known Phase 3 limitation — full tracking requires mutable scope
    // analysis (Phase 6).
    let src = r#"
        fn rebound_alias(iso x: Payload) -> Unit {
            let a = consume(x);
            let b = a;
            consume(b)
        }
    "#;
    let errors = errors_for(src);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, CheckError::IsoAliasingViolation { name, .. } if name == "a")),
        "L5: aliasing of rebound iso (after consume) is not yet detected, got: {errors:?}"
    );
}

#[test]
fn iso_multiple_aliasing_all_sites_reported() {
    // Each individual let-binding of an iso param is flagged independently.
    // Both `let a = x` and `let b = x` generate separate violations.
    let src = r#"
        fn double_alias(iso x: Payload) -> Unit {
            let a = x;
            let b = x;
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

// ── #58/#66: Map/Set literals and multiline/raw strings ──────────────────────

#[test]
fn map_set_literals_corpus_parses_and_checks() {
    let src = include_str!("corpus/02_types/map_set_literals.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "map_set_literals corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn literals_corpus_with_multiline_raw_strings_checks() {
    let src = include_str!("corpus/01_basics/literals.mvl");
    let result = check_src(src);
    assert!(
        result.is_ok(),
        "literals corpus should type-check cleanly, got: {:?}",
        result.errors
    );
}

#[test]
fn map_literal_infers_named_map_type() {
    let errors = errors_for(r#"fn f() -> Unit { let _m = {"a": 1, "b": 2}; }"#);
    assert!(
        errors.is_empty(),
        "map literal should type-check cleanly, got: {errors:?}"
    );
}

#[test]
fn set_literal_infers_named_set_type() {
    let errors = errors_for(r#"fn f() -> Unit { let _s = {1, 2, 3}; }"#);
    assert!(
        errors.is_empty(),
        "set literal should type-check cleanly, got: {errors:?}"
    );
}

// ── 003-information-flow/Req 6: Logging label enforcement ────────────────────

/// `println` with a Secret argument MUST be rejected (003-information-flow/Req 6).
#[test]
fn println_rejects_secret_argument() {
    let errors = errors_for(r#"fn f(pwd: Secret<String>) -> Unit ! Console { println(pwd); }"#);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::LoggingLabelViolation { label, .. } if label == "Secret")
        ),
        "println with Secret arg should emit LoggingLabelViolation, got: {errors:?}"
    );
}

/// `println` with a Tainted argument MUST be rejected (003-information-flow/Req 6).
#[test]
fn println_rejects_tainted_argument() {
    let errors =
        errors_for(r#"fn f(input: Tainted<String>) -> Unit ! Console { println(input); }"#);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::LoggingLabelViolation { label, .. } if label == "Tainted")
        ),
        "println with Tainted arg should emit LoggingLabelViolation, got: {errors:?}"
    );
}

/// `println` with a Public argument MUST be accepted (003-information-flow/Req 6).
#[test]
fn println_accepts_public_argument() {
    let errors = errors_for(r#"fn f(msg: Public<String>) -> Unit ! Console { println(msg); }"#);
    let label_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, CheckError::LoggingLabelViolation { .. }))
        .collect();
    assert!(
        label_errors.is_empty(),
        "println with Public arg should not emit LoggingLabelViolation, got: {label_errors:?}"
    );
}

/// `println` with a Clean argument MUST be rejected (003-information-flow/Req 6).
/// Clean<T> is sanitized but not declassified — an explicit declassify() is required
/// before logging.
#[test]
fn println_rejects_clean_argument() {
    let errors = errors_for(r#"fn f(s: Clean<String>) -> Unit ! Console { println(s); }"#);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::LoggingLabelViolation { label, .. } if label == "Clean")
        ),
        "println with Clean arg should emit LoggingLabelViolation, got: {errors:?}"
    );
}

/// `print` with a Secret argument MUST be rejected (003-information-flow/Req 6).
#[test]
fn print_rejects_secret_argument() {
    let errors = errors_for(r#"fn f(pwd: Secret<String>) -> Unit ! Console { print(pwd); }"#);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::LoggingLabelViolation { label, .. } if label == "Secret")
        ),
        "print with Secret arg should emit LoggingLabelViolation, got: {errors:?}"
    );
}

/// `print` with a Tainted argument MUST be rejected (003-information-flow/Req 6).
#[test]
fn print_rejects_tainted_argument() {
    let errors = errors_for(r#"fn f(input: Tainted<String>) -> Unit ! Console { print(input); }"#);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::LoggingLabelViolation { label, .. } if label == "Tainted")
        ),
        "print with Tainted arg should emit LoggingLabelViolation, got: {errors:?}"
    );
}

// ── 002-effect-system/Req 2: Effect name validation ──────────────────────────

/// Unknown effect name MUST be rejected (002-effect-system/Req 2).
#[test]
fn invalid_effect_name_rejected() {
    let errors = errors_for(r#"fn f() -> Unit ! IoMagic { }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidEffectName { name, .. } if name == "IoMagic")),
        "unknown effect name should emit InvalidEffectName, got: {errors:?}"
    );
}

/// All canonical effect names MUST be accepted (002-effect-system/Req 2).
#[test]
fn valid_effect_names_accepted() {
    // Test all 12 canonical effect names from VALID_EFFECT_NAMES in checker/mod.rs.
    let canonical = [
        "Console",
        "FileRead",
        "FileWrite",
        "FileDelete",
        "Net",
        "DB",
        "ProcessSpawn",
        "Random",
        "Clock",
        "Env",
        "Log",
        "Async",
    ];
    for name in &canonical {
        let src = format!("fn f() -> Unit ! {name} {{ }}");
        let result = check_src(&src);
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

/// The legacy `IO` catch-all bucket MUST be rejected (002-effect-system/Req 2).
#[test]
fn io_effect_bucket_rejected() {
    let errors = errors_for(r#"fn f() -> Unit ! IO { }"#);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CheckError::InvalidEffectName { name, .. } if name == "IO")),
        "`IO` should be rejected as a non-canonical effect bucket, got: {errors:?}"
    );
}

// ── ADR-0002: Lambda capture immutability ────────────────────────────────────

/// Lambda capturing a mutable binding MUST be rejected (ADR-0002).
/// Ignored until lambda syntax is added to the hand-written recursive-descent parser.
#[test]
#[ignore = "lambda parsing not yet implemented in the hand-written parser (Phase 2)"]
fn lambda_mutable_capture_rejected() {
    let errors =
        errors_for(r#"fn f() -> Unit { let mut x = 1; let _g = |y: Int| -> Int { x + y }; }"#);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::CaptureMutabilityViolation { name, .. } if name == "x")
        ),
        "lambda capturing mut x should emit CaptureMutabilityViolation, got: {errors:?}"
    );
}

/// Lambda capturing an immutable binding MUST be accepted (ADR-0002).
/// Ignored until lambda syntax is added to the hand-written recursive-descent parser.
#[test]
#[ignore = "lambda parsing not yet implemented in the hand-written parser (Phase 2)"]
fn lambda_immutable_capture_accepted() {
    let result = check_src(r#"fn f() -> Unit { let x = 1; let _g = |y: Int| -> Int { x + y }; }"#);
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

// ── IFC Phase 3: implicit flow detection (003-information-flow/Req 11) ────────

/// `println` inside a branch controlled by a `Secret` condition MUST be rejected.
///
/// Even though the argument to `println` is a literal (Public), the fact that
/// the print fires at all reveals whether `flag` was truthy — an implicit flow.
///
/// - GIVEN `fn f(flag: Secret<Bool>) -> Unit`
/// - WHEN `if flag { println("branch taken") }`
/// - THEN `ImplicitFlowViolation` with pc_label="Secret" is emitted
#[test]
fn implicit_flow_secret_if_condition_rejected() {
    let errors = errors_for(
        r#"fn f(flag: Secret<Bool>) -> Unit ! Console { if flag { println("branch taken"); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, sink, .. }
                if pc_label == "Secret" && sink == "println")
        ),
        "println inside Secret branch should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// `println` inside a branch controlled by a `Tainted` condition MUST be rejected.
///
/// - GIVEN `fn f(cond: Tainted<Bool>) -> Unit`
/// - WHEN `if cond { println("ok") }`
/// - THEN `ImplicitFlowViolation` with pc_label="Tainted" is emitted
#[test]
fn implicit_flow_tainted_if_condition_rejected() {
    let errors =
        errors_for(r#"fn f(cond: Tainted<Bool>) -> Unit ! Console { if cond { println("ok"); } }"#);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, sink, .. }
                if pc_label == "Tainted" && sink == "println")
        ),
        "println inside Tainted branch should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// `println` inside a branch controlled by a `Public` condition MUST be accepted.
///
/// No implicit flow: the condition has no security label, so the branch is safe.
///
/// - GIVEN `fn f(x: Public<Bool>) -> Unit`
/// - WHEN `if x { println("ok") }`
/// - THEN no `ImplicitFlowViolation`
#[test]
fn implicit_flow_public_condition_accepted() {
    let errors =
        errors_for(r#"fn f(x: Public<Bool>) -> Unit ! Console { if x { println("ok"); } }"#);
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
/// - GIVEN `fn g(s: Secret<Bool>) -> Unit`
/// - WHEN `if s { print("x"); }`
/// - THEN `ImplicitFlowViolation` with sink="print" is emitted
#[test]
fn implicit_flow_print_sink_rejected() {
    let errors = errors_for(r#"fn g(s: Secret<Bool>) -> Unit ! Console { if s { print("x"); } }"#);
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, sink, .. }
                if pc_label == "Secret" && sink == "print")
        ),
        "print inside Secret branch should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// `println` in the else-branch of a `Secret`-controlled if MUST also be rejected.
///
/// Both branches are controlled by the condition; the else branch also leaks
/// information (its firing reveals the condition was false).
///
/// - GIVEN `fn h(flag: Secret<Bool>) -> Unit`
/// - WHEN `if flag { 0; } else { println("not taken"); }`
/// - THEN `ImplicitFlowViolation` is emitted for the else-branch println
#[test]
fn implicit_flow_else_branch_rejected() {
    let errors = errors_for(
        r#"fn h(flag: Secret<Bool>) -> Unit ! Console { if flag { 0; } else { println("not taken"); } }"#,
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
/// - GIVEN `fn f(raw: Secret<Int>) -> Unit`
/// - WHEN `let x: Secret<Int> = raw; if x { println("y"); }`
/// - THEN `ImplicitFlowViolation` is emitted (label propagated through let binding)
#[test]
fn implicit_flow_label_propagated_through_let() {
    let errors = errors_for(
        r#"fn f(raw: Secret<Int>) -> Unit ! Console { let x: Secret<Int> = raw; if x { println("y"); } }"#,
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
/// - GIVEN `fn poll(flag: Secret<Bool>) -> Unit ! Console`
/// - WHEN `while flag { println("still waiting"); }`
/// - THEN `ImplicitFlowViolation` with pc_label="Secret" and sink="println" is emitted
#[test]
fn implicit_flow_while_secret_condition_rejected() {
    let errors = errors_for(
        r#"fn poll(flag: Secret<Bool>) -> Unit ! Console { while flag { println("still waiting"); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, sink, .. }
                if pc_label == "Secret" && sink == "println")
        ),
        "println inside Secret while-loop should emit ImplicitFlowViolation, got: {errors:?}"
    );
}

/// `implicit_flow_else_branch_rejected` additionally verifies the sink field.
///
/// The else-branch println leaks information about the Secret condition
/// (its firing proves the condition was false).
///
/// - GIVEN `fn h(flag: Secret<Bool>) -> Unit ! Console`
/// - WHEN `if flag { 0; } else { println("not taken"); }`
/// - THEN `ImplicitFlowViolation` with pc_label="Secret" and sink="println"
#[test]
fn implicit_flow_else_branch_sink_verified() {
    let errors = errors_for(
        r#"fn h(flag: Secret<Bool>) -> Unit ! Console { if flag { 0; } else { println("not taken"); } }"#,
    );
    assert!(
        errors.iter().any(
            |e| matches!(e, CheckError::ImplicitFlowViolation { pc_label, sink, .. }
                if pc_label == "Secret" && sink == "println")
        ),
        "println in else of Secret branch should emit ImplicitFlowViolation with sink=println, got: {errors:?}"
    );
}

/// Implicit flow corpus: load and verify the implicit_flow.mvl corpus file.
///
/// The corpus contains only INVALID programs that should each produce
/// `ImplicitFlowViolation` errors. This test validates the corpus itself.
#[test]
fn implicit_flow_corpus_has_violations() {
    let src = include_str!("corpus/05_ifc/implicit_flow.mvl");
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
    let src = include_str!("corpus/06_refinements/refinements_valid.mvl");
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
    let src = include_str!("corpus/06_refinements/refinements_violations.mvl");
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
        total fn caller(mut y: Int where y > 0) -> Int {
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
