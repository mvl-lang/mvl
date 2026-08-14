// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_stmts.rs (+ heap-drop tracking in emit_types.rs)` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/stmts.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

#[test]
fn let_binding_aliases_ssa_value() {
    let ir = compile("fn f(x: Int) -> Int { let y: Int = x; y }");
    assert!(ir.contains("ret i64"), "{ir}");
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

// ── Type-aware element drops for nested collections (#1991) ──────────────

#[test]
fn list_of_string_uses_string_ptr_array_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let xs: List[String] = [\"a\", \"b\"];\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_string_ptr_array_drop(ptr"),
        "{ir}"
    );
    assert!(
        ir.contains("declare void @_mvl_string_ptr_array_drop(ptr)"),
        "{ir}"
    );
    assert!(!ir.contains("call void @_mvl_array_drop(ptr"), "{ir}");
}

#[test]
fn list_of_list_uses_array_drop_mvlarray() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let xs: List[List[Int]] = [[1, 2], [3]];\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_array_drop_mvlarray(ptr") && ir.contains("@_mvl_array_drop)"),
        "{ir}"
    );
    assert!(
        ir.contains("declare void @_mvl_array_drop_mvlarray(ptr, ptr)"),
        "{ir}"
    );
}

#[test]
fn list_of_list_of_string_picks_string_inner_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let xs: List[List[String]] = [[\"a\"]];\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_array_drop_mvlarray(ptr")
            && ir.contains("@_mvl_string_ptr_array_drop)"),
        "{ir}"
    );
}

#[test]
fn list_of_option_int_uses_array_drop_option_with_null_payload_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let ys: List[Option[Int]] = [Some(1), None];\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_array_drop_option(ptr") && ir.contains("i64 8, ptr null)"),
        "{ir}"
    );
    assert!(
        ir.contains("declare void @_mvl_array_drop_option(ptr, i64, ptr)"),
        "{ir}"
    );
}

#[test]
fn list_of_option_string_uses_string_drop_payload() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let ys: List[Option[String]] = [Some(\"a\"), None];\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_array_drop_option(ptr") && ir.contains("@_mvl_string_drop)"),
        "{ir}"
    );
}

#[test]
fn list_of_result_uses_array_drop_result() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let rs: List[Result[Int, Bool]] = [Ok(1), Err(true)];\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_array_drop_result(ptr"), "{ir}");
    assert!(
        ir.contains("declare void @_mvl_array_drop_result(ptr, i64, ptr, i64, ptr)"),
        "{ir}"
    );
}

#[test]
fn map_string_value_uses_map_drop_ptr_values() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let m: Map[String, String] = {\"a\": \"1\"};\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_map_drop_ptr_values(ptr") && ir.contains("@_mvl_string_drop)"),
        "{ir}"
    );
    assert!(
        ir.contains("declare void @_mvl_map_drop_ptr_values(ptr, ptr)"),
        "{ir}"
    );
    assert!(!ir.contains("call void @_mvl_map_drop(ptr"), "{ir}");
}

#[test]
fn map_scalar_value_still_uses_plain_map_drop() {
    let ir = compile(
        "fn f() -> Unit {\n\
         let m: Map[String, Int] = {\"a\": 1};\n\
         }",
    );
    assert!(ir.contains("call void @_mvl_map_drop(ptr"), "{ir}");
    assert!(!ir.contains("_mvl_map_drop_ptr_values"), "{ir}");
}

#[test]
fn map_insert_string_value_excludes_from_heap_locals() {
    // The inserted `v` must not also be dropped at scope exit — otherwise
    // it would be double-freed once the map's own drop follows the value
    // pointer (#1991).
    let ir = compile(
        "fn f() -> Unit {\n\
         let m: ref Map[String, String] = {\"seed\": \"0\"};\n\
         let v: String = \"hello\";\n\
         m.insert(\"k\", v);\n\
         }",
    );
    let string_drop_count = ir.matches("call void @_mvl_string_drop(ptr").count();
    assert_eq!(
        string_drop_count, 0,
        "expected 0 direct _mvl_string_drop calls on `v` (owned by the map now), got {string_drop_count}\n{ir}"
    );
}

/// #2265: `ref_locals` and `locals` are both function-scoped, and
/// `emit_expr_tir`'s `Var` arm consults `ref_locals` first — so a `ref`
/// binding introduced in one branch kept capturing that name in a *sibling*
/// branch that declared its own plain local. The sibling's reads compiled to
/// a load from the other branch's alloca (never stored to on that path), i.e.
/// uninitialized stack memory. `examples/bzip/huffman.mvl::build_tree` hit
/// this with a `codes` binding declared `ref` in one arm and plain in the
/// other.
#[test]
fn plain_let_shadows_ref_binding_from_sibling_branch() {
    let ir = compile(
        "fn pick(flag: Bool) -> List[Int] {\n\
         if flag {\n\
         let xs: ref List[Int] = [1, 2];\n\
         xs.push(3);\n\
         xs\n\
         } else {\n\
         let xs: List[Int] = [9];\n\
         xs\n\
         }\n\
         }",
    );
    // Isolate the `else_N:` basic block (label line through its terminator).
    let else_body: String = ir
        .lines()
        .skip_while(|l| !l.starts_with("else_"))
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with("br label"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !else_body.is_empty(),
        "else branch must be emitted:\n{ir}"
    );
    // The else arm builds its own array and returns that; loading the
    // then-arm's `alloca ptr` here is the bug this pins.
    assert!(
        !else_body.contains("load ptr, ptr"),
        "else branch must not load the sibling ref binding's alloca:\n{else_body}\n--- full ---\n{ir}"
    );
    assert!(
        else_body.contains("call ptr @_mvl_array_new"),
        "else branch must build its own array:\n{else_body}"
    );
}

/// #2265: `let result: ref T = <existing heap local>` aliases the
/// initializer's allocation rather than copying it, so ownership moves into
/// the new binding — the source must stop being independently drop-tracked.
/// Without this the same allocation was dropped twice, and when the binding
/// was the return value the stale entry freed it before the caller read it
/// (`examples/bzip/huffman.mvl::remove_at_ll`).
#[test]
fn ref_let_from_heap_local_transfers_ownership() {
    let ir = compile(
        "fn take(a: List[List[Int]], b: List[List[Int]]) -> List[List[Int]] {\n\
         let before: List[List[Int]] = a.slice(0, 1);\n\
         let out: ref List[List[Int]] = before;\n\
         out.extend(b);\n\
         out\n\
         }",
    );
    let body = ir
        .split("define ptr @take")
        .nth(1)
        .expect("@take must be emitted");
    let body: String = body.chars().take_while(|c| *c != '}').collect();
    // `before`'s slice result is the returned allocation — it must not be
    // dropped on the way out.
    let slice_reg = body
        .lines()
        .find(|l| l.contains("_mvl_list_slice"))
        .and_then(|l| l.split_whitespace().next())
        .expect("slice call must be emitted")
        .to_string();
    assert!(
        !body.contains(&format!("_mvl_array_drop_mvlarray(ptr {slice_reg},")),
        "the aliased-and-returned slice result must not be dropped:\n{body}"
    );
}

/// #2286: a tail-position `match` statement yields the function's value
/// through a phi, but `exclude_returned_value_tir` was never applied to its
/// arms — so an arm returning an owned local had that local dropped by the
/// scope-exit sweep immediately before the `ret` that returned it.
#[test]
fn tail_match_arm_value_excluded_from_drop_sweep() {
    let ir = compile(
        "fn opt_or(opt: Option[String], default: String) -> String {\n\
         let d: String = consume(default);\n\
         match opt { Some(s) => s, None => d, }\n\
         }",
    );
    let body: String = ir
        .split("define ptr @opt_or")
        .nth(1)
        .expect("@opt_or must be emitted")
        .lines()
        .take_while(|l| *l != "}")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !body.contains("_mvl_string_drop(ptr %default)"),
        "the arm value being returned must not be dropped on the way out:\n{body}"
    );
}
