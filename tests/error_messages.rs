//! Error message quality tests.
//!
//! Each test compiles an intentionally broken MVL fixture and asserts that:
//!   1. `mvl check` exits with a non-zero status (compilation fails).
//!   2. stderr contains the requirement tag `error[req{N}]`.
//!   3. stderr contains a key phrase from the expected human-readable message.
//!
//! Tests do NOT assert exact message text — only key fragments — so minor
//! wording improvements do not break the suite. The fragments are chosen to
//! be distinctive enough to confirm the *right* error was emitted.
//!
//! Fixture files live in `tests/integration/error_messages/`.

use std::process::Command;

fn mvl_bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().expect("current_exe");
    p.pop(); // test binary
    p.pop(); // deps/
    p.push("mvl");
    p
}

fn fixture(name: &str) -> String {
    format!(
        "{}/tests/integration/error_messages/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Run `mvl check` on a fixture file and return the stderr output.
/// Asserts the check *fails* (non-zero exit code).
fn check_fails(fixture_name: &str) -> String {
    let out = Command::new(mvl_bin())
        .args(["check", &fixture(fixture_name)])
        .output()
        .unwrap_or_else(|e| panic!("failed to run mvl check on {fixture_name}: {e}"));

    assert!(
        !out.status.success(),
        "{fixture_name}: expected check to fail but it succeeded"
    );

    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Assert that `stderr` contains all of the given key fragments.
fn assert_contains(stderr: &str, fixture_name: &str, fragments: &[&str]) {
    for fragment in fragments {
        assert!(
            stderr.contains(fragment),
            "{fixture_name}: expected fragment {fragment:?} in stderr:\n{stderr}"
        );
    }
}

// ── Req 1: Type Safety ────────────────────────────────────────────────────────

#[test]
fn req1_type_mismatch_reports_expected_and_found_types() {
    let stderr = check_fails("req1_type_mismatch.mvl");
    assert_contains(
        &stderr,
        "req1_type_mismatch.mvl",
        &["error[req1]", "type mismatch"],
    );
}

#[test]
fn req1_undefined_variable_names_the_variable() {
    let stderr = check_fails("req1_undefined_variable.mvl");
    assert_contains(
        &stderr,
        "req1_undefined_variable.mvl",
        &["error[req1]", "undefined variable", "`z`"],
    );
}

#[test]
fn req1_undefined_function_names_the_function() {
    let stderr = check_fails("req1_undefined_function.mvl");
    assert_contains(
        &stderr,
        "req1_undefined_function.mvl",
        &["error[req1]", "undefined function", "`nonexistent`"],
    );
}

#[test]
fn req1_wrong_arg_count_states_expected_and_actual() {
    let stderr = check_fails("req1_wrong_arg_count.mvl");
    assert_contains(
        &stderr,
        "req1_wrong_arg_count.mvl",
        &["error[req1]", "`add`", "2", "3"],
    );
}

#[test]
fn req1_unknown_field_names_the_field_and_type() {
    let stderr = check_fails("req1_unknown_field.mvl");
    assert_contains(
        &stderr,
        "req1_unknown_field.mvl",
        &["error[req1]", "unknown field", "`z`"],
    );
}

#[test]
fn req1_missing_field_names_the_field_and_type() {
    let stderr = check_fails("req1_missing_field.mvl");
    assert_contains(
        &stderr,
        "req1_missing_field.mvl",
        &["error[req1]", "missing field", "`y`"],
    );
}

#[test]
fn req1_undefined_type_names_the_type() {
    let stderr = check_fails("req1_undefined_type.mvl");
    assert_contains(
        &stderr,
        "req1_undefined_type.mvl",
        &["error[req1]", "undefined type", "`Nonexistent`"],
    );
}

// ── Req 2: Memory Safety ──────────────────────────────────────────────────────

#[test]
fn req2_use_after_move_names_the_variable() {
    let stderr = check_fails("req2_use_after_move.mvl");
    assert_contains(
        &stderr,
        "req2_use_after_move.mvl",
        &["error[req2]", "use of moved value", "`s`"],
    );
}

#[test]
fn req2_reference_escapes_scope_names_the_variable() {
    let stderr = check_fails("req2_reference_escapes_scope.mvl");
    assert_contains(
        &stderr,
        "req2_reference_escapes_scope.mvl",
        &["error[req2]", "escapes"],
    );
}

// ── Req 3: Exhaustive Match ───────────────────────────────────────────────────

#[test]
fn req3_non_exhaustive_match_lists_missing_variants() {
    let stderr = check_fails("req3_non_exhaustive_match.mvl");
    assert_contains(
        &stderr,
        "req3_non_exhaustive_match.mvl",
        &["error[req3]", "non-exhaustive", "match"],
    );
}

#[test]
fn req3_guard_non_exhaustive_missing_variant() {
    let stderr = check_fails("req3_guard_non_exhaustive.mvl");
    assert_contains(
        &stderr,
        "req3_guard_non_exhaustive.mvl",
        &["error[req3]", "non-exhaustive", "Blue"],
    );
}

// ── Req 4: Null Elimination ───────────────────────────────────────────────────

#[test]
fn req4_option_direct_access_suggests_match_or_question_mark() {
    let stderr = check_fails("req4_option_direct_access.mvl");
    assert_contains(
        &stderr,
        "req4_option_direct_access.mvl",
        &["error[req4]", "Option", "match"],
    );
}

// ── Req 5: Error Visibility ───────────────────────────────────────────────────

#[test]
fn req5_result_ignored_tells_user_to_handle_or_propagate() {
    let stderr = check_fails("req5_result_ignored.mvl");
    assert_contains(
        &stderr,
        "req5_result_ignored.mvl",
        &["error[req5]", "Result", "match"],
    );
}

#[test]
fn req5_propagate_not_result_names_the_type() {
    let stderr = check_fails("req5_propagate_not_result.mvl");
    assert_contains(
        &stderr,
        "req5_propagate_not_result.mvl",
        &["error[req5]", "`?`", "`Int`"],
    );
}

// ── Req 6: Ownership / Immutability ──────────────────────────────────────────

#[test]
fn req6_assign_to_immutable_names_the_binding() {
    let stderr = check_fails("req6_assign_to_immutable.mvl");
    assert_contains(
        &stderr,
        "req6_assign_to_immutable.mvl",
        &["error[req6]", "immutable", "`x`"],
    );
}

#[test]
fn req6_capture_mutability_names_the_binding() {
    let stderr = check_fails("req6_capture_mutability.mvl");
    assert_contains(
        &stderr,
        "req6_capture_mutability.mvl",
        &["error[req6]", "mutable", "`counter`"],
    );
}

// ── Req 7: Effect Tracking ────────────────────────────────────────────────────

#[test]
fn req7_undeclared_effect_names_the_callee_and_effect() {
    let stderr = check_fails("req7_undeclared_effect.mvl");
    assert_contains(
        &stderr,
        "req7_undeclared_effect.mvl",
        &["error[req7]", "effect"],
    );
}

#[test]
fn req7_missing_effect_names_caller_callee_and_effect() {
    let stderr = check_fails("req7_missing_effect.mvl");
    assert_contains(
        &stderr,
        "req7_missing_effect.mvl",
        &["error[req7]", "effect"],
    );
}

#[test]
fn req7_invalid_effect_name_names_the_bad_effect() {
    let stderr = check_fails("req7_invalid_effect_name.mvl");
    assert_contains(
        &stderr,
        "req7_invalid_effect_name.mvl",
        &["error[req7]", "Teleport"],
    );
}

// ── Req 8: Termination ────────────────────────────────────────────────────────

#[test]
fn req8_unbounded_loop_in_total_suggests_partial() {
    let stderr = check_fails("req8_unbounded_loop_in_total.mvl");
    assert_contains(
        &stderr,
        "req8_unbounded_loop_in_total.mvl",
        &["error[req8]", "total"],
    );
}

#[test]
fn req8_partial_call_in_total_names_the_callee() {
    let stderr = check_fails("req8_partial_call_in_total.mvl");
    assert_contains(
        &stderr,
        "req8_partial_call_in_total.mvl",
        &["error[req8]", "`do_work`"],
    );
}

#[test]
fn req8_unproven_recursion_names_the_function() {
    let stderr = check_fails("req8_unproven_recursion.mvl");
    assert_contains(
        &stderr,
        "req8_unproven_recursion.mvl",
        &["error[req8]", "`loop_forever`"],
    );
}

// ── Req 9: Data Race Freedom ──────────────────────────────────────────────────

#[test]
fn req9_iso_aliasing_names_the_variable_and_suggests_consume() {
    let stderr = check_fails("req9_iso_aliasing.mvl");
    assert_contains(
        &stderr,
        "req9_iso_aliasing.mvl",
        &["error[req9]", "iso", "`data`"],
    );
}

// ── Req 10: Refinement Types ──────────────────────────────────────────────────

#[test]
fn req10_refinement_violated_states_the_predicate() {
    let stderr = check_fails("req10_refinement_violated.mvl");
    assert_contains(
        &stderr,
        "req10_refinement_violated.mvl",
        &["error[req10]", "refinement"],
    );
}

// ── Req 11: Information Flow Control ─────────────────────────────────────────

#[test]
fn req11_logging_label_violation_names_the_label() {
    let stderr = check_fails("req11_logging_label_violation.mvl");
    assert_contains(
        &stderr,
        "req11_logging_label_violation.mvl",
        &["error[req11]", "Secret"],
    );
}

#[test]
fn req11_implicit_flow_violation_names_the_sink() {
    let stderr = check_fails("req11_implicit_flow_violation.mvl");
    assert_contains(
        &stderr,
        "req11_implicit_flow_violation.mvl",
        &["error[req11]", "implicit"],
    );
}

#[test]
fn req11_invalid_declassify_names_the_actual_type() {
    let stderr = check_fails("req11_invalid_declassify.mvl");
    assert_contains(
        &stderr,
        "req11_invalid_declassify.mvl",
        &["error[req11]", "declassify", "`Int`"],
    );
}

// ── Refinement solver CLI flags ───────────────────────────────────────────────

fn corpus(name: &str) -> String {
    format!("{}/tests/corpus/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Run `mvl check` with extra args on a file; assert it *succeeds* (exit 0).
/// Returns stderr so callers can assert on diagnostic output.
fn check_ok_with_args(path: &str, extra_args: &[&str]) -> String {
    let mut cmd = Command::new(mvl_bin());
    cmd.arg("check").arg(path);
    for arg in extra_args {
        cmd.arg(arg);
    }
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to run mvl check: {e}"));
    assert!(
        out.status.success(),
        "expected check to succeed but it failed.\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Run `mvl check` with extra args on a file; assert it *fails* (non-zero exit).
/// Returns stderr.
fn check_fails_with_args(path: &str, extra_args: &[&str]) -> String {
    let mut cmd = Command::new(mvl_bin());
    cmd.arg("check").arg(path);
    for arg in extra_args {
        cmd.arg(arg);
    }
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to run mvl check: {e}"));
    assert!(
        !out.status.success(),
        "expected check to fail but it succeeded"
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn refinement_solver_bogus_mode_exits_with_error() {
    let stderr = check_fails_with_args(
        &corpus("07_refinements/refinements_fully_proven.mvl"),
        &["--refinement-solver=bogus"],
    );
    assert_contains(
        &stderr,
        "--refinement-solver=bogus",
        &["unknown refinement-solver", "bogus"],
    );
}

#[test]
fn refinement_stats_prints_header_and_layer_for_proven_file() {
    let stderr = check_ok_with_args(
        &corpus("07_refinements/refinements_fully_proven.mvl"),
        &["--refinement-stats"],
    );
    assert_contains(
        &stderr,
        "--refinement-stats proven",
        &[
            "refinement stats (solver: layered)",
            "proven:",
            "L1:trivial",
        ],
    );
}

#[test]
fn refinement_stats_respects_solver_mode_label() {
    let stderr = check_ok_with_args(
        &corpus("07_refinements/refinements_fully_proven.mvl"),
        &["--refinement-stats", "--refinement-solver=fast-only"],
    );
    assert_contains(
        &stderr,
        "--refinement-stats fast-only",
        &["refinement stats (solver: fast-only)"],
    );
}

#[test]
fn refinement_stats_no_layer_lines_when_zero_proven() {
    // A file with no refinement call sites should print stats but no layer lines.
    let stderr = check_ok_with_args(
        &corpus("01_basics/expressions.mvl"),
        &["--refinement-stats"],
    );
    assert_contains(
        &stderr,
        "--refinement-stats zero-proven",
        &["refinement stats (solver: layered)", "proven:"],
    );
    assert!(
        !stderr.contains("L1:trivial"),
        "expected no layer line when proven=0, got:\n{stderr}"
    );
}
