// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.log` stdlib functions.
//!
//! Mirrors `mvl_runtime::stdlib::log`. Each function takes a `MvlString*` message
//! and a `MvlMap*` fields map, reconstructs Rust-native types, and delegates to
//! the Rust implementation which handles formatting and stderr output.
//!
//! # Map iteration
//!
//! `MvlMap` uses open-addressing hashing. We iterate all `cap` slots and collect
//! occupied entries. Key and value bytes are valid UTF-8 (MVL strings are always
//! UTF-8); `from_utf8_lossy` guards against any edge cases without panicking.
//!
//! # Ownership
//!
//! The C-ABI functions borrow both pointers — they do not drop or clone the
//! MvlString or MvlMap. The LLVM caller retains ownership and drops as normal.

use std::collections::HashMap;
use std::slice;

use crate::memory::{MvlMap, MvlString};
use mvl_runtime::stdlib::log::{
    log_debug, log_error, log_info, log_set_format, log_warn, LogFormat,
};

// ── helpers ───────────────────────────────────────────────────────────────────

#[allow(unsafe_code)]
unsafe fn read_mvl_string(s: *const MvlString) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = (*s).len as usize;
    if len == 0 {
        return String::new();
    }
    if (*s).ptr.is_null() {
        return String::new();
    }
    let bytes = slice::from_raw_parts((*s).ptr as *const u8, len);
    String::from_utf8_lossy(bytes).into_owned()
}

#[allow(unsafe_code)]
unsafe fn read_mvl_map(m: *const MvlMap) -> HashMap<String, String> {
    let mut result = HashMap::new();
    if m.is_null() {
        return result;
    }
    let cap = (*m).cap as usize;
    // Guard against a corrupt or attacker-influenced cap field.
    if cap > (1 << 24) || (*m).slots.is_null() || (*m).len > (*m).cap {
        return result;
    }
    for i in 0..cap {
        let slot = &*(*m).slots.add(i);
        if slot.occupied == 0 {
            continue;
        }
        // Guard against corrupt key_len before creating a slice.
        if slot.key_len > 1 << 20 {
            continue;
        }
        // Keys are stored as raw UTF-8 bytes (via mvl_string_ptr in codegen).
        let key = String::from_utf8_lossy(slice::from_raw_parts(
            slot.key_ptr as *const u8,
            slot.key_len as usize,
        ))
        .into_owned();
        // Values are stored as a heap copy of the MvlString* pointer (8 bytes).
        // The codegen does build_alloca + build_store of the PointerValue, then
        // passes (alloca_ptr, 8) to mvl_map_insert. So slot.val_ptr points to
        // 8 bytes that contain the address of the MvlString.
        if slot.val_ptr.is_null() {
            result.insert(key, String::new());
            continue;
        }
        let mvl_str_ptr = (slot.val_ptr as *const *const MvlString).read();
        let val = read_mvl_string(mvl_str_ptr);
        result.insert(key, val);
    }
    result
}

// ── C-ABI exports ─────────────────────────────────────────────────────────────

/// Set the output format for all subsequent log calls.
///
/// Enum discriminant: 0 = Plain, 1 = Logfmt, 2 = Json (matches LogFormat declaration order).
/// Unknown discriminants default to Plain.
///
/// # Safety
/// No pointers — always safe to call.
#[no_mangle]
pub extern "C" fn _mvl_log_set_format(fmt: i8) {
    let format = match fmt {
        1 => LogFormat::Logfmt,
        2 => LogFormat::Json,
        _ => LogFormat::Plain,
    };
    log_set_format(format);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{mvl_map_drop, mvl_map_new, mvl_string_drop, mvl_string_new};
    use crate::memory_ops::mvl_map_insert;

    #[test]
    fn read_mvl_string_null_returns_empty() {
        let s = unsafe { read_mvl_string(std::ptr::null()) };
        assert_eq!(s, "");
    }

    #[test]
    fn read_mvl_string_empty_returns_empty() {
        unsafe {
            let ms = mvl_string_new(b"".as_ptr(), 0);
            assert_eq!(read_mvl_string(ms), "");
            mvl_string_drop(ms);
        }
    }

    #[test]
    fn read_mvl_string_roundtrip() {
        unsafe {
            let ms = mvl_string_new(b"hello".as_ptr(), 5);
            assert_eq!(read_mvl_string(ms), "hello");
            mvl_string_drop(ms);
        }
    }

    #[test]
    fn read_mvl_map_null_returns_empty() {
        let m = unsafe { read_mvl_map(std::ptr::null()) };
        assert!(m.is_empty());
    }

    #[test]
    fn set_format_discriminants() {
        use mvl_runtime::stdlib::log::{get_current_format, LogFormat};
        _mvl_log_set_format(0);
        assert_eq!(get_current_format(), LogFormat::Plain);
        _mvl_log_set_format(1);
        assert_eq!(get_current_format(), LogFormat::Logfmt);
        _mvl_log_set_format(2);
        assert_eq!(get_current_format(), LogFormat::Json);
        _mvl_log_set_format(99);
        assert_eq!(get_current_format(), LogFormat::Plain);
    }

    #[test]
    fn read_mvl_map_double_pointer_roundtrip() {
        // Reproduces the LLVM codegen pattern: val_ptr in the map points to 8 bytes
        // that hold the address of a MvlString (i.e. a pointer-to-pointer).
        unsafe {
            let ms = mvl_string_new(b"8080".as_ptr(), 4);
            let ms_addr = ms as usize;
            let val_bytes = ms_addr.to_ne_bytes();
            let m = mvl_map_new(0);
            mvl_map_insert(m, b"port".as_ptr(), 4, val_bytes.as_ptr(), val_bytes.len());
            let result = read_mvl_map(m);
            assert_eq!(result.get("port").map(String::as_str), Some("8080"));
            mvl_map_drop(m);
            mvl_string_drop(ms);
        }
    }
}

/// Emit a DEBUG-level structured log record.
///
/// # Safety
/// `msg` and `fields` must be valid pointers to live `MvlString` / `MvlMap` values
/// for the duration of the call. Null is accepted and treated as empty string / empty map.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_log_debug(msg: *const MvlString, fields: *const MvlMap) {
    log_debug(read_mvl_string(msg), read_mvl_map(fields));
}

/// Emit an INFO-level structured log record.
///
/// # Safety
/// See `_mvl_log_debug`. Null pointers are accepted and treated as empty.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_log_info(msg: *const MvlString, fields: *const MvlMap) {
    log_info(read_mvl_string(msg), read_mvl_map(fields));
}

/// Emit a WARN-level structured log record.
///
/// # Safety
/// See `_mvl_log_debug`. Null pointers are accepted and treated as empty.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_log_warn(msg: *const MvlString, fields: *const MvlMap) {
    log_warn(read_mvl_string(msg), read_mvl_map(fields));
}

/// Emit an ERROR-level structured log record.
///
/// # Safety
/// See `_mvl_log_debug`. Null pointers are accepted and treated as empty.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_log_error(msg: *const MvlString, fields: *const MvlMap) {
    log_error(read_mvl_string(msg), read_mvl_map(fields));
}
