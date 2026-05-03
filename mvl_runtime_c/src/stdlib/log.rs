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

use mvl_memory::{MvlMap, MvlString};
use mvl_runtime::stdlib::log::{log_debug, log_error, log_info, log_warn};

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
    for i in 0..cap {
        let slot = &*(*m).slots.add(i);
        if slot.occupied == 0 {
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
        let mvl_str_ptr = (slot.val_ptr as *const *const MvlString).read();
        let val = read_mvl_string(mvl_str_ptr);
        result.insert(key, val);
    }
    result
}

// ── C-ABI exports ─────────────────────────────────────────────────────────────

/// Emit a DEBUG-level structured log record.
///
/// # Safety
/// `msg` and `fields` must be valid non-null pointers to live `MvlString` / `MvlMap`
/// values for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_log_debug(msg: *const MvlString, fields: *const MvlMap) {
    log_debug(read_mvl_string(msg), read_mvl_map(fields));
}

/// Emit an INFO-level structured log record.
///
/// # Safety
/// See `_mvl_log_debug`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_log_info(msg: *const MvlString, fields: *const MvlMap) {
    log_info(read_mvl_string(msg), read_mvl_map(fields));
}

/// Emit a WARN-level structured log record.
///
/// # Safety
/// See `_mvl_log_debug`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_log_warn(msg: *const MvlString, fields: *const MvlMap) {
    log_warn(read_mvl_string(msg), read_mvl_map(fields));
}

/// Emit an ERROR-level structured log record.
///
/// # Safety
/// See `_mvl_log_debug`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_log_error(msg: *const MvlString, fields: *const MvlMap) {
    log_error(read_mvl_string(msg), read_mvl_map(fields));
}
