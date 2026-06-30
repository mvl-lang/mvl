// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! TIR walker parity tests — mirrors `emit_closures.rs`.
//!
//! Each test re-uses the same `compile(...)` input as the corresponding
//! AST substring test in `emitter_tests/closures.rs`, asserting that the
//! TIR walker emits byte-identical IR. When parity is achieved across the
//! corpus, PR 2 of #1612 flips the CLI to use the TIR path and deletes the
//! AST walker — at which point these tests become the primary coverage.

use super::common::assert_tir_parity;

#[test]
fn closure_type_emitted_once() {
    assert_tir_parity("fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2];\n\
         let _a: List[Int] = xs.filter(|x: Int| x > 0);\n\
         let _b: Bool = xs.any(|x: Int| x > 1);\n\
         }");
}

#[test]
fn non_capturing_lambda_emits_function_and_null_env() {
    assert_tir_parity("fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let _d: List[Int] = xs.filter(|x: Int| x > 0);\n\
         }");
}

#[test]
fn capturing_lambda_emits_env_struct_and_getelementptr() {
    assert_tir_parity("fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let threshold: Int = 2;\n\
         let _above: List[Int] = xs.filter(|x: Int| x > threshold);\n\
         }");
}

#[test]
fn hof_filter_with_lambda_emits_list_filter_call() {
    assert_tir_parity("fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let evens: List[Int] = xs.filter(|x: Int| x > 0);\n\
         }");
}

#[test]
fn hof_any_with_lambda_emits_i1_call() {
    assert_tir_parity("fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let b: Bool = xs.any(|x: Int| x > 0);\n\
         }");
}

#[test]
fn named_fn_closure_wraps_in_closure_struct() {
    assert_tir_parity("fn is_pos(x: Int) -> Bool { x > 0 }\n\
         fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let evens: List[Int] = xs.filter(is_pos);\n\
         }");
}

#[test]
fn hof_fold_emits_init_slot_and_list_fold_call() {
    assert_tir_parity("fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let sum: Int = xs.fold(0, |acc: Int, x: Int| acc + x);\n\
         }");
}

#[test]
fn capturing_lambda_with_two_captures() {
    assert_tir_parity("fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3, 4, 5];\n\
         let lo: Int = 1;\n\
         let hi: Int = 4;\n\
         let mid: List[Int] = xs.filter(|x: Int| x > lo);\n\
         }");
}

#[test]
fn ref_local_capture_loads_before_storing_into_env() {
    assert_tir_parity("fn run() -> Int {\n\
         let count: ref Int = 0;\n\
         count = count + 1;\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let above: List[Int] = xs.filter(|x: Int| x > count);\n\
         above.len()\n\
         }");
}
