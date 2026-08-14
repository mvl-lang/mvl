// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_construct.rs` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/construct.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

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

/// #2265: an empty list literal as a *struct field* value used the static,
/// context-free `llvm_ty` to size its elements, which falls any
/// module-registered base name it doesn't know (a payload enum, here
/// `Tree`) straight to a bare 8-byte `ptr`. A payload enum's real
/// representation is the 16-byte `{ i8, ptr }` tagged union, so the array
/// was created with `elem_size 8` and every later `.push` stored only the
/// discriminant half, silently dropping the payload pointer.
#[test]
fn empty_payload_enum_list_struct_field_uses_16_byte_elems() {
    let ir = compile(
        "type Tree = enum { Leaf(Int, Int), Node(Int, Box[Tree], Box[Tree]) }\n\
         type St = struct { queue: List[Tree] }\n\
         fn main() -> Unit {\n\
         let s: St = St { queue: [] };\n\
         }",
    );
    assert!(
        ir.contains("call ptr @_mvl_array_new(i64 16,"),
        "empty List[<payload enum>] struct field must allocate 16-byte elements:\n{ir}"
    );
}

/// #2265: the same empty-literal element sizing, via a `let` binding's own
/// declared type rather than a struct field's — already correct before the
/// fix (that path uses the context-aware `ty_to_llvm_ctx`), pinned here so
/// the two call sites can't drift apart again.
#[test]
fn empty_payload_enum_list_let_binding_uses_16_byte_elems() {
    let ir = compile(
        "type Tree = enum { Leaf(Int, Int), Node(Int, Box[Tree], Box[Tree]) }\n\
         fn main() -> Unit {\n\
         let q: List[Tree] = [];\n\
         }",
    );
    assert!(
        ir.contains("call ptr @_mvl_array_new(i64 16,"),
        "empty List[<payload enum>] let binding must allocate 16-byte elements:\n{ir}"
    );
}

/// #2265: `List[T]::extend` on a payload-enum element type dispatches to
/// `_mvl_array_extend_enum` with the per-type clone trampoline as its 3rd
/// argument; before, the arm fell through to `None` and emitted nothing at
/// all — the call silently vanished.
#[test]
fn extend_on_payload_enum_list_dispatches_with_clone_trampoline() {
    let ir = compile(
        "type Tree = enum { Leaf(Int, Int), Node(Int, Box[Tree], Box[Tree]) }\n\
         fn join(a: List[Tree], b: List[Tree]) -> List[Tree] {\n\
         let out: ref List[Tree] = a;\n\
         out.extend(b);\n\
         out\n\
         }",
    );
    assert!(
        ir.contains("call void @_mvl_array_extend_enum(ptr")
            && ir.contains("ptr @_mvl_clone_enum_Tree)"),
        "extend on List[<payload enum>] must call _mvl_array_extend_enum with the \
         per-type clone trampoline:\n{ir}"
    );
}

/// #2265: the generated per-enum clone trampoline recurses through its own
/// `Box[Tree]` fields via the same memoized symbol (not an unbounded
/// generator recursion), and allocates a fresh box per boxed field rather
/// than aliasing the source's.
#[test]
fn enum_clone_trampoline_recurses_through_box_fields() {
    let ir = compile(
        "type Tree = enum { Leaf(Int, Int), Node(Int, Box[Tree], Box[Tree]) }\n\
         fn head(q: List[Tree]) -> Option[Tree] { q.first() }",
    );
    assert!(
        ir.contains("define ptr @_mvl_clone_enum_Tree(i8 %disc, ptr %payload)"),
        "clone trampoline must be emitted:\n{ir}"
    );
    assert!(
        ir.matches("call ptr @_mvl_clone_enum_Tree(").count() >= 2,
        "Node's two Box[Tree] fields must each recurse through the trampoline:\n{ir}"
    );
    assert!(
        ir.matches("call ptr @_mvl_box_new(i64 16)").count() >= 2,
        "each cloned Box[Tree] field needs its own fresh 16-byte box:\n{ir}"
    );
    assert_eq!(
        ir.matches("define ptr @_mvl_clone_enum_Tree(").count(),
        1,
        "trampoline must be emitted exactly once (memoized):\n{ir}"
    );
}

/// #2286: a second, *static* `llvm_type_size` returned a flat 8 for any named
/// struct, and the collection call sites used it — so a `List[Rec]` of a
/// 40-byte `%Rec` was created with `elem_size 8` and every element was
/// truncated to its first 8 bytes. Reading field 0 worked; every later field
/// returned garbage.
#[test]
fn struct_element_list_uses_real_struct_size() {
    let ir = compile(
        "type Rec = struct { name: String, age: Int, amount: Float, category: Option[String] }\n\
         fn mk() -> List[Rec] {\n\
         [Rec { name: \"a\", age: 1, amount: 2.0, category: None }]\n\
         }",
    );
    // ptr 8 + i64 8 + double 8 + { i8, ptr } 16 = 40.
    assert!(
        ir.contains("call ptr @_mvl_array_new(i64 40,"),
        "a List[Rec] must size its elements at sizeof(%Rec) = 40, not 8:\n{ir}"
    );
}

/// #2286: `fold`'s empty accumulator literal has no element to infer from and
/// no declared type at that position, so it fell back to the 8-byte "ptr"
/// default — then got `concat`-ed with correctly sized arrays built inside the
/// closure, mixing strides in one buffer. The closure's own first parameter
/// carries the exact accumulator type.
#[test]
fn fold_empty_accumulator_sizes_from_closure_param() {
    let ir = compile(
        "type Rec = struct { name: String, age: Int, amount: Float, category: Option[String] }\n\
         fn ids(rs: List[Rec]) -> List[Rec] {\n\
         rs.fold([], |acc: List[Rec], r: Rec| acc.concat([r]))\n\
         }",
    );
    assert!(
        !ir.contains("call ptr @_mvl_array_new(i64 8,"),
        "fold's empty List[Rec] accumulator must not be sized at 8 bytes:\n{ir}"
    );
    assert!(
        ir.contains("call ptr @_mvl_array_new(i64 40,"),
        "fold's empty accumulator must take its element size from the \
         closure's `acc` parameter:\n{ir}"
    );
}
