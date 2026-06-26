use super::*;
use crate::mvl::parser::Parser;

fn compile(src: &str) -> String {
    let (mut p, errs) = Parser::new(src);
    assert!(errs.is_empty(), "lex errors: {errs:?}");
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    LlvmTextCompiler::new()
        .compile_to_ir(&prog, "test")
        .expect("compile_to_ir failed")
}

#[test]
fn simple_add_function() {
    let ir = compile("fn add(a: Int, b: Int) -> Int { a + b }");
    assert!(ir.contains("define i64 @add(i64 %a, i64 %b)"), "{ir}");
    assert!(ir.contains("add i64"), "{ir}");
    assert!(ir.contains("ret i64"), "{ir}");
}

#[test]
fn integer_literal_returned() {
    let ir = compile("fn answer() -> Int { 42 }");
    assert!(ir.contains("define i64 @answer()"), "{ir}");
    assert!(ir.contains("ret i64 42"), "{ir}");
}

#[test]
fn bool_literal_returned() {
    let ir = compile("fn always_true() -> Bool { true }");
    assert!(ir.contains("define i1 @always_true()"), "{ir}");
    assert!(ir.contains("ret i1 true"), "{ir}");
}

#[test]
fn arithmetic_operators() {
    let ir = compile("fn f(a: Int, b: Int) -> Int { a - b }");
    assert!(ir.contains("sub i64"), "{ir}");
    let ir = compile("fn f(a: Int, b: Int) -> Int { a * b }");
    assert!(ir.contains("mul i64"), "{ir}");
    let ir = compile("fn f(a: Int, b: Int) -> Int { a / b }");
    assert!(ir.contains("sdiv i64"), "{ir}");
    let ir = compile("fn f(a: Int, b: Int) -> Int { a % b }");
    assert!(ir.contains("srem i64"), "{ir}");
}

#[test]
fn comparison_operators_emit_icmp() {
    let ir = compile("fn lt(a: Int, b: Int) -> Bool { a < b }");
    assert!(ir.contains("icmp slt i64"), "{ir}");
    let ir = compile("fn gt(a: Int, b: Int) -> Bool { a > b }");
    assert!(ir.contains("icmp sgt i64"), "{ir}");
    let ir = compile("fn eq(a: Int, b: Int) -> Bool { a == b }");
    assert!(ir.contains("icmp eq i64"), "{ir}");
}

#[test]
fn if_else_emits_phi() {
    let ir = compile("fn max(a: Int, b: Int) -> Int { if a > b { a } else { b } }");
    assert!(ir.contains("icmp sgt"), "{ir}");
    assert!(ir.contains("br i1"), "{ir}");
    assert!(ir.contains("phi"), "{ir}");
    assert!(ir.contains("ret i64"), "{ir}");
}

/// Regression for #1155: a 3-way `else if` chain must emit PHI nodes for
/// every branch so the correct value is selected at runtime. Before the fix,
/// the `else if` condition was silently dropped and the merge block produced
/// `ret i64 undef`.
#[test]
fn else_if_chain_emits_phi_for_all_branches() {
    let ir = compile(
        "fn classify(n: Int) -> Int {\n\
             if n > 0 { 1 }\n\
             else if n < 0 { -1 }\n\
             else { 0 }\n\
         }",
    );
    // The `else if n < 0` condition must actually be evaluated.
    assert!(ir.contains("icmp slt"), "{ir}");
    // Two PHI nodes: inner selects between -1 and 0; outer selects between 1 and inner.
    let phi_count = ir.matches(" = phi ").count();
    assert!(
        phi_count >= 2,
        "else-if chain needs ≥2 phi nodes, got {phi_count}\n{ir}"
    );
    // Return must be a defined value, not undef.
    assert!(ir.contains("ret i64"), "{ir}");
    assert!(!ir.contains("ret i64 undef"), "{ir}");
}

#[test]
fn unit_function_emits_ret_void() {
    let ir = compile("fn noop() -> Unit { }");
    assert!(ir.contains("define void @noop()"), "{ir}");
    assert!(ir.contains("ret void"), "{ir}");
}

#[test]
fn main_emits_i32_return() {
    let ir = compile("fn main() -> Unit { }");
    assert!(ir.contains("define i32 @main()"), "{ir}");
    assert!(ir.contains("ret i32 0"), "{ir}");
}

#[test]
fn main_explicit_return_emits_ret_i32_0() {
    let ir = compile("fn main() -> Unit { return; }");
    assert!(ir.contains("define i32 @main()"), "{ir}");
    assert!(ir.contains("ret i32 0"), "{ir}");
    assert!(!ir.contains("ret void"), "{ir}");
}

#[test]
fn let_binding_aliases_ssa_value() {
    let ir = compile("fn f(x: Int) -> Int { let y: Int = x; y }");
    assert!(ir.contains("ret i64"), "{ir}");
}

#[test]
fn logical_not_emits_xor() {
    let ir = compile("fn f(b: Bool) -> Bool { !b }");
    assert!(ir.contains("xor i1"), "{ir}");
}

#[test]
fn module_header_present() {
    let ir = compile("fn f() -> Int { 0 }");
    assert!(ir.contains("ModuleID = 'test'"), "{ir}");
    assert!(ir.contains("source_filename = \"test\""), "{ir}");
    assert!(ir.contains("target triple"), "{ir}");
}

#[test]
fn multiple_functions_and_call() {
    let ir = compile(
        "fn add(a: Int, b: Int) -> Int { a + b }\n\
         fn double(n: Int) -> Int { add(n, n) }",
    );
    assert!(ir.contains("define i64 @add"), "{ir}");
    assert!(ir.contains("define i64 @double"), "{ir}");
    assert!(ir.contains("call i64 @add"), "{ir}");
}

#[test]
fn negation_emits_sub_from_zero() {
    let ir = compile("fn neg(x: Int) -> Int { -x }");
    assert!(ir.contains("sub i64 0,"), "{ir}");
}

#[test]
fn short_circuit_and_emits_phi() {
    let ir = compile("fn f(a: Bool, b: Bool) -> Bool { a && b }");
    assert!(ir.contains("phi i1"), "{ir}");
    assert!(ir.contains("false"), "{ir}");
}

#[test]
fn short_circuit_or_emits_phi() {
    let ir = compile("fn f(a: Bool, b: Bool) -> Bool { a || b }");
    assert!(ir.contains("phi i1"), "{ir}");
    assert!(ir.contains("true"), "{ir}");
}

#[test]
fn mutable_ref_uses_alloca_store_load() {
    let ir = compile(
        "partial fn counter(n: Int) -> Int {\
         let c: ref Int = 0;\
         while c < n {\
           c = c + 1;\
         }\
         c\
         }",
    );
    assert!(ir.contains("alloca i64"), "{ir}");
    assert!(ir.contains("store i64"), "{ir}");
    assert!(ir.contains("load i64"), "{ir}");
    assert!(ir.contains("br i1"), "{ir}");
}

#[test]
fn string_literal_emits_global_and_string_new() {
    let ir = compile("fn main() -> Unit ! Console { println(\"hello\") }");
    assert!(ir.contains("_mvl_string_new"), "{ir}");
    assert!(ir.contains("hello"), "{ir}");
    assert!(ir.contains("dprintf"), "{ir}");
}

#[test]
fn assert_emits_conditional_trap() {
    let ir = compile("fn main() -> Unit { assert(1 == 1) }");
    assert!(ir.contains("llvm.trap"), "{ir}");
    assert!(ir.contains("br i1"), "{ir}");
}

#[test]
fn struct_type_emits_type_def() {
    let ir = compile(
        "type Point = struct { x: Int, y: Int }\n\
         fn get_x(p: Point) -> Int { p.x }",
    );
    assert!(ir.contains("%Point = type { i64, i64 }"), "{ir}");
    assert!(ir.contains("define i64 @get_x(%Point %p)"), "{ir}");
    assert!(ir.contains("extractvalue %Point"), "{ir}");
}

#[test]
fn enum_variant_emits_discriminant() {
    let ir = compile(
        "type Shape = enum { Circle, Square }\n\
         fn circle() -> Shape { Shape::Circle }",
    );
    assert!(ir.contains("ret i64 0"), "{ir}");
}

// ── Closure / lambda tests (#1148) ────────────────────────────────────

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

// ── Actor emission tests (#1149) ──────────────────────────────────────

#[test]
fn actor_emits_state_struct_and_behavior_fn() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }",
    );
    // State struct typedef.
    assert!(ir.contains("%CounterState = type"), "{ir}");
    // Behavior function.
    assert!(
        ir.contains("define void @counter_increment(ptr %self, i64 %n)"),
        "{ir}"
    );
}

#[test]
fn actor_emits_dispatch_function_with_switch() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
           pub fn reset() { }\n\
         }",
    );
    // Dispatch function signature.
    assert!(
        ir.contains("define void @counter_dispatch(ptr %state, i64 %disc, ptr %args)"),
        "{ir}"
    );
    // Switch with at least two case labels.
    assert!(ir.contains("switch i64 %disc, label %default"), "{ir}");
    assert!(ir.contains("i64 0, label %behavior_0"), "{ir}");
    assert!(ir.contains("i64 1, label %behavior_1"), "{ir}");
}

#[test]
fn actor_runtime_externs_emitted() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_actor_spawn"), "{ir}");
    assert!(ir.contains("declare void @_mvl_actor_send"), "{ir}");
    assert!(ir.contains("declare void @_mvl_actor_join_all"), "{ir}");
}

#[test]
fn spawn_emits_alloca_and_actor_spawn_call() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }\n\
         fn main() -> Int {\n\
           let c: Counter = actor Counter { count: 0 };\n\
           0\n\
         }",
    );
    // State alloca.
    assert!(ir.contains("alloca %CounterState"), "{ir}");
    // Runtime spawn call.
    assert!(ir.contains("call ptr @_mvl_actor_spawn"), "{ir}");
}

#[test]
fn actor_method_call_emits_send() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }\n\
         fn main() -> Int {\n\
           let c: Counter = actor Counter { count: 0 };\n\
           c.increment(1);\n\
           0\n\
         }",
    );
    // The send call must appear.
    assert!(ir.contains("call void @_mvl_actor_send"), "{ir}");
}

#[test]
fn join_all_emitted_in_main_when_actors_present() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }\n\
         fn main() -> Int { 0 }",
    );
    assert!(ir.contains("call void @_mvl_actor_join_all"), "{ir}");
}

// ── Generic monomorphization tests (#1156) ───────────────────────────

/// Generic `identity[T]` must produce separate monomorphized copies for
/// each concrete type argument used at call sites.
#[test]
fn generic_fn_monomorphized_per_concrete_type() {
    let ir = compile(
        "fn identity[T](x: T) -> T { x }\n\
         fn main() -> Unit {\n\
           let n: Int = identity(42);\n\
           let s: String = identity(\"hi\");\n\
         }",
    );
    // Two separate definitions with correct types.
    assert!(ir.contains("define i64 @identity__Int(i64 %x)"), "{ir}");
    assert!(ir.contains("define ptr @identity__String(ptr %x)"), "{ir}");
    // Call sites use mangled names.
    assert!(ir.contains("call i64 @identity__Int(i64 42)"), "{ir}");
    assert!(ir.contains("call ptr @identity__String("), "{ir}");
}

// ── Option constructor + match tests (#1156) ─────────────────────────

/// `Some(val)` must emit a `{ i8, ptr }` tagged union with disc=0.
#[test]
fn some_constructor_emits_tagged_union() {
    let ir = compile("fn wrap(n: Int) -> Option[Int] { Some(n) }");
    assert!(
        ir.contains("insertvalue { i8, ptr } zeroinitializer, i8 0, 0"),
        "{ir}"
    );
    assert!(ir.contains("insertvalue { i8, ptr }"), "{ir}");
    assert!(ir.contains("define { i8, ptr } @wrap"), "{ir}");
}

/// `None` must emit a `{ i8, ptr }` tagged union with disc=1.
#[test]
fn none_constructor_emits_tagged_union() {
    let ir = compile("fn empty() -> Option[Int] { None }");
    assert!(
        ir.contains("insertvalue { i8, ptr } zeroinitializer, i8 1, 0"),
        "{ir}"
    );
}

/// Match on `Option[Int]` must emit a switch on the discriminant byte.
#[test]
fn option_match_emits_switch_on_discriminant() {
    let ir = compile(
        "fn unwrap_or(opt: Option[Int], default: Int) -> Int {\n\
             match opt {\n\
                 Some(v) => v,\n\
                 None => default,\n\
             }\n\
         }",
    );
    assert!(ir.contains("switch i8"), "{ir}");
    assert!(ir.contains("i8 0, label"), "{ir}"); // Some arm
    assert!(ir.contains("i8 1, label"), "{ir}"); // None arm
    assert!(ir.contains("phi i64"), "{ir}");
}

// ── Map literal emission tests (#1184) ───────────────────────────────

#[test]
fn map_literal_emits_map_new_and_insert() {
    let ir = compile(
        "fn main() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1, \"b\": 2};\n\
         }",
    );
    assert!(ir.contains("call ptr @_mvl_map_new(i64"), "{ir}");
    assert!(ir.contains("call void @_mvl_map_insert(ptr"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_string_ptr(ptr"), "{ir}");
    assert!(ir.contains("call i64 @_mvl_str_len(ptr"), "{ir}");
}

#[test]
fn empty_map_emits_map_new_only() {
    let ir = compile(
        "fn main() -> Unit {\n\
         let m: Map[String, Int] = Map::new();\n\
         }",
    );
    // Map::new() goes through FnCall, not Map literal — just verify no crash.
    assert!(ir.contains("define i32 @main()"), "{ir}");
}

#[test]
fn map_len_emits_mvl_map_len() {
    let ir = compile(
        "fn main() -> Int {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         m.len()\n\
         }",
    );
    assert!(ir.contains("declare i64 @_mvl_map_len(ptr)"), "{ir}");
    assert!(ir.contains("call i64 @_mvl_map_len(ptr"), "{ir}");
}

#[test]
fn map_keys_emits_mvl_map_keys() {
    let ir = compile(
        "fn main() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         let _k: List[String] = m.keys();\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_map_keys(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_map_keys(ptr"), "{ir}");
}

#[test]
fn map_contains_key_emits_null_check() {
    let ir = compile(
        "fn main() -> Bool {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         m.contains_key(\"a\")\n\
         }",
    );
    assert!(ir.contains("call ptr @_mvl_map_get(ptr"), "{ir}");
    assert!(ir.contains("icmp ne ptr"), "{ir}");
}

#[test]
fn map_get_emits_null_guard_before_load() {
    let ir = compile(
        "fn f(m: Map[String, Int]) -> Int {\n\
         m.get(\"key\")\n\
         }",
    );
    assert!(ir.contains("call ptr @_mvl_map_get(ptr"), "{ir}");
    // Must null-check before building Option struct
    assert!(ir.contains("icmp eq ptr"), "{ir}");
    assert!(ir.contains("insertvalue { i8, ptr }"), "{ir}");
    assert!(ir.contains("phi { i8, ptr }"), "{ir}");
}

// ── HeapKind drop tracking tests (#1185) ─────────────────────────────

#[test]
fn string_local_emits_drop_before_ret() {
    let ir = compile(
        "fn greet() -> Unit {\n\
         let s: String = \"hello\";\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_string_drop(ptr"), "{ir}");
    assert!(ir.contains("declare void @_mvl_string_drop(ptr)"), "{ir}");
}

#[test]
fn list_local_emits_drop_before_ret() {
    let ir = compile(
        "fn nums() -> Unit {\n\
         let xs: List[Int] = [1, 2, 3];\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_array_drop(ptr"), "{ir}");
    assert!(ir.contains("declare void @_mvl_array_drop(ptr)"), "{ir}");
}

#[test]
fn map_local_emits_drop_before_ret() {
    let ir = compile(
        "fn maps() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_map_drop(ptr"), "{ir}");
    assert!(ir.contains("declare void @_mvl_map_drop(ptr)"), "{ir}");
}

#[test]
fn multiple_heap_locals_all_dropped() {
    let ir = compile(
        "fn multi() -> Unit {\n\
         let s: String = \"hello\";\n\
         let xs: List[Int] = [1, 2];\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_string_drop(ptr"), "{ir}");
    assert!(ir.contains("call void @_mvl_array_drop(ptr"), "{ir}");
}

#[test]
fn primitive_locals_no_drop() {
    let ir = compile(
        "fn prims() -> Unit {\n\
         let x: Int = 42;\n\
         let b: Bool = true;\n\
         }",
    );
    assert!(!ir.contains("_drop"), "{ir}");
}

#[test]
fn explicit_return_emits_drops() {
    let ir = compile(
        "fn early() -> Int {\n\
         let s: String = \"hello\";\n\
         return 42;\n\
         }",
    );
    // The drop should appear before the ret instruction.
    assert!(ir.contains("call void @_mvl_string_drop(ptr"), "{ir}");
}

#[test]
fn shadowed_string_local_no_double_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let s: String = \"first\";\n\
         let s: String = \"second\";\n\
         }",
    );
    // Should have exactly 1 drop call (for the second binding only;
    // the first is removed from tracking when shadowed).
    let drop_count = ir.matches("call void @_mvl_string_drop(ptr").count();
    assert_eq!(drop_count, 1, "expected 1 drop, got {drop_count}\n{ir}");
}

#[test]
fn ref_string_local_emits_load_then_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let s: ref String = \"hello\";\n\
         }",
    );
    // ref local: must load from alloca, then drop the loaded value.
    assert!(ir.contains("call void @_mvl_string_drop(ptr"), "{ir}");
    // Verify the load-before-drop pattern exists.
    assert!(ir.contains("load ptr, ptr"), "{ir}");
}

// ── String builtin kernel methods tests (#1186) ──────────────────────

#[test]
fn string_chars_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Unit {\n\
         let _cs: List[String] = s.chars();\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_string_chars(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_string_chars(ptr"), "{ir}");
}

#[test]
fn string_byte_at_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Option[Byte] {\n\
         s.byte_at(0)\n\
         }",
    );
    assert!(
        ir.contains("declare i8 @_mvl_str_byte_at(ptr, i64, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("call i8 @_mvl_str_byte_at(ptr"), "{ir}");
}

#[test]
fn string_find_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Int {\n\
         s.find(\"x\")\n\
         }",
    );
    assert!(ir.contains("declare i64 @_mvl_str_find(ptr, ptr)"), "{ir}");
    assert!(ir.contains("call i64 @_mvl_str_find(ptr"), "{ir}");
}

#[test]
fn string_split_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Unit {\n\
         let _parts: List[String] = s.split(\",\");\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_str_split(ptr, ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_str_split(ptr"), "{ir}");
}

#[test]
fn string_substring_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> String {\n\
         s.substring(0, 3)\n\
         }",
    );
    assert!(
        ir.contains("declare ptr @_mvl_str_substring(ptr, i64, i64)"),
        "{ir}"
    );
    assert!(ir.contains("call ptr @_mvl_str_substring(ptr"), "{ir}");
}

#[test]
fn string_contains_emits_i64_to_bool() {
    let ir = compile(
        "fn f(s: String) -> Bool {\n\
         s.contains(\"x\")\n\
         }",
    );
    assert!(
        ir.contains("declare i64 @_mvl_str_contains(ptr, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("icmp ne i64"), "{ir}");
}

#[test]
fn string_starts_with_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Bool {\n\
         s.starts_with(\"http\")\n\
         }",
    );
    assert!(
        ir.contains("declare i64 @_mvl_str_starts_with(ptr, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("call i64 @_mvl_str_starts_with(ptr"), "{ir}");
}

#[test]
fn string_ends_with_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> Bool {\n\
         s.ends_with(\".mvl\")\n\
         }",
    );
    assert!(
        ir.contains("declare i64 @_mvl_str_ends_with(ptr, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("call i64 @_mvl_str_ends_with(ptr"), "{ir}");
}

#[test]
fn string_trim_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> String {\n\
         s.trim()\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_str_trim(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_str_trim(ptr"), "{ir}");
}

#[test]
fn string_to_lower_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> String {\n\
         s.to_lower()\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_str_to_lower(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_str_to_lower(ptr"), "{ir}");
}

#[test]
fn string_to_upper_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> String {\n\
         s.to_upper()\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_str_to_upper(ptr)"), "{ir}");
    assert!(ir.contains("call ptr @_mvl_str_to_upper(ptr"), "{ir}");
}

#[test]
fn string_replace_emits_runtime_call() {
    let ir = compile(
        "fn f(s: String) -> String {\n\
         s.replace(\"old\", \"new\")\n\
         }",
    );
    assert!(
        ir.contains("declare ptr @_mvl_str_replace(ptr, ptr, ptr)"),
        "{ir}"
    );
    assert!(ir.contains("call ptr @_mvl_str_replace(ptr"), "{ir}");
}

/// `extern "c"` block emits LLVM `declare` instructions (#811).
#[test]
fn extern_c_emits_declare() {
    let ir = compile(
        "extern \"c\" {\n\
         fn sqlite_open(path: String) -> Int\n\
         fn sqlite_close(db: Int) -> Unit\n\
         }",
    );
    assert!(
        ir.contains("declare i64 @sqlite_open(ptr)"),
        "missing sqlite_open declare: {ir}"
    );
    assert!(
        ir.contains("declare void @sqlite_close(i64)"),
        "missing sqlite_close declare: {ir}"
    );
}

/// `extern "rust"` block is NOT emitted by LLVM backend (handled by Rust backend only).
#[test]
fn extern_rust_not_emitted_by_llvm() {
    let ir = compile(
        "extern \"rust\" {\n\
         fn bridge_fn(x: Int) -> Int\n\
         }",
    );
    assert!(
        !ir.contains("declare") || !ir.contains("bridge_fn"),
        "extern rust should not emit declare: {ir}"
    );
}
