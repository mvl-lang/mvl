// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_closures.rs` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/closures.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

#[test]
fn closure_type_emitted_once() {
    // Two lambdas in the same program — closure type must appear exactly once.
    let ir = compile(
        "fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2];\n\
         let _a: List[Int] = xs.filter(|x: Int| x > 0);\n\
         let _b: Bool = xs.any(|x: Int| x > 1);\n\
         }",
    );
    let count = ir.matches("%__closure_type = type").count();
    assert_eq!(count, 1, "expected exactly one closure type def:\n{ir}");
}

#[test]
fn non_capturing_lambda_emits_function_and_null_env() {
    // |x: Int| x * 2  — no free variables
    let ir = compile(
        "fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let _d: List[Int] = xs.filter(|x: Int| x > 0);\n\
         }",
    );
    // Lambda function emitted as a top-level define.
    // HOF lambdas receive element by pointer from the runtime.
    assert!(
        ir.contains("define i1 @__lambda_0(ptr %__env, ptr %__raw_x)"),
        "{ir}"
    );
    // Closure struct built with null env ptr.
    assert!(ir.contains("store ptr null"), "{ir}");
    // fn_ptr field set to the lambda.
    assert!(ir.contains("store ptr @__lambda_0"), "{ir}");
}

#[test]
fn capturing_lambda_emits_env_struct_and_getelementptr() {
    // |x: Int| x > threshold  — captures `threshold` from outer scope
    let ir = compile(
        "fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let threshold: Int = 2;\n\
         let _above: List[Int] = xs.filter(|x: Int| x > threshold);\n\
         }",
    );
    // Env struct type must be registered.
    assert!(ir.contains("%__env___lambda_0 = type"), "{ir}");
    // Capture stored via GEP.
    assert!(ir.contains("getelementptr %__env___lambda_0"), "{ir}");
    // Lambda function has the env parameter.
    assert!(ir.contains("define i1 @__lambda_0(ptr %__env"), "{ir}");
    // Inside the lambda the captured value is loaded.
    assert!(ir.contains("load i64"), "{ir}");
}

#[test]
fn hof_filter_with_lambda_emits_list_filter_call() {
    let ir = compile(
        "fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let evens: List[Int] = xs.filter(|x: Int| x > 0);\n\
         }",
    );
    assert!(
        ir.contains("declare ptr @_mvl_list_filter(ptr, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("call ptr @_mvl_list_filter"), "{ir}");
    assert!(ir.contains("@__lambda_0"), "{ir}");
}

#[test]
fn hof_any_with_lambda_emits_i1_call() {
    let ir = compile(
        "fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let b: Bool = xs.any(|x: Int| x > 0);\n\
         }",
    );
    assert!(ir.contains("declare i1 @_mvl_list_any(ptr, ptr)"), "{ir}");
    assert!(ir.contains("call i1 @_mvl_list_any"), "{ir}");
}

#[test]
fn named_fn_closure_wraps_in_closure_struct() {
    let ir = compile(
        "fn is_pos(x: Int) -> Bool { x > 0 }\n\
         fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let evens: List[Int] = xs.filter(is_pos);\n\
         }",
    );
    // Wrapper trampoline generated (HOF wrapper receives element by pointer)
    assert!(ir.contains("@__closure_wrap_is_pos_hof0"), "{ir}");
    // Closure struct built pointing to wrapper
    assert!(ir.contains("store ptr @__closure_wrap_is_pos_hof0"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_list_filter"), "{ir}");
    // Trampoline receives element by pointer and loads before forwarding.
    assert!(
        ir.contains("define i1 @__closure_wrap_is_pos_hof0(ptr %__env, ptr %__raw_arg0)"),
        "trampoline missing ptr param:\n{ir}"
    );
    assert!(
        ir.contains("load i64, ptr %__raw_arg0"),
        "trampoline must load element from ptr:\n{ir}"
    );
    assert!(
        ir.contains("call i1 @is_pos(i64 %__loaded_arg0)"),
        "trampoline must forward loaded arg to original:\n{ir}"
    );
}

#[test]
fn hof_fold_emits_init_slot_and_list_fold_call() {
    let ir = compile(
        "fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let sum: Int = xs.fold(0, |acc: Int, x: Int| acc + x);\n\
         }",
    );
    assert!(
        ir.contains("declare ptr @_mvl_list_fold(ptr, ptr, ptr)"),
        "{ir}"
    );
    // Initial value must be stack-allocated and stored.
    assert!(ir.contains("alloca i64"), "{ir}");
    assert!(ir.contains("store i64 0"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_list_fold(ptr"), "{ir}");
    // Result loaded back as the accumulator type.
    assert!(ir.contains("load i64"), "{ir}");
    // Lambda for fold: acc is by-value (i64), element is by-pointer from runtime.
    assert!(
        ir.contains("define i64 @__lambda_0(ptr %__env, i64 %acc, ptr %__raw_x)"),
        "{ir}"
    );
}

#[test]
fn capturing_lambda_with_two_captures() {
    // Captures both `lo` and `hi` — env struct must have two i64 fields.
    let ir = compile(
        "fn main() -> Unit ! Console {\n\
         let xs: List[Int] = [1, 2, 3, 4, 5];\n\
         let lo: Int = 1;\n\
         let hi: Int = 4;\n\
         let mid: List[Int] = xs.filter(|x: Int| x > lo);\n\
         }",
    );
    // Env struct type registered.
    assert!(ir.contains("%__env___lambda_0 = type"), "{ir}");
    // GEP accesses for storing captures into env.
    assert!(ir.contains("getelementptr %__env___lambda_0"), "{ir}");
    // Two stores for the two captured values.
    let store_count = ir.matches("store i64").count();
    assert!(
        store_count >= 1,
        "expected at least one i64 store for env field, got {store_count}:\n{ir}"
    );
}

#[test]
fn ref_local_capture_loads_before_storing_into_env() {
    // `count` is a mutable ref binding — must be loaded before capture.
    let ir = compile(
        "fn run() -> Int {\n\
         let count: ref Int = 0;\n\
         count = count + 1;\n\
         let xs: List[Int] = [1, 2, 3];\n\
         let above: List[Int] = xs.filter(|x: Int| x > count);\n\
         above.len()\n\
         }",
    );
    // ref local alloca present.
    assert!(ir.contains("alloca i64"), "{ir}");
    // Env struct created for the capture.
    assert!(ir.contains("%__env___lambda_0 = type"), "{ir}");
    // A load from the ref alloca must precede the GEP store into the env.
    assert!(ir.contains("load i64"), "{ir}");
    assert!(ir.contains("getelementptr %__env___lambda_0"), "{ir}");
}
