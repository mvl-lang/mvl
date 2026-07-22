// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Method return-type lookup tables for built-in types.
//!
//! Each function takes a method name (and optionally argument types) and returns
//! the `Ty` that method call produces.  All functions are pure, stateless helpers
//! with no `&self` parameter — split into this submodule purely for size.
//!
//! # 4-way sync (#992)
//!
//! Adding a new method to a builtin type requires **synchronized changes** in four places:
//!
//! 1. **`std/*.mvl`** — declare the method (signature + body or `builtin`)
//! 2. **`checker/method_types.rs`** ← *this file* — add the return type mapping
//! 3. **`backends/rust/emit_exprs.rs`** — add an emission match arm, or add a
//!    `rust_emit` hint to `BUILTINS` in `backends.rs` for table-driven dispatch
//! 4. **`backends/llvm_text/emit_method_call.rs`** — add the LLVM emission arm
//!
//! **Shared constants** (`STDLIB_UFCS_METHODS`, `STRING_LABEL_PRESERVING_METHODS`)
//! now live in `backends.rs` as the single source of truth — both TIR and AST
//! emitters import from there.  Runtime function mappings (previously
//! `STDLIB_BUILTIN_METHODS`) are encoded as `BuiltinDesc.rust_emit` hints in
//! the `BUILTINS` registry.  `STDLIB_UFCS_METHODS` now carries
//! `(method, receiver_type)` pairs, and the `ufcs_sync_tests` module at the
//! bottom of this file asserts every entry has a non-`Unknown` return-type arm
//! here — closing one direction of the divergence gap (#1390).
//!
//! Missing any one of these causes: wrong type inference (#985), missing emission
//! (runtime crash), or method not callable.  See issue #992 for the planned fix
//! (method desugaring in the checker that eliminates the 4-way requirement).

use crate::mvl::checker::types::Ty;

use super::TypeChecker;

impl TypeChecker {
    /// Return type for methods on `Int`.
    pub(super) fn int_method_ty(method: &str) -> Ty {
        match method {
            // Conversion
            "to_float" => Ty::Float,
            "to_string" => Ty::String,
            // Arithmetic
            "abs" | "pow" | "min" | "max" | "clamp" => Ty::Int,
            // Bitwise
            "bit_and" | "bit_or" | "bit_xor" | "bit_not" | "shift_left" | "shift_right" => Ty::Int,
            // Overflow-checking (return Option[Int])
            "checked_add" | "checked_sub" | "checked_mul" | "checked_div" => {
                Ty::Option(Box::new(Ty::Int))
            }
            // Explicit wrapping (document intent)
            "wrapping_add" | "wrapping_sub" | "wrapping_mul" => Ty::Int,
            // Predicates
            "is_positive" | "is_negative" | "is_zero" => Ty::Bool,
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Byte`.
    pub(super) fn byte_method_ty(method: &str) -> Ty {
        match method {
            // Conversion
            "to_int" => Ty::Int,
            "to_string" => Ty::String,
            // Bitwise (same set as Int)
            "bit_and" | "bit_or" | "bit_xor" | "bit_not" | "shift_left" | "shift_right" => Ty::Byte,
            // Arithmetic — Rust's u8 exposes these natively; the transpiler's
            // generic method-call fallthrough emits `receiver.wrapping_add(arg)`
            // which is valid Rust.  No dedicated emit arm is required.
            "wrapping_add" | "wrapping_sub" | "wrapping_mul" => Ty::Byte,
            "checked_add" | "checked_sub" | "checked_mul" => Ty::Option(Box::new(Ty::Byte)),
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `UByte`.
    pub(super) fn ubyte_method_ty(method: &str) -> Ty {
        match method {
            "to_int" => Ty::Int,
            "to_string" => Ty::String,
            "bit_and" | "bit_or" | "bit_xor" | "bit_not" | "shift_left" | "shift_right" => {
                Ty::UByte
            }
            "wrapping_add" | "wrapping_sub" | "wrapping_mul" => Ty::UByte,
            "checked_add" | "checked_sub" | "checked_mul" => Ty::Option(Box::new(Ty::UByte)),
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `UInt`.
    pub(super) fn uint_method_ty(method: &str) -> Ty {
        match method {
            "to_float" => Ty::Float,
            "to_string" => Ty::String,
            "abs" | "pow" | "min" | "max" | "clamp" => Ty::UInt,
            "bit_and" | "bit_or" | "bit_xor" | "bit_not" | "shift_left" | "shift_right" => Ty::UInt,
            "is_zero" => Ty::Bool,
            "wrapping_add" | "wrapping_sub" | "wrapping_mul" => Ty::UInt,
            "checked_add" | "checked_sub" | "checked_mul" => Ty::Option(Box::new(Ty::UInt)),
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Float`.
    pub(super) fn float_method_ty(method: &str) -> Ty {
        match method {
            // Conversion
            "to_int" => Ty::Int,
            "to_string" => Ty::String,
            // Arithmetic
            "abs" | "ceil" | "floor" | "round" | "sqrt" | "min" | "max" | "clamp" | "pow" => {
                Ty::Float
            }
            // Predicates
            "is_nan" | "is_infinite" | "is_finite" | "is_positive" | "is_negative" => Ty::Bool,
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Option<T>`.
    pub(super) fn option_method_ty(inner: &Ty, method: &str, arg_tys: &[Ty]) -> Ty {
        match method {
            "is_some" | "is_none" => Ty::Bool,
            "unwrap_or" => inner.clone(),
            // map(f: fn(T) -> U) -> Option<U>
            "map" => {
                let u = if let Some(Ty::Fn(_, ret, ..)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                };
                Ty::Option(Box::new(u))
            }
            // and_then(f: fn(T) -> Option<U>) -> Option<U>
            "and_then" => {
                if let Some(Ty::Fn(_, ret, ..)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                }
            }
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Result<T, E>`.
    pub(super) fn result_method_ty(ok_ty: &Ty, method: &str, arg_tys: &[Ty]) -> Ty {
        match method {
            "is_ok" | "is_err" => Ty::Bool,
            "unwrap_or" => ok_ty.clone(),
            // map(f: fn(T) -> U) -> Result<U, E>  — infer U from lambda return type
            "map" => {
                let u = if let Some(Ty::Fn(_, ret, ..)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                };
                // We don't track E in the return type here; use Unknown for E
                Ty::Result(Box::new(u), Box::new(Ty::Unknown))
            }
            // and_then(f: fn(T) -> Result<U,E>) -> Result<U,E>
            "and_then" => {
                if let Some(Ty::Fn(_, ret, ..)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                }
            }
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Bool`.
    pub(super) fn bool_method_ty(method: &str) -> Ty {
        match method {
            "to_string" => Ty::String,
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `String`.
    pub(super) fn string_method_ty(method: &str, arg_tys: &[Ty]) -> Ty {
        match method {
            // Splitting: String → List<String> (never panics, always valid)
            "split" | "chars" | "lines" => Ty::List(Box::new(Ty::String)),
            // Transformations returning String
            "trim" | "trim_start" | "trim_end" | "to_upper" | "to_lower" | "replace"
            | "replace_all" | "format" => Ty::String,
            // concat(other: String) -> String — exactly one String argument required
            "concat" if arg_tys.len() == 1 && matches!(arg_tys[0], Ty::String) => Ty::String,
            "concat" => Ty::Unknown,
            // Searching: Option<Int> — returns None when not found
            "find" | "rfind" => Ty::Option(Box::new(Ty::Int)),
            // Predicates
            "contains" | "starts_with" | "ends_with" | "is_empty" => Ty::Bool,
            // Indexing: char_at(i) -> Option[String], byte_at(i) -> Option[Byte]
            "char_at" => Ty::Option(Box::new(Ty::String)),
            "byte_at" => Ty::Option(Box::new(Ty::Byte)),
            // Numeric
            "len" => Ty::Int,
            // Parsing
            "parse_int" => Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            "parse_float" => Ty::Result(Box::new(Ty::Float), Box::new(Ty::String)),
            // Slicing: substring(start, end) — exclusive range → String; requires 2 Int args
            "substring"
                if arg_tys.len() == 2
                    && matches!((&arg_tys[0], &arg_tys[1]), (Ty::Int, Ty::Int)) =>
            {
                Ty::String
            }
            "substring" => Ty::Unknown,
            // std/text.mvl — List[Span] returning String extensions (#1371)
            // LLVM step 4 pending (#1372): LLVM backend returns Ok(None) for these.
            "split_spans" | "find_all_spans" => {
                Ty::List(Box::new(Ty::Named("Span".into(), vec![])))
            }
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `List<T>`.
    pub(super) fn list_method_ty(elem_ty: &Ty, method: &str, arg_tys: &[Ty]) -> Ty {
        match method {
            // map(f: fn(T) -> U) -> List<U>  — infer U from lambda return type
            "map" => {
                let u_ty = if let Some(Ty::Fn(_, ret, ..)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                };
                Ty::List(Box::new(u_ty))
            }
            // filter(f: fn(T) -> Bool) -> List<T>
            "filter" | "sort" | "sort_by" | "collect" | "rev" | "dedup" => {
                Ty::List(Box::new(elem_ty.clone()))
            }
            // min_by / max_by (f: fn(T, T) -> Bool) -> Option<T>
            "min_by" | "max_by" => Ty::Option(Box::new(elem_ty.clone())),
            // fold(init: U, f: fn(U, T) -> U) -> U  — U inferred from init type
            "fold" => {
                if let Some(init_ty) = arg_tys.first() {
                    init_ty.clone()
                } else {
                    Ty::Unknown
                }
            }
            // reduce(f: fn(T, T) -> T) -> Option<T>  — returns None for empty list
            "reduce" => Ty::Option(Box::new(elem_ty.clone())),
            // enumerate() -> List[Indexed[T]]   (struct, not tuple — ADR-0002, #1380)
            "enumerate" => Ty::List(Box::new(Ty::Named("Indexed".into(), vec![elem_ty.clone()]))),
            // zip(other: List[U]) -> List[Pair[T, U]]   (struct, not tuple — #1380)
            "zip" => {
                let u_ty = if let Some(Ty::List(u)) = arg_tys.first() {
                    *u.clone()
                } else {
                    Ty::Unknown
                };
                Ty::List(Box::new(Ty::Named(
                    "Pair".into(),
                    vec![elem_ty.clone(), u_ty],
                )))
            }
            // join(sep: String) -> String  — only meaningful for List<String>
            "join" => Ty::String,
            // Numeric
            "len" => Ty::Int,
            // Predicates
            "contains" | "is_empty" | "any" | "all" => Ty::Bool,
            // Safe indexed access — Option, never panic
            "first" | "last" => Ty::Option(Box::new(elem_ty.clone())),
            "get" => Ty::Option(Box::new(elem_ty.clone())),
            // Mutations
            "push" | "extend" | "append" | "set" => Ty::Unit,
            // List → List transformations
            "concat" | "reverse" => Ty::List(Box::new(elem_ty.clone())),
            // Flat-map
            "flat_map" => {
                let u_ty = if let Some(Ty::Fn(_, ret, ..)) = arg_tys.first() {
                    if let Ty::List(inner) = ret.as_ref() {
                        *inner.clone()
                    } else {
                        *ret.clone()
                    }
                } else {
                    Ty::Unknown
                };
                Ty::List(Box::new(u_ty))
            }
            // find returns the element wrapped in Option
            "find" => Ty::Option(Box::new(elem_ty.clone())),
            // min/max — Option<T>
            "min" | "max" => Ty::Option(Box::new(elem_ty.clone())),
            // slice(start, end) — exclusive range → List<T>; requires exactly 2 Int args
            "slice"
                if arg_tys.len() == 2
                    && matches!((&arg_tys[0], &arg_tys[1]), (Ty::Int, Ty::Int)) =>
            {
                Ty::List(Box::new(elem_ty.clone()))
            }
            "slice" => Ty::Unknown,
            // take(n)/skip(n) — first/last N elements → List<T>
            "take" | "skip" => Ty::List(Box::new(elem_ty.clone())),
            // take_while(f)/skip_while(f) — List<T>
            "take_while" | "skip_while" => Ty::List(Box::new(elem_ty.clone())),
            // windows(n)/chunks(n) — List<List<T>>
            "windows" | "chunks" => Ty::List(Box::new(Ty::List(Box::new(elem_ty.clone())))),
            // flatten() — List<List<U>> → List<U>; infer U from elem_ty
            "flatten" => {
                let inner = if let Ty::List(u) = elem_ty {
                    *u.clone()
                } else {
                    Ty::Unknown
                };
                Ty::List(Box::new(inner))
            }
            // partition(f) -> Partitioned[T]   (struct, not tuple — ADR-0002, #1380)
            "partition" => Ty::Named("Partitioned".into(), vec![elem_ty.clone()]),
            // group_by(f: fn(T) -> K) — Map<K, List<T>>
            "group_by" => {
                let k_ty = if let Some(Ty::Fn(_, ret, ..)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                };
                Ty::Map(
                    Box::new(k_ty),
                    Box::new(Ty::List(Box::new(elem_ty.clone()))),
                )
            }
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Map<K, V>`.
    pub(super) fn map_method_ty(k_ty: &Ty, v_ty: &Ty, method: &str, arg_tys: &[Ty]) -> Ty {
        match method {
            // Safe access — Option<V>, never panic
            "get" => Ty::Option(Box::new(v_ty.clone())),
            // Predicates
            "contains_key" | "is_empty" => Ty::Bool,
            // Numeric
            "len" => Ty::Int,
            // Mutation
            "insert" | "remove_entry" => Ty::Unit,
            // remove returns old value if present
            "remove" => Ty::Option(Box::new(v_ty.clone())),
            // Iteration views
            "keys" => Ty::List(Box::new(k_ty.clone())),
            "values" => Ty::List(Box::new(v_ty.clone())),
            // entries() -> List[Entry[K, V]]   (struct, not tuple — ADR-0002, #1380)
            "entries" => Ty::List(Box::new(Ty::Named(
                "Entry".into(),
                vec![k_ty.clone(), v_ty.clone()],
            ))),
            // ── HOFs ──
            // map_values(f: fn(V) -> U) -> Map<K, U>  — infer U from lambda return type
            "map_values" => {
                let u_ty = if let Some(Ty::Fn(_, ret, ..)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                };
                Ty::Map(Box::new(k_ty.clone()), Box::new(u_ty))
            }
            // filter(f: fn(V) -> Bool) -> Map<K, V>
            "filter" => Ty::Map(Box::new(k_ty.clone()), Box::new(v_ty.clone())),
            // fold(init: U, f: fn(U, V) -> U) -> U  — U inferred from init type
            "fold" => {
                if let Some(init_ty) = arg_tys.first() {
                    init_ty.clone()
                } else {
                    Ty::Unknown
                }
            }
            // any/all(f: fn(V) -> Bool) -> Bool
            "any" | "all" => Ty::Bool,
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Set<T>`.
    pub(super) fn set_method_ty(t_ty: &Ty, method: &str, arg_tys: &[Ty]) -> Ty {
        match method {
            "contains" | "is_empty" | "is_subset" | "is_superset" => Ty::Bool,
            "len" => Ty::Int,
            "insert" | "remove" => Ty::Unit,
            "iter" | "to_list" => Ty::List(Box::new(t_ty.clone())),
            "union" | "intersection" | "difference" => Ty::Set(Box::new(t_ty.clone())),
            // ── HOFs ──
            // map(f: fn(T) -> U) -> Set<U>  — infer U from lambda return type
            "map" => {
                let u_ty = if let Some(Ty::Fn(_, ret, ..)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                };
                Ty::Set(Box::new(u_ty))
            }
            // filter(f: fn(T) -> Bool) -> Set<T>
            "filter" => Ty::Set(Box::new(t_ty.clone())),
            // fold(init: U, f: fn(U, T) -> U) -> U  — U inferred from init type
            "fold" => {
                if let Some(init_ty) = arg_tys.first() {
                    init_ty.clone()
                } else {
                    Ty::Unknown
                }
            }
            // any/all(f: fn(T) -> Bool) -> Bool
            "any" | "all" => Ty::Bool,
            _ => Ty::Unknown,
        }
    }
}

#[cfg(test)]
mod ufcs_sync_tests {
    //! Closes one direction of the 4-way sync (#992): every method listed in
    //! `STDLIB_UFCS_METHODS` (backends.rs) must have a non-`Unknown` return
    //! type arm in this file.  Without this test, a UFCS entry could be added
    //! to the transpiler without a matching type-inference arm, causing
    //! silent `Unknown`-typed expressions downstream.

    use super::*;
    use crate::mvl::backends::STDLIB_UFCS_METHODS;
    use crate::mvl::checker::TypeChecker;

    #[test]
    fn stdlib_ufcs_methods_have_return_types() {
        let placeholder = Ty::Int;
        for (method, receiver) in STDLIB_UFCS_METHODS {
            let ty = match *receiver {
                "String" => TypeChecker::string_method_ty(method, &[]),
                "List" => TypeChecker::list_method_ty(&placeholder, method, &[]),
                other => panic!(
                    "STDLIB_UFCS_METHODS entry ({method}, {other}) — receiver type \
                     not handled by ufcs_sync_tests; add an arm for '{other}'"
                ),
            };
            assert_ne!(
                ty,
                Ty::Unknown,
                "{receiver}::{method} is in STDLIB_UFCS_METHODS but \
                 {}_method_ty returns Ty::Unknown — divergence between \
                 backends.rs and checker/method_types.rs",
                receiver.to_lowercase()
            );
        }
    }
}
