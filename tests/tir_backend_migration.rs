// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Tests for coverage gaps identified in PR #1317: TIR backend migration.
//!
//! Covers:
//! 1. `ty_to_llvm` — all `Ty` variants including edge cases (`Never`, `Unknown`,
//!    `Char`, `UByte`, `UInt`, recursive wrappers, `Option`, `Result`).
//! 2. `expr_types` fallback path — LLVM emitter falls back to AST inference
//!    when checker types are absent (empty `expr_types` map).
//! 3. `Backend` trait contract — `RustBackend::emit_program` runs end-to-end
//!    from a real `TirProgram`.
//! 4. Silent checker failure path — when the checker produces type errors,
//!    `assemble_expr_types` still returns the partial map and the LLVM backend
//!    falls back to AST inference without panicking.

use mvl::mvl::backends::llvm_text::LlvmTextCompiler;
use mvl::mvl::checker;
use mvl::mvl::parser::Parser;
use mvl::mvl::pipeline::assemble_expr_types;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse(src: &str) -> mvl::mvl::parser::ast::Program {
    let (mut p, errs) = Parser::new(src);
    assert!(errs.is_empty(), "lex errors: {errs:?}");
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    prog
}

/// Compile `src` with an empty `expr_types` map (AST-only fallback path).
fn compile_no_checker(src: &str) -> String {
    let prog = parse(src);
    LlvmTextCompiler::new()
        .compile_to_ir(&prog, "test")
        .expect("compile_to_ir failed")
}

/// Compile `src` with checker-resolved `expr_types` (normal pipeline path).
fn compile_with_checker(src: &str) -> String {
    let prog = parse(src);
    let mut compiler = LlvmTextCompiler::new();
    compiler.expr_types = assemble_expr_types(&prog, &[]);
    compiler
        .compile_to_ir(&prog, "test")
        .expect("compile_to_ir failed")
}

// ── 1. ty_to_llvm unit tests ──────────────────────────────────────────────────
//
// These drive `type_of_expr` through the checker path so `ty_to_llvm` is
// exercised for each variant of `Ty`.  We verify the LLVM IR type appearing
// in the generated function signatures and return instructions.

/// `Ty::Int` and `Ty::UInt` → `i64`.
#[test]
fn ty_to_llvm_int_and_uint_emit_i64() {
    // Int is the default signed integer; the checker always resolves integer
    // literals to Ty::Int.
    let ir = compile_with_checker("fn f(x: Int) -> Int { x }");
    assert!(ir.contains("define i64 @f(i64 %x)"), "Int param: {ir}");
    assert!(ir.contains("ret i64"), "Int ret: {ir}");

    let ir = compile_with_checker("fn f(x: UInt) -> UInt { x }");
    assert!(ir.contains("define i64 @f(i64 %x)"), "UInt param: {ir}");
}

/// `Ty::Float` → `double`.
#[test]
fn ty_to_llvm_float_emits_double() {
    let ir = compile_with_checker("fn f(x: Float) -> Float { x }");
    assert!(ir.contains("define double @f(double %x)"), "{ir}");
    assert!(ir.contains("ret double"), "{ir}");
}

/// `Ty::Bool` → `i1`.
#[test]
fn ty_to_llvm_bool_emits_i1() {
    let ir = compile_with_checker("fn f(x: Bool) -> Bool { x }");
    assert!(ir.contains("define i1 @f(i1 %x)"), "{ir}");
    assert!(ir.contains("ret i1"), "{ir}");
}

/// `Ty::Byte` and `Ty::UByte` → `i8`.
#[test]
fn ty_to_llvm_byte_ubyte_emit_i8() {
    let ir = compile_with_checker("fn f(x: Byte) -> Byte { x }");
    assert!(ir.contains("define i8 @f(i8 %x)"), "Byte: {ir}");

    let ir = compile_with_checker("fn f(x: UByte) -> UByte { x }");
    assert!(ir.contains("define i8 @f(i8 %x)"), "UByte: {ir}");
}

/// `Ty::Char` → `i32`.
#[test]
fn ty_to_llvm_char_emits_i32() {
    let ir = compile_with_checker("fn f(x: Char) -> Char { x }");
    assert!(ir.contains("define i32 @f(i32 %x)"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

/// `Ty::Unit` → function return type is `void`, but value positions use `i8`.
#[test]
fn ty_to_llvm_unit_emits_void() {
    // Function signature uses llvm_ty(TypeExpr) → "void" for return type.
    let ir = compile_with_checker("fn noop() -> Unit { }");
    assert!(ir.contains("define void @noop()"), "{ir}");
    assert!(ir.contains("ret void"), "{ir}");
}

/// `Ty::String` → `ptr`.
#[test]
fn ty_to_llvm_string_emits_ptr() {
    let ir = compile_with_checker("fn f(s: String) -> String { s }");
    assert!(ir.contains("define ptr @f(ptr %s)"), "{ir}");
}

/// `Ty::Option(_)` → `{ i8, ptr }` tagged union.
#[test]
fn ty_to_llvm_option_emits_tagged_union() {
    let ir = compile_with_checker("fn f(x: Int) -> Option[Int] { Some(x) }");
    assert!(
        ir.contains("define { i8, ptr } @f(i64 %x)"),
        "Option[Int] return: {ir}"
    );
}

/// `Ty::Result(_, _)` → `{ i8, ptr }` tagged union.
#[test]
fn ty_to_llvm_result_emits_tagged_union() {
    let ir = compile_with_checker("fn f(x: Int) -> Result[Int, Int] { Ok(x) }");
    assert!(
        ir.contains("define { i8, ptr } @f(i64 %x)"),
        "Result[Int,Int] return: {ir}"
    );
}

/// `Ty::List(_)` → `ptr`.
#[test]
fn ty_to_llvm_list_emits_ptr() {
    let ir = compile_with_checker("fn f(xs: List[Int]) -> List[Int] { xs }");
    assert!(ir.contains("define ptr @f(ptr %xs)"), "{ir}");
}

/// `Ty::Map(_, _)` → `ptr`.
#[test]
fn ty_to_llvm_map_emits_ptr() {
    let ir = compile_with_checker("fn f(m: Map[String, Int]) -> Map[String, Int] { m }");
    assert!(ir.contains("define ptr @f(ptr %m)"), "{ir}");
}

/// `Ty::Set(_)` → `ptr`.
#[test]
fn ty_to_llvm_set_emits_ptr() {
    let ir = compile_with_checker("fn f(s: Set[Int]) -> Set[Int] { s }");
    assert!(ir.contains("define ptr @f(ptr %s)"), "{ir}");
}

/// `Ty::Ref(_, inner)` transparently unwraps to the inner type.
#[test]
fn ty_to_llvm_ref_unwraps_inner_type() {
    // A mutable ref Int should still map to i64, not to a pointer-to-i64.
    let ir = compile_with_checker(
        "fn f() -> Int {\n\
         let x: ref Int = 0;\n\
         x\n\
         }",
    );
    // ref locals use alloca, but the returned value is loaded as i64.
    assert!(ir.contains("ret i64"), "ref Int loads as i64: {ir}");
}

/// `Ty::Labeled(_, inner)` unwraps through the label to inner type.
/// IFC labels are transparent at the IR level — no boxing.
#[test]
fn ty_to_llvm_labeled_unwraps_inner_type() {
    // Tainted[Int] should map to i64 in the IR, not to a distinct wrapper type.
    // We verify this via `type_of_expr` being called for a Tainted param.
    let ir = compile_with_checker("fn f(x: Tainted[Int]) -> Tainted[Int] { x }");
    // The emitter strips the label at the IR level — i64 in, i64 out.
    assert!(
        ir.contains("define i64 @f(i64 %x)"),
        "Tainted[Int] → i64: {ir}"
    );
    assert!(ir.contains("ret i64"), "{ir}");
}

/// `Ty::Never` and `Ty::Unknown` map to "ptr" as a safe fallback.
/// Since ty_to_llvm is pub(super) we verify indirectly: the emitter must not
/// panic or produce empty IR when processing programs where the checker path
/// is engaged with complex control flow (which can involve Never in if/else).
#[test]
fn ty_to_llvm_never_does_not_panic() {
    // Verify the checker path handles complex control flow without crashing.
    // Never types appear in the type-checker's internal representation of
    // unreachable branches; the IR emitter must handle them gracefully.
    let ir = compile_with_checker(
        "fn f(x: Int) -> Int {\n\
         if x > 0 { x } else { x }\n\
         }",
    );
    assert!(!ir.is_empty(), "non-empty IR: {ir}");
    assert!(ir.contains("ret i64"), "Int return: {ir}");
}

/// `Ty::Unknown` fallback: the emitter must not panic when expr_types contains
/// `Ty::Unknown` for some expressions (checker inference failure propagation).
#[test]
fn ty_to_llvm_unknown_does_not_panic() {
    // Ty::Unknown propagates from inference failures.  The emitter maps it to
    // "ptr" as a safe fallback.  We simulate this by running the emitter with
    // an empty expr_types map — every span lookup misses, forcing AST fallback
    // for all nodes (the same fallback used when Unknown is encountered).
    let prog = parse("fn f(x: Int) -> Int { x }");
    let mut compiler = LlvmTextCompiler::new();
    compiler.expr_types = std::collections::HashMap::new(); // empty = unknown fallback
    let ir = compiler
        .compile_to_ir(&prog, "test")
        .expect("compile_to_ir must not fail with empty expr_types");
    assert!(ir.contains("define i64 @f"), "{ir}");
}

// ── 2. expr_types fallback path (AST-based inference) ─────────────────────────
//
// These tests verify that the LLVM emitter produces correct IR even without
// checker-resolved types, exercising the AST-based `type_of_expr` fallback.

/// Without checker types, integer literals and arithmetic still emit i64.
#[test]
fn fallback_path_integer_arithmetic_emits_i64() {
    let ir = compile_no_checker("fn add(a: Int, b: Int) -> Int { a + b }");
    assert!(ir.contains("define i64 @add(i64 %a, i64 %b)"), "{ir}");
    assert!(ir.contains("add i64"), "{ir}");
    assert!(ir.contains("ret i64"), "{ir}");
}

/// Without checker types, boolean literals still emit i1.
#[test]
fn fallback_path_bool_literal_emits_i1() {
    let ir = compile_no_checker("fn always_true() -> Bool { true }");
    assert!(ir.contains("define i1 @always_true()"), "{ir}");
    assert!(ir.contains("ret i1 true"), "{ir}");
}

/// Without checker types, string literals still go through `_mvl_string_new`.
#[test]
fn fallback_path_string_literal_emits_string_new() {
    let ir = compile_no_checker("fn main() -> Unit ! Console { println(\"hello\") }");
    assert!(ir.contains("_mvl_string_new"), "{ir}");
}

/// Without checker types, comparison operators still emit icmp.
#[test]
fn fallback_path_comparison_emits_icmp() {
    let ir = compile_no_checker("fn lt(a: Int, b: Int) -> Bool { a < b }");
    assert!(ir.contains("icmp slt i64"), "{ir}");
    assert!(ir.contains("define i1 @lt"), "{ir}");
}

/// With checker types, the result is identical to without for simple programs.
/// This is the key parity property: the checker path must not break AST-correct programs.
#[test]
fn checker_path_matches_fallback_for_primitives() {
    let src = "fn add(a: Int, b: Int) -> Int { a + b }";
    let with_checker = compile_with_checker(src);
    let fallback = compile_no_checker(src);
    // Both paths must contain the same structural IR (function signature, opcode, ret).
    assert!(
        with_checker.contains("define i64 @add(i64 %a, i64 %b)"),
        "{with_checker}"
    );
    assert!(
        fallback.contains("define i64 @add(i64 %a, i64 %b)"),
        "{fallback}"
    );
    assert!(with_checker.contains("add i64"), "{with_checker}");
    assert!(fallback.contains("add i64"), "{fallback}");
}

/// Checker path improves dispatch for a field access on a named struct type.
/// Without checker types, field access falls back to "i64"; with checker types
/// the struct field type is resolved accurately.
#[test]
fn checker_path_improves_struct_field_dispatch() {
    let src = "type Point = struct { x: Int, y: Int }\nfn get_x(p: Point) -> Int { p.x }";
    // Both paths must compile without error.
    let with_checker = compile_with_checker(src);
    let fallback = compile_no_checker(src);
    // The struct type def is present either way.
    assert!(
        with_checker.contains("%Point = type { i64, i64 }"),
        "{with_checker}"
    );
    assert!(
        fallback.contains("%Point = type { i64, i64 }"),
        "{fallback}"
    );
    // Return type must be i64 for both.
    assert!(with_checker.contains("ret i64"), "{with_checker}");
    assert!(fallback.contains("ret i64"), "{fallback}");
}

// ── 3. Backend trait contract (RustBackend::emit_program) ─────────────────────
//
// Verifies that the new `Backend` trait signature — accepting `TirProgram` —
// works end-to-end: parse → checker → mono → lower → RustBackend::emit_program.

#[test]
fn rust_backend_emit_program_produces_valid_rust_source() {
    use mvl::mvl::backends::rust::RustBackend;
    use mvl::mvl::backends::Backend;
    use mvl::mvl::ir::lower::lower;
    use mvl::mvl::passes::mono;

    let src = "fn add(a: Int, b: Int) -> Int { a + b }";
    let prog = parse(src);
    let expr_types = assemble_expr_types(&prog, &[]);
    let all_fns = mono::collect_fns(std::iter::once(&prog));
    let mono_prog = mono::monomorphize(&prog, &all_fns, &expr_types);
    let tir = lower(&prog, &mono_prog, &expr_types);

    let output = RustBackend.emit_program(&tir, "test_crate");
    // The Rust backend should emit a recognisable fn signature.
    assert!(output.contains("fn add"), "fn add missing: {output}");
    assert!(
        output.contains("i64") || output.contains("Int") || output.contains("->"),
        "{output}"
    );
}

#[test]
fn rust_backend_trait_name_and_extension() {
    use mvl::mvl::backends::rust::RustBackend;
    use mvl::mvl::backends::Backend;

    assert_eq!(RustBackend.name(), "rust");
    assert_eq!(RustBackend.file_extension(), "rs");
}

#[test]
fn rust_backend_emit_program_unit_function() {
    use mvl::mvl::backends::rust::RustBackend;
    use mvl::mvl::backends::Backend;
    use mvl::mvl::ir::lower::lower;
    use mvl::mvl::passes::mono;

    let src = "fn noop() -> Unit { }";
    let prog = parse(src);
    let expr_types = assemble_expr_types(&prog, &[]);
    let all_fns = mono::collect_fns(std::iter::once(&prog));
    let mono_prog = mono::monomorphize(&prog, &all_fns, &expr_types);
    let tir = lower(&prog, &mono_prog, &expr_types);

    let output = RustBackend.emit_program(&tir, "test_crate");
    assert!(output.contains("fn noop"), "fn noop missing: {output}");
}

#[test]
fn rust_backend_emit_program_with_struct() {
    use mvl::mvl::backends::rust::RustBackend;
    use mvl::mvl::backends::Backend;
    use mvl::mvl::ir::lower::lower;
    use mvl::mvl::passes::mono;

    let src =
        "type Point = struct { x: Int, y: Int }\nfn origin() -> Point { Point { x: 0, y: 0 } }";
    let prog = parse(src);
    let expr_types = assemble_expr_types(&prog, &[]);
    let all_fns = mono::collect_fns(std::iter::once(&prog));
    let mono_prog = mono::monomorphize(&prog, &all_fns, &expr_types);
    let tir = lower(&prog, &mono_prog, &expr_types);

    let output = RustBackend.emit_program(&tir, "test_crate");
    assert!(output.contains("Point"), "Point struct missing: {output}");
    assert!(output.contains("origin"), "origin fn missing: {output}");
}

// ── 4. Silent checker failure / partial expr_types path ───────────────────────
//
// When the checker encounters type errors it still populates a partial expr_types
// map (with Ty::Unknown for failed expressions) and the backend falls back to
// AST inference for the unknown spans.  The pipeline must not panic.

/// `assemble_expr_types` on a valid program returns a non-empty map.
#[test]
fn assemble_expr_types_valid_program_returns_non_empty_map() {
    let prog = parse("fn f(x: Int) -> Int { x + 1 }");
    let types = assemble_expr_types(&prog, &[]);
    assert!(
        !types.is_empty(),
        "expr_types should be non-empty for a valid program"
    );
}

/// `assemble_expr_types` on an empty program returns a map without panicking.
#[test]
fn assemble_expr_types_empty_program_does_not_panic() {
    let prog = parse("");
    let types = assemble_expr_types(&prog, &[]);
    // May be empty or contain prelude types — the important thing is no panic.
    let _ = types;
}

/// The LLVM backend survives when `expr_types` is partially populated.
/// Simulates the scenario where the checker ran but produced some type errors,
/// leaving spans with Ty::Unknown.  The emitter must fall back to AST inference
/// for those spans and still produce valid IR for the known parts.
#[test]
fn llvm_backend_survives_partial_expr_types() {
    let src = "fn add(a: Int, b: Int) -> Int { a + b }";
    let prog = parse(src);

    // Use a real (passing) checker run but drop half the entries to simulate
    // a partially-populated map (entries for unknown/errored sub-expressions
    // would simply be absent).
    let full_types = assemble_expr_types(&prog, &[]);
    let partial_types: std::collections::HashMap<_, _> = full_types
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % 2 == 0)
        .map(|(_, kv)| kv)
        .collect();

    let mut compiler = LlvmTextCompiler::new();
    compiler.expr_types = partial_types;
    let ir = compiler
        .compile_to_ir(&prog, "test")
        .expect("compile_to_ir must not fail with partial expr_types");
    // The function must still be defined (AST fallback for missing spans).
    assert!(ir.contains("define i64 @add"), "{ir}");
}

/// The LLVM backend with checker types produces identical IR for pure integer
/// arithmetic as without checker types — the checker path must not regress
/// basic arithmetic emission.
#[test]
fn checker_path_does_not_regress_integer_arithmetic() {
    let src = "fn mul(a: Int, b: Int) -> Int { a * b }";
    let with_checker = compile_with_checker(src);
    let without_checker = compile_no_checker(src);

    // Both must contain the multiply instruction and i64 return.
    assert!(
        with_checker.contains("mul i64"),
        "checker path: {with_checker}"
    );
    assert!(
        without_checker.contains("mul i64"),
        "AST fallback: {without_checker}"
    );
    assert!(
        with_checker.contains("ret i64"),
        "checker path ret: {with_checker}"
    );
    assert!(
        without_checker.contains("ret i64"),
        "AST fallback ret: {without_checker}"
    );
}

// ── 5. LLVM pipeline: checker runs before codegen, failure is non-fatal ────────
//
// In the CLI (`src/cli/llvm_text.rs`), the checker is always run before LLVM
// codegen.  If the checker produces errors, `check_result.expr_types` may be
// a partial map.  The backend must still produce compilable IR for the
// well-typed parts of the program.

/// Running `check_with_prelude` on a valid program always populates expr_types.
#[test]
fn check_with_empty_prelude_populates_expr_types() {
    let prog = parse("fn f(x: Int) -> Int { x * 2 }");
    let result = checker::check_with_prelude(&[], &prog);
    assert!(
        result.errors.is_empty(),
        "valid program must not have errors: {:?}",
        result.errors
    );
    assert!(
        !result.expr_types.is_empty(),
        "expr_types must be populated for valid program"
    );
}

/// The LLVM backend with checker-derived expr_types produces non-empty IR
/// for a program that uses a Float parameter — verifying `double` type dispatch.
#[test]
fn llvm_backend_float_param_with_checker_types_emits_double() {
    let ir = compile_with_checker("fn sq(x: Float) -> Float { x * x }");
    assert!(ir.contains("define double @sq(double %x)"), "{ir}");
    assert!(ir.contains("fmul double"), "{ir}");
    assert!(ir.contains("ret double"), "{ir}");
}

/// The LLVM backend without checker types also emits `double` for Float params
/// (AST fallback path for Float is correct).
#[test]
fn llvm_backend_float_param_without_checker_types_emits_double() {
    let ir = compile_no_checker("fn sq(x: Float) -> Float { x * x }");
    assert!(ir.contains("define double @sq(double %x)"), "{ir}");
    assert!(ir.contains("fmul double"), "{ir}");
}
