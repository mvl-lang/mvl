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
///
/// Future variants (deferred to later #1399 phases) will cover Shape B
/// (bool-from-i64 coercion), Shape C (Option-via-out-ptr), Shape D (struct
/// assembly), and Shape E (HOF closures).
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
}

impl Dispatch {
    /// Returns the C-ABI symbol if this dispatch has one.
    pub const fn sym(&self) -> &'static str {
        match self {
            Dispatch::CCall { sym, .. } => sym,
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
        Dispatch::CCall {
            sym: "_mvl_str_char_at",
            signature: "i8 @_mvl_str_char_at(ptr, i64, ptr)",
            ret_ty: "i8",
        },
    ),
    (
        "byte_at",
        Dispatch::CCall {
            sym: "_mvl_str_byte_at",
            signature: "i8 @_mvl_str_byte_at(ptr, i64, ptr)",
            ret_ty: "i8",
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
    (
        "partition",
        Dispatch::CCall {
            sym: "_mvl_list_partition",
            signature: "ptr @_mvl_list_partition(ptr, ptr)",
            ret_ty: "ptr",
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
];

/// Look up the full `Dispatch` for a method by name.  Returns `None` when
/// the method has no LLVM-backend dispatch row (the emitter handles it inline).
pub fn lookup(name: &str) -> Option<&'static Dispatch> {
    LLVM_DISPATCH
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, d)| d)
}

/// Convenience: look up just the C-ABI symbol for a method.
pub fn sym(name: &str) -> Option<&'static str> {
    lookup(name).map(|d| d.sym())
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
