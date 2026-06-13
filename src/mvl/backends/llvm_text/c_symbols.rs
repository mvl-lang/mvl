// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI symbol derivation for the LLVM-text backend.
//!
//! Maps MVL stdlib `builtin fn` declarations to the C runtime symbols
//! defined in `runtime/llvm/` and `runtime/rust/`. This is a backend
//! concern; the loader calls into it to pre-populate the builtin symbol
//! table consumed by [`crate::mvl::backends::llvm_text::LlvmTextCompiler`].
//!
//! Naming convention: see ADR-0041 and the LLVM Backend section of
//! `CLAUDE.md` — IR uses unprefixed `@mvl_*` declarations; the C compiler
//! adds platform prefixes (`_` on Darwin, none on Linux). The strings
//! returned here include the leading underscore because they are stored
//! as the literal C symbol name handed to the runtime linker, not as
//! LLVM IR identifiers.

/// Derive the C-ABI symbol name for a `builtin fn` in a given stdlib module.
///
/// For extension methods (e.g. `pub builtin fn String::from_chars`), the
/// receiver type prefix is translated to a short module prefix
/// (`String` → `str`, `List` → `list`, etc.) to match the runtime naming
/// convention. Module-level builtins delegate to [`derive_c_abi_symbol`].
pub fn derive_builtin_c_symbol(
    module: &str,
    receiver_type: &Option<String>,
    fn_name: &str,
) -> String {
    if let Some(recv) = receiver_type {
        let prefix = match recv.as_str() {
            "String" => "str",
            "List" => "list",
            "Map" => "map",
            "Set" => "set",
            "Option" => "option",
            "Result" => "result",
            other => other,
        };
        return format!("_mvl_{prefix}_{fn_name}");
    }
    derive_c_abi_symbol(module, fn_name)
}

/// Derive the C-ABI symbol name for a module-level `builtin fn`.
pub fn derive_c_abi_symbol(module: &str, fn_name: &str) -> String {
    match (module, fn_name) {
        ("time", "sleep") => "_mvl_time_thread_sleep".to_string(),
        ("log", _) => format!("_mvl_{fn_name}"),
        ("crypto", _) if fn_name.starts_with("crypto_") => format!("_mvl_{fn_name}"),
        ("crypto", _) if fn_name.starts_with('_') => {
            format!("_mvl_crypto_{}", &fn_name[1..])
        }
        _ => format!("_mvl_{module}_{fn_name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_method_uses_short_prefix() {
        let s = derive_builtin_c_symbol("strings", &Some("String".to_string()), "from_chars");
        assert_eq!(s, "_mvl_str_from_chars");
    }

    #[test]
    fn list_extension_method() {
        let s = derive_builtin_c_symbol("lists", &Some("List".to_string()), "sort");
        assert_eq!(s, "_mvl_list_sort");
    }

    #[test]
    fn module_level_default() {
        assert_eq!(
            derive_c_abi_symbol("net", "tcp_connect"),
            "_mvl_net_tcp_connect"
        );
    }

    #[test]
    fn time_sleep_special_case() {
        assert_eq!(
            derive_c_abi_symbol("time", "sleep"),
            "_mvl_time_thread_sleep"
        );
    }

    #[test]
    fn log_module_drops_module_prefix() {
        assert_eq!(derive_c_abi_symbol("log", "log_info"), "_mvl_log_info");
    }

    #[test]
    fn crypto_already_prefixed() {
        assert_eq!(
            derive_c_abi_symbol("crypto", "crypto_hash"),
            "_mvl_crypto_hash"
        );
    }

    #[test]
    fn crypto_underscore_prefix() {
        assert_eq!(
            derive_c_abi_symbol("crypto", "_internal"),
            "_mvl_crypto_internal"
        );
    }
}
