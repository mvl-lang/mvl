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

use crate::mvl::ir::TirProgram;

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
/// 1. One entry here (with optional `rust_emit` / `llvm_symbol` dispatch hints).
/// 2. One emit arm in each backend's dispatcher — *unless* the method can use
///    table-driven dispatch via `rust_emit` / `llvm_symbol` hints, in which
///    case no hand-written match arm is needed.
///
/// Dispatch hints allow simple methods (runtime-fn calls) to be emitted by a
/// single generic match arm that queries [`rust_emit_by_name`] or similar.
/// Complex methods (type-aware, higher-order, etc.) leave hints as `None` and
/// require hand-written emit arms in each backend.
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
    /// Rust backend dispatch hint: the runtime function name used for
    /// `fn_name(receiver.clone().into(), args)` dispatch.
    /// `None` means the backend uses a hand-written match arm or passthrough.
    pub rust_emit: Option<&'static str>,
    /// LLVM backend dispatch hint: C-ABI symbol for `ensure_extern + call` dispatch.
    /// `None` means the backend uses a hand-written match arm or the method is not
    /// yet implemented in the LLVM backend.
    pub llvm_symbol: Option<&'static str>,
}

impl BuiltinDesc {
    pub const fn method(name: &'static str, ty: &'static str, min: usize, max: usize) -> Self {
        Self {
            name,
            receiver: Receiver::Type(ty),
            min_args: min,
            max_args: max,
            rust_emit: None,
            llvm_symbol: None,
        }
    }

    pub const fn method_with(
        name: &'static str,
        ty: &'static str,
        min: usize,
        max: usize,
        rust_emit: Option<&'static str>,
        llvm_symbol: Option<&'static str>,
    ) -> Self {
        Self {
            name,
            receiver: Receiver::Type(ty),
            min_args: min,
            max_args: max,
            rust_emit,
            llvm_symbol,
        }
    }

    pub const fn free(name: &'static str, module: &'static str, min: usize, max: usize) -> Self {
        Self {
            name,
            receiver: Receiver::Free(module),
            min_args: min,
            max_args: max,
            rust_emit: None,
            llvm_symbol: None,
        }
    }

    pub fn is_method(&self) -> bool {
        matches!(self.receiver, Receiver::Type(_))
    }

    pub fn is_free_fn(&self) -> bool {
        matches!(self.receiver, Receiver::Free(_))
    }
}

/// Look up the Rust runtime function name for a builtin method on a given type.
///
/// Returns `Some("fn_name")` when the method has a `rust_emit` dispatch hint,
/// meaning the backend should emit `fn_name(receiver.clone().into(), args)`.
/// Returns `None` for methods that require hand-written dispatch logic.
pub fn rust_emit_for(name: &str, ty: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|d| {
            d.name == name
                && matches!(&d.receiver, Receiver::Type(t) if *t == ty)
                && d.rust_emit.is_some()
        })
        .and_then(|d| d.rust_emit)
}

/// Look up the Rust runtime function name for a builtin method by name only.
///
/// Unlike [`rust_emit_for`], this does not require knowing the receiver type.
/// It returns the first match across all receiver types.  This is safe when
/// the method name is unambiguous (only one receiver type has a `rust_emit`
/// hint for it), which is the case for all current entries.
pub fn rust_emit_by_name(name: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|d| d.name == name && d.rust_emit.is_some())
        .and_then(|d| d.rust_emit)
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
    // to_upper/to_lower: now `builtin fn`, dispatched via BUILTINS rust_emit hints
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
    // std/text.mvl — String extension methods returning List[Span] (#1371)
    "split_spans",
    "find_all_spans",
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
    BuiltinDesc::method_with(
        "chars",
        "String",
        0,
        0,
        Some("str_chars"),
        Some("mvl_string_chars"),
    ),
    BuiltinDesc::method_with(
        "char_at",
        "String",
        1,
        1,
        Some("str_char_at"),
        Some("_mvl_str_char_at"),
    ),
    BuiltinDesc::method_with(
        "byte_at",
        "String",
        1,
        1,
        Some("str_byte_at"),
        Some("_mvl_str_byte_at"),
    ),
    BuiltinDesc::method("concat", "String", 1, 1),
    BuiltinDesc::method_with(
        "find",
        "String",
        1,
        1,
        Some("str_find"),
        Some("_mvl_str_find"),
    ),
    BuiltinDesc::method_with(
        "split",
        "String",
        1,
        1,
        Some("str_split"),
        Some("_mvl_str_split"),
    ),
    BuiltinDesc::method_with(
        "substring",
        "String",
        2,
        2,
        Some("str_substring"),
        Some("_mvl_str_substring"),
    ),
    BuiltinDesc::method_with("parse_int", "String", 0, 0, Some("str_parse_int"), None),
    BuiltinDesc::method_with("parse_float", "String", 0, 0, Some("str_parse_float"), None),
    // String — Unicode-aware case conversion (builtin fn, #1267)
    BuiltinDesc::method_with(
        "to_upper",
        "String",
        0,
        0,
        Some("str_to_upper"),
        Some("_mvl_str_to_upper"),
    ),
    BuiltinDesc::method_with(
        "to_lower",
        "String",
        0,
        0,
        Some("str_to_lower"),
        Some("_mvl_str_to_lower"),
    ),
    // String — compiler intrinsics (both backends emit explicitly)
    BuiltinDesc::method("contains", "String", 1, 1),
    BuiltinDesc::method("is_empty", "String", 0, 0),
    BuiltinDesc::method("to_string", "String", 0, 0),
    // ── List — kernel builtins (std/lists.mvl `pub builtin fn`) ──────────────
    BuiltinDesc::method("len", "List", 0, 0),
    BuiltinDesc::method_with("get", "List", 1, 1, Some("list_get"), None),
    BuiltinDesc::method("push", "List", 1, 1),
    BuiltinDesc::method("set", "List", 2, 2),
    BuiltinDesc::method_with("slice", "List", 2, 2, Some("list_slice"), None),
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
    // List — category-D builtins: explicit emitter arms in both backends (#1290)
    BuiltinDesc::method_with("sort", "List", 0, 0, None, Some("_mvl_list_sort")),
    BuiltinDesc::method_with("partition", "List", 1, 1, None, Some("_mvl_list_partition")),
    BuiltinDesc::method_with("group_by", "List", 1, 1, None, Some("_mvl_list_group_by")),
    BuiltinDesc::method_with("windows", "List", 1, 1, None, Some("_mvl_list_windows")),
    BuiltinDesc::method_with("chunks", "List", 1, 1, None, Some("_mvl_list_chunks")),
    // ── Map — compiler intrinsics ─────────────────────────────────────────────
    BuiltinDesc::method("get", "Map", 1, 1),
    BuiltinDesc::method("insert", "Map", 2, 2),
    BuiltinDesc::method("contains_key", "Map", 1, 1),
    BuiltinDesc::method("keys", "Map", 0, 0),
    BuiltinDesc::method("values", "Map", 0, 0),
    BuiltinDesc::method("remove", "Map", 1, 1),
    BuiltinDesc::method("is_empty", "Map", 0, 0),
    BuiltinDesc::method("len", "Map", 0, 0),
    // Map — higher-order methods (pure MVL bodies; both backends emit explicitly)
    BuiltinDesc::method("map_values", "Map", 1, 1),
    BuiltinDesc::method("filter", "Map", 1, 1),
    BuiltinDesc::method("fold", "Map", 2, 2),
    BuiltinDesc::method("any", "Map", 1, 1),
    BuiltinDesc::method("all", "Map", 1, 1),
    // ── Set — compiler intrinsics ─────────────────────────────────────────────
    BuiltinDesc::method("insert", "Set", 1, 1),
    BuiltinDesc::method("contains", "Set", 1, 1),
    BuiltinDesc::method("is_empty", "Set", 0, 0),
    BuiltinDesc::method("len", "Set", 0, 0),
    BuiltinDesc::method("to_list", "Set", 0, 0),
    BuiltinDesc::method("union", "Set", 1, 1),
    BuiltinDesc::method("intersection", "Set", 1, 1),
    BuiltinDesc::method("difference", "Set", 1, 1),
    // Set — higher-order methods (pure MVL bodies; both backends emit explicitly)
    BuiltinDesc::method("map", "Set", 1, 1),
    BuiltinDesc::method("filter", "Set", 1, 1),
    BuiltinDesc::method("fold", "Set", 2, 2),
    BuiltinDesc::method("any", "Set", 1, 1),
    BuiltinDesc::method("all", "Set", 1, 1),
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

    #[test]
    fn rust_emit_hints_are_valid_identifiers() {
        for d in BUILTINS {
            if let Some(hint) = d.rust_emit {
                assert!(
                    hint.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                    "rust_emit '{}' for {} is not a valid Rust identifier",
                    hint,
                    d.name
                );
            }
        }
    }

    #[test]
    fn llvm_symbol_hints_are_valid_identifiers() {
        for d in BUILTINS {
            if let Some(sym) = d.llvm_symbol {
                assert!(
                    sym.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                    "llvm_symbol '{}' for {} is not a valid C identifier",
                    sym,
                    d.name
                );
            }
        }
    }

    #[test]
    fn rust_emit_by_name_returns_expected_values() {
        assert_eq!(rust_emit_by_name("chars"), Some("str_chars"));
        assert_eq!(rust_emit_by_name("find"), Some("str_find"));
        assert_eq!(rust_emit_by_name("split"), Some("str_split"));
        assert_eq!(rust_emit_by_name("substring"), Some("str_substring"));
        assert_eq!(rust_emit_by_name("parse_int"), Some("str_parse_int"));
        assert_eq!(rust_emit_by_name("parse_float"), Some("str_parse_float"));
        assert_eq!(rust_emit_by_name("char_at"), Some("str_char_at"));
        assert_eq!(rust_emit_by_name("byte_at"), Some("str_byte_at"));
        assert_eq!(rust_emit_by_name("slice"), Some("list_slice"));
        assert_eq!(rust_emit_by_name("get"), Some("list_get"));
        // Methods without hints return None.
        assert_eq!(rust_emit_by_name("len"), None);
        assert_eq!(rust_emit_by_name("contains"), None);
        assert_eq!(rust_emit_by_name("map"), None);
    }

    #[test]
    fn rust_emit_for_type_specific() {
        assert_eq!(rust_emit_for("chars", "String"), Some("str_chars"));
        assert_eq!(rust_emit_for("get", "List"), Some("list_get"));
        // Same method name on a different type returns None.
        assert_eq!(rust_emit_for("get", "Map"), None);
    }

    #[test]
    fn no_conflicting_rust_emit_hints() {
        // Ensure no two entries with the same name have different rust_emit hints.
        // (Same name on different types with hints would cause rust_emit_by_name ambiguity.)
        let with_hints: Vec<_> = BUILTINS.iter().filter(|d| d.rust_emit.is_some()).collect();
        let mut seen: HashSet<&str> = HashSet::new();
        for d in &with_hints {
            if seen.contains(d.name) {
                let others: Vec<_> = with_hints.iter().filter(|o| o.name == d.name).collect();
                panic!(
                    "method '{}' has rust_emit hints on multiple types: {:?}",
                    d.name,
                    others.iter().map(|o| &o.receiver).collect::<Vec<_>>()
                );
            }
            seen.insert(d.name);
        }
    }
}

/// Common interface shared by all MVL code-generation backends.
///
/// Each backend receives a fully-typed [`TirProgram`] (post-checker,
/// post-monomorphization, post-TIR-lowering) and produces output that can be
/// compiled or executed.  The caller is responsible for running the analysis
/// pipeline (`checker → mono → lower`) before invoking the backend.
///
/// The trait is intentionally minimal; specialised functionality (coverage,
/// MC/DC, mutation) lives on concrete backend types and is called directly
/// from `src/main.rs`.
pub trait Backend {
    /// Human-readable backend identifier (matches the `--backend=` flag value).
    fn name(&self) -> &'static str;

    /// File extension for generated source files (without leading dot).
    fn file_extension(&self) -> &'static str;

    /// Emit a single-file program to a source string.
    ///
    /// `crate_name` is used as the Rust crate/module name for the Rust backend
    /// and ignored by the LLVM backend.
    fn emit_program(&self, tir: &TirProgram, crate_name: &str) -> String;
}
