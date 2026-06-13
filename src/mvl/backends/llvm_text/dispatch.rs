// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! LLVM-text backend dispatch table.
//!
//! Per-method LLVM dispatch info (C-ABI symbol, extern signature, return
//! type) lives in this file, **not** in the shared `crate::mvl::backends`
//! registry.  The shared `BUILTINS` table carries only backend-neutral facts
//! (name, receiver, arity); each backend stores its own dispatch metadata in
//! its own module.  Tables join by method name.
//!
//! See #1399 for the original motivation and #1388 for the broader
//! "fix leaky abstractions" epic — putting LLVM IR strings
//! (`"ptr @sym(ptr, i64)"`, `"i64"`) into a backend-neutral file was itself
//! a leak.

/// How the LLVM-text backend should emit a call to a builtin method.
///
/// Variants cover the dispatch shapes catalogued in #1399:
///
/// - [`Dispatch::CCall`] — Shape A: simple C-ABI runtime call.
/// - [`Dispatch::CCallBoolFromI64`] — Shape B: C call returns `i64`,
///   coerced to `i1` via `icmp ne i64 X, 0`.
///
/// - [`Dispatch::CCallOptionOutPtr`] — Shape C: C call returns `i8` discriminant + out-ptr payload.
/// - [`Dispatch::CCallStructFromSlots`] — Shape D: C call returns ptr to N-slot array, assembled into a named struct.
///
/// Shape E (`CCallHof`, HOF closures) is deferred to a later #1399 phase.
#[derive(Debug, Clone)]
pub enum Dispatch {
    /// Simple C call producing a single return register.
    ///
    /// Emits:
    /// ```text
    /// declare {signature}      // via ensure_extern (deduped)
    /// {reg} = call {ret_ty} @{sym}({arg_list})
    /// ```
    CCall {
        /// C-ABI symbol used in both `declare` and `call` instructions.
        sym: &'static str,
        /// Text emitted after `declare ` in `ensure_extern`,
        /// e.g. `"ptr @_mvl_string_chars(ptr)"`.
        signature: &'static str,
        /// LLVM type of the call's result register.
        ret_ty: &'static str,
    },
    /// Shape B: C call returns `i64`, the result is then coerced to `i1`
    /// via `icmp ne i64 X, 0`.  The C runtime convention for predicate
    /// builtins (returns 0/1 as i64 instead of i1 directly).
    ///
    /// Emits:
    /// ```text
    /// declare {signature}              // ensure_extern, returns i64
    /// {raw} = call i64 @{sym}({arg_list})
    /// {reg} = icmp ne i64 {raw}, 0
    /// ```
    CCallBoolFromI64 {
        sym: &'static str,
        signature: &'static str,
    },
    /// Shape C: C call uses an out-pointer for the payload, returning the
    /// `i8` discriminant directly.  The MVL-level result is `Option[T]`
    /// where `T` has LLVM type `payload_ty`.
    ///
    /// Emits:
    /// ```text
    /// declare {signature}                              // returns i8
    /// {out} = alloca {payload_ty}
    /// {tag} = call i8 @{sym}({arg_list}, ptr {out})
    /// {payload} = load {payload_ty}, ptr {out}
    /// {slot} = alloca {payload_ty}
    /// store {payload_ty} {payload}, ptr {slot}
    /// {reg} = wrap_result_pair({tag}, {slot})
    /// ```
    /// The trailing `, ptr {out}` is appended to the call's `arg_list`
    /// automatically by the helper — the dispatch entry's `signature`
    /// must already include the out-ptr parameter.
    CCallOptionOutPtr {
        sym: &'static str,
        signature: &'static str,
        /// LLVM type of the payload stored at the out-pointer
        /// (e.g. `"ptr"` for `Option[String]`, `"i64"` for `Option[Byte]`).
        payload_ty: &'static str,
    },
    /// Shape D: C call returns a pointer to an N-slot array; emitter loads
    /// each slot and assembles a named LLVM struct.  Currently used only
    /// for `List::partition`, which returns ptr to a 2-slot array of
    /// `MvlArray*` and is wrapped as `%Partitioned { ptr, ptr }`.
    ///
    /// Emits:
    /// ```text
    /// declare {signature}                                  // returns ptr
    /// {raw} = call ptr @{sym}({arg_list})
    /// for i in 0..slot_tys.len():
    ///   {ptr_i} = getelementptr {slot_tys[i]}, ptr {raw}, i64 i
    ///   {val_i} = load {slot_tys[i]}, ptr {ptr_i}
    /// {tmp_0} = insertvalue {struct_name} undef, {slot_tys[0]} {val_0}, 0
    /// {tmp_i} = insertvalue {struct_name} {tmp_(i-1)}, {slot_tys[i]} {val_i}, i
    /// ```
    /// The final `{tmp_(N-1)}` is the result register.
    CCallStructFromSlots {
        sym: &'static str,
        signature: &'static str,
        /// Named LLVM struct type to assemble (e.g. `"%Partitioned"`).
        struct_name: &'static str,
        /// LLVM type of each slot in the runtime-returned array, in order.
        slot_tys: &'static [&'static str],
    },
}

impl Dispatch {
    /// Returns the C-ABI symbol for this dispatch.
    pub const fn sym(&self) -> &'static str {
        match self {
            Dispatch::CCall { sym, .. } => sym,
            Dispatch::CCallBoolFromI64 { sym, .. } => sym,
            Dispatch::CCallOptionOutPtr { sym, .. } => sym,
            Dispatch::CCallStructFromSlots { sym, .. } => sym,
        }
    }
}

/// Per-method LLVM dispatch table, keyed by MVL method name.
///
/// Joined with `crate::mvl::backends::BUILTINS` by name lookup.  Entries
/// here are LLVM-backend-private; the shared registry has no visibility into
/// them.
///
/// Adding a new entry: register the method in `BUILTINS` (so name, arity,
/// receiver are visible to both backends), then add a row here with the
/// LLVM call shape.
pub const LLVM_DISPATCH: &[(&str, Dispatch)] = &[
    (
        "chars",
        Dispatch::CCall {
            sym: "_mvl_string_chars",
            signature: "ptr @_mvl_string_chars(ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "char_at",
        Dispatch::CCallOptionOutPtr {
            sym: "_mvl_str_char_at",
            signature: "i8 @_mvl_str_char_at(ptr, i64, ptr)",
            payload_ty: "ptr",
        },
    ),
    (
        "byte_at",
        Dispatch::CCallOptionOutPtr {
            sym: "_mvl_str_byte_at",
            signature: "i8 @_mvl_str_byte_at(ptr, i64, ptr)",
            payload_ty: "i64",
        },
    ),
    (
        "find",
        Dispatch::CCall {
            sym: "_mvl_str_find",
            signature: "i64 @_mvl_str_find(ptr, ptr)",
            ret_ty: "i64",
        },
    ),
    (
        "split",
        Dispatch::CCall {
            sym: "_mvl_str_split",
            signature: "ptr @_mvl_str_split(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "substring",
        Dispatch::CCall {
            sym: "_mvl_str_substring",
            signature: "ptr @_mvl_str_substring(ptr, i64, i64)",
            ret_ty: "ptr",
        },
    ),
    (
        "to_upper",
        Dispatch::CCall {
            sym: "_mvl_str_to_upper",
            signature: "ptr @_mvl_str_to_upper(ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "to_lower",
        Dispatch::CCall {
            sym: "_mvl_str_to_lower",
            signature: "ptr @_mvl_str_to_lower(ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "sort",
        Dispatch::CCall {
            sym: "_mvl_list_sort",
            signature: "ptr @_mvl_list_sort(ptr)",
            ret_ty: "ptr",
        },
    ),
    // List::enumerate() → List[Indexed[T]]   elem_size=16: { i64 index, 8-byte value }
    (
        "enumerate",
        Dispatch::CCall {
            sym: "_mvl_list_enumerate",
            signature: "ptr @_mvl_list_enumerate(ptr)",
            ret_ty: "ptr",
        },
    ),
    // List::zip(other) → List[Pair[T, U]]   elem_size=16: { 8-byte first, 8-byte second }
    (
        "zip",
        Dispatch::CCall {
            sym: "_mvl_list_zip",
            signature: "ptr @_mvl_list_zip(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    // Map::entries() → List[Entry[K, V]]   elem_size=16: { ptr key, 8-byte value }
    (
        "entries",
        Dispatch::CCall {
            sym: "_mvl_map_entries",
            signature: "ptr @_mvl_map_entries(ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "partition",
        Dispatch::CCallStructFromSlots {
            sym: "_mvl_list_partition",
            signature: "ptr @_mvl_list_partition(ptr, ptr)",
            struct_name: "%Partitioned",
            slot_tys: &["ptr", "ptr"],
        },
    ),
    (
        "group_by",
        Dispatch::CCall {
            sym: "_mvl_list_group_by",
            signature: "ptr @_mvl_list_group_by(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "windows",
        Dispatch::CCall {
            sym: "_mvl_list_windows",
            signature: "ptr @_mvl_list_windows(ptr, i64)",
            ret_ty: "ptr",
        },
    ),
    (
        "chunks",
        Dispatch::CCall {
            sym: "_mvl_list_chunks",
            signature: "ptr @_mvl_list_chunks(ptr, i64)",
            ret_ty: "ptr",
        },
    ),
    // ── Phase 5: String additional arms ─────────────────────────────────
    (
        "trim",
        Dispatch::CCall {
            sym: "_mvl_str_trim",
            signature: "ptr @_mvl_str_trim(ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "replace",
        Dispatch::CCall {
            sym: "_mvl_str_replace",
            signature: "ptr @_mvl_str_replace(ptr, ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "concat",
        Dispatch::CCall {
            sym: "_mvl_string_concat",
            signature: "ptr @_mvl_string_concat(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    // "list_concat" is a synthetic dispatch key (MVL method is also named "concat"
    // but the receiver is a List/Array — the emit arm guards on receiver kind and
    // routes to this entry to avoid calling _mvl_string_concat on list data).
    (
        "list_concat",
        Dispatch::CCall {
            sym: "_mvl_list_concat",
            signature: "ptr @_mvl_list_concat(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    // ── Phase 5: Map arms ────────────────────────────────────────────────
    (
        "keys",
        Dispatch::CCall {
            sym: "_mvl_map_keys",
            signature: "ptr @_mvl_map_keys(ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "values",
        Dispatch::CCall {
            sym: "_mvl_map_values",
            signature: "ptr @_mvl_map_values(ptr)",
            ret_ty: "ptr",
        },
    ),
    // ── Phase 5: Set algebra arms ────────────────────────────────────────
    (
        "intersection",
        Dispatch::CCall {
            sym: "_mvl_set_intersection",
            signature: "ptr @_mvl_set_intersection(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "difference",
        Dispatch::CCall {
            sym: "_mvl_set_difference",
            signature: "ptr @_mvl_set_difference(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "union",
        Dispatch::CCall {
            sym: "_mvl_set_union",
            signature: "ptr @_mvl_set_union(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    // ── Phase 5: List HOF arms (closure is just a ptr arg — Shape A) ────
    (
        "filter",
        Dispatch::CCall {
            sym: "_mvl_list_filter",
            signature: "ptr @_mvl_list_filter(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "map",
        Dispatch::CCall {
            sym: "_mvl_list_map",
            signature: "ptr @_mvl_list_map(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "take_while",
        Dispatch::CCall {
            sym: "_mvl_list_take_while",
            signature: "ptr @_mvl_list_take_while(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "skip_while",
        Dispatch::CCall {
            sym: "_mvl_list_skip_while",
            signature: "ptr @_mvl_list_skip_while(ptr, ptr)",
            ret_ty: "ptr",
        },
    ),
    (
        "any",
        Dispatch::CCall {
            sym: "_mvl_list_any",
            signature: "i1 @_mvl_list_any(ptr, ptr)",
            ret_ty: "i1",
        },
    ),
    (
        "all",
        Dispatch::CCall {
            sym: "_mvl_list_all",
            signature: "i1 @_mvl_list_all(ptr, ptr)",
            ret_ty: "i1",
        },
    ),
    // ── Shape B: C call returns i64, coerced to i1 via `icmp ne` ────────
    (
        "contains",
        Dispatch::CCallBoolFromI64 {
            sym: "_mvl_str_contains",
            signature: "i64 @_mvl_str_contains(ptr, ptr)",
        },
    ),
    (
        "starts_with",
        Dispatch::CCallBoolFromI64 {
            sym: "_mvl_str_starts_with",
            signature: "i64 @_mvl_str_starts_with(ptr, ptr)",
        },
    ),
    (
        "ends_with",
        Dispatch::CCallBoolFromI64 {
            sym: "_mvl_str_ends_with",
            signature: "i64 @_mvl_str_ends_with(ptr, ptr)",
        },
    ),
];

/// Look up the full `Dispatch` for a method by name.  Returns `None` when
/// the method has no LLVM-backend dispatch row (the emitter handles it inline).
pub fn lookup(name: &str) -> Option<&'static Dispatch> {
    LLVM_DISPATCH
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, d)| d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syms_are_valid_c_identifiers() {
        for (name, d) in LLVM_DISPATCH {
            let s = d.sym();
            assert!(
                s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "sym '{}' for method '{}' is not a valid C identifier",
                s,
                name
            );
        }
    }

    #[test]
    fn no_duplicate_names() {
        use std::collections::HashSet;
        let mut seen: HashSet<&str> = HashSet::new();
        for (name, _) in LLVM_DISPATCH {
            assert!(
                seen.insert(name),
                "duplicate LLVM_DISPATCH entry for method '{}'",
                name
            );
        }
    }

    #[test]
    fn lookup_finds_chars() {
        let d = lookup("chars").expect("chars should be present");
        assert_eq!(d.sym(), "_mvl_string_chars");
    }

    #[test]
    fn lookup_misses_unknown_method() {
        assert!(lookup("totally_made_up_method").is_none());
    }
}
