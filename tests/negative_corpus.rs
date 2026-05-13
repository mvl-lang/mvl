//! Negative corpus tests — programs that MUST be rejected by `mvl check`.
//!
//! Each test runs `mvl check` on a `.mvl` file from `tests/corpus/negative/`
//! and asserts the exit code is non-zero.  Files are annotated with:
//!   `// corpus:expect-fail`   — signals this is an intentional negative fixture
//!   `// expect-requirement: N` — the ADR-0001 requirement being violated
//!
//! See tests/COVERAGE.md for the gap analysis that motivated these tests.
//! Related: #677 (audit), #680 (this suite).

use std::process::Command;

fn mvl_bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().expect("current_exe");
    p.pop(); // test binary name
    p.pop(); // deps/
    p.push("mvl");
    p
}

fn negative(rel_path: &str) -> String {
    format!(
        "{}/tests/corpus/negative/{rel_path}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Run `mvl check` on a negative fixture and assert it FAILS (non-zero exit).
/// Returns stderr so callers can inspect error messages if needed.
fn assert_check_fails(rel_path: &str) -> String {
    let path = negative(rel_path);
    let out = Command::new(mvl_bin())
        .args(["check", &path])
        .output()
        .unwrap_or_else(|e| panic!("failed to run mvl check on {rel_path}: {e}"));

    assert!(
        !out.status.success(),
        "{rel_path}: expected check to FAIL (non-zero exit) but it succeeded.\n\
         This means the checker is not catching the intended violation.\n\
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    String::from_utf8_lossy(&out.stderr).into_owned()
}

// ── Req 1: Type Safety ───────────────────────────────────────────────────────

/// Assigning a String return value where Int is expected → TypeMismatch.
#[test]
fn req01_type_mismatch_rejected() {
    let stderr = assert_check_fails("req01/type_mismatch.mvl");
    assert!(
        stderr.contains("req1") || stderr.contains("req01"),
        "req01/type_mismatch.mvl: expected error[req1] in stderr:\n{stderr}"
    );
}

/// Calling `add(x, y, z)` when `add` takes two args → WrongArgCount.
#[test]
fn req01_wrong_arg_count_rejected() {
    let stderr = assert_check_fails("req01/wrong_arg_count.mvl");
    assert!(
        stderr.contains("req1"),
        "req01/wrong_arg_count.mvl: expected error[req1] in stderr:\n{stderr}"
    );
}

// ── Req 2: Memory Safety ─────────────────────────────────────────────────────

/// Reading a String after it was consume()-d → UseAfterMove.
#[test]
fn req02_use_after_move_rejected() {
    let stderr = assert_check_fails("req02/use_after_move.mvl");
    assert!(
        stderr.contains("req2"),
        "req02/use_after_move.mvl: expected error[req2] in stderr:\n{stderr}"
    );
}

/// consume()-ing an iso value twice (double-free) → UseAfterMove.
#[test]
fn req02_double_consume_rejected() {
    let stderr = assert_check_fails("req02/double_consume.mvl");
    assert!(
        stderr.contains("req2"),
        "req02/double_consume.mvl: expected error[req2] in stderr:\n{stderr}"
    );
}

// ── Req 3: Exhaustive Match ───────────────────────────────────────────────────

/// Three-variant enum with only two arms → NonExhaustiveMatch.
#[test]
fn req03_missing_arm_rejected() {
    let stderr = assert_check_fails("req03/missing_arm.mvl");
    assert!(
        stderr.contains("req3"),
        "req03/missing_arm.mvl: expected error[req3] in stderr:\n{stderr}"
    );
}

/// Four-variant enum with three arms → NonExhaustiveMatch.
#[test]
fn req03_added_variant_rejected() {
    let stderr = assert_check_fails("req03/added_variant.mvl");
    assert!(
        stderr.contains("req3"),
        "req03/added_variant.mvl: expected error[req3] in stderr:\n{stderr}"
    );
}

// ── Req 4: Null Elimination ───────────────────────────────────────────────────

/// Field access directly on Option[User] → OptionDirectAccess.
#[test]
fn req04_option_field_access_rejected() {
    let stderr = assert_check_fails("req04/option_field_access.mvl");
    assert!(
        stderr.contains("req4"),
        "req04/option_field_access.mvl: expected error[req4] in stderr:\n{stderr}"
    );
}

/// Arithmetic on Option[Int] without extracting → OptionDirectAccess.
#[test]
fn req04_option_unwrap_none_rejected() {
    let stderr = assert_check_fails("req04/option_unwrap_none.mvl");
    assert!(
        stderr.contains("req4"),
        "req04/option_unwrap_none.mvl: expected error[req4] in stderr:\n{stderr}"
    );
}

// ── Req 5: Error Visibility ───────────────────────────────────────────────────

/// Result[Int, String] return silently discarded → ResultIgnored.
#[test]
fn req05_result_ignored_rejected() {
    let stderr = assert_check_fails("req05/result_ignored.mvl");
    assert!(
        stderr.contains("req5"),
        "req05/result_ignored.mvl: expected error[req5] in stderr:\n{stderr}"
    );
}

/// ? operator applied to a non-Result Int → PropagateNotResult.
#[test]
fn req05_propagate_non_result_rejected() {
    let stderr = assert_check_fails("req05/propagate_non_result.mvl");
    assert!(
        stderr.contains("req5"),
        "req05/propagate_non_result.mvl: expected error[req5] in stderr:\n{stderr}"
    );
}

// ── Req 6: Ownership / Immutability ──────────────────────────────────────────

/// Reassigning an immutable `let` binding → AssignToImmutable.
#[test]
fn req06_reassign_immutable_rejected() {
    let stderr = assert_check_fails("req06/reassign_immutable.mvl");
    assert!(
        stderr.contains("req6"),
        "req06/reassign_immutable.mvl: expected error[req6] in stderr:\n{stderr}"
    );
}

/// Mutating a field on a non-mut struct → MutateImmutableField.
#[test]
fn req06_mutate_immutable_field_rejected() {
    let stderr = assert_check_fails("req06/mutate_immutable_field.mvl");
    assert!(
        stderr.contains("req6"),
        "req06/mutate_immutable_field.mvl: expected error[req6] in stderr:\n{stderr}"
    );
}

// ── Req 7: Effect Tracking ────────────────────────────────────────────────────

/// Function calls println without `! Console` in its signature → UndeclaredEffect.
#[test]
fn req07_undeclared_effect_rejected() {
    let stderr = assert_check_fails("req07/undeclared_effect.mvl");
    assert!(
        stderr.contains("req7"),
        "req07/undeclared_effect.mvl: expected error[req7] in stderr:\n{stderr}"
    );
}

/// Caller declares only `! Log` but calls fn requiring `! Console` → MissingEffect.
#[test]
fn req07_missing_propagation_rejected() {
    let stderr = assert_check_fails("req07/missing_propagation.mvl");
    assert!(
        stderr.contains("req7"),
        "req07/missing_propagation.mvl: expected error[req7] in stderr:\n{stderr}"
    );
}

// ── Req 8: Termination ────────────────────────────────────────────────────────

/// total fn contains a while loop → UnboundedLoopInTotal.
#[test]
fn req08_while_in_total_rejected() {
    let stderr = assert_check_fails("req08/while_in_total.mvl");
    assert!(
        stderr.contains("req8"),
        "req08/while_in_total.mvl: expected error[req8] in stderr:\n{stderr}"
    );
}

/// total fn calls a partial fn → PartialCallInTotal.
#[test]
fn req08_partial_call_in_total_rejected() {
    let stderr = assert_check_fails("req08/partial_call_in_total.mvl");
    assert!(
        stderr.contains("req8"),
        "req08/partial_call_in_total.mvl: expected error[req8] in stderr:\n{stderr}"
    );
}

// ── Req 9: Data Race Freedom ──────────────────────────────────────────────────

/// ref-capability value passed where iso is required → CapabilityViolation.
#[test]
fn req09_send_ref_across_actor_rejected() {
    let stderr = assert_check_fails("req09/send_ref_across_actor.mvl");
    assert!(
        stderr.contains("req9"),
        "req09/send_ref_across_actor.mvl: expected error[req9] in stderr:\n{stderr}"
    );
}

/// iso value aliased without consume() → IsoAliasingViolation.
#[test]
fn req09_iso_aliased_rejected() {
    let stderr = assert_check_fails("req09/iso_aliased.mvl");
    assert!(
        stderr.contains("req9"),
        "req09/iso_aliased.mvl: expected error[req9] in stderr:\n{stderr}"
    );
}

// ── Req 10: Refinement Types ─────────────────────────────────────────────────

/// Literal 0 passed to parameter with `where denominator != 0` → RefinementViolated.
#[test]
fn req10_division_by_zero_rejected() {
    let stderr = assert_check_fails("req10/division_by_zero.mvl");
    assert!(
        stderr.contains("req10"),
        "req10/division_by_zero.mvl: expected error[req10] in stderr:\n{stderr}"
    );
}

/// Unrefined Int passed to fn with `requires n >= 0` → PreconditionViolated.
#[test]
fn req10_precondition_violated_rejected() {
    let stderr = assert_check_fails("req10/precondition_violated.mvl");
    assert!(
        stderr.contains("req10"),
        "req10/precondition_violated.mvl: expected error[req10] in stderr:\n{stderr}"
    );
}
