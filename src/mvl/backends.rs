// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Backend dispatch for MVL — defines the [`Backend`] trait and sub-modules
//! for each concrete backend.
//!
//! # Extension points
//!
//! To add a new backend:
//! 1. Add a sub-module (e.g. `pub mod wasm;`).
//! 2. Implement the `Backend` trait for your emitter type.
//! 3. Wire it up in `src/main.rs` via `parse_backend`.

pub mod llvm_text;
pub mod rust;

use crate::mvl::parser::ast::Program;

/// Controls how struct invariants and refinement conditions are enforced at runtime.
///
/// Both the Rust and LLVM backends respect this setting for parity (issue #662).
///
/// | Mode        | Rust backend      | LLVM backend                        |
/// |-------------|-------------------|-------------------------------------|
/// | `Always`    | `assert!`         | `icmp + llvm.trap()`                |
/// | `DebugOnly` | `debug_assert!`   | conditional on `debug_assertions`   |
/// | `Assume`    | omit check        | `llvm.assume()` (optimizer hint)    |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssertMode {
    /// Enforce invariants unconditionally in all build modes (default).
    #[default]
    Always,
    /// Enforce invariants only in debug builds; elided in release.
    DebugOnly,
    /// Omit runtime check; emit as optimizer hint (`llvm.assume`) where supported.
    Assume,
}

impl AssertMode {
    /// Parse from a CLI flag value (e.g. `--assert-mode=debug-only`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "always" => Some(AssertMode::Always),
            "debug-only" => Some(AssertMode::DebugOnly),
            "assume" => Some(AssertMode::Assume),
            _ => None,
        }
    }
}

// ── Builtin registry ──────────────────────────────────────────────────────────

/// Where a builtin lives: a method on a concrete type, or a free stdlib function
/// (grouped by module).
///
/// Both backends share this enum so they can filter [`BUILTINS`] by dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Receiver {
    /// Instance method on a named type, e.g. `Type("String")` → `String::len`.
    Type(&'static str),
    /// Free stdlib function; the field is the module name, e.g. `Free("crypto")`.
    Free(&'static str),
}

/// Descriptor for one builtin method or free stdlib function.
///
/// [`BUILTINS`] is the single source of truth that both backends use to:
///
/// 1. **Enforce completeness** — the `_` fallthrough in each backend's method/fn
///    dispatcher calls [`is_stdlib_method`] / [`is_stdlib_fn`] in debug builds.
///    Reaching the fallthrough for a known builtin is a backend bug.
/// 2. **Drive parity tests** — cross-backend tests iterate the table to verify
///    both backends handle every entry identically.
///
/// Adding a new builtin requires:
/// 1. One entry here.
/// 2. One emit arm in each backend's dispatcher.
///
/// C-ABI calling-convention details (`StdlibSig` in the LLVM backend) stay
/// backend-local — `BuiltinDesc` captures *identity*, not *calling convention*.
#[derive(Debug, Clone)]
pub struct BuiltinDesc {
    /// Method or function name (without receiver prefix).
    pub name: &'static str,
    /// Whether this is a type method or free function, and on/in which type/module.
    pub receiver: Receiver,
    /// Minimum argument count, excluding `self` for methods.
    pub min_args: usize,
    /// Maximum argument count, excluding `self` for methods.
    pub max_args: usize,
}

impl BuiltinDesc {
    pub const fn method(name: &'static str, ty: &'static str, min: usize, max: usize) -> Self {
        Self {
            name,
            receiver: Receiver::Type(ty),
            min_args: min,
            max_args: max,
        }
    }

    pub const fn free(name: &'static str, module: &'static str, min: usize, max: usize) -> Self {
        Self {
            name,
            receiver: Receiver::Free(module),
            min_args: min,
            max_args: max,
        }
    }

    pub fn is_method(&self) -> bool {
        matches!(self.receiver, Receiver::Type(_))
    }

    pub fn is_free_fn(&self) -> bool {
        matches!(self.receiver, Receiver::Free(_))
    }
}

/// Returns `true` if `name` is a known stdlib method on any type.
///
/// Called by backend `_` dispatch arms (debug builds only) to detect missing
/// implementations before falling through to user-defined method emission.
pub fn is_stdlib_method(name: &str) -> bool {
    BUILTINS.iter().any(|d| d.is_method() && d.name == name)
}

/// Returns `true` if `name` is a known stdlib free function in any module.
pub fn is_stdlib_fn(name: &str) -> bool {
    BUILTINS.iter().any(|d| d.is_free_fn() && d.name == name)
}

// ── Shared dispatch constants (Rust backend) ────────────────────────────────
//
// These constants are used by both the TIR and AST expression emitters in the
// Rust backend.  Centralised here to eliminate duplication between
// `emit_exprs.rs` and `emit_exprs_ast.rs`.

/// Pure MVL stdlib methods — transpiled as free functions, dispatched via UFCS
/// as `method(receiver.clone().into(), args)`.
///
/// When the transpiler sees `receiver.method(args)` for one of these names it
/// emits a UFCS free-function call instead: `method(receiver.clone().into(), args)`.
/// The `.into()` coercion allows IFC-label wrapper types (`Clean<String>`, etc.) to
/// flow into functions that take the plain inner type — `From<Label<T>> for T` is
/// implemented in `mvl_runtime::ifc`.
///
/// # 4-way sync (#992)
///
/// This list is one of **four** places that must stay in sync when adding a new builtin
/// method.  See `checker/method_types.rs` for the authoritative list and full explanation.
/// The planned fix (method desugaring) is tracked in issue #992.
pub(crate) const STDLIB_UFCS_METHODS: &[&str] = &[
    // std/strings.mvl (pure MVL, have bodies)
    "trim",
    "to_upper",
    "to_lower",
    "starts_with",
    "ends_with",
    "replace",
    // Note: `contains` and `is_empty` have hardcoded type-aware handlers above.
    // std/lists.mvl (pure MVL, have bodies)
    "take",
    "skip",
    "first",
    "last",
    "flatten",
    "reverse",
];

/// Builtin stdlib methods that are unambiguous (only one receiver type).
/// Dispatched as `runtime_fn(receiver.clone().into(), args)`.
pub(crate) const STDLIB_BUILTIN_METHODS: &[(&str, &str)] = &[
    // String-only methods
    ("chars", "str_chars"),
    ("find", "str_find"),
    ("split", "str_split"),
    ("substring", "str_substring"),
    ("parse_int", "str_parse_int"),
    ("parse_float", "str_parse_float"),
    ("char_at", "str_char_at"),
    ("byte_at", "str_byte_at"),
    // List-only methods
    ("slice", "list_slice"),
    ("get", "list_get"),
];

/// String methods that return a `String` with the same IFC label as their receiver.
/// When the receiver is `Label<String>`, the call result must be re-wrapped in `Label::new(…)`
/// because the UFCS trampoline (`method(receiver.clone().into(), …)`) strips the label via
/// `.into()` before passing to the stdlib function (which returns plain `String`).
pub(crate) const STRING_LABEL_PRESERVING_METHODS: &[&str] = &[
    "trim",
    "to_upper",
    "to_lower",
    "concat",
    "replace",
    "substring",
];

/// Shared registry of all MVL builtin methods and stdlib free functions.
///
/// **Scope:** methods that require explicit backend emission logic (kernel
/// builtins and compiler intrinsics).  Pure-MVL UFCS methods (e.g. `trim`,
/// `starts_with`, `flatten`) have MVL bodies and are compiled transparently —
/// they are intentionally absent.
///
/// **Organisation:**
/// - Type methods: String, List, Map, Set (collections); Int, Float, Bool, Byte
///   (primitives); Option, Result (algebraic).
/// - Free functions: grouped by stdlib module (`crypto`, `random`, `env`,
///   `time`, `io`, `net`, `regex`).
pub const BUILTINS: &[BuiltinDesc] = &[
    // ── String — kernel builtins (std/strings.mvl `pub builtin fn`) ──────────
    BuiltinDesc::method("len", "String", 0, 0),
    BuiltinDesc::method("chars", "String", 0, 0),
    BuiltinDesc::method("char_at", "String", 1, 1),
    BuiltinDesc::method("byte_at", "String", 1, 1),
    BuiltinDesc::method("concat", "String", 1, 1),
    BuiltinDesc::method("find", "String", 1, 1),
    BuiltinDesc::method("split", "String", 1, 1),
    BuiltinDesc::method("substring", "String", 2, 2),
    BuiltinDesc::method("parse_int", "String", 0, 0),
    BuiltinDesc::method("parse_float", "String", 0, 0),
    // String — compiler intrinsics (both backends emit explicitly)
    BuiltinDesc::method("contains", "String", 1, 1),
    BuiltinDesc::method("is_empty", "String", 0, 0),
    BuiltinDesc::method("to_string", "String", 0, 0),
    // ── List — kernel builtins (std/lists.mvl `pub builtin fn`) ──────────────
    BuiltinDesc::method("len", "List", 0, 0),
    BuiltinDesc::method("get", "List", 1, 1),
    BuiltinDesc::method("push", "List", 1, 1),
    BuiltinDesc::method("slice", "List", 2, 2),
    BuiltinDesc::method("concat", "List", 1, 1),
    BuiltinDesc::method("contains", "List", 1, 1),
    // List — compiler intrinsics
    BuiltinDesc::method("is_empty", "List", 0, 0),
    BuiltinDesc::method("first", "List", 0, 0),
    // List — higher-order methods (pure MVL bodies; both backends emit explicitly)
    BuiltinDesc::method("map", "List", 1, 1),
    BuiltinDesc::method("filter", "List", 1, 1),
    BuiltinDesc::method("fold", "List", 2, 2),
    BuiltinDesc::method("any", "List", 1, 1),
    BuiltinDesc::method("all", "List", 1, 1),
    BuiltinDesc::method("find", "List", 1, 1),
    BuiltinDesc::method("take_while", "List", 1, 1),
    BuiltinDesc::method("skip_while", "List", 1, 1),
    // ── Map — compiler intrinsics ─────────────────────────────────────────────
    BuiltinDesc::method("get", "Map", 1, 1),
    BuiltinDesc::method("insert", "Map", 2, 2),
    BuiltinDesc::method("contains_key", "Map", 1, 1),
    BuiltinDesc::method("keys", "Map", 0, 0),
    BuiltinDesc::method("values", "Map", 0, 0),
    BuiltinDesc::method("remove", "Map", 1, 1),
    BuiltinDesc::method("is_empty", "Map", 0, 0),
    BuiltinDesc::method("len", "Map", 0, 0),
    // ── Set — compiler intrinsics ─────────────────────────────────────────────
    BuiltinDesc::method("insert", "Set", 1, 1),
    BuiltinDesc::method("contains", "Set", 1, 1),
    BuiltinDesc::method("is_empty", "Set", 0, 0),
    BuiltinDesc::method("len", "Set", 0, 0),
    BuiltinDesc::method("to_list", "Set", 0, 0),
    BuiltinDesc::method("union", "Set", 1, 1),
    BuiltinDesc::method("intersection", "Set", 1, 1),
    BuiltinDesc::method("difference", "Set", 1, 1),
    // ── Int — compiler intrinsics ─────────────────────────────────────────────
    BuiltinDesc::method("abs", "Int", 0, 0),
    BuiltinDesc::method("pow", "Int", 1, 1),
    BuiltinDesc::method("min", "Int", 1, 1),
    BuiltinDesc::method("max", "Int", 1, 1),
    BuiltinDesc::method("clamp", "Int", 2, 2),
    BuiltinDesc::method("to_float", "Int", 0, 0),
    BuiltinDesc::method("to_string", "Int", 0, 0),
    BuiltinDesc::method("is_positive", "Int", 0, 0),
    BuiltinDesc::method("is_negative", "Int", 0, 0),
    BuiltinDesc::method("is_zero", "Int", 0, 0),
    BuiltinDesc::method("bit_and", "Int", 1, 1),
    BuiltinDesc::method("bit_or", "Int", 1, 1),
    BuiltinDesc::method("bit_xor", "Int", 1, 1),
    BuiltinDesc::method("bit_not", "Int", 0, 0),
    BuiltinDesc::method("shift_left", "Int", 1, 1),
    BuiltinDesc::method("shift_right", "Int", 1, 1),
    BuiltinDesc::method("checked_add", "Int", 1, 1),
    BuiltinDesc::method("checked_sub", "Int", 1, 1),
    BuiltinDesc::method("checked_mul", "Int", 1, 1),
    BuiltinDesc::method("checked_div", "Int", 1, 1),
    BuiltinDesc::method("wrapping_add", "Int", 1, 1),
    BuiltinDesc::method("wrapping_sub", "Int", 1, 1),
    BuiltinDesc::method("wrapping_mul", "Int", 1, 1),
    // ── Float — compiler intrinsics ───────────────────────────────────────────
    BuiltinDesc::method("abs", "Float", 0, 0),
    BuiltinDesc::method("ceil", "Float", 0, 0),
    BuiltinDesc::method("floor", "Float", 0, 0),
    BuiltinDesc::method("round", "Float", 0, 0),
    BuiltinDesc::method("sqrt", "Float", 0, 0),
    BuiltinDesc::method("pow", "Float", 1, 1),
    BuiltinDesc::method("min", "Float", 1, 1),
    BuiltinDesc::method("max", "Float", 1, 1),
    BuiltinDesc::method("clamp", "Float", 2, 2),
    BuiltinDesc::method("to_int", "Float", 0, 0),
    BuiltinDesc::method("to_string", "Float", 0, 0),
    BuiltinDesc::method("is_nan", "Float", 0, 0),
    BuiltinDesc::method("is_infinite", "Float", 0, 0),
    BuiltinDesc::method("is_finite", "Float", 0, 0),
    BuiltinDesc::method("is_positive", "Float", 0, 0),
    BuiltinDesc::method("is_negative", "Float", 0, 0),
    // ── Bool — compiler intrinsics ────────────────────────────────────────────
    BuiltinDesc::method("to_string", "Bool", 0, 0),
    // ── Byte — compiler intrinsics ────────────────────────────────────────────
    BuiltinDesc::method("to_int", "Byte", 0, 0),
    BuiltinDesc::method("to_string", "Byte", 0, 0),
    BuiltinDesc::method("bit_and", "Byte", 1, 1),
    BuiltinDesc::method("bit_or", "Byte", 1, 1),
    BuiltinDesc::method("bit_xor", "Byte", 1, 1),
    BuiltinDesc::method("bit_not", "Byte", 0, 0),
    BuiltinDesc::method("shift_left", "Byte", 1, 1),
    BuiltinDesc::method("shift_right", "Byte", 1, 1),
    BuiltinDesc::method("wrapping_add", "Byte", 1, 1),
    BuiltinDesc::method("wrapping_sub", "Byte", 1, 1),
    BuiltinDesc::method("wrapping_mul", "Byte", 1, 1),
    BuiltinDesc::method("checked_add", "Byte", 1, 1),
    BuiltinDesc::method("checked_sub", "Byte", 1, 1),
    BuiltinDesc::method("checked_mul", "Byte", 1, 1),
    // ── Option — compiler intrinsics ──────────────────────────────────────────
    BuiltinDesc::method("is_some", "Option", 0, 0),
    BuiltinDesc::method("is_none", "Option", 0, 0),
    BuiltinDesc::method("unwrap_or", "Option", 1, 1),
    BuiltinDesc::method("map", "Option", 1, 1),
    BuiltinDesc::method("and_then", "Option", 1, 1),
    // ── Result — compiler intrinsics ──────────────────────────────────────────
    BuiltinDesc::method("is_ok", "Result", 0, 0),
    BuiltinDesc::method("is_err", "Result", 0, 0),
    BuiltinDesc::method("unwrap_or", "Result", 1, 1),
    BuiltinDesc::method("map", "Result", 1, 1),
    BuiltinDesc::method("and_then", "Result", 1, 1),
    // ── Free: crypto (std/crypto.mvl) ─────────────────────────────────────────
    BuiltinDesc::free("sha256", "crypto", 1, 1),
    BuiltinDesc::free("sha512", "crypto", 1, 1),
    BuiltinDesc::free("crypto_random_bytes", "crypto", 1, 1),
    // ── Free: random (std/random.mvl) ─────────────────────────────────────────
    BuiltinDesc::free("int", "random", 2, 2),
    BuiltinDesc::free("float", "random", 0, 0),
    BuiltinDesc::free("bytes", "random", 1, 1),
    BuiltinDesc::free("choice", "random", 1, 1),
    BuiltinDesc::free("shuffle", "random", 1, 1),
    // ── Free: time (std/time.mvl) ─────────────────────────────────────────────
    BuiltinDesc::free("now", "time", 0, 0),
    // ── Free: env (std/env.mvl) ───────────────────────────────────────────────
    BuiltinDesc::free("get", "env", 1, 1),
    BuiltinDesc::free("set", "env", 2, 2),
    BuiltinDesc::free("remove_var", "env", 1, 1),
    BuiltinDesc::free("all", "env", 0, 0),
    BuiltinDesc::free("args", "env", 0, 0),
    BuiltinDesc::free("exit", "env", 1, 1),
    BuiltinDesc::free("current_dir", "env", 0, 0),
    BuiltinDesc::free("chdir", "env", 1, 1),
    BuiltinDesc::free("getuid", "env", 0, 0),
    BuiltinDesc::free("getgid", "env", 0, 0),
    BuiltinDesc::free("signal_on", "env", 2, 2),
    BuiltinDesc::free("signal_reset", "env", 1, 1),
    BuiltinDesc::free("signal_ignore", "env", 1, 1),
    // ── Free: io (std/io.mvl) ─────────────────────────────────────────────────
    BuiltinDesc::free("stdout", "io", 0, 0),
    BuiltinDesc::free("stderr", "io", 0, 0),
    BuiltinDesc::free("stdin", "io", 0, 0),
    BuiltinDesc::free("path_exists", "io", 1, 1),
    BuiltinDesc::free("is_file", "io", 1, 1),
    BuiltinDesc::free("is_dir", "io", 1, 1),
    BuiltinDesc::free("open", "io", 1, 1),
    BuiltinDesc::free("close", "io", 1, 1),
    BuiltinDesc::free("write", "io", 2, 2),
    BuiltinDesc::free("read", "io", 2, 2),
    BuiltinDesc::free("read_line", "io", 1, 1),
    BuiltinDesc::free("read_to_string", "io", 1, 1),
    BuiltinDesc::free("read_file", "io", 1, 1),
    BuiltinDesc::free("write_file", "io", 2, 2),
    BuiltinDesc::free("append", "io", 2, 2),
    BuiltinDesc::free("create_dir_all", "io", 1, 1),
    BuiltinDesc::free("remove", "io", 1, 1),
    BuiltinDesc::free("read_dir", "io", 1, 1),
    BuiltinDesc::free("metadata", "io", 1, 1),
    // ── Free: net (std/net.mvl) ───────────────────────────────────────────────
    BuiltinDesc::free("tcp_listen", "net", 2, 2),
    BuiltinDesc::free("tcp_connect", "net", 2, 2),
    BuiltinDesc::free("tcp_accept", "net", 1, 1),
    BuiltinDesc::free("tcp_read", "net", 1, 1),
    BuiltinDesc::free("tcp_read_request", "net", 1, 1),
    BuiltinDesc::free("tcp_read_exact", "net", 2, 2),
    BuiltinDesc::free("tcp_shutdown_write", "net", 1, 1),
    BuiltinDesc::free("tcp_write", "net", 2, 2),
    BuiltinDesc::free("tcp_listener_port", "net", 1, 1),
    BuiltinDesc::free("tcp_close_listener", "net", 1, 1),
    BuiltinDesc::free("tcp_close_stream", "net", 1, 1),
    BuiltinDesc::free("http_request_path", "net", 1, 1),
    // ── Free: regex (std/regex.mvl) ───────────────────────────────────────────
    BuiltinDesc::free("compile", "regex", 1, 1),
    BuiltinDesc::free("find", "regex", 2, 2),
    BuiltinDesc::free("captures", "regex", 2, 2),
];

#[cfg(test)]
mod registry_tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_collection_types_present() {
        let types: HashSet<&str> = BUILTINS
            .iter()
            .filter_map(|d| {
                if let Receiver::Type(t) = &d.receiver {
                    Some(*t)
                } else {
                    None
                }
            })
            .collect();
        for ty in &[
            "String", "List", "Map", "Set", "Int", "Float", "Bool", "Byte", "Option", "Result",
        ] {
            assert!(
                types.contains(*ty),
                "type '{ty}' missing from BUILTINS registry"
            );
        }
    }

    #[test]
    fn all_stdlib_modules_present() {
        let modules: HashSet<&str> = BUILTINS
            .iter()
            .filter_map(|d| {
                if let Receiver::Free(m) = &d.receiver {
                    Some(*m)
                } else {
                    None
                }
            })
            .collect();
        for module in &["crypto", "random", "time", "env", "io", "net", "regex"] {
            assert!(
                modules.contains(*module),
                "module '{module}' missing from BUILTINS registry"
            );
        }
    }

    #[test]
    fn no_duplicate_entries() {
        let mut seen: HashSet<(&str, &str)> = HashSet::new();
        for d in BUILTINS {
            let key = match &d.receiver {
                Receiver::Type(t) => (d.name, *t),
                Receiver::Free(m) => (d.name, *m),
            };
            assert!(
                seen.insert(key),
                "duplicate BUILTINS entry: ({}, {})",
                key.0,
                key.1
            );
        }
    }
}

/// Common interface shared by all MVL code-generation backends.
///
/// Each backend receives a checked program (plus any prelude programs) and
/// produces output that can be compiled or executed.  The trait is intentionally
/// minimal; specialised functionality (coverage, MC/DC, mutation) lives on the
/// concrete backend types and is called directly from `src/main.rs`.
pub trait Backend {
    /// Human-readable backend identifier (matches the `--backend=` flag value).
    fn name(&self) -> &'static str;

    /// File extension for generated source files (without leading dot).
    fn file_extension(&self) -> &'static str;

    /// Emit a single-file program to a source string.
    ///
    /// `crate_name` is used as the Rust crate/module name for the Rust backend
    /// and ignored by the LLVM backend.
    fn emit_program(&self, prog: &Program, crate_name: &str) -> String;
}
