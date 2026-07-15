//! Solver corpus regression tests: parse + type-check all 53 .mvl files in tests/solver/.
//!
//! Convention:
//!   - Files containing `// solver:expect-fail` MUST produce checker errors.
//!   - All other files MUST type-check cleanly (checker accepts them).
//!
//! Issue: #1230

use std::process::Command;

fn mvl_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_mvl"))
}

fn solver_file(layer: &str, name: &str) -> String {
    format!("{}/tests/solver/{layer}/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Returns true if the file contains the `// solver:expect-fail` annotation.
fn expects_failure(path: &str) -> bool {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .contains("solver:expect-fail")
}

/// Run `mvl check` on a solver corpus file.
///
/// - If the file has `// solver:expect-fail`, assert that check returns non-zero.
/// - Otherwise, assert that check returns zero (no errors).
fn assert_solver_file(layer: &str, name: &str) {
    let path = solver_file(layer, name);
    let out = Command::new(mvl_bin())
        .args(["check", &path])
        .output()
        .unwrap_or_else(|e| panic!("failed to run mvl check on {path}: {e}"));

    if expects_failure(&path) {
        assert!(
            !out.status.success(),
            "{layer}/{name}: expected checker errors (solver:expect-fail) but check succeeded.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    } else {
        assert!(
            out.status.success(),
            "{layer}/{name}: expected clean check but got errors.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
}

/// Generate a test function for each solver corpus file.
macro_rules! solver_test {
    ($test_name:ident, $layer:expr, $file:expr) => {
        #[test]
        fn $test_name() {
            assert_solver_file($layer, $file);
        }
    };
}

// ── Layer 1: Trivial (literal evaluation) ────────────────────────────────────

solver_test!(
    solver_l1_01_literals_proven,
    "layer1",
    "01_literals_proven.mvl"
);
solver_test!(
    solver_l1_02_literals_violation,
    "layer1",
    "02_literals_violation.mvl"
);
solver_test!(solver_l1_03_subsumption, "layer1", "03_subsumption.mvl");
solver_test!(solver_l1_04_tautology, "layer1", "04_tautology.mvl");
solver_test!(solver_l1_05_contradiction, "layer1", "05_contradiction.mvl");
solver_test!(
    solver_l1_06_constant_folding,
    "layer1",
    "06_constant_folding.mvl"
);
solver_test!(
    solver_l1_07_equality_hypothesis,
    "layer1",
    "07_equality_hypothesis.mvl"
);
solver_test!(
    solver_l1_08_branch_condition_match,
    "layer1",
    "08_branch_condition_match.mvl"
);
solver_test!(
    solver_l1_09_deep_constant_folding,
    "layer1",
    "09_deep_constant_folding.mvl"
);
solver_test!(
    solver_l1_10_violations_equality,
    "layer1",
    "10_violations_equality.mvl"
);
solver_test!(
    solver_l1_11_ensures_result_field,
    "layer1",
    "11_ensures_result_field.mvl"
);
solver_test!(
    solver_l1_12_ensures_result_field_violation,
    "layer1",
    "12_ensures_result_field_violation.mvl"
);
solver_test!(
    solver_l1_13_let_unfold_ensures,
    "layer1",
    "13_let_unfold_ensures.mvl"
);
solver_test!(
    solver_l1_14_construct_if_field_lift,
    "layer1",
    "14_construct_if_field_lift.mvl"
);
solver_test!(
    solver_l1_15_const_inlining,
    "layer1",
    "15_const_inlining.mvl"
);

// ── Layer 2: Interval analysis ───────────────────────────────────────────────

solver_test!(
    solver_l2_01_variable_bounds,
    "layer2",
    "01_variable_bounds.mvl"
);
solver_test!(solver_l2_02_if_narrowing, "layer2", "02_if_narrowing.mvl");
solver_test!(
    solver_l2_03_compound_bounds,
    "layer2",
    "03_compound_bounds.mvl"
);
solver_test!(solver_l2_04_violations, "layer2", "04_violations.mvl");
solver_test!(
    solver_l2_05_type_alias_bounds,
    "layer2",
    "05_type_alias_bounds.mvl"
);
solver_test!(
    solver_l2_06_multiple_params_interval,
    "layer2",
    "06_multiple_params_interval.mvl"
);
solver_test!(
    solver_l2_07_strict_to_nonstrict,
    "layer2",
    "07_strict_to_nonstrict.mvl"
);
solver_test!(
    solver_l2_08_param_plus_if_narrowing,
    "layer2",
    "08_param_plus_if_narrowing.mvl"
);
solver_test!(
    solver_l2_09_chained_calls_interval,
    "layer2",
    "09_chained_calls_interval.mvl"
);
solver_test!(
    solver_l2_10_violations_interval,
    "layer2",
    "10_violations_interval.mvl"
);
solver_test!(
    solver_l2_11_field_access_atoms,
    "layer2",
    "11_field_access_atoms.mvl"
);
solver_test!(solver_l2_12_len_axiom, "layer2", "12_len_axiom.mvl");

// ── Layer 3: Symbolic (path-sensitive) ───────────────────────────────────────

solver_test!(solver_l3_01_clamp, "layer3", "01_clamp.mvl");
solver_test!(solver_l3_02_min_max, "layer3", "02_min_max.mvl");
solver_test!(solver_l3_03_violations, "layer3", "03_violations.mvl");
solver_test!(solver_l3_04_no_else, "layer3", "04_no_else.mvl");
solver_test!(
    solver_l3_05_constant_all_paths,
    "layer3",
    "05_constant_all_paths.mvl"
);
solver_test!(
    solver_l3_06_bounded_select,
    "layer3",
    "06_bounded_select.mvl"
);
solver_test!(
    solver_l3_07_three_path_literal,
    "layer3",
    "07_three_path_literal.mvl"
);
solver_test!(solver_l3_08_early_returns, "layer3", "08_early_returns.mvl");
solver_test!(
    solver_l3_09_non_negative_clamp,
    "layer3",
    "09_non_negative_clamp.mvl"
);
solver_test!(
    solver_l3_10_violations_one_path,
    "layer3",
    "10_violations_one_path.mvl"
);

// ── Layer 4: Cooper (linear integer arithmetic) ──────────────────────────────

solver_test!(
    solver_l4_01_linear_expr_arg,
    "layer4",
    "01_linear_expr_arg.mvl"
);
solver_test!(solver_l4_02_divisibility, "layer4", "02_divisibility.mvl");
solver_test!(
    solver_l4_03_not_proven_runtime,
    "layer4",
    "03_not_proven_runtime.mvl"
);
solver_test!(
    solver_l4_04_sum_of_positives,
    "layer4",
    "04_sum_of_positives.mvl"
);
solver_test!(
    solver_l4_05_ordered_difference,
    "layer4",
    "05_ordered_difference.mvl"
);
solver_test!(
    solver_l4_06_negative_coefficient,
    "layer4",
    "06_negative_coefficient.mvl"
);
solver_test!(
    solver_l4_07_linear_combination,
    "layer4",
    "07_linear_combination.mvl"
);
solver_test!(
    solver_l4_08_divisibility_variants,
    "layer4",
    "08_divisibility_variants.mvl"
);
solver_test!(
    solver_l4_09_three_variable_fm,
    "layer4",
    "09_three_variable_fm.mvl"
);
solver_test!(
    solver_l4_10_runtime_for_nonlinear,
    "layer4",
    "10_runtime_for_nonlinear.mvl"
);

// ── Layer 5: Z3 / SMT ───────────────────────────────────────────────────────

solver_test!(
    solver_l5_01_chained_hypotheses,
    "layer5",
    "01_chained_hypotheses.mvl"
);
solver_test!(
    solver_l5_02_multi_hypothesis,
    "layer5",
    "02_multi_hypothesis.mvl"
);
solver_test!(
    solver_l5_03_ge_and_gt_chain,
    "layer5",
    "03_ge_and_gt_chain.mvl"
);
solver_test!(
    solver_l5_04_four_variable_chain,
    "layer5",
    "04_four_variable_chain.mvl"
);
solver_test!(
    solver_l5_05_asymmetric_bounds,
    "layer5",
    "05_asymmetric_bounds.mvl"
);
solver_test!(
    solver_l5_06_sandwich_bounds,
    "layer5",
    "06_sandwich_bounds.mvl"
);
solver_test!(
    solver_l5_07_three_step_bound,
    "layer5",
    "07_three_step_bound.mvl"
);
solver_test!(
    solver_l5_08_cross_param_bounds,
    "layer5",
    "08_cross_param_bounds.mvl"
);
solver_test!(
    solver_l5_09_violations_literal,
    "layer5",
    "09_violations_literal.mvl"
);
solver_test!(
    solver_l5_10_nonlinear_runtime,
    "layer5",
    "10_nonlinear_runtime.mvl"
);

// ── Cross-layer: fallthrough tests ───────────────────────────────────────────

solver_test!(
    solver_cross_01_l1_falls_to_l2,
    "cross_layer",
    "01_l1_falls_to_l2.mvl"
);
solver_test!(
    solver_cross_02_l2_falls_to_l3,
    "cross_layer",
    "02_l2_falls_to_l3.mvl"
);
solver_test!(
    solver_cross_03_all_fall_to_runtime,
    "cross_layer",
    "03_all_fall_to_runtime.mvl"
);
